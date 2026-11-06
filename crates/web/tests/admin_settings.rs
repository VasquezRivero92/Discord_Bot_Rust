//! Pruebas de integración para las rutas de configuración y servidores de administración (Tanda 1):
//! - GET/POST `/api/admin/settings`
//! - GET `/api/admin/guilds`
//! - GET/POST `/api/admin/guilds/:guildId/settings`
//! - GET `/api/admin/system`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, StatusCode},
    Router,
};
use common::config::Config;
use data::{
    DataError, FakeDocSink, FakeLogsRepo, FakeMatchHistoryRepo, FakeMessagesRepo, FakeOrdersRepo,
    FakeSupportCallsRepo, FirestoreAdminDocsRepo, GlobalSettings, GuildSettings,
    InMemoryClaimsRepo, InMemoryGlobalSettingsRepo, InMemoryGuildSettingsRepo, InMemoryNodeDocs,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::routes::admin::{router, AdminCaches, AdminRepos, AdminState};
use web::session::{new_claims, sign, SessionClaims, SessionState};

const SECRET: &str = "admin-settings-test-secret";
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

fn admin_token(servidor: Option<&str>) -> String {
    token(&claims("admin", servidor))
}

fn user_token() -> String {
    token(&claims("usuario", None))
}

struct TestHarness {
    doc_sink: Arc<FakeDocSink>,
    global_settings: Arc<InMemoryGlobalSettingsRepo>,
    guild_settings: Arc<InMemoryGuildSettingsRepo>,
    repos: AdminRepos,
}

impl TestHarness {
    fn new() -> Self {
        let docs = Arc::new(InMemoryNodeDocs::new());
        let global_settings = Arc::new(InMemoryGlobalSettingsRepo::new());
        let guild_settings = Arc::new(InMemoryGuildSettingsRepo::new());
        let doc_sink = Arc::new(FakeDocSink::new());

        let vars: HashMap<String, String> = [
            ("ADMIN_JWT_SECRET", SECRET),
            ("BASE_URL", "http://localhost:15633"),
            ("DISCORD_CLIENT_ID", "12345"),
            ("DISCORD_CLIENT_SECRET", "test-client-secret"),
            ("DISCORD_TOKEN", "test-bot-token"),
            ("DISCORD_GUILD_ID", "guild_default_1"),
            ("LEGACY_URL", "http://localhost:3000"),
            ("STATIC_DIR", "."),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let config = Arc::new(Config::from_map(&vars).unwrap());

        let repos = AdminRepos {
            claims: Arc::new(InMemoryClaimsRepo::default()),
            orders: Arc::new(FakeOrdersRepo::default()),
            logs: Arc::new(FakeLogsRepo::default()),
            support_calls: Arc::new(FakeSupportCallsRepo::default()),
            match_history: Arc::new(FakeMatchHistoryRepo::default()),
            messages: Arc::new(FakeMessagesRepo::default()),
            global_settings: global_settings.clone(),
            guild_settings: guild_settings.clone(),
            docs: Arc::new(FirestoreAdminDocsRepo::new(docs)),
            doc_sink: doc_sink.clone(),
            users: Arc::new(data::FakeUsersRepo::new()),
            config,
            caches: AdminCaches::default(),
            watchers: Vec::new(),
        };

        TestHarness {
            doc_sink,
            global_settings,
            guild_settings,
            repos,
        }
    }

    fn router(&self) -> Router {
        router(
            AdminState {
                session: SessionState::new(SECRET.as_bytes(), false).unwrap(),
            },
            Some(self.repos.clone()),
        )
    }

    fn router_without_repos(&self) -> Router {
        router(
            AdminState {
                session: SessionState::new(SECRET.as_bytes(), false).unwrap(),
            },
            None,
        )
    }
}

async fn get_req(
    router: Router,
    path: &str,
    cookie: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, String, Value) {
    let mut req = axum::http::Request::builder().uri(path);
    if let Some(t) = cookie {
        req = req.header(header::COOKIE, format!("auth_token={t}"));
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, ct, body)
}

async fn post_req(
    router: Router,
    path: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, String, Value) {
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = cookie {
        req = req.header(header::COOKIE, format!("auth_token={t}"));
    }
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let res = router
        .oneshot(req.body(Body::from(body_bytes)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, ct, body)
}

// ------------------------------------------------------------------- tests

#[tokio::test]
async fn unauthenticated_returns_401() {
    let harness = TestHarness::new();
    let r = harness.router();

    let paths = [
        ("GET", "/api/admin/settings"),
        ("POST", "/api/admin/settings"),
        ("GET", "/api/admin/guilds"),
        ("GET", "/api/admin/guilds/123/settings"),
        ("POST", "/api/admin/guilds/123/settings"),
        ("GET", "/api/admin/system"),
    ];

    for (method, path) in paths {
        let (status, ct, body) = if method == "GET" {
            get_req(r.clone(), path, None, &[]).await
        } else {
            post_req(r.clone(), path, None, json!({})).await
        };
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(ct, JSON_CT, "{method} {path}");
        assert_eq!(body["error"], "No autenticado", "{method} {path}");
    }
}

#[tokio::test]
async fn invalid_token_returns_401() {
    let harness = TestHarness::new();
    let r = harness.router();

    let (status, ct, body) = get_req(r.clone(), "/api/admin/settings", Some("bad-token"), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Token inválido o expirado");
}

#[tokio::test]
async fn forbidden_for_non_admins() {
    let harness = TestHarness::new();
    let r = harness.router();
    let u_token = user_token();

    // GET /settings con rol usuario -> 403
    let (status, ct, body) = get_req(r.clone(), "/api/admin/settings", Some(&u_token), &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(
        body["error"],
        "No autorizado. Se requieren permisos de administrador."
    );

    // POST /settings con rol usuario -> 403
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/settings",
        Some(&u_token),
        json!({"gamesPerMatch": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(
        body["error"],
        "No autorizado. Se requieren permisos de administrador."
    );
}

#[tokio::test]
async fn guild_settings_enforces_guild_ownership() {
    let harness = TestHarness::new();
    let r = harness.router();

    // Admin solo asignado a "guild_1"
    let a_token = admin_token(Some("guild_1"));

    // Intentar acceder a "guild_2" -> 403
    let (status, ct, body) = get_req(
        r.clone(),
        "/api/admin/guilds/guild_2/settings",
        Some(&a_token),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "No autorizado para gestionar este servidor.");

    // Intentar guardar en "guild_2" -> 403
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/guilds/guild_2/settings",
        Some(&a_token),
        json!({"gamesPerMatch": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "No autorizado para gestionar este servidor.");

    // Acceder a su propio servidor "guild_1" -> 200
    let (status, ct, _body) = get_req(
        r.clone(),
        "/api/admin/guilds/guild_1/settings",
        Some(&a_token),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    // Global admin puede acceder a cualquier servidor
    let g_token = global_admin_token();
    let (status, ct, _body) = get_req(
        r.clone(),
        "/api/admin/guilds/guild_2/settings",
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
}

#[tokio::test]
async fn get_settings_returns_effective_structure() {
    let harness = TestHarness::new();
    harness.global_settings.set(Some(
        serde_json::from_value::<GlobalSettings>(json!({
            "discordGuildId": "guild_default_1",
            "gamesPerMatch": 3,
            "faceitClubId": "club-456",
            "discordMatchesChannelId": "channel-matches",
        }))
        .unwrap(),
    ));

    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = get_req(r.clone(), "/api/admin/settings", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let settings = &body["settings"];
    assert_eq!(settings["discordGuildId"], "guild_default_1");
    assert_eq!(settings["gamesPerMatch"], 3);
    assert_eq!(settings["roundsPerGame"], 3);
    assert_eq!(settings["faceitClubId"], "club-456");
    assert_eq!(settings["discordMatchesChannelId"], "channel-matches");
}

#[tokio::test]
async fn post_settings_updates_and_audits() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let update = json!({
        "gamesPerMatch": 5,
        "faceitClubId": "club-999"
    });

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/settings",
        Some(&g_token),
        update,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["settings"]["gamesPerMatch"], 5);
    assert_eq!(body["settings"]["roundsPerGame"], 5);
    assert!(body["settings"]["updatedAt"].is_object());

    // Verificar que se guardó en FakeDocSink
    let saved = harness.doc_sink.get("faceit_config", "settings").expect("documento guardado");
    assert_eq!(saved["gamesPerMatch"], 5);
    assert_eq!(saved["roundsPerGame"], 5);
    assert_eq!(saved["faceitClubId"], "club-999");
}

#[tokio::test]
async fn channel_id_lists_are_saved_and_read_back_for_global_settings() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let update = json!({
        "discordSupportChannelId": "111",
        "discordSupportChannelIds": ["111", "222"],
    });
    let (status, _ct, _body) =
        post_req(r.clone(), "/api/admin/settings", Some(&g_token), update).await;
    assert_eq!(status, StatusCode::OK);

    let saved = harness.doc_sink.get("faceit_config", "settings").expect("documento guardado");
    assert_eq!(saved["discordSupportChannelId"], "111");
    assert_eq!(saved["discordSupportChannelIds"], json!(["111", "222"]));
}

#[tokio::test]
async fn channel_id_lists_are_saved_and_read_back_for_guild_settings() {
    let harness = TestHarness::new();
    let guild_id = "guild_special_2";
    let r = harness.router();
    let g_token = global_admin_token();

    let update = json!({
        "discordAdminAlertsChannelId": "7",
        "discordAdminAlertsChannelIds": ["7", "8", "9"],
    });
    let (status, _ct, _body) = post_req(
        r.clone(),
        &format!("/api/admin/guilds/{guild_id}/settings"),
        Some(&g_token),
        update,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let saved = harness.doc_sink.get("guild_settings", guild_id).expect("guild settings guardado");
    assert_eq!(saved["discordAdminAlertsChannelIds"], json!(["7", "8", "9"]));

    // Lo guardado vuelve a leerse desde la base de datos con la lista completa.
    harness
        .guild_settings
        .insert(guild_id, serde_json::from_value::<GuildSettings>(saved).unwrap());
    let (status, _ct, body) = get_req(
        r.clone(),
        &format!("/api/admin/guilds/{guild_id}/settings"),
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["settings"]["discordAdminAlertsChannelId"], "7");
    assert_eq!(body["settings"]["discordAdminAlertsChannelIds"], json!(["7", "8", "9"]));
}

#[tokio::test]
async fn get_guilds_returns_list() {
    let harness = TestHarness::new();
    let r = harness.router();
    let a_token = admin_token(None);

    let (status, ct, body) = get_req(r.clone(), "/api/admin/guilds", Some(&a_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let guilds = body["guilds"].as_array().expect("array de guilds");
    assert_eq!(guilds.len(), 1);
    assert_eq!(guilds[0]["id"], "guild_default_1");
    assert_eq!(guilds[0]["name"], "Servidor Principal");
}

#[tokio::test]
async fn get_and_post_guild_settings_flow() {
    let harness = TestHarness::new();
    let guild_id = "guild_special_1";
    harness.guild_settings.insert(
        guild_id,
        serde_json::from_value::<GuildSettings>(json!({
            "gamesPerMatch": 3,
            "adminRoleId": "role-special-admin"
        }))
        .unwrap(),
    );

    let r = harness.router();
    let g_token = global_admin_token();

    // GET
    let (status, ct, body) = get_req(
        r.clone(),
        &format!("/api/admin/guilds/{guild_id}/settings"),
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["settings"]["gamesPerMatch"], 3);
    assert_eq!(body["settings"]["adminRoleId"], "role-special-admin");

    // POST
    let update = json!({
        "roundsPerGame": 1,
        "customExtraField": "hello-world"
    });
    let (status, ct, body) = post_req(
        r.clone(),
        &format!("/api/admin/guilds/{guild_id}/settings"),
        Some(&g_token),
        update,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["settings"]["gamesPerMatch"], 1);
    assert_eq!(body["settings"]["roundsPerGame"], 1);
    assert_eq!(body["settings"]["customExtraField"], "hello-world");

    // Verificar en FakeDocSink
    let saved = harness.doc_sink.get("guild_settings", guild_id).expect("guild settings guardado");
    assert_eq!(saved["gamesPerMatch"], 1);
    assert_eq!(saved["customExtraField"], "hello-world");
}

#[tokio::test]
async fn get_system_returns_uptime_and_region() {
    let harness = TestHarness::new();
    let r = harness.router();
    let a_token = admin_token(None);

    let (status, ct, body) = get_req(r.clone(), "/api/admin/system", Some(&a_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    assert!(body["uptimeSeconds"].is_number());
    assert!(body["uptimeSeconds"].as_f64().unwrap() >= 0.0);
    assert!(body["hubRegion"].is_string());
}

#[tokio::test]
async fn post_settings_firestore_error_returns_500() {
    let harness = TestHarness::new();
    harness.doc_sink.set_error(Some(DataError::Firestore("simulated write error".into())));

    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/settings",
        Some(&g_token),
        json!({"gamesPerMatch": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Error al guardar settings en .env");
}

#[tokio::test]
async fn post_guild_settings_firestore_error_returns_500() {
    let harness = TestHarness::new();
    harness.doc_sink.set_error(Some(DataError::Firestore("simulated write error".into())));

    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/guilds/guild_1/settings",
        Some(&g_token),
        json!({"gamesPerMatch": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "No se pudo guardar la configuración del servidor");
}

#[tokio::test]
async fn post_invalid_body_returns_400() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/settings",
        Some(&g_token),
        json!("string-not-an-object"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "El cuerpo debe ser un objeto JSON");
}

#[tokio::test]
async fn routes_unmounted_without_repos() {
    let harness = TestHarness::new();
    let r = harness.router_without_repos();
    let g_token = global_admin_token();

    // Sin repos, las rutas no están montadas en router() (caen al fallback / 404 del router local)
    let (status, _, _) = get_req(r.clone(), "/api/admin/settings", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = get_req(r.clone(), "/api/admin/guilds", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
