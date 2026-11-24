//! Vanquis monthly-statement PDF importer.
//!
//! Host rclone copies new PDFs from Dropbox
//! (`dropbox:!Inbox/Documents/Vanquis Statements`, including `Inbox/`) onto
//! `/app/inbound/vanquis/inbox`. After a successful import (including dedup
//! no-op), the host script files the PDF onto OneDrive as
//! `YYYYMM -MonthlyStatement.pdf` under
//! `onedrive:Documents/Financial/Vanquis Statements` and removes the Dropbox
//! copy. The inbound Docker mount stays read-only. Statement lines dated before
//! 2025 are not stored.
//!
//! Polarity: SPEND (purchases/fees/interest) → `is_credit = false`. REFUNDS AND
//! ADJUSTMENTS and PAYMENTS RECEIVED → `is_credit = true`. Amounts in GBP pence.

use crate::db::Db;
use crate::learn::LearnEngine;
use crate::models::Transaction;
use crate::rules::{self, CompiledRules};
use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tracing::{info, warn};

pub const DEFAULT_DIR: &str = "/app/inbound/vanquis";
pub const DEFAULT_ACCOUNT_ID: i64 = 29;
/// Statement lines dated before this year stay out of Tally.
pub const KEEP_FROM_YEAR: i32 = 2025;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTxn {
    pub date: NaiveDate,
    pub description: String,
    pub amount_cents: i64,
    pub is_credit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStatement {
    pub statement_date: NaiveDate,
    pub statement_balance_cents: i64,
    pub transactions: Vec<ParsedTxn>,
}

impl ParsedStatement {
    pub fn period_yyyymm(&self) -> String {
        self.statement_date.format("%Y%m").to_string()
    }

    pub fn archive_name(&self) -> String {
        format!("{} -MonthlyStatement.pdf", self.period_yyyymm())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub source: String,
    pub original_name: String,
    pub period: Option<String>,
    pub archive_name: Option<String>,
    pub statement_date: Option<String>,
    pub statement_balance_cents: Option<i64>,
    pub imported: i64,
    pub skipped: i64,
    pub errors: Vec<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ImportReport {
    pub account_id: i64,
    pub files: Vec<FileReport>,
    pub files_ok: i64,
    pub files_failed: i64,
    pub transactions_imported: i64,
    pub transactions_skipped: i64,
}

pub fn inbound_dir() -> PathBuf {
    std::env::var("TALLY_VANQUIS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DIR))
}

pub fn account_id() -> i64 {
    std::env::var("TALLY_VANQUIS_ACCOUNT_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ACCOUNT_ID)
}

fn txn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(\d{2}/\d{2}/\d{2})\s+(\S.*\S)\s+([\d,]+\.\d{2})\s*$").unwrap())
}

fn stmt_date_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)Statement Date:\s+(\d{1,2}\s+(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{4})",
        )
        .unwrap()
    })
}

fn bal_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)This month's balance\s+£([\d,]+\.\d{2})").unwrap())
}

fn fx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b[A-Z]{3}\s+@\s+\d").unwrap())
}

fn money_cents(raw: &str) -> Option<i64> {
    let t = raw.trim().replace(",", "").replace("£", "");
    if t.is_empty() {
        return None;
    }
    let n: f64 = t.parse().ok()?;
    Some((n.abs() * 100.0).round() as i64)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn header_kind(line: &str) -> Option<&'static str> {
    let t = collapse_ws(line).to_ascii_uppercase();
    match t.as_str() {
        "SPEND" => Some("spend"),
        "REFUNDS AND ADJUSTMENTS" => Some("credit"),
        "PAYMENTS RECEIVED" => Some("credit"),
        _ if t.starts_with("STATEMENT BALANCE") => Some("stop"),
        _ if t.starts_with("USEFUL INFORMATION") => Some("stop"),
        _ if t.starts_with("YOUR INTEREST RATES") => Some("stop"),
        _ if t.starts_with("WAYS TO MAKE A PAYMENT") => Some("stop"),
        _ if t.starts_with("HOW TO MAKE A PAYMENT") => Some("stop"),
        _ if t.starts_with("MINIMUM PAYMENTS") => Some("stop"),
        _ => None,
    }
}

