//! GET /api/transactions — filter / search / paginate
//! GET /api/transactions/:id
//! GET /api/transactions/review — uncategorised + mixed + transfer-like
//! POST /api/transactions/apply-learned — run learned mapping over uncategorised
//! PATCH /api/transactions/:id — update category, notes, tags (category PATCH is the remember event)

use crate::claims::{self, is_dual};
use crate::learn::{self, ReviewReason};
use crate::models::Transaction;
use crate::pairs;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub account_ids: Option<String>,   // comma-separated
    pub category_ids: Option<String>,  // comma-separated, "null" means uncategorised
    pub from: Option<i64>,             // unix seconds
    pub to: Option<i64>,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    pub is_credit: Option<bool>,
    pub q: Option<String>, // free-text on description + merchant
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub status: Option<String>, // transfers page: "all" | "unpaired"
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let account_ids: Option<Vec<i64>> = q.account_ids.as_deref().map(parse_id_list);
    let category_ids: Option<Vec<i64>> = q.category_ids.as_deref().map(parse_id_list);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);

    let txns = state
        .db
        .list_transactions(
            account_ids.as_deref(),
            category_ids.as_deref(),
            q.from,
            q.to,
            q.min_amount_cents,
            q.max_amount_cents,
            q.is_credit,
            q.q.as_deref(),
            limit,
            offset,
        )
        .await
        .map_err(internal)?;

    let total = state
        .db
        .count_transactions_filtered(
            account_ids.as_deref(),
            category_ids.as_deref(),
            q.from,
            q.to,
        )
        .await
        .unwrap_or(0);

    Ok(Json(json!({
        "transactions": txns.iter().map(enrich_txn).collect::<Vec<_>>(),
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    let tags = state
        .db
        .tags_for_transaction(id)
        .await
        .unwrap_or_default();
    let pair = pair_info(&state.db, &txn).await?;
    Ok(Json(json!({
        "transaction": enrich_txn(&txn),
        "tags": tags,
        "pair": pair,
    })))
}

pub async fn review(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let engine = learn::LearnEngine::from_db(&state.db)
        .await
        .map_err(internal)?;
    let uncat = state
        .db
        .list_uncategorised_transactions()
        .await
        .map_err(internal)?;
    let items = learn::review_items(&engine, uncat);
    let total = items.len() as i64;
    let page: Vec<Value> = items
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|(t, reason, signature, payee)| {
            json!({
                "transaction": t,
                "reason": reason.as_str(),
                "signature": signature,
                "payee": payee,
            })
        })
        .collect();
    Ok(Json(json!({
        "items": page,
        "total": total,
        "limit": limit,
        "offset": offset,
        "reasons": [
            ReviewReason::Transfer.as_str(),
            ReviewReason::Mixed.as_str(),
            ReviewReason::NoHistory.as_str(),
            ReviewReason::Ready.as_str(),
        ],
    })))
}

pub async fn apply_learned(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let stats = learn::apply_uncategorised(&state.db)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "stats": stats })))
}

/// Loopback-only: same as apply-learned, then try to pair unmatched transfer legs.
pub async fn apply_learned_loopback(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !addr.ip().is_loopback() {
        return Err((StatusCode::FORBIDDEN, "loopback only".into()));
    }
    let stats = learn::apply_uncategorised(&state.db)
        .await
        .map_err(internal)?;
    let paired = pairs::autosuggest_unpaired(&state.db)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "stats": stats, "paired": paired })))
}

