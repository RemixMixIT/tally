//! REST API for manual financial items, valuations, holdings, property, mortgages,
//! forecasts, and net worth. Authentication is the same `require_auth` layer as the
//! rest of `/api`. Existing `/api/holdings` (broker/Yahoo) is unchanged.

use crate::finance::{self, FinanceError};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn http_err(e: FinanceError) -> (StatusCode, String) {
    match e {
        FinanceError::Invalid(m) => (StatusCode::BAD_REQUEST, m),
        FinanceError::NotFound(m) => (StatusCode::NOT_FOUND, m),
        FinanceError::Conflict(m) => (StatusCode::CONFLICT, m),
    }
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}

fn default_currency() -> String {
    "GBP".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ItemListQuery {
    pub class: Option<String>,
    pub owner_entity: Option<String>,
    pub is_asset: Option<bool>,
    pub is_liability: Option<bool>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateItemBody {
    pub name: String,
    pub class: String,
    pub item_type: String,
    pub owner_entity: String,
    pub provider: Option<String>,
    pub reference: Option<String>,
    pub parent_item_id: Option<i64>,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub is_rollup: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateItemBody {
    pub name: String,
    pub item_type: String,
    pub owner_entity: String,
    pub provider: Option<String>,
    pub reference: Option<String>,
    pub parent_item_id: Option<i64>,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub is_rollup: bool,
    #[serde(default = "default_true")]
    pub is_active: bool,
    pub notes: Option<String>,
}
fn default_true() -> bool {
    true
}

pub async fn list_items(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ItemListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut items = state.db.list_financial_items().await.map_err(internal)?;
    if let Some(c) = q.class.as_deref() {
        items.retain(|i| i.class == c);
    }
    if let Some(o) = q.owner_entity.as_deref() {
        items.retain(|i| i.owner_entity == o);
    }
    if let Some(a) = q.is_asset {
        items.retain(|i| (i.is_asset == 1) == a);
    }
    if let Some(l) = q.is_liability {
        items.retain(|i| (i.is_liability == 1) == l);
    }
    if let Some(a) = q.is_active {
        items.retain(|i| (i.is_active == 1) == a);
    }

    let world = state.db.load_finance_world().await.map_err(internal)?;
    let report = finance::compute_net_worth(&world, None);
    let counted_by: std::collections::HashMap<i64, &finance::CountedLine> = report
        .counted
        .iter()
        .filter_map(|c| c.financial_item_id.map(|id| (id, c)))
        .collect();
    let unrec_by: std::collections::HashMap<i64, &finance::UnreconciledLine> = report
        .unreconciled
        .iter()
        .map(|u| (u.financial_item_id, u))
        .collect();

    let mut out = Vec::new();
    for item in items {
        let latest = state
            .db
            .list_valuations(item.id)
            .await
            .map_err(internal)?
            .into_iter()
            .next();
        let counted = counted_by.get(&item.id);
        out.push(json!({
            "item": item,
            "latest_valuation": latest,
            "counted_gbp": counted.map(|c| c.gbp_value),
            "counted_source": counted.map(|c| c.source.clone()),
            "unreconciled": unrec_by.get(&item.id),
        }));
    }
    Ok(Json(json!({
        "items": out,
        "classes": finance::ITEM_CLASSES,
        "source_types": finance::SOURCE_TYPES,
        "confidence": finance::CONFIDENCE,
    })))
}

pub async fn create_item(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateItemBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let item = state
        .db
        .create_financial_item(
            &b.name,
            &b.class,
            &b.item_type,
            &b.owner_entity,
            b.provider.as_deref(),
            b.reference.as_deref(),
            b.parent_item_id,
            &b.currency,
            b.is_rollup,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "item": item })))
}

