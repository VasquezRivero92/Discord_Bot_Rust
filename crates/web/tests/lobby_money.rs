//! Pruebas de integración para rutas de dinero del lobby Mixton (Tarea B15).
//!
//! Verifica que:
//! - `player-balance` y `quick-add-balance` ahora existen en el router de Rust:
//!   - Sin sesión: devuelven 401 Unauthorized.
//!   - Con sesión pero con `wallet: None`: devuelven 503 Service Unavailable.
//! - `wallet` y `wallet/recharge-yape` siguen sin estar implementadas en Rust:
//!   - Devuelven 404 Not Found o 405 Method Not Allowed (caen al proxy del legado).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::json;
use tower::ServiceExt;

use lobby::{LobbyApiState, LobbyManager};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-money-test-secret-at-least-32-chars!";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_token(role: &str, is_admin: bool, servidor: Option<&str>) -> String {
    let c = new_claims(
        "user_123".into(),
        "Tester".into(),
        Some("avatar.png".into()),
        role.into(),
        is_admin,
        servidor.map(str::to_owned),
        now(),
    );
    sign(&c, SECRET.as_bytes()).unwrap()
}

fn test_router(wallet: Option<Arc<dyn data::WalletStore>>) -> Router {
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let manager = Arc::new(LobbyManager::new("default"));
    let tmp_uploads = tempfile::tempdir().expect("tempdir");
    let uploads_dir = tmp_uploads.path().to_path_buf();
    std::mem::forget(tmp_uploads);
    lobby::router(LobbyApiState {
        manager,
        auth: Arc::new(session),
        uploads_dir,
        cs2: None,
        docs: None,
        notifier: Default::default(),
        wallet,
        bets_enabled: false,
    })
}

#[tokio::test]
async fn test_rutas_existentes_sin_sesion_dan_401() {
    let app = test_router(None);

    // 1. GET /api/lobby/player-balance/123 -> 401
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/player-balance/123")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // 2. POST /api/lobby/quick-add-balance -> 401
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "discordId": "123", "amount": 10.0 }).to_string(),
        ))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_rutas_con_wallet_none_dan_503() {
    let app = test_router(None);
    let admin_token = make_token("admin", true, Some("default"));

    // 1. GET /api/lobby/player-balance/123 -> 503
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/player-balance/123?guild_id=default")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    // 2. POST /api/lobby/quick-add-balance -> 503
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "discordId": "123", "amount": 10.0 }).to_string(),
        ))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_rutas_no_migradas_siguen_devolviendo_404_o_405() {
    let app = test_router(None);
    let user_token = make_token("user", false, None);

    // 1. GET /api/lobby/wallet
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/wallet")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_ne!(res.status(), StatusCode::OK);
    assert!(
        res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::METHOD_NOT_ALLOWED,
        "Se esperaba 404 o 405, recibido: {}",
        res.status()
    );

    // 2. POST /api/lobby/wallet/recharge-yape
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/wallet/recharge-yape")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "amount": 10.0 }).to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_ne!(res.status(), StatusCode::OK);
    assert!(
        res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::METHOD_NOT_ALLOWED,
        "Se esperaba 404 o 405, recibido: {}",
        res.status()
    );
}
