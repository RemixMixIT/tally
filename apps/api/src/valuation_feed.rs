//! Batty → Tally valuation feed.
//!
//! Consumes `tally_valuations.csv` published to Google Drive
//! (`_outbound/tally/tally_valuations.csv`) and already copied onto Speed.
//! Child holding rows attach to the parent SIPP; they are never created as
//! extra financial_items, so net worth cannot double-count them.
//!
//! Locked rules (ClickUp 124032dg3tt):
//! - SIPP account total = AJ Bell SIPP roll-up `gbp_value`.
//! - USD amounts are already converted on Batty; do not re-apply GBPUSD as a
//!   multiply-to-GBP rate (Tally stores gbp_value and derives its own rate).
//! - Global Prime uses equity. A login missing from the file is left alone
//!   (never written as £0).
//! - ISA is out of this feed.
//! - New dates insert snapshots. Same-day feed rows update the existing
//!   `batty-feed` snapshot so cadence refreshes do not duplicate history.

use crate::db::Db;
use crate::finance::{self, FinanceError};
use crate::models::{FinancialItem, ValuationSnapshot};
use chrono::NaiveDate;
use csv::ReaderBuilder;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::{info, warn};

pub const FEED_SOURCE_TYPE: &str = "other";
pub const FEED_SOURCE_PREFIX: &str = "batty-feed";
pub const DEFAULT_FEED_PATH: &str = "/app/inbound/tally_valuations.csv";
pub const FINGERPRINT_SETTING: &str = "valuation_feed_fingerprint";