pub async fn get_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let item = state
        .db
        .get_financial_item(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "financial item not found".into()))?;
    let valuations = state.db.list_valuations(id).await.map_err(internal)?;
    let holdings = state.db.list_item_holdings(id).await.map_err(internal)?;
    let property = state.db.get_property(id).await.map_err(internal)?;
    let mortgage = state.db.get_mortgage(id).await.map_err(internal)?;
    let tax = state.db.get_tax_details(id).await.map_err(internal)?;
    let receivable = state.db.get_receivable(id).await.map_err(internal)?;
    let children: Vec<_> = state
        .db
        .list_financial_items()
        .await
        .map_err(internal)?
        .into_iter()
        .filter(|i| i.parent_item_id == Some(id))
        .collect();
    let parent = match item.parent_item_id {
        Some(pid) => state.db.get_financial_item(pid).await.map_err(internal)?,
        None => None,
    };
    let holdings_gbp: f64 = holdings.iter().map(|h| h.gbp_value).sum();
    let parent_gbp = valuations.first().map(|v| v.gbp_value);
    let unreconciled = parent_gbp.and_then(|p| {
        if holdings.is_empty() {
            None
        } else if (p - holdings_gbp).abs()
            > finance::RECONCILE_FLOOR_GBP.max(p.abs() * finance::RECONCILE_PCT)
        {
            Some(json!({
                "parent_gbp": p,
                "holdings_gbp": holdings_gbp,
                "difference_gbp": p - holdings_gbp,
            }))
        } else {
            None
        }
    });
    let linked_mortgage = if item.class == "property" {
        let all = state.db.list_all_mortgages().await.map_err(internal)?;
        let mut found = None;
        for m in all {
            if m.property_item_id == Some(id) {
                found = Some(m);
                break;
            }
        }
        found
    } else {
        None
    };
    Ok(Json(json!({
        "item": item,
        "valuations": valuations,
        "holdings": holdings,
        "holdings_total_gbp": holdings_gbp,
        "unreconciled": unreconciled,
        "property": property,
        "mortgage": mortgage,
        "linked_mortgage": linked_mortgage,
        "tax": tax,
        "receivable": receivable,
        "children": children,
        "parent": parent,
        "suggested_item_types": finance::suggested_item_types(&item.class),
    })))
}

pub async fn update_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<UpdateItemBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let item = state
        .db
        .update_financial_item(
            id,
            &b.name,
            &b.item_type,
            &b.owner_entity,
            b.provider.as_deref(),
            b.reference.as_deref(),
            b.parent_item_id,
            &b.currency,
            b.is_rollup,
            b.is_active,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "item": item })))
}

pub async fn delete_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let result = state
        .db
        .delete_or_deactivate_financial_item(id)
        .await
        .map_err(http_err)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ValuationBody {
    pub valuation_date: String,
    pub native_value: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub gbp_value: Option<f64>,
    pub fx_rate: Option<f64>,
    pub fx_rate_date: Option<String>,
    pub source_type: String,
    pub source_reference: Option<String>,
    pub confidence: String,
    pub contributions: Option<f64>,
    pub withdrawals: Option<f64>,
    pub notes: Option<String>,
}

pub async fn list_valuations(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if state
        .db
        .get_financial_item(id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err((StatusCode::NOT_FOUND, "financial item not found".into()));
    }
    let valuations = state.db.list_valuations(id).await.map_err(internal)?;
    Ok(Json(json!({ "valuations": valuations })))
}

pub async fn create_valuation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<ValuationBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let v = state
        .db
        .create_valuation(
            id,
            &b.valuation_date,
            b.native_value,
            &b.currency,
            b.gbp_value,
            b.fx_rate,
            b.fx_rate_date.as_deref(),
            &b.source_type,
            b.source_reference.as_deref(),
            &b.confidence,
            b.contributions,
            b.withdrawals,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "valuation": v })))
}

pub async fn update_valuation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<ValuationBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let v = state
        .db
        .update_valuation(
            id,
            &b.valuation_date,
            b.native_value,
            &b.currency,
            b.gbp_value,
            b.fx_rate,
            b.fx_rate_date.as_deref(),
            &b.source_type,
            b.source_reference.as_deref(),
            &b.confidence,
            b.contributions,
            b.withdrawals,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "valuation": v })))
}

