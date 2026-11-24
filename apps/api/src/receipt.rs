//! Package C receipt intake.
//!
//! A generic trigger event carries a photo and which account scope it belongs to.
//! The first adapter is Immich tags: `Business receipt` and `Personal receipt`.
//! It only lists tags, searches by tag id, and downloads the original. It does not
//! create, update, or untag. The poller is off unless `TALLY_IMMICH_RECEIPTS=1`.
//!
//! New photos are found with a seen-set of asset ids. `updatedAfter` is not the signal.
//! Two solid-colour test photos are ignored and are not stored as receipts.

use crate::db::Db;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const BUSINESS_RECEIPT_TAG_ID: &str = "43050eb5-7eec-46b2-bef0-43dee0f43cc2";
pub const PERSONAL_RECEIPT_TAG_ID: &str = "fef84dd6-7e92-4203-a237-c3303e6c5942";

const BUSINESS_RECEIPT_TAG_NAME: &str = "Business receipt";
const PERSONAL_RECEIPT_TAG_NAME: &str = "Personal receipt";

/// Solid-colour leftovers from the tag-creation check. Not receipts.
const TEST_LEFTOVER_ASSETS: &[&str] = &[
    "6e29124b-4e42-4c12-b5fb-bc6c2c50ab12",
    "51432b4d-652b-446c-b817-4bca17be83e2",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountScope {
    Business,
    Personal,
}

impl AccountScope {
    fn as_str(self) -> &'static str {
        match self {
            AccountScope::Business => "business",
            AccountScope::Personal => "personal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentKind {
    Cash,
    Account,
    Unknown,
}

impl PaymentKind {
    fn as_str(self) -> &'static str {
        match self {
            PaymentKind::Cash => "cash",
            PaymentKind::Account => "account",
            PaymentKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReceipt {
    pub amount_cents: Option<i64>,
    pub date_utc: Option<i64>,
    pub supplier: Option<String>,
    pub payment: PaymentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchCandidate {
    pub transaction_id: i64,
    pub amount_cents: i64,
    pub timestamp: i64,
    pub scope: AccountScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchDecision {
    Matched { transaction_id: i64 },
    Pending { status: &'static str },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PollReport {
    pub ingested: usize,
    pub skipped_seen: usize,
    pub skipped_test: usize,
    pub errors: usize,
}

struct TagSpec {
    id: &'static str,
    name: &'static str,
    scope: AccountScope,
}

fn tag_specs() -> [TagSpec; 2] {
    [
        TagSpec {
            id: BUSINESS_RECEIPT_TAG_ID,
            name: BUSINESS_RECEIPT_TAG_NAME,
            scope: AccountScope::Business,
        },
        TagSpec {
            id: PERSONAL_RECEIPT_TAG_ID,
            name: PERSONAL_RECEIPT_TAG_NAME,
            scope: AccountScope::Personal,
        },
    ]
}

pub fn immich_receipts_enabled() -> bool {
    match std::env::var("TALLY_IMMICH_RECEIPTS") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

pub fn is_test_leftover(asset_id: &str) -> bool {
    TEST_LEFTOVER_ASSETS
        .iter()
        .any(|id| id.eq_ignore_ascii_case(asset_id))
}

/// Business when the account type starts with "business" or the name is Tide.
pub fn scope_of_account(
    account_type: Option<&str>,
    display_name: &str,
    custom_display_name: Option<&str>,
) -> AccountScope {
    if account_type
        .unwrap_or("")
        .to_ascii_lowercase()
        .starts_with("business")
    {
        return AccountScope::Business;
    }
    let custom = custom_display_name.unwrap_or("");
    let blob = format!("{display_name} {custom}").to_ascii_lowercase();
    if blob.contains("tide") {
        AccountScope::Business
    } else {
        AccountScope::Personal
    }
}

pub fn days_apart(txn_ts: i64, receipt_midnight_utc: i64) -> i64 {
    let txn_day = txn_ts.div_euclid(86_400);
    let receipt_day = receipt_midnight_utc.div_euclid(86_400);
    (txn_day - receipt_day).abs()
}

/// Exact amount, transaction date within ±3 calendar days, then account scope.
/// Cash is never linked. Zero or several hits stay pending.
pub fn decide(
    parsed: &ParsedReceipt,
    scope: AccountScope,
    candidates: &[MatchCandidate],
) -> MatchDecision {
    if parsed.payment == PaymentKind::Cash {
        return MatchDecision::Pending { status: "cash" };
    }
    let (Some(amount), Some(day)) = (parsed.amount_cents, parsed.date_utc) else {
        return MatchDecision::Pending {
            status: "unmatched",
        };
    };
    let mut hits = candidates.iter().filter(|c| {
        c.amount_cents == amount && c.scope == scope && days_apart(c.timestamp, day) <= 3
    });
    match (hits.next(), hits.next()) {
        (Some(hit), None) => MatchDecision::Matched {
            transaction_id: hit.transaction_id,
        },
        _ => MatchDecision::Pending {
            status: "unmatched",
        },
    }
}

pub fn parse_receipt(text: &str) -> ParsedReceipt {
    ParsedReceipt {
        amount_cents: parse_amount(text),
        date_utc: parse_date(text),
        supplier: parse_supplier(text),
        payment: parse_payment(text),
    }
}

fn parse_payment(text: &str) -> PaymentKind {
    let cash =
        Regex::new(r"(?i)paid by cash|payment(?:\s+method)?\s*:\s*cash|cash tender|cash sale")
            .expect("cash pattern");
    if cash.is_match(text) || text.lines().any(|l| l.trim().eq_ignore_ascii_case("cash")) {
        return PaymentKind::Cash;
    }
    let card = Regex::new(r"(?i)\b(visa|mastercard|maestro|amex|debit|credit card)\b")
        .expect("card pattern");
    if card.is_match(text) {
        PaymentKind::Account
    } else {
        PaymentKind::Unknown
    }
}

fn parse_amount(text: &str) -> Option<i64> {
    let money = Regex::new(r"(?i)(?:£|\bgbp\b\s*)?(\d{1,7})[.,](\d{2})\b").expect("money");
    let mut on_total: Option<i64> = None;
    let mut last: Option<i64> = None;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let is_total = (lower.contains("total")
            || lower.contains("amount due")
            || lower.contains("to pay")
            || lower.contains("balance due"))
            && !lower.contains("subtotal")
            && !lower.contains("sub-total");
        for caps in money.captures_iter(line) {
            let pounds: i64 = caps.get(1)?.as_str().parse().ok()?;
            let pence: i64 = caps.get(2)?.as_str().parse().ok()?;
            if pence > 99 {
                continue;
            }
            let cents = pounds * 100 + pence;
            last = Some(cents);
            if is_total {
                on_total = Some(cents);
            }
        }
    }
    on_total.or(last)
}

fn parse_date(text: &str) -> Option<i64> {
    let numeric = Regex::new(r"\b(\d{1,2})[./-](\d{1,2})[./-](\d{2,4})\b").expect("dmy");
    let named = Regex::new(
        r"(?i)\b(\d{1,2})\s+(jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\s+(\d{4})\b",
    )
    .expect("named");
    let mut preferred: Option<i64> = None;
    let mut first: Option<i64> = None;
    for line in text.lines() {
        let on_date_line = line.to_ascii_lowercase().contains("date");
        if let Some(caps) = numeric.captures(line) {
            if let Some(ts) = ymd_midnight(
                caps.get(3)?.as_str().parse().ok()?,
                caps.get(2)?.as_str().parse().ok()?,
                caps.get(1)?.as_str().parse().ok()?,
            ) {
                if first.is_none() {
                    first = Some(ts);
                }
                if on_date_line {
                    preferred = Some(ts);
                }
            }
        }
        if let Some(caps) = named.captures(line) {
            let month = month_index(caps.get(2)?.as_str())?;
            if let Some(ts) = ymd_midnight(
                caps.get(3)?.as_str().parse().ok()?,
                month,
                caps.get(1)?.as_str().parse().ok()?,
            ) {
                if first.is_none() {
                    first = Some(ts);
                }
                if on_date_line {
                    preferred = Some(ts);
                }
            }
        }
    }
    preferred.or(first)
}

fn ymd_midnight(mut year: i32, month: u32, day: u32) -> Option<i64> {
    if year < 100 {
        year += 2000;
    }
    Some(
        NaiveDate::from_ymd_opt(year, month, day)?
            .and_hms_opt(0, 0, 0)?
            .and_utc()
            .timestamp(),
    )
}

fn month_index(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    let n = n.as_str();
    Some(match n {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" | "sept" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

fn parse_supplier(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if t.len() < 3 {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("date")
            || lower.contains("total")
            || lower.contains("vat")
            || lower.contains("visa")
            || lower.contains("mastercard")
            || lower.contains("paid by")
            || lower.contains("receipt")
        {
            continue;
        }
        let letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
        if letters < 3 {
            continue;
        }
        return Some(t.chars().take(200).collect());
    }
    None
}

/// Body for `POST /api/search/metadata`. Tag ids only; no `updatedAfter`.
pub fn search_body(tag_id: &str, page: u32, cursor: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "tagIds": [tag_id],
        "size": 100,
        "page": page,
    });
    if let Some(cursor) = cursor {
        body["cursor"] = serde_json::json!(cursor);
    }
    body
}

pub struct ImmichConfig {
    pub base_url: String,
    pub api_key: String,
    pub receipt_dir: PathBuf,
}

impl ImmichConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("TALLY_IMMICH_API_KEY")
            .context("TALLY_IMMICH_API_KEY must be set when the receipt poller is enabled")?;
        if api_key.trim().is_empty() {
            anyhow::bail!("TALLY_IMMICH_API_KEY is empty");
        }
        let base_url = std::env::var("TALLY_IMMICH_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:2283".to_string());
        let receipt_dir =
            std::env::var("TALLY_RECEIPT_DIR").unwrap_or_else(|_| "/app/data/receipts".to_string());
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            receipt_dir: PathBuf::from(receipt_dir),
        })
    }
}

pub async fn poll_immich(db: &Db) -> Result<PollReport> {
    if !immich_receipts_enabled() {
        return Ok(PollReport::default());
    }
    let cfg = ImmichConfig::from_env()?;
    poll_with(db, &cfg).await
}

async fn poll_with(db: &Db, cfg: &ImmichConfig) -> Result<PollReport> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("immich http client")?;
    let tags = fetch_tags(&client, cfg).await?;
    let mut report = PollReport::default();
    for spec in tag_specs() {
        let Some(tag_id) = tags.iter().find_map(|tag| {
            if tag.id.eq_ignore_ascii_case(spec.id)
                && (tag.name == spec.name || tag.value == spec.name)
            {
                Some(tag.id.clone())
            } else {
                None
            }
        }) else {
            tracing::warn!(
                tag_id = spec.id,
                "receipt tag missing or renamed; not polling it"
            );
            continue;
        };
        let assets = search_tag(&client, cfg, &tag_id).await?;
        for asset_id in assets {
            if is_test_leftover(&asset_id) {
                report.skipped_test += 1;
                continue;
            }
            if db.receipt_already_known(&asset_id).await? {
                report.skipped_seen += 1;
                continue;
            }
            let meta = match fetch_asset(&client, cfg, &asset_id).await {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(asset_id = %asset_id, "receipt metadata failed: {e:#}");
                    report.errors += 1;
                    continue;
                }
            };
            let (bytes, content_type) = match download_original(&client, cfg, &meta).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(asset_id = %meta, "receipt download failed: {e:#}");
                    report.errors += 1;
                    continue;
                }
            };
            if let Err(e) = save_original(&cfg.receipt_dir, &meta, &bytes, &content_type) {
                tracing::warn!(asset_id = %meta, "receipt save failed: {e:#}");
            }
            let ocr_text = match ocr_image_bytes(&bytes) {
                Ok(text) => text,
                Err(e) if missing_tesseract(&e) => return Err(e),
                Err(e) => {
                    tracing::warn!(asset_id = %meta, "ocr failed: {e:#}");
                    String::new()
                }
            };
            let parsed = parse_receipt(&ocr_text);
            ingest_parsed(db, &meta, spec.scope, &parsed, "immich").await?;
            db.mark_receipt_asset_seen(&meta, &tag_id).await?;
            report.ingested += 1;
        }
    }
    Ok(report)
}