#[derive(Debug, Clone)]
pub struct FeedRow {
    pub record_name: String,
    pub class: String,
    pub owner_entity: String,
    pub provider_reference: String,
    pub parent_asset: String,
    pub currency: String,
    pub native_value: f64,
    pub gbp_value: f64,
    pub fx_rate: Option<f64>,
    pub fx_date: Option<String>,
    pub valuation_date: String,
    pub source_evidence: String,
    pub confidence: String,
    pub holding_name: String,
    pub units: Option<f64>,
    pub price: Option<f64>,
    pub cost_basis: Option<f64>,
    pub notes: String,
    /// GBP cash into the book on `valuation_date`. `None` = column blank/missing.
    pub contributions: Option<f64>,
    /// GBP cash out of the book on `valuation_date`. `None` = column blank/missing.
    pub withdrawals: Option<f64>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct FeedReport {
    pub path: Option<String>,
    pub skipped_unchanged: bool,
    pub items_created: i64,
    pub snapshots_inserted: i64,
    pub snapshots_updated: i64,
    pub holdings_upserted: i64,
    pub holdings_removed: i64,
    pub rows_skipped: i64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub rows: Vec<FeedRow>,
    pub warnings: Vec<String>,
}

pub fn feed_path() -> std::path::PathBuf {
    std::env::var("TALLY_VALUATION_FEED_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(DEFAULT_FEED_PATH))
}

pub fn fingerprint(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn parse_csv(content: &str) -> Result<ParsedFeed, FinanceError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .quoting(true)
        .double_quote(true)
        .from_reader(content.as_bytes());
    let headers = rdr
        .headers()
        .map_err(|e| FinanceError::invalid(format!("bad CSV headers: {e}")))?
        .clone();
    let idx = |name: &str| -> Option<usize> {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let required = [
        "record_name",
        "class",
        "currency",
        "native_value",
        "gbp_value",
        "valuation_date",
    ];
    for col in required {
        if idx(col).is_none() {
            return Err(FinanceError::invalid(format!(
                "valuation feed missing required column {col}"
            )));
        }
    }

    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| FinanceError::invalid(format!("bad CSV row {i}: {e}")))?;
        let get = |name: &str| -> String {
            idx(name)
                .and_then(|i| rec.get(i))
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let record_name = get("record_name");
        if record_name.is_empty() {
            warnings.push(format!("row {i}: blank record_name, skipped"));
            continue;
        }
        let native = match parse_number(&get("native_value")) {
            Ok(Some(v)) => v,
            Ok(None) => {
                warnings.push(format!("row {i} ({record_name}): missing native_value, skipped"));
                continue;
            }
            Err(e) => {
                warnings.push(format!("row {i} ({record_name}): {e}"));
                continue;
            }
        };
        let gbp = match parse_number(&get("gbp_value")) {
            Ok(Some(v)) => v,
            Ok(None) => {
                warnings.push(format!("row {i} ({record_name}): missing gbp_value, skipped"));
                continue;
            }
            Err(e) => {
                warnings.push(format!("row {i} ({record_name}): {e}"));
                continue;
            }
        };
        let date = match parse_feed_date(&get("valuation_date")) {
            Ok(d) => d,
            Err(e) => {
                warnings.push(format!("row {i} ({record_name}): {e}"));
                continue;
            }
        };
        let fx_date = match nonempty(&get("fx_date")) {
            Some(raw) => match parse_feed_date(raw) {
                Ok(d) => Some(d),
                Err(_) => {
                    warnings.push(format!(
                        "row {i} ({record_name}): ignoring unparseable fx_date {raw}"
                    ));
                    None
                }
            },
            None => None,
        };
        let currency = get("currency").to_ascii_uppercase();
        let confidence = {
            let c = get("confidence").to_ascii_lowercase();
            if finance::CONFIDENCE.contains(&c.as_str()) {
                c
            } else {
                "confirmed".into()
            }
        };
        rows.push(FeedRow {
            record_name,
            class: get("class").to_ascii_lowercase(),
            owner_entity: map_owner(&get("owner_entity")),
            provider_reference: get("provider_reference"),
            parent_asset: get("parent_asset"),
            currency,
            native_value: native,
            gbp_value: gbp,
            fx_rate: parse_number(&get("fx_rate")).ok().flatten(),
            fx_date,
            valuation_date: date,
            source_evidence: get("source_evidence"),
            confidence,
            holding_name: get("holding_name"),
            units: parse_number(&get("units")).ok().flatten(),
            price: parse_number(&get("price")).ok().flatten(),
            cost_basis: parse_number(&get("cost_basis")).ok().flatten(),
            notes: get("notes"),
            contributions: parse_number(&get("contributions")).ok().flatten(),
            withdrawals: parse_number(&get("withdrawals")).ok().flatten(),
        });
    }
    Ok(ParsedFeed { rows, warnings })
}

pub async fn import_path(db: &Db, path: &Path, force: bool) -> Result<FeedReport, FinanceError> {
    let mut report = FeedReport {
        path: Some(path.display().to_string()),
        ..FeedReport::default()
    };
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            report.rows_skipped = 0;
            report
                .errors
                .push(format!("feed file not found: {}", path.display()));
            return Ok(report);
        }
        Err(e) => return Err(FinanceError::invalid(format!("read feed: {e}"))),
    };
    let fp = fingerprint(&content);
    if !force {
        if let Ok(Some(prev)) = db.get_setting(FINGERPRINT_SETTING).await {
            if prev == fp {
                report.skipped_unchanged = true;
                return Ok(report);
            }
        }
    }
    report = apply_csv(db, &content).await?;
    report.path = Some(path.display().to_string());
    db.set_setting(FINGERPRINT_SETTING, &fp)
        .await
        .map_err(|e| FinanceError::invalid(e.to_string()))?;
    Ok(report)
}

