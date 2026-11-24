use crate::bill_match::{self, FundingLine, MatchDecision, VariableAmount};
use crate::models::{Account, Bill};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let bills = state.db.list_bills().await.map_err(internal)?;
    Ok(Json(json!({ "bills": bills })))
}

#[derive(Debug, Deserialize)]
pub struct UpcomingQuery {
    pub within_days: Option<i64>,
}

pub async fn upcoming(
    State(state): State<Arc<AppState>>,
    Query(q): Query<UpcomingQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let days = q.within_days.unwrap_or(30).clamp(1, 365);
    let bills = state
        .db
        .list_bills_due_within(days)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "bills": bills, "within_days": days })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub expected_amount_min_cents: i64,
    pub expected_amount_max_cents: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub repeat_freq: String,
    pub next_expected_date: Option<i64>,
    pub match_description_regex: Option<String>,
    pub account_id: Option<i64>,
    #[serde(default)]
    pub is_inflow: bool,
}
fn default_currency() -> String {
    "GBP".to_string()
}

fn default_regex(name: &str, explicit: Option<&str>) -> Option<String> {
    let trimmed = explicit.map(str::trim).filter(|s| !s.is_empty());
    if trimmed.is_some() {
        return trimmed.map(|s| s.to_string());
    }
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(regex::escape(name))
    }
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = b.name.trim();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name is required".into()));
    }
    if b.account_id.is_none() {
        return Err((StatusCode::BAD_REQUEST, "account_id is required".into()));
    }
    if b.next_expected_date.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "next_expected_date is required".into(),
        ));
    }
    let regex = default_regex(name, b.match_description_regex.as_deref());
    let id = state
        .db
        .create_bill(
            name,
            b.expected_amount_min_cents,
            b.expected_amount_max_cents,
            &b.currency,
            &b.repeat_freq,
            b.next_expected_date,
            regex.as_deref(),
            None,
            b.account_id,
            b.is_inflow,
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub name: Option<String>,
    pub expected_amount_min_cents: Option<i64>,
    pub expected_amount_max_cents: Option<i64>,
    pub next_expected_date: Option<i64>,
    pub match_description_regex: Option<String>,
    pub enabled: Option<bool>,
    pub account_id: Option<i64>,
    pub is_inflow: Option<bool>,
    pub repeat_freq: Option<String>,
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<UpdateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .db
        .update_bill(
            id,
            b.name.as_deref(),
            b.expected_amount_min_cents,
            b.expected_amount_max_cents,
            b.next_expected_date,
            b.match_description_regex.as_deref(),
            b.enabled,
            b.account_id,
            b.is_inflow,
            b.repeat_freq.as_deref(),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_bill(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn amount_exceptions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let bills = state.db.list_bills().await.map_err(internal)?;
    let matchers = bill_match::compile(&bills);
    let mut exceptions = Vec::new();
    for b in &bills {
        let Some(compiled) = bill_match::CompiledBill::from_bill(b) else {
            continue;
        };
        let from = compiled.next_expected_date - bill_match::DATE_WINDOW_SECS;
        let to = compiled.next_expected_date + bill_match::DATE_WINDOW_SECS;
        let txns = state
            .db
            .list_transactions(
                Some(&[compiled.account_id]),
                None,
                Some(from),
                Some(to),
                None,
                None,
                None,
                None,
                500,
                0,
            )
            .await
            .map_err(internal)?;
        for t in txns {
            if state
                .db
                .bill_payment_exists(b.id, t.id)
                .await
                .map_err(internal)?
            {
                continue;
            }
            match bill_match::decide(
                &matchers,
                t.account_id,
                &t.description,
                t.merchant_name.as_deref(),
                t.amount_cents,
                t.is_credit != 0,
                t.timestamp,
            ) {
                MatchDecision::Exception { bill_id } if bill_id == b.id => {
                    exceptions.push(json!({
                        "bill": b,
                        "transaction": t,
                    }));
                }
                _ => {}
            }
        }
    }
    Ok(Json(json!({ "exceptions": exceptions })))
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub bill_id: i64,
    pub transaction_id: i64,
    /// `this_payment_only` or `update_regular`
    pub action: String,
}

pub async fn confirm_match(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConfirmBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let action = body.action.trim();
    let update_amount = match action {
        "this_payment_only" => false,
        "update_regular" => true,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "action must be this_payment_only or update_regular".into(),
            ))
        }
    };
    let bill = state
        .db
        .get_bill(body.bill_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "unknown bill".into()))?;
    let txn = state
        .db
        .get_transaction(body.transaction_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "unknown transaction".into()))?;
    if let Some(aid) = bill.account_id {
        if aid != txn.account_id {
            return Err((
                StatusCode::BAD_REQUEST,
                "transaction is not on this regular's account".into(),
            ));
        }
    }
    if state
        .db
        .bill_payment_exists(bill.id, txn.id)
        .await
        .map_err(internal)?
    {
        return Err((StatusCode::BAD_REQUEST, "already linked".into()));
    }
    state
        .db
        .mark_bill_paid_by_transaction(
            bill.id,
            txn.id,
            txn.timestamp,
            update_amount,
            txn.amount_cents,
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct FundingQuery {
    pub account_id: i64,
}

pub async fn funding(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FundingQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let account = state
        .db
        .get_account(q.account_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::BAD_REQUEST, "unknown account".into()))?;
    let now = chrono::Utc::now().timestamp();
    let this_end = bill_match::calendar_month_end(now);
    let (ny, nm) = bill_match::next_calendar_month(now);
    let next_start = bill_match::month_start(ny, nm);
    let next_end = bill_match::month_end(ny, nm);
    let next_label = bill_match::month_label(ny, nm);
    let bills = state.db.list_bills().await.map_err(internal)?;
    let cards = state
        .db
        .list_all_enabled_accounts()
        .await
        .map_err(internal)?;

    let mut this_month = Vec::new();
    let mut next_month = Vec::new();
    let mut warnings = Vec::new();

    for b in bills {
        if b.enabled == 0 || b.account_id != Some(q.account_id) {
            continue;
        }
        let Some(expected) = b.next_expected_date else {
            continue;
        };
        let (amount, estimated) = match bill_match::typical_amount_cents(
            b.expected_amount_min_cents,
            b.expected_amount_max_cents,
        ) {
            Some(c) => (c, false),
            None => match variable_amount(&state.db, &b, &cards).await? {
                VariableAmount::Statement(c) => (c, false),
                VariableAmount::LastPayment(c) => (c, true),
                VariableAmount::Unknown => {
                    warnings.push(format!(
                        "{} omitted: amount unknown and no last payment",
                        b.name
                    ));
                    continue;
                }
            },
        };
        if estimated {
            warnings.push(format!(
                "{}: using last payment £{:.2} — set the expected amount when you know it (e.g. after the statement)",
                b.name,
                amount as f64 / 100.0
            ));
        }
        for ts in bill_match::remaining_this_month(expected, &b.repeat_freq, now) {
            this_month.push(FundingLine {
                bill_id: b.id,
                name: b.name.clone(),
                expected_date: ts,
                expected_amount_cents: amount,
                is_inflow: b.is_inflow != 0,
            });
        }
        for ts in bill_match::occurrences_in_range(expected, &b.repeat_freq, next_start, next_end) {
            next_month.push(FundingLine {
                bill_id: b.id,
                name: b.name.clone(),
                expected_date: ts,
                expected_amount_cents: amount,
                is_inflow: b.is_inflow != 0,
            });
        }
    }
    this_month.sort_by_key(|l| l.expected_date);
    next_month.sort_by_key(|l| l.expected_date);

    let balance = account.current_balance_cents.unwrap_or(0);
    let slack = account.funding_slack_cents;
    let mut combined = this_month.clone();
    combined.extend(next_month.iter().cloned());
    let result = bill_match::funding_forecast(balance, slack, combined);
    let this_net = bill_match::funding_forecast(0, 0, this_month.clone()).remaining_net_out_cents;
    let next_net = bill_match::funding_forecast(0, 0, next_month.clone()).remaining_net_out_cents;

    fn lines_json(lines: &[FundingLine]) -> Vec<Value> {
        lines
            .iter()
            .map(|l| {
                json!({
                    "bill_id": l.bill_id,
                    "name": l.name,
                    "expected_date": l.expected_date,
                    "expected_amount_cents": l.expected_amount_cents,
                    "is_inflow": l.is_inflow,
                })
            })
            .collect()
    }

    Ok(Json(json!({
        "account_id": account.id,
        "account_name": account.custom_display_name.clone().unwrap_or(account.display_name.clone()),
        "current_balance_cents": balance,
        "slack_cents": slack,
        "remaining_net_out_cents": result.remaining_net_out_cents,
        "this_month_net_cents": this_net,
        "next_month_net_cents": next_net,
        "transfer_in_cents": result.transfer_in_cents,
        "month_end": this_end,
        "next_month_label": next_label,
        "next_month_start": next_start,
        "next_month_end": next_end,
        "this_month_lines": lines_json(&this_month),
        "next_month_lines": lines_json(&next_month),
        "lines": lines_json(&next_month),
        "warnings": warnings,
    })))
}

