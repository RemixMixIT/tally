//! CSV importer.
//!
//! Two-step flow:
//!   1. POST /api/csv/preview  — upload raw CSV bytes; server detects columns, returns suggested mapping
//!   2. POST /api/csv/commit   — apply mapping + account selection, batch-insert transactions
//!
//! Dedup: `provider_txn_id` is set to `csv:<sha256-of-row>` so re-uploads are idempotent.
//! Parser is RFC 4180 (quoted fields, doubled quotes, commas inside quotes are not delimiters).

use crate::AppState;
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
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub async fn commit(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CommitBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if state
        .db
        .get_account(body.account_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?
        .is_none()
    {
        return Err((StatusCode::BAD_REQUEST, "unknown account".into()));
    }

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

    if amount_i.is_none() && credit_i.is_none() && debit_i.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "either amount_column OR (credit_column + debit_column) required".into(),
        ));
    }

    let currency = body.currency.unwrap_or_else(|| "GBP".to_string());
    let mut imported = 0i64;
    let mut skipped = 0i64;
    let mut errors: Vec<String> = Vec::new();

    for (row_n, rec) in rows.iter().enumerate() {
        let cell = |i: usize| rec.get(i).map(|s| s.as_str()).unwrap_or("");
        let raw_row: String = rec.join("\u{1f}");
        let mut h = Sha256::new();
        h.update(raw_row.as_bytes());
        let hash = format!("csv:{}:{:x}", body.account_id, h.finalize());

        let date_str = cell(date_i).trim();
        let desc = cell(desc_i).trim().to_string();
        let ts = parse_date(date_str, nonempty(&body.mapping.date_format));
        let Some(ts) = ts else {
            errors.push(format!("row {row_n}: couldn't parse date '{date_str}'"));
            continue;
        };

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

        if state.db.is_txn_seen(body.account_id, &hash).await.unwrap_or(false) {
            skipped += 1;
            continue;
        }

        let new_id = match state
            .db
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
                None,
                None,
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
        let _ = state
            .db
            .record_txn_imported(body.account_id, &hash, Some(new_id), false, None)
            .await;
        imported += 1;
    }

    Ok(Json(json!({
        "imported": imported,
        "skipped": skipped,
        "errors": errors,
    })))
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
}
