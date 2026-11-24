//! Personal rolling-average expenditure summary.

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn summary(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = state
        .db
        .list_spend_source_rows()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    let as_of = Utc::now().date_naive();
    let report = crate::expenditure::summarise(&rows, as_of);
    Ok(Json(json!({ "expenditure": report })))
}