#[derive(Debug, Deserialize)]
pub struct HoldingBody {
    pub holding_name: String,
    pub ticker_or_identifier: Option<String>,
    pub units: Option<f64>,
    pub price: Option<f64>,
    pub price_currency: Option<String>,
    pub cost_basis: Option<f64>,
    pub target_allocation: Option<f64>,
    pub target_return: Option<f64>,
    pub valuation_date: String,
    pub native_value: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub gbp_value: Option<f64>,
    pub source_reference: Option<String>,
    pub notes: Option<String>,
}

pub async fn list_item_holdings(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if state
        .db
        .get_financial_item(id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err((StatusCode::NOT_FOUND, "financial item not found".into()));
    }
    let holdings = state.db.list_item_holdings(id).await.map_err(internal)?;
    let total: f64 = holdings.iter().map(|h| h.gbp_value).sum();
    Ok(Json(json!({ "holdings": holdings, "total_gbp": total })))
}

pub async fn create_item_holding(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<HoldingBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let h = state
        .db
        .create_item_holding(
            id,
            &b.holding_name,
            b.ticker_or_identifier.as_deref(),
            b.units,
            b.price,
            b.price_currency.as_deref(),
            b.cost_basis,
            b.target_allocation,
            b.target_return,
            &b.valuation_date,
            b.native_value,
            &b.currency,
            b.gbp_value,
            b.source_reference.as_deref(),
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "holding": h })))
}

pub async fn update_item_holding(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<HoldingBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let h = state
        .db
        .update_item_holding(
            id,
            &b.holding_name,
            b.ticker_or_identifier.as_deref(),
            b.units,
            b.price,
            b.price_currency.as_deref(),
            b.cost_basis,
            b.target_allocation,
            b.target_return,
            &b.valuation_date,
            b.native_value,
            &b.currency,
            b.gbp_value,
            b.source_reference.as_deref(),
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "holding": h })))
}

pub async fn delete_item_holding(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_item_holding(id).await.map_err(http_err)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn get_property(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let item = state
        .db
        .get_financial_item(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "financial item not found".into()))?;
    if item.class != "property" {
        return Err((StatusCode::BAD_REQUEST, "item is not a property".into()));
    }
    let details = state.db.get_property(id).await.map_err(internal)?;
    let latest = state
        .db
        .list_valuations(id)
        .await
        .map_err(internal)?
        .into_iter()
        .next();
    let mortgages = state.db.list_all_mortgages().await.map_err(internal)?;
    let linked: Vec<_> = mortgages
        .into_iter()
        .filter(|m| m.property_item_id == Some(id))
        .collect();
    let mortgage_gbp: f64 = linked.iter().map(|m| m.outstanding_balance).sum();
    let property_gbp = latest.as_ref().map(|v| v.gbp_value).unwrap_or(0.0);
    Ok(Json(json!({
        "item": item,
        "property": details,
        "latest_valuation": latest,
        "linked_mortgages": linked,
        "equity_gbp": property_gbp - mortgage_gbp,
    })))
}

#[derive(Debug, Deserialize)]
pub struct PropertyBody {
    pub region: String,
    pub postcode_area: Option<String>,
    pub valuation_method: String,
    pub valuation_url: Option<String>,
    pub valuation_cadence: String,
    pub comparable_sales_notes: Option<String>,
    pub valuation_confidence: String,
    pub last_reviewed: Option<String>,
}

pub async fn put_property(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<PropertyBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let details = state
        .db
        .upsert_property(
            id,
            &b.region,
            b.postcode_area.as_deref(),
            &b.valuation_method,
            b.valuation_url.as_deref(),
            &b.valuation_cadence,
            b.comparable_sales_notes.as_deref(),
            &b.valuation_confidence,
            b.last_reviewed.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "property": details })))
}

