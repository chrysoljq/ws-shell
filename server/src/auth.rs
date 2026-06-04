use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ws_shell_common::ApiError;

use crate::state::AppState;

// ── JWT Claims ───────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

pub fn create_token(secret: &str, username: &str) -> String {
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(24))
        .unwrap()
        .timestamp() as usize;
    let claims = Claims {
        sub: username.to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("jwt encode failed")
}

pub fn verify_token(secret: &str, token: &str) -> Option<Claims> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()
    .map(|data| data.claims)
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

// ── Unified API Error Response ───────────────────────────────────

pub struct ApiErrorResponse {
    status: StatusCode,
    body: ApiError,
}

impl ApiErrorResponse {
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            body: ApiError {
                error: "unauthorized".into(),
                message: msg.into(),
            },
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: ApiError {
                error: "not_found".into(),
                message: msg.into(),
            },
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ApiError {
                error: "internal_error".into(),
                message: msg.into(),
            },
        }
    }
}

impl IntoResponse for ApiErrorResponse {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

// ── Auth Extractor ───────────────────────────────────────────────
// Extracts and verifies JWT from:
//   1. Authorization: Bearer <token> header
//   2. ?token=<token> query param (backward compat)

#[allow(dead_code)]
pub struct AuthUser(pub Claims);

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // 1) Try Authorization: Bearer <token>
        if let Some(header) = parts.headers.get("authorization") {
            if let Ok(s) = header.to_str() {
                if let Some(token) = s.strip_prefix("Bearer ") {
                    if let Some(claims) = verify_token(&state.jwt_secret, token.trim()) {
                        return Ok(AuthUser(claims));
                    }
                }
            }
        }

        // 2) Fall back to ?token= or ?t= query param
        if let Some(query) = parts.uri.query() {
            for pair in query.split('&') {
                let mut kv = pair.splitn(2, '=');
                if let (Some(key), Some(val)) = (kv.next(), kv.next()) {
                    if (key == "token" || key == "t")
                        && !val.is_empty()
                    {
                        if let Some(claims) = verify_token(&state.jwt_secret, val) {
                            return Ok(AuthUser(claims));
                        }
                    }
                }
            }
        }

        Err(ApiErrorResponse::unauthorized(
            "invalid or missing authentication token",
        ))
    }
}
