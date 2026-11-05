//! Pruebas de integración para rutas de dinero del lobby Mixton (Tarea B4).
//!
//! Verifica que las 4 rutas de dinero (wallet, player-balance, quick-add-balance, recharge-yape)
//! NO sean respondidas por el router del lobby (devuelven 404 o 405, nunca 200 con success/balance),
//! de modo que caigan al proxy del legado Node que sí maneja la persistencia en Firestore.

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

fn test_router() -> Router {
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let manager = Arc::new(LobbyManager::new("default"));
    let tmp_uploads = tempfile::tempdir().expect("tempdir");
    let uploads_dir = tmp_uploads.path().to_path_buf();
    std::mem::forget(tmp_uploads);
    lobby::router(LobbyApiState {
        manager,
        auth: Arc::new(session),
        uploads_dir,
    })
}

#[tokio::test]
async fn test_rutas_de_dinero_no_son_respondidas_por_el_lobby_usuario_autenticado() {
    let app = test_router();
    let user_token = make_token("user", false, None);

    // 1. GET /api/lobby/player-balance/123
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/player-balance/123")
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

    // 2. GET /api/lobby/wallet
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

    // 3. POST /api/lobby/quick-add-balance
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::from(
            json!({"discordId": "user_123", "amount": 50.0}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_ne!(res.status(), StatusCode::OK);
    assert!(
        res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::METHOD_NOT_ALLOWED,
        "Se esperaba 404 o 405, recibido: {}",
        res.status()
    );

    // 4. POST /api/lobby/wallet/recharge-yape
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/wallet/recharge-yape")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::from(
            json!({"amount": 50.0, "code": "123456"}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_ne!(res.status(), StatusCode::OK);
    assert!(
        res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::METHOD_NOT_ALLOWED,
        "Se esperaba 404 o 405, recibido: {}",
        res.status()
    );
}

#[tokio::test]
async fn test_rutas_de_dinero_no_son_respondidas_por_el_lobby_admin_autenticado() {
    let app = test_router();
    let admin_token = make_token("admin", false, Some("default"));

    // Probar las 4 rutas con admin para asegurar que ni siquiera con privilegios el lobby las atiende
    let endpoints = vec![
        ("GET", "/api/lobby/player-balance/123", Body::empty()),
        ("GET", "/api/lobby/wallet", Body::empty()),
        (
            "POST",
            "/api/lobby/quick-add-balance",
            Body::from(json!({"discordId": "user_123", "amount": 100.0}).to_string()),
        ),
        (
            "POST",
            "/api/lobby/wallet/recharge-yape",
            Body::from(json!({"amount": 100.0}).to_string()),
        ),
    ];

    for (method, uri, body) in endpoints {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, format!("auth_token={admin_token}"))
            .body(body)
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::OK,
            "Ruta {} devolvió 200 inesperadamente",
            uri
        );
        assert!(
            res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::METHOD_NOT_ALLOWED,
            "Ruta {} devolvió estado {}, se esperaba 404 o 405",
            uri,
            res.status()
        );
    }
}
