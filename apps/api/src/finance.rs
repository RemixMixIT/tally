//! Manual assets/liabilities: validation, enums, and the central net-worth calculator.
//!
//! The calculator is a pure function over an in-memory snapshot so API handlers, history
//! reconstruction, and unit tests share one implementation. Actual net worth is assets
//! minus liabilities: valuation items, plus enabled bank/manual account balances and
//! the broker/Yahoo portfolio. Forecasts and transactions never enter actual net worth.
//! Bank and broker figures are the latest available balances, not reconstructed history.

use chrono::NaiveDate;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

pub const ITEM_CLASSES: &[&str] = &[
    "cash",
    "investment",
    "pension",
    "trading",
    "property",
    "mortgage",
    "tax_liability",
    "receivable",
];

pub const SOURCE_TYPES: &[&str] = &[
    "bank_feed",
    "statement",
    "portal",
    "valuation_site",
    "comparable_sale",
    "manual_estimate",
    "other",
];

pub const CONFIDENCE: &[&str] = &["confirmed", "estimated", "provisional"];

pub const REPAYMENT_TYPES: &[&str] = &["interest_only", "capital_and_interest"];

pub const TAX_KINDS: &[&str] = &["vat", "corporation_tax", "personal_tax", "other"];

pub const RECEIVABLE_STATUSES: &[&str] =
    &["open", "part_paid", "overdue", "settled", "written_off"];

pub const SCENARIO_TYPES: &[&str] = &["conservative", "base", "optimistic"];

pub const VALUATION_METHODS: &[&str] = &[
    "portal_estimate",
    "comparable_sales",
    "surveyor",
    "manual_estimate",
    "other",
];

pub const VALUATION_CADENCES: &[&str] =
    &["monthly", "quarterly", "six_monthly", "yearly", "ad_hoc"];

pub const RATE_TYPES: &[&str] = &["fixed", "tracker", "variable", "other"];

pub const PAYMENT_FREQUENCIES: &[&str] = &["monthly", "quarterly", "yearly", "ad_hoc"];

/// Suggested `item_type` values per class. Other non-empty types are accepted so users
/// can label NS&I, Crowdcube, FX, etc. without a schema change.
pub fn suggested_item_types(class: &str) -> &'static [&'static str] {
    match class {
        "cash" => &[
            "current",
            "savings",
            "fx_account",
            "nsi_premium_bonds",
            "nsi_savings",
            "cash_isa",
            "other_cash",
        ],
        "investment" => &["isa", "gia", "crowdcube", "fund", "other_investment"],
        "pension" => &["sipp", "workplace_pension", "frozen_pension", "other_pension"],
        "trading" => &["trading_account", "gia", "other_trading"],
        "property" => &["residential", "buy_to_let", "other_property"],
        "mortgage" => &["interest_only", "capital_and_interest"],
        "tax_liability" => &["vat", "corporation_tax", "personal_tax", "other"],
        "receivable" => &["invoice", "other_receivable"],
        _ => &[],
    }
}

pub const HOLDING_CLASSES: &[&str] = &["investment", "pension"];

/// Holdings vs parent snapshot: warn if the absolute gap exceeds the larger of £50 or 1%.
pub const RECONCILE_FLOOR_GBP: f64 = 50.0;
pub const RECONCILE_PCT: f64 = 0.01;

#[derive(Debug, Error)]
pub enum FinanceError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
}

impl FinanceError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }
}

pub fn validate_class(class: &str) -> Result<(), FinanceError> {
    if ITEM_CLASSES.contains(&class) {
        Ok(())
    } else {
        Err(FinanceError::invalid(format!(
            "class must be one of: {}",
            ITEM_CLASSES.join(", ")
        )))
    }
}

pub fn validate_currency(code: &str) -> Result<(), FinanceError> {
    let t = code.trim();
    if t.len() == 3 && t.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(FinanceError::invalid(
            "currency must be a 3-letter ISO code (e.g. GBP, EUR, USD)".to_string(),
        ))
    }
}

pub fn validate_iso_date(label: &str, value: &str) -> Result<NaiveDate, FinanceError> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").map_err(|_| {
        FinanceError::invalid(format!("{label} must be an ISO date YYYY-MM-DD"))
    })
}

pub fn validate_date_order(start: &str, end: &str) -> Result<(), FinanceError> {
    let s = validate_iso_date("forecast_start", start)?;
    let e = validate_iso_date("forecast_end", end)?;
    if e < s {
        Err(FinanceError::invalid(
            "forecast_end must be on or after forecast_start".to_string(),
        ))
    } else {
        Ok(())
    }
}

pub fn validate_finite(label: &str, value: f64) -> Result<(), FinanceError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(FinanceError::invalid(format!(
            "{label} must be a finite number"
        )))
    }
}

