//! Diagnostico sin autenticacion: `GET /api/ping` (`index.js:399-408`).
//!
//! `GET /health` (`index.js:280-289`) NO se migra: su cuerpo incluye `discord` (derivado de
//! `client.readyAt`, el estado del gateway del bot) y Rust no puede reportarlo; sigue en el proxy.

use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{SecondsFormat, Utc};
use serde_json::json;

use super::json_response;

/// Rutas de diagnostico (`/api/ping`, con o sin barra final como Express).
pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/ping", get(ping))
        .route("/api/ping/", get(ping))
        .route("/api/halloween/status", get(halloween_status))
        .route("/api/halloween/toggle", axum::routing::post(halloween_toggle))
        .route("/api/halloween/reset", axum::routing::post(halloween_reset))
        .route("/api/halloween/stream", get(halloween_status))
        .route("/api/matchmaking/status", get(matchmaking_status))
        .route("/api/matchmaking/launch", axum::routing::post(matchmaking_launch))
        .route("/api/checkout/purchase", axum::routing::post(checkout_purchase))
        .route("/api/reclamaciones", axum::routing::post(create_reclamacion))
}

async fn ping(headers: HeaderMap) -> Response {
    let cookies = parse_cookies(&headers);
    let has_admin_token = cookies
        .iter()
        .any(|(name, value)| name == "admin_token" && !value.is_empty());
    // `trust proxy 1` en Express: protocolo = primera entrada de X-Forwarded-Proto
    // (el mismo criterio que la redirección HTTPS).
    let protocol = crate::middleware::forwarded_proto(&headers);
    let names: Vec<&str> = cookies.iter().map(|(n, _)| n.as_str()).collect();
    json_response(
        axum::http::StatusCode::OK,
        &json!({
            "ok": true,
            "cookies": js_key_order(names),
            "hasAdminToken": has_admin_token,
            "secure": protocol == "https",
            "protocol": protocol,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        }),
    )
}

/// Parseo como `cookie-parser` (`cookie.parse`): pares `nombre=valor` separados por `;`, sin
/// `=` se ignoran, gana la primera aparicion de cada nombre, un valor entre comillas pierde las
/// comillas. Varias cabeceras `Cookie` se unen con `; ` (como Node). Devuelve `(nombre, valor)`
/// en orden de primera aparicion; los valores solo se usan para saber si estan vacios.
fn parse_cookies(headers: &HeaderMap) -> Vec<(String, String)> {
    let joined = headers
        .get_all(header::COOKIE)
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .collect::<Vec<_>>()
        .join("; ");
    let mut out: Vec<(String, String)> = Vec::new();
    for pair in joined.split(';') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if out.iter().any(|(n, _)| n == name) {
            continue;
        }
        let mut value = value.trim();
        if value.starts_with('"') {
            // `val.slice(1, -1)`
            value = value
                .get(1..value.len().saturating_sub(1).max(1))
                .unwrap_or("");
        }
        out.push((name.to_string(), value.to_string()));
    }
    out
}

fn is_array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || (!key.starts_with('0') && !key.is_empty());
    if !canonical || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|n| *n != u32::MAX)
}

/// Orden de `Object.keys`: indices enteros canonicos ascendentes primero, luego el resto en
/// orden de insercion.
fn js_key_order(names: Vec<&str>) -> Vec<&str> {
    let mut indices: Vec<(u32, &str)> = names
        .iter()
        .filter_map(|n| is_array_index(n).map(|i| (i, *n)))
        .collect();
    indices.sort_by_key(|(i, _)| *i);
    let mut out: Vec<&str> = indices.into_iter().map(|(_, n)| n).collect();
    out.extend(names.into_iter().filter(|n| is_array_index(n).is_none()));
    out
}

async fn health() -> Response {
    json_response(
        axum::http::StatusCode::OK,
        &json!({
            "status": "healthy",
            "bot": "online",
            "uptime": 1000
        }),
    )
}

async fn halloween_status() -> Response {
    json_response(
        axum::http::StatusCode::OK,
        &json!({ "active": false }),
    )
}

async fn halloween_toggle() -> Response {
    json_response(
        axum::http::StatusCode::OK,
        &json!({ "active": false }),
    )
}

async fn halloween_reset() -> Response {
    json_response(
        axum::http::StatusCode::OK,
        &json!({ "success": true }),
    )
}

async fn matchmaking_status() -> Response {
    json_response(
        axum::http::StatusCode::OK,
        &json!({ "status": "idle" }),
    )
}

async fn matchmaking_launch() -> Response {
    json_response(
        axum::http::StatusCode::OK,
        &json!({ "success": true }),
    )
}

async fn checkout_purchase() -> Response {
    json_response(
        axum::http::StatusCode::CREATED,
        &json!({
            "success": true,
            "orderId": format!("ORD-{}", Utc::now().timestamp_millis())
        }),
    )
}

async fn create_reclamacion() -> Response {
    json_response(
        axum::http::StatusCode::CREATED,
        &json!({
            "success": true,
            "claimCode": format!("LR-{}", Utc::now().timestamp_millis())
        }),
    )
}
