//! Payee-history categorisation.
//!
//! Mapping is derived from existing categorised transactions (and from each manual
//! `PATCH` of `category_id`). There is no separate mapping table: auto-apply never
//! writes a remember that could flip a mixed payee.
//!
//! Rules still win: callers apply the regex engine first and only consult this
//! module when no rule set a category.

use crate::db::Db;
use crate::models::{Category, Transaction};
use crate::rules::{self, CompiledRules};
use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum History {
    Unanimous(i64),
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewReason {
    Transfer,
    Mixed,
    NoHistory,
    Ready,
}

impl ReviewReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Mixed => "mixed",
            Self::NoHistory => "no_history",
            Self::Ready => "ready",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Apply(i64),
    Skip(ReviewReason),
}

#[derive(Debug, Clone)]
pub struct LearnEngine {
    history: HashMap<String, History>,
    account_names: Vec<String>,
}

impl LearnEngine {
    pub async fn from_db(db: &Db) -> Result<Self> {
        let account_names = db.list_account_display_names().await?;
        let rows = db.list_payee_history().await?;
        Ok(Self::from_parts(account_names, &rows))
    }

    pub fn from_parts(
        account_names: Vec<String>,
        rows: &[(Option<String>, String, i64)],
    ) -> Self {
        let account_names = account_names
            .into_iter()
            .map(|n| normalize(&n))
            .filter(|n| n.len() >= 4)
            .collect();
        let mut history: HashMap<String, History> = HashMap::new();
        for (merchant, description, category_id) in rows {
            let sig = payee_signature(merchant.as_deref(), description);
            observe_map(&mut history, &sig, *category_id);
        }
        Self {
            history,
            account_names,
        }
    }

    pub fn history_for(&self, merchant: Option<&str>, description: &str) -> Option<History> {
        self.history.get(&payee_signature(merchant, description)).copied()
    }

    pub fn is_transfer_like(&self, merchant: Option<&str>, description: &str) -> bool {
        is_transfer_like(merchant, description, &self.account_names)
    }

    pub fn is_always_manual(&self, merchant: Option<&str>, description: &str) -> bool {
        is_always_manual(merchant, description)
    }

    /// Learned category for a new/uncategorised txn. Never returns a value for
    /// mixed history, no history, transfer-like, or always-manual payees.
    pub fn verdict(&self, merchant: Option<&str>, description: &str) -> Verdict {
        if self.is_transfer_like(merchant, description) {
            return Verdict::Skip(ReviewReason::Transfer);
        }
        if self.is_always_manual(merchant, description) {
            // PayPal eBay (and similar) bucket unrelated purchases under one MID.
            return Verdict::Skip(ReviewReason::Mixed);
        }
        match self.history_for(merchant, description) {
            Some(History::Unanimous(cid)) => Verdict::Apply(cid),
            Some(History::Mixed) => Verdict::Skip(ReviewReason::Mixed),
            None => Verdict::Skip(ReviewReason::NoHistory),
        }
    }

    pub fn review_reason(&self, merchant: Option<&str>, description: &str) -> ReviewReason {
        match self.verdict(merchant, description) {
            Verdict::Skip(reason) => reason,
            Verdict::Apply(_) => ReviewReason::Ready,
        }
    }

    /// Record a categorised txn so later rows in the same batch can use it.
    /// Does not flip Mixed back to Unanimous.
    pub fn observe(&mut self, merchant: Option<&str>, description: &str, category_id: i64) {
        let sig = payee_signature(merchant, description);
        observe_map(&mut self.history, &sig, category_id);
    }

    /// Category to persist for a new txn: rule first, else learned unanimous.
    /// Always-manual payees skip learned history and global rules; an
    /// account-scoped rule (e.g. Capital On Tap eBay → IT Hardware) still files.
    pub fn category_for_new(
        &self,
        txn: &Transaction,
        rules: &CompiledRules,
    ) -> (Option<i64>, bool) {
        let always_manual = is_always_manual(txn.merchant_name.as_deref(), &txn.description);
        if let Some(cid) = rules::winning_category(txn, rules, always_manual) {
            return (Some(cid), true);
        }
        if always_manual {
            return (None, false);
        }
        match self.verdict(txn.merchant_name.as_deref(), &txn.description) {
            Verdict::Apply(cid) => (Some(cid), false),
            Verdict::Skip(_) => (None, false),
        }
    }
}

fn observe_map(history: &mut HashMap<String, History>, sig: &str, category_id: i64) {
    if sig.is_empty() {
        return;
    }
    match history.get(sig) {
        None => {
            history.insert(sig.to_string(), History::Unanimous(category_id));
        }
        Some(History::Unanimous(existing)) if *existing == category_id => {}
        Some(History::Unanimous(_)) => {
            history.insert(sig.to_string(), History::Mixed);
        }
        Some(History::Mixed) => {}
    }
}

/// Identity used for learning/review.
///
/// For Wise/card lines `Card transaction of <amount> <ccy> issued by <company>`,
/// the signature is the company after `issued by` — never the amount. That
/// wrapper wins even when `merchant_name` is present (the UI title is the
/// wrapper; merchant is not the discriminator). Merchant is used only when it
/// is a real name: not empty, and not itself a copy of the wrapper.
pub fn payee_signature(merchant_name: Option<&str>, description: &str) -> String {
    normalize(&payee_label(merchant_name, description))
}