pub fn validate_non_negative(label: &str, value: f64) -> Result<(), FinanceError> {
    validate_finite(label, value)?;
    if value < 0.0 {
        Err(FinanceError::invalid(format!(
            "{label} must be zero or positive"
        )))
    } else {
        Ok(())
    }
}

pub fn validate_rate(label: &str, value: f64) -> Result<(), FinanceError> {
    validate_finite(label, value)?;
    if value <= 0.0 {
        Err(FinanceError::invalid(format!("{label} must be greater than zero")))
    } else {
        Ok(())
    }
}

pub fn validate_one_of(label: &str, value: &str, allowed: &[&str]) -> Result<(), FinanceError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(FinanceError::invalid(format!(
            "{label} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

pub fn validate_name(name: &str) -> Result<(), FinanceError> {
    if name.trim().is_empty() {
        Err(FinanceError::invalid("name is required"))
    } else {
        Ok(())
    }
}

/// Derive GBP from native + optional FX. Native and GBP are always stored separately.
pub fn resolve_gbp(
    native_value: f64,
    currency: &str,
    gbp_value: Option<f64>,
    fx_rate: Option<f64>,
) -> Result<(f64, Option<f64>), FinanceError> {
    validate_non_negative("native_value", native_value)?;
    validate_currency(currency)?;
    if let Some(r) = fx_rate {
        validate_rate("fx_rate", r)?;
    }
    if let Some(g) = gbp_value {
        validate_non_negative("gbp_value", g)?;
    }

    if currency == "GBP" {
        let gbp = gbp_value.unwrap_or(native_value);
        if (gbp - native_value).abs() > 0.01 {
            return Err(FinanceError::invalid(
                "gbp_value must match native_value when currency is GBP".to_string(),
            ));
        }
        if let Some(r) = fx_rate {
            if (r - 1.0).abs() > 0.001 {
                return Err(FinanceError::invalid(
                    "fx_rate must be omitted or 1 when currency is GBP".to_string(),
                ));
            }
        }
        return Ok((gbp, fx_rate));
    }

    match (gbp_value, fx_rate) {
        (Some(g), Some(r)) => {
            let implied = native_value * r;
            if implied > 0.0 && (g - implied).abs() / implied.max(1.0) > 0.02 {
                return Err(FinanceError::invalid(format!(
                    "gbp_value {g} is inconsistent with native_value {native_value} × fx_rate {r}"
                )));
            }
            Ok((g, Some(r)))
        }
        (Some(g), None) => {
            let rate = if native_value.abs() < 1e-9 {
                None
            } else {
                Some(g / native_value)
            };
            Ok((g, rate))
        }
        (None, Some(r)) => Ok((native_value * r, Some(r))),
        (None, None) => Err(FinanceError::invalid(
            "non-GBP valuations require gbp_value or fx_rate".to_string(),
        )),
    }
}

pub fn flags_for_class(class: &str) -> (i64, i64) {
    match class {
        "mortgage" | "tax_liability" => (0, 1),
        _ => (1, 0),
    }
}

pub fn holdings_allowed(class: &str) -> bool {
    HOLDING_CLASSES.contains(&class)
}

/// Walk parent ids already loaded in memory; used by create/update before write.
pub fn would_cycle(
    item_id: Option<i64>,
    new_parent: Option<i64>,
    parent_of: &HashMap<i64, Option<i64>>,
) -> bool {
    let Some(mut cursor) = new_parent else {
        return false;
    };
    let mut seen = HashSet::new();
    if let Some(id) = item_id {
        seen.insert(id);
    }
    loop {
        if !seen.insert(cursor) {
            return true;
        }
        match parent_of.get(&cursor) {
            Some(Some(p)) => cursor = *p,
            _ => return false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ItemView {
    pub id: i64,
    pub name: String,
    pub class: String,
    pub item_type: String,
    pub owner_entity: String,
    pub parent_item_id: Option<i64>,
    pub currency: String,
    pub is_asset: i64,
    pub is_liability: i64,
    pub is_rollup: i64,
    pub is_active: i64,
}

#[derive(Debug, Clone)]
pub struct SnapshotView {
    pub id: i64,
    pub financial_item_id: i64,
    pub valuation_date: String,
    pub native_value: f64,
    pub currency: String,
    pub gbp_value: f64,
    pub confidence: String,
    pub source_type: String,
}

#[derive(Debug, Clone)]
pub struct HoldingView {
    pub id: i64,
    pub financial_item_id: i64,
    pub holding_name: String,
    pub gbp_value: f64,
    pub valuation_date: String,
}

#[derive(Debug, Clone)]
pub struct MortgageView {
    pub financial_item_id: i64,
    pub property_item_id: Option<i64>,
    pub outstanding_balance: f64,
    pub repayment_type: String,
}

#[derive(Debug, Clone)]
pub struct AccountBalanceView {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub account_type: Option<String>,
    pub balance_cents: Option<i64>,
    pub currency: String,
}

/// Everything the calculator needs. Forecasts are intentionally absent.
#[derive(Debug, Clone, Default)]
pub struct FinanceWorld {
    pub items: Vec<ItemView>,
    pub snapshots: Vec<SnapshotView>,
    pub holdings: Vec<HoldingView>,
    pub mortgages: Vec<MortgageView>,
    /// Linked TrueLayer + manual/offline accounts (Banks). Latest balance only.
    pub accounts: Vec<AccountBalanceView>,
    /// Broker/Yahoo portfolio market value (Investments). Latest quotes only.
    pub broker_holdings_gbp: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CountedLine {
    pub financial_item_id: Option<i64>,
    pub name: String,
    pub class: String,
    pub role: String,
    pub gbp_value: f64,
    pub valuation_date: Option<String>,
    pub source: String,
}

/// Map a Banks-page account onto a net-worth class and asset/liability role.
pub fn classify_account(
    kind: &str,
    account_type: Option<&str>,
    balance_cents: i64,
) -> (&'static str, &'static str, f64) {
    let gbp = (balance_cents as f64).abs() / 100.0;
    let t = account_type.unwrap_or("");
    let class = if kind == "card" || t == "credit_card" || t == "business_credit_card" {
        "credit_card"
    } else {
        match t {
            "mortgage" => "mortgage",
            "tax_liability" => "tax_liability",
            "property" => "property",
            "receivables" => "receivable",
            "sipp" | "workplace_pension" | "frozen_pension" => "pension",
            "trading_account" => "trading",
            "investment" | "isa" => "investment",
            _ => "cash",
        }
    };
    let debt_type = class == "credit_card" || class == "mortgage" || class == "tax_liability";
    let role = if debt_type {
        if balance_cents < 0 {
            "asset"
        } else {
            "liability"
        }
    } else if balance_cents < 0 {
        "liability"
    } else {
        "asset"
    };
    (class, role, gbp)
}

#[derive(Debug, Clone, Serialize)]
pub struct ExcludedLine {
    pub financial_item_id: Option<i64>,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnreconciledLine {
    pub financial_item_id: i64,
    pub name: String,
    pub parent_gbp: f64,
    pub holdings_gbp: f64,
    pub difference_gbp: f64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassSubtotal {
    pub class: String,
    pub assets_gbp: f64,
    pub liabilities_gbp: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetWorthReport {
    pub as_of: Option<String>,
    pub assets_gbp: f64,
    pub liabilities_gbp: f64,
    pub net_worth_gbp: f64,
    pub by_class: Vec<ClassSubtotal>,
    pub counted: Vec<CountedLine>,
    pub excluded: Vec<ExcludedLine>,
    pub unreconciled: Vec<UnreconciledLine>,
}

fn latest_snapshot_as_of<'a>(
    snapshots: &'a [SnapshotView],
    item_id: i64,
    as_of: Option<&str>,
) -> Option<&'a SnapshotView> {
    snapshots
        .iter()
        .filter(|s| s.financial_item_id == item_id)
        .filter(|s| match as_of {
            Some(d) => s.valuation_date.as_str() <= d,
            None => true,
        })
        .max_by(|a, b| {
            a.valuation_date
                .cmp(&b.valuation_date)
                .then_with(|| a.id.cmp(&b.id))
        })
}

fn holdings_as_of<'a>(
    holdings: &'a [HoldingView],
    item_id: i64,
    as_of: Option<&str>,
) -> Vec<&'a HoldingView> {
    holdings
        .iter()
        .filter(|h| h.financial_item_id == item_id)
        .filter(|h| match as_of {
            Some(d) => h.valuation_date.as_str() <= d,
            None => true,
        })
        .collect()
}

fn gap_exceeds_tolerance(parent: f64, child_sum: f64) -> bool {
    let diff = (parent - child_sum).abs();
    let tol = RECONCILE_FLOOR_GBP.max(parent.abs() * RECONCILE_PCT);
    diff > tol + 1e-9
}

/// Single net-worth implementation reused by GET /api/net-worth, history, and explanation.
pub fn compute_net_worth(world: &FinanceWorld, as_of: Option<&str>) -> NetWorthReport {
    let mut counted = Vec::new();
    let mut excluded = Vec::new();
    let mut unreconciled = Vec::new();

    let children_of: HashMap<i64, Vec<i64>> = {
        let mut m: HashMap<i64, Vec<i64>> = HashMap::new();
        for it in &world.items {
            if let Some(p) = it.parent_item_id {
                m.entry(p).or_default().push(it.id);
            }
        }
        m
    };

    let mortgage_by_item: HashMap<i64, &MortgageView> = world
        .mortgages
        .iter()
        .map(|m| (m.financial_item_id, m))
        .collect();

    for item in &world.items {
        if item.is_active != 1 {
            excluded.push(ExcludedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                reason: "inactive".into(),
            });
            continue;
        }

        let snap = latest_snapshot_as_of(&world.snapshots, item.id, as_of);
        let holds = holdings_as_of(&world.holdings, item.id, as_of);
        let child_ids = children_of.get(&item.id).cloned().unwrap_or_default();
        // Child financial_items that are themselves assets (SIPP funds), not mortgages.
        let rollup_children: Vec<&ItemView> = child_ids
            .iter()
            .filter_map(|cid| world.items.iter().find(|c| c.id == *cid))
            .filter(|c| c.class != "mortgage" && c.is_active == 1)
            .collect();

        let has_holdings = !holds.is_empty();
        let is_rollup = item.is_rollup == 1 || has_holdings || !rollup_children.is_empty();

        if item.class == "mortgage" {
            // Count once: prefer the latest snapshot; otherwise the outstanding_balance field.
            // Never add snapshot and balance together. Interest is not a second liability.
            let (gbp, date, source) = if let Some(s) = snap {
                (s.gbp_value, Some(s.valuation_date.clone()), "valuation_snapshot")
            } else if let Some(m) = mortgage_by_item.get(&item.id) {
                (m.outstanding_balance, None, "mortgage_outstanding_balance")
            } else {
                excluded.push(ExcludedLine {
                    financial_item_id: Some(item.id),
                    name: item.name.clone(),
                    reason: "mortgage has no snapshot or outstanding balance".into(),
                });
                continue;
            };
            counted.push(CountedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                class: item.class.clone(),
                role: "liability".into(),
                gbp_value: gbp,
                valuation_date: date,
                source: source.into(),
            });
            continue;
        }

        if has_holdings {
            let holdings_gbp: f64 = holds.iter().map(|h| h.gbp_value).sum();
            if let Some(s) = snap {
                if gap_exceeds_tolerance(s.gbp_value, holdings_gbp) {
                    unreconciled.push(UnreconciledLine {
                        financial_item_id: item.id,
                        name: item.name.clone(),
                        parent_gbp: s.gbp_value,
                        holdings_gbp,
                        difference_gbp: s.gbp_value - holdings_gbp,
                        message: format!(
                            "Holdings total £{holdings_gbp:.2} does not match parent snapshot £{:.2} (tolerance £{:.2} or 1%)",
                            s.gbp_value,
                            RECONCILE_FLOOR_GBP.max(s.gbp_value.abs() * RECONCILE_PCT)
                        ),
                    });
                }
            }
            counted.push(CountedLine {
                financial_item_id: Some(item.id),
                name: format!("{} (holdings roll-up)", item.name),
                class: item.class.clone(),
                role: if item.is_liability == 1 {
                    "liability".into()
                } else {
                    "asset".into()
                },
                gbp_value: holdings_gbp,
                valuation_date: holds
                    .iter()
                    .map(|h| h.valuation_date.as_str())
                    .max()
                    .map(|s| s.to_string()),
                source: "investment_holdings".into(),
            });
            excluded.push(ExcludedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                reason: "parent total excluded because holdings are counted instead".into(),
            });
            continue;
        }

        if is_rollup && !rollup_children.is_empty() {
            // Children are counted on their own rows; parent account total is excluded.
            excluded.push(ExcludedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                reason: "rollup parent excluded; child items counted independently".into(),
            });
            continue;
        }

        if item.parent_item_id.is_some() && item.class != "mortgage" {
            // Counted as a child of a roll-up, or as a standalone child if the parent is not
            // a roll-up. Either way the child's own snapshot is the source of truth.
        }

        if let Some(s) = snap {
            let role = if item.is_liability == 1 {
                "liability"
            } else {
                "asset"
            };
            counted.push(CountedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                class: item.class.clone(),
                role: role.into(),
                gbp_value: s.gbp_value,
                valuation_date: Some(s.valuation_date.clone()),
                source: "valuation_snapshot".into(),
            });
        } else if item.class == "receivable" {
            excluded.push(ExcludedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                reason: "receivable has no valuation snapshot".into(),
            });
        } else {
            excluded.push(ExcludedLine {
                financial_item_id: Some(item.id),
                name: item.name.clone(),
                reason: "no valuation snapshot".into(),
            });
        }
    }

    for acct in &world.accounts {
        let Some(cents) = acct.balance_cents else {
            excluded.push(ExcludedLine {
                financial_item_id: None,
                name: acct.name.clone(),
                reason: "bank/manual account has no current balance".into(),
            });
            continue;
        };
        let (class, role, gbp) = classify_account(&acct.kind, acct.account_type.as_deref(), cents);
        if gbp.abs() < 1e-9 {
            continue;
        }
        counted.push(CountedLine {
            financial_item_id: None,
            name: acct.name.clone(),
            class: class.into(),
            role: role.into(),
            gbp_value: gbp,
            valuation_date: None,
            source: "bank_or_manual_account".into(),
        });
    }
    if world.broker_holdings_gbp.abs() > 1e-9 {
        counted.push(CountedLine {
            financial_item_id: None,
            name: "Investment portfolio".into(),
            class: "investment".into(),
            role: "asset".into(),
            gbp_value: world.broker_holdings_gbp,
            valuation_date: None,
            source: "broker_holdings".into(),
        });
    }

    // Forecasts are not in FinanceWorld; record that they are structurally excluded.
    excluded.push(ExcludedLine {
        financial_item_id: None,
        name: "forecasts".into(),
        reason: "forecast scenarios never contribute to actual net worth".into(),
    });
    excluded.push(ExcludedLine {
        financial_item_id: None,
        name: "transactions".into(),
        reason: "transactions are not asset values".into(),
    });

    let mut assets = 0.0;
    let mut liabilities = 0.0;
    let mut by_class_map: HashMap<String, (f64, f64)> = HashMap::new();
    for line in &counted {
        let entry = by_class_map.entry(line.class.clone()).or_insert((0.0, 0.0));
        if line.role == "liability" {
            liabilities += line.gbp_value;
            entry.1 += line.gbp_value;
        } else {
            assets += line.gbp_value;
            entry.0 += line.gbp_value;
        }
    }
    let mut by_class: Vec<ClassSubtotal> = ITEM_CLASSES
        .iter()
        .filter_map(|c| {
            by_class_map.get(*c).map(|(a, l)| ClassSubtotal {
                class: (*c).to_string(),
                assets_gbp: *a,
                liabilities_gbp: *l,
            })
        })
        .collect();
    let mut extra: Vec<ClassSubtotal> = by_class_map
        .iter()
        .filter(|(k, _)| !ITEM_CLASSES.contains(&k.as_str()))
        .map(|(k, (a, l))| ClassSubtotal {
            class: k.clone(),
            assets_gbp: *a,
            liabilities_gbp: *l,
        })
        .collect();
    extra.sort_by(|a, b| a.class.cmp(&b.class));
    by_class.extend(extra);
    by_class.retain(|c| c.assets_gbp.abs() > 1e-9 || c.liabilities_gbp.abs() > 1e-9);

    NetWorthReport {
        as_of: as_of.map(|s| s.to_string()),
        assets_gbp: assets,
        liabilities_gbp: liabilities,
        net_worth_gbp: assets - liabilities,
        by_class,
        counted,
        excluded,
        unreconciled,
    }
}

