//! Pending-receipt claim flags.
//!
//! GET   /api/pending-receipts
//! PATCH /api/pending-receipts/{id}  { claim_flags: ["RemixMix", "Client-X"] }
//!
//! Transaction flags use PATCH /api/transactions/{id} with the same array.
//! Creating a pending receipt is the receipt module (B4–B6), not this route.

use crate::claims::{self, is_dual};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = state.db.list_pending_receipts().await.map_err(internal)?;
    let pending_receipts: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "photo_id": r.photo_id,
                "extracted_amount": r.extracted_amount,
                "extracted_date": r.extracted_date,
                "status": r.status,
                "linked_transaction_id": r.linked_transaction_id,
                "claim_flags": r.claim_flags,
                "dual": is_dual(&r.claim_flags),
                "created_at": r.created_at,
                "updated_at": r.updated_at,
            })
        })
        .collect();
    Ok(Json(json!({ "pending_receipts": pending_receipts })))
}

#[derive(Debug, Deserialize)]
pub struct PatchBody {
    pub claim_flags: Vec<String>,
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let stored = claims::normalize_claim_flags(&body.claim_flags)
        .map_err(|m| (StatusCode::BAD_REQUEST, m))?;
    let updated = state
        .db
        .update_pending_receipt_claim_flags(id, stored.as_deref())
        .await
        .map_err(internal)?;
    if !updated {
        return Err((StatusCode::NOT_FOUND, "pending receipt not found".into()));
    }
    let row = state
        .db
        .get_pending_receipt(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "pending receipt not found".into()))?;
    Ok(Json(json!({
        "pending_receipt": {
            "id": row.id,
            "photo_id": row.photo_id,
            "extracted_amount": row.extracted_amount,
            "extracted_date": row.extracted_date,
            "status": row.status,
            "linked_transaction_id": row.linked_transaction_id,
            "claim_flags": row.claim_flags,
            "dual": is_dual(&row.claim_flags),
            "created_at": row.created_at,
            "updated_at": row.updated_at,
        }
    })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}

fn pack_json(pack: &crate::claim_packs::ClaimPack) -> Value {
    json!({
        "id": pack.id,
        "party": pack.party,
        "created_at": pack.created_at,
        "total_cents": pack.total_cents,
        "total": pack.total,
        "lines": pack.lines,
    })
}

pub async fn list_packs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let packs = crate::claim_packs::list_saved_packs(&state.db)
        .await
        .map_err(internal)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut source_total_cents = 0;
    for pack in &packs {
        for line in &pack.lines {
            if seen.insert((line.source_kind.clone(), line.source_id)) {
                source_total_cents += line.amount_cents;
            }
        }
    }
    Ok(Json(json!({
        "packs": packs.iter().map(pack_json).collect::<Vec<_>>(),
        "source_total_cents": source_total_cents,
        "source_total": crate::claim_packs::format_gbp(source_total_cents),
    })))
}

pub async fn generate(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let created_at = chrono::Utc::now().timestamp();
    let set = crate::claim_packs::generate(&state.db, created_at)
        .await
        .map_err(internal)?;
    Ok(Json(json!({
        "packs": set.packs.iter().map(pack_json).collect::<Vec<_>>(),
        "source_total_cents": set.source_total_cents(),
        "source_total": crate::claim_packs::format_gbp(set.source_total_cents()),
    })))
}

#[derive(Debug, Deserialize)]
pub struct ShowQuery {
    pub format: Option<String>,
}

pub async fn show_pack(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<ShowQuery>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    use axum::response::IntoResponse;
    let packs = crate::claim_packs::list_saved_packs(&state.db)
        .await
        .map_err(internal)?;
    let pack = packs
        .into_iter()
        .find(|pack| pack.id == id)
        .ok_or((StatusCode::NOT_FOUND, "claim pack not found".into()))?;
    let format = query.format.as_deref().unwrap_or("json");
    let response = match format {
        "csv" => (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/csv; charset=utf-8".to_string(),
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{id}.csv\""),
                ),
            ],
            crate::claim_packs::render_csv(&pack),
        )
            .into_response(),
        "md" => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/markdown; charset=utf-8".to_string(),
            )],
            crate::claim_packs::render_markdown(&pack),
        )
            .into_response(),
        _ => Json(pack_json(&pack)).into_response(),
    };
    Ok(response)
}
