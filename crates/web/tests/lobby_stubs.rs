//! Pruebas de integración para rutas stub/simuladas eliminadas del lobby (Tarea B11).
//!
//! Verifica que las rutas que fingían éxito sin implementar la acción real
//! (mixton-check-bet-status, mixton-load-team, mixton-override-tier,
//! sync-faceit-from-steam, verify-faceit, mixton-toggle-command,
//! create-match, mixton-create-match) ya no sean respondidas por el router
//! del lobby (devuelven 404 o 405, nunca 200), de modo que caigan al proxy
//! del legado Node.

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

const SECRET: &str = "lobby-stubs-test-secret-at-least-32-chars!";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_token(role: &str, is_admin: bool, servidor: Option<&str>) -> String {
    let c = new_claims(
        "admin_123".into(),
        "TesterAdmin".into(),
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
        cs2: None,
        docs: None,
    })
}

#[tokio::test]
async fn test_rutas_stub_no_son_respondidas_por_el_lobby_admin_autenticado() {
    let app = test_router();
    let admin_token = make_token("admin", true, Some("default"));

    let stubs = [
        "/api/lobby/mixton-check-bet-status",
        "/api/lobby/mixton-load-team",
        "/api/lobby/mixton-override-tier",
        "/api/lobby/sync-faceit-from-steam",
        "/api/lobby/verify-faceit",
        "/api/lobby/mixton-toggle-command",
        "/api/lobby/create-match",
    ];

    for uri in stubs {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, format!("auth_token={admin_token}"))
            .body(Body::from(
                json!({
                    "guildId": "default",
                    "discordId": "admin_123",
                    "teamId": "team_1",
                    "teamIdA": "team_1",
                    "teamIdB": "team_2"
                })
                .to_string(),
            ))
            .unwrap();

        let res = app.clone().oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::OK,
            "Ruta {uri} devolvió 200 inesperadamente"
        );
        assert!(
            res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::METHOD_NOT_ALLOWED,
            "Ruta {uri} devolvió estado {}, se esperaba 404 o 405",
            res.status()
        );
    }
}