pub async fn transfers(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let unpaired_only = q
        .status
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("unpaired"))
        .unwrap_or(false);
    let limit = q.limit.unwrap_or(500).clamp(1, 2000);
    let offset = q.offset.unwrap_or(0).max(0);
    if let Err(e) = pairs::autosuggest_unpaired(&state.db).await {
        tracing::warn!("autosuggest unpaired transfers: {e:#}");
    }
    let page = pairs::list_transfers(&state.db, unpaired_only, limit, offset)
        .await
        .map_err(internal)?;
    let accounts = state
        .db
        .list_all_enabled_accounts()
        .await
        .unwrap_or_default();
    let learned = state
        .db
        .list_paired_counterparts()
        .await
        .unwrap_or_default();
    let transactions: Vec<Value> = page
        .transactions
        .iter()
        .map(|t| {
            let mut v = enrich_txn(t);
            if let Some(obj) = v.as_object_mut() {
                if let Some(a) = pairs::preferred_pair_accounts_from(t, &accounts, &learned).first()
                {
                    obj.insert(
                        "preferred_account".to_string(),
                        json!({ "id": a.id, "name": account_name(a) }),
                    );
                }
                if let Some(aid) = t.transfer_await_account_id {
                    if let Some(a) = accounts.iter().find(|x| x.id == aid) {
                        obj.insert(
                            "await_account".to_string(),
                            json!({ "id": a.id, "name": account_name(a) }),
                        );
                    }
                }
            }
            v
        })
        .collect();
    Ok(Json(json!({
        "transactions": transactions,
        "total": page.total,
        "all_total": page.all_total,
        "unpaired_total": page.unpaired_total,
        "status": if unpaired_only { "unpaired" } else { "all" },
        "limit": limit,
        "offset": offset,
    })))
}

#[derive(Debug, Deserialize)]
pub struct PatchBody {
    pub category_id: Option<Option<i64>>, // explicit None to clear, omitted to leave alone
    pub notes: Option<Option<String>>,
    /// Present replaces the flags. An empty array clears them. Omitted leaves them.
    pub claim_flags: Option<Vec<String>>,
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if let Some(cat) = body.category_id {
        state
            .db
            .update_transaction_category(id, cat)
            .await
            .map_err(internal)?;
    }
    if let Some(notes) = body.notes {
        state
            .db
            .update_transaction_notes(id, notes.as_deref())
            .await
            .map_err(internal)?;
    }
    if let Some(flags) = body.claim_flags {
        let stored = claims::normalize_claim_flags(&flags)
            .map_err(|m| (StatusCode::BAD_REQUEST, m))?;
        let updated = state
            .db
            .update_transaction_claim_flags(id, stored.as_deref())
            .await
            .map_err(internal)?;
        if !updated {
            return Err((StatusCode::NOT_FOUND, "transaction not found".into()));
        }
    }
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    let mut remembered = 0i64;
    if body.category_id.flatten().is_some() {
        remembered = learn::remember_after_patch(&state.db, &txn)
            .await
            .map_err(internal)?;
    }
    let pair = pair_info(&state.db, &txn).await?;
    Ok(Json(json!({
        "transaction": enrich_txn(&txn),
        "remembered": remembered,
        "pair": pair,
    })))
}

#[derive(Debug, Deserialize)]
pub struct PairBody {
    pub other_id: Option<i64>,
    pub account_id: Option<i64>,
    pub financial_item_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PairSuggestQuery {
    pub account_id: Option<i64>,
}

pub async fn pair_suggestions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(q): Query<PairSuggestQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    let sugg = if let Some(aid) = q.account_id {
        pairs::suggestions_for_account(&state.db, &txn, aid).await
    } else {
        pairs::suggestions_for(&state.db, &txn).await
    }
    .map_err(internal)?;
    let preferred = if q.account_id.is_none() {
        pairs::preferred_pair_accounts(&state.db, &txn)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(Json(json!({
        "suggestions": sugg.into_iter().map(|s| json!({
            "other": enrich_txn(&s.other),
            "implied_fx_rate": s.implied_fx_rate,
            "leaf": s.leaf,
        })).collect::<Vec<_>>(),
        "preferred_accounts": preferred.into_iter().map(|a| json!({
            "id": a.id,
            "name": a.custom_display_name.clone().filter(|s| !s.is_empty()).unwrap_or(a.display_name),
        })).collect::<Vec<_>>(),
    })))
}

pub async fn pair(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<PairBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    match (body.other_id, body.account_id, body.financial_item_id) {
        (Some(other_id), None, None) => {
            let (a, b, rate) = pairs::pair(&state.db, id, other_id).await.map_err(pair_err)?;
            Ok(Json(paired_json(&a, &b, rate)))
        }
        (None, Some(account_id), None) => {
            let out = pairs::pair_to_account(&state.db, id, account_id)
                .await
                .map_err(pair_err)?;
            Ok(Json(assign_json(&out)))
        }
        (None, None, Some(item_id)) => {
            let (a, b, rate) = pairs::pair_to_item(&state.db, id, item_id)
                .await
                .map_err(pair_err)?;
            Ok(Json(paired_json(&a, &b, rate)))
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            "send other_id, account_id, or financial_item_id".into(),
        )),
    }
}

