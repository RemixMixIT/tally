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

#[derive(Deserialize)]
pub struct AccountPatch {
    pub custom_display_name: Option<String>,
}

/// PATCH /api/accounts/:id — for now only `custom_display_name` is mutable. Pass empty string
/// or null to clear the override and fall back to TrueLayer's `display_name`.
pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<AccountPatch>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let custom = body
        .custom_display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    state
        .db
        .set_account_custom_display_name(id, custom)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CreateManualBody {
    pub name: String,
    /// `current` | `savings` | `credit_card`
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
    if !matches!(account_type, "current" | "savings" | "credit_card") {
        return Err((
            StatusCode::BAD_REQUEST,
            "type must be current, savings, or credit_card".into(),
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