/// Parse pdftotext -layout output (also works on pypdf-style collapsed text).
pub fn parse_text(text: &str) -> Result<ParsedStatement, String> {
    let mut statement_date = None;
    let mut balance = None;
    let mut polarity: Option<bool> = None; // Some(true) = credit
    let mut transactions: Vec<ParsedTxn> = Vec::new();

    for raw in text.lines() {
        if statement_date.is_none() {
            if let Some(c) = stmt_date_re().captures(raw) {
                let d = NaiveDate::parse_from_str(c[1].trim(), "%d %B %Y")
                    .map_err(|e| format!("statement date '{}': {e}", &c[1]))?;
                statement_date = Some(d);
            }
        }
        if balance.is_none() {
            if let Some(c) = bal_re().captures(raw) {
                balance = money_cents(&c[1]);
            }
        }

        match header_kind(raw) {
            Some("stop") => {
                if !transactions.is_empty() {
                    break;
                }
            }
            Some("spend") => {
                polarity = Some(false);
                continue;
            }
            Some("credit") => {
                polarity = Some(true);
                continue;
            }
            _ => {}
        }

        if fx_re().is_match(raw) && txn_re().captures(raw).is_none() {
            continue;
        }
        let Some(c) = txn_re().captures(raw) else {
            continue;
        };
        let date = NaiveDate::parse_from_str(&c[1], "%d/%m/%y")
            .map_err(|e| format!("txn date '{}': {e}", &c[1]))?;
        let description = collapse_ws(&c[2]);
        let Some(amount_cents) = money_cents(&c[3]) else {
            continue;
        };
        if amount_cents == 0 {
            continue;
        }
        let is_credit = polarity.unwrap_or(false)
            || description.eq_ignore_ascii_case("DIRECT DEBIT PAYMENT")
            || description.to_ascii_uppercase().contains("FEE CREDIT")
            || description.to_ascii_uppercase().contains("REFUND");
        let txn = ParsedTxn {
            date,
            description,
            amount_cents,
            is_credit,
        };
        if transactions.last() == Some(&txn) {
            continue;
        }
        transactions.push(txn);
    }

    let statement_date = statement_date.ok_or_else(|| "no Statement Date on PDF".to_string())?;
    let statement_balance_cents =
        balance.ok_or_else(|| "no This month's balance on PDF".to_string())?;
    if transactions.is_empty() {
        return Err("no transactions found on PDF".into());
    }
    Ok(ParsedStatement {
        statement_date,
        statement_balance_cents,
        transactions,
    })
}

pub fn extract_pdf_text(path: &Path) -> Result<String, String> {
    let out = Command::new("pdftotext")
        .args(["-layout", "-nopgbrk"])
        .arg(path)
        .arg("-")
        .output()
        .map_err(|e| format!("pdftotext missing or failed to spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pdftotext {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("pdftotext utf8: {e}"))
}

fn source_of(dir: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(dir).unwrap_or(path);
    let first = rel
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");
    if first.eq_ignore_ascii_case("inbox") {
        "inbox".into()
    } else {
        "archive".into()
    }
}

fn collect_pdfs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for sub in ["inbox", "archive"] {
        walk_pdfs(&dir.join(sub), &mut out);
    }
    out.sort();
    out
}

fn walk_pdfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_pdfs(&p, out);
        } else if p
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("pdf"))
        {
            out.push(p);
        }
    }
}

fn provider_id(account_id: i64, t: &ParsedTxn) -> String {
    let mut h = Sha256::new();
    h.update(format!(
        "{}|{}|{}|{}",
        t.date.format("%Y-%m-%d"),
        t.amount_cents,
        if t.is_credit { 1 } else { 0 },
        t.description.to_ascii_lowercase()
    ));
    format!("vanquis-pdf:{account_id}:{:x}", h.finalize())
}

async fn resolve_account(db: &Db, id: i64) -> Result<crate::models::Account, String> {
    let acct = db
        .get_account(id)
        .await
        .map_err(|e| format!("{e}"))?
        .ok_or_else(|| format!("Vanquis account id {id} not found"))?;
    let name = acct
        .custom_display_name
        .as_deref()
        .unwrap_or(&acct.display_name);
    if !name.to_ascii_lowercase().contains("vanquis") {
        return Err(format!(
            "account {id} is '{name}', not Vanquis — refusing to import"
        ));
    }
    Ok(acct)
}