/// Display form of the payee (collapsed whitespace, original case).
pub fn payee_label(merchant_name: Option<&str>, description: &str) -> String {
    if let Some(name) = family_pocket_label(merchant_name, description) {
        return name.to_string();
    }
    let raw = if let Some(company) = issued_by_company(description) {
        tighten_issued_by(&company)
    } else if let Some(m) = merchant_name {
        if let Some(company) = issued_by_company(m) {
            tighten_issued_by(&company)
        } else {
            let n = collapse_ws(m);
            if !n.is_empty() {
                n
            } else {
                description.to_string()
            }
        }
    } else {
        description.to_string()
    };
    collapse_ws(&strip_volatile_suffixes(&raw))
}

/// Smile `TFR 110394…` / `TFR 110049…` are pocket money to John's sons, not
/// a second Tally account. Relabel and keep them off the transfer queue.
const FAMILY_POCKET: &[(&str, &str)] = &[("110394", "The Lucas"), ("110049", "The Jacob")];

fn family_pocket_label(merchant: Option<&str>, description: &str) -> Option<&'static str> {
    family_pocket_in(description).or_else(|| merchant.and_then(family_pocket_in))
}

fn family_pocket_in(s: &str) -> Option<&'static str> {
    for token in s.split(|c: char| !c.is_ascii_digit()) {
        if token.len() < 10 {
            continue;
        }
        for (prefix, name) in FAMILY_POCKET {
            if token.starts_with(prefix) {
                return Some(name);
            }
        }
    }
    None
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize(s: &str) -> String {
    collapse_ws(s).to_lowercase()
}

/// True when `account_name` appears as a whole phrase in the description
/// (`Smile` in `SMILE RENTS HOUSEHOLD`). Names shorter than 4 characters are ignored.
pub fn description_mentions_name(description: &str, account_name: &str) -> bool {
    let name = normalize(account_name);
    if name.len() < 4 {
        return false;
    }
    let hay = format!(" {} ", normalize(description));
    hay.contains(&format!(" {name} "))
}

/// Drop volatile trailing tokens (payment refs, billing period dates) so monthly
/// variants of the same payee share one signature.
fn strip_volatile_suffixes(s: &str) -> String {
    let mut out = s.to_string();
    for _ in 0..8 {
        let next = strip_trailing_household(&strip_trailing_city(&strip_trailing_country(
            &strip_trailing_bank_ref(&strip_fee_paren(&strip_trailing_slash_ref(
                &strip_trailing_period_label(&strip_trailing_ref_token(&out)),
            ))),
        )));
        if next == out {
            break;
        }
        out = next;
    }
    out
}

/// Drop trailing payment/reference tokens such as `3089c643f622a556PL` on
/// `Payward Ltd. 3089c643f622a556PL`, so each code does not become a new payee.
/// Requires a long hex body (12+); short account numbers are left alone.
fn strip_trailing_ref_token(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\s+[0-9]*[a-f][0-9a-f]{10,}[a-z]{0,4}$").expect("trailing ref")
    });
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Drop trailing billing-period dates: `- Jul 2026`, `07/2026`, `2026-07`.
fn strip_trailing_period_label(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            ^(.+?)
            (?:\s*[-–—]\s*|\s+)
            (?:
                (?:jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|jun(?:e)?|jul(?:y)?|
                   aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)
                \s+\d{4}
              | \d{1,2}/\d{4}
              | \d{4}-\d{2}
            )
            \s*$",
        )
        .expect("trailing period")
    });
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Drop ` / ref: 484149566` style numeric/code refs. Keeps word refs like
/// `/ ref: TopUp` and `/ ref: Salary` (no digit in the token).
fn strip_trailing_slash_ref(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\s*/\s*ref:\s*([0-9A-Za-z]+)\s*$").expect("slash ref")
    });
    if let Some(caps) = re.captures(s.trim()) {
        let token = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if token.chars().any(|c| c.is_ascii_digit()) {
            let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    s.to_string()
}

/// Drop trailing `(fee: 0.08 USD)` so fee-marked card wrappers share the company.
fn strip_fee_paren(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\s*\(fee:\s*[^)]+\)\s*$").expect("fee paren"));
    collapse_ws(&re.replace(s.trim(), "").into_owned())
}

/// Smile/CSV bank narratives: `KCOM GROUP LTD 3351530-1784313965`,
/// `SHEILAS WHEELS DR 0082945420-190726`, `PC/INTELLIGENT INS 04AEKX4683/022/105`.
/// Leaves short store numbers (`TESCO STORES 1234`) and 10-digit ids (`Clickup 8886254258`).
fn strip_trailing_bank_ref(s: &str) -> String {
    static DASHED: OnceLock<Regex> = OnceLock::new();
    static SLASHED: OnceLock<Regex> = OnceLock::new();
    let dashed = DASHED.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\s+[0-9]{6,}-[0-9]{4,}\s*$").expect("dashed bank ref")
    });
    let slashed = SLASHED.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\s+[0-9A-Z]{6,}(?:/[0-9A-Z]{2,})+\s*$").expect("slashed bank ref")
    });
    let t = s.trim();
    if let Some(caps) = dashed.captures(t).or_else(|| slashed.captures(t)) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Trailing country token on card/CSV lines (`BBM BARGAINS HULL GB`).
fn strip_trailing_country(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)^(.+?)\s+(?:GB|UK|IE)\s*$").expect("country"));
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Co-op/Smile `John Lancaster HOUSEHOLD` is the same payee as `John Lancaster`.
fn strip_trailing_household(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)^(.+?)\s+household\s*$").expect("household"));
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Tighten an issued-by company: drop fee, trailing UK city, Ltd/Limited, and .co.uk.
/// `Just Eat.co.uk Ltd London` and `Just Eat London` both become `Just Eat`.
fn tighten_issued_by(s: &str) -> String {
    let mut out = strip_fee_paren(s);
    out = strip_trailing_city(&out);
    out = strip_legal_suffix(&out);
    out = strip_attached_tld(&out);
    collapse_ws(&out)
}

