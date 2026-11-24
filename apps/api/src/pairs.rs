//! Pair two bank legs of a transfer (same currency or FX). Both rows stay.

use crate::db::Db;
use crate::learn::{self, LearnEngine};
use crate::models::{Account, Category, Transaction};
use anyhow::{bail, Result};
use regex::Regex;
use std::sync::OnceLock;

const WINDOW_SECS: i64 = 3 * 86_400;
/// User-picked account: a bit wider than autosuggest so a weekend-delayed leg still shows.
const PAIR_WINDOW_SECS: i64 = 7 * 86_400;
/// After the user names the other account, wait this long for the CSV/sync leg.
const WAIT_WINDOW_SECS: i64 = 31 * 86_400;
/// Same-currency match: allow 2 minor units of rounding.
const SAME_CCY_TOLERANCE_CENTS: i64 = 2;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PairSuggestion {
    pub other: Transaction,
    pub implied_fx_rate: Option<f64>,
    pub leaf: &'static str,
}

pub fn implied_fx_rate(debit: &Transaction, credit: &Transaction) -> Option<f64> {
    if debit.currency.eq_ignore_ascii_case(&credit.currency) {
        return None;
    }
    if debit.amount_cents == 0 {
        return None;
    }
    Some(credit.amount_cents as f64 / debit.amount_cents as f64)
}

fn is_card(account: Option<&Account>) -> bool {
    matches!(
        account.and_then(|a| a.account_type.as_deref()),
        Some("credit_card" | "business_credit_card")
    )
}

pub fn pair_leaf_name(a: Option<&Account>, b: Option<&Account>) -> &'static str {
    if is_card(a) || is_card(b) {
        "Card payment"
    } else {
        "Between accounts"
    }
}

fn same_currency_match(a: &Transaction, b: &Transaction) -> bool {
    a.currency.eq_ignore_ascii_case(&b.currency)
        && (a.amount_cents - b.amount_cents).abs() <= SAME_CCY_TOLERANCE_CENTS
}

fn eligible_core(a: &Transaction, b: &Transaction, window: i64) -> bool {
    if a.id == b.id || a.account_id == b.account_id {
        return false;
    }
    if a.transfer_pair_id.is_some() || b.transfer_pair_id.is_some() {
        return false;
    }
    if a.is_credit == b.is_credit {
        return false;
    }
    (a.timestamp - b.timestamp).abs() <= window
}

/// Explicit pair: same-currency amount match, or any FX (user picked both legs).
fn eligible_pair(a: &Transaction, b: &Transaction) -> bool {
    if !eligible_core(a, b, PAIR_WINDOW_SECS) {
        return false;
    }
    if a.currency.eq_ignore_ascii_case(&b.currency) {
        same_currency_match(a, b)
    } else {
        true
    }
}

/// Suggestions: same-currency amount match, or Wise-style Converted narrative for FX.
/// Random cross-currency rows in ±3 days are not suggestions — they drowned autosuggest.
fn is_unmatchable(t: &Transaction) -> bool {
    t.transfer_unmatchable != 0
}

fn is_parked(t: &Transaction) -> bool {
    is_unmatchable(t) || t.transfer_await_account_id.is_some()
}

fn eligible_suggestion(a: &Transaction, b: &Transaction) -> bool {
    if is_unmatchable(a) || is_unmatchable(b) {
        return false;
    }
    if !eligible_core(a, b, WINDOW_SECS) {
        return false;
    }
    if a.currency.eq_ignore_ascii_case(&b.currency) {
        same_currency_match(a, b)
    } else {
        fx_narrative_match(a, b)
    }
}

fn account_named_in_description(account: &Account, description: &str) -> bool {
    if let Some(custom) = account.custom_display_name.as_deref().filter(|s| !s.is_empty()) {
        if learn::description_mentions_name(description, custom) {
            return true;
        }
    }
    learn::description_mentions_name(description, &account.display_name)
}

fn desc_key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Destination implied by the payee (`SMILE RENTS HOUSEHOLD` → Smile) or by an
/// already-paired row with the same description on this account.
pub fn preferred_pair_accounts_from(
    txn: &Transaction,
    accounts: &[Account],
    learned: &[(i64, String, i64)],
) -> Vec<Account> {
    let desc = desc_key(&txn.description);
    let mut ids: Vec<i64> = Vec::new();
    for a in accounts {
        if a.id == txn.account_id || a.enabled == 0 {
            continue;
        }
        if a.truelayer_id.starts_with("item:") {
            continue;
        }
        if account_named_in_description(a, &txn.description) && !ids.contains(&a.id) {
            ids.push(a.id);
        }
    }
    for (src, d, dest) in learned {
        if *src == txn.account_id && desc_key(d) == desc && !ids.contains(dest) {
            ids.push(*dest);
        }
    }
    accounts
        .iter()
        .filter(|a| ids.contains(&a.id))
        .cloned()
        .collect()
}

