//! Capital on Tap monthly-statement PDF importer.
//!
//! Host rclone copies new PDFs from Dropbox
//! (`dropbox:!Inbox/Documents/CapitalOnTap Statements`, folder root only) onto
//! `/app/inbound/capital-on-tap/inbox`. After a successful import (including
//! dedup no-op), the host script files the PDF onto OneDrive as
//! `YYYYMM -MonthlyStatement.pdf` under
//! `onedrive:Documents/Financial/CapitalOnTap Statements` and removes the
//! Dropbox copy. The inbound Docker mount stays read-only.
//!
//! Polarity: card spend / fees / interest → `is_credit = false`. Payments and
//! refunds → `is_credit = true`. Prefer the cleared date when both authorised
//! and cleared exist. Amounts in GBP pence.

use crate::db::Db;
use crate::learn::LearnEngine;
use crate::models::Transaction;
use crate::rules::{self, CompiledRules};
use chrono::NaiveDate;
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tracing::{info, warn};

pub const DEFAULT_DIR: &str = "/app/inbound/capital-on-tap";
pub const DEFAULT_ACCOUNT_ID: i64 = 30;

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
    pub due_date: Option<NaiveDate>,
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
    std::env::var("TALLY_CAPITAL_ON_TAP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DIR))
}

pub fn account_id() -> i64 {
    std::env::var("TALLY_CAPITAL_ON_TAP_ACCOUNT_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ACCOUNT_ID)
}

fn period_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(\d{1,2}\s+(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{4})\s*[-–—]\s*(\d{1,2}\s+(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{4})",
        )
        .unwrap()
    })
}

fn row_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            ^\s*
            (?:(\d{2}/\d{2}/\d{4})|-)
            \s+
            (?:(\d{2}/\d{2}/\d{4})|-)
            \s+
            (.+?)
            \s+
            (-|£?-?[\d,]+\.\d{2})
            \s+
            (-?£?[\d,]+\.\d{2})
            \s*$",
        )
        .unwrap()
    })
}

fn summary_amounts_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"£([\d,]+\.\d{2})").unwrap())
}

fn known_types() -> &'static [&'static str] {
    &[
        "Savings to Credit Transfer",
        "Instant Bank Transfer",
        "Manual Bank Transfer",
        "Point Redemption",
        "Other Repayment",
        "Direct Debit",
        "Debit Card",
        "Smart Repay",
        "Adjustments",
        "Adjustment",
        "Interest",
        "Refund",
        "Payment",
        "Transfer",
        "Charge",
        "Card",
        "Fee",
        "ATM",
        "-",
    ]
}

fn is_credit_type(type_name: &str) -> bool {
    matches!(
        type_name.to_ascii_lowercase().as_str(),
        "direct debit"
            | "debit card"
            | "instant bank transfer"
            | "manual bank transfer"
            | "smart repay"
            | "point redemption"
            | "savings to credit transfer"
            | "other repayment"
            | "refund"
            | "payment"
            | "adjustment"
            | "adjustments"
    )
}

fn money_cents(raw: &str) -> Option<i64> {
    let t = raw
        .trim()
        .replace(",", "")
        .replace("£", "")
        .replace("+", "");
    if t.is_empty() || t == "-" {
        return None;
    }
    let t = t.trim_matches(|c: char| c == '(' || c == ')');
    let n: f64 = t.parse().ok()?;
    Some((n.abs() * 100.0).round() as i64)
}

fn amount_negative(raw: &str) -> bool {
    let t = raw.trim().replace(",", "").replace("£", "").replace("+", "");
    t.starts_with('-') || (t.starts_with('(') && t.ends_with(')'))
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_uk_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%d/%m/%Y").map_err(|e| format!("date '{s}': {e}"))
}

fn parse_long_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s.trim(), "%d %B %Y").map_err(|e| format!("date '{}': {e}", s.trim()))
}

fn starts_type(middle: &str, typ: &str) -> bool {
    if typ == "-" {
        return middle == "-" || middle.starts_with("- ");
    }
    let m = middle.to_ascii_lowercase();
    let t = typ.to_ascii_lowercase();
    m == t || m.starts_with(&format!("{t} "))
}

fn split_type_card_desc(middle: &str) -> (String, String, String) {
    let m = collapse_ws(middle);
    for typ in known_types() {
        if !starts_type(&m, typ) {
            continue;
        }
        let rest = m[typ.len()..].trim();
        if rest.is_empty() {
            return ((*typ).to_string(), "-".into(), String::new());
        }
        let mut parts = rest.splitn(2, ' ');
        let card = parts.next().unwrap_or("-").to_string();
        let desc = parts.next().unwrap_or("").trim().to_string();
        return ((*typ).to_string(), card, desc);
    }
    let mut parts = m.splitn(3, ' ');
    let typ = parts.next().unwrap_or("-").to_string();
    let card = parts.next().unwrap_or("-").to_string();
    let desc = parts.next().unwrap_or("").to_string();
    (typ, card, desc)
}

