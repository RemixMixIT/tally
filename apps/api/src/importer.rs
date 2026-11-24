//! Per-consent sync loop.
//!
//! For each enabled consent:
//!  - Refresh access token if expired
//!  - List TrueLayer accounts + cards → upsert into our `accounts` table
//!  - Pull transactions (posted + pending) since last sync
//!  - Apply the rule engine to each new transaction (auto-categorise, auto-tag)
//!  - Persist to tally's own `transactions` table (full content, not just IDs)
//!  - Match against existing bills' regex → record bill_payments
//!  - Pull standing orders + direct debits → tally `recurring` + auto-promote to `bills`
//!  - Write a sync_log row throughout

use crate::bill_match::{self, CompiledBill};
use crate::clients::truelayer::TrueLayerClient;
use crate::db::Db;
use crate::models::truelayer::*;
use crate::models::Consent;
use crate::learn::LearnEngine;
use crate::notifier::Notifier;
use crate::rules::{self, CompiledRules};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use tracing::{debug, error, info, warn};

pub struct Importer {
    pub db: Db,
    pub tl: TrueLayerClient,
    pub notifier: Notifier,
    pub ai: crate::ai::AiClient,
}

#[derive(Debug, Default)]
pub struct SyncResult {
    pub accounts_synced: i64,
    pub transactions_imported: i64,
    pub transactions_skipped: i64,
    /// Tide re-pulls that updated an existing base row. Other importers leave this at 0.
    pub transactions_refreshed: i64,
    pub recurring_imported: i64,
    pub errors: Vec<String>,
}

impl Importer {
    pub async fn sync_consent(&self, consent: &Consent) -> Result<SyncResult> {
        let log_id = self.db.start_sync_log(Some(consent.id)).await?;
        let mut result = SyncResult::default();

        let (tl_accounts, tl_cards) = match self.fetch_tl_inventory(consent).await {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("failed to fetch TL inventory: {e:#}");
                error!("{msg}");
                self.db
                    .finish_sync_log(log_id, "fail", 0, 0, 0, 0, Some(&msg))
                    .await?;
                self.db
                    .touch_consent_sync_status(consent.id, "fail", Some(&msg))
                    .await?;
                return Err(e);
            }
        };

        // Compile rules once for the whole sync. Learned mapping is derived from
        // existing categorised rows (rules still win when they set a category).
        let rules_raw = self.db.list_rules(true).await.unwrap_or_default();
        let compiled_rules = CompiledRules::compile(rules_raw);
        let mut learned = LearnEngine::from_db(&self.db)
            .await
            .unwrap_or_else(|_| LearnEngine::from_parts(Vec::new(), &[]));

        let bills = self.db.list_bills().await.unwrap_or_default();
        let bill_matchers = bill_match::compile(&bills);

        for tl_acc in &tl_accounts {
            if let Err(e) = self
                .sync_account(
                    consent,
                    tl_acc,
                    &compiled_rules,
                    &mut learned,
                    &bill_matchers,
                    &mut result,
                )
                .await
            {
                let msg = format!(
                    "account {} sync error: {e:#}",
                    tl_acc.display_name.as_deref().unwrap_or(&tl_acc.account_id)
                );
                warn!("{msg}");
                result.errors.push(msg);
            }
            result.accounts_synced += 1;
        }

        for tl_card in &tl_cards {
            if let Err(e) = self
                .sync_card(
                    consent,
                    tl_card,
                    &compiled_rules,
                    &mut learned,
                    &bill_matchers,
                    &mut result,
                )
                .await
            {
                let msg = format!(
                    "card {} sync error: {e:#}",
                    tl_card.display_name.as_deref().unwrap_or(&tl_card.account_id)
                );
                warn!("{msg}");
                result.errors.push(msg);
            }
            result.accounts_synced += 1;
        }

        let status = if result.errors.is_empty() {
            "success"
        } else if result.transactions_imported > 0 || result.accounts_synced > 0 {
            "partial"
        } else {
            "fail"
        };
        let err_msg = if result.errors.is_empty() {
            None
        } else {
            Some(result.errors.join("\n"))
        };

        self.db
            .finish_sync_log(
                log_id,
                status,
                result.accounts_synced,
                result.transactions_imported,
                result.transactions_skipped,
                result.recurring_imported,
                err_msg.as_deref(),
            )
            .await?;
        self.db
            .touch_consent_sync_status(consent.id, status, err_msg.as_deref())
            .await?;

        info!(
            consent = %consent.nickname,
            accounts = result.accounts_synced,
            imported = result.transactions_imported,
            skipped = result.transactions_skipped,
            refreshed = result.transactions_refreshed,
            recurring = result.recurring_imported,
            "sync complete"
        );

