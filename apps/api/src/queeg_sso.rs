//! Queeg hub SSO: accept the `queeg_session` JWT cookie (same secret as Queeg).
//! Username in the token must already exist in Tally (case-insensitive). Access
//! grants are enforced by nginx `auth_request`; this only maps the cookie to a
//! Tally user.

use crate::auth::User;
use crate::models::FromLibsqlRow;
use crate::AppState;
use axum::http::HeaderMap;
use jsonwebtoken::{decode, DecodingKey, Validation};
use libsql::params;
use serde::Deserialize;
use std::sync::LazyLock;

static SSO_SECRET: LazyLock<Option<String>> = LazyLock::new(|| {
    std::env::var("QUEEG_SSO_SECRET")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
});

#[derive(Debug, Deserialize)]
struct QueegClaims {
    sub: String,
}

fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(name) {
            return rest.strip_prefix('=');
        }
    }
    None
}

fn username_from_headers(headers: &HeaderMap) -> Option<String> {
    let secret = SSO_SECRET.as_ref()?;
    let header = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    let token = cookie_value(header, "queeg_session")?;
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_aud = false;
    let data = decode::<QueegClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .ok()?;
    let name = data.claims.sub.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub async fn user_from_headers(state: &AppState, headers: &HeaderMap) -> Option<User> {
    let username = username_from_headers(headers)?;
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT * FROM users WHERE lower(username) = lower(?1)",
            params![username],
        )
        .await
        .ok()?;
    let row = rows.next().await.ok()??;
    User::from_row(&row).ok()
}
