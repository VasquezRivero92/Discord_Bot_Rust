use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use data::FirestoreClient;
use serde_json::json;
use tower::ServiceExt;

use web::routes::account::{router, AccountState};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "test-secret-at-least-32-chars-long-abc!";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_token(user_id: &str, username: &str) -> String {
    let c = new_claims(
        user_id.into(),
        username.into(),
        None,
        "usuario".into(),
        false,
        None,
        now(),
    );
    sign(&c, SECRET.as_bytes()).unwrap()
}

#[tokio::test]
async fn sync_faceit_from_steam_fails_without_steam_id_64() {
    let fs = Arc::new(FirestoreClient::in_memory_mysql());
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let st = AccountState::new(
        fs,
        session,
        Some("test_faceit_api_key".into()),
        "http://localhost:15633".into(),
        "default".into(),
    );
    let app = router(st);

    let token = make_token("u123", "Gamer123");
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/sync-faceit-from-steam")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from("{}"))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["error"].as_str().unwrap().contains("No tienes una cuenta de Steam conectada"));
}

#[tokio::test]
async fn verify_faceit_rejects_empty_nickname() {
    let fs = Arc::new(FirestoreClient::in_memory_mysql());
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let st = AccountState::new(
        fs,
        session,
        Some("test_faceit_api_key".into()),
        "http://localhost:15633".into(),
        "default".into(),
    );
    let app = router(st);

    let token = make_token("u123", "Gamer123");
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/verify-faceit")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(json!({ "nickname": "" }).to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "Ingresa un nickname de FACEIT válido.");
}

#[tokio::test]
async fn link_steam_validates_steam_id_format() {
    let fs = Arc::new(FirestoreClient::in_memory_mysql());
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let st = AccountState::new(
        fs,
        session,
        Some("test_faceit_api_key".into()),
        "http://localhost:15633".into(),
        "default".into(),
    );
    let app = router(st);

    let token = make_token("u123", "Gamer123");
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/link-steam")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(json!({ "steamInput": "invalido123" }).to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["error"].as_str().unwrap().contains("Formato inválido"));
}

#[tokio::test]
async fn link_steam_saves_valid_steam_id() {
    let fs = Arc::new(FirestoreClient::in_memory_mysql());
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let st = AccountState::new(
        fs.clone(),
        session,
        Some("test_faceit_api_key".into()),
        "http://localhost:15633".into(),
        "default".into(),
    );
    let app = router(st);

    let token = make_token("u456", "SteamPlayer");
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/link-steam")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(json!({ "steamInput": "76561198012345678" }).to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let doc: serde_json::Value = fs.get_doc("faceit_users", "u456").await.unwrap().unwrap();
    assert_eq!(doc["steam_id_64"], "76561198012345678");
}