        // Auto-categorise newly-imported transactions, if the user enabled it (Settings) and
        // OpenRouter is configured. Capped so a sync can't run up a big AI bill.
        if result.transactions_imported > 0
            && self
                .db
                .get_setting("ai_auto_categorise")
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some("1")
        {
            match self.ai.categorise_uncategorised(25).await {
                Ok(n) if n > 0 => info!(consent = %consent.nickname, categorised = n, "auto-categorised"),
                Ok(_) => {}
                Err(e) => warn!("auto-categorise failed: {e:#}"),
            }
        }

        Ok(result)
    }

    async fn fetch_tl_inventory(
        &self,
        consent: &Consent,
    ) -> Result<(Vec<TLAccount>, Vec<TLCard>)> {
        let accounts = self
            .tl
            .get_accounts(&self.db, consent)
            .await
            .context("get_accounts")?;
        let cards = self.tl.get_cards(&self.db, consent).await.unwrap_or_default();
        Ok((accounts, cards))
    }

    /// UK Open Banking allows ~90 days of transactions without a fresh SCA.
    /// Banks treat `from` as start-of-day, so a calendar `now - 90 days` is often
    /// *more* than 90×24h ago and Tide returns 403 `UK.OBIE.Reauthenticate`.
    /// Cap at 88 days (TrueLayer's default) and don't surface that 403 as a
    /// failed sync when a shorter window already succeeded (including empty).
    async fn fetch_posted_transactions(
        &self,
        consent: &Consent,
        kind: &str,
        provider_id: &str,
        label: &str,
        result: &mut SyncResult,
    ) -> Vec<crate::models::truelayer::TLTransaction> {
        let to = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let mut last_error: Option<String> = None;
        let mut any_success = false;
        // 31d covers recent activity; 88d is the max history without re-auth.
        for days in [31i64, 88] {
            let from = posted_txn_from_date(days);
            let fetched = if kind == "card" {
                self.tl
                    .get_card_transactions(&self.db, consent, provider_id, Some(&from), Some(&to))
                    .await
            } else {
                self.tl
                    .get_account_transactions(
                        &self.db,
                        consent,
                        provider_id,
                        Some(&from),
                        Some(&to),
                    )
                    .await
            };
            match fetched {
                Ok(txns) => {
                    info!(
                        consent = %consent.nickname,
                        account = label,
                        days,
                        n = txns.len(),
                        "fetched posted transactions"
                    );
                    any_success = true;
                    if !txns.is_empty() {
                        return txns;
                    }
                }
                Err(e) => {
                    warn!(
                        consent = %consent.nickname,
                        account = label,
                        days,
                        "posted transactions failed: {e:#}"
                    );
                    if error_is_rate_limited(&e) {
                        result
                            .errors
                            .push(format!("{label} transactions ({days}d): {e:#}"));
                        return Vec::new();
                    }
                    if error_is_from_date_expired(&e) {
                        if !any_success {
                            last_error = Some(format!("{label} transactions ({days}d): {e:#}"));
                        }
                        break;
                    }
                    last_error = Some(format!("{label} transactions ({days}d): {e:#}"));
                }
            }
        }
        if !any_success {
            if let Some(err) = last_error {
                result.errors.push(err);
            }
        }
        Vec::new()
    }

    async fn fetch_pending_transactions(
        &self,
        consent: &Consent,
        kind: &str,
        provider_id: &str,
        label: &str,
        result: &mut SyncResult,
    ) -> Vec<crate::models::truelayer::TLTransaction> {
        let fetched = if kind == "card" {
            self.tl.get_card_pending(&self.db, consent, provider_id).await
        } else {
            self.tl
                .get_account_pending(&self.db, consent, provider_id)
                .await
        };
        match fetched {
            Ok(txns) => txns,
            Err(e) => {
                warn!(
                    consent = %consent.nickname,
                    account = label,
                    "pending transactions failed: {e:#}"
                );
                if error_is_rate_limited(&e) {
                    result
                        .errors
                        .push(format!("{label} pending transactions: {e:#}"));
                }
                Vec::new()
            }
        }
    }

    async fn sync_account(
        &self,
        consent: &Consent,
        tl: &TLAccount,
        rules: &CompiledRules,
        learned: &mut LearnEngine,
        bill_matchers: &[CompiledBill],
        result: &mut SyncResult,
    ) -> Result<()> {
        let iban = tl.account_number.as_ref().and_then(|n| n.iban.clone());
        let sort_code = tl.account_number.as_ref().and_then(|n| n.sort_code.clone());
        let number = tl.account_number.as_ref().and_then(|n| n.number.clone());
        let currency = tl.currency.clone().unwrap_or_else(|| "GBP".to_string());
        let display_name = tl
            .display_name
            .clone()
            .unwrap_or_else(|| format!("Account {}", tl.account_id));

        let account = self
            .db
            .upsert_account(
                consent.id,
                &tl.account_id,
                "account",
                &display_name,
                iban.as_deref(),
                sort_code.as_deref(),
                number.as_deref(),
                None,
                &currency,
            )
            .await?;

        // Capture account_type (TRANSACTION/SAVINGS/etc). Best-effort.
        if tl.account_type.is_some() {
            let _ = self
                .db
                .update_account_metadata(account.id, tl.account_type.as_deref(), None, None)
                .await;
        }

        // Pull live balance — separate endpoint we previously had scope for but never called.
        match self
            .tl
            .get_account_balance(&self.db, consent, &tl.account_id)
            .await
        {
            Ok(b) => {
                let current = (b.current * 100.0).round() as i64;
                let available = b.available.map(|v| (v * 100.0).round() as i64);
                let overdraft = b.overdraft.map(|v| (v * 100.0).round() as i64);
                if let Err(e) = self
                    .db
                    .update_account_balance(account.id, current, available, overdraft)
                    .await
                {
                    warn!("update_account_balance failed: {e:#}");
                }
            }
            Err(e) => debug!("balance fetch failed for {}: {e:#}", tl.account_id),
        }

        let posted = self
            .fetch_posted_transactions(consent, "account", &tl.account_id, &display_name, result)
            .await;
        let pending = self
            .fetch_pending_transactions(consent, "account", &tl.account_id, &display_name, result)
            .await;

        self.persist_transactions(
            &account,
            posted,
            false,
            rules,
            learned,
            bill_matchers,
            result,
        )
        .await;
        self.persist_transactions(
            &account,
            pending,
            true,
            rules,
            learned,
            bill_matchers,
            result,
        )
        .await;

        // Standing orders + direct debits → tally `recurring` + auto-promote to bills.
        let sos = self
            .tl
            .get_standing_orders(&self.db, consent, &tl.account_id)
            .await
            .unwrap_or_default();
        for so in sos {
            if let Err(e) = self.sync_standing_order(&account, &so, &currency, result).await {
                warn!("standing order sync error: {e:#}");
            }
        }
        let dds = self
            .tl
            .get_direct_debits(&self.db, consent, &tl.account_id)
            .await
            .unwrap_or_default();
        for dd in dds {
            if let Err(e) = self.sync_direct_debit(&account, &dd, &currency, result).await {
                warn!("direct debit sync error: {e:#}");
            }
        }

        // Reconcile any mandate we've seen before but TrueLayer didn't re-return this sync
        // (Nationwide's DD endpoint is flaky and often comes back empty). Without this, a DD
        // that was synced once but has no bill yet would silently never appear.
        self.reconcile_orphan_recurring(&account, &currency).await;

        Ok(())
    }

    /// Ensure every stored recurring mandate for this account has a bill, inferring the amount
    /// and next date from transaction history when the provider didn't supply them. Idempotent:
    /// skips mandates that already have a bill.
    async fn reconcile_orphan_recurring(&self, account: &crate::models::Account, currency: &str) {
        let rows = self
            .db
            .list_recurring_for_account(account.id)
            .await
            .unwrap_or_default();
        for r in rows {
            if r.firefly_bill_id.is_some() {
                continue;
            }
            if matches!(self.db.bill_id_for_recurring(r.id).await, Ok(Some(_))) {
                continue;
            }
            let inferred = self.infer_payment(account.id, &r.name).await;
            let amount_cents = r
                .amount
                .map(|a| (a * 100.0).round() as i64)
                .or(inferred.map(|(c, _)| c));
            let freq = freq_to_tally(r.frequency.as_deref()).unwrap_or("monthly");
            let last_ts = r
                .next_payment_date
                .as_deref()
                .and_then(parse_iso_date)
                .or(inferred.map(|(_, ts)| ts));
            let now = Utc::now().timestamp();
            let next_ts = last_ts.map(|t| crate::db::advance_past(t, freq, now));
            let display_name = if r.kind == "direct_debit" {
                format!("{} (DD)", r.name)
            } else {
                r.name.clone()
            };
            self.upsert_bill_for_recurring(
                r.id,
                account.id,
                &display_name,
                &r.name,
                amount_cents,
                currency,
                freq,
                next_ts,
            )
                .await;
        }
    }

    async fn sync_card(
        &self,
        consent: &Consent,
        tl: &TLCard,
        rules: &CompiledRules,
        learned: &mut LearnEngine,
        bill_matchers: &[CompiledBill],
        result: &mut SyncResult,
    ) -> Result<()> {
        let display_name = tl
            .display_name
            .clone()
            .unwrap_or_else(|| format!("Card {}", tl.account_id));
        let last4 = tl
            .partial_card_number
            .as_ref()
            .map(|s| s.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>());
        let currency = tl.currency.clone().unwrap_or_else(|| "GBP".to_string());

        let account = self
            .db
            .upsert_account(
                consent.id,
                &tl.account_id,
                "card",
                &display_name,
                None,
                None,
                None,
                last4.as_deref(),
                &currency,
            )
            .await?;

        // Card metadata — network (VISA/MASTERCARD/AMEX) + name on card.
        if tl.card_network.is_some() || tl.name_on_card.is_some() {
            let _ = self
                .db
                .update_account_metadata(
                    account.id,
                    Some("CARD"),
                    tl.card_network.as_deref(),
                    tl.name_on_card.as_deref(),
                )
                .await;
        }

        // Card-specific balance is richer than account balance — credit_limit, statement, payment due.
        match self
            .tl
            .get_card_balance(&self.db, consent, &tl.account_id)
            .await
        {
            Ok(b) => {
                let current = (b.current * 100.0).round() as i64;
                let available = b.available.map(|v| (v * 100.0).round() as i64);
                let credit_limit = b.credit_limit.map(|v| (v * 100.0).round() as i64);
                let last_stmt = b.last_statement_balance.map(|v| (v * 100.0).round() as i64);
                let payment_due = b.payment_due.map(|v| (v * 100.0).round() as i64);
                if let Err(e) = self
                    .db
                    .update_card_balance(
                        account.id,
                        current,
                        available,
                        credit_limit,
                        last_stmt,
                        b.last_statement_date.as_deref(),
                        payment_due,
                        b.payment_due_date.as_deref(),
                    )
                    .await
                {
                    warn!("update_card_balance failed: {e:#}");
                }
            }
            Err(e) => debug!("card balance fetch failed for {}: {e:#}", tl.account_id),
        }

        let posted = self
            .fetch_posted_transactions(consent, "card", &tl.account_id, &display_name, result)
            .await;
        let pending = self
            .fetch_pending_transactions(consent, "card", &tl.account_id, &display_name, result)
            .await;

        self.persist_transactions(
            &account,
            posted,
            false,
            rules,
            learned,
            bill_matchers,
            result,
        )
        .await;
        self.persist_transactions(
            &account,
            pending,
            true,
            rules,
            learned,
            bill_matchers,
            result,
        )
        .await;

        Ok(())
    }

    /// Returns true when this transaction must not be inserted.
    /// Tide accounts that were seen already have their base row refreshed.
    /// A Tide seen-set hit with no transaction row returns false so the insert path runs once.
    async fn handle_seen_transaction(
        &self,
        account: &crate::models::Account,
        txn: &TLTransaction,
        is_pending: bool,
        result: &mut SyncResult,
    ) -> bool {
        let tide = crate::director::is_tide_statement_account(
            &account.display_name,
            account.custom_display_name.as_deref(),
        );
        match self.db.is_txn_seen(account.id, &txn.transaction_id).await {
            Ok(false) => false,
            Ok(true) if !tide => {
                result.transactions_skipped += 1;
                true
            }
            Err(e) => {
                warn!("is_txn_seen lookup failed: {e:#}");
                true
            }
            Ok(true) => match self.refresh_tide_base_row(account, txn, is_pending).await {
                Ok(true) => {
                    result.transactions_refreshed += 1;
                    let _ = self
                        .db
                        .record_txn_imported(
                            account.id,
                            &txn.transaction_id,
                            None,
                            is_pending,
                            None,
                        )
                        .await;
                    true
                }
                Ok(false) => false,
                Err(e) => {
                    warn!("tide refresh failed: {e:#}");
                    result
                        .errors
                        .push(format!("txn {}: {e:#}", txn.transaction_id));
                    true
                }
            },
        }
    }

    async fn refresh_tide_base_row(
        &self,
        account: &crate::models::Account,
        txn: &TLTransaction,
        is_pending: bool,
    ) -> Result<bool> {
        let is_credit = !txn.transaction_type.eq_ignore_ascii_case("DEBIT");
        let amount_cents = (txn.amount.abs() * 100.0).round() as i64;
        let ts = DateTime::parse_from_rfc3339(&txn.timestamp)
            .map(|d| d.timestamp())
            .unwrap_or_else(|_| Utc::now().timestamp());
        let cp_iban = txn.meta.as_ref().and_then(|m| m.counter_party_iban.clone());
        let cp_name = txn
            .meta
            .as_ref()
            .and_then(|m| m.counter_party_preferred_name.clone())
            .or_else(|| txn.merchant_name.clone());
        let raw_json = serde_json::to_string(&serde_json::json!({
            "description": txn.description,
            "category": txn.transaction_category,
            "classification": txn.transaction_classification,
            "merchant": txn.merchant_name,
            "currency": txn.currency,
        }))
        .ok();
        self.db
            .refresh_transaction_base(
                account.id,
                &txn.transaction_id,
                ts,
                &txn.description,
                amount_cents,
                &txn.currency,
                is_credit,
                is_pending,
                txn.merchant_name.as_deref(),
                cp_iban.as_deref(),
                cp_name.as_deref(),
                raw_json.as_deref(),
            )
            .await
    }

    async fn persist_transactions(
        &self,
        account: &crate::models::Account,
        txns: Vec<TLTransaction>,
        is_pending: bool,
        rules: &CompiledRules,
        learned: &mut LearnEngine,
        bill_matchers: &[CompiledBill],
        result: &mut SyncResult,
    ) {
        for txn in txns {
            // Other TrueLayer accounts keep the seen-set skip. Tide re-pulls refresh
            // the base row instead, and never insert a second copy.
            if self
                .handle_seen_transaction(account, &txn, is_pending, result)
                .await
            {
                continue;
            }

            let is_credit = !txn.transaction_type.eq_ignore_ascii_case("DEBIT");
            let amount_cents = (txn.amount.abs() * 100.0).round() as i64;

            // TrueLayer timestamps are ISO 8601 — parse to unix.
            let ts = DateTime::parse_from_rfc3339(&txn.timestamp)
                .map(|d| d.timestamp())
                .unwrap_or_else(|_| Utc::now().timestamp());

            let cp_iban = txn.meta.as_ref().and_then(|m| m.counter_party_iban.clone());
            let cp_name = txn
                .meta
                .as_ref()
                .and_then(|m| m.counter_party_preferred_name.clone())
                .or_else(|| txn.merchant_name.clone());
            let raw_json = serde_json::to_string(&serde_json::json!({
                "description": txn.description,
                "category": txn.transaction_category,
                "classification": txn.transaction_classification,
                "merchant": txn.merchant_name,
                "currency": txn.currency,
            }))
            .ok();

            // Build a temporary Transaction-like view to feed the rules engine.
            // We don't have the db id yet (assigned on insert), so use a synthetic 0.
            let pseudo = crate::models::Transaction {
                id: 0,
                account_id: account.id,
                provider_txn_id: txn.transaction_id.clone(),
                timestamp: ts,
                description: txn.description.clone(),
                amount_cents,
                currency: txn.currency.clone(),
                is_credit: if is_credit { 1 } else { 0 },
                is_pending: if is_pending { 1 } else { 0 },
                merchant_name: txn.merchant_name.clone(),
                counterparty_iban: cp_iban.clone(),
                counterparty_name: cp_name.clone(),
                category_id: None,
                notes: None,
                raw_json: None,
                transfer_pair_id: None,
                transfer_unmatchable: 0,
                transfer_await_account_id: None,
                created_at: 0,
                updated_at: 0,
            };

            // Rules first; learned unanimous history fills in when no rule set a category.
            let (category_id, _) = learned.category_for_new(&pseudo, rules);
            let eff = rules::apply(&pseudo, rules);

            // Insert.
            let new_id = match self
                .db
                .upsert_transaction(
                    account.id,
                    &txn.transaction_id,
                    ts,
                    &txn.description,
                    amount_cents,
                    &txn.currency,
                    is_credit,
                    is_pending,
                    txn.merchant_name.as_deref(),
                    cp_iban.as_deref(),
                    cp_name.as_deref(),
                    category_id,
                    eff.set_notes.as_deref(),
                    raw_json.as_deref(),
                )
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    warn!("upsert transaction failed: {e:#}");
                    result
                        .errors
                        .push(format!("txn {}: {e:#}", txn.transaction_id));
                    continue;
                }
            };

            if let Some(cid) = category_id {
                learned.observe(txn.merchant_name.as_deref(), &txn.description, cid);
            }

            if let Err(e) = crate::pairs::on_new_transaction(&self.db, new_id).await {
                warn!("pair/await for txn {new_id}: {e:#}");
            }

            // Mark dedup so future syncs don't reprocess.
            let _ = self
                .db
                .record_txn_imported(account.id, &txn.transaction_id, Some(new_id), is_pending, None)
                .await;

            // Apply tag effects.
            for tid in &eff.add_tag_ids {
                let _ = self.db.tag_transaction(new_id, *tid).await;
            }
            for rid in &eff.rules_fired {
                let _ = self.db.bump_rule_applied(*rid).await;
            }

            // Link in-window / unknown-amount regulars. Out-of-window amounts stay
            // unlinked for the amount-exception prompt.
            let _ = self
                .db
                .auto_link_bill_if_matched(
                    bill_matchers,
                    account.id,
                    new_id,
                    &txn.description,
                    txn.merchant_name.as_deref(),
                    amount_cents,
                    is_credit,
                    ts,
                )
                .await;

            result.transactions_imported += 1;
            self.notifier
                .notify_transaction(&txn, account, is_pending)
                .await;
        }
    }

    async fn sync_standing_order(
        &self,
        account: &crate::models::Account,
        so: &TLStandingOrder,
        currency: &str,
        result: &mut SyncResult,
    ) -> Result<()> {
        let tl_id = so.standing_order_id.clone().unwrap_or_else(|| {
            format!("so:{}:{}", account.truelayer_id, so.payee.clone().unwrap_or_default())
        });
        let name = so.payee.clone().unwrap_or_else(|| "Standing Order".to_string());
        let freq = freq_to_tally(so.frequency.as_deref()).unwrap_or("monthly");

        // Amount: prefer TrueLayer's, else infer from matching debits in transaction history.
        let tl_cents = so.next_payment_amount.map(|a| (a * 100.0).round() as i64);
        let inferred = self.infer_payment(account.id, &name).await;
        let amount_cents = tl_cents.or(inferred.map(|(c, _)| c));

        // Anchor date: TL's next_payment_date if present, else the last inferred payment ts.
        let anchor_ts = so
            .next_payment_date
            .as_deref()
            .and_then(parse_iso_date)
            .or(inferred.map(|(_, ts)| ts));
        let now = Utc::now().timestamp();
        let next_ts = anchor_ts.map(|t| crate::db::advance_past(t, freq, now));
        let next_iso = next_ts.map(iso_from_ts);

        let entry = self
            .db
            .upsert_recurring(
                account.id,
                &tl_id,
                "standing_order",
                &name,
                amount_cents.map(|c| c as f64 / 100.0),
                Some(currency),
                so.frequency.as_deref(),
                next_iso.as_deref().or(so.next_payment_date.as_deref()),
                None,
            )
            .await?;
        result.recurring_imported += 1;

        self.upsert_bill_for_recurring(
            entry.id,
            account.id,
            &name,
            &name,
            amount_cents,
            currency,
            freq,
            next_ts,
        )
            .await;
        Ok(())
    }

    async fn sync_direct_debit(
        &self,
        account: &crate::models::Account,
        dd: &TLDirectDebit,
        currency: &str,
        result: &mut SyncResult,
    ) -> Result<()> {
        let tl_id = dd.direct_debit_id.clone().unwrap_or_else(|| {
            format!("dd:{}:{}", account.truelayer_id, dd.name.clone().unwrap_or_default())
        });
        let name = dd.name.clone().unwrap_or_else(|| "Direct Debit".to_string());

        // TrueLayer (Nationwide especially) usually leaves previous_payment_amount/date null on
        // the mandate. Fall back to the real transaction history to recover both.
        let tl_cents = dd.previous_payment_amount.map(|a| (a * 100.0).round() as i64);
        let inferred = self.infer_payment(account.id, &name).await;
        let amount_cents = tl_cents.or(inferred.map(|(c, _)| c));

        // Last payment date: TL's, else the inferred transaction ts. Project next as +1 month
        // (DDs are effectively monthly) rolled forward until it's in the future.
        let last_ts = dd
            .previous_payment_timestamp
            .as_deref()
            .and_then(parse_iso_date)
            .or(inferred.map(|(_, ts)| ts));
        let now = Utc::now().timestamp();
        let next_ts = last_ts.map(|t| crate::db::advance_past(t, "monthly", now));
        let next_iso = next_ts.map(iso_from_ts);

        let entry = self
            .db
            .upsert_recurring(
                account.id,
                &tl_id,
                "direct_debit",
                &name,
                amount_cents.map(|c| c as f64 / 100.0),
                Some(currency),
                Some("monthly"),
                next_iso.as_deref(),
                dd.status.as_deref(),
            )
            .await?;
        result.recurring_imported += 1;

        let display_name = format!("{name} (DD)");
        self.upsert_bill_for_recurring(
            entry.id,
            account.id,
            &display_name,
            &name,
            amount_cents,
            currency,
            "monthly",
            next_ts,
        )
            .await;
        Ok(())
    }

    /// Recover a recurring payment's amount + last date from transaction history when the
    /// provider didn't supply them. Tries the full mandate name, then its leading token.
    async fn infer_payment(&self, account_id: i64, name: &str) -> Option<(i64, i64)> {
        let full = format!("%{name}%");
        if let Ok(Some(hit)) = self.db.infer_recurring_payment(account_id, &full).await {
            return Some(hit);
        }
        if let Some(tok) = leading_token(name) {
            let like = format!("%{tok}%");
            if let Ok(Some(hit)) = self.db.infer_recurring_payment(account_id, &like).await {
                return Some(hit);
            }
        }
        None
    }

    /// Create or update (idempotently across re-syncs) the bill backing a recurring mandate.
    /// Bills are created even when the amount is still unknown (0/0 → UI shows "amount unknown")
    /// so every DD / standing order stays visible in Bills, not silently dropped.
    async fn upsert_bill_for_recurring(
        &self,
        recurring_id: i64,
        account_id: i64,
        display_name: &str,
        match_name: &str,
        amount_cents: Option<i64>,
        currency: &str,
        freq: &str,
        next_ts: Option<i64>,
    ) {
        let (min, max) = match amount_cents {
            Some(c) => ((c as f64 * 0.95).round() as i64, (c as f64 * 1.05).round() as i64),
            None => (0, 0),
        };
        match self.db.bill_id_for_recurring(recurring_id).await {
            Ok(Some(bid)) => {
                if let Err(e) = self
                    .db
                    .update_bill_schedule(bid, min, max, next_ts, Some(account_id))
                    .await
                {
                    debug!("update bill {bid} for recurring {recurring_id} failed: {e:#}");
                }
            }
            Ok(None) => {
                let regex = Some(escape_for_regex(match_name));
                match self
                    .db
                    .create_bill(
                        display_name,
                        min,
                        max,
                        currency,
                        freq,
                        next_ts,
                        regex.as_deref(),
                        Some(recurring_id),
                        Some(account_id),
                        false,
                    )
                    .await
                {
                    Ok(bid) => {
                        let _ = self.db.map_recurring_to_firefly_bill(recurring_id, bid).await;
                    }
                    Err(e) => debug!("auto-promote recurring {recurring_id} to bill failed: {e:#}"),
                }
            }
            Err(e) => debug!("bill_id_for_recurring lookup failed: {e:#}"),
        }
    }
}

