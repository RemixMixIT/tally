//! Director-payment classification view for the late accounts period.
//!
//! GET  /api/director-payments
//! PUT  /api/director-payments  { transaction_id, classification_type|null, notes, evidence_link }

use crate::director::{
    self, accounts_period, classification_type_ok, period_label, DIRECTOR_TARGETS,
};
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let (start, end) = accounts_period();
    let payments = state
        .db
        .list_director_payments(start, end)
        .await
        .map_err(internal)?;
    let outside_period_count = state
        .db
        .count_tide_payments_outside_period(start, end)
        .await
        .map_err(internal)?;
    let targets: Vec<_> = DIRECTOR_TARGETS
        .iter()
        .map(|t| json!({ "name": t.name, "amount_gbp": t.amount_gbp }))
        .collect();
    Ok(Json(json!({
        "period": {
            "from": "2025-01-16",
            "to": "2026-01-31",
            "label": period_label(),
        },
        "targets": targets,
        "payments": payments,
        "outside_period_count": outside_period_count,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SetBody {
    pub transaction_id: i64,
    pub classification_type: Option<String>,
    pub notes: Option<String>,
    pub evidence_link: Option<String>,
}

pub async fn set(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = state
        .db
        .get_transaction(body.transaction_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".into()))?;
    let account = state
        .db
        .get_account(txn.account_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "account not found".into()))?;
    if !director::is_tide_statement_account(
        &account.display_name,
        account.custom_display_name.as_deref(),
    ) || txn.is_credit != 0
        || !director::in_accounts_period(txn.timestamp)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "only a Tide payment inside 16 Jan 2025 – 31 Jan 2026 can be classified".into(),
        ));
    }

    let kind = body
        .classification_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match kind {
        None => {
            state
                .db
                .delete_director_classification(txn.account_id, &txn.provider_txn_id)
                .await
                .map_err(internal)?;
        }
        Some(kind) if classification_type_ok(kind) => {
            let notes = clean_text(body.notes.as_deref(), 2_000)?;
            let evidence = clean_text(body.evidence_link.as_deref(), 2_000)?;
            state
                .db
                .set_director_classification(
                    txn.account_id,
                    &txn.provider_txn_id,
                    kind,
                    txn.amount_cents,
                    evidence.as_deref(),
                    notes.as_deref(),
                )
                .await
                .map_err(internal)?;
        }
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "classification must be dividend, pension, salary, or expense".into(),
            ));
        }
    }
    Ok(Json(json!({ "ok": true })))
}

fn clean_text(value: Option<&str>, max: usize) -> Result<Option<String>, (StatusCode, String)> {
    let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err((StatusCode::BAD_REQUEST, "text is too long".into()));
    }
    Ok(Some(value.to_string()))
}

fn internal(err: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
