//! Persistence for the 0014 finance valuation model. Kept out of `db.rs` so the
//! existing account/transaction/holdings queries stay untouched.

use crate::db::Db;
use crate::finance::{
    self, AccountBalanceView, FinanceError, FinanceWorld, HoldingView, ItemView, MortgageView,
    SnapshotView,
};
use crate::models::*;
use anyhow::Result;
use chrono::Utc;
use libsql::params;

fn now() -> i64 {
    Utc::now().timestamp()
}

fn opt_str(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|v| !v.is_empty()).map(|v| v.to_string())
}

impl Db {
    pub async fn list_financial_items(&self) -> Result<Vec<FinancialItem>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM financial_items ORDER BY is_active DESC, class, name",
                (),
            )
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn get_financial_item(&self, id: i64) -> Result<Option<FinancialItem>> {
        let rows = self
            .conn
            .query("SELECT * FROM financial_items WHERE id = ?1", params![id])
            .await?;
        crate::db::map_opt(rows).await
    }

    pub async fn parent_map(&self) -> Result<std::collections::HashMap<i64, Option<i64>>> {
        let mut rows = self
            .conn
            .query("SELECT id, parent_item_id FROM financial_items", ())
            .await?;
        let mut m = std::collections::HashMap::new();
        while let Some(row) = rows.next().await? {
            m.insert(row.get::<i64>(0)?, row.get::<Option<i64>>(1)?);
        }
        Ok(m)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_financial_item(
        &self,
        name: &str,
        class: &str,
        item_type: &str,
        owner_entity: &str,
        provider: Option<&str>,
        reference: Option<&str>,
        parent_item_id: Option<i64>,
        currency: &str,
        is_rollup: bool,
        notes: Option<&str>,
    ) -> Result<FinancialItem, FinanceError> {
        finance::validate_name(name)?;
        finance::validate_class(class)?;
        finance::validate_currency(currency)?;
        if item_type.trim().is_empty() {
            return Err(FinanceError::invalid("item_type is required"));
        }
        if owner_entity.trim().is_empty() {
            return Err(FinanceError::invalid("owner_entity is required"));
        }
        if let Some(pid) = parent_item_id {
            if self
                .get_financial_item(pid)
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?
                .is_none()
            {
                return Err(FinanceError::not_found("parent_item_id not found"));
            }
            let parents = self
                .parent_map()
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
            if finance::would_cycle(None, Some(pid), &parents) {
                return Err(FinanceError::invalid("parent_item_id would create a cycle"));
            }
        }
        let (is_asset, is_liability) = finance::flags_for_class(class);
        let ts = now();
        self.conn
            .execute(
                "INSERT INTO financial_items (
                    name, class, item_type, owner_entity, provider, reference,
                    parent_item_id, currency, is_asset, is_liability, is_rollup,
                    is_active, notes, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14)",
                params![
                    name.trim(),
                    class,
                    item_type.trim(),
                    owner_entity.trim(),
                    opt_str(provider),
                    opt_str(reference),
                    parent_item_id,
                    currency,
                    is_asset,
                    is_liability,
                    if is_rollup { 1i64 } else { 0 },
                    opt_str(notes),
                    ts,
                    ts,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let id = self.conn.last_insert_rowid();
        if class == "tax_liability" {
            let kind = if finance::TAX_KINDS.contains(&item_type) {
                item_type
            } else {
                "other"
            };
            self.conn
                .execute(
                    "INSERT INTO tax_liability_details (financial_item_id, tax_kind) VALUES (?1, ?2)",
                    params![id, kind],
                )
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        }
        self.get_financial_item(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("item disappeared after insert"))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_financial_item(
        &self,
        id: i64,
        name: &str,
        item_type: &str,
        owner_entity: &str,
        provider: Option<&str>,
        reference: Option<&str>,
        parent_item_id: Option<i64>,
        currency: &str,
        is_rollup: bool,
        is_active: bool,
        notes: Option<&str>,
    ) -> Result<FinancialItem, FinanceError> {
        let existing = self
            .get_financial_item(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))?;
        finance::validate_name(name)?;
        finance::validate_currency(currency)?;
        if item_type.trim().is_empty() {
            return Err(FinanceError::invalid("item_type is required"));
        }
        if owner_entity.trim().is_empty() {
            return Err(FinanceError::invalid("owner_entity is required"));
        }
        if let Some(pid) = parent_item_id {
            if pid == id {
                return Err(FinanceError::invalid("item cannot be its own parent"));
            }
            if self
                .get_financial_item(pid)
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?
                .is_none()
            {
                return Err(FinanceError::not_found("parent_item_id not found"));
            }
            let parents = self
                .parent_map()
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
            if finance::would_cycle(Some(id), Some(pid), &parents) {
                return Err(FinanceError::invalid("parent_item_id would create a cycle"));
            }
        }
        let ts = now();
        self.conn
            .execute(
                "UPDATE financial_items SET
                    name = ?1, item_type = ?2, owner_entity = ?3, provider = ?4,
                    reference = ?5, parent_item_id = ?6, currency = ?7, is_rollup = ?8,
                    is_active = ?9, notes = ?10, updated_at = ?11
                 WHERE id = ?12",
                params![
                    name.trim(),
                    item_type.trim(),
                    owner_entity.trim(),
                    opt_str(provider),
                    opt_str(reference),
                    parent_item_id,
                    currency,
                    if is_rollup { 1i64 } else { 0 },
                    if is_active { 1i64 } else { 0 },
                    opt_str(notes),
                    ts,
                    id,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        if existing.class == "tax_liability" && finance::TAX_KINDS.contains(&item_type) {
            self.conn
                .execute(
                    "INSERT INTO tax_liability_details (financial_item_id, tax_kind) VALUES (?1, ?2)
                     ON CONFLICT(financial_item_id) DO UPDATE SET tax_kind = excluded.tax_kind",
                    params![id, item_type.trim()],
                )
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        }
        self.get_financial_item(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))
    }

    pub async fn delete_or_deactivate_financial_item(
        &self,
        id: i64,
    ) -> Result<serde_json::Value, FinanceError> {
        let item = self
            .get_financial_item(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))?;

        let mut rows = self
            .conn
            .query(
                "SELECT
                    (SELECT count(*) FROM financial_items WHERE parent_item_id = ?1),
                    (SELECT count(*) FROM valuation_snapshots WHERE financial_item_id = ?1),
                    (SELECT count(*) FROM investment_holdings WHERE financial_item_id = ?1),
                    (SELECT count(*) FROM forecast_scenarios WHERE financial_item_id = ?1),
                    (SELECT count(*) FROM mortgage_details WHERE property_item_id = ?1)",
                params![id],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let row = rows
            .next()
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("count query returned no row"))?;
        let children: i64 = row.get(0).map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let snaps: i64 = row.get(1).map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let holds: i64 = row.get(2).map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let forecasts: i64 = row.get(3).map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let linked_mortgages: i64 = row.get(4).map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let has_history = children + snaps + holds + forecasts + linked_mortgages > 0;

        if has_history {
            self.conn
                .execute(
                    "UPDATE financial_items SET is_active = 0, updated_at = ?1 WHERE id = ?2",
                    params![now(), id],
                )
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
            return Ok(serde_json::json!({
                "ok": true,
                "deactivated": true,
                "id": id,
                "reason": "historical or child records exist; item marked inactive"
            }));
        }

        self.conn
            .execute("DELETE FROM property_details WHERE financial_item_id = ?1", params![id])
            .await
            .ok();
        self.conn
            .execute("DELETE FROM mortgage_details WHERE financial_item_id = ?1", params![id])
            .await
            .ok();
        self.conn
            .execute("DELETE FROM tax_liability_details WHERE financial_item_id = ?1", params![id])
            .await
            .ok();
        self.conn
            .execute("DELETE FROM receivable_details WHERE financial_item_id = ?1", params![id])
            .await
            .ok();
        self.conn
            .execute("DELETE FROM financial_items WHERE id = ?1", params![id])
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let _ = item;
        Ok(serde_json::json!({ "ok": true, "deleted": true, "id": id }))
    }

    pub async fn list_valuations(&self, item_id: i64) -> Result<Vec<ValuationSnapshot>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM valuation_snapshots WHERE financial_item_id = ?1
                 ORDER BY valuation_date DESC, id DESC",
                params![item_id],
            )
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn get_valuation(&self, id: i64) -> Result<Option<ValuationSnapshot>> {
        let rows = self
            .conn
            .query("SELECT * FROM valuation_snapshots WHERE id = ?1", params![id])
            .await?;
        crate::db::map_opt(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_valuation(
        &self,
        item_id: i64,
        valuation_date: &str,
        native_value: f64,
        currency: &str,
        gbp_value: Option<f64>,
        fx_rate: Option<f64>,
        fx_rate_date: Option<&str>,
        source_type: &str,
        source_reference: Option<&str>,
        confidence: &str,
        contributions: Option<f64>,
        withdrawals: Option<f64>,
        notes: Option<&str>,
    ) -> Result<ValuationSnapshot, FinanceError> {
        if self
            .get_financial_item(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .is_none()
        {
            return Err(FinanceError::not_found("financial item not found"));
        }
        finance::validate_iso_date("valuation_date", valuation_date)?;
        if let Some(d) = fx_rate_date {
            finance::validate_iso_date("fx_rate_date", d)?;
        }
        finance::validate_one_of("source_type", source_type, finance::SOURCE_TYPES)?;
        finance::validate_one_of("confidence", confidence, finance::CONFIDENCE)?;
        if let Some(c) = contributions {
            finance::validate_finite("contributions", c)?;
        }
        if let Some(w) = withdrawals {
            finance::validate_finite("withdrawals", w)?;
        }
        let (gbp, rate) = finance::resolve_gbp(native_value, currency, gbp_value, fx_rate)?;
        let ts = now();
        self.conn
            .execute(
                "INSERT INTO valuation_snapshots (
                    financial_item_id, valuation_date, native_value, currency, gbp_value,
                    fx_rate, fx_rate_date, source_type, source_reference, confidence,
                    contributions, withdrawals, notes, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    item_id,
                    valuation_date.trim(),
                    native_value,
                    currency,
                    gbp,
                    rate,
                    opt_str(fx_rate_date),
                    source_type,
                    opt_str(source_reference),
                    confidence,
                    contributions,
                    withdrawals,
                    opt_str(notes),
                    ts,
                    ts,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let id = self.conn.last_insert_rowid();
        self.get_valuation(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("valuation disappeared after insert"))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_valuation(
        &self,
        id: i64,
        valuation_date: &str,
        native_value: f64,
        currency: &str,
        gbp_value: Option<f64>,
        fx_rate: Option<f64>,
        fx_rate_date: Option<&str>,
        source_type: &str,
        source_reference: Option<&str>,
        confidence: &str,
        contributions: Option<f64>,
        withdrawals: Option<f64>,
        notes: Option<&str>,
    ) -> Result<ValuationSnapshot, FinanceError> {
        if self
            .get_valuation(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .is_none()
        {
            return Err(FinanceError::not_found("valuation not found"));
        }
        finance::validate_iso_date("valuation_date", valuation_date)?;
        if let Some(d) = fx_rate_date {
            finance::validate_iso_date("fx_rate_date", d)?;
        }
        finance::validate_one_of("source_type", source_type, finance::SOURCE_TYPES)?;
        finance::validate_one_of("confidence", confidence, finance::CONFIDENCE)?;
        if let Some(c) = contributions {
            finance::validate_finite("contributions", c)?;
        }
        if let Some(w) = withdrawals {
            finance::validate_finite("withdrawals", w)?;
        }
        let (gbp, rate) = finance::resolve_gbp(native_value, currency, gbp_value, fx_rate)?;
        self.conn
            .execute(
                "UPDATE valuation_snapshots SET
                    valuation_date = ?1, native_value = ?2, currency = ?3, gbp_value = ?4,
                    fx_rate = ?5, fx_rate_date = ?6, source_type = ?7, source_reference = ?8,
                    confidence = ?9, contributions = ?10, withdrawals = ?11, notes = ?12,
                    updated_at = ?13
                 WHERE id = ?14",
                params![
                    valuation_date.trim(),
                    native_value,
                    currency,
                    gbp,
                    rate,
                    opt_str(fx_rate_date),
                    source_type,
                    opt_str(source_reference),
                    confidence,
                    contributions,
                    withdrawals,
                    opt_str(notes),
                    now(),
                    id,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        self.get_valuation(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("valuation not found"))
    }

    pub async fn list_item_holdings(&self, item_id: i64) -> Result<Vec<InvestmentHolding>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM investment_holdings WHERE financial_item_id = ?1 ORDER BY holding_name",
                params![item_id],
            )
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn get_item_holding(&self, id: i64) -> Result<Option<InvestmentHolding>> {
        let rows = self
            .conn
            .query("SELECT * FROM investment_holdings WHERE id = ?1", params![id])
            .await?;
        crate::db::map_opt(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_item_holding(
        &self,
        item_id: i64,
        holding_name: &str,
        ticker_or_identifier: Option<&str>,
        units: Option<f64>,
        price: Option<f64>,
        price_currency: Option<&str>,
        cost_basis: Option<f64>,
        target_allocation: Option<f64>,
        target_return: Option<f64>,
        valuation_date: &str,
        native_value: f64,
        currency: &str,
        gbp_value: Option<f64>,
        source_reference: Option<&str>,
        notes: Option<&str>,
    ) -> Result<InvestmentHolding, FinanceError> {
        let item = self
            .get_financial_item(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))?;
        if !finance::holdings_allowed(&item.class) {
            return Err(FinanceError::invalid(
                "holdings may only belong to investment or pension items",
            ));
        }
        finance::validate_name(holding_name)?;
        finance::validate_iso_date("valuation_date", valuation_date)?;
        if let Some(u) = units {
            finance::validate_non_negative("units", u)?;
        }
        if let Some(p) = price {
            finance::validate_non_negative("price", p)?;
        }
        if let Some(c) = cost_basis {
            finance::validate_non_negative("cost_basis", c)?;
        }
        if let Some(a) = target_allocation {
            finance::validate_finite("target_allocation", a)?;
        }
        if let Some(r) = target_return {
            finance::validate_finite("target_return", r)?;
        }
        if let Some(ccy) = price_currency {
            finance::validate_currency(ccy)?;
        }
        let (gbp, _) = finance::resolve_gbp(native_value, currency, gbp_value, None)?;
        let ts = now();
        self.conn
            .execute(
                "INSERT INTO investment_holdings (
                    financial_item_id, holding_name, ticker_or_identifier, units, price,
                    price_currency, cost_basis, target_allocation, target_return,
                    valuation_date, native_value, currency, gbp_value, source_reference,
                    notes, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    item_id,
                    holding_name.trim(),
                    opt_str(ticker_or_identifier),
                    units,
                    price,
                    opt_str(price_currency),
                    cost_basis,
                    target_allocation,
                    target_return,
                    valuation_date.trim(),
                    native_value,
                    currency,
                    gbp,
                    opt_str(source_reference),
                    opt_str(notes),
                    ts,
                    ts,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        // A parent with holdings must be a roll-up so net worth does not also count its total.
        self.conn
            .execute(
                "UPDATE financial_items SET is_rollup = 1, updated_at = ?1 WHERE id = ?2",
                params![ts, item_id],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let id = self.conn.last_insert_rowid();
        self.get_item_holding(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("holding disappeared after insert"))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_item_holding(
        &self,
        id: i64,
        holding_name: &str,
        ticker_or_identifier: Option<&str>,
        units: Option<f64>,
        price: Option<f64>,
        price_currency: Option<&str>,
        cost_basis: Option<f64>,
        target_allocation: Option<f64>,
        target_return: Option<f64>,
        valuation_date: &str,
        native_value: f64,
        currency: &str,
        gbp_value: Option<f64>,
        source_reference: Option<&str>,
        notes: Option<&str>,
    ) -> Result<InvestmentHolding, FinanceError> {
        if self
            .get_item_holding(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .is_none()
        {
            return Err(FinanceError::not_found("holding not found"));
        }
        finance::validate_name(holding_name)?;
        finance::validate_iso_date("valuation_date", valuation_date)?;
        if let Some(u) = units {
            finance::validate_non_negative("units", u)?;
        }
        if let Some(p) = price {
            finance::validate_non_negative("price", p)?;
        }
        if let Some(c) = cost_basis {
            finance::validate_non_negative("cost_basis", c)?;
        }
        if let Some(a) = target_allocation {
            finance::validate_finite("target_allocation", a)?;
        }
        if let Some(r) = target_return {
            finance::validate_finite("target_return", r)?;
        }
        if let Some(ccy) = price_currency {
            finance::validate_currency(ccy)?;
        }
        let (gbp, _) = finance::resolve_gbp(native_value, currency, gbp_value, None)?;
        self.conn
            .execute(
                "UPDATE investment_holdings SET
                    holding_name = ?1, ticker_or_identifier = ?2, units = ?3, price = ?4,
                    price_currency = ?5, cost_basis = ?6, target_allocation = ?7,
                    target_return = ?8, valuation_date = ?9, native_value = ?10,
                    currency = ?11, gbp_value = ?12, source_reference = ?13, notes = ?14,
                    updated_at = ?15
                 WHERE id = ?16",
                params![
                    holding_name.trim(),
                    opt_str(ticker_or_identifier),
                    units,
                    price,
                    opt_str(price_currency),
                    cost_basis,
                    target_allocation,
                    target_return,
                    valuation_date.trim(),
                    native_value,
                    currency,
                    gbp,
                    opt_str(source_reference),
                    opt_str(notes),
                    now(),
                    id,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        self.get_item_holding(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("holding not found"))
    }

    pub async fn delete_item_holding(&self, id: i64) -> Result<(), FinanceError> {
        let existing = self
            .get_item_holding(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("holding not found"))?;
        self.conn
            .execute("DELETE FROM investment_holdings WHERE id = ?1", params![id])
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let remaining = self
            .list_item_holdings(existing.financial_item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        if remaining.is_empty() {
            self.conn
                .execute(
                    "UPDATE financial_items SET is_rollup = 0, updated_at = ?1 WHERE id = ?2
                     AND NOT EXISTS (SELECT 1 FROM financial_items c WHERE c.parent_item_id = financial_items.id AND c.class != 'mortgage')",
                    params![now(), existing.financial_item_id],
                )
                .await
                .ok();
        }
        Ok(())
    }

    pub async fn get_property(&self, item_id: i64) -> Result<Option<PropertyDetails>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM property_details WHERE financial_item_id = ?1",
                params![item_id],
            )
            .await?;
        crate::db::map_opt(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_property(
        &self,
        item_id: i64,
        region: &str,
        postcode_area: Option<&str>,
        valuation_method: &str,
        valuation_url: Option<&str>,
        valuation_cadence: &str,
        comparable_sales_notes: Option<&str>,
        valuation_confidence: &str,
        last_reviewed: Option<&str>,
    ) -> Result<PropertyDetails, FinanceError> {
        let item = self
            .get_financial_item(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))?;
        if item.class != "property" {
            return Err(FinanceError::invalid("item is not a property"));
        }
        if region.trim().is_empty() {
            return Err(FinanceError::invalid("region is required"));
        }
        finance::validate_one_of("valuation_method", valuation_method, finance::VALUATION_METHODS)?;
        finance::validate_one_of("valuation_cadence", valuation_cadence, finance::VALUATION_CADENCES)?;
        finance::validate_one_of("valuation_confidence", valuation_confidence, finance::CONFIDENCE)?;
        if let Some(d) = last_reviewed {
            finance::validate_iso_date("last_reviewed", d)?;
        }
        self.conn
            .execute(
                "INSERT INTO property_details (
                    financial_item_id, region, postcode_area, valuation_method, valuation_url,
                    valuation_cadence, comparable_sales_notes, valuation_confidence, last_reviewed
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(financial_item_id) DO UPDATE SET
                    region = excluded.region,
                    postcode_area = excluded.postcode_area,
                    valuation_method = excluded.valuation_method,
                    valuation_url = excluded.valuation_url,
                    valuation_cadence = excluded.valuation_cadence,
                    comparable_sales_notes = excluded.comparable_sales_notes,
                    valuation_confidence = excluded.valuation_confidence,
                    last_reviewed = excluded.last_reviewed",
                params![
                    item_id,
                    region.trim(),
                    opt_str(postcode_area),
                    valuation_method,
                    opt_str(valuation_url),
                    valuation_cadence,
                    opt_str(comparable_sales_notes),
                    valuation_confidence,
                    opt_str(last_reviewed),
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        self.get_property(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("property details missing after upsert"))
    }

    pub async fn get_mortgage(&self, item_id: i64) -> Result<Option<MortgageDetails>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM mortgage_details WHERE financial_item_id = ?1",
                params![item_id],
            )
            .await?;
        crate::db::map_opt(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_mortgage(
        &self,
        item_id: i64,
        property_item_id: Option<i64>,
        repayment_type: &str,
        outstanding_balance: f64,
        interest_rate: Option<f64>,
        rate_type: Option<&str>,
        fixed_period_expiry: Option<&str>,
        payment_amount: Option<f64>,
        payment_frequency: Option<&str>,
        term_end_date: Option<&str>,
        overpayment_total: Option<f64>,
        projected_balance: Option<f64>,
        notes: Option<&str>,
    ) -> Result<MortgageDetails, FinanceError> {
        let item = self
            .get_financial_item(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))?;
        if item.class != "mortgage" {
            return Err(FinanceError::invalid("item is not a mortgage"));
        }
        finance::validate_one_of("repayment_type", repayment_type, finance::REPAYMENT_TYPES)?;
        finance::validate_non_negative("outstanding_balance", outstanding_balance)?;
        if let Some(r) = interest_rate {
            finance::validate_finite("interest_rate", r)?;
        }
        if let Some(rt) = rate_type {
            finance::validate_one_of("rate_type", rt, finance::RATE_TYPES)?;
        }
        if let Some(d) = fixed_period_expiry {
            finance::validate_iso_date("fixed_period_expiry", d)?;
        }
        if let Some(p) = payment_amount {
            finance::validate_non_negative("payment_amount", p)?;
        }
        if let Some(f) = payment_frequency {
            finance::validate_one_of("payment_frequency", f, finance::PAYMENT_FREQUENCIES)?;
        }
        if let Some(d) = term_end_date {
            finance::validate_iso_date("term_end_date", d)?;
        }
        if let Some(o) = overpayment_total {
            finance::validate_non_negative("overpayment_total", o)?;
        }
        if let Some(p) = projected_balance {
            finance::validate_non_negative("projected_balance", p)?;
        }
        let previous_property_id = self
            .get_mortgage(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .and_then(|m| m.property_item_id);
        if let Some(pid) = property_item_id {
            let prop = self
                .get_financial_item(pid)
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?
                .ok_or_else(|| FinanceError::not_found("property_item_id not found"))?;
            if prop.class != "property" {
                return Err(FinanceError::invalid("property_item_id must be a property item"));
            }
            self.conn
                .execute(
                    "UPDATE financial_items SET parent_item_id = ?1, updated_at = ?2 WHERE id = ?3",
                    params![pid, now(), item_id],
                )
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        } else if let Some(old_pid) = previous_property_id {
            // Clearing the property link also clears the parent pointer when it still
            // points at that property, so the mortgage can stand alone again.
            self.conn
                .execute(
                    "UPDATE financial_items SET parent_item_id = NULL, updated_at = ?1
                     WHERE id = ?2 AND parent_item_id = ?3",
                    params![now(), item_id, old_pid],
                )
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        }
        self.conn
            .execute(
                "INSERT INTO mortgage_details (
                    financial_item_id, property_item_id, repayment_type, outstanding_balance,
                    interest_rate, rate_type, fixed_period_expiry, payment_amount,
                    payment_frequency, term_end_date, overpayment_total, projected_balance, notes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(financial_item_id) DO UPDATE SET
                    property_item_id = excluded.property_item_id,
                    repayment_type = excluded.repayment_type,
                    outstanding_balance = excluded.outstanding_balance,
                    interest_rate = excluded.interest_rate,
                    rate_type = excluded.rate_type,
                    fixed_period_expiry = excluded.fixed_period_expiry,
                    payment_amount = excluded.payment_amount,
                    payment_frequency = excluded.payment_frequency,
                    term_end_date = excluded.term_end_date,
                    overpayment_total = excluded.overpayment_total,
                    projected_balance = excluded.projected_balance,
                    notes = excluded.notes",
                params![
                    item_id,
                    property_item_id,
                    repayment_type,
                    outstanding_balance,
                    interest_rate,
                    opt_str(rate_type),
                    opt_str(fixed_period_expiry),
                    payment_amount,
                    opt_str(payment_frequency),
                    opt_str(term_end_date),
                    overpayment_total,
                    projected_balance,
                    opt_str(notes),
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        self.get_mortgage(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("mortgage details missing after upsert"))
    }

    pub async fn get_tax_details(&self, item_id: i64) -> Result<Option<TaxLiabilityDetails>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM tax_liability_details WHERE financial_item_id = ?1",
                params![item_id],
            )
            .await?;
        crate::db::map_opt(rows).await
    }

    pub async fn get_receivable(&self, item_id: i64) -> Result<Option<ReceivableDetails>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM receivable_details WHERE financial_item_id = ?1",
                params![item_id],
            )
            .await?;
        crate::db::map_opt(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_receivable(
        &self,
        item_id: i64,
        debtor: &str,
        invoice_reference: &str,
        due_date: &str,
        expected_amount: f64,
        currency: &str,
        gbp_value: Option<f64>,
        status: &str,
    ) -> Result<ReceivableDetails, FinanceError> {
        let item = self
            .get_financial_item(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("financial item not found"))?;
        if item.class != "receivable" {
            return Err(FinanceError::invalid("item is not a receivable"));
        }
        if debtor.trim().is_empty() {
            return Err(FinanceError::invalid("debtor is required"));
        }
        if invoice_reference.trim().is_empty() {
            return Err(FinanceError::invalid("invoice_reference is required"));
        }
        finance::validate_iso_date("due_date", due_date)?;
        finance::validate_one_of("status", status, finance::RECEIVABLE_STATUSES)?;
        let (gbp, _) = finance::resolve_gbp(expected_amount, currency, gbp_value, None)?;
        self.conn
            .execute(
                "INSERT INTO receivable_details (
                    financial_item_id, debtor, invoice_reference, due_date,
                    expected_amount, currency, gbp_value, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(financial_item_id) DO UPDATE SET
                    debtor = excluded.debtor,
                    invoice_reference = excluded.invoice_reference,
                    due_date = excluded.due_date,
                    expected_amount = excluded.expected_amount,
                    currency = excluded.currency,
                    gbp_value = excluded.gbp_value,
                    status = excluded.status",
                params![
                    item_id,
                    debtor.trim(),
                    invoice_reference.trim(),
                    due_date.trim(),
                    expected_amount,
                    currency,
                    gbp,
                    status,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        self.get_receivable(item_id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("receivable details missing after upsert"))
    }

    pub async fn list_forecasts(&self) -> Result<Vec<ForecastScenario>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM forecast_scenarios ORDER BY scenario_type, name",
                (),
            )
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn get_forecast(&self, id: i64) -> Result<Option<ForecastScenario>> {
        let rows = self
            .conn
            .query("SELECT * FROM forecast_scenarios WHERE id = ?1", params![id])
            .await?;
        crate::db::map_opt(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_forecast(
        &self,
        name: &str,
        scenario_type: &str,
        financial_item_id: Option<i64>,
        forecast_start: &str,
        forecast_end: &str,
        starting_gbp_value: f64,
        target_return: Option<f64>,
        property_growth_rate: Option<f64>,
        mortgage_rate: Option<f64>,
        repayment_assumption: Option<&str>,
        fx_assumption: Option<f64>,
        contributions: Option<f64>,
        withdrawals: Option<f64>,
        ending_projected_value: Option<f64>,
        assumptions: &str,
        notes: Option<&str>,
    ) -> Result<ForecastScenario, FinanceError> {
        finance::validate_name(name)?;
        finance::validate_one_of("scenario_type", scenario_type, finance::SCENARIO_TYPES)?;
        finance::validate_iso_date("forecast_start", forecast_start)?;
        finance::validate_iso_date("forecast_end", forecast_end)?;
        finance::validate_date_order(forecast_start, forecast_end)?;
        if let Some(iid) = financial_item_id {
            if self
                .get_financial_item(iid)
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?
                .is_none()
            {
                return Err(FinanceError::not_found("financial_item_id not found"));
            }
        }
        for (label, v) in [
            ("target_return", target_return),
            ("property_growth_rate", property_growth_rate),
            ("mortgage_rate", mortgage_rate),
            ("fx_assumption", fx_assumption),
            ("ending_projected_value", ending_projected_value),
        ] {
            if let Some(n) = v {
                finance::validate_finite(label, n)?;
            }
        }
        let annual = target_return.or(property_growth_rate);
        let projected = match ending_projected_value {
            Some(v) => v,
            None => finance::project_ending_value(
                starting_gbp_value,
                forecast_start,
                forecast_end,
                annual,
                contributions,
                withdrawals,
            )?,
        };
        let ts = now();
        self.conn
            .execute(
                "INSERT INTO forecast_scenarios (
                    name, scenario_type, financial_item_id, forecast_start, forecast_end,
                    starting_gbp_value, target_return, property_growth_rate, mortgage_rate,
                    repayment_assumption, fx_assumption, contributions, withdrawals,
                    ending_projected_value, assumptions, notes, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                params![
                    name.trim(),
                    scenario_type,
                    financial_item_id,
                    forecast_start.trim(),
                    forecast_end.trim(),
                    starting_gbp_value,
                    target_return,
                    property_growth_rate,
                    mortgage_rate,
                    opt_str(repayment_assumption),
                    fx_assumption,
                    contributions,
                    withdrawals,
                    projected,
                    assumptions,
                    opt_str(notes),
                    ts,
                    ts,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        let id = self.conn.last_insert_rowid();
        self.get_forecast(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::invalid("forecast disappeared after insert"))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_forecast(
        &self,
        id: i64,
        name: &str,
        scenario_type: &str,
        financial_item_id: Option<i64>,
        forecast_start: &str,
        forecast_end: &str,
        starting_gbp_value: f64,
        target_return: Option<f64>,
        property_growth_rate: Option<f64>,
        mortgage_rate: Option<f64>,
        repayment_assumption: Option<&str>,
        fx_assumption: Option<f64>,
        contributions: Option<f64>,
        withdrawals: Option<f64>,
        ending_projected_value: Option<f64>,
        assumptions: &str,
        notes: Option<&str>,
    ) -> Result<ForecastScenario, FinanceError> {
        if self
            .get_forecast(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .is_none()
        {
            return Err(FinanceError::not_found("forecast not found"));
        }
        finance::validate_name(name)?;
        finance::validate_one_of("scenario_type", scenario_type, finance::SCENARIO_TYPES)?;
        finance::validate_iso_date("forecast_start", forecast_start)?;
        finance::validate_iso_date("forecast_end", forecast_end)?;
        finance::validate_date_order(forecast_start, forecast_end)?;
        if let Some(iid) = financial_item_id {
            if self
                .get_financial_item(iid)
                .await
                .map_err(|e| FinanceError::Invalid(e.to_string()))?
                .is_none()
            {
                return Err(FinanceError::not_found("financial_item_id not found"));
            }
        }
        for (label, v) in [
            ("target_return", target_return),
            ("property_growth_rate", property_growth_rate),
            ("mortgage_rate", mortgage_rate),
            ("fx_assumption", fx_assumption),
            ("ending_projected_value", ending_projected_value),
        ] {
            if let Some(n) = v {
                finance::validate_finite(label, n)?;
            }
        }
        let annual = target_return.or(property_growth_rate);
        let projected = match ending_projected_value {
            Some(v) => v,
            None => finance::project_ending_value(
                starting_gbp_value,
                forecast_start,
                forecast_end,
                annual,
                contributions,
                withdrawals,
            )?,
        };
        self.conn
            .execute(
                "UPDATE forecast_scenarios SET
                    name = ?1, scenario_type = ?2, financial_item_id = ?3, forecast_start = ?4,
                    forecast_end = ?5, starting_gbp_value = ?6, target_return = ?7,
                    property_growth_rate = ?8, mortgage_rate = ?9, repayment_assumption = ?10,
                    fx_assumption = ?11, contributions = ?12, withdrawals = ?13,
                    ending_projected_value = ?14, assumptions = ?15, notes = ?16, updated_at = ?17
                 WHERE id = ?18",
                params![
                    name.trim(),
                    scenario_type,
                    financial_item_id,
                    forecast_start.trim(),
                    forecast_end.trim(),
                    starting_gbp_value,
                    target_return,
                    property_growth_rate,
                    mortgage_rate,
                    opt_str(repayment_assumption),
                    fx_assumption,
                    contributions,
                    withdrawals,
                    projected,
                    assumptions,
                    opt_str(notes),
                    now(),
                    id,
                ],
            )
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        self.get_forecast(id)
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?
            .ok_or_else(|| FinanceError::not_found("forecast not found"))
    }

    pub async fn delete_forecast(&self, id: i64) -> Result<(), FinanceError> {
        let n = self
            .conn
            .execute("DELETE FROM forecast_scenarios WHERE id = ?1", params![id])
            .await
            .map_err(|e| FinanceError::Invalid(e.to_string()))?;
        if n == 0 {
            return Err(FinanceError::not_found("forecast not found"));
        }
        Ok(())
    }

    pub async fn list_all_snapshots(&self) -> Result<Vec<ValuationSnapshot>> {
        let rows = self
            .conn
            .query("SELECT * FROM valuation_snapshots ORDER BY valuation_date, id", ())
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn list_all_item_holdings(&self) -> Result<Vec<InvestmentHolding>> {
        let rows = self
            .conn
            .query("SELECT * FROM investment_holdings", ())
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn list_all_mortgages(&self) -> Result<Vec<MortgageDetails>> {
        let rows = self.conn.query("SELECT * FROM mortgage_details", ()).await?;
        crate::db::map_rows(rows).await
    }

    pub async fn list_mortgages_for_property(
        &self,
        property_item_id: i64,
    ) -> Result<Vec<MortgageDetails>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM mortgage_details WHERE property_item_id = ?1
                 ORDER BY financial_item_id",
                params![property_item_id],
            )
            .await?;
        crate::db::map_rows(rows).await
    }

    pub async fn load_finance_world(&self) -> Result<FinanceWorld> {
        let items = self.list_financial_items().await?;
        let snapshots = self.list_all_snapshots().await?;
        let holdings = self.list_all_item_holdings().await?;
        let mortgages = self.list_all_mortgages().await?;
        let accounts_raw = self.list_all_enabled_accounts().await?;
        let broker_rows = self.list_holdings().await?;
        let quotes = self.all_latest_quotes().await?;
        let quote_by_symbol: std::collections::HashMap<_, _> =
            quotes.iter().map(|q| (q.symbol.clone(), q.price)).collect();
        let broker_holdings_gbp = broker_rows
            .iter()
            .map(|h| {
                quote_by_symbol
                    .get(&h.symbol)
                    .map(|p| p * h.quantity)
                    .unwrap_or(0.0)
            })
            .sum();
        Ok(FinanceWorld {
            items: items
                .into_iter()
                .map(|i| ItemView {
                    id: i.id,
                    name: i.name,
                    class: i.class,
                    item_type: i.item_type,
                    owner_entity: i.owner_entity,
                    parent_item_id: i.parent_item_id,
                    currency: i.currency,
                    is_asset: i.is_asset,
                    is_liability: i.is_liability,
                    is_rollup: i.is_rollup,
                    is_active: i.is_active,
                })
                .collect(),
            snapshots: snapshots
                .into_iter()
                .map(|s| SnapshotView {
                    id: s.id,
                    financial_item_id: s.financial_item_id,
                    valuation_date: s.valuation_date,
                    native_value: s.native_value,
                    currency: s.currency,
                    gbp_value: s.gbp_value,
                    confidence: s.confidence,
                    source_type: s.source_type,
                })
                .collect(),
            holdings: holdings
                .into_iter()
                .map(|h| HoldingView {
                    id: h.id,
                    financial_item_id: h.financial_item_id,
                    holding_name: h.holding_name,
                    gbp_value: h.gbp_value,
                    valuation_date: h.valuation_date,
                })
                .collect(),
            mortgages: mortgages
                .into_iter()
                .map(|m| MortgageView {
                    financial_item_id: m.financial_item_id,
                    property_item_id: m.property_item_id,
                    outstanding_balance: m.outstanding_balance,
                    repayment_type: m.repayment_type,
                })
                .collect(),
            accounts: accounts_raw
                .into_iter()
                .map(|a| AccountBalanceView {
                    id: a.id,
                    name: a
                        .custom_display_name
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(a.display_name),
                    kind: a.kind,
                    account_type: a.account_type,
                    balance_cents: a.current_balance_cents,
                    currency: a.currency,
                })
                .collect(),
            broker_holdings_gbp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use crate::finance;
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

    #[tokio::test]
    async fn create_manual_item_and_snapshot_history() {
        let db = test_db().await;
        let item = db
            .create_financial_item(
                "NS&I premium bonds",
                "cash",
                "nsi_premium_bonds",
                "personal",
                Some("NS&I"),
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        assert_eq!(item.is_asset, 1);
        assert_eq!(item.is_liability, 0);
        db.create_valuation(
            item.id,
            "2026-01-01",
            10_000.0,
            "GBP",
            None,
            None,
            None,
            "statement",
            None,
            "confirmed",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        db.create_valuation(
            item.id,
            "2026-06-01",
            12_000.0,
            "GBP",
            None,
            None,
            None,
            "portal",
            None,
            "confirmed",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let snaps = db.list_valuations(item.id).await.unwrap();
        assert_eq!(snaps.len(), 2, "new valuations must not overwrite history");
        assert_eq!(snaps[0].gbp_value, 12_000.0);
        let world = db.load_finance_world().await.unwrap();
        let r = finance::compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 12_000.0);
    }

    #[tokio::test]
    async fn native_to_gbp_stored_separately() {
        let db = test_db().await;
        let item = db
            .create_financial_item(
                "Wise EUR",
                "cash",
                "fx_account",
                "personal",
                None,
                None,
                None,
                "EUR",
                false,
                None,
            )
            .await
            .unwrap();
        let v = db
            .create_valuation(
                item.id,
                "2026-08-01",
                10_000.0,
                "EUR",
                None,
                Some(0.85),
                Some("2026-08-01"),
                "portal",
                None,
                "estimated",
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(v.native_value, 10_000.0);
        assert!((v.gbp_value - 8_500.0).abs() < 1e-9);
        assert_eq!(v.currency, "EUR");
    }

    #[tokio::test]
    async fn sipp_holdings_and_one_fund_pension() {
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
                false,
                None,
            )
            .await
            .unwrap();
        db.create_valuation(
            sipp.id, "2026-08-01", 100_000.0, "GBP", None, None, None,
            "portal", None, "confirmed", None, None, None,
        )
        .await
        .unwrap();
        db.create_item_holding(
            sipp.id, "Vanguard FTSE", Some("VWRP.L"), Some(100.0), Some(400.0),
            Some("GBP"), Some(35_000.0), Some(0.6), Some(0.07), "2026-08-01",
            60_000.0, "GBP", None, None, None,
        )
        .await
        .unwrap();
        db.create_item_holding(
            sipp.id, "Cash", None, None, None, None, None, Some(0.4), None,
            "2026-08-01", 40_000.0, "GBP", None, None, None,
        )
        .await
        .unwrap();
        let nest = db
            .create_financial_item(
                "NEST",
                "pension",
                "workplace_pension",
                "personal",
                None,
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        db.create_valuation(
            nest.id, "2026-08-01", 22_000.0, "GBP", None, None, None,
            "statement", None, "confirmed", None, None, None,
        )
        .await
        .unwrap();
        let world = db.load_finance_world().await.unwrap();
        let r = finance::compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 122_000.0);
        assert!(r.counted.iter().any(|c| c.source == "investment_holdings"));
        assert!(r.counted.iter().any(|c| c.name == "NEST"));
    }

    #[tokio::test]
    async fn holdings_rejected_on_cash_item() {
        let db = test_db().await;
        let cash = db
            .create_financial_item("NS&I", "cash", "nsi_savings", "personal", None, None, None, "GBP", false, None)
            .await
            .unwrap();
        let err = db
            .create_item_holding(
                cash.id, "x", None, None, None, None, None, None, None,
                "2026-08-01", 1.0, "GBP", None, None, None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("investment or pension"));
    }

    #[tokio::test]
    async fn property_mortgage_tax_receivable_and_forecast() {
        let db = test_db().await;
        let prop = db
            .create_financial_item(
                "Grangeside",
                "property",
                "residential",
                "personal",
                None,
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        db.upsert_property(
            prop.id,
            "East Yorkshire",
            Some("HU"),
            "portal_estimate",
            Some("https://example.invalid/grangeside"),
            "six_monthly",
            Some("Local comps around £270–290k"),
            "estimated",
            Some("2026-08-01"),
        )
        .await
        .unwrap();
        db.create_valuation(
            prop.id, "2026-08-01", 280_000.0, "GBP", None, None, None,
            "valuation_site", Some("https://example.invalid/grangeside"), "estimated",
            None, None, None,
        )
        .await
        .unwrap();

        let mtg = db
            .create_financial_item(
                "Grangeside IO",
                "mortgage",
                "interest_only",
                "personal",
                Some("Lender"),
                None,
                Some(prop.id),
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        db.upsert_mortgage(
            mtg.id, Some(prop.id), "interest_only", 120_000.0, Some(0.045),
            Some("fixed"), Some("2027-03-01"), Some(450.0), Some("monthly"),
            Some("2045-01-01"), Some(2_000.0), None, None,
        )
        .await
        .unwrap();

        let ci = db
            .create_financial_item(
                "Future repayment",
                "mortgage",
                "capital_and_interest",
                "personal",
                None, None, None, "GBP", false, None,
            )
            .await
            .unwrap();
        db.upsert_mortgage(
            ci.id, None, "capital_and_interest", 80_000.0, Some(0.04),
            Some("tracker"), None, Some(620.0), Some("monthly"),
            Some("2040-01-01"), None, Some(76_000.0), None,
        )
        .await
        .unwrap();

        let vat = db
            .create_financial_item("VAT Q1", "tax_liability", "vat", "limited_company", None, None, None, "GBP", false, None)
            .await
            .unwrap();
        db.create_valuation(vat.id, "2026-08-01", 3_000.0, "GBP", None, None, None, "statement", None, "confirmed", None, None, None).await.unwrap();
        let rec = db
            .create_financial_item("Invoice 9", "receivable", "invoice", "limited_company", None, None, None, "GBP", false, None)
            .await
            .unwrap();
        db.upsert_receivable(rec.id, "Acme Ltd", "INV-9", "2026-09-01", 1_500.0, "GBP", None, "open").await.unwrap();
        db.create_valuation(rec.id, "2026-08-01", 1_500.0, "GBP", None, None, None, "statement", None, "confirmed", None, None, None).await.unwrap();

        db.create_forecast(
            "Base plan", "base", Some(prop.id), "2026-08-01", "2031-08-01",
            280_000.0, None, Some(0.03), None, None, None, None, None, None,
            "3% property growth — user input, not advice", None,
        )
        .await
        .unwrap();

        let world = db.load_finance_world().await.unwrap();
        let r = finance::compute_net_worth(&world, None);
        assert!((r.assets_gbp - 281_500.0).abs() < 1e-6);
        assert!((r.liabilities_gbp - 203_000.0).abs() < 1e-6);
        assert!((r.net_worth_gbp - 78_500.0).abs() < 1e-6);
        assert!(r.excluded.iter().any(|e| e.name == "forecasts"));
        assert_eq!(db.list_forecasts().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn mortgage_property_link_roundtrip_and_validation() {
        let db = test_db().await;
        let prop_a = db
            .create_financial_item(
                "64 Grangeside",
                "property",
                "residential",
                "personal",
                None,
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        let prop_b = db
            .create_financial_item(
                "Other house",
                "property",
                "residential",
                "personal",
                None,
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        let cash = db
            .create_financial_item(
                "Not a house",
                "cash",
                "savings",
                "personal",
                None,
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();
        let mtg = db
            .create_financial_item(
                "IO mortgage",
                "mortgage",
                "interest_only",
                "personal",
                None,
                None,
                None,
                "GBP",
                false,
                None,
            )
            .await
            .unwrap();

        let unlinked = db
            .upsert_mortgage(
                mtg.id,
                None,
                "interest_only",
                120_000.0,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(unlinked.property_item_id, None);
        assert!(db.list_mortgages_for_property(prop_a.id).await.unwrap().is_empty());

        let linked = db
            .upsert_mortgage(
                mtg.id,
                Some(prop_a.id),
                "interest_only",
                120_000.0,
                Some(0.04),
                Some("fixed"),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(linked.property_item_id, Some(prop_a.id));
        let fetched = db.get_mortgage(mtg.id).await.unwrap().unwrap();
        assert_eq!(fetched.property_item_id, Some(prop_a.id));
        let for_a = db.list_mortgages_for_property(prop_a.id).await.unwrap();
        assert_eq!(for_a.len(), 1);
        assert_eq!(for_a[0].financial_item_id, mtg.id);
        let parent = db.get_financial_item(mtg.id).await.unwrap().unwrap();
        assert_eq!(parent.parent_item_id, Some(prop_a.id));

        db.create_valuation(
            prop_a.id,
            "2026-08-01",
            180_000.0,
            "GBP",
            None,
            None,
            None,
            "manual_estimate",
            None,
            "estimated",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let world = db.load_finance_world().await.unwrap();
        let r = finance::compute_net_worth(&world, None);
        assert_eq!(r.assets_gbp, 180_000.0);
        assert_eq!(r.liabilities_gbp, 120_000.0);
        assert_eq!(r.counted.len(), 2);

        let moved = db
            .upsert_mortgage(
                mtg.id,
                Some(prop_b.id),
                "interest_only",
                120_000.0,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(moved.property_item_id, Some(prop_b.id));
        assert!(db.list_mortgages_for_property(prop_a.id).await.unwrap().is_empty());
        assert_eq!(db.list_mortgages_for_property(prop_b.id).await.unwrap().len(), 1);

        let cleared = db
            .upsert_mortgage(
                mtg.id,
                None,
                "interest_only",
                120_000.0,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(cleared.property_item_id, None);
        let after_clear = db.get_financial_item(mtg.id).await.unwrap().unwrap();
        assert_eq!(after_clear.parent_item_id, None);

        let err = db
            .upsert_mortgage(
                mtg.id,
                Some(cash.id),
                "interest_only",
                120_000.0,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("property item"));

        let missing = db
            .upsert_mortgage(
                mtg.id,
                Some(999_999),
                "interest_only",
                120_000.0,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn circular_parent_rejected_and_delete_deactivates() {
        let db = test_db().await;
        let a = db
            .create_financial_item("A", "pension", "sipp", "personal", None, None, None, "GBP", true, None)
            .await
            .unwrap();
        let b = db
            .create_financial_item("B", "investment", "fund", "personal", None, None, Some(a.id), "GBP", false, None)
            .await
            .unwrap();
        let err = db
            .update_financial_item(a.id, "A", "sipp", "personal", None, None, Some(b.id), "GBP", true, true, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cycle"));

        db.create_valuation(a.id, "2026-08-01", 1.0, "GBP", None, None, None, "manual_estimate", None, "provisional", None, None, None).await.unwrap();
        let result = db.delete_or_deactivate_financial_item(a.id).await.unwrap();
        assert_eq!(result["deactivated"], true);
        let after = db.get_financial_item(a.id).await.unwrap().unwrap();
        assert_eq!(after.is_active, 0);
    }

    #[tokio::test]
    async fn invalid_inputs_rejected() {
        let db = test_db().await;
        assert!(db
            .create_financial_item("", "cash", "savings", "personal", None, None, None, "GBP", false, None)
            .await
            .is_err());
        assert!(db
            .create_financial_item("X", "spaceship", "x", "personal", None, None, None, "GBP", false, None)
            .await
            .is_err());
        let item = db
            .create_financial_item("FX", "cash", "fx_account", "personal", None, None, None, "USD", false, None)
            .await
            .unwrap();
        assert!(db
            .create_valuation(item.id, "not-a-date", 1.0, "USD", None, Some(0.8), None, "portal", None, "confirmed", None, None, None)
            .await
            .is_err());
        assert!(db
            .create_valuation(item.id, "2026-08-01", f64::NAN, "USD", None, Some(0.8), None, "portal", None, "confirmed", None, None, None)
            .await
            .is_err());
        assert!(db
            .create_valuation(item.id, "2026-08-01", 100.0, "usd", None, Some(0.8), None, "portal", None, "confirmed", None, None, None)
            .await
            .is_err());
        assert!(db
            .create_valuation(item.id, "2026-08-01", 100.0, "USD", None, Some(0.0), None, "portal", None, "confirmed", None, None, None)
            .await
            .is_err());
        assert!(db
            .create_valuation(item.id, "2026-08-01", 100.0, "USD", None, None, None, "portal", None, "confirmed", None, None, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn existing_manual_account_untouched() {
        let db = test_db().await;
        let a = db.create_manual_account("Smile current", "current", Some(300_532)).await.unwrap();
        db.create_financial_item("SIPP", "pension", "sipp", "personal", None, None, None, "GBP", false, None)
            .await
            .unwrap();
        let listed = db.list_all_enabled_accounts().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, a.id);
        assert_eq!(listed[0].current_balance_cents, Some(300_532));
        let world = db.load_finance_world().await.unwrap();
        let r = finance::compute_net_worth(&world, None);
        assert!((r.net_worth_gbp - 3005.32).abs() < 1e-6);
        assert!(r.counted.iter().any(|c| c.name == "Smile current"));
    }
}