pub async fn apply_csv(db: &Db, content: &str) -> Result<FeedReport, FinanceError> {
    let parsed = parse_csv(content)?;
    let mut report = FeedReport::default();
    report.errors.extend(parsed.warnings);

    let items = db
        .list_financial_items()
        .await
        .map_err(|e| FinanceError::invalid(e.to_string()))?;
    let mut by_name: HashMap<String, FinancialItem> = HashMap::new();
    for item in items {
        by_name.insert(norm_name(&item.name), item);
    }

    let (accounts, holdings): (Vec<&FeedRow>, Vec<&FeedRow>) = parsed
        .rows
        .iter()
        .partition(|r| r.parent_asset.is_empty());

    let mut seen_holding_keys: HashMap<i64, HashSet<String>> = HashMap::new();

    for row in accounts {
        if is_isa_name(&row.record_name) {
            report.rows_skipped += 1;
            continue;
        }
        if row.class != "pension" && row.class != "trading" {
            report.rows_skipped += 1;
            continue;
        }
        match apply_account_row(db, row, &mut by_name, &mut report).await {
            Ok(_) => {}
            Err(e) => report.errors.push(format!("{}: {e}", row.record_name)),
        }
    }

    for row in holdings {
        if is_isa_name(&row.record_name) || is_isa_name(&row.parent_asset) {
            report.rows_skipped += 1;
            continue;
        }
        match apply_holding_row(db, row, &by_name, &mut seen_holding_keys, &mut report).await {
            Ok(_) => {}
            Err(e) => report
                .errors
                .push(format!("{} / {}: {e}", row.parent_asset, row.record_name)),
        }
    }

    remove_stale_feed_holdings(db, &seen_holding_keys, &mut report).await?;

    info!(
        created = report.items_created,
        inserted = report.snapshots_inserted,
        updated = report.snapshots_updated,
        holdings = report.holdings_upserted,
        removed = report.holdings_removed,
        "valuation feed applied"
    );
    Ok(report)
}

async fn apply_account_row(
    db: &Db,
    row: &FeedRow,
    by_name: &mut HashMap<String, FinancialItem>,
    report: &mut FeedReport,
) -> Result<(), FinanceError> {
    let key = norm_name(&row.record_name);
    let item = if let Some(existing) = by_name.get(&key) {
        existing.clone()
    } else {
        let (provider, reference) = split_provider(&row.provider_reference, &row.record_name);
        let item_type = default_item_type(&row.class);
        let created = db
            .create_financial_item(
                &row.record_name,
                &row.class,
                item_type,
                &row.owner_entity,
                provider.as_deref(),
                reference.as_deref(),
                None,
                if row.currency == "GBP" {
                    "GBP"
                } else {
                    &row.currency
                },
                row.class == "pension",
                None,
            )
            .await?;
        report.items_created += 1;
        by_name.insert(key, created.clone());
        created
    };

    upsert_feed_snapshot(db, item.id, row, report).await?;
    Ok(())
}

async fn apply_holding_row(
    db: &Db,
    row: &FeedRow,
    by_name: &HashMap<String, FinancialItem>,
    seen: &mut HashMap<i64, HashSet<String>>,
    report: &mut FeedReport,
) -> Result<(), FinanceError> {
    let parent = by_name
        .get(&norm_name(&row.parent_asset))
        .ok_or_else(|| {
            FinanceError::invalid(format!(
                "parent '{}' not found for holding {}",
                row.parent_asset, row.record_name
            ))
        })?;
    if !finance::holdings_allowed(&parent.class) {
        return Err(FinanceError::invalid(format!(
            "parent '{}' ({}) cannot hold positions",
            parent.name, parent.class
        )));
    }

    let ticker = nonempty(&row.record_name).map(|s| s.to_string());
    let holding_name = if row.holding_name.is_empty() {
        row.record_name.clone()
    } else {
        row.holding_name.clone()
    };
    let match_key = holding_match_key(ticker.as_deref(), &holding_name);
    seen.entry(parent.id).or_default().insert(match_key.clone());

    let existing = db
        .list_item_holdings(parent.id)
        .await
        .map_err(|e| FinanceError::invalid(e.to_string()))?;
    let found = existing.into_iter().find(|h| {
        holding_match_key(h.ticker_or_identifier.as_deref(), &h.holding_name) == match_key
    });

    let source_ref = format!("{FEED_SOURCE_PREFIX}:{match_key}");
    let notes = combined_notes(row);
    let price_ccy = nonempty(&row.currency);

    let mut cost_basis = row.cost_basis;
    if is_cash_holding(ticker.as_deref(), &holding_name) && cost_basis.is_none() {
        cost_basis = Some(row.native_value);
    }

    if let Some(h) = found {
        db.update_item_holding(
            h.id,
            &holding_name,
            ticker.as_deref(),
            row.units,
            row.price,
            price_ccy,
            cost_basis,
            None,
            None,
            &row.valuation_date,
            row.native_value,
            &row.currency,
            Some(row.gbp_value),
            Some(&source_ref),
            notes.as_deref(),
        )
        .await?;
    } else {
        db.create_item_holding(
            parent.id,
            &holding_name,
            ticker.as_deref(),
            row.units,
            row.price,
            price_ccy,
            cost_basis,
            None,
            None,
            &row.valuation_date,
            row.native_value,
            &row.currency,
            Some(row.gbp_value),
            Some(&source_ref),
            notes.as_deref(),
        )
        .await?;
    }
    report.holdings_upserted += 1;
    Ok(())
}