fn missing_tesseract(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}");
    text.contains("tesseract not found") || text.contains("No such file")
}

struct ImmichTag {
    id: String,
    name: String,
    value: String,
}

fn api_url(cfg: &ImmichConfig, path: &str) -> String {
    format!("{}{}", cfg.base_url, path)
}

async fn immich_send(
    client: &reqwest::Client,
    cfg: &ImmichConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<reqwest::Response> {
    let url = api_url(cfg, path);
    let mut req = client
        .request(method.clone(), &url)
        .header("x-api-key", &cfg.api_key);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("immich {method} {path}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("immich {method} {path} -> {}", resp.status());
    }
    Ok(resp)
}

async fn fetch_tags(client: &reqwest::Client, cfg: &ImmichConfig) -> Result<Vec<ImmichTag>> {
    let resp = immich_send(client, cfg, reqwest::Method::GET, "/api/tags", None).await?;
    let value: serde_json::Value = resp.json().await.context("immich tags json")?;
    let rows = value
        .as_array()
        .context("immich tags response was not a list")?;
    let mut out = Vec::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        out.push(ImmichTag {
            id: id.to_string(),
            name: row
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            value: row
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

async fn search_tag(
    client: &reqwest::Client,
    cfg: &ImmichConfig,
    tag_id: &str,
) -> Result<Vec<String>> {
    let mut page = 1u32;
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for _ in 0..100 {
        let body = search_body(tag_id, page, cursor.as_deref());
        debug_assert!(body.get("updatedAfter").is_none());
        let resp = immich_send(
            client,
            cfg,
            reqwest::Method::POST,
            "/api/search/metadata",
            Some(body),
        )
        .await?;
        let value: serde_json::Value = resp.json().await.context("immich search json")?;
        let items = value
            .pointer("/assets/items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if items.is_empty() {
            break;
        }
        let mut added = 0;
        for item in items {
            let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            if seen.insert(id.to_string()) {
                out.push(id.to_string());
                added += 1;
            }
        }
        if added == 0 {
            break;
        }
        let next_cursor = value
            .pointer("/assets/nextCursor")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let next_page = value
            .pointer("/assets/nextPage")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if next_cursor.is_none() && next_page.is_none() {
            break;
        }
        cursor = next_cursor;
        page += 1;
    }
    Ok(out)
}

async fn fetch_asset(
    client: &reqwest::Client,
    cfg: &ImmichConfig,
    asset_id: &str,
) -> Result<String> {
    let path = format!("/api/assets/{asset_id}");
    let resp = immich_send(client, cfg, reqwest::Method::GET, &path, None).await?;
    let value: serde_json::Value = resp.json().await.context("immich asset json")?;
    let id = value.get("id").and_then(|v| v.as_str()).unwrap_or(asset_id);
    Ok(id.to_string())
}

async fn download_original(
    client: &reqwest::Client,
    cfg: &ImmichConfig,
    asset_id: &str,
) -> Result<(Vec<u8>, String)> {
    let path = format!("/api/assets/{asset_id}/original");
    let resp = immich_send(client, cfg, reqwest::Method::GET, &path, None).await?;
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = resp.bytes().await.context("immich original body")?.to_vec();
    Ok((bytes, content_type))
}

fn save_original(dir: &Path, asset_id: &str, bytes: &[u8], content_type: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let ext = if content_type.contains("png") {
        "png"
    } else if content_type.contains("jpeg") || content_type.contains("jpg") {
        "jpg"
    } else {
        "bin"
    };
    let path = dir.join(format!("{asset_id}.{ext}"));
    std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn ocr_image_bytes(bytes: &[u8]) -> Result<String> {
    let prepared = prepare_image(bytes).unwrap_or_else(|_| bytes.to_vec());
    let path = std::env::temp_dir().join(format!(
        "tally-ocr-{}-{}.png",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::write(&path, prepared).context("write ocr temp file")?;
    let output = match std::process::Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .arg("--psm")
        .arg("6")
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e).context("tesseract not found");
        }
    };
    let _ = std::fs::remove_file(&path);
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("tesseract failed: {err}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn prepare_image(bytes: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes).context("decode receipt image")?;
    let img = if img.width().max(img.height()) > 2000 {
        img.thumbnail(2000, 2000)
    } else {
        img
    };
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .context("encode receipt png")?;
    Ok(buf)
}

async fn ingest_parsed(
    db: &Db,
    photo_id: &str,
    scope: AccountScope,
    parsed: &ParsedReceipt,
    source: &str,
) -> Result<()> {
    let candidates = match parsed.amount_cents {
        Some(amount) => db.list_open_debit_candidates(amount).await?,
        None => Vec::new(),
    };
    let decision = decide(parsed, scope, &candidates);
    let (status, linked) = match decision {
        MatchDecision::Matched { transaction_id } => {
            let linked = db.attach_receipt_photo(transaction_id, photo_id).await?;
            if linked {
                ("matched", Some(transaction_id))
            } else {
                ("unmatched", None)
            }
        }
        MatchDecision::Pending { status } => (status, None),
    };
    let flagged = status != "matched";
    db.store_receipt_intake(ReceiptIntake {
        photo_id,
        amount_cents: parsed.amount_cents,
        date_utc: parsed.date_utc,
        supplier: parsed.supplier.as_deref(),
        status,
        linked_transaction_id: linked,
        flagged,
        scope,
        payment: parsed.payment,
        source,
    })
    .await
}

struct ReceiptIntake<'a> {
    photo_id: &'a str,
    amount_cents: Option<i64>,
    date_utc: Option<i64>,
    supplier: Option<&'a str>,
    status: &'a str,
    linked_transaction_id: Option<i64>,
    flagged: bool,
    scope: AccountScope,
    payment: PaymentKind,
    source: &'a str,
}

impl Db {
    async fn receipt_already_known(&self, asset_id: &str) -> Result<bool> {
        let mut seen = self
            .conn
            .query(
                "SELECT 1 FROM receipt_asset_seen WHERE asset_id = ?1
                 UNION ALL
                 SELECT 1 FROM pending_receipt WHERE photo_id = ?1
                 LIMIT 1",
                libsql::params![asset_id],
            )
            .await?;
        Ok(seen.next().await?.is_some())
    }

    async fn mark_receipt_asset_seen(&self, asset_id: &str, tag_id: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO receipt_asset_seen (asset_id, tag_id, seen_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(asset_id) DO NOTHING",
                libsql::params![asset_id, tag_id, chrono::Utc::now().timestamp()],
            )
            .await?;
        Ok(())
    }

    async fn list_open_debit_candidates(&self, amount_cents: i64) -> Result<Vec<MatchCandidate>> {
        let mut rows = self
            .conn
            .query(
                "SELECT t.id, t.amount_cents, t.timestamp, a.account_type, a.display_name,
                        a.custom_display_name
                 FROM transactions t
                 JOIN accounts a ON a.id = t.account_id
                 WHERE t.is_credit = 0
                   AND t.amount_cents = ?1
                   AND t.receipt_photo_id IS NULL",
                libsql::params![amount_cents],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let account_type: Option<String> = row.get(3)?;
            let display_name: String = row.get(4)?;
            let custom: Option<String> = row.get(5)?;
            out.push(MatchCandidate {
                transaction_id: row.get(0)?,
                amount_cents: row.get(1)?,
                timestamp: row.get(2)?,
                scope: scope_of_account(account_type.as_deref(), &display_name, custom.as_deref()),
            });
        }
        Ok(out)
    }

    async fn attach_receipt_photo(&self, transaction_id: i64, photo_id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE transactions
                 SET receipt_photo_id = ?1, updated_at = ?2
                 WHERE id = ?3 AND receipt_photo_id IS NULL",
                libsql::params![photo_id, chrono::Utc::now().timestamp(), transaction_id],
            )
            .await?;
        Ok(n > 0)
    }

    async fn store_receipt_intake(&self, row: ReceiptIntake<'_>) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO pending_receipt (
                    photo_id, extracted_amount, extracted_date, status, linked_transaction_id,
                    created_at, updated_at, extracted_supplier, flagged_for_review, account_scope,
                    payment_kind, source
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                libsql::params![
                    row.photo_id,
                    row.amount_cents,
                    row.date_utc,
                    row.status,
                    row.linked_transaction_id,
                    now,
                    now,
                    row.supplier,
                    if row.flagged { 1_i64 } else { 0 },
                    row.scope.as_str(),
                    row.payment.as_str(),
                    row.source,
                ],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn candidate(id: i64, amount: i64, ts: i64, scope: AccountScope) -> MatchCandidate {
        MatchCandidate {
            transaction_id: id,
            amount_cents: amount,
            timestamp: ts,
            scope,
        }
    }

    fn parsed(amount: i64, day: i64, payment: PaymentKind) -> ParsedReceipt {
        ParsedReceipt {
            amount_cents: Some(amount),
            date_utc: Some(day),
            supplier: Some("ACME".into()),
            payment,
        }
    }

    #[test]
    fn exact_amount_within_three_days_matches() {
        let day = ymd_midnight(2026, 11, 12).unwrap();
        let hits = [candidate(7, 1850, day + 12 * 3600, AccountScope::Business)];
        assert_eq!(
            decide(
                &parsed(1850, day, PaymentKind::Account),
                AccountScope::Business,
                &hits
            ),
            MatchDecision::Matched { transaction_id: 7 }
        );
        assert_eq!(
            decide(
                &parsed(1850, day, PaymentKind::Account),
                AccountScope::Business,
                &[candidate(
                    8,
                    1850,
                    day + 3 * 86_400 + 3600,
                    AccountScope::Business
                )]
            ),
            MatchDecision::Matched { transaction_id: 8 }
        );
        assert_eq!(
            decide(
                &parsed(1850, day, PaymentKind::Unknown),
                AccountScope::Business,
                &[candidate(
                    9,
                    1850,
                    day - 3 * 86_400 + 3600,
                    AccountScope::Business
                )]
            ),
            MatchDecision::Matched { transaction_id: 9 }
        );
    }

    #[test]
    fn one_penny_or_four_days_or_wrong_account_is_pending() {
        let day = ymd_midnight(2026, 11, 12).unwrap();
        let scope = AccountScope::Business;
        let receipt = parsed(1850, day, PaymentKind::Account);
        assert_eq!(
            decide(&receipt, scope, &[candidate(1, 1851, day, scope)]),
            MatchDecision::Pending {
                status: "unmatched"
            }
        );
        assert_eq!(
            decide(&receipt, scope, &[candidate(1, 1849, day, scope)]),
            MatchDecision::Pending {
                status: "unmatched"
            }
        );
        assert_eq!(
            decide(
                &receipt,
                scope,
                &[candidate(1, 1850, day + 4 * 86_400, scope)]
            ),
            MatchDecision::Pending {
                status: "unmatched"
            }
        );
        assert_eq!(
            decide(
                &receipt,
                scope,
                &[candidate(1, 1850, day - 4 * 86_400, scope)]
            ),
            MatchDecision::Pending {
                status: "unmatched"
            }
        );
        assert_eq!(
            decide(
                &receipt,
                scope,
                &[candidate(1, 1850, day, AccountScope::Personal)]
            ),
            MatchDecision::Pending {
                status: "unmatched"
            }
        );
        assert_eq!(
            decide(
                &receipt,
                scope,
                &[
                    candidate(1, 1850, day, scope),
                    candidate(2, 1850, day + 86_400, scope)
                ]
            ),
            MatchDecision::Pending {
                status: "unmatched"
            }
        );
    }

    #[test]
    fn cash_is_pending_even_when_a_transaction_matches() {
        let day = ymd_midnight(2026, 6, 2).unwrap();
        let decision = decide(
            &parsed(420, day, PaymentKind::Cash),
            AccountScope::Personal,
            &[candidate(3, 420, day + 3600, AccountScope::Personal)],
        );
        assert_eq!(decision, MatchDecision::Pending { status: "cash" });
    }

    #[test]
    fn parser_reads_total_date_supplier_and_cash() {
        let card = parse_receipt(
            "ACME SUPPLIES LTD\nDate 12/11/2026\nPaper ream\nTOTAL GBP 18.50\nVISA\n",
        );
        assert_eq!(card.amount_cents, Some(1850));
        assert_eq!(card.date_utc, ymd_midnight(2026, 11, 12));
        assert_eq!(card.supplier.as_deref(), Some("ACME SUPPLIES LTD"));
        assert_eq!(card.payment, PaymentKind::Account);

        let cash =
            parse_receipt("MARKET STALL\nDate 02/06/2026\nFruit\nTOTAL GBP 4.20\nPAID BY CASH\n");
        assert_eq!(cash.amount_cents, Some(420));
        assert_eq!(cash.date_utc, ymd_midnight(2026, 6, 2));
        assert_eq!(cash.supplier.as_deref(), Some("MARKET STALL"));
        assert_eq!(cash.payment, PaymentKind::Cash);
    }

    #[test]
    fn search_body_uses_tag_ids_and_not_updated_after() {
        let body = search_body(BUSINESS_RECEIPT_TAG_ID, 1, None);
        assert_eq!(body["tagIds"][0], BUSINESS_RECEIPT_TAG_ID);
        assert!(body.get("updatedAfter").is_none());
        assert!(body.get("updatedBefore").is_none());
    }

    #[test]
    fn test_leftovers_are_not_receipts() {
        assert!(is_test_leftover("6e29124b-4e42-4c12-b5fb-bc6c2c50ab12"));
        assert!(is_test_leftover("51432b4d-652b-446c-b817-4bca17be83e2"));
        assert!(!is_test_leftover(BUSINESS_RECEIPT_TAG_ID));
    }

    #[test]
    fn tide_and_business_type_are_business_scope() {
        assert_eq!(
            scope_of_account(Some("business_current"), "Tide Current Account", None),
            AccountScope::Business
        );
        assert_eq!(
            scope_of_account(Some("TRANSACTION"), "Tide Current Account", None),
            AccountScope::Business
        );
        assert_eq!(
            scope_of_account(Some("current"), "Barclays Bank Account", None),
            AccountScope::Personal
        );
    }

    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64.encode([9u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        Db {
            conn: std::sync::Arc::new(conn),
            crypto,
        }
    }

    #[tokio::test]
    async fn ingest_links_exact_and_holds_penny_and_four_day_gaps() {
        let db = test_db().await;
        let tide = db
            .create_manual_account("Tide Current Account", "business_current", None)
            .await
            .unwrap();
        let day = ymd_midnight(2026, 11, 12).unwrap();
        let txn_id = db
            .upsert_transaction(
                tide.id,
                "tl-match",
                day + 3600,
                "ACME",
                1850,
                "GBP",
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let _off_day = db
            .upsert_transaction(
                tide.id,
                "tl-later",
                day + 4 * 86_400,
                "ACME LATER",
                740,
                "GBP",
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        ingest_parsed(
            &db,
            "photo-match",
            AccountScope::Business,
            &parsed(1850, day, PaymentKind::Account),
            "test",
        )
        .await
        .unwrap();
        ingest_parsed(
            &db,
            "photo-penny",
            AccountScope::Business,
            &parsed(1851, day, PaymentKind::Account),
            "test",
        )
        .await
        .unwrap();
        ingest_parsed(
            &db,
            "photo-later",
            AccountScope::Business,
            &parsed(740, day, PaymentKind::Account),
            "test",
        )
        .await
        .unwrap();

        let match_row = pending_row(&db, "photo-match").await;
        assert_eq!(match_row.status, "matched");
        assert_eq!(match_row.flagged, 0);
        assert_eq!(match_row.linked, Some(txn_id));
        let photo: String = scalar_string(
            &db,
            "SELECT receipt_photo_id FROM transactions WHERE id = ?1",
            txn_id,
        )
        .await;
        assert_eq!(photo, "photo-match");

        let penny = pending_row(&db, "photo-penny").await;
        assert_eq!(penny.status, "unmatched");
        assert_eq!(penny.flagged, 1);
        assert_eq!(penny.linked, None);

        let later = pending_row(&db, "photo-later").await;
        assert_eq!(later.status, "unmatched");
        assert_eq!(later.flagged, 1);
        assert_eq!(later.linked, None);
    }

    struct PendingRow {
        status: String,
        flagged: i64,
        linked: Option<i64>,
        amount: Option<i64>,
    }

    async fn scalar_string(db: &Db, sql: &str, id: i64) -> String {
        let mut rows = db.conn.query(sql, libsql::params![id]).await.unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    async fn pending_row(db: &Db, photo_id: &str) -> PendingRow {
        let mut rows = db
            .conn
            .query(
                "SELECT status, flagged_for_review, linked_transaction_id, extracted_amount
                 FROM pending_receipt WHERE photo_id = ?1",
                libsql::params![photo_id],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        PendingRow {
            status: row.get(0).unwrap(),
            flagged: row.get(1).unwrap(),
            linked: row.get(2).unwrap(),
            amount: row.get(3).unwrap(),
        }
    }

    #[derive(Clone)]
    struct MockImmich {
        hits: Arc<Mutex<Vec<String>>>,
        bodies: Arc<Mutex<Vec<serde_json::Value>>>,
        files: Arc<Vec<(String, Vec<u8>)>>,
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    async fn spawn_mock() -> (String, MockImmich) {
        let matched = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d".to_string();
        let awaiting = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5e".to_string();
        let cash = "c1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5f".to_string();
        let files = Arc::new(vec![
            (matched, fixture("matched-business.png")),
            (awaiting, fixture("awaiting-personal.png")),
            (cash, fixture("cash-personal.png")),
            ("6e29124b-4e42-4c12-b5fb-bc6c2c50ab12".into(), vec![1, 2, 3]),
            ("51432b4d-652b-446c-b817-4bca17be83e2".into(), vec![4, 5, 6]),
        ]);
        let state = MockImmich {
            hits: Arc::new(Mutex::new(Vec::new())),
            bodies: Arc::new(Mutex::new(Vec::new())),
            files,
        };
        let app = Router::new()
            .route("/api/tags", get(mock_tags))
            .route("/api/search/metadata", post(mock_search))
            .route("/api/assets/{id}/original", get(mock_original))
            .route("/api/assets/{id}", get(mock_asset))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), state)
    }

    fn allow(headers: &HeaderMap, hits: &Mutex<Vec<String>>, what: &str) -> bool {
        let ok = headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some("test-key");
        if ok {
            hits.lock().unwrap().push(what.to_string());
        } else {
            hits.lock().unwrap().push(format!("UNAUTH {what}"));
        }
        ok
    }

    async fn mock_tags(
        State(state): State<MockImmich>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, StatusCode> {
        if !allow(&headers, &state.hits, "GET /api/tags") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(Json(serde_json::json!([
            {"id": BUSINESS_RECEIPT_TAG_ID, "name": "Business receipt", "value": "Business receipt"},
            {"id": PERSONAL_RECEIPT_TAG_ID, "name": "Personal receipt", "value": "Personal receipt"}
        ])))
    }

    async fn mock_search(
        State(state): State<MockImmich>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<serde_json::Value>, StatusCode> {
        if !allow(&headers, &state.hits, "POST /api/search/metadata") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        state.bodies.lock().unwrap().push(body.clone());
        let tag = body["tagIds"][0].as_str().unwrap_or("");
        let ids: Vec<&str> = if tag == BUSINESS_RECEIPT_TAG_ID {
            vec![
                "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
                "6e29124b-4e42-4c12-b5fb-bc6c2c50ab12",
            ]
        } else if tag == PERSONAL_RECEIPT_TAG_ID {
            vec![
                "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5e",
                "c1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5f",
                "51432b4d-652b-446c-b817-4bca17be83e2",
            ]
        } else {
            vec![]
        };
        let items: Vec<_> = ids
            .into_iter()
            .map(|id| serde_json::json!({"id": id}))
            .collect();
        Ok(Json(serde_json::json!({
            "assets": {
                "total": items.len(),
                "count": items.len(),
                "items": items,
                "facets": [],
                "nextPage": null,
                "nextCursor": null
            }
        })))
    }

    async fn mock_asset(
        State(state): State<MockImmich>,
        headers: HeaderMap,
        Path(id): Path<String>,
    ) -> Result<Json<serde_json::Value>, StatusCode> {
        if !allow(&headers, &state.hits, "GET /api/assets/{id}") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(Json(serde_json::json!({"id": id})))
    }

    async fn mock_original(
        State(state): State<MockImmich>,
        headers: HeaderMap,
        Path(id): Path<String>,
    ) -> Result<Vec<u8>, StatusCode> {
        if !allow(&headers, &state.hits, "GET /api/assets/{id}/original") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        state
            .files
            .iter()
            .find(|(asset, _)| asset == &id)
            .map(|(_, bytes)| bytes.clone())
            .ok_or(StatusCode::NOT_FOUND)
    }

    #[tokio::test]
    async fn three_tagged_receipts_match_pending_and_cash_within_five_minutes() {
        let db = test_db().await;
        let tide = db
            .create_manual_account("Tide Current Account", "business_current", None)
            .await
            .unwrap();
        let personal = db
            .create_manual_account("Barclays Bank Account", "current", None)
            .await
            .unwrap();
        let day = ymd_midnight(2026, 11, 12).unwrap();
        let txn_id = db
            .upsert_transaction(
                tide.id,
                "tl-acme",
                day + 3 * 3600,
                "ACME SUPPLIES",
                1850,
                "GBP",
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        // Same amount and day as the cash receipt. Cash must not link to it.
        db.upsert_transaction(
            personal.id,
            "barc-cash-decoy",
            ymd_midnight(2026, 6, 2).unwrap() + 3600,
            "MARKET",
            420,
            "GBP",
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let (base, mock) = spawn_mock().await;
        let dir = std::env::temp_dir().join(format!("tally-receipts-{}", std::process::id()));
        let cfg = ImmichConfig {
            base_url: base,
            api_key: "test-key".into(),
            receipt_dir: dir,
        };
        let started = std::time::Instant::now();
        let first = poll_with(&db, &cfg).await.unwrap();
        assert!(
            started.elapsed().as_secs() < 300,
            "intake took {:?}",
            started.elapsed()
        );
        assert_eq!(first.ingested, 3, "{first:?}");
        assert_eq!(first.skipped_test, 2, "{first:?}");
        assert_eq!(first.errors, 0, "{first:?}");

        let matched = pending_row(&db, "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d").await;
        assert_eq!(matched.status, "matched");
        assert_eq!(matched.flagged, 0);
        assert_eq!(matched.linked, Some(txn_id));
        assert_eq!(matched.amount, Some(1850));
        let mut rows = db
            .conn
            .query(
                "SELECT receipt_photo_id FROM transactions WHERE provider_txn_id = 'tl-acme'",
                (),
            )
            .await
            .unwrap();
        let linked_photo: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(linked_photo, "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d");

        let awaiting = pending_row(&db, "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5e").await;
        assert_eq!(awaiting.status, "unmatched");
        assert_eq!(awaiting.flagged, 1);
        assert_eq!(awaiting.linked, None);
        assert_eq!(awaiting.amount, Some(740));

        let cash = pending_row(&db, "c1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5f").await;
        assert_eq!(cash.status, "cash");
        assert_eq!(cash.flagged, 1);
        assert_eq!(cash.linked, None);
        assert_eq!(cash.amount, Some(420));

        let mut leftover = db
            .conn
            .query(
                "SELECT count(*) FROM pending_receipt WHERE photo_id IN (?1, ?2)",
                libsql::params![
                    "6e29124b-4e42-4c12-b5fb-bc6c2c50ab12",
                    "51432b4d-652b-446c-b817-4bca17be83e2"
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            leftover
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            0
        );

        let second = poll_with(&db, &cfg).await.unwrap();
        assert_eq!(second.ingested, 0);
        assert_eq!(second.skipped_seen, 3);
        assert_eq!(second.skipped_test, 2);
        let mut count = db
            .conn
            .query("SELECT count(*) FROM pending_receipt", ())
            .await
            .unwrap();
        assert_eq!(
            count.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            3
        );

        let hits = mock.hits.lock().unwrap().clone();
        assert!(hits.iter().any(|h| h == "GET /api/tags"));
        assert!(hits.iter().any(|h| h == "POST /api/search/metadata"));
        assert!(hits.iter().any(|h| h == "GET /api/assets/{id}"));
        assert!(hits.iter().any(|h| h == "GET /api/assets/{id}/original"));
        assert!(hits.iter().all(|h| !h.starts_with("UNAUTH")));
        assert!(hits
            .iter()
            .all(|h| !h.contains("PUT") && !h.contains("DELETE")));
        let bodies = mock.bodies.lock().unwrap().clone();
        assert!(!bodies.is_empty());
        for body in bodies {
            assert!(body.get("updatedAfter").is_none());
            assert!(body["tagIds"].as_array().unwrap().len() == 1);
        }
    }
}