pub async fn get_mortgage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let item = state
        .db
        .get_financial_item(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "financial item not found".into()))?;
    if item.class != "mortgage" {
        return Err((StatusCode::BAD_REQUEST, "item is not a mortgage".into()));
    }
    let details = state.db.get_mortgage(id).await.map_err(internal)?;
    let latest = state
        .db
        .list_valuations(id)
        .await
        .map_err(internal)?
        .into_iter()
        .next();
    let property = match details.as_ref().and_then(|d| d.property_item_id) {
        Some(pid) => state.db.get_financial_item(pid).await.map_err(internal)?,
        None => None,
    };
    let property_val = if let Some(p) = &property {
        state
            .db
            .list_valuations(p.id)
            .await
            .map_err(internal)?
            .into_iter()
            .next()
    } else {
        None
    };
    let balance = latest
        .as_ref()
        .map(|v| v.gbp_value)
        .or_else(|| details.as_ref().map(|d| d.outstanding_balance))
        .unwrap_or(0.0);
    let equity_impact = property_val.as_ref().map(|v| v.gbp_value - balance);
    Ok(Json(json!({
        "item": item,
        "mortgage": details,
        "latest_valuation": latest,
        "property": property,
        "property_valuation": property_val,
        "counted_balance_gbp": balance,
        "equity_impact_gbp": equity_impact,
    })))
}

#[derive(Debug, Deserialize)]
pub struct MortgageBody {
    pub property_item_id: Option<i64>,
    pub repayment_type: String,
    pub outstanding_balance: f64,
    pub interest_rate: Option<f64>,
    pub rate_type: Option<String>,
    pub fixed_period_expiry: Option<String>,
    pub payment_amount: Option<f64>,
    pub payment_frequency: Option<String>,
    pub term_end_date: Option<String>,
    pub overpayment_total: Option<f64>,
    pub projected_balance: Option<f64>,
    pub notes: Option<String>,
}

pub async fn put_mortgage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<MortgageBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let details = state
        .db
        .upsert_mortgage(
            id,
            b.property_item_id,
            &b.repayment_type,
            b.outstanding_balance,
            b.interest_rate,
            b.rate_type.as_deref(),
            b.fixed_period_expiry.as_deref(),
            b.payment_amount,
            b.payment_frequency.as_deref(),
            b.term_end_date.as_deref(),
            b.overpayment_total,
            b.projected_balance,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "mortgage": details })))
}

#[derive(Debug, Deserialize)]
pub struct ReceivableBody {
    pub debtor: String,
    pub invoice_reference: String,
    pub due_date: String,
    pub expected_amount: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub gbp_value: Option<f64>,
    pub status: String,
}

pub async fn put_receivable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<ReceivableBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let details = state
        .db
        .upsert_receivable(
            id,
            &b.debtor,
            &b.invoice_reference,
            &b.due_date,
            b.expected_amount,
            &b.currency,
            b.gbp_value,
            &b.status,
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "receivable": details })))
}

#[derive(Debug, Deserialize)]
pub struct ForecastBody {
    pub name: String,
    pub scenario_type: String,
    pub financial_item_id: Option<i64>,
    pub forecast_start: String,
    pub forecast_end: String,
    pub starting_gbp_value: f64,
    pub target_return: Option<f64>,
    pub property_growth_rate: Option<f64>,
    pub mortgage_rate: Option<f64>,
    pub repayment_assumption: Option<String>,
    pub fx_assumption: Option<f64>,
    pub contributions: Option<f64>,
    pub withdrawals: Option<f64>,
    pub ending_projected_value: Option<f64>,
    #[serde(default)]
    pub assumptions: String,
    pub notes: Option<String>,
}

pub async fn list_forecasts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let forecasts = state.db.list_forecasts().await.map_err(internal)?;
    Ok(Json(json!({
        "forecasts": forecasts,
        "note": "Projected values are not actual net worth.",
    })))
}