async fn upsert_feed_snapshot(
    db: &Db,
    item_id: i64,
    row: &FeedRow,
    report: &mut FeedReport,
) -> Result<ValuationSnapshot, FinanceError> {
    let source_ref = FEED_SOURCE_PREFIX.to_string();
    let notes = combined_notes(row);
    let existing = db
        .list_valuations(item_id)
        .await
        .map_err(|e| FinanceError::invalid(e.to_string()))?;
    let same_day_feed = existing.into_iter().find(|v| {
        v.valuation_date == row.valuation_date
            && v.source_reference
                .as_deref()
                .is_some_and(|s| s.starts_with(FEED_SOURCE_PREFIX))
    });

    // Batty's fx_rate is a GBPUSD pair quote, not Tally's multiply-to-GBP rate.
    // Pass gbp_value only; Tally derives the stored rate when currency ≠ GBP.
    let snap = if let Some(prev) = same_day_feed {
        let updated = db
            .update_valuation(
                prev.id,
                &row.valuation_date,
                row.native_value,
                &row.currency,
                Some(row.gbp_value),
                None,
                row.fx_date.as_deref(),
                FEED_SOURCE_TYPE,
                Some(&source_ref),
                &row.confidence,
                row.contributions,
                row.withdrawals,
                notes.as_deref(),
            )
            .await?;
        report.snapshots_updated += 1;
        updated
    } else {
        let inserted = db
            .create_valuation(
                item_id,
                &row.valuation_date,
                row.native_value,
                &row.currency,
                Some(row.gbp_value),
                None,
                row.fx_date.as_deref(),
                FEED_SOURCE_TYPE,
                Some(&source_ref),
                &row.confidence,
                row.contributions,
                row.withdrawals,
                notes.as_deref(),
            )
            .await?;
        report.snapshots_inserted += 1;
        inserted
    };
    Ok(snap)
}

async fn remove_stale_feed_holdings(
    db: &Db,
    seen: &HashMap<i64, HashSet<String>>,
    report: &mut FeedReport,
) -> Result<(), FinanceError> {
    for (item_id, keep) in seen {
        let holdings = db
            .list_item_holdings(*item_id)
            .await
            .map_err(|e| FinanceError::invalid(e.to_string()))?;
        for h in holdings {
            let from_feed = h
                .source_reference
                .as_deref()
                .is_some_and(|s| s.starts_with(FEED_SOURCE_PREFIX));
            if !from_feed {
                continue;
            }
            let key = holding_match_key(h.ticker_or_identifier.as_deref(), &h.holding_name);
            if keep.contains(&key) {
                continue;
            }
            db.delete_item_holding(h.id).await?;
            report.holdings_removed += 1;
        }
    }
    Ok(())
}

pub async fn run_scheduled(db: &Db) {
    let path = feed_path();
    match import_path(db, &path, false).await {
        Ok(r) if r.skipped_unchanged => {
            tracing::debug!(path = %path.display(), "valuation feed unchanged");
        }
        Ok(r) => {
            if !r.errors.is_empty() {
                warn!(path = %path.display(), errors = ?r.errors, "valuation feed applied with warnings");
            }
            info!(
                path = %path.display(),
                inserted = r.snapshots_inserted,
                updated = r.snapshots_updated,
                holdings = r.holdings_upserted,
                "valuation feed import ok"
            );
        }
        Err(e) => warn!(path = %path.display(), "valuation feed import failed: {e}"),
    }
}