pub async fn import_parsed(
    db: &Db,
    account_id: i64,
    parsed: &ParsedStatement,
) -> Result<(i64, i64, Vec<String>), String> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    let mut errors: Vec<String> = Vec::new();

    let rules_raw = db.list_rules(true).await.unwrap_or_default();
    let compiled_rules = CompiledRules::compile(rules_raw);
    let mut learned = LearnEngine::from_db(db)
        .await
        .unwrap_or_else(|_| LearnEngine::from_parts(Vec::new(), &[]));
    let bills = db.list_bills().await.unwrap_or_default();
    let bill_matchers = crate::bill_match::compile(&bills);

    for (n, t) in parsed.transactions.iter().enumerate() {
        let ts = t
            .date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| "invalid txn date".to_string())?
            .and_utc()
            .timestamp();
        let hash = provider_id(account_id, t);

        if t.date.year() < KEEP_FROM_YEAR {
            let _ = db
                .record_txn_imported(account_id, &hash, None, false, None)
                .await;
            skipped += 1;
            continue;
        }

        if db.is_txn_seen(account_id, &hash).await.unwrap_or(false) {
            skipped += 1;
            continue;
        }
        if let Ok(Some(existing_id)) = db
            .find_matching_transaction(account_id, ts, &t.description, t.amount_cents, t.is_credit)
            .await
        {
            let _ = db
                .record_txn_imported(account_id, &hash, Some(existing_id), false, None)
                .await;
            skipped += 1;
            continue;
        }

        let pseudo = Transaction {
            id: 0,
            account_id,
            provider_txn_id: hash.clone(),
            timestamp: ts,
            description: t.description.clone(),
            amount_cents: t.amount_cents,
            currency: "GBP".into(),
            is_credit: if t.is_credit { 1 } else { 0 },
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
            created_at: 0,
            updated_at: 0,
        };
        let (category_id, _) = learned.category_for_new(&pseudo, &compiled_rules);
        let eff = rules::apply(&pseudo, &compiled_rules);

        let new_id = match db
            .upsert_transaction(
                account_id,
                &hash,
                ts,
                &t.description,
                t.amount_cents,
                "GBP",
                t.is_credit,
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
                errors.push(format!("row {n}: db error {e:#}"));
                continue;
            }
        };
        if let Some(cid) = category_id {
            learned.observe(None, &t.description, cid);
        }
        if let Err(e) = crate::pairs::on_new_transaction(db, new_id).await {
            tracing::warn!("pair/await for vanquis txn {new_id}: {e:#}");
        }
        for tid in &eff.add_tag_ids {
            let _ = db.tag_transaction(new_id, *tid).await;
        }
        for rid in &eff.rules_fired {
            let _ = db.bump_rule_applied(*rid).await;
        }
        let _ = db
            .record_txn_imported(account_id, &hash, Some(new_id), false, None)
            .await;
        let _ = db
            .auto_link_bill_if_matched(
                &bill_matchers,
                account_id,
                new_id,
                &t.description,
                None,
                t.amount_cents,
                t.is_credit,
                ts,
            )
            .await;
        imported += 1;
    }

    if parsed.statement_date.year() >= KEEP_FROM_YEAR {
        let stmt_iso = parsed.statement_date.format("%Y-%m-%d").to_string();
        if let Err(e) = db
            .update_last_statement(account_id, parsed.statement_balance_cents, &stmt_iso)
            .await
        {
            errors.push(format!("could not set last statement: {e:#}"));
        }
    }

    Ok((imported, skipped, errors))
}