fn strip_trailing_city(s: &str) -> String {
    static CITIES: OnceLock<Regex> = OnceLock::new();
    let re = CITIES.get_or_init(|| {
        Regex::new(
            r"(?ix)
            ^(.+?)\s+
            (?:london|hull|sheffield|leeds|manchester|birmingham|bristol|liverpool|glasgow|
               edinburgh|cardiff|belfast|york|oxford|cambridge|nottingham|leicester|coventry|
               newcastle|brighton|reading|southampton|portsmouth|plymouth|derby|wolverhampton|
               stoke|sunderland|wolverhampton|cumbria|skipton|troutbeck|bowness|newlands|
               windermere)
            \s*$",
        )
        .expect("city")
    });
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

fn strip_legal_suffix(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)(?:\s+|\.)(?:ltd|limited|plc|inc|llc|llp)\.?\s*$").expect("legal")
    });
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

fn strip_attached_tld(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\.(?:co\.uk|com|net|org|uk)\s*$").expect("tld")
    });
    if let Some(caps) = re.captures(s.trim()) {
        let base = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Company after `issued by` for `Card transaction of <amount> <ccy> issued by …`.
fn issued_by_company(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)^card\s+transaction\s+of\s+-?\d[\d,]*(?:\.\d+)?\s+[a-z]{3}\s+issued\s+by\s+(.+)$",
        )
        .expect("card wrapper")
    });
    re.captures(text.trim())
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Payees that must never auto-file: one signature covers many unrelated buys.
/// `PAYPAL *EBAY UK 795653703 GB` (and other PayPal eBay MIDs) stay on review.
pub fn is_always_manual(merchant: Option<&str>, description: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)paypal\s*\*\s*ebay").expect("paypal ebay always-manual")
    });
    re.is_match(description) || merchant.is_some_and(|m| re.is_match(m))
}

fn is_transfer_like(merchant: Option<&str>, description: &str, account_names: &[String]) -> bool {
    if family_pocket_label(merchant, description).is_some() {
        return false;
    }
    static KEYWORDS: OnceLock<Regex> = OnceLock::new();
    let keywords = KEYWORDS.get_or_init(|| {
        Regex::new(r"(?i)\b(?:transfer|top-?up|top\s+up|vrp|converted|tfr)\b")
            .expect("transfer keywords")
    });
    let hay_desc = description;
    let hay_merch = merchant.unwrap_or("");
    if keywords.is_match(hay_desc) || keywords.is_match(hay_merch) {
        return true;
    }
    let sig = payee_signature(merchant, description);
    if sig.is_empty() {
        return false;
    }
    account_names.iter().any(|name| {
        sig == *name || description_mentions_name(description, name) || description_mentions_name(hay_merch, name)
    })
}

pub fn transfer_tree_ids(categories: &[Category]) -> HashSet<i64> {
    let transfer_roots: Vec<i64> = categories
        .iter()
        .filter(|c| c.name.eq_ignore_ascii_case("transfer"))
        .map(|c| c.id)
        .collect();
    if transfer_roots.is_empty() {
        return HashSet::new();
    }
    let mut ids: HashSet<i64> = transfer_roots.iter().copied().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for c in categories {
            if let Some(pid) = c.parent_id {
                if ids.contains(&pid) && ids.insert(c.id) {
                    changed = true;
                }
            }
        }
    }
    ids
}

/// Apply learned (and unmatched-rule) categories to every currently uncategorised txn.
pub async fn apply_uncategorised(db: &Db) -> Result<ApplyStats> {
    let rules_raw = db.list_rules(true).await.unwrap_or_default();
    let compiled = CompiledRules::compile(rules_raw);
    let mut engine = LearnEngine::from_db(db).await?;
    let uncat = db.list_uncategorised_transactions().await?;
    let mut stats = ApplyStats {
        scanned: uncat.len() as i64,
        ..Default::default()
    };
    for t in uncat {
        let (cid, from_rule) = engine.category_for_new(&t, &compiled);
        if let Some(cid) = cid {
            db.update_transaction_category(t.id, Some(cid)).await?;
            engine.observe(t.merchant_name.as_deref(), &t.description, cid);
            if from_rule {
                stats.applied_rules += 1;
            } else {
                stats.applied_learned += 1;
            }
            continue;
        }
        match engine.review_reason(t.merchant_name.as_deref(), &t.description) {
            ReviewReason::Transfer => stats.skipped_transfer += 1,
            ReviewReason::Mixed => stats.skipped_mixed += 1,
            ReviewReason::NoHistory => stats.skipped_no_history += 1,
            ReviewReason::Ready => {}
        }
    }
    Ok(stats)
}

