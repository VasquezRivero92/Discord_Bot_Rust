//! Rutas migradas de solo lectura. Cada modulo expone un `router()` autocontenido (sin
//! fallback propio) que el llamador (Tarea 17) fusiona con `Router::merge` junto al proxy.

pub mod account;
pub mod admin;
pub mod auth;
pub mod cs2_api;
pub mod diagnostic;
pub mod halloween;
pub mod match_launch;
pub mod matchmaking;
pub mod public_api;
pub mod wallet;
pub mod webhooks;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// Respuesta JSON con el mismo `Content-Type` que `res.json` de Express.
pub(crate) fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        )],
        serde_json::to_vec(body).expect("serde_json::Value siempre serializa"),
    )
        .into_response()
}