pub async fn import_dir(db: &Db, dir: &Path) -> ImportReport {
    let mut report = ImportReport {
        account_id: account_id(),
        ..ImportReport::default()
    };
    let account = match resolve_account(db, report.account_id).await {
        Ok(a) => a,
        Err(e) => {
            report.files_failed = 1;
            report.files.push(FileReport {
                path: dir.display().to_string(),
                source: "setup".into(),
                original_name: String::new(),
                period: None,
                archive_name: None,
                statement_date: None,
                statement_balance_cents: None,
                imported: 0,
                skipped: 0,
                errors: vec![e],
                ok: false,
            });
            return report;
        }
    };
    report.account_id = account.id;

    let files = collect_pdfs(dir);
    if files.is_empty() {
        info!(dir = %dir.display(), "vanquis import: no PDFs");
        return report;
    }

    for path in files {
        let source = source_of(dir, &path);
        let original_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let mut fr = FileReport {
            path: path.display().to_string(),
            source,
            original_name,
            period: None,
            archive_name: None,
            statement_date: None,
            statement_balance_cents: None,
            imported: 0,
            skipped: 0,
            errors: Vec::new(),
            ok: false,
        };
        match extract_pdf_text(&path).and_then(|t| parse_text(&t)) {
            Ok(parsed) => {
                fr.period = Some(parsed.period_yyyymm());
                fr.archive_name = Some(parsed.archive_name());
                fr.statement_date = Some(parsed.statement_date.format("%Y-%m-%d").to_string());
                fr.statement_balance_cents = Some(parsed.statement_balance_cents);
                match import_parsed(db, account.id, &parsed).await {
                    Ok((imported, skipped, errors)) => {
                        fr.imported = imported;
                        fr.skipped = skipped;
                        fr.errors = errors;
                        fr.ok = fr.errors.is_empty();
                    }
                    Err(e) => fr.errors.push(e),
                }
            }
            Err(e) => fr.errors.push(e),
        }
        report.transactions_imported += fr.imported;
        report.transactions_skipped += fr.skipped;
        if fr.ok {
            report.files_ok += 1;
        } else {
            report.files_failed += 1;
        }
        report.files.push(fr);
    }

    info!(
        account_id = account.id,
        files_ok = report.files_ok,
        files_failed = report.files_failed,
        imported = report.transactions_imported,
        skipped = report.transactions_skipped,
        "vanquis import finished"
    );
    report
}

