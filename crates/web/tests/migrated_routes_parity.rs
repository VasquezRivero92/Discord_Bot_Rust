//! Pruebas de integración para las 36 rutas migradas del legado Node hacia Rust.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use common::config::Config;
use data::{
    FakeDocSink, FakeLogsRepo, FakeMatchHistoryRepo, FakeMessagesRepo, FakeOrdersRepo,
    FakeSupportCallsRepo, FakeUsersRepo, FirestoreAdminDocsRepo, InMemoryClaimsRepo,
    InMemoryGlobalSettingsRepo, InMemoryGuildSettingsRepo, InMemoryNodeDocs,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::routes::admin::{AdminCaches, AdminRepos};
use web::session::{new_claims, sign, SessionClaims, SessionState};

const SECRET: &str = "test-secret-parity";
const JSON_CT: &str = "application/json; charset=utf-8";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims(role: &str, servidor: Option<&str>) -> SessionClaims {
    new_claims(
        "100".into(),
        format!("user-{role}"),
        None,
        role.into(),
        false,
        servidor.map(str::to_owned),
        now(),
    )
}

fn token(c: &SessionClaims) -> String {
    sign(c, SECRET.as_bytes()).unwrap()
}

fn global_admin_token() -> String {
    token(&claims("global_admin", None))
}

#[allow(dead_code)]
fn admin_token(servidor: Option<&str>) -> String {
    token(&claims("admin", servidor))
}

struct TestApp {
    app: axum::Router,
    #[allow(dead_code)]
    sink: Arc<FakeDocSink>,
}

fn build_test_app() -> TestApp {
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", SECRET),
        ("SESSION_SECRET", SECRET),
        ("BASE_URL", "http://localhost:15633"),
        ("DISCORD_CLIENT_ID", "12345"),
        ("DISCORD_CLIENT_SECRET", "test-client-secret"),
        ("DISCORD_TOKEN", "test-bot-token"),
        ("DISCORD_GUILD_ID", "guild-main"),
        ("LEGACY_URL", "http://localhost:3000"),
        ("STATIC_DIR", "."),
        ("STEAM_BOT_API_KEY", "test-steam-key-123"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let cfg = Arc::new(Config::from_map(&vars).unwrap());

    let sink = Arc::new(FakeDocSink::new());
    let claims_repo = Arc::new(InMemoryClaimsRepo::new());
    claims_repo.insert(
        "LR-2026-000001",
        data::Claim {
            claim_code: Some("LR-2026-000001".into()),
            ..Default::default()
        },
    );
    let orders_repo = Arc::new(FakeOrdersRepo::new());
    let logs_repo = Arc::new(FakeLogsRepo::new());
    let support_calls_repo = Arc::new(FakeSupportCallsRepo::new());
    let match_history_repo = Arc::new(FakeMatchHistoryRepo::new());
    let messages_repo = Arc::new(FakeMessagesRepo::new());
    let global_settings = Arc::new(InMemoryGlobalSettingsRepo::new());
    let guild_settings = Arc::new(InMemoryGuildSettingsRepo::new());
    let docs = Arc::new(FirestoreAdminDocsRepo::new(InMemoryNodeDocs::new()));
    let users = Arc::new(FakeUsersRepo::new());

    let repos = AdminRepos {
        claims: claims_repo,
        orders: orders_repo,
        logs: logs_repo,
        support_calls: support_calls_repo,
        match_history: match_history_repo,
        messages: messages_repo,
        global_settings,
        guild_settings,
        docs,
        doc_sink: sink.clone(),
        users,
        config: cfg.clone(),
        firestore: None,
        caches: AdminCaches::default(),
        watchers: vec![],
    };

    let _session = SessionState::from_config(&cfg).unwrap();
    let app = web::build_app(web::AppDeps {
        config: (*cfg).clone(),
        claims: None,
        admin: Some(repos),
        cs2: None,
        lobby: None,
        firestore: None,
    });

    TestApp { app, sink }
}

async fn req_json(
    app: &axum::Router,
    method: Method,
    uri: &str,
    auth_token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, JSON_CT);

    if let Some(t) = auth_token {
        builder = builder.header(header::COOKIE, format!("auth_token={t}"));
    }

    let req_body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };

    let req = builder.body(req_body).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

#[tokio::test]
async fn test_admin_user_management_writes() {
    let app = build_test_app();
    let g_tok = global_admin_token();

    // 1. Unlink user
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/users/123456/unlink",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // 2. Admin role assignment (usuario, admin, global_admin)
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/users/123456/admin-role",
        Some(&g_tok),
        Some(json!({ "role": "admin", "guildId": "guild-main" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // 3. User roles
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/users/123456/roles",
        Some(&g_tok),
        Some(json!({ "roleType": "level", "roleValue": 10 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // 4. Verify user
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/users/verify",
        Some(&g_tok),
        Some(json!({ "discordId": "123456", "faceitNickname": "pro_player" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
    assert_eq!(b["player"]["nickname"], "pro_player");

    // 5. Sync-all and sync-nicks
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/users/sync-all",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/users/sync-nicks",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
}

#[tokio::test]
async fn test_support_orders_claims_writes() {
    let app = build_test_app();
    let g_tok = global_admin_token();

    // 1. Resolve support call
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/support-calls/call_999/resolve",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // 2. Order status
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/orders/ord_123/status",
        Some(&g_tok),
        Some(json!({ "status": "completada", "note": "Entregado con exito" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
    assert_eq!(b["message"], "Orden actualizada a COMPLETADA.");

    // 3. Claims respond
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/claims/LR-2026-000001/respond",
        Some(&g_tok),
        Some(json!({ "response": "Solucionado satisfactoriamente." })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // 4. Claims delete (global_admin)
    let (s, b) = req_json(
        &app.app,
        Method::DELETE,
        "/api/admin/claims/LR-2026-000001",
        Some(&g_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
}

#[tokio::test]
async fn test_yape_manual_approve_and_messages() {
    let app = build_test_app();
    let g_tok = global_admin_token();

    // 1. Yape manual approve
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/yape/manual-approve",
        Some(&g_tok),
        Some(json!({
            "operationNumber": "OP123456",
            "discordId": "user_yape_1",
            "amount": 25.50,
            "reason": "Recarga confirmada en banco"
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
    assert_eq!(b["newBalance"], 25.50);

    // 2. Update message templates
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/messages",
        Some(&g_tok),
        Some(json!({ "welcome": "Bienvenido a MachuPeek!" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
    assert_eq!(b["templates"]["welcome"], "Bienvenido a MachuPeek!");
}

#[tokio::test]
async fn test_steam_bot_admin_endpoints() {
    let app = build_test_app();
    let g_tok = global_admin_token();

    // 1. Steam status
    let (s, b) = req_json(
        &app.app,
        Method::GET,
        "/api/admin/steam/status",
        Some(&g_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
    assert_eq!(b["status"]["connected"], false);

    // 2. Steam Guard (valid code)
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/steam/guard",
        Some(&g_tok),
        Some(json!({ "code": "ABC12" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // 3. Steam Guard (invalid code length)
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/steam/guard",
        Some(&g_tok),
        Some(json!({ "code": "ABC" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(b["error"].as_str().unwrap().contains("5 caracteres"));

    // 4. Steam login & reset
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/steam/login",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/steam/reset",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);
}

#[tokio::test]
async fn test_broadcast_refresh_simulate_status_queues_metrics() {
    let app = build_test_app();
    let g_tok = global_admin_token();

    // Broadcast
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/broadcast",
        Some(&g_tok),
        Some(json!({ "message": "Atención a todos los jugadores" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // Panel refresh
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/panel/refresh",
        Some(&g_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // Simulate webhook
    let (s, b) = req_json(
        &app.app,
        Method::POST,
        "/api/admin/simulate-webhook",
        Some(&g_tok),
        Some(json!({ "event": "match_status_ready" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["success"], true);

    // Status
    let (s, b) = req_json(
        &app.app,
        Method::GET,
        "/api/admin/status",
        Some(&g_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["discordConnected"], true);

    // Firestore metrics
    let (s, b) = req_json(
        &app.app,
        Method::GET,
        "/api/admin/firestore-metrics",
        Some(&g_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["reads"].is_number());

    // Faceit club queues
    let (s, b) = req_json(
        &app.app,
        Method::GET,
        "/api/admin/faceit-club-queues",
        Some(&g_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["queues"].is_array());

    // Stats by nickname
    let (s, b) = req_json(
        &app.app,
        Method::GET,
        "/api/admin/stats/test_nick",
        Some(&g_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["player"]["nickname"], "test_nick");
}

#[tokio::test]
async fn test_matchmaking_bridge() {
    std::env::set_var("STEAM_BOT_API_KEY", "test-steam-key-123");
    let app = build_test_app();

    // 1. Without API key -> 401
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/matchmaking/status")
        .body(Body::empty())
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // 2. With valid API key -> 200
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/matchmaking/status")
        .header("x-api-key", "test-steam-key-123")
        .body(Body::empty())
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(val["status"], "online");

    // 3. Launch validation: 10 players required
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/matchmaking/launch")
        .header("Authorization", "Bearer test-steam-key-123")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({
            "serverIp": "127.0.0.1:27015",
            "players": ["76561198000000001", "76561198000000002"]
        }).to_string()))
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 4. Launch success: exactly 10 valid unique SteamID64s
    let players: Vec<String> = (1..=10)
        .map(|i| format!("765611980000000{:02}", i))
        .collect();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/matchmaking/launch")
        .header("x-api-key", "test-steam-key-123")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({
            "serverIp": "127.0.0.1:27015",
            "players": players
        }).to_string()))
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(val["success"], true);
    assert!(val["lobbyId"].as_str().unwrap().starts_with("lobby_"));
}

#[tokio::test]
async fn test_halloween_endpoints() {
    let app = build_test_app();

    // 1. Status
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/halloween/status")
        .body(Body::empty())
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["state"]["user1"], false);

    // 2. Toggle user 1
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/halloween/toggle")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "userId": 1 }).to_string()))
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["state"]["user1"], true);

    // 3. Reset
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/halloween/reset")
        .body(Body::empty())
        .unwrap();
    let res = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["state"]["user1"], false);
}