pub async fn preferred_pair_accounts(db: &Db, txn: &Transaction) -> Result<Vec<Account>> {
    let accounts = db.list_all_enabled_accounts().await?;
    let learned = db.list_paired_counterparts().await?;
    Ok(preferred_pair_accounts_from(txn, &accounts, &learned))
}

/// Wise (and similar) `Converted 58.87 GBP to 79.92 USD` on both legs, fee suffix ignored.
fn fx_narrative_match(a: &Transaction, b: &Transaction) -> bool {
    match (
        converted_clause(&a.description),
        converted_clause(&b.description),
    ) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn converted_clause(description: &str) -> Option<(String, String, String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)converted\s+([\d,.]+)\s+([a-z]{3})\s+to\s+([\d,.]+)\s+([a-z]{3})",
        )
        .expect("converted clause")
    });
    let caps = re.captures(description)?;
    Some((
        caps[1].replace(',', ""),
        caps[2].to_ascii_uppercase(),
        caps[3].replace(',', ""),
        caps[4].to_ascii_uppercase(),
    ))
}

pub fn debit_credit<'a>(
    a: &'a Transaction,
    b: &'a Transaction,
) -> Result<(&'a Transaction, &'a Transaction)> {
    match (a.is_credit, b.is_credit) {
        (0, 1) => Ok((a, b)),
        (1, 0) => Ok((b, a)),
        _ => bail!("pair needs one debit and one credit"),
    }
}

pub async fn suggestions_for(db: &Db, txn: &Transaction) -> Result<Vec<PairSuggestion>> {
    collect_suggestions(db, txn, None, false).await
}

/// User chose the other account: any eligible pair on that account (FX without a
/// Converted narrative is allowed — they already named the destination).
pub async fn suggestions_for_account(
    db: &Db,
    txn: &Transaction,
    account_id: i64,
) -> Result<Vec<PairSuggestion>> {
    collect_suggestions(db, txn, Some(account_id), true).await
}

async fn collect_suggestions(
    db: &Db,
    txn: &Transaction,
    account_id: Option<i64>,
    explicit_account: bool,
) -> Result<Vec<PairSuggestion>> {
    if txn.transfer_pair_id.is_some() || is_unmatchable(txn) {
        return Ok(vec![]);
    }
    if let Some(aid) = account_id {
        if aid == txn.account_id {
            return Ok(vec![]);
        }
    }
    let window = if explicit_account {
        PAIR_WINDOW_SECS
    } else {
        WINDOW_SECS
    };
    let from = txn.timestamp - window;
    let to = txn.timestamp + window;
    let cands = db
        .list_unpaired_in_window(txn.id, txn.account_id, txn.is_credit, from, to)
        .await?;
    let acct_a = db.get_account(txn.account_id).await?;
    let mut out = Vec::new();
    for other in cands {
        if is_unmatchable(&other) || other.transfer_await_account_id.is_some() {
            continue;
        }
        if let Some(aid) = account_id {
            if other.account_id != aid {
                continue;
            }
        }
        let ok = if explicit_account {
            eligible_pair(txn, &other)
        } else {
            eligible_suggestion(txn, &other)
        };
        if !ok {
            continue;
        }
        let acct_b = db.get_account(other.account_id).await?;
        let (debit, credit) = match debit_credit(txn, &other) {
            Ok(v) => v,
            Err(_) => continue,
        };
        out.push(PairSuggestion {
            implied_fx_rate: implied_fx_rate(debit, credit),
            leaf: pair_leaf_name(acct_a.as_ref(), acct_b.as_ref()),
            other,
        });
    }
    out.sort_by_key(|s| (s.other.timestamp - txn.timestamp).abs());
    Ok(out)
}

pub async fn transfer_leaf_id(db: &Db, leaf_name: &str) -> Result<i64> {
    let cats = db.list_categories().await?;
    leaf_id(&cats, leaf_name)
        .ok_or_else(|| anyhow::anyhow!("seeded Transfer leaf '{leaf_name}' is missing"))
}

fn leaf_id(cats: &[Category], name: &str) -> Option<i64> {
    cats.iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
        .map(|c| c.id)
}

