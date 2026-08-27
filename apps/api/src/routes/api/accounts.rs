use crate::account_types::{self, CANONICAL_ACCOUNT_TYPES};
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let accounts = state
        .db
        .list_all_enabled_accounts()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    Ok(Json(json!({ "accounts": accounts })))
}

/// PATCH /api/accounts/:id
///
/// - `custom_display_name`: user label override. Empty string or null clears it.
/// - `account_type`: one of the canonical types (same dropdown as create). Allowed on
///   linked and manual accounts. Once set, TrueLayer sync will not overwrite it.
/// - `current_balance`: major units (pounds). Only accepted for manual/offline accounts
///   (`consent_id IS NULL`) so TrueLayer snapshots are never overwritten from the UI.
pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if body.get("custom_display_name").is_some() {
        let custom = body
            .get("custom_display_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        state
            .db
            .set_account_custom_display_name(id, custom)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    }

    if body.get("account_type").is_some() {
        let account_type = body
            .get("account_type")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or((StatusCode::BAD_REQUEST, "account_type is required".into()))?;
        if !account_types::is_canonical(account_type) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("type must be one of: {}", CANONICAL_ACCOUNT_TYPES.join(", ")),
            ));
        }
        state
            .db
            .set_account_type(id, account_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    }

    if body.get("current_balance").is_some() {
        let account = state
            .db
            .get_account(id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?
            .ok_or((StatusCode::BAD_REQUEST, "unknown account".into()))?;
        if account.consent_id.is_some() {
            return Err((
                StatusCode::BAD_REQUEST,
                "cannot set balance on a linked Open Banking account".into(),
            ));
        }
        let n = body
            .get("current_balance")
            .and_then(|v| v.as_f64())
            .ok_or((StatusCode::BAD_REQUEST, "current_balance must be a number".into()))?;
        let cents = (n * 100.0).round() as i64;
        state
            .db
            .update_account_balance(id, cents, Some(cents), None)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    }

    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CreateManualBody {
    pub name: String,
    /// One of the canonical account types.
    #[serde(rename = "type")]
    pub account_type: String,
    /// Opening balance in major units (pounds), e.g. 2500.00
    pub opening_balance: Option<f64>,
}

/// POST /api/accounts — create a manual/offline account. No TrueLayer OAuth is triggered.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateManualBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name is required".into()));
    }
    let account_type = body.account_type.trim();
    if !account_types::is_canonical(account_type) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("type must be one of: {}", CANONICAL_ACCOUNT_TYPES.join(", ")),
        ));
    }
    let opening_cents = body.opening_balance.map(|n| (n * 100.0).round() as i64);
    let account = state
        .db
        .create_manual_account(name, account_type, opening_cents)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    Ok(Json(json!({ "account": account })))
}