fn account_name(a: &crate::models::Account) -> String {
    a.custom_display_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| a.display_name.clone())
}

fn paired_json(a: &Transaction, b: &Transaction, rate: Option<f64>) -> Value {
    json!({
        "waiting": false,
        "transaction": enrich_txn(a),
        "other": enrich_txn(b),
        "implied_fx_rate": rate,
    })
}

fn assign_json(out: &pairs::AssignOutcome) -> Value {
    match out {
        pairs::AssignOutcome::Paired {
            a,
            b,
            implied_fx_rate,
        } => paired_json(a, b, *implied_fx_rate),
        pairs::AssignOutcome::Waiting {
            transaction,
            account,
        } => json!({
            "waiting": true,
            "transaction": enrich_txn(transaction),
            "await_account": { "id": account.id, "name": account_name(account) },
        }),
    }
}

pub async fn unpair(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    pairs::unpair(&state.db, id).await.map_err(pair_err)?;
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    Ok(Json(json!({ "transaction": enrich_txn(&txn) })))
}

pub async fn mark_unmatchable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = pairs::mark_unmatchable(&state.db, id)
        .await
        .map_err(pair_err)?;
    Ok(Json(json!({ "transaction": enrich_txn(&txn) })))
}

pub async fn clear_unmatchable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = pairs::clear_unmatchable(&state.db, id)
        .await
        .map_err(pair_err)?;
    Ok(Json(json!({ "transaction": enrich_txn(&txn) })))
}

pub async fn clear_await(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = pairs::clear_await(&state.db, id).await.map_err(pair_err)?;
    Ok(Json(json!({ "transaction": enrich_txn(&txn) })))
}

pub async fn spending_summary(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let from = q.from.unwrap_or_else(|| chrono::Utc::now().timestamp() - 30 * 86_400);
    let to = q.to.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let rows = state
        .db
        .spending_by_category(from, to)
        .await
        .map_err(internal)?;
    Ok(Json(json!({
        "from": from,
        "to": to,
        "by_category": rows.into_iter().map(|(cid, cents)| json!({
            "category_id": cid,
            "amount_cents": cents,
        })).collect::<Vec<_>>(),
    })))
}

fn enrich_txn(t: &Transaction) -> Value {
    let mut v = serde_json::to_value(t).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "payee".to_string(),
            json!(learn::payee_label(t.merchant_name.as_deref(), &t.description)),
        );
        obj.insert("dual".to_string(), json!(is_dual(&t.claim_flags)));
    }
    v
}

async fn pair_info(
    db: &crate::db::Db,
    t: &Transaction,
) -> Result<Value, (StatusCode, String)> {
    let Some(other_id) = t.transfer_pair_id else {
        return Ok(Value::Null);
    };
    let Some(other) = db.get_transaction(other_id).await.map_err(internal)? else {
        return Ok(Value::Null);
    };
    let rate = match pairs::debit_credit(t, &other) {
        Ok((debit, credit)) => pairs::implied_fx_rate(debit, credit),
        Err(_) => None,
    };
    Ok(json!({
        "other": enrich_txn(&other),
        "implied_fx_rate": rate,
    }))
}

fn parse_id_list(s: &str) -> Vec<i64> {
    s.split(',').filter_map(|p| p.trim().parse::<i64>().ok()).collect()
}

fn pair_err(e: anyhow::Error) -> (StatusCode, String) {
    let msg = format!("{e}");
    if msg.contains("not found") {
        (StatusCode::NOT_FOUND, msg)
    } else if msg.contains("not a valid") || msg.contains("cannot pair") || msg.contains("itself")
    {
        (StatusCode::BAD_REQUEST, msg)
    } else {
        internal(e)
    }
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