/// After a manual PATCH, fill other uncategorised txns with the same signature
/// only when history is now unanimous (and not transfer-like or always-manual).
/// Rules still win for those siblings if a rule matches.
pub async fn remember_after_patch(db: &Db, patched: &Transaction) -> Result<i64> {
    let Some(cid) = patched.category_id else {
        return Ok(0);
    };
    let rules_raw = db.list_rules(true).await.unwrap_or_default();
    let compiled = CompiledRules::compile(rules_raw);
    let mut engine = LearnEngine::from_db(db).await?;
    engine.observe(patched.merchant_name.as_deref(), &patched.description, cid);
    if engine.is_transfer_like(patched.merchant_name.as_deref(), &patched.description) {
        return Ok(0);
    }
    if engine.is_always_manual(patched.merchant_name.as_deref(), &patched.description) {
        return Ok(0);
    }
    let Some(History::Unanimous(unanimous)) =
        engine.history_for(patched.merchant_name.as_deref(), &patched.description)
    else {
        return Ok(0);
    };
    let target_sig = payee_signature(patched.merchant_name.as_deref(), &patched.description);
    let uncat = db.list_uncategorised_transactions().await?;
    let mut applied = 0i64;
    for t in uncat {
        if t.id == patched.id {
            continue;
        }
        let sig = payee_signature(t.merchant_name.as_deref(), &t.description);
        if sig != target_sig {
            continue;
        }
        if engine.is_transfer_like(t.merchant_name.as_deref(), &t.description) {
            continue;
        }
        if engine.is_always_manual(t.merchant_name.as_deref(), &t.description) {
            continue;
        }
        let (chosen, _) = engine.category_for_new(&t, &compiled);
        let apply_id = match chosen {
            Some(rule_or_learned) => rule_or_learned,
            None => unanimous,
        };
        db.update_transaction_category(t.id, Some(apply_id)).await?;
        engine.observe(t.merchant_name.as_deref(), &t.description, apply_id);
        applied += 1;
    }
    Ok(applied)
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ApplyStats {
    pub scanned: i64,
    pub applied_rules: i64,
    pub applied_learned: i64,
    pub skipped_mixed: i64,
    pub skipped_transfer: i64,
    pub skipped_no_history: i64,
}

pub fn review_items(
    engine: &LearnEngine,
    uncategorised: Vec<Transaction>,
) -> Vec<(Transaction, ReviewReason, String, String)> {
    uncategorised
        .into_iter()
        .map(|t| {
            let merchant = t.merchant_name.as_deref();
            let sig = payee_signature(merchant, &t.description);
            let label = payee_label(merchant, &t.description);
            let reason = engine.review_reason(merchant, &t.description);
            (t, reason, sig, label)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use crate::db::Db;
    use crate::models::Rule;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    fn sig(merchant: Option<&str>, desc: &str) -> String {
        payee_signature(merchant, desc)
    }

    #[test]
    fn signature_prefers_merchant() {
        assert_eq!(sig(Some("Tesco"), "CARD PAYMENT TESCO STORES"), "tesco");
        assert_eq!(sig(Some("  Tesco  Extra  "), "ignored"), "tesco extra");
    }

    #[test]
    fn signature_falls_back_to_normalised_description() {
        assert_eq!(sig(None, "  TESCO   STORES  "), "tesco stores");
        assert_eq!(sig(Some("   "), "Waitrose"), "waitrose");
        assert_eq!(sig(Some(""), "Waitrose"), "waitrose");
    }

    #[test]
    fn signature_is_case_insensitive() {
        assert_eq!(sig(Some("TESCO"), "x"), sig(Some("tesco"), "x"));
        assert_eq!(sig(None, "KCOM Group Ltd"), sig(None, "kcom group ltd"));
    }

    #[test]
    fn transfer_like_when_description_names_an_account() {
        let names = vec!["smile".into(), "tsb main account".into()];
        assert!(is_transfer_like(None, "SMILE RENTS HOUSEHOLD", &names));
        assert!(!is_transfer_like(None, "TESCO, HULL", &names));
        assert!(is_transfer_like(None, "TFR SMILE SAVINGS", &names));
        assert!(!is_transfer_like(None, "TFR 11039418019262", &names));
        assert!(!is_transfer_like(None, "TFR 11004913166968", &names));
    }

    #[test]
    fn signature_strips_card_transaction_wrapper() {
        let just_eat_21 =
            "Card transaction of 21.67 GBP issued by Just Eat.co.uk Ltd London";
        let just_eat_9 =
            "Card transaction of 9.40 GBP issued by Just Eat.co.uk Ltd London";
        assert_eq!(sig(None, just_eat_21), "just eat");
        assert_eq!(sig(None, just_eat_9), "just eat");
        assert_eq!(sig(None, just_eat_21), sig(None, just_eat_9));
        assert_eq!(
            sig(
                None,
                "Card transaction of 21.31 GBP issued by Just Eat London"
            ),
            "just eat"
        );
        assert_eq!(
            sig(
                None,
                "Card transaction of 17.20 GBP issued by Just Eat London (fee: 0.08 USD)"
            ),
            "just eat"
        );
        assert_ne!(sig(None, just_eat_21), "21.67 gbp");
        assert_eq!(
            sig(
                None,
                "CARD TRANSACTION OF 12.00 EUR ISSUED BY Pret A Manger"
            ),
            "pret a manger"
        );
        // Wrapper wins even when merchant is present — merchant is not the discriminator.
        assert_eq!(sig(Some("Pret"), just_eat_21), "just eat");
        // Merchant that is itself a copy of the wrapper still yields the company.
        assert_eq!(sig(Some(just_eat_21), "other narrative"), "just eat");
        assert_eq!(payee_label(None, just_eat_21), "Just Eat");
        assert_eq!(
            sig(
                None,
                "Card transaction of -99.00 USD issued by Clickup 8886254258"
            ),
            "clickup 8886254258"
        );
    }

    #[test]
    fn signature_strips_trailing_hex_reference_codes() {
        let a = "Payward Ltd. 3089c643f622a556PL";
        let b = "Payward Ltd. e9694a5bbd92fb44PL";
        assert_eq!(sig(None, a), "payward ltd.");
        assert_eq!(sig(None, b), "payward ltd.");
        assert_eq!(sig(None, a), sig(None, b));
        assert_eq!(payee_label(None, a), "Payward Ltd.");
        // Short trailing numbers must not be stripped (Clickup account ids, etc.).
        assert_eq!(
            sig(None, "Card transaction of 10.00 USD issued by Clickup 8886254258"),
            "clickup 8886254258"
        );
        assert_eq!(sig(None, "TESCO STORES 1234"), "tesco stores 1234");
    }

    #[test]
    fn signature_strips_trailing_billing_period_dates() {
        let admin_jul = "ref: Admin Extra subscription - Jul 2026";
        let admin_aug = "ref: Admin Extra subscription - Aug 2026";
        let payroll_jul = "ref: Payroll subscription - Jul 2026";
        let payroll_jun = "ref: Payroll subscription - Jun 2026";
        assert_eq!(sig(None, admin_jul), "ref: admin extra subscription");
        assert_eq!(sig(None, admin_aug), sig(None, admin_jul));
        assert_eq!(sig(None, payroll_jul), "ref: payroll subscription");
        assert_eq!(sig(None, payroll_jun), sig(None, payroll_jul));
        assert_ne!(sig(None, admin_jul), sig(None, payroll_jul));
        assert_eq!(
            payee_label(None, admin_jul),
            "ref: Admin Extra subscription"
        );
        assert_eq!(
            sig(None, "REMIXMIX LTD Salary 07/2026"),
            "remixmix ltd salary"
        );
        assert_eq!(
            sig(None, "REMIXMIX LTD Salary 06/2026"),
            sig(None, "REMIXMIX LTD Salary 07/2026")
        );
        assert_eq!(
            sig(None, "John lancaster / ref: Salary 07/2026"),
            "john lancaster / ref: salary"
        );
    }

    #[test]
    fn signature_strips_numeric_slash_refs() {
        assert_eq!(sig(None, "HMRC VAT / ref: 484149566"), "hmrc vat");
        assert_eq!(
            sig(None, "HMRC VAT / ref: 484149566"),
            sig(None, "HMRC VAT / ref: 999888777")
        );
        assert_eq!(payee_label(None, "HMRC VAT / ref: 484149566"), "HMRC VAT");
        assert_eq!(sig(None, "HMRC SDDS / ref: 0000688529"), "hmrc sdds");
        assert_eq!(
            sig(None, "PAYPAL PAYMENT / ref: 43XJ222G99LXL"),
            "paypal payment"
        );
        // Word refs stay — they discriminate the payee.
        assert_eq!(
            sig(None, "John Lancaster / ref: TopUp"),
            "john lancaster / ref: topup"
        );
        assert_eq!(
            sig(None, "John lancaster / ref: Salary"),
            "john lancaster / ref: salary"
        );
    }

    #[test]
    fn signature_strips_smile_csv_payment_refs_and_place() {
        assert_eq!(
            sig(None, "KCOM GROUP LTD     3351530-1784313965"),
            "kcom group ltd"
        );
        assert_eq!(
            sig(None, "KCOM GROUP LTD     3351530-1787165155"),
            sig(None, "KCOM GROUP LTD     3351530-1784313965")
        );
        assert_eq!(
            sig(None, "SHEILAS WHEELS DR  0082945420-190726"),
            "sheilas wheels dr"
        );
        assert_eq!(
            sig(None, "SHEILAS WHEELS DR  0082945420-190826"),
            sig(None, "SHEILAS WHEELS DR  0082945420-190726")
        );
        assert_eq!(
            sig(None, "PC/INTELLIGENT INS 04AEKX4683/022/105"),
            "pc/intelligent ins"
        );
        assert_eq!(
            sig(None, "PC/INTELLIGENT INS 04AEKX4683/022/106"),
            sig(None, "PC/INTELLIGENT INS 04AEKX4683/022/105")
        );
        assert_eq!(sig(None, "BBM BARGAINS Hull GB"), "bbm bargains");
        assert_eq!(sig(None, "BBM BARGAINS"), "bbm bargains");
        assert_eq!(
            sig(None, "John Lancaster     HOUSEHOLD"),
            "john lancaster"
        );
        assert_eq!(
            sig(None, "John Lancaster     HOUSEHOLD"),
            sig(None, "John Lancaster")
        );
        // Sons' Co-op pockets: same person shares a signature; the two boys stay distinct.
        assert_eq!(sig(None, "TFR 11004913166968"), "the jacob");
        assert_eq!(sig(None, "TFR 11039418019262"), "the lucas");
        assert_eq!(
            sig(None, "TFR 11004913166968"),
            sig(None, "TFR 11004999999999")
        );
        assert_ne!(
            sig(None, "TFR 11004913166968"),
            sig(None, "TFR 11039418019262")
        );
        assert_eq!(payee_label(None, "TFR 11039418019262"), "The Lucas");
        assert_eq!(payee_label(None, "TFR 11004913166968"), "The Jacob");
        assert_eq!(sig(None, "TESCO STORES 1234"), "tesco stores 1234");
        assert_eq!(
            sig(
                None,
                "Card transaction of 10.00 USD issued by Clickup 8886254258"
            ),
            "clickup 8886254258"
        );
    }

    #[test]
    fn wrapper_amounts_share_learned_category() {
        let engine = LearnEngine::from_parts(
            vec![],
            &[(
                None,
                "Card transaction of 21.67 GBP issued by Just Eat.co.uk Ltd London".into(),
                42,
            )],
        );
        assert_eq!(
            engine.verdict(
                None,
                "Card transaction of 9.40 GBP issued by Just Eat.co.uk Ltd London"
            ),
            Verdict::Apply(42)
        );
        assert_eq!(
            engine.verdict(
                Some("ignored merchant"),
                "Card transaction of 3.50 GBP issued by Just Eat.co.uk Ltd London"
            ),
            Verdict::Apply(42)
        );
    }

    #[test]
    fn unanimous_history_applies_mixed_does_not() {
        let engine = LearnEngine::from_parts(
            vec![],
            &[
                (Some("Tesco".into()), "TESCO".into(), 10),
                (Some("Tesco".into()), "TESCO STORES".into(), 10),
                (Some("Shell".into()), "SHELL".into(), 20),
                (Some("Shell".into()), "SHELL PETROL".into(), 21),
            ],
        );
        assert_eq!(
            engine.verdict(Some("Tesco"), "TESCO HULL"),
            Verdict::Apply(10)
        );
        assert_eq!(
            engine.verdict(Some("Shell"), "SHELL"),
            Verdict::Skip(ReviewReason::Mixed)
        );
        assert_eq!(
            engine.verdict(Some("New Payee"), "BRAND NEW"),
            Verdict::Skip(ReviewReason::NoHistory)
        );
    }

    #[test]
    fn observing_a_second_category_makes_mixed_and_stays_mixed() {
        let mut engine = LearnEngine::from_parts(vec![], &[(Some("X".into()), "X".into(), 1)]);
        assert_eq!(engine.verdict(Some("X"), "X"), Verdict::Apply(1));
        engine.observe(Some("X"), "X", 2);
        assert_eq!(engine.verdict(Some("X"), "X"), Verdict::Skip(ReviewReason::Mixed));
        engine.observe(Some("X"), "X", 1);
        assert_eq!(
            engine.verdict(Some("X"), "X"),
            Verdict::Skip(ReviewReason::Mixed),
            "one later pick must not silently overwrite mixed history"
        );
    }

    #[test]
    fn transfer_like_never_auto_files() {
        let engine = LearnEngine::from_parts(
            vec!["Nationwide Current".into(), "Smile".into()],
            &[
                (None, "TopUp".into(), 99),
                (None, "nationwide current".into(), 99),
            ],
        );
        assert_eq!(
            engine.verdict(None, "TopUp"),
            Verdict::Skip(ReviewReason::Transfer)
        );
        assert_eq!(
            engine.verdict(None, "VRP payment"),
            Verdict::Skip(ReviewReason::Transfer)
        );
        assert_eq!(
            engine.verdict(None, "Transfer to savings"),
            Verdict::Skip(ReviewReason::Transfer)
        );
        assert_eq!(
            engine.verdict(None, "Nationwide Current"),
            Verdict::Skip(ReviewReason::Transfer)
        );
        assert_eq!(
            engine.verdict(None, "TFR SMILE SAVINGS"),
            Verdict::Skip(ReviewReason::Transfer)
        );
        assert_eq!(
            engine.verdict(None, "TFR 11004913166968"),
            Verdict::Skip(ReviewReason::NoHistory)
        );
        assert_eq!(
            engine.verdict(None, "TFR 11039418019262"),
            Verdict::Skip(ReviewReason::NoHistory)
        );
        assert_eq!(
            engine.verdict(Some("TESCO"), "TESCO"),
            Verdict::Skip(ReviewReason::NoHistory)
        );
    }

    #[test]
    fn paypal_ebay_is_always_manual() {
        assert!(is_always_manual(
            None,
            "PAYPAL *EBAY UK 795653703 GB"
        ));
        assert!(is_always_manual(
            Some("PAYPAL *EBAY UK 35314369001"),
            "card payment"
        ));
        assert!(!is_always_manual(None, "eBay O*01-13666-76547 INTERNET GB"));
        assert!(!is_always_manual(None, "PAYPAL *TOOLSTATION 08081007211 GB"));
    }

    #[test]
    fn paypal_ebay_never_auto_files_even_with_unanimous_history() {
        let engine = LearnEngine::from_parts(
            vec![],
            &[(
                None,
                "PAYPAL *EBAY UK 795653703 GB".into(),
                10,
            )],
        );
        assert_eq!(
            engine.verdict(None, "PAYPAL *EBAY UK 795653703 GB"),
            Verdict::Skip(ReviewReason::Mixed)
        );
    }

    #[test]
    fn paypal_ebay_skips_matching_rules() {
        let engine = LearnEngine::from_parts(vec![], &[]);
        let compiled = CompiledRules::compile(vec![rule(1, "PAYPAL", 77)]);
        let t = txn("PAYPAL *EBAY UK 795653703 GB", None, None);
        let (cid, from_rule) = engine.category_for_new(&t, &compiled);
        assert_eq!(cid, None);
        assert!(!from_rule);
    }

    #[test]
    fn paypal_ebay_account_scoped_rule_applies() {
        let engine = LearnEngine::from_parts(vec![], &[]);
        let mut scoped = rule(1, "ebay", 122);
        scoped.match_account_id = Some(30);
        let compiled = CompiledRules::compile(vec![rule(2, "PAYPAL", 77), scoped]);
        let mut t = txn("PAYPAL *EBAY UK - 795653703", None, None);
        t.account_id = 30;
        let (cid, from_rule) = engine.category_for_new(&t, &compiled);
        assert_eq!(cid, Some(122));
        assert!(from_rule);
        t.account_id = 29;
        let (cid, from_rule) = engine.category_for_new(&t, &compiled);
        assert_eq!(cid, None);
        assert!(!from_rule);
    }

    #[test]
    fn transfer_tree_collects_parent_and_leaves() {
        let cats = vec![
            Category {
                id: 1,
                name: "Transfer".into(),
                parent_id: None,
                icon: None,
                colour: None,
                created_at: 0,
            },
            Category {
                id: 2,
                name: "Internal".into(),
                parent_id: Some(1),
                icon: None,
                colour: None,
                created_at: 0,
            },
            Category {
                id: 3,
                name: "Groceries".into(),
                parent_id: None,
                icon: None,
                colour: None,
                created_at: 0,
            },
        ];
        let ids = transfer_tree_ids(&cats);
        assert!(ids.contains(&1) && ids.contains(&2));
        assert!(!ids.contains(&3));
    }

    fn rule(id: i64, desc_re: &str, category_id: i64) -> Rule {
        Rule {
            id,
            name: "r".into(),
            enabled: 1,
            priority: 10,
            match_description_regex: Some(desc_re.into()),
            match_merchant_regex: None,
            match_min_amount_cents: None,
            match_max_amount_cents: None,
            match_account_id: None,
            match_is_credit: None,
            set_category_id: Some(category_id),
            add_tag_ids: None,
            set_notes: None,
            times_applied: 0,
            last_applied_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn txn(desc: &str, merchant: Option<&str>, category_id: Option<i64>) -> Transaction {
        Transaction {
            id: 0,
            account_id: 1,
            provider_txn_id: "x".into(),
            timestamp: 1,
            description: desc.into(),
            amount_cents: 100,
            currency: "GBP".into(),
            is_credit: 0,
            is_pending: 0,
            merchant_name: merchant.map(|s| s.to_string()),
            counterparty_iban: None,
            counterparty_name: None,
            category_id,
            notes: None,
            raw_json: None,
            transfer_pair_id: None,
            transfer_unmatchable: 0,
            transfer_await_account_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn rules_win_over_learned_mapping() {
        let engine = LearnEngine::from_parts(
            vec![],
            &[(Some("Tesco".into()), "TESCO".into(), 10)],
        );
        let compiled = CompiledRules::compile(vec![rule(1, "TESCO", 77)]);
        let t = txn("TESCO HULL", Some("Tesco"), None);
        let (cid, from_rule) = engine.category_for_new(&t, &compiled);
        assert_eq!(cid, Some(77));
        assert!(from_rule);
    }

    #[test]
    fn learned_applies_when_no_rule_matches() {
        let engine = LearnEngine::from_parts(
            vec![],
            &[(Some("Tesco".into()), "TESCO".into(), 10)],
        );
        let compiled = CompiledRules::compile(vec![rule(1, "SHELL", 77)]);
        let t = txn("TESCO HULL", Some("Tesco"), None);
        let (cid, from_rule) = engine.category_for_new(&t, &compiled);
        assert_eq!(cid, Some(10));
        assert!(!from_rule);
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

    async fn seed_account(db: &Db) -> i64 {
        let c = db
            .upsert_consent("bank", "creds", None, None, "at", "rt", 1, None, "accounts")
            .await
            .unwrap();
        db.upsert_account(c.id, "acc", "account", "Current", None, None, None, None, "GBP")
            .await
            .unwrap()
            .id
    }

    async fn category_named(db: &Db, name: &str) -> i64 {
        db.list_categories()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("seeded category {name}"))
            .id
    }

    #[tokio::test]
    async fn remember_on_patch_applies_to_matching_uncategorised() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let groceries = category_named(&db, "Groceries").await;
        db.upsert_transaction(
            aid, "a", 1, "TESCO", 500, "GBP", false, false, Some("Tesco"), None, None,
            None, None, None,
        )
        .await
        .unwrap();
        let first = db
            .upsert_transaction(
                aid, "b", 2, "TESCO HULL", 700, "GBP", false, false, Some("Tesco"), None, None,
                None, None, None,
            )
            .await
            .unwrap();
        db.update_transaction_category(first, Some(groceries))
            .await
            .unwrap();
        let patched = db.get_transaction(first).await.unwrap().unwrap();
        let n = remember_after_patch(&db, &patched).await.unwrap();
        assert_eq!(n, 1, "the earlier uncategorised Tesco should be remembered");
        let a = db
            .list_transactions(None, None, None, None, None, None, None, Some("TESCO"), 10, 0)
            .await
            .unwrap();
        assert!(a.iter().all(|t| t.category_id == Some(groceries)));
    }

    #[tokio::test]
    async fn remember_on_patch_applies_to_issued_by_wrappers_of_any_amount() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let eating = category_named(&db, "Eating Out").await;
        let first = db
            .upsert_transaction(
                aid,
                "je1",
                1,
                "Card transaction of 21.67 GBP issued by Just Eat.co.uk Ltd London",
                2167,
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
        let second = db
            .upsert_transaction(
                aid,
                "je2",
                2,
                "Card transaction of 9.40 GBP issued by Just Eat.co.uk Ltd London",
                940,
                "GBP",
                false,
                false,
                Some("ignored merchant"),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        db.update_transaction_category(first, Some(eating))
            .await
            .unwrap();
        let patched = db.get_transaction(first).await.unwrap().unwrap();
        let n = remember_after_patch(&db, &patched).await.unwrap();
        assert_eq!(n, 1, "other Just Eat wrapper amounts should be remembered");
        assert_eq!(
            db.get_transaction(second).await.unwrap().unwrap().category_id,
            Some(eating)
        );
    }

    #[tokio::test]
    async fn remember_on_patch_applies_across_trailing_ref_codes() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let cat = category_named(&db, "Eating Out").await;
        let first = db
            .upsert_transaction(
                aid,
                "pw1",
                1,
                "Payward Ltd. 3089c643f622a556PL",
                17500,
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
        let second = db
            .upsert_transaction(
                aid,
                "pw2",
                2,
                "Payward Ltd. e9694a5bbd92fb44PL",
                35000,
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
        db.update_transaction_category(first, Some(cat))
            .await
            .unwrap();
        let patched = db.get_transaction(first).await.unwrap().unwrap();
        let n = remember_after_patch(&db, &patched).await.unwrap();
        assert_eq!(n, 1, "other Payward ref codes should share the signature");
        assert_eq!(
            db.get_transaction(second).await.unwrap().unwrap().category_id,
            Some(cat)
        );
    }

    #[tokio::test]
    async fn remember_on_patch_applies_across_billing_period_dates() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let cat = category_named(&db, "Subscriptions").await;
        let first = db
            .upsert_transaction(
                aid,
                "ae1",
                1,
                "ref: Admin Extra subscription - Jul 2026",
                2999,
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
        let second = db
            .upsert_transaction(
                aid,
                "ae2",
                2,
                "ref: Admin Extra subscription - Aug 2026",
                2999,
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
        let other = db
            .upsert_transaction(
                aid,
                "pr1",
                3,
                "ref: Payroll subscription - Jul 2026",
                1440,
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
        db.update_transaction_category(first, Some(cat))
            .await
            .unwrap();
        let patched = db.get_transaction(first).await.unwrap().unwrap();
        let n = remember_after_patch(&db, &patched).await.unwrap();
        assert_eq!(n, 1, "other months of Admin Extra should be remembered");
        assert_eq!(
            db.get_transaction(second).await.unwrap().unwrap().category_id,
            Some(cat)
        );
        assert_eq!(
            db.get_transaction(other).await.unwrap().unwrap().category_id,
            None,
            "Payroll is a different payee"
        );
    }

    #[tokio::test]
    async fn remember_on_patch_does_not_flip_mixed_payee() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let groceries = category_named(&db, "Groceries").await;
        let eating = category_named(&db, "Eating Out").await;
        db.upsert_transaction(
            aid, "a", 1, "TESCO", 500, "GBP", false, false, Some("Tesco"), None, None,
            Some(groceries), None, None,
        )
        .await
        .unwrap();
        db.upsert_transaction(
            aid, "b", 2, "TESCO CAFE", 700, "GBP", false, false, Some("Tesco"), None, None,
            Some(eating), None, None,
        )
        .await
        .unwrap();
        let third = db
            .upsert_transaction(
                aid, "c", 3, "TESCO HULL", 300, "GBP", false, false, Some("Tesco"), None, None,
                None, None, None,
            )
            .await
            .unwrap();
        db.update_transaction_category(third, Some(groceries))
            .await
            .unwrap();
        let patched = db.get_transaction(third).await.unwrap().unwrap();
        let n = remember_after_patch(&db, &patched).await.unwrap();
        assert_eq!(n, 0, "mixed history must not cascade");
        let extra = db
            .upsert_transaction(
                aid, "d", 4, "TESCO EXTRA", 100, "GBP", false, false, Some("Tesco"), None, None,
                None, None, None,
            )
            .await
            .unwrap();
        let stats = apply_uncategorised(&db).await.unwrap();
        assert_eq!(stats.applied_learned, 0);
        assert!(stats.skipped_mixed >= 1);
        let still = db.get_transaction(extra).await.unwrap().unwrap();
        assert_eq!(still.category_id, None);
    }

    #[tokio::test]
    async fn remember_on_patch_does_not_cascade_paypal_ebay() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let groceries = category_named(&db, "Groceries").await;
        let desc = "PAYPAL *EBAY UK 795653703 GB";
        db.upsert_transaction(
            aid, "a", 1, desc, 500, "GBP", false, false, None, None, None,
            None, None, None,
        )
        .await
        .unwrap();
        let first = db
            .upsert_transaction(
                aid, "b", 2, desc, 1200, "GBP", false, false, None, None, None,
                None, None, None,
            )
            .await
            .unwrap();
        db.update_transaction_category(first, Some(groceries))
            .await
            .unwrap();
        let patched = db.get_transaction(first).await.unwrap().unwrap();
        let n = remember_after_patch(&db, &patched).await.unwrap();
        assert_eq!(n, 0, "PayPal eBay must stay per-row manual");
        let leftover = db
            .list_transactions(None, None, None, None, None, None, None, Some("EBAY"), 10, 0)
            .await
            .unwrap();
        let uncat = leftover.iter().filter(|t| t.category_id.is_none()).count();
        assert_eq!(uncat, 1);
        let stats = apply_uncategorised(&db).await.unwrap();
        assert_eq!(stats.applied_learned, 0);
        assert_eq!(stats.applied_rules, 0);
    }

    #[tokio::test]
    async fn apply_uncategorised_rules_win_then_learned() {
        let db = test_db().await;
        let aid = seed_account(&db).await;
        let groceries = category_named(&db, "Groceries").await;
        let bills = category_named(&db, "Bills").await;
        db.upsert_transaction(
            aid, "hist", 1, "TESCO", 100, "GBP", false, false, Some("Tesco"), None, None,
            Some(groceries), None, None,
        )
        .await
        .unwrap();
        db.create_rule(
            "kcom",
            10,
            Some("KCOM"),
            None,
            None,
            None,
            None,
            None,
            Some(bills),
            None,
            None,
        )
        .await
        .unwrap();
        let tesco_new = db
            .upsert_transaction(
                aid, "t2", 2, "TESCO HULL", 200, "GBP", false, false, Some("Tesco"), None, None,
                None, None, None,
            )
            .await
            .unwrap();
        let kcom = db
            .upsert_transaction(
                aid, "k1", 3, "KCOM GROUP LTD", 4099, "GBP", false, false, None, None, None,
                None, None, None,
            )
            .await
            .unwrap();
        let stats = apply_uncategorised(&db).await.unwrap();
        assert_eq!(stats.applied_learned, 1);
        assert_eq!(stats.applied_rules, 1);
        assert_eq!(
            db.get_transaction(tesco_new).await.unwrap().unwrap().category_id,
            Some(groceries)
        );
        assert_eq!(
            db.get_transaction(kcom).await.unwrap().unwrap().category_id,
            Some(bills),
            "regex rule must win over any learned mapping"
        );
    }
}