fn skip_description(desc: &str) -> bool {
    matches!(
        desc.to_ascii_lowercase().as_str(),
        "opening balance" | "closing balance"
    )
}

fn row_is_credit(type_name: &str, description: &str, amount_raw: &str) -> bool {
    let d = description.to_ascii_uppercase();
    let mut credit = is_credit_type(type_name) || d.contains("REFUND");
    if amount_negative(amount_raw) {
        credit = !credit;
    }
    credit
}

/// Parse pdftotext -layout output from a Capital on Tap monthly statement.
pub fn parse_text(text: &str) -> Result<ParsedStatement, String> {
    let mut statement_date = None;
    let mut balance = None;
    let mut due_date = None;
    let mut expect_due = false;
    let mut saw_summary_headers = false;
    let mut transactions: Vec<ParsedTxn> = Vec::new();

    for raw in text.lines() {
        let collapsed = collapse_ws(raw);
        if collapsed.contains("Authorised Date") && collapsed.contains("Cleared Date") {
            continue;
        }

        if expect_due {
            if let Ok(d) = parse_long_date(&collapsed) {
                due_date = Some(d);
            }
            expect_due = false;
        }
        if collapsed.eq_ignore_ascii_case("due date") {
            expect_due = true;
        }
        if due_date.is_none() {
            if let Some(rest) = collapsed
                .strip_prefix("Due Date")
                .or_else(|| collapsed.strip_prefix("due date"))
            {
                let rest = rest.trim();
                if !rest.is_empty() {
                    if let Ok(d) = parse_long_date(rest) {
                        due_date = Some(d);
                    }
                }
            }
        }

        if statement_date.is_none() {
            if let Some(c) = period_re().captures(&collapsed) {
                statement_date = Some(parse_long_date(&c[2])?);
            }
        }

        let upper = collapsed.to_ascii_uppercase();
        if upper.contains("OPENING BALANCE")
            && upper.contains("CLOSING BALANCE")
            && upper.contains("SPENDING")
        {
            saw_summary_headers = true;
        } else if saw_summary_headers && balance.is_none() {
            let amounts: Vec<_> = summary_amounts_re()
                .captures_iter(&collapsed)
                .filter_map(|c| money_cents(&c[1]))
                .collect();
            if amounts.len() >= 4 {
                balance = amounts.last().copied();
            }
            saw_summary_headers = false;
        }

        let Some(c) = row_re().captures(raw) else {
            continue;
        };
        let auth = c.get(1).map(|m| m.as_str());
        let cleared = c.get(2).map(|m| m.as_str());
        let date = match (cleared, auth) {
            (Some(d), _) => parse_uk_date(d)?,
            (None, Some(d)) => parse_uk_date(d)?,
            (None, None) => continue,
        };
        let (type_name, _card, description) = split_type_card_desc(&c[3]);
        let amount_raw = c[4].trim();
        let bal_raw = c[5].trim();
        if description.eq_ignore_ascii_case("closing balance") {
            if let Some(b) = money_cents(bal_raw) {
                balance = Some(b);
            }
            continue;
        }
        if skip_description(&description) {
            continue;
        }
        let Some(amount_cents) = money_cents(amount_raw) else {
            continue;
        };
        if amount_cents == 0 {
            continue;
        }
        let is_credit = row_is_credit(&type_name, &description, amount_raw);
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

    let statement_date =
        statement_date.ok_or_else(|| "no statement period on PDF".to_string())?;
    let statement_balance_cents =
        balance.ok_or_else(|| "no Closing Balance on PDF".to_string())?;
    if transactions.is_empty() {
        return Err("no transactions found on PDF".into());
    }
    Ok(ParsedStatement {
        statement_date,
        statement_balance_cents,
        due_date,
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
    format!("cot-pdf:{account_id}:{:x}", h.finalize())
}

async fn resolve_account(db: &Db, id: i64) -> Result<crate::models::Account, String> {
    let acct = db
        .get_account(id)
        .await
        .map_err(|e| format!("{e}"))?
        .ok_or_else(|| format!("Capital On Tap account id {id} not found"))?;
    let name = acct
        .custom_display_name
        .as_deref()
        .unwrap_or(&acct.display_name);
    if !name.to_ascii_lowercase().contains("capital on tap") {
        return Err(format!(
            "account {id} is '{name}', not Capital On Tap — refusing to import"
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
            tracing::warn!("pair/await for capital-on-tap txn {new_id}: {e:#}");
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

    let stmt_iso = parsed.statement_date.format("%Y-%m-%d").to_string();
    if let Err(e) = db
        .update_last_statement(account_id, parsed.statement_balance_cents, &stmt_iso)
        .await
    {
        errors.push(format!("could not set last statement: {e:#}"));
    }
    if let Some(due) = parsed.due_date {
        if let Err(e) = set_repayment_bill_due(db, due).await {
            errors.push(format!("could not set repayment bill due: {e:#}"));
        }
    }

    Ok((imported, skipped, errors))
}

async fn set_repayment_bill_due(db: &Db, due: NaiveDate) -> Result<(), String> {
    let due_ts = due
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| "invalid due date".to_string())?
        .and_utc()
        .timestamp();
    let bills = db.list_bills().await.map_err(|e| format!("{e}"))?;
    for b in bills {
        if b.enabled == 0 {
            continue;
        }
        let blob = format!(
            "{} {}",
            b.name,
            b.match_description_regex.as_deref().unwrap_or("")
        );
        if !blob.to_ascii_uppercase().contains("CAPITAL ON TAP") {
            continue;
        }
        if b.next_expected_date.map(|d| d >= due_ts).unwrap_or(false) {
            continue;
        }
        db.update_bill_schedule(
            b.id,
            b.expected_amount_min_cents,
            b.expected_amount_max_cents,
            Some(due_ts),
            b.account_id,
        )
        .await
        .map_err(|e| format!("{e}"))?;
    }
    Ok(())
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
        info!(dir = %dir.display(), "capital-on-tap import: no PDFs");
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
        "capital-on-tap import finished"
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
            "capital-on-tap import had failures"
        );
    } else if report.transactions_imported > 0 {
        info!(
            imported = report.transactions_imported,
            files = report.files_ok,
            "capital-on-tap scheduled import ok"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    /// pdftotext -layout extract of Dropbox `Statement 08-08-2026 07-09-2026.pdf`.
    const AUG_SEP_2026: &str = r#"
                                                                                                                                               Statement Summary
                                                                                                                08 August 2026 - 07 September 2026

RemixMix LTD                                                                                                                                                    Customer Reference
                                                                                                                                                                         G921X65
                                                                                                                                                                         Due Date
                                                                                                                                                                14 September 2026

                     Opening Balance                                 Spending Activity                           Repayment Activity                                 Closing Balance

                                  £0.00                                       +£140.04                                           £0.00                                       £140.04
                                                                    Card Spend: £140.04                             Direct Debit: £0.00
                                                                          Interest: £0.00

Account Statement

  Authorised Date            Cleared Date           Type              Card          Description                                                  Amount (£)           Balance (£)


  -                          08/08/2026             -                 -             Opening Balance                                                         -                 0.00


  27/08/2026                 29/08/2026             Card              *4080         PAYPAL *EBAY UK - 795653703                                        13.98                 13.98


  02/09/2026                 03/09/2026             Card              *4080         St Stephens Shopping Cent - Hull                                    1.00                 14.98


  02/09/2026                 03/09/2026             Card              *4080         The Real China - Hull                                              54.90                 69.88


  01/09/2026                 04/09/2026             Card              *4080         PAYPAL *EBAY UK - 795653703                                        33.60               103.48


  01/09/2026                 04/09/2026             Card              *4080         PAYPAL *EBAY UK - 795653703                                        19.00               122.48


  03/09/2026                 06/09/2026             Card              *4080         PAYPAL *EBAY UK - 795653703                                         9.48               131.96


  -                          07/09/2026             Interest          -             Interest Charge (08/08/2026 - 07/09/2026)                               -              131.96


  06/09/2026                 07/09/2026             Card              *4080         PAYPAL *CLOUDFLARE - 4029357733                                     8.08               140.04


  Authorised Date            Cleared Date           Type            Card           Description                                                   Amount (£)           Balance (£)


  -                          07/09/2026             -               -              Closing Balance                                                          -             140.04
"#;

    const WITH_PAYMENT_AND_REFUND: &str = r#"
                                                                                                                08 September 2026 - 07 October 2026
                     Opening Balance                                 Spending Activity                           Repayment Activity                                 Closing Balance
                                £140.04                                        +£10.00                                        £145.04                                         £5.00
  07/09/2026                 08/09/2026             Card              *4080         PAYPAL *EBAY UK - 795653703                                        15.00               155.04
  09/09/2026                 10/09/2026             Card              *4080         PAYPAL *EBAY UK - REFUND                                           5.00               150.04
  10/09/2026                 11/09/2026             Card              *4080         St Stephens Shopping Cent - Hull                                   -5.00               145.04
  14/09/2026                 14/09/2026             Direct Debit      -             Direct Debit Payment                                           140.04                 5.00
  15/09/2026                 16/09/2026             Interest          -             Interest Charge                                                  0.00                 5.00
  -                          07/10/2026             -               -              Closing Balance                                                          -               5.00
"#;

    #[test]
    fn first_drop_pdf_spend_skips_balances_and_blank_interest() {
        let p = parse_text(AUG_SEP_2026).unwrap();
        assert_eq!(
            p.statement_date,
            NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()
        );
        assert_eq!(p.period_yyyymm(), "202609");
        assert_eq!(p.archive_name(), "202609 -MonthlyStatement.pdf");
        assert_eq!(p.statement_balance_cents, 14_004);
        assert_eq!(
            p.due_date,
            Some(NaiveDate::from_ymd_opt(2026, 9, 14).unwrap())
        );
        assert_eq!(p.transactions.len(), 7);
        assert!(p.transactions.iter().all(|t| !t.is_credit));
        assert_eq!(
            p.transactions[0].date,
            NaiveDate::from_ymd_opt(2026, 8, 29).unwrap(),
            "prefer cleared date over authorised"
        );
        assert_eq!(p.transactions[0].description, "PAYPAL *EBAY UK - 795653703");
        assert_eq!(p.transactions[0].amount_cents, 1_398);
        assert_eq!(
            p.transactions[1].description,
            "St Stephens Shopping Cent - Hull"
        );
        assert_eq!(p.transactions[1].amount_cents, 100);
        assert_eq!(p.transactions[2].description, "The Real China - Hull");
        assert_eq!(p.transactions[2].amount_cents, 5_490);
        assert_eq!(p.transactions[3].amount_cents, 3_360);
        assert_eq!(p.transactions[4].amount_cents, 1_900);
        assert_eq!(p.transactions[5].amount_cents, 948);
        assert_eq!(
            p.transactions[6].date,
            NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()
        );
        assert_eq!(
            p.transactions[6].description,
            "PAYPAL *CLOUDFLARE - 4029357733"
        );
        assert_eq!(p.transactions[6].amount_cents, 808);
        assert!(
            p.transactions
                .iter()
                .all(|t| !t.description.to_ascii_lowercase().contains("interest")),
            "interest with no amount must not become a txn"
        );
    }

    #[test]
    fn payments_refunds_and_signed_amounts() {
        let p = parse_text(WITH_PAYMENT_AND_REFUND).unwrap();
        assert_eq!(p.period_yyyymm(), "202610");
        assert_eq!(p.statement_balance_cents, 500);
        assert_eq!(p.transactions.len(), 4);
        assert!(!p.transactions[0].is_credit);
        assert_eq!(p.transactions[0].amount_cents, 1_500);
        assert!(p.transactions[1].is_credit, "REFUND in description");
        assert_eq!(p.transactions[1].amount_cents, 500);
        assert!(p.transactions[2].is_credit, "negative card amount is a refund");
        assert_eq!(p.transactions[2].amount_cents, 500);
        assert!(p.transactions[3].is_credit);
        assert_eq!(p.transactions[3].description, "Direct Debit Payment");
        assert_eq!(p.transactions[3].amount_cents, 14_004);
        assert_eq!(
            p.transactions[3].date,
            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
        );
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
    async fn import_dedup_and_statement_snapshot() {
        let db = test_db().await;
        let acct = db
            .create_manual_account("Capital On Tap", "business_credit_card", None)
            .await
            .unwrap();
        let tide = db
            .create_manual_account("Tide Current Account", "business_current", None)
            .await
            .unwrap();
        db.create_bill(
            "CAPITAL ON TAP (DD)",
            0,
            0,
            "GBP",
            "monthly",
            None,
            Some("CAPITAL ON TAP"),
            None,
            Some(tide.id),
            false,
        )
        .await
        .unwrap();
        let parsed = parse_text(AUG_SEP_2026).unwrap();
        let (i1, s1, e1) = import_parsed(&db, acct.id, &parsed).await.unwrap();
        assert!(e1.is_empty(), "{e1:?}");
        assert_eq!(i1, 7);
        assert_eq!(s1, 0);
        let after = db.get_account(acct.id).await.unwrap().unwrap();
        assert_eq!(after.last_statement_balance_cents, Some(14_004));
        assert_eq!(after.last_statement_date.as_deref(), Some("2026-09-07"));
        let bills = db.list_bills().await.unwrap();
        let due = NaiveDate::from_ymd_opt(2026, 9, 14)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(bills[0].next_expected_date, Some(due));

        let (i2, s2, e2) = import_parsed(&db, acct.id, &parsed).await.unwrap();
        assert!(e2.is_empty(), "{e2:?}");
        assert_eq!(i2, 0);
        assert_eq!(s2, 7);
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
                20,
                0,
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 7);
    }
}