fn error_is_rate_limited(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}").to_ascii_lowercase();
    msg.contains("status 429")
        || msg.contains("too_many_requests")
        || msg.contains("request_limit")
        || msg.contains("provider_too_many_requests")
}

fn error_is_from_date_expired(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}").to_ascii_lowercase();
    msg.contains("fromdatelong")
        || msg.contains("uk.obie.reauthenticate")
        || (msg.contains("re-auth") && msg.contains("90 days"))
}

/// Inclusive start-of-day `from` must stay strictly inside 90 days or Tide 403s.
fn posted_txn_from_date(days: i64) -> String {
    let days = days.clamp(1, 88);
    (Utc::now().date_naive() - Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod fetch_window_tests {
    use super::*;

    #[test]
    fn posted_from_date_never_exceeds_88_days() {
        let from = posted_txn_from_date(90);
        let parsed = chrono::NaiveDate::parse_from_str(&from, "%Y-%m-%d").unwrap();
        let today = Utc::now().date_naive();
        assert!((today - parsed).num_days() <= 88);
    }

    #[test]
    fn detects_obie_from_date_reauth() {
        let e = anyhow::anyhow!(
            "status 403 Forbidden, body: expected_fromDateLong_expired UK.OBIE.Reauthenticate"
        );
        assert!(error_is_from_date_expired(&e));
        assert!(!error_is_rate_limited(&e));
    }
}

fn freq_to_tally(tl_freq: Option<&str>) -> Option<&'static str> {
    match tl_freq?.to_uppercase().as_str() {
        "WEEKLY" | "EVRYWEEK" => Some("weekly"),
        "MONTHLY" | "EVRYMNTH" => Some("monthly"),
        "YEARLY" | "EVRYWORK" => Some("yearly"),
        "FORTNIGHTLY" | "EVRY2WEEK" => Some("fortnightly"),
        _ => Some("monthly"),
    }
}

fn escape_for_regex(s: &str) -> String {
    regex::escape(s)
}

/// Parse a TrueLayer date — either a full RFC3339 timestamp ("2018-09-17T00:00:00+01:00")
/// or a bare "YYYY-MM-DD" — to a unix timestamp.
fn parse_iso_date(d: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(d) {
        return Some(dt.timestamp());
    }
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}

fn iso_from_ts(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d")
        .to_string()
}

/// The first "significant" whitespace token of a mandate name (length >= 4, trailing
/// punctuation trimmed) — used as a looser LIKE fallback when the full name doesn't match
/// a transaction description verbatim. e.g. "AQUA CREDIT CARD" -> "AQUA".
fn leading_token(name: &str) -> Option<String> {
    name.split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '.'))
        .find(|t| t.len() >= 4)
        .map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::truelayer::TrueLayerClient;
    use crate::crypto::Crypto;
    use crate::db::Db;
    use crate::learn::LearnEngine;
    use crate::models::truelayer::TLTransaction;
    use crate::notifier::Notifier;
    use crate::rules::CompiledRules;
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

    fn importer(db: Db) -> Importer {
        let mut notifier = Notifier::from_env();
        notifier.ha_url = None;
        notifier.ha_token = None;
        notifier.telegram_token = None;
        notifier.telegram_chat = None;
        notifier.db = None;
        Importer {
            tl: TrueLayerClient::new("test".into(), "test".into(), "http://127.0.0.1".into())
                .unwrap(),
            ai: crate::ai::AiClient::new(db.clone()),
            db,
            notifier,
        }
    }

    fn txn(id: &str, description: &str, amount: f64) -> TLTransaction {
        serde_json::from_value(serde_json::json!({
            "transaction_id": id,
            "timestamp": "2025-06-15T12:00:00Z",
            "description": description,
            "amount": amount,
            "currency": "GBP",
            "transaction_type": "DEBIT",
            "transaction_category": "OFFICE",
            "merchant_name": "LANDLORD"
        }))
        .unwrap()
    }

    async fn pull(imp: &Importer, account: &crate::models::Account, txns: Vec<TLTransaction>) -> SyncResult {
        let mut result = SyncResult::default();
        let rules = CompiledRules { rules: vec![] };
        let mut learned = LearnEngine::from_parts(vec![], &[]);
        imp.persist_transactions(account, txns, false, &rules, &mut learned, &[], &mut result)
            .await;
        result
    }

    async fn one_row(db: &Db, account_id: i64) -> crate::models::Transaction {
        let rows = db
            .list_transactions(Some(&[account_id]), None, None, None, None, None, None, None, 10, 0)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        rows.into_iter().next().unwrap()
    }

    #[tokio::test]
    async fn tide_repull_refreshes_base_row_and_keeps_classification() {
        let db = test_db().await;
        let tide = db
            .create_manual_account("Tide Current Account", "business_current", None)
            .await
            .unwrap();
        let imp = importer(db.clone());

        let first = pull(&imp, &tide, vec![txn("tl-tide-1", "OFFICE RENT", 10.0)]).await;
        assert_eq!(first.transactions_imported, 1);
        assert_eq!(first.transactions_refreshed, 0);

        let row = one_row(&db, tide.id).await;
        let cat = db
            .create_category("Office", None, None, None)
            .await
            .unwrap();
        db.update_transaction_category(row.id, Some(cat)).await.unwrap();
        db.update_transaction_notes(row.id, Some("keep me"))
            .await
            .unwrap();
        db.conn
            .execute(
                "UPDATE transactions
                 SET receipt_photo_id = 'immich-1',
                     claim_flags = '[\"remixmix\"]',
                     claim_pack_id = 'pack-a',
                     claim_pack_id_2 = 'pack-b'
                 WHERE id = ?1",
                libsql::params![row.id],
            )
            .await
            .unwrap();
        db.set_director_classification(
            tide.id,
            "tl-tide-1",
            "expense",
            row.amount_cents,
            Some("https://example.test/evidence"),
            Some("director note"),
        )
        .await
        .unwrap();

        let second = pull(
            &imp,
            &tide,
            vec![txn("tl-tide-1", "OFFICE RENT UPDATED", 12.5)],
        )
        .await;
        assert_eq!(second.transactions_imported, 0);
        assert_eq!(second.transactions_refreshed, 1);
        assert_eq!(second.transactions_skipped, 0);
        assert!(second.errors.is_empty());

        let again = one_row(&db, tide.id).await;
        assert_eq!(again.id, row.id);
        assert_eq!(again.description, "OFFICE RENT UPDATED");
        assert_eq!(again.amount_cents, 1250);
        assert_eq!(again.category_id, Some(cat));
        assert_eq!(again.notes.as_deref(), Some("keep me"));

        let mut flags = db
            .conn
            .query(
                "SELECT receipt_photo_id, claim_flags, claim_pack_id, claim_pack_id_2
                 FROM transactions WHERE id = ?1",
                libsql::params![row.id],
            )
            .await
            .unwrap();
        let flag_row = flags.next().await.unwrap().unwrap();
        assert_eq!(flag_row.get::<String>(0).unwrap(), "immich-1");
        assert_eq!(flag_row.get::<String>(1).unwrap(), "[\"remixmix\"]");
        assert_eq!(flag_row.get::<String>(2).unwrap(), "pack-a");
        assert_eq!(flag_row.get::<String>(3).unwrap(), "pack-b");

        let (start, end) = crate::director::accounts_period();
        let listed = db.list_director_payments(start, end).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].classification_type.as_deref(), Some("expense"));
        assert_eq!(listed[0].classification_notes.as_deref(), Some("director note"));
        assert_eq!(listed[0].classification_amount_cents, Some(row.amount_cents));
        assert_eq!(listed[0].description, "OFFICE RENT UPDATED");
        assert_eq!(listed[0].amount_cents, 1250);
    }

    #[tokio::test]
    async fn non_tide_repull_still_skips() {
        let db = test_db().await;
        let barclays = db
            .create_manual_account("Barclays Bank Account", "current", None)
            .await
            .unwrap();
        let imp = importer(db.clone());
        let first = pull(&imp, &barclays, vec![txn("tl-barc-1", "COFFEE", 3.5)]).await;
        assert_eq!(first.transactions_imported, 1);
        let second = pull(&imp, &barclays, vec![txn("tl-barc-1", "COFFEE CHANGED", 9.0)]).await;
        assert_eq!(second.transactions_imported, 0);
        assert_eq!(second.transactions_refreshed, 0);
        assert_eq!(second.transactions_skipped, 1);
        let row = one_row(&db, barclays.id).await;
        assert_eq!(row.description, "COFFEE");
        assert_eq!(row.amount_cents, 350);
    }
}