pub async fn pair(db: &Db, a_id: i64, b_id: i64) -> Result<(Transaction, Transaction, Option<f64>)> {
    if a_id == b_id {
        bail!("cannot pair a transaction with itself");
    }
    let a = db
        .get_transaction(a_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {a_id} not found"))?;
    let b = db
        .get_transaction(b_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {b_id} not found"))?;
    if !eligible_pair(&a, &b) {
        bail!("transactions are not a valid transfer pair");
    }
    let acct_a = db.get_account(a.account_id).await?;
    let acct_b = db.get_account(b.account_id).await?;
    let leaf = pair_leaf_name(acct_a.as_ref(), acct_b.as_ref());
    let cid = transfer_leaf_id(db, leaf).await?;
    let (debit, credit) = debit_credit(&a, &b)?;
    let rate = implied_fx_rate(debit, credit);
    db.set_transfer_pair(a.id, b.id, cid).await?;
    let a = db
        .get_transaction(a_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {a_id} missing after pair"))?;
    let b = db
        .get_transaction(b_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {b_id} missing after pair"))?;
    Ok((a, b, rate))
}

pub async fn unpair(db: &Db, id: i64) -> Result<()> {
    let t = db
        .get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} not found"))?;
    let Some(other_id) = t.transfer_pair_id else {
        return Ok(());
    };
    db.clear_transfer_pair(id, other_id).await
}

/// Leave unpaired: hide from the Unpaired queue until the other leg exists (or undo).
pub async fn mark_unmatchable(db: &Db, id: i64) -> Result<Transaction> {
    let t = db
        .get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} not found"))?;
    if t.transfer_pair_id.is_some() {
        unpair(db, id).await?;
    }
    db.set_transfer_unmatchable(id, true).await?;
    db.get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} missing after mark"))
}

pub async fn clear_unmatchable(db: &Db, id: i64) -> Result<Transaction> {
    db.get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} not found"))?;
    db.set_transfer_unmatchable(id, false).await?;
    db.get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} missing after clear"))
}

fn can_mint_contra(account: &Account) -> bool {
    account.truelayer_id.starts_with("item:")
        || matches!(
            account.account_type.as_deref(),
            Some("trading_account" | "investment" | "sipp" | "isa")
        )
}

pub async fn ensure_item_ledger_account(db: &Db, item_id: i64) -> Result<Account> {
    let item = db
        .get_financial_item(item_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("financial item {item_id} not found"))?;
    if item.is_active != 1 {
        bail!("financial item '{}' is inactive", item.name);
    }
    let tl_id = format!("item:{}", item.id);
    if let Some(existing) = db.account_by_truelayer_id(&tl_id).await? {
        return Ok(existing);
    }
    db.create_item_ledger_account(&item.name, &item.currency, &tl_id)
        .await
}

async fn mint_contra(db: &Db, source: &Transaction, dest: &Account) -> Result<i64> {
    let src_acct = db.get_account(source.account_id).await?;
    let src_name = src_acct
        .as_ref()
        .map(|a| {
            a.custom_display_name
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| a.display_name.clone())
        })
        .unwrap_or_else(|| "other account".into());
    let dest_is_credit = source.is_credit == 0;
    let description = if dest_is_credit {
        format!("Transfer from {src_name}")
    } else {
        format!("Transfer to {src_name}")
    };
    let pid = format!("pair-contra:{}", source.id);
    db.upsert_transaction(
        dest.id,
        &pid,
        source.timestamp,
        &description,
        source.amount_cents,
        &source.currency,
        dest_is_credit,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
}