fn combined_notes(row: &FeedRow) -> Option<String> {
    let mut parts = Vec::new();
    if !row.source_evidence.is_empty() {
        parts.push(row.source_evidence.clone());
    }
    if let Some(r) = row.fx_rate {
        let date = row.fx_date.as_deref().unwrap_or(&row.valuation_date);
        parts.push(format!("GBPUSD {r} on {date} (pair quote; not applied as multiply-to-GBP)"));
    }
    if !row.notes.is_empty() {
        parts.push(row.notes.clone());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" | "))
    }
}

fn default_item_type(class: &str) -> &'static str {
    match class {
        "pension" => "sipp",
        "trading" => "trading_account",
        _ => "other_investment",
    }
}

fn split_provider(provider_reference: &str, record_name: &str) -> (Option<String>, Option<String>) {
    let raw = if provider_reference.trim().is_empty() {
        record_name
    } else {
        provider_reference
    };
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("Global Prime") {
        let login = rest.trim();
        (
            Some("Global Prime".into()),
            if login.is_empty() {
                None
            } else {
                Some(login.to_string())
            },
        )
    } else if raw.is_empty() {
        (None, None)
    } else {
        (Some(raw.to_string()), None)
    }
}

fn map_owner(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "john" | "personal" => "personal".into(),
        "claire" | "joint" => "joint".into(),
        "remixmix" | "remixmix ltd" | "limited_company" | "limited company" => {
            "limited_company".into()
        }
        other => other.to_string(),
    }
}

fn is_isa_name(name: &str) -> bool {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t.eq_ignore_ascii_case("isa"))
}

fn is_cash_holding(ticker: Option<&str>, holding_name: &str) -> bool {
    if ticker
        .map(|t| t.trim().eq_ignore_ascii_case("CASH"))
        .unwrap_or(false)
    {
        return true;
    }
    holding_name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t.eq_ignore_ascii_case("cash"))
}

fn norm_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn holding_match_key(ticker: Option<&str>, holding_name: &str) -> String {
    nonempty(ticker.unwrap_or(""))
        .map(|t| t.to_ascii_uppercase())
        .unwrap_or_else(|| norm_name(holding_name))
}

fn nonempty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn parse_number(raw: &str) -> Result<Option<f64>, FinanceError> {
    let t = raw.trim().replace(',', "");
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>()
        .map(Some)
        .map_err(|_| FinanceError::invalid(format!("not a number: {raw}")))
}

