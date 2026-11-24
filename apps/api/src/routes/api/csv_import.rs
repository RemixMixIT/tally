//! CSV importer.
//!
//! Two-step flow:
//!   1. POST /api/csv/preview  — upload raw CSV bytes; server detects columns, returns suggested mapping
//!   2. POST /api/csv/commit   — apply mapping + account selection, batch-insert transactions
//!
//! Dedup (account-scoped): skip a row if this account already has a transaction with the same
//! calendar date + amount + description. The row-hash `provider_txn_id` (`csv:<account>:<sha256>`)
//! is an additional fast path for exact re-uploads, but overlapping exports often differ in a
//! Balance column so the composite key is the source of truth.
//!
//! Offline accounts: if a Balance column is mapped, `current_balance_cents` is set from the
//! chronologically latest row. Linked TrueLayer accounts are left untouched.
//! Parser is RFC 4180 (quoted fields, doubled quotes, commas inside quotes are not delimiters).

use crate::AppState;
use crate::learn::LearnEngine;
use crate::models::Transaction;
use crate::rules::{self, CompiledRules};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::NaiveDate;
use csv::ReaderBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct PreviewBody {
    /// Raw CSV text. Frontend reads the file as a string and posts it here.
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct PreviewReply {
    pub headers: Vec<String>,
    /// Up to 5 sample rows for the UI to render a mapping preview.
    pub sample_rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub suggested: SuggestedMapping,
}

#[derive(Debug, Serialize, Default, PartialEq, Eq)]
pub struct SuggestedMapping {
    pub date_column: Option<String>,
    pub description_column: Option<String>,
    pub amount_column: Option<String>,
    pub credit_column: Option<String>,
    pub debit_column: Option<String>,
    pub balance_column: Option<String>,
}

/// RFC 4180 reader: commas inside double-quoted fields are not delimiters; `""` is an escaped quote.
fn csv_reader(content: &str) -> csv::Reader<&[u8]> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .quoting(true)
        .double_quote(true)
        .from_reader(content.as_bytes())
}

fn read_table(content: &str) -> Result<(Vec<String>, Vec<Vec<String>>), (StatusCode, String)> {
    let mut rdr = csv_reader(content);
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad CSV headers: {e}")))?
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut rows = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| (StatusCode::BAD_REQUEST, format!("bad CSV row {i}: {e}")))?;
        rows.push(rec.iter().map(|s| s.to_string()).collect());
    }
    Ok((headers, rows))
}