/// Pair `txn_id` to an existing account: unique match, mint on trading ledgers,
/// or wait for a bank/card feed/CSV to arrive.
pub async fn pair_to_account(db: &Db, txn_id: i64, account_id: i64) -> Result<AssignOutcome> {
    let txn = db
        .get_transaction(txn_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {txn_id} not found"))?;
    if txn.transfer_pair_id.is_some() {
        bail!("transaction is already paired");
    }
    let account = db
        .get_account(account_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("account {account_id} not found"))?;
    if account.id == txn.account_id {
        bail!("cannot pair a transfer to the same account");
    }
    let sugg = suggestions_for_account(db, &txn, account_id).await?;
    match sugg.len() {
        1 => {
            let (a, b, implied_fx_rate) = pair(db, txn_id, sugg[0].other.id).await?;
            Ok(AssignOutcome::Paired {
                a,
                b,
                implied_fx_rate,
            })
        }
        n if n > 1 => bail!("multiple matching legs on that account — pick one"),
        _ if can_mint_contra(&account) => {
            let other_id = mint_contra(db, &txn, &account).await?;
            let (a, b, implied_fx_rate) = pair(db, txn_id, other_id).await?;
            Ok(AssignOutcome::Paired {
                a,
                b,
                implied_fx_rate,
            })
        }
        _ => {
            let acct_a = db.get_account(txn.account_id).await?;
            let leaf = pair_leaf_name(acct_a.as_ref(), Some(&account));
            let cid = transfer_leaf_id(db, leaf).await?;
            db.set_transfer_await(txn_id, account.id, cid).await?;
            let transaction = db
                .get_transaction(txn_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("transaction {txn_id} missing after await"))?;
            Ok(AssignOutcome::Waiting {
                transaction,
                account,
            })
        }
    }
}

#[derive(Debug)]
pub enum AssignOutcome {
    Paired {
        a: Transaction,
        b: Transaction,
        implied_fx_rate: Option<f64>,
    },
    Waiting {
        transaction: Transaction,
        account: Account,
    },
}

pub async fn pair_to_item(
    db: &Db,
    txn_id: i64,
    item_id: i64,
) -> Result<(Transaction, Transaction, Option<f64>)> {
    let account = ensure_item_ledger_account(db, item_id).await?;
    match pair_to_account(db, txn_id, account.id).await? {
        AssignOutcome::Paired {
            a,
            b,
            implied_fx_rate,
        } => Ok((a, b, implied_fx_rate)),
        AssignOutcome::Waiting { .. } => {
            bail!("trading ledger should mint a contra, not wait")
        }
    }
}

pub async fn clear_await(db: &Db, id: i64) -> Result<Transaction> {
    db.get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} not found"))?;
    db.clear_transfer_await(id).await?;
    db.get_transaction(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transaction {id} missing after clear await"))
}

async fn fulfill_waiter(db: &Db, waiter: &Transaction) -> Result<Option<i64>> {
    let Some(aid) = waiter.transfer_await_account_id else {
        return Ok(None);
    };
    let from = waiter.timestamp - WAIT_WINDOW_SECS;
    let to = waiter.timestamp + WAIT_WINDOW_SECS;
    let cands = db
        .list_unpaired_in_window(waiter.id, waiter.account_id, waiter.is_credit, from, to)
        .await?;
    let hits: Vec<Transaction> = cands
        .into_iter()
        .filter(|o| {
            o.account_id == aid
                && eligible_core(waiter, o, WAIT_WINDOW_SECS)
                && same_currency_match(waiter, o)
        })
        .collect();
    if hits.len() != 1 {
        return Ok(None);
    }
    let other_id = hits[0].id;
    pair(db, waiter.id, other_id).await?;
    Ok(Some(other_id))
}

pub async fn fulfill_all_waiting(db: &Db) -> Result<i64> {
    let waiters = db.list_waiting_transfers().await?;
    let mut n = 0i64;
    for w in waiters {
        match fulfill_waiter(db, &w).await {
            Ok(Some(_)) => n += 1,
            Ok(None) => {}
            Err(e) => tracing::warn!("fulfill waiting txn {}: {e:#}", w.id),
        }
    }
    Ok(n)
}

/// After a new row is imported: try autosuggest, then complete any waits targeting that account.
pub async fn on_new_transaction(db: &Db, new_id: i64) -> Result<Option<i64>> {
    let auto = autosuggest_pair(db, new_id).await?;
    if auto.is_some() {
        return Ok(auto);
    }
    fulfill_all_waiting(db).await?;
    Ok(None)
}

/// If exactly one suggestion exists, pair it. Used after import.
pub async fn autosuggest_pair(db: &Db, txn_id: i64) -> Result<Option<i64>> {
    let t = match db.get_transaction(txn_id).await? {
        Some(t) => t,
        None => return Ok(None),
    };
    if t.transfer_unmatchable != 0 {
        return Ok(None);
    }
    if t.transfer_await_account_id.is_some() {
        return fulfill_waiter(db, &t).await;
    }
    let preferred = preferred_pair_accounts(db, &t).await?;
    let sugg = if preferred.len() == 1 {
        suggestions_for_account(db, &t, preferred[0].id).await?
    } else {
        suggestions_for(db, &t).await?
    };
    if sugg.len() == 1 {
        let other_id = sugg[0].other.id;
        pair(db, t.id, other_id).await?;
        return Ok(Some(other_id));
    }
    // Debit that uniquely names a card (Tide `CAPITAL ON TAP`, Smile `VANQUIS BANK`)
    // waits for the card-statement credit, which often lands days later.
    if t.is_credit == 0 {
        if let [dest] = preferred.as_slice() {
            if is_card(Some(dest)) {
                let acct_a = db.get_account(t.account_id).await?;
                let leaf = pair_leaf_name(acct_a.as_ref(), Some(dest));
                let cid = transfer_leaf_id(db, leaf).await?;
                db.set_transfer_await(txn_id, dest.id, cid).await?;
            }
        }
    }
    Ok(None)
}

/// Pair every unpaired transfer that currently has exactly one suggestion.
/// Used after import and when opening the Transfers page (historical rows never ran autosuggest).
pub async fn autosuggest_unpaired(db: &Db) -> Result<i64> {
    let mut n = fulfill_all_waiting(db).await.unwrap_or(0);
    let page = list_transfers(db, true, 2000, 0).await?;
    for t in page.transactions {
        if t.transfer_pair_id.is_some() {
            continue;
        }
        match autosuggest_pair(db, t.id).await {
            Ok(Some(_)) => n += 1,
            Ok(None) => {}
            Err(e) => tracing::warn!("autosuggest pair for txn {}: {e:#}", t.id),
        }
    }
    Ok(n)
}

#[derive(Debug, Clone)]
pub struct TransferList {
    pub transactions: Vec<Transaction>,
    pub total: i64,
    pub all_total: i64,
    pub unpaired_total: i64,
}

/// Paired legs, Transfer-tree rows, and transfer-like descriptions — not ordinary spend.
pub async fn list_transfers(
    db: &Db,
    unpaired_only: bool,
    limit: i64,
    offset: i64,
) -> Result<TransferList> {
    let cats = db.list_categories().await?;
    let tree = learn::transfer_tree_ids(&cats);
    let tree_ids: Vec<i64> = tree.iter().copied().collect();
    let engine = LearnEngine::from_db(db).await?;
    let scanned = db.list_transfer_scan(&tree_ids).await?;
    let all: Vec<Transaction> = scanned
        .into_iter()
        .filter(|t| {
            t.transfer_pair_id.is_some()
                || t.category_id
                    .map(|id| tree.contains(&id))
                    .unwrap_or(false)
                || engine.is_transfer_like(t.merchant_name.as_deref(), &t.description)
        })
        .collect();
    let unpaired_total = all
        .iter()
        .filter(|t| t.transfer_pair_id.is_none() && !is_parked(t))
        .count() as i64;
    let all_total = all.len() as i64;
    let chosen: Vec<Transaction> = if unpaired_only {
        all.into_iter()
            .filter(|t| t.transfer_pair_id.is_none() && !is_parked(t))
            .collect()
    } else {
        all
    };
    let total = chosen.len() as i64;
    let start = offset.max(0) as usize;
    let end = (start + limit.max(0) as usize).min(chosen.len());
    let transactions = if start >= chosen.len() {
        Vec::new()
    } else {
        chosen[start..end].to_vec()
    };
    Ok(TransferList {
        transactions,
        total,
        all_total,
        unpaired_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use crate::db::Db;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

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

    async fn seed_account(db: &Db, name: &str, account_type: &str) -> i64 {
        db.create_manual_account(name, account_type, None)
            .await
            .unwrap()
            .id
    }

    async fn seed_transfer_leaves(db: &Db) {
        let cats = db.list_categories().await.unwrap();
        let transfer = cats
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case("Transfer") && c.parent_id.is_none())
            .map(|c| c.id)
            .expect("seeded Transfer parent");
        if leaf_id(&cats, "Between accounts").is_none() {
            db.create_category("Between accounts", Some(transfer), None, None)
                .await
                .unwrap();
        }
        if leaf_id(&cats, "Card payment").is_none() {
            db.create_category("Card payment", Some(transfer), None, None)
                .await
                .unwrap();
        }
    }

    async fn put_txn(
        db: &Db,
        aid: i64,
        pid: &str,
        ts: i64,
        desc: &str,
        cents: i64,
        ccy: &str,
        credit: bool,
    ) -> i64 {
        db.upsert_transaction(
            aid, pid, ts, desc, cents, ccy, credit, false, None, None, None, None, None, None,
        )
        .await
        .unwrap()
    }

    fn leaf_named(cats: &[Category], name: &str) -> i64 {
        cats.iter().find(|c| c.name == name).unwrap().id
    }

    #[test]
    fn fx_rate_is_credit_over_debit_and_does_not_use_mid_market() {
        let debit = Transaction {
            id: 1,
            account_id: 1,
            provider_txn_id: "d".into(),
            timestamp: 1,
            description: "out".into(),
            amount_cents: 10000,
            currency: "GBP".into(),
            is_credit: 0,
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
        let credit = Transaction {
            id: 2,
            account_id: 2,
            amount_cents: 12700,
            currency: "USD".into(),
            is_credit: 1,
            provider_txn_id: "c".into(),
            description: "in".into(),
            ..debit.clone()
        };
        let r = implied_fx_rate(&debit, &credit).unwrap();
        assert!((r - 1.27).abs() < 1e-9);
        assert_eq!(debit.amount_cents, 10000);
        assert_eq!(credit.amount_cents, 12700);
    }

    #[tokio::test]
    async fn pair_same_currency_and_unpair() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "Current", "current").await;
        let b = seed_account(&db, "Savings", "savings").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, a, "o1", t0, "to savings", 5000, "GBP", false).await;
        let inn = put_txn(&db, b, "i1", t0 + 3600, "from current", 5000, "GBP", true).await;
        let (oa, ob, rate) = pair(&db, out, inn).await.unwrap();
        assert!(rate.is_none());
        assert_eq!(oa.transfer_pair_id, Some(inn));
        assert_eq!(ob.transfer_pair_id, Some(out));
        let cats = db.list_categories().await.unwrap();
        let between = leaf_named(&cats, "Between accounts");
        assert_eq!(oa.category_id, Some(between));
        assert_eq!(ob.category_id, Some(between));
        unpair(&db, out).await.unwrap();
        let oa = db.get_transaction(out).await.unwrap().unwrap();
        let ob = db.get_transaction(inn).await.unwrap().unwrap();
        assert!(oa.transfer_pair_id.is_none());
        assert!(ob.transfer_pair_id.is_none());
    }

    #[tokio::test]
    async fn pair_fx_stores_implied_rate_without_rewriting_amounts() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "GBP", "current").await;
        let b = seed_account(&db, "USD", "multi_currency").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, a, "o1", t0, "wise usd", 10000, "GBP", false).await;
        let inn = put_txn(&db, b, "i1", t0 + 100, "wise gbp", 12700, "USD", true).await;
        let (oa, ob, rate) = pair(&db, out, inn).await.unwrap();
        assert!((rate.unwrap() - 1.27).abs() < 1e-9);
        assert_eq!(oa.amount_cents, 10000);
        assert_eq!(ob.amount_cents, 12700);
        assert_eq!(oa.currency, "GBP");
        assert_eq!(ob.currency, "USD");
    }

    #[tokio::test]
    async fn card_repayment_uses_card_payment_leaf() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "Current", "current").await;
        let b = seed_account(&db, "Visa", "credit_card").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, a, "o1", t0, "card payment", 2000, "GBP", false).await;
        let inn = put_txn(&db, b, "i1", t0, "payment received", 2000, "GBP", true).await;
        let (oa, _, _) = pair(&db, out, inn).await.unwrap();
        let cats = db.list_categories().await.unwrap();
        assert_eq!(oa.category_id, Some(leaf_named(&cats, "Card payment")));
    }

    #[tokio::test]
    async fn spending_excludes_paired_and_transfer_legs() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "Current", "current").await;
        let b = seed_account(&db, "Savings", "savings").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, a, "o1", t0, "to savings", 5000, "GBP", false).await;
        let inn = put_txn(&db, b, "i1", t0, "from current", 5000, "GBP", true).await;
        let spend = put_txn(&db, a, "s1", t0, "TESCO", 999, "GBP", false).await;
        pair(&db, out, inn).await.unwrap();
        let rows = db.spending_by_category(t0 - 10, t0 + 10).await.unwrap();
        let total: i64 = rows.iter().map(|(_, c)| *c).sum();
        assert_eq!(total, 999, "paired 5000 must not inflate spend");
        let still = db.get_transaction(spend).await.unwrap().unwrap();
        assert_eq!(still.amount_cents, 999);
    }

    #[tokio::test]
    async fn autosuggest_pairs_when_unambiguous() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "Current", "current").await;
        let b = seed_account(&db, "Savings", "savings").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, a, "o1", t0, "to savings", 5000, "GBP", false).await;
        let inn = put_txn(&db, b, "i1", t0 + 10, "from current", 5000, "GBP", true).await;
        let other = autosuggest_pair(&db, out).await.unwrap();
        assert_eq!(other, Some(inn));
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
    }

    #[tokio::test]
    async fn transfer_page_lists_all_and_unpaired() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "Current", "current").await;
        let b = seed_account(&db, "Savings", "savings").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, a, "o1", t0, "Transfer to savings", 5000, "GBP", false).await;
        let inn = put_txn(
            &db,
            b,
            "i1",
            t0 + 10,
            "Transfer from current",
            5000,
            "GBP",
            true,
        )
        .await;
        let tesco = put_txn(&db, a, "s1", t0, "TESCO", 999, "GBP", false).await;
        let unpaired = list_transfers(&db, true, 100, 0).await.unwrap();
        let ids: Vec<i64> = unpaired.transactions.iter().map(|t| t.id).collect();
        assert!(ids.contains(&out) && ids.contains(&inn), "transfer-like stay unpaired");
        assert!(!ids.contains(&tesco), "ordinary spend must not appear");
        assert_eq!(unpaired.unpaired_total, 2);
        pair(&db, out, inn).await.unwrap();
        let unpaired = list_transfers(&db, true, 100, 0).await.unwrap();
        assert_eq!(unpaired.total, 0);
        assert_eq!(unpaired.unpaired_total, 0);
        let all = list_transfers(&db, false, 100, 0).await.unwrap();
        let ids: Vec<i64> = all.transactions.iter().map(|t| t.id).collect();
        assert!(ids.contains(&out) && ids.contains(&inn));
        assert!(!ids.contains(&tesco));
        assert_eq!(all.unpaired_total, 0);
        assert_eq!(all.total, 2);
        assert_eq!(all.all_total, 2);
    }

    #[tokio::test]
    async fn sons_coop_tfr_is_not_an_unpaired_transfer() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let smile = seed_account(&db, "Smile", "current").await;
        let t0 = 1_700_000_000;
        let lucas = put_txn(&db, smile, "l1", t0, "TFR 11039418019262", 1000, "GBP", false).await;
        let jacob = put_txn(&db, smile, "j1", t0, "TFR 11004913166968", 645, "GBP", false).await;
        let real = put_txn(&db, smile, "t1", t0, "TFR SMILE SAVINGS", 5000, "GBP", false).await;
        let page = list_transfers(&db, true, 100, 0).await.unwrap();
        let ids: Vec<i64> = page.transactions.iter().map(|t| t.id).collect();
        assert!(!ids.contains(&lucas) && !ids.contains(&jacob));
        assert!(ids.contains(&real));
    }

    #[tokio::test]
    async fn unmatchable_leaves_unpaired_queue_and_is_undoable() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "TSB", "current").await;
        let b = seed_account(&db, "Smile", "current").await;
        let t0 = 1_700_000_000;
        let out = put_txn(
            &db,
            a,
            "o1",
            t0,
            "Transfer SMILE RENTS HOUSEHOLD",
            250_000,
            "GBP",
            false,
        )
        .await;
        let inn = put_txn(
            &db,
            b,
            "i1",
            t0 + 10,
            "Transfer John Lancaster",
            250_000,
            "GBP",
            true,
        )
        .await;
        mark_unmatchable(&db, out).await.unwrap();
        let unpaired = list_transfers(&db, true, 100, 0).await.unwrap();
        let ids: Vec<i64> = unpaired.transactions.iter().map(|t| t.id).collect();
        assert!(!ids.contains(&out), "won't-pair must leave Unpaired");
        assert!(ids.contains(&inn));
        assert_eq!(unpaired.unpaired_total, 1);
        let all = list_transfers(&db, false, 100, 0).await.unwrap();
        assert!(all.transactions.iter().any(|t| t.id == out && t.transfer_unmatchable == 1));
        let n = autosuggest_unpaired(&db).await.unwrap();
        assert_eq!(n, 0, "must not auto-pair a won't-pair debit");
        assert!(db
            .get_transaction(out)
            .await
            .unwrap()
            .unwrap()
            .transfer_pair_id
            .is_none());
        clear_unmatchable(&db, out).await.unwrap();
        let n = autosuggest_unpaired(&db).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_unmatchable,
            0
        );
    }

    #[tokio::test]
    async fn smile_named_in_tsb_description_pairs_despite_same_amount_noise() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let tsb = seed_account(&db, "TSB Main Account", "current").await;
        let smile = seed_account(&db, "Smile", "current").await;
        let tide = seed_account(&db, "Tide Current Account", "current").await;
        let t0 = 1_700_000_000;
        let out = put_txn(
            &db,
            tsb,
            "o1",
            t0,
            "SMILE RENTS HOUSEHOLD",
            250_000,
            "GBP",
            false,
        )
        .await;
        let inn = put_txn(
            &db,
            smile,
            "i1",
            t0 + 5 * 86_400,
            "John Lancaster",
            250_000,
            "GBP",
            true,
        )
        .await;
        let noise = put_txn(&db, tide, "n1", t0, "TopUp", 250_000, "GBP", true).await;
        let txn = db.get_transaction(out).await.unwrap().unwrap();
        let pref = preferred_pair_accounts(&db, &txn).await.unwrap();
        assert_eq!(pref.len(), 1);
        assert_eq!(pref[0].id, smile);
        let n = autosuggest_unpaired(&db).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
        assert!(db
            .get_transaction(noise)
            .await
            .unwrap()
            .unwrap()
            .transfer_pair_id
            .is_none());
    }

    #[tokio::test]
    async fn fx_suggestions_use_converted_narrative_not_every_cross_ccy() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let gbp = seed_account(&db, "GBP", "current").await;
        let usd = seed_account(&db, "USD", "multi_currency").await;
        let t0 = 1_700_000_000;
        let out = put_txn(
            &db,
            gbp,
            "o1",
            t0,
            "Converted 58.87 GBP to 79.92 USD (fee: 0.21 GBP)",
            5866,
            "GBP",
            false,
        )
        .await;
        let inn = put_txn(
            &db,
            usd,
            "i1",
            t0 + 10,
            "Converted 58.87 GBP to 79.92 USD",
            7992,
            "USD",
            true,
        )
        .await;
        let noise = put_txn(&db, usd, "n1", t0, "Topped up account", 20000, "USD", true).await;
        let txn = db.get_transaction(out).await.unwrap().unwrap();
        let sugg = suggestions_for(&db, &txn).await.unwrap();
        assert_eq!(sugg.len(), 1);
        assert_eq!(sugg[0].other.id, inn);
        assert!(sugg.iter().all(|s| s.other.id != noise));
        let n = autosuggest_unpaired(&db).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
    }

    #[tokio::test]
    async fn autosuggest_unpaired_skips_when_two_same_amount_credits() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let a = seed_account(&db, "Current", "current").await;
        let b = seed_account(&db, "Smile", "current").await;
        let t0 = 1_700_000_000;
        let out = put_txn(
            &db,
            a,
            "o1",
            t0,
            "Transfer SMILE RENTS HOUSEHOLD",
            250000,
            "GBP",
            false,
        )
        .await;
        let c1 = put_txn(&db, b, "i1", t0, "Transfer John Lancaster", 250000, "GBP", true).await;
        let c2 = put_txn(
            &db,
            b,
            "i2",
            t0 + 1,
            "Transfer John Lancaster HOUSEHOLD",
            250000,
            "GBP",
            true,
        )
        .await;
        let n = autosuggest_unpaired(&db).await.unwrap();
        assert_eq!(n, 1);
        let out_pair = db
            .get_transaction(out)
            .await
            .unwrap()
            .unwrap()
            .transfer_pair_id;
        assert!(out_pair == Some(c1) || out_pair == Some(c2));
        let c1p = db.get_transaction(c1).await.unwrap().unwrap().transfer_pair_id;
        let c2p = db.get_transaction(c2).await.unwrap().unwrap().transfer_pair_id;
        assert_eq!(
            c1p.is_some() as i32 + c2p.is_some() as i32,
            1,
            "exactly one duplicate credit stays unpaired"
        );
    }

    #[tokio::test]
    async fn account_pick_suggests_fx_without_converted_narrative() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let gbp = seed_account(&db, "Current", "current").await;
        let usd = seed_account(&db, "Wise USD", "multi_currency").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, gbp, "o1", t0, "Topped up account", 14800, "GBP", false).await;
        let inn = put_txn(&db, usd, "i1", t0 + 60, "Topped up account", 20000, "USD", true).await;
        let auto = suggestions_for(&db, &db.get_transaction(out).await.unwrap().unwrap())
            .await
            .unwrap();
        assert!(auto.is_empty(), "FX without Converted must not autosuggest");
        let picked = suggestions_for_account(
            &db,
            &db.get_transaction(out).await.unwrap().unwrap(),
            usd,
        )
        .await
        .unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].other.id, inn);
        pair(&db, out, inn).await.unwrap();
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
    }

    #[tokio::test]
    async fn pair_to_global_prime_item_mints_contra() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let tsb = seed_account(&db, "TSB Main Account", "current").await;
        let item = db
            .create_financial_item(
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
        let t0 = 1_700_000_000;
        let inn = put_txn(&db, tsb, "pp1", t0, "PAYPAL PPWDL", 350000, "GBP", true).await;
        let (a, b, rate) = pair_to_item(&db, inn, item.id).await.unwrap();
        assert!(rate.is_none());
        assert_eq!(a.transfer_pair_id, Some(b.id));
        assert_eq!(b.transfer_pair_id, Some(a.id));
        assert_ne!(b.account_id, tsb);
        assert_eq!(b.is_credit, 0);
        assert_eq!(b.amount_cents, 350000);
        let ledger = db
            .account_by_truelayer_id(&format!("item:{}", item.id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.account_id, ledger.id);
        assert!(ledger.current_balance_cents.is_none());
    }

    #[tokio::test]
    async fn pair_to_card_without_feed_does_not_mint() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let smile = seed_account(&db, "Smile", "current").await;
        let vanquis = seed_account(&db, "Vanquis", "credit_card").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, smile, "v1", t0, "VANQUIS BANK", 127978, "GBP", false).await;
        let outcome = pair_to_account(&db, out, vanquis).await.unwrap();
        match outcome {
            AssignOutcome::Waiting {
                transaction,
                account,
            } => {
                assert_eq!(transaction.id, out);
                assert_eq!(account.id, vanquis);
                assert_eq!(transaction.transfer_await_account_id, Some(vanquis));
                assert!(transaction.transfer_pair_id.is_none());
            }
            AssignOutcome::Paired { .. } => panic!("empty Vanquis feed must wait, not pair"),
        }
        let unpaired = list_transfers(&db, true, 100, 0).await.unwrap();
        assert!(
            !unpaired.transactions.iter().any(|t| t.id == out),
            "waiting rows leave the Unpaired queue"
        );
        let inn = put_txn(
            &db,
            vanquis,
            "v-in",
            t0 + 2 * 86_400,
            "PAYMENT RECEIVED",
            127978,
            "GBP",
            true,
        )
        .await;
        let n = fulfill_all_waiting(&db).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
    }

    #[tokio::test]
    async fn autosuggest_awaits_named_card_until_statement_credit() {
        let db = test_db().await;
        seed_transfer_leaves(&db).await;
        let tide = seed_account(&db, "Tide Current Account", "business_current").await;
        let cot = seed_account(&db, "Capital On Tap", "business_credit_card").await;
        let t0 = 1_700_000_000;
        let out = put_txn(&db, tide, "cot-dd", t0, "CAPITAL ON TAP", 14004, "GBP", false).await;
        assert!(on_new_transaction(&db, out).await.unwrap().is_none());
        let parked = db.get_transaction(out).await.unwrap().unwrap();
        assert_eq!(parked.transfer_await_account_id, Some(cot));
        assert!(parked.transfer_pair_id.is_none());
        let cats = db.list_categories().await.unwrap();
        assert_eq!(parked.category_id, Some(leaf_named(&cats, "Card payment")));

        let inn = put_txn(
            &db,
            cot,
            "cot-in",
            t0 + 5 * 86_400,
            "Direct Debit Payment",
            14004,
            "GBP",
            true,
        )
        .await;
        on_new_transaction(&db, inn).await.unwrap();
        assert_eq!(
            db.get_transaction(out)
                .await
                .unwrap()
                .unwrap()
                .transfer_pair_id,
            Some(inn)
        );
    }
}
