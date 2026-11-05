//! Pruebas de autenticación y autorización para las rutas del lobby Mixton.
//!
//! Verifica las 5 condiciones obligatorias de SDD Task B1:
//! (a) token sin firma válida -> 401 en ruta protegida;
//! (b) token válido no admin -> 403 en mixton-setup y ok en mixton-captain-proposal;
//! (c) admin válido pasa;
//! (d) role=admin con guild ajeno -> 403;
//! (e) GET /api/lobby/state no devuelve isAdmin:true a usuario normal ni a token forjado.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use lobby::{LobbyApiState, LobbyManager};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-auth-test-secret-at-least-32-chars!";

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
    lobby::router(LobbyApiState {
        manager,
        auth: Arc::new(session),
    })
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[tokio::test]
async fn test_a_token_sin_firma_valida_devuelve_401() {
    let app = test_router();

    // 1. Petición sin cookies a ruta RequireUser
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-captain-proposal")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"teamId": "team_1", "betAmount": 10.0}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(res).await;
    assert_eq!(body["error"], "No autenticado");

    // 2. Petición con cookie forjada / firma inválida a ruta RequireUser
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-captain-proposal")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, "auth_token=forged.token.here")
        .body(Body::from(
            json!({"teamId": "team_1", "betAmount": 10.0}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(res).await;
    assert_eq!(body["error"], "Token inválido o expirado");

    // 3. Petición con cookie forjada a ruta RequireAdmin
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-setup")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, "auth_token=forged.jwt.token")
        .body(Body::from(json!({"numTeams": 2}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(res).await;
    assert_eq!(body["error"], "Token inválido o expirado");
}

#[tokio::test]
async fn test_b_token_valido_no_admin_devuelve_403_en_setup_y_ok_en_proposal() {
    let app = test_router();
    let user_token = make_token("user", false, None);

    // 1. Ruta de administración (mixton-setup) -> 403 Forbidden
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-setup")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::from(json!({"numTeams": 2}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(body["error"], "Se requieren permisos de administrador.");

    // 2. Ruta de usuario (mixton-captain-proposal) -> 200 OK
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-captain-proposal")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::from(
            json!({"teamId": "team_1", "betAmount": 50.0}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn test_c_admin_valido_pasa() {
    let app = test_router();
    let admin_token = make_token("admin", false, Some("default"));

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-setup")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(json!({"numTeams": 2}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    // Con global_admin
    let global_admin_token = make_token("global_admin", false, None);
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-setup")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={global_admin_token}"))
        .body(Body::from(json!({"numTeams": 2}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn test_d_role_admin_con_guild_ajeno_devuelve_403() {
    let app = test_router();
    // Admin asignado a servidor "guild_alpha"
    let admin_token = make_token("admin", false, Some("guild_alpha"));

    // 1. Acceder a servidor ajeno en ruta admin -> 403 con error de gestión
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-setup?guildId=guild_beta")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(json!({"numTeams": 2}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "No tienes permisos para gestionar este servidor."
    );

    // 2. Acceder a servidor ajeno en ruta de usuario -> 403 con error de acceso
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-captain-proposal?guildId=guild_beta")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({"teamId": "team_1", "betAmount": 10.0}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "No tienes permisos para acceder a este servidor."
    );

    // 3. Acceder a su propio servidor ("guild_alpha") -> 200 OK
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-setup?guildId=guild_alpha")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(json!({"numTeams": 2}).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_e_get_state_no_devuelve_is_admin_true_a_usuario_normal_ni_a_token_forjado() {
    let app = test_router();

    // 1. Sin token: 200 OK, currentUser es null / ausente
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/state")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert!(body["currentUser"].is_null());

    // 2. Con token forjado: 200 OK, currentUser es null / ausente (no isAdmin: true)
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/state")
        .header(
            header::COOKIE,
            "auth_token=eyJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiJmb3JnZWQifQ.fake_signature",
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert!(body["currentUser"].is_null());

    // 3. Usuario normal verificado: currentUser presente con isAdmin: false
    let user_token = make_token("user", false, None);
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/state")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["currentUser"]["userId"], "user_123");
    assert_eq!(body["currentUser"]["isAdmin"], false);

    // 4. Admin verificado: currentUser presente con isAdmin: true
    let admin_token = make_token("admin", false, Some("default"));
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/state")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["currentUser"]["userId"], "user_123");
    assert_eq!(body["currentUser"]["isAdmin"], true);
}