pub async fn create_forecast(
    State(state): State<Arc<AppState>>,
    Json(b): Json<ForecastBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let f = state
        .db
        .create_forecast(
            &b.name,
            &b.scenario_type,
            b.financial_item_id,
            &b.forecast_start,
            &b.forecast_end,
            b.starting_gbp_value,
            b.target_return,
            b.property_growth_rate,
            b.mortgage_rate,
            b.repayment_assumption.as_deref(),
            b.fx_assumption,
            b.contributions,
            b.withdrawals,
            b.ending_projected_value,
            &b.assumptions,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "forecast": f })))
}

pub async fn update_forecast(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<ForecastBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let f = state
        .db
        .update_forecast(
            id,
            &b.name,
            &b.scenario_type,
            b.financial_item_id,
            &b.forecast_start,
            &b.forecast_end,
            b.starting_gbp_value,
            b.target_return,
            b.property_growth_rate,
            b.mortgage_rate,
            b.repayment_assumption.as_deref(),
            b.fx_assumption,
            b.contributions,
            b.withdrawals,
            b.ending_projected_value,
            &b.assumptions,
            b.notes.as_deref(),
        )
        .await
        .map_err(http_err)?;
    Ok(Json(json!({ "forecast": f })))
}

pub async fn delete_forecast(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_forecast(id).await.map_err(http_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct NetWorthQuery {
    pub as_of: Option<String>,
}

pub async fn net_worth(
    State(state): State<Arc<AppState>>,
    Query(q): Query<NetWorthQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if let Some(d) = q.as_of.as_deref() {
        finance::validate_iso_date("as_of", d).map_err(http_err)?;
    }
    let world = state.db.load_finance_world().await.map_err(internal)?;
    let report = finance::compute_net_worth(&world, q.as_of.as_deref());
    Ok(Json(json!({ "net_worth": report })))
}

pub async fn net_worth_explanation(
    State(state): State<Arc<AppState>>,
    Query(q): Query<NetWorthQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if let Some(d) = q.as_of.as_deref() {
        finance::validate_iso_date("as_of", d).map_err(http_err)?;
    }
    let world = state.db.load_finance_world().await.map_err(internal)?;
    let report = finance::compute_net_worth(&world, q.as_of.as_deref());
    Ok(Json(json!({
        "net_worth": report,
        "rules": [
            "Actual net worth is assets minus liabilities.",
            "Latest valid snapshot is the current value; history is never overwritten.",
            "If a parent has holdings it is a roll-up: holdings are counted, not the parent total.",
            "If no holdings exist, the parent account total is counted.",
            "Property is an asset; mortgage is a liability counted once (snapshot or outstanding balance, not both).",
            "Receivables are assets; tax liabilities are liabilities.",
            "Enabled bank and manual account balances are included (cards, mortgages, and tax-type accounts as liabilities).",
            "Broker/Yahoo holdings are included at latest market value.",
            "Do not also enter the same bank cash, card, or broker position as a valuation item.",
            "Bank and broker figures are the latest available balances, not reconstructed historically.",
            "Inactive items, forecasts, and transactions are excluded.",
            "Duplicate snapshots are not summed — only the latest as-of date counts.",
        ],
    })))
}

pub async fn net_worth_history(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let world = state.db.load_finance_world().await.map_err(internal)?;
    let mut dates: Vec<String> = world
        .snapshots
        .iter()
        .map(|s| s.valuation_date.clone())
        .collect();
    dates.extend(world.holdings.iter().map(|h| h.valuation_date.clone()));
    dates.sort();
    dates.dedup();
    let points: Vec<Value> = dates
        .iter()
        .map(|d| {
            let r = finance::compute_net_worth(&world, Some(d.as_str()));
            json!({
                "date": d,
                "assets_gbp": r.assets_gbp,
                "liabilities_gbp": r.liabilities_gbp,
                "net_worth_gbp": r.net_worth_gbp,
            })
        })
        .collect();
    Ok(Json(json!({ "points": points })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_err_maps_status() {
        let (s, m) = http_err(FinanceError::invalid("bad date"));
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(m, "bad date");
        let (s, _) = http_err(FinanceError::not_found("nope"));
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = http_err(FinanceError::conflict("busy"));
        assert_eq!(s, StatusCode::CONFLICT);
    }
}