/// Compound a starting GBP value over a dated window. Target returns are user inputs.
pub fn project_ending_value(
    starting_gbp: f64,
    start: &str,
    end: &str,
    annual_rate: Option<f64>,
    contributions: Option<f64>,
    withdrawals: Option<f64>,
) -> Result<f64, FinanceError> {
    validate_non_negative("starting_gbp_value", starting_gbp)?;
    let s = validate_iso_date("forecast_start", start)?;
    let e = validate_iso_date("forecast_end", end)?;
    if e < s {
        return Err(FinanceError::invalid(
            "forecast_end must be on or after forecast_start".to_string(),
        ));
    }
    if let Some(r) = annual_rate {
        validate_finite("annual_rate", r)?;
    }
    let contrib = contributions.unwrap_or(0.0);
    let withd = withdrawals.unwrap_or(0.0);
    validate_finite("contributions", contrib)?;
    validate_finite("withdrawals", withd)?;
    let days = (e - s).num_days() as f64;
    let years = days / 365.25;
    let grown = starting_gbp * (1.0 + annual_rate.unwrap_or(0.0)).powf(years);
    Ok(grown + contrib - withd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        id: i64,
        name: &str,
        class: &str,
        parent: Option<i64>,
        rollup: i64,
        active: i64,
    ) -> ItemView {
        let (is_asset, is_liability) = flags_for_class(class);
        ItemView {
            id,
            name: name.into(),
            class: class.into(),
            item_type: class.into(),
            owner_entity: "personal".into(),
            parent_item_id: parent,
            currency: "GBP".into(),
            is_asset,
            is_liability,
            is_rollup: rollup,
            is_active: active,
        }
    }

    fn snap(id: i64, item_id: i64, date: &str, gbp: f64) -> SnapshotView {
        SnapshotView {
            id,
            financial_item_id: item_id,
            valuation_date: date.into(),
            native_value: gbp,
            currency: "GBP".into(),
            gbp_value: gbp,
            confidence: "confirmed".into(),
            source_type: "manual_estimate".into(),
        }
    }

    fn holding(id: i64, item_id: i64, name: &str, gbp: f64, date: &str) -> HoldingView {
        HoldingView {
            id,
            financial_item_id: item_id,
            holding_name: name.into(),
            gbp_value: gbp,
            valuation_date: date.into(),
        }
    }

    #[test]
    fn currency_and_date_validation() {
        assert!(validate_currency("GBP").is_ok());
        assert!(validate_currency("usd").is_err());
        assert!(validate_currency("GB").is_err());
        assert!(validate_iso_date("d", "2026-08-28").is_ok());
        assert!(validate_iso_date("d", "28/08/2026").is_err());
        assert!(validate_finite("n", f64::NAN).is_err());
        assert!(validate_finite("n", f64::INFINITY).is_err());
        assert!(validate_rate("fx", 0.0).is_err());
        assert!(validate_rate("fx", -1.0).is_err());
    }

    #[test]
    fn native_to_gbp_gbp_and_fx() {
        let (g, r) = resolve_gbp(100.0, "GBP", None, None).unwrap();
        assert_eq!(g, 100.0);
        assert!(r.is_none());
        let (g, r) = resolve_gbp(100.0, "EUR", None, Some(0.85)).unwrap();
        assert!((g - 85.0).abs() < 1e-9);
        assert_eq!(r, Some(0.85));
        let (g, r) = resolve_gbp(100.0, "USD", Some(78.0), None).unwrap();
        assert_eq!(g, 78.0);
        assert!((r.unwrap() - 0.78).abs() < 1e-9);
        assert!(resolve_gbp(100.0, "EUR", None, None).is_err());
        assert!(resolve_gbp(100.0, "GBP", Some(90.0), None).is_err());
        assert!(resolve_gbp(100.0, "EUR", Some(10.0), Some(0.85)).is_err());
    }

    #[test]
    fn snapshot_history_uses_latest_not_sum() {
        let world = FinanceWorld {
            items: vec![item(1, "NS&I", "cash", None, 0, 1)],
            snapshots: vec![
                snap(1, 1, "2025-01-01", 10_000.0),
                snap(2, 1, "2026-01-01", 12_000.0),
                snap(3, 1, "2026-06-01", 12_500.0),
            ],
            ..Default::default()
        };
        let now = compute_net_worth(&world, None);
        assert_eq!(now.net_worth_gbp, 12_500.0);
        let then = compute_net_worth(&world, Some("2026-01-15"));
        assert_eq!(then.net_worth_gbp, 12_000.0);
        assert_eq!(now.counted.len(), 1);
    }

    #[test]
    fn sipp_holdings_rollup_excludes_parent() {
        let world = FinanceWorld {
            items: vec![item(1, "AJ Bell SIPP", "pension", None, 1, 1)],
            snapshots: vec![snap(1, 1, "2026-08-01", 100_000.0)],
            holdings: vec![
                holding(1, 1, "VWRP", 60_000.0, "2026-08-01"),
                holding(2, 1, "Cash", 40_000.0, "2026-08-01"),
            ],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 100_000.0);
        assert_eq!(r.counted.len(), 1);
        assert_eq!(r.counted[0].source, "investment_holdings");
        assert!(r.excluded.iter().any(|e| e.reason.contains("holdings")));
        assert!(r.unreconciled.is_empty());
    }

    #[test]
    fn sipp_holdings_unreconciled_when_gap_large() {
        let world = FinanceWorld {
            items: vec![item(1, "SIPP", "pension", None, 1, 1)],
            snapshots: vec![snap(1, 1, "2026-08-01", 100_000.0)],
            holdings: vec![holding(1, 1, "Fund", 80_000.0, "2026-08-01")],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 80_000.0);
        assert_eq!(r.unreconciled.len(), 1);
    }

    #[test]
    fn one_fund_pension_uses_account_total() {
        let world = FinanceWorld {
            items: vec![item(1, "NEST", "pension", None, 0, 1)],
            snapshots: vec![snap(1, 1, "2026-08-01", 45_000.0)],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 45_000.0);
        assert_eq!(r.counted[0].source, "valuation_snapshot");
    }

    #[test]
    fn fx_and_nsi_account_totals() {
        let world = FinanceWorld {
            items: vec![
                item(1, "Wise EUR", "cash", None, 0, 1),
                item(2, "NS&I bonds", "cash", None, 0, 1),
            ],
            snapshots: vec![
                SnapshotView {
                    id: 1,
                    financial_item_id: 1,
                    valuation_date: "2026-08-01".into(),
                    native_value: 10_000.0,
                    currency: "EUR".into(),
                    gbp_value: 8_500.0,
                    confidence: "estimated".into(),
                    source_type: "portal".into(),
                },
                snap(2, 2, "2026-08-01", 20_000.0),
            ],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert!((r.net_worth_gbp - 28_500.0).abs() < 1e-9);
    }

    #[test]
    fn property_plus_mortgage_equity() {
        let world = FinanceWorld {
            items: vec![
                item(1, "Grangeside", "property", None, 0, 1),
                item(2, "Grangeside mortgage", "mortgage", Some(1), 0, 1),
            ],
            snapshots: vec![snap(1, 1, "2026-08-01", 280_000.0)],
            mortgages: vec![MortgageView {
                financial_item_id: 2,
                property_item_id: Some(1),
                outstanding_balance: 120_000.0,
                repayment_type: "interest_only".into(),
            }],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.assets_gbp, 280_000.0);
        assert_eq!(r.liabilities_gbp, 120_000.0);
        assert_eq!(r.net_worth_gbp, 160_000.0);
        // Linked mortgage is not also counted via a property roll-up.
        assert_eq!(r.counted.len(), 2);
    }

    #[test]
    fn property_mortgage_link_without_parent_still_counts_once_each() {
        let world = FinanceWorld {
            items: vec![
                item(1, "House", "property", None, 0, 1),
                item(2, "Mortgage", "mortgage", None, 0, 1),
            ],
            snapshots: vec![snap(1, 1, "2026-08-01", 280_000.0)],
            mortgages: vec![MortgageView {
                financial_item_id: 2,
                property_item_id: Some(1),
                outstanding_balance: 120_000.0,
                repayment_type: "interest_only".into(),
            }],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.assets_gbp, 280_000.0);
        assert_eq!(r.liabilities_gbp, 120_000.0);
        assert_eq!(r.net_worth_gbp, 160_000.0);
        assert_eq!(r.counted.len(), 2);
        assert_eq!(r.counted.iter().filter(|c| c.role == "asset").count(), 1);
        assert_eq!(r.counted.iter().filter(|c| c.role == "liability").count(), 1);
    }

    #[test]
    fn mortgage_snapshot_not_added_to_balance() {
        let world = FinanceWorld {
            items: vec![item(1, "IO mortgage", "mortgage", None, 0, 1)],
            snapshots: vec![snap(1, 1, "2026-08-01", 119_000.0)],
            mortgages: vec![MortgageView {
                financial_item_id: 1,
                property_item_id: None,
                outstanding_balance: 120_000.0,
                repayment_type: "interest_only".into(),
            }],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.liabilities_gbp, 119_000.0);
        assert_eq!(r.counted[0].source, "valuation_snapshot");
    }

    #[test]
    fn tax_liabilities_separated_and_receivable_included() {
        let world = FinanceWorld {
            items: vec![
                item(1, "VAT Q1", "tax_liability", None, 0, 1),
                item(2, "CT FY25", "tax_liability", None, 0, 1),
                item(3, "SA 2025/26", "tax_liability", None, 0, 1),
                item(4, "Invoice 44", "receivable", None, 0, 1),
            ],
            snapshots: vec![
                snap(1, 1, "2026-08-01", 3_000.0),
                snap(2, 2, "2026-08-01", 8_000.0),
                snap(3, 3, "2026-08-01", 2_000.0),
                snap(4, 4, "2026-08-01", 1_500.0),
            ],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.assets_gbp, 1_500.0);
        assert_eq!(r.liabilities_gbp, 13_000.0);
        assert_eq!(r.net_worth_gbp, -11_500.0);
        assert_eq!(r.by_class.iter().find(|c| c.class == "tax_liability").unwrap().liabilities_gbp, 13_000.0);
        assert_eq!(r.by_class.iter().find(|c| c.class == "receivable").unwrap().assets_gbp, 1_500.0);
    }

    #[test]
    fn forecasts_not_in_world_so_excluded() {
        let world = FinanceWorld {
            items: vec![item(1, "Cash", "cash", None, 0, 1)],
            snapshots: vec![snap(1, 1, "2026-08-01", 100.0)],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert!(r.excluded.iter().any(|e| e.name == "forecasts"));
        assert_eq!(r.net_worth_gbp, 100.0);
    }

    #[test]
    fn parent_child_double_count_prevented() {
        let world = FinanceWorld {
            items: vec![
                item(1, "SIPP", "pension", None, 1, 1),
                item(2, "Fund A", "investment", Some(1), 0, 1),
                item(3, "Fund B", "investment", Some(1), 0, 1),
            ],
            snapshots: vec![
                snap(1, 1, "2026-08-01", 100_000.0),
                snap(2, 2, "2026-08-01", 60_000.0),
                snap(3, 3, "2026-08-01", 40_000.0),
            ],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 100_000.0);
        assert!(r.excluded.iter().any(|e| e.reason.contains("rollup parent")));
    }

    #[test]
    fn inactive_excluded() {
        let world = FinanceWorld {
            items: vec![
                item(1, "Live", "cash", None, 0, 1),
                item(2, "Closed", "cash", None, 0, 0),
            ],
            snapshots: vec![
                snap(1, 1, "2026-08-01", 10.0),
                snap(2, 2, "2026-08-01", 9_999.0),
            ],
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert_eq!(r.net_worth_gbp, 10.0);
        assert!(r.excluded.iter().any(|e| e.reason == "inactive"));
    }

    #[test]
    fn circular_parent_detected() {
        let mut parents = HashMap::new();
        parents.insert(1, Some(2));
        parents.insert(2, Some(3));
        parents.insert(3, Some(1));
        assert!(would_cycle(Some(1), Some(2), &parents));
        let mut tree = HashMap::new();
        tree.insert(1, None);
        tree.insert(2, Some(1));
        assert!(!would_cycle(Some(3), Some(2), &tree));
        assert!(would_cycle(Some(1), Some(1), &tree));
    }

    #[test]
    fn forecast_projection_compounds() {
        let end = project_ending_value(100_000.0, "2026-01-01", "2027-01-01", Some(0.05), None, None)
            .unwrap();
        assert!((end - 105_000.0).abs() < 50.0);
        assert!(project_ending_value(100.0, "2027-01-01", "2026-01-01", None, None, None).is_err());
    }

    #[test]
    fn classify_account_cash_card_and_overdraft() {
        let (c, r, g) = classify_account("account", Some("current"), 300_532);
        assert_eq!((c, r), ("cash", "asset"));
        assert!((g - 3005.32).abs() < 1e-9);
        let (c, r, g) = classify_account("card", Some("credit_card"), 50_000);
        assert_eq!((c, r), ("credit_card", "liability"));
        assert!((g - 500.0).abs() < 1e-9);
        let (c, r, _) = classify_account("card", None, -1_000);
        assert_eq!((c, r), ("credit_card", "asset"));
        let (c, r, _) = classify_account("account", Some("current"), -5_000);
        assert_eq!((c, r), ("cash", "liability"));
        let (c, r, _) = classify_account("account", Some("mortgage"), 12_000_000);
        assert_eq!((c, r), ("mortgage", "liability"));
    }

    #[test]
    fn bank_card_broker_and_valuation_item() {
        let world = FinanceWorld {
            items: vec![item(1, "House", "property", None, 0, 1)],
            snapshots: vec![snap(1, 1, "2026-08-01", 280_000.0)],
            accounts: vec![
                AccountBalanceView {
                    id: 1,
                    name: "Smile current".into(),
                    kind: "account".into(),
                    account_type: Some("current".into()),
                    balance_cents: Some(300_532),
                    currency: "GBP".into(),
                },
                AccountBalanceView {
                    id: 2,
                    name: "Barclaycard".into(),
                    kind: "card".into(),
                    account_type: Some("credit_card".into()),
                    balance_cents: Some(50_000),
                    currency: "GBP".into(),
                },
                AccountBalanceView {
                    id: 3,
                    name: "Unsynced".into(),
                    kind: "account".into(),
                    account_type: Some("savings".into()),
                    balance_cents: None,
                    currency: "GBP".into(),
                },
            ],
            broker_holdings_gbp: 10_000.0,
            ..Default::default()
        };
        let r = compute_net_worth(&world, None);
        assert!((r.assets_gbp - 293_005.32).abs() < 1e-6);
        assert!((r.liabilities_gbp - 500.0).abs() < 1e-6);
        assert!((r.net_worth_gbp - 292_505.32).abs() < 1e-6);
        assert!(r.counted.iter().any(|c| c.source == "bank_or_manual_account" && c.role == "asset"));
        assert!(r.counted.iter().any(|c| c.class == "credit_card" && c.role == "liability"));
        assert!(r.counted.iter().any(|c| c.source == "broker_holdings"));
        assert!(r.counted.iter().all(|c| c.financial_item_id.is_none() || c.financial_item_id == Some(1)));
        assert!(r.by_class.iter().any(|c| c.class == "credit_card" && (c.liabilities_gbp - 500.0).abs() < 1e-9));
        assert!(r.excluded.iter().any(|e| e.name == "Unsynced"));
        let then = compute_net_worth(&world, Some("2026-01-01"));
        assert!((then.assets_gbp - 13_005.32).abs() < 1e-6);
        assert!((then.liabilities_gbp - 500.0).abs() < 1e-6);
    }
}