pub async fn run_scheduled(db: &Db) {
    let dir = inbound_dir();
    let report = import_dir(db, &dir).await;
    if report.files.is_empty() {
        return;
    }
    if report.files_failed > 0 {
        warn!(
            failed = report.files_failed,
            errors = ?report
                .files
                .iter()
                .filter(|f| !f.ok)
                .flat_map(|f| f.errors.iter())
                .take(8)
                .collect::<Vec<_>>(),
            "vanquis import had failures"
        );
    } else if report.transactions_imported > 0 {
        info!(
            imported = report.transactions_imported,
            files = report.files_ok,
            "vanquis scheduled import ok"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    const JULY_2023: &str = r#"
                                                                                       Statement Date:                       10 July 2023
     by Direct Debit from your bank account on or                                    This month's balance                      £159.48
  TRANSACTIONS
  LAST STATEMENT BALANCE                                                                      +        £228.99
  Date               Description                                            Amount
  SPEND
  16/06/23           W M MORRISON PETROL      ANLABY 107      GB                78.11
  18/06/23           W M MORRISON PETROL      ANLABY 107      GB                81.37
                                                                                              +          £159.48
  PAYMENTS RECEIVED
  06/07/23           DIRECT DEBIT PAYMENT                                      228.99
                                                                                              -          £228.99   Cr
  STATEMENT BALANCE                                                                           =        £159.48
"#;

    const WITH_REFUNDS: &str = r#"
                                                                                     Statement Date:                       12 July 2026
     claimed by Direct Debit from your bank account                                This month's balance                    £2,446.94
  SPEND
  10/06/26         LIDL GB WILLERBY             LIDL GB WILLE     GB      43.45
  13/06/26         NON STERLING TRANSACTION FEE                            3.07
  18/06/26         CUFFE & TAYLOR EVENTS        LONDON            GB      10.25
  18/06/26           CUFFE & TAYLOR EVENTS      LONDON           GB             10.25
  REFUNDS AND ADJUSTMENTS
  18/06/26           PAYPAL *TOOLSTATION      08081007211   GB                   3.29
  02/07/26           PAYPAL *FISHPONDWOR      226582341     GB                  23.32
                                                                                              -          £26.61    Cr
  PAYMENTS RECEIVED
  06/07/26           DIRECT DEBIT PAYMENT                                     2,173.56
                                                                                              -        £2,173.56   Cr
  STATEMENT BALANCE                                                                           =      £2,446.94
"#;

    #[test]
    fn july_2023_spend_and_payment() {
        let p = parse_text(JULY_2023).unwrap();
        assert_eq!(p.statement_date, NaiveDate::from_ymd_opt(2023, 7, 10).unwrap());
        assert_eq!(p.period_yyyymm(), "202307");
        assert_eq!(p.archive_name(), "202307 -MonthlyStatement.pdf");
        assert_eq!(p.statement_balance_cents, 15_948);
        assert_eq!(p.transactions.len(), 3);
        assert!(!p.transactions[0].is_credit);
        assert_eq!(p.transactions[0].amount_cents, 7811);
        assert_eq!(p.transactions[1].amount_cents, 8137);
        assert!(p.transactions[2].is_credit);
        assert_eq!(p.transactions[2].description, "DIRECT DEBIT PAYMENT");
        assert_eq!(p.transactions[2].amount_cents, 22_899);
    }

    #[test]
    fn refunds_and_page_break_dedup() {
        let p = parse_text(WITH_REFUNDS).unwrap();
        assert_eq!(p.period_yyyymm(), "202607");
        assert_eq!(p.statement_balance_cents, 244_694);
        let cuffe: Vec<_> = p
            .transactions
            .iter()
            .filter(|t| t.description.contains("CUFFE"))
            .collect();
        assert_eq!(cuffe.len(), 1, "page-break duplicate must drop");
        let credits: Vec<_> = p.transactions.iter().filter(|t| t.is_credit).collect();
        assert_eq!(credits.len(), 3);
        assert_eq!(credits[0].amount_cents, 329);
        assert_eq!(credits[1].amount_cents, 2332);
        assert_eq!(credits[2].amount_cents, 217_356);
        assert!(credits[2].is_credit);
        let fees: Vec<_> = p
            .transactions
            .iter()
            .filter(|t| t.description.contains("NON STERLING"))
            .collect();
        assert_eq!(fees.len(), 1);
        assert!(!fees[0].is_credit);
    }

    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64STD.encode([9u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        Db {
            conn: std::sync::Arc::new(conn),
            crypto,
        }
    }

    #[tokio::test]
    async fn import_dedup_and_latest_statement() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Vanquis", "credit_card", None)
            .await
            .unwrap();
        let older = parse_text(JULY_2023).unwrap();
        let (i1, s1, e1) = import_parsed(&db, acct.id, &older).await.unwrap();
        assert!(e1.is_empty(), "{e1:?}");
        assert_eq!(i1, 0, "pre-2025 statement lines are not stored");
        assert_eq!(s1, 3);
        let (i2, s2, e2) = import_parsed(&db, acct.id, &older).await.unwrap();
        assert!(e2.is_empty(), "{e2:?}");
        assert_eq!(i2, 0);
        assert_eq!(s2, 3);

        let newer = parse_text(WITH_REFUNDS).unwrap();
        import_parsed(&db, acct.id, &newer).await.unwrap();
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.last_statement_balance_cents, Some(244_694));
        assert_eq!(after.last_statement_date.as_deref(), Some("2026-07-12"));
    }

    #[tokio::test]
    async fn older_statement_does_not_clobber_newer_snapshot() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Vanquis", "credit_card", None)
            .await
            .unwrap();
        import_parsed(&db, acct.id, &parse_text(WITH_REFUNDS).unwrap())
            .await
            .unwrap();
        import_parsed(&db, acct.id, &parse_text(JULY_2023).unwrap())
            .await
            .unwrap();
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.last_statement_date.as_deref(), Some("2026-07-12"));
        assert_eq!(after.last_statement_balance_cents, Some(244_694));
    }

    #[tokio::test]
    async fn skips_txns_before_2025_keeps_from_new_year() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Vanquis", "credit_card", None)
            .await
            .unwrap();
        let parsed = ParsedStatement {
            statement_date: NaiveDate::from_ymd_opt(2025, 1, 12).unwrap(),
            statement_balance_cents: 100,
            transactions: vec![
                ParsedTxn {
                    date: NaiveDate::from_ymd_opt(2024, 12, 31).unwrap(),
                    description: "OLD FUEL".into(),
                    amount_cents: 1000,
                    is_credit: false,
                },
                ParsedTxn {
                    date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                    description: "NEW FUEL".into(),
                    amount_cents: 2000,
                    is_credit: false,
                },
            ],
        };
        let (imported, skipped, errors) = import_parsed(&db, acct.id, &parsed).await.unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(imported, 1);
        assert_eq!(skipped, 1);
        let rows = db
            .list_transactions(
                Some(&[acct.id]),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                10,
                0,
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].description, "NEW FUEL");
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.last_statement_date.as_deref(), Some("2025-01-12"));
    }
}