pub fn parse_feed_date(raw: &str) -> Result<String, FinanceError> {
    let t = raw.trim();
    let normalized = t.replace('.', "-").replace('/', "-");
    let date = NaiveDate::parse_from_str(&normalized, "%Y-%m-%d").map_err(|_| {
        FinanceError::invalid(format!("valuation_date must be YYYY-MM-DD (got {raw})"))
    })?;
    Ok(date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use crate::finance;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    const SAMPLE: &str = r#"record_name,class,owner_entity,provider_reference,parent_asset,currency,native_value,gbp_value,fx_rate,fx_date,valuation_date,source_evidence,confidence,holding_name,units,price,cost_basis,notes
AJ Bell SIPP,pension,John,AJ Bell,,GBP,52792.30,52792.30,1.35027480,2026-09-02,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,,,,,Account roll-up. Child holding rows must not be double-counted.
ITWN.L,investment,John,AJ Bell,AJ Bell SIPP,GBP,6235.28,6235.28,,,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,iShares MSCI Taiwan ETF USD Dist GBP,41,152.08,5892.52,sector=taiwan
SEMG.L,investment,John,AJ Bell,AJ Bell SIPP,GBP,5610.17,5610.17,,,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,Amundi MSCI Semiconductors ETF Acc GBP,61,91.97,5562.89,sector=semiconductors
LLY,investment,John,AJ Bell,AJ Bell SIPP,USD,5800.00,4295.42,1.35027480,2026-09-02,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,Eli Lilly and Co,5,1160,6282.06,sector=healthcare
ISPY.L,investment,John,AJ Bell,AJ Bell SIPP,GBP,5063.03,5063.03,,,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,L&G Cyber Security ETF GBP,151,33.53,4941.55,sector=tech
HSBA.L,investment,John,AJ Bell,AJ Bell SIPP,GBP,4584.87,4584.87,,,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,HSBC Holdings PLC,299,15.33400024,4555.32,sector=financials
UNH,investment,John,AJ Bell,AJ Bell SIPP,USD,5548.20,4108.94,1.35027480,2026-09-02,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,UnitedHealth Group Inc,14,396.29998779,5562.51,sector=healthcare
ARM,investment,John,AJ Bell,AJ Bell SIPP,USD,7749.06,5738.88,1.35027480,2026-09-02,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,ARM,33,234.82000732,8444.09,sector=growth
GOOGL,investment,John,AJ Bell,AJ Bell SIPP,USD,6365.38,4714.14,1.35027480,2026-09-02,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,GOOGL,19,335.01998901,6351.72,sector=growth
CASH,investment,John,AJ Bell,AJ Bell SIPP,GBP,12441.58,12441.58,,,2026-09-02,trading-database sipp_portfolio_state.json (Batty TM P3 book),confirmed,AJ Bell SIPP cash,,,,Uninvested cash in the SIPP book.
Global Prime 1103757,trading,John,Global Prime 1103757,,GBP,58132.59,58132.59,,,2026.09.03,Bielsa ExportActualsCSV account_snapshot (AccountEquity),confirmed,,,,,equity (not balance). balance=57181.29; open_positions=23
Global Prime 84007275,trading,John,Global Prime 84007275,,GBP,1074.07,1074.07,,,2026.09.03,Bielsa ExportActualsCSV account_snapshot (AccountEquity),confirmed,,,,,equity (not balance). balance=1424.32; open_positions=11
AJ Bell ISA,investment,John,AJ Bell,,GBP,8,8,,,2026-09-02,should-not-import,confirmed,,,,,cash
"#;

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

    #[test]
    fn dotted_date_normalises() {
        assert_eq!(parse_feed_date("2026.09.03").unwrap(), "2026-09-03");
        assert_eq!(parse_feed_date("2026-09-02").unwrap(), "2026-09-02");
    }

    #[test]
    fn isa_token_detected() {
        assert!(is_isa_name("AJ Bell ISA"));
        assert!(!is_isa_name("AJ Bell SIPP"));
    }

    #[test]
    fn cash_holding_detected() {
        assert!(is_cash_holding(Some("CASH"), "AJ Bell SIPP cash"));
        assert!(is_cash_holding(None, "Uninvested cash"));
        assert!(!is_cash_holding(Some("LLY"), "Eli Lilly"));
    }

    #[tokio::test]
    async fn sample_feed_matches_locked_rules() {
        let db = test_db().await;
        let sipp = db
            .create_financial_item(
                "AJ Bell SIPP",
                "pension",
                "sipp",
                "personal",
                Some("AJ Bell"),
                None,
                None,
                "GBP",
                true,
                None,
            )
            .await
            .unwrap();
        db.create_valuation(
            sipp.id,
            "2026-09-02",
            52_792.30,
            "GBP",
            None,
            None,
            None,
            "other",
            Some("Speed dashboard"),
            "provisional",
            None,
            None,
            Some("provisional until GBP feed"),
        )
        .await
        .unwrap();
        db.create_financial_item(
            "AJ Bell ISA",
            "investment",
            "isa",
            "personal",
            Some("AJ Bell"),
            None,
            None,
            "GBP",
            false,
            None,
        )
        .await
        .unwrap();
        db.create_valuation(
            db.list_financial_items()
                .await
                .unwrap()
                .iter()
                .find(|i| i.name == "AJ Bell ISA")
                .unwrap()
                .id,
            "2026-09-02",
            8.0,
            "GBP",
            None,
            None,
            None,
            "manual_estimate",
            Some("cash only"),
            "estimated",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        db.create_financial_item(
            "Global Prime 1103757",
            "trading",
            "trading_account",
            "personal",
            Some("Global Prime"),
            Some("1103757"),
            None,
            "GBP",
            false,
            None,
        )
        .await
        .unwrap();
        db.create_financial_item(
            "Global Prime 84007275",
            "trading",
            "trading_account",
            "personal",
            Some("Global Prime"),
            Some("84007275"),
            None,
            "GBP",
            false,
            None,
        )
        .await
        .unwrap();

        let report = apply_csv(&db, SAMPLE).await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.items_created, 0);
        assert_eq!(report.snapshots_inserted, 3); // SIPP confirmed + two GP
        assert_eq!(report.holdings_upserted, 9);
        assert_eq!(report.rows_skipped, 1); // ISA

        let sipp_snaps = db.list_valuations(sipp.id).await.unwrap();
        assert_eq!(sipp_snaps.len(), 2, "provisional history kept");
        assert_eq!(sipp_snaps[0].gbp_value, 52_792.30);
        assert_eq!(sipp_snaps[0].confidence, "confirmed");
        assert_eq!(sipp_snaps[0].source_reference.as_deref(), Some(FEED_SOURCE_PREFIX));
        assert_eq!(sipp_snaps[0].contributions, None);
        assert_eq!(sipp_snaps[0].withdrawals, None);
        assert_eq!(sipp_snaps[1].confidence, "provisional");

        let holds = db.list_item_holdings(sipp.id).await.unwrap();
        assert_eq!(holds.len(), 9);
        let lly = holds.iter().find(|h| h.ticker_or_identifier.as_deref() == Some("LLY")).unwrap();
        assert!((lly.gbp_value - 4295.42).abs() < 1e-6);
        assert_eq!(lly.currency, "USD");
        assert!((lly.native_value - 5800.0).abs() < 1e-6);

        let cash = holds
            .iter()
            .find(|h| h.ticker_or_identifier.as_deref() == Some("CASH"))
            .unwrap();
        assert!((cash.gbp_value - 12_441.58).abs() < 1e-6);
        assert_eq!(
            cash.cost_basis,
            Some(12_441.58),
            "cash cost equals native value so P&L is zero"
        );

        let world = db.load_finance_world().await.unwrap();
        let nw = finance::compute_net_worth(&world, None);
        let sipp_line = nw
            .counted
            .iter()
            .find(|c| c.financial_item_id == Some(sipp.id))
            .unwrap();
        assert_eq!(sipp_line.source, "investment_holdings");
        assert!((sipp_line.gbp_value - 52_792.30).abs() < 1.0);
        assert!(!nw.counted.iter().any(|c| c.name.contains("ISA") && (c.gbp_value - 8.0).abs() > 1.0 && c.source == "investment_holdings"));

        let isa = db
            .list_financial_items()
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.name == "AJ Bell ISA")
            .unwrap();
        let isa_snaps = db.list_valuations(isa.id).await.unwrap();
        assert_eq!(isa_snaps.len(), 1);
        assert_eq!(isa_snaps[0].gbp_value, 8.0);
        assert_eq!(isa_snaps[0].source_type, "manual_estimate");

        let gp = db
            .list_financial_items()
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.name == "Global Prime 1103757")
            .unwrap();
        let gp_snaps = db.list_valuations(gp.id).await.unwrap();
        assert_eq!(gp_snaps.len(), 1);
        assert_eq!(gp_snaps[0].valuation_date, "2026-09-03");
        assert!((gp_snaps[0].gbp_value - 58_132.59).abs() < 1e-6);

        // Same-day reimport updates the feed snapshot, does not duplicate it.
        let report2 = apply_csv(&db, SAMPLE).await.unwrap();
        assert_eq!(report2.snapshots_inserted, 0);
        assert_eq!(report2.snapshots_updated, 3);
        assert_eq!(db.list_valuations(sipp.id).await.unwrap().len(), 2);
        assert_eq!(db.list_item_holdings(sipp.id).await.unwrap().len(), 9);

        // Login omitted from file: do not write £0.
        let without_small_gp = SAMPLE
            .lines()
            .filter(|l| !l.starts_with("Global Prime 84007275"))
            .collect::<Vec<_>>()
            .join("\n");
        apply_csv(&db, &without_small_gp).await.unwrap();
        let gp2 = db
            .list_financial_items()
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.name == "Global Prime 84007275")
            .unwrap();
        let snaps2 = db.list_valuations(gp2.id).await.unwrap();
        assert_eq!(snaps2.len(), 1);
        assert!((snaps2[0].gbp_value - 1074.07).abs() < 1e-6);
    }

    #[tokio::test]
    async fn rollup_cashflow_zero_is_stored_holding_blank_is_none() {
        let db = test_db().await;
        db.create_financial_item(
            "AJ Bell SIPP",
            "pension",
            "sipp",
            "personal",
            Some("AJ Bell"),
            None,
            None,
            "GBP",
            true,
            None,
        )
        .await
        .unwrap();
        db.create_financial_item(
            "Global Prime 1103757",
            "trading",
            "trading_account",
            "personal",
            Some("Global Prime"),
            Some("1103757"),
            None,
            "GBP",
            false,
            None,
        )
        .await
        .unwrap();
        let csv = r#"record_name,class,owner_entity,provider_reference,parent_asset,currency,native_value,gbp_value,fx_rate,fx_date,valuation_date,source_evidence,confidence,holding_name,units,price,cost_basis,notes,contributions,withdrawals
AJ Bell SIPP,pension,John,AJ Bell,,GBP,66155.42,66155.42,,,2026-09-08,batty,confirmed,,,,,roll-up,0.00,0.00
CASH,investment,John,AJ Bell,AJ Bell SIPP,GBP,100,100,,,2026-09-08,batty,confirmed,cash,,,,,
Global Prime 1103757,trading,John,Global Prime 1103757,,GBP,60167.16,60167.16,,,2026-09-08,batty,confirmed,,,,,equity,2083.00,0.00
"#;
        apply_csv(&db, csv).await.unwrap();
        let items = db.list_financial_items().await.unwrap();
        let sipp = items.iter().find(|i| i.name == "AJ Bell SIPP").unwrap();
        let gp = items
            .iter()
            .find(|i| i.name == "Global Prime 1103757")
            .unwrap();
        let sipp_snap = &db.list_valuations(sipp.id).await.unwrap()[0];
        assert_eq!(sipp_snap.contributions, Some(0.0));
        assert_eq!(sipp_snap.withdrawals, Some(0.0));
        let gp_snap = &db.list_valuations(gp.id).await.unwrap()[0];
        assert_eq!(gp_snap.contributions, Some(2083.0));
        assert_eq!(gp_snap.withdrawals, Some(0.0));
    }

    #[tokio::test]
    async fn missing_login_is_not_created_as_zero() {
        let db = test_db().await;
        db.create_financial_item(
            "AJ Bell SIPP",
            "pension",
            "sipp",
            "personal",
            Some("AJ Bell"),
            None,
            None,
            "GBP",
            true,
            None,
        )
        .await
        .unwrap();
        let csv = "record_name,class,owner_entity,provider_reference,parent_asset,currency,native_value,gbp_value,fx_rate,fx_date,valuation_date,source_evidence,confidence,holding_name,units,price,cost_basis,notes\n\
AJ Bell SIPP,pension,John,AJ Bell,,GBP,100,100,,,2026-09-01,book,confirmed,,,,,\n";
        apply_csv(&db, csv).await.unwrap();
        let names: Vec<String> = db
            .list_financial_items()
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert!(!names.iter().any(|n| n.contains("Global Prime")));
        assert_eq!(names.iter().filter(|n| n.as_str() == "AJ Bell SIPP").count(), 1);
    }
}