pub async fn preview(Json(body): Json<PreviewBody>) -> Result<Json<PreviewReply>, (StatusCode, String)> {
    let (headers, rows) = read_table(&body.content)?;
    let sample_rows = rows.iter().take(5).cloned().collect();
    let suggested = suggest_mapping(&headers);
    Ok(Json(PreviewReply {
        headers,
        sample_rows,
        row_count: rows.len(),
        suggested,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CommitBody {
    pub content: String,
    pub account_id: i64,
    pub mapping: CommitMapping,
    pub currency: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommitMapping {
    pub date_column: String,
    pub description_column: String,
    /// Either a single amount column (positive = credit, negative = debit)
    /// OR separate credit + debit columns.
    pub amount_column: Option<String>,
    pub credit_column: Option<String>,
    pub debit_column: Option<String>,
    /// Date format string (chrono syntax), e.g. "%d/%m/%Y" for Nationwide.
    /// Defaults try ISO 8601 (`%Y-%m-%d`), then DD/MM/YYYY, then MM/DD/YYYY.
    pub date_format: Option<String>,
    /// Optional running-balance column (e.g. Smile's "Balance"). Used to snapshot
    /// the offline account's current balance from the latest imported row.
    #[serde(default)]
    pub balance_column: Option<String>,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub async fn commit(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CommitBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (imported, skipped, errors) = commit_csv(&state.db, &body).await?;
    Ok(Json(json!({
        "imported": imported,
        "skipped": skipped,
        "errors": errors,
    })))
}

async fn commit_csv(
    db: &crate::db::Db,
    body: &CommitBody,
) -> Result<(i64, i64, Vec<String>), (StatusCode, String)> {
    let account = db
        .get_account(body.account_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?
        .ok_or((StatusCode::BAD_REQUEST, "unknown account".into()))?;

    let (headers, rows) = read_table(&body.content)?;

    let idx = |name: &str| -> Result<usize, (StatusCode, String)> {
        headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
            .ok_or((StatusCode::BAD_REQUEST, format!("column '{name}' not in CSV")))
    };
    let date_i = idx(&body.mapping.date_column)?;
    let desc_i = idx(&body.mapping.description_column)?;
    let amount_i = nonempty(&body.mapping.amount_column).map(idx).transpose()?;
    let credit_i = nonempty(&body.mapping.credit_column).map(idx).transpose()?;
    let debit_i = nonempty(&body.mapping.debit_column).map(idx).transpose()?;
    let balance_i = nonempty(&body.mapping.balance_column).map(idx).transpose()?;

    if amount_i.is_none() && credit_i.is_none() && debit_i.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "either amount_column OR (credit_column + debit_column) required".into(),
        ));
    }

    let date_fmt = nonempty(&body.mapping.date_format);
    let newest_first = csv_is_newest_first(&rows, date_i, date_fmt);
    let currency = body.currency.clone().unwrap_or_else(|| "GBP".to_string());
    let mut imported = 0i64;
    let mut skipped = 0i64;
    let mut errors: Vec<String> = Vec::new();
    // (timestamp, row_index, balance_cents) of the chronologically latest row with a balance.
    let mut latest_balance: Option<(i64, usize, i64)> = None;

    let rules_raw = db.list_rules(true).await.unwrap_or_default();
    let compiled_rules = CompiledRules::compile(rules_raw);
    let mut learned = LearnEngine::from_db(db)
        .await
        .unwrap_or_else(|_| LearnEngine::from_parts(Vec::new(), &[]));
    let bills = db.list_bills().await.unwrap_or_default();
    let bill_matchers = crate::bill_match::compile(&bills);

    for (row_n, rec) in rows.iter().enumerate() {
        let cell = |i: usize| rec.get(i).map(|s| s.as_str()).unwrap_or("");
        let raw_row: String = rec.join("\u{1f}");
        let mut h = Sha256::new();
        h.update(raw_row.as_bytes());
        let hash = format!("csv:{}:{:x}", body.account_id, h.finalize());

        let date_str = cell(date_i).trim();
        let desc = cell(desc_i).trim().to_string();
        let ts = parse_date(date_str, date_fmt);
        let Some(ts) = ts else {
            errors.push(format!("row {row_n}: couldn't parse date '{date_str}'"));
            continue;
        };

        if let Some(bi) = balance_i {
            let raw_bal = cell(bi);
            if !raw_bal.trim().is_empty() {
                let cents = (parse_money(raw_bal) * 100.0).round() as i64;
                let take = match latest_balance {
                    None => true,
                    Some((best_ts, best_n, _)) => {
                        prefer_balance_row(ts, row_n, best_ts, best_n, newest_first)
                    }
                };
                if take {
                    latest_balance = Some((ts, row_n, cents));
                }
            }
        }

        // Prefer split Credit/Debit when those columns are mapped — even if an amount
        // column was also selected — so Smile-style Money In / Money Out CSVs work.
        let amount = if credit_i.is_some() || debit_i.is_some() {
            let c = credit_i.map(cell).unwrap_or("");
            let d = debit_i.map(cell).unwrap_or("");
            credit_debit_amount(c, d)
        } else if let Some(ai) = amount_i {
            Some(signed_amount(cell(ai)))
        } else {
            None
        };
        let Some((is_credit, amount_cents)) = amount else {
            skipped += 1;
            continue;
        };

        if db.is_txn_seen(body.account_id, &hash).await.unwrap_or(false) {
            skipped += 1;
            continue;
        }

        if let Ok(Some(existing_id)) = db
            .find_matching_transaction(body.account_id, ts, &desc, amount_cents, is_credit)
            .await
        {
            let _ = db
                .record_txn_imported(body.account_id, &hash, Some(existing_id), false, None)
                .await;
            skipped += 1;
            continue;
        }

        let pseudo = Transaction {
            id: 0,
            account_id: body.account_id,
            provider_txn_id: hash.clone(),
            timestamp: ts,
            description: desc.clone(),
            amount_cents,
            currency: currency.clone(),
            is_credit: if is_credit { 1 } else { 0 },
            is_pending: 0,
            merchant_name: None,
            counterparty_iban: None,
            counterparty_name: None,
            category_id: None,
            notes: None,
            raw_json: None,
            transfer_pair_id: None,
            transfer_unmatchable: 0,
            transfer_await_account_id: None,
            claim_flags: Vec::new(),
            created_at: 0,
            updated_at: 0,
        };
        let (category_id, _) = learned.category_for_new(&pseudo, &compiled_rules);
        let eff = rules::apply(&pseudo, &compiled_rules);

        let new_id = match db
            .upsert_transaction(
                body.account_id,
                &hash,
                ts,
                &desc,
                amount_cents,
                &currency,
                is_credit,
                false,
                None,
                None,
                None,
                category_id,
                eff.set_notes.as_deref(),
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                errors.push(format!("row {row_n}: db error {e:#}"));
                continue;
            }
        };
        if let Some(cid) = category_id {
            learned.observe(None, &desc, cid);
        }
        if let Err(e) = crate::pairs::on_new_transaction(db, new_id).await {
            tracing::warn!("pair/await for csv txn {new_id}: {e:#}");
        }
        for tid in &eff.add_tag_ids {
            let _ = db.tag_transaction(new_id, *tid).await;
        }
        for rid in &eff.rules_fired {
            let _ = db.bump_rule_applied(*rid).await;
        }
        let _ = db
            .record_txn_imported(body.account_id, &hash, Some(new_id), false, None)
            .await;
        let _ = db
            .auto_link_bill_if_matched(
                &bill_matchers,
                body.account_id,
                new_id,
                &desc,
                None,
                amount_cents,
                is_credit,
                ts,
            )
            .await;
        imported += 1;
    }

    // Snapshot the latest CSV running balance onto manual/offline accounts only.
    if account.consent_id.is_none() {
        if let Some((_, _, cents)) = latest_balance {
            if let Err(e) = db
                .update_account_balance(account.id, cents, Some(cents), None)
                .await
            {
                errors.push(format!("could not update account balance: {e:#}"));
            }
        }
    }

    tracing::info!(
        account_id = body.account_id,
        imported,
        skipped,
        "csv import finished"
    );

    Ok((imported, skipped, errors))
}

/// True when the file's dates go newest → oldest (typical UK bank export).
fn csv_is_newest_first(rows: &[Vec<String>], date_i: usize, date_fmt: Option<&str>) -> bool {
    let mut prev: Option<i64> = None;
    for rec in rows {
        let date_str = rec.get(date_i).map(|s| s.as_str()).unwrap_or("");
        let Some(ts) = parse_date(date_str, date_fmt) else {
            continue;
        };
        if let Some(p) = prev {
            if ts != p {
                return ts < p;
            }
        }
        prev = Some(ts);
    }
    true
}

fn prefer_balance_row(
    candidate_ts: i64,
    candidate_row: usize,
    best_ts: i64,
    best_row: usize,
    newest_first: bool,
) -> bool {
    if candidate_ts > best_ts {
        return true;
    }
    if candidate_ts < best_ts {
        return false;
    }
    if newest_first {
        candidate_row < best_row
    } else {
        candidate_row > best_row
    }
}

/// Empty / whitespace / unparseable money strings are treated as zero.
fn parse_money(s: &str) -> f64 {
    let t = s.trim().replace(",", "").replace("£", "");
    if t.is_empty() {
        0.0
    } else {
        t.parse().unwrap_or(0.0)
    }
}

/// Split Credit (Money In) / Debit (Money Out): credit → positive, debit → negative.
/// Both empty (zero) → None so the row can be skipped.
fn credit_debit_amount(credit: &str, debit: &str) -> Option<(bool, i64)> {
    let c = parse_money(credit);
    let d = parse_money(debit);
    if c != 0.0 {
        Some((true, (c.abs() * 100.0).round() as i64))
    } else if d != 0.0 {
        Some((false, (d.abs() * 100.0).round() as i64))
    } else {
        None
    }
}

fn signed_amount(raw: &str) -> (bool, i64) {
    let n = parse_money(raw);
    (n >= 0.0, (n.abs() * 100.0).round() as i64)
}

fn parse_date(s: &str, explicit_fmt: Option<&str>) -> Option<i64> {
    let s = s.trim();
    if let Some(fmt) = explicit_fmt {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    for fmt in &[
        "%Y-%m-%d",
        "%d/%m/%Y",
        "%m/%d/%Y",
        "%d-%m-%Y",
        "%d %b %Y",
        "%d %B %Y",
        "%Y/%m/%d",
    ] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    // Try RFC3339 with time
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp());
    }
    None
}

fn suggest_mapping(headers: &[String]) -> SuggestedMapping {
    let find = |patterns: &[&str]| -> Option<String> {
        headers
            .iter()
            .find(|h| {
                let lower = h.to_lowercase();
                patterns.iter().any(|p| lower.contains(p))
            })
            .cloned()
    };
    let credit_column = find(&["credit", "money in", "paid in", "deposit"]);
    let debit_column = find(&["debit", "money out", "paid out", "withdrawal"]);
    // When the file is split Credit/Debit, don't also suggest a single Amount column —
    // that would take precedence in older importers and drop the sign.
    let amount_column = if credit_column.is_some() || debit_column.is_some() {
        None
    } else {
        find(&["amount", "value"])
    };
    SuggestedMapping {
        date_column: find(&["date", "transaction date"]),
        description_column: find(&["description", "narrative", "details", "transaction"]),
        amount_column,
        credit_column,
        debit_column,
        balance_column: find(&["balance"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMILE: &str = r#""Date","Description","Type","Money In","Money Out","Balance"
"2026-08-27","John Lancaster","CREDIT","2500.00","","3005.32"
"2026-08-24","KCOM GROUP LTD","DD","","40.99","555.51"
"2026-08-20","TESCO, HULL","POS","","12.50","543.01"
"#;

    #[test]
    fn quoted_fields_and_embedded_commas() {
        let (headers, rows) = read_table(SMILE).unwrap();
        assert_eq!(
            headers,
            vec!["Date", "Description", "Type", "Money In", "Money Out", "Balance"]
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][1], "John Lancaster");
        assert_eq!(rows[0][3], "2500.00");
        assert_eq!(rows[0][4], "");
        assert_eq!(rows[1][1], "KCOM GROUP LTD");
        assert_eq!(rows[1][3], "");
        assert_eq!(rows[1][4], "40.99");
        // Comma inside quotes is not a delimiter.
        assert_eq!(rows[2][1], "TESCO, HULL");
        assert_eq!(rows[2].len(), 6);
    }

    #[test]
    fn smile_column_suggestions() {
        let (headers, _) = read_table(SMILE).unwrap();
        let s = suggest_mapping(&headers);
        assert_eq!(s.date_column.as_deref(), Some("Date"));
        assert_eq!(s.description_column.as_deref(), Some("Description"));
        assert_eq!(s.credit_column.as_deref(), Some("Money In"));
        assert_eq!(s.debit_column.as_deref(), Some("Money Out"));
        assert_eq!(s.amount_column, None, "split credit/debit must not also suggest Amount");
        assert_eq!(s.balance_column.as_deref(), Some("Balance"));
    }

    #[test]
    fn empty_credit_debit_is_zero_and_signs() {
        // Credit filled, debit empty → inbound.
        assert_eq!(credit_debit_amount("2500.00", ""), Some((true, 250_000)));
        assert_eq!(credit_debit_amount("2500.00", "   "), Some((true, 250_000)));
        // Debit filled, credit empty → outbound (is_credit = false).
        assert_eq!(credit_debit_amount("", "40.99"), Some((false, 4099)));
        assert_eq!(credit_debit_amount("", ""), None);
        assert_eq!(parse_money(""), 0.0);
        assert_eq!(parse_money("  "), 0.0);
    }

    #[test]
    fn iso_date_ymd() {
        let ts = parse_date("2026-08-27", None).unwrap();
        let d = chrono::DateTime::from_timestamp(ts, 0).unwrap().date_naive();
        assert_eq!(d.to_string(), "2026-08-27");
        assert_eq!(
            parse_date("2026-08-27", Some("%Y-%m-%d")).unwrap(),
            ts
        );
    }

    #[test]
    fn smile_rows_map_to_signed_amounts() {
        let (headers, rows) = read_table(SMILE).unwrap();
        let s = suggest_mapping(&headers);
        let date_i = headers.iter().position(|h| h == s.date_column.as_ref().unwrap()).unwrap();
        let desc_i = headers.iter().position(|h| h == s.description_column.as_ref().unwrap()).unwrap();
        let credit_i = headers.iter().position(|h| h == s.credit_column.as_ref().unwrap()).unwrap();
        let debit_i = headers.iter().position(|h| h == s.debit_column.as_ref().unwrap()).unwrap();

        let mapped: Vec<_> = rows
            .iter()
            .map(|r| {
                let ts = parse_date(&r[date_i], None).unwrap();
                let (is_credit, cents) = credit_debit_amount(&r[credit_i], &r[debit_i]).unwrap();
                (ts, r[desc_i].clone(), is_credit, cents)
            })
            .collect();

        assert_eq!(mapped[0].1, "John Lancaster");
        assert!(mapped[0].2);
        assert_eq!(mapped[0].3, 250_000);
        assert_eq!(mapped[1].1, "KCOM GROUP LTD");
        assert!(!mapped[1].2);
        assert_eq!(mapped[1].3, 4099);
        assert_eq!(mapped[2].1, "TESCO, HULL");
        assert!(!mapped[2].2);
        assert_eq!(mapped[2].3, 1250);
    }

    fn smile_mapping() -> CommitMapping {
        CommitMapping {
            date_column: "Date".into(),
            description_column: "Description".into(),
            amount_column: None,
            credit_column: Some("Money In".into()),
            debit_column: Some("Money Out".into()),
            date_format: Some("%Y-%m-%d".into()),
            balance_column: Some("Balance".into()),
        }
    }

    async fn test_db() -> crate::db::Db {
        use crate::crypto::Crypto;
        use base64::engine::general_purpose::STANDARD as B64STD;
        use base64::Engine;
        let crypto = Crypto::from_b64(&B64STD.encode([9u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        crate::db::Db {
            conn: std::sync::Arc::new(conn),
            crypto,
        }
    }

    #[tokio::test]
    async fn csv_import_matches_regular_in_window() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Smile", "current", None)
            .await
            .unwrap();
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 8, 24)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let bill_id = db
            .create_bill(
                "KCOM",
                3894,
                4304,
                "GBP",
                "monthly",
                Some(expected),
                Some("KCOM"),
                None,
                Some(acct.id),
                false,
            )
            .await
            .unwrap();
        let body = CommitBody {
            content: SMILE.to_string(),
            account_id: acct.id,
            mapping: smile_mapping(),
            currency: None,
        };
        let (imported, _, errors) = commit_csv(&db, &body).await.unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(imported, 3);
        let txns = db
            .list_transactions(Some(&[acct.id]), None, None, None, None, None, None, Some("KCOM"), 10, 0)
            .await
            .unwrap();
        assert!(db.bill_payment_exists(bill_id, txns[0].id).await.unwrap());
        let bill = db.get_bill(bill_id).await.unwrap().unwrap();
        assert!(bill.next_expected_date.unwrap() > expected);
        assert_eq!(bill.expected_amount_min_cents, 3894);
        assert_eq!(bill.expected_amount_max_cents, 4304);
    }

    #[tokio::test]
    async fn csv_out_of_window_amount_does_not_auto_link() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Smile", "current", None)
            .await
            .unwrap();
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 8, 24)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let bill_id = db
            .create_bill(
                "KCOM",
                1000,
                1100,
                "GBP",
                "monthly",
                Some(expected),
                Some("KCOM"),
                None,
                Some(acct.id),
                false,
            )
            .await
            .unwrap();
        let body = CommitBody {
            content: SMILE.to_string(),
            account_id: acct.id,
            mapping: smile_mapping(),
            currency: None,
        };
        commit_csv(&db, &body).await.unwrap();
        let txns = db
            .list_transactions(Some(&[acct.id]), None, None, None, None, None, None, Some("KCOM"), 10, 0)
            .await
            .unwrap();
        assert!(!db.bill_payment_exists(bill_id, txns[0].id).await.unwrap());
        // Confirm this-payment-only: links, leaves window, rolls date.
        db.mark_bill_paid_by_transaction(bill_id, txns[0].id, txns[0].timestamp, false, txns[0].amount_cents)
            .await
            .unwrap();
        let after = db.get_bill(bill_id).await.unwrap().unwrap();
        assert_eq!(after.expected_amount_min_cents, 1000);
        assert_eq!(after.expected_amount_max_cents, 1100);
        assert!(after.next_expected_date.unwrap() > expected);

        let bill2 = db
            .create_bill(
                "TESCO shop",
                1000,
                1100,
                "GBP",
                "monthly",
                Some(
                    chrono::NaiveDate::from_ymd_opt(2026, 8, 20)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .timestamp(),
                ),
                Some("TESCO"),
                None,
                Some(acct.id),
                false,
            )
            .await
            .unwrap();
        let tesco = db
            .list_transactions(Some(&[acct.id]), None, None, None, None, None, None, Some("TESCO"), 10, 0)
            .await
            .unwrap();
        db.mark_bill_paid_by_transaction(bill2, tesco[0].id, tesco[0].timestamp, true, tesco[0].amount_cents)
            .await
            .unwrap();
        let updated = db.get_bill(bill2).await.unwrap().unwrap();
        let (min, max) = crate::bill_match::window_from_amount(tesco[0].amount_cents);
        assert_eq!(updated.expected_amount_min_cents, min);
        assert_eq!(updated.expected_amount_max_cents, max);
    }

    #[tokio::test]
    async fn smile_import_sets_balance_and_skips_overlapping_reimport() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Smile", "current", None)
            .await
            .unwrap();
        let body = CommitBody {
            content: SMILE.to_string(),
            account_id: acct.id,
            mapping: smile_mapping(),
            currency: None,
        };
        let (imported, skipped, errors) = commit_csv(&db, &body).await.unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(imported, 3);
        assert_eq!(skipped, 0);
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.current_balance_cents, Some(300_532));

        // Same file again — hash + composite key both skip.
        let (imported2, skipped2, errors2) = commit_csv(&db, &body).await.unwrap();
        assert!(errors2.is_empty(), "{errors2:?}");
        assert_eq!(imported2, 0);
        assert_eq!(skipped2, 3);
    }

    #[tokio::test]
    async fn overlapping_csv_with_changed_balance_is_semantic_dedup() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Smile", "current", None)
            .await
            .unwrap();
        let first = CommitBody {
            content: SMILE.to_string(),
            account_id: acct.id,
            mapping: smile_mapping(),
            currency: None,
        };
        commit_csv(&db, &first).await.unwrap();

        // Overlapping export: same date/amount/description, different Balance (so row hash differs).
        let overlapping = r#""Date","Description","Type","Money In","Money Out","Balance"
"2026-08-28","NEW SHOP","POS","","3.00","2999.99"
"2026-08-27","John Lancaster","CREDIT","2500.00","","3002.99"
"2026-08-24","KCOM GROUP LTD","DD","","40.99","502.99"
"#;
        let second = CommitBody {
            content: overlapping.to_string(),
            account_id: acct.id,
            mapping: smile_mapping(),
            currency: None,
        };
        let (imported, skipped, errors) = commit_csv(&db, &second).await.unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(imported, 1, "only the new 28 Aug row should insert");
        assert_eq!(skipped, 2);
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        // Latest date in the overlapping file is 28 Aug with running balance 2999.99.
        assert_eq!(after.current_balance_cents, Some(299_999));
    }

    #[tokio::test]
    async fn same_txn_on_two_accounts_is_not_a_duplicate() {
        let db = test_db().await;
        let a1 = db.create_manual_account("Smile", "current", None).await.unwrap();
        let a2 = db.create_manual_account("Vanquis", "credit_card", None).await.unwrap();
        let m = smile_mapping();
        let (i1, _, e1) = commit_csv(
            &db,
            &CommitBody {
                content: SMILE.to_string(),
                account_id: a1.id,
                mapping: smile_mapping(),
                currency: None,
            },
        )
        .await
        .unwrap();
        let (i2, s2, e2) = commit_csv(
            &db,
            &CommitBody {
                content: SMILE.to_string(),
                account_id: a2.id,
                mapping: m,
                currency: None,
            },
        )
        .await
        .unwrap();
        assert!(e1.is_empty() && e2.is_empty());
        assert_eq!(i1, 3);
        assert_eq!(i2, 3);
        assert_eq!(s2, 0);
    }

    #[tokio::test]
    async fn csv_without_balance_does_not_clear_opening_balance() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Vanquis", "credit_card", Some(50_000))
            .await
            .unwrap();
        let csv = r#""Date","Description","Amount"
"2026-08-01","PAYMENT","-10.00"
"#;
        let body = CommitBody {
            content: csv.to_string(),
            account_id: acct.id,
            mapping: CommitMapping {
                date_column: "Date".into(),
                description_column: "Description".into(),
                amount_column: Some("Amount".into()),
                credit_column: None,
                debit_column: None,
                date_format: Some("%Y-%m-%d".into()),
                balance_column: None,
            },
            currency: None,
        };
        let (imported, _, errors) = commit_csv(&db, &body).await.unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(imported, 1);
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.current_balance_cents, Some(50_000));
    }

    #[tokio::test]
    async fn linked_account_balance_is_not_overwritten_by_csv() {
        let db = test_db().await;
        let consent = db
            .upsert_consent("bank", "creds", None, None, "at", "rt", 1, None, "accounts")
            .await
            .unwrap();
        let acct = db
            .upsert_account(consent.id, "acc-1", "account", "Current", None, None, None, None, "GBP")
            .await
            .unwrap();
        db.update_account_balance(acct.id, 99_000, Some(99_000), None)
            .await
            .unwrap();
        let body = CommitBody {
            content: SMILE.to_string(),
            account_id: acct.id,
            mapping: smile_mapping(),
            currency: None,
        };
        commit_csv(&db, &body).await.unwrap();
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.current_balance_cents, Some(99_000));
    }
}
