//! POST /api/capital-on-tap/import — authenticated "run now" (mirrors vanquis).
//! POST /internal/capital-on-tap/import — loopback-only, used by the host rclone script.

use crate::AppState;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Debug, Deserialize, Default)]
pub struct ImportQuery {
    #[serde(default)]
    pub _force: bool,
}

async fn run(state: Arc<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let dir = crate::capital_on_tap::inbound_dir();
    let report = crate::capital_on_tap::import_dir(&state.db, &dir).await;
    Ok(Json(json!({ "report": report })))
}

pub async fn import(
    State(state): State<Arc<AppState>>,
    Query(_q): Query<ImportQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    run(state).await
}

pub async fn import_loopback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    Query(_q): Query<ImportQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !addr.ip().is_loopback() {
        return Err((StatusCode::FORBIDDEN, "loopback only".into()));
    }
    run(state).await
}
