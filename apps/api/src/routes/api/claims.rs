//! Pending-receipt claim flags.
//!
//! GET   /api/pending-receipts
//! PATCH /api/pending-receipts/{id}  { claim_flags: ["RemixMix", "Client-X"] }
//!
//! Transaction flags use PATCH /api/transactions/{id} with the same array.
//! Creating a pending receipt is the receipt module (B4–B6), not this route.

use crate::claims::{self, is_dual};
use crate::AppState;
use axum::extract::{Path, State};
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