fn matching_card<'a>(cards: &'a [Account], b: &Bill) -> Option<&'a Account> {
    cards
        .iter()
        .filter(|a| {
            a.kind == "card"
                && bill_match::card_matches_bill(
                    a.custom_display_name.as_deref().unwrap_or(&a.display_name),
                    &b.name,
                    b.match_description_regex.as_deref(),
                )
        })
        .max_by_key(|a| {
            a.custom_display_name
                .as_deref()
                .unwrap_or(&a.display_name)
                .len()
        })
}

async fn variable_amount(
    db: &crate::db::Db,
    b: &Bill,
    cards: &[Account],
) -> Result<VariableAmount, (StatusCode, String)> {
    let linked = db.last_bill_payment(b.id).await.map_err(internal)?;
    let from_history = match (
        linked,
        b.account_id,
        b.match_description_regex.as_deref(),
    ) {
        (Some(p), _, _) => Some(p),
        (None, Some(aid), Some(pat)) if !pat.is_empty() => db
            .last_matching_txn(aid, b.is_inflow != 0, pat)
            .await
            .map_err(internal)?,
        _ => None,
    };
    let last_payment_cents = from_history.map(|(cents, _)| cents);
    let last_payment_ts = from_history.map(|(_, ts)| ts).or(b.last_paid_date);
    let card = matching_card(cards, b);
    Ok(bill_match::resolve_variable_amount(
        card.and_then(|c| c.last_statement_balance_cents),
        card.and_then(|c| c.last_statement_date.as_deref()),
        last_payment_cents,
        last_payment_ts,
    ))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
