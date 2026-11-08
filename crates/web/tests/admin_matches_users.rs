//! Pruebas de integración para las rutas de administración (Tanda 2):
//! - GET `/api/admin/leaderboard`
//! - GET `/api/admin/discord/roles`
//! - GET/POST `/api/admin/map-catalog`
//! - GET `/api/admin/users`
//! - GET `/api/admin/matches`
//! - GET `/api/admin/matches/:id/stats`
//! - GET `/api/admin/matches/:id/demo`

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, StatusCode},
    routing::get,
    Json, Router,
};
use common::config::Config;
use data::{
    FakeDocSink, FakeLogsRepo, FakeMatchHistoryRepo, FakeMessagesRepo, FakeOrdersRepo,
    FakeSupportCallsRepo, FakeUsersRepo, FirestoreAdminDocsRepo, GlobalSettings,
    InMemoryClaimsRepo, InMemoryGlobalSettingsRepo, InMemoryGuildSettingsRepo, InMemoryNodeDocs,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;
use web::routes::admin::{router, AdminCaches, AdminRepos, AdminState};
use web::session::{new_claims, sign, SessionClaims, SessionState};

const SECRET: &str = "admin-matches-users-test-secret";
/// Los tests fijan `DISCORD_API_BASE_URL` / `FACEIT_API_BASE_URL` (globales del proceso):
/// se ejecutan de uno en uno para que no se pisen.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    docs: Arc<InMemoryNodeDocs>,
    doc_sink: Arc<FakeDocSink>,
    users: Arc<FakeUsersRepo>,
    global_settings: Arc<InMemoryGlobalSettingsRepo>,
    guild_settings: Arc<InMemoryGuildSettingsRepo>,
    repos: AdminRepos,
}

impl TestHarness {
    fn new() -> Self {
        Self::with_config_vars(&[])
    }

    fn with_config_vars(extra: &[(&str, &str)]) -> Self {
        let docs = Arc::new(InMemoryNodeDocs::new());
        let global_settings = Arc::new(InMemoryGlobalSettingsRepo::new());
        let guild_settings = Arc::new(InMemoryGuildSettingsRepo::new());
        let doc_sink = Arc::new(FakeDocSink::new());
        let users = Arc::new(FakeUsersRepo::new());

        let mut vars: HashMap<String, String> = [
            ("ADMIN_JWT_SECRET", SECRET),
            ("BASE_URL", "http://localhost:15633"),
            ("DISCORD_CLIENT_ID", "12345"),
            ("DISCORD_CLIENT_SECRET", "test-client-secret"),
            ("DISCORD_TOKEN", "test-bot-token"),
            ("DISCORD_GUILD_ID", "guild_default_1"),
            ("LEGACY_URL", "http://localhost:3000"),
            ("STATIC_DIR", "."),
            ("FACEIT_API_KEY", "test-faceit-key"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();

        for (k, v) in extra {
            vars.insert((*k).to_owned(), (*v).to_owned());
        }

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
            docs: Arc::new(FirestoreAdminDocsRepo::new(docs.clone())),
            doc_sink: doc_sink.clone(),
            users: users.clone(),
            config,
            firestore: None,
            caches: AdminCaches::default(),
            watchers: Vec::new(),
        };

        TestHarness {
            docs,
            doc_sink,
            users,
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

async fn serve_mock(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, handle)
}

// =================================================================== TESTS

#[tokio::test]
async fn unauthenticated_returns_401() {
    let _env = ENV_LOCK.lock().await;
    let harness = TestHarness::new();
    let r = harness.router();

    let paths = [
        ("GET", "/api/admin/leaderboard"),
        ("GET", "/api/admin/discord/roles"),
        ("GET", "/api/admin/map-catalog"),
        ("POST", "/api/admin/map-catalog"),
        ("GET", "/api/admin/users"),
        ("GET", "/api/admin/matches"),
        ("GET", "/api/admin/matches/m1/stats"),
        ("GET", "/api/admin/matches/m1/demo"),
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
    let _env = ENV_LOCK.lock().await;
    let harness = TestHarness::new();
    let r = harness.router();

    let (status, ct, body) =
        get_req(r.clone(), "/api/admin/leaderboard", Some("bad-token"), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Token inválido o expirado");
}

#[tokio::test]
async fn forbidden_for_non_admins() {
    let _env = ENV_LOCK.lock().await;
    let harness = TestHarness::new();
    let r = harness.router();
    let u_token = user_token();

    let paths = [
        ("GET", "/api/admin/leaderboard"),
        ("GET", "/api/admin/discord/roles"),
        ("GET", "/api/admin/map-catalog"),
        ("POST", "/api/admin/map-catalog"),
        ("GET", "/api/admin/users"),
        ("GET", "/api/admin/matches"),
        ("GET", "/api/admin/matches/m1/stats"),
        ("GET", "/api/admin/matches/m1/demo"),
    ];

    for (method, path) in paths {
        let (status, ct, body) = if method == "GET" {
            get_req(r.clone(), path, Some(&u_token), &[]).await
        } else {
            post_req(r.clone(), path, Some(&u_token), json!({})).await
        };
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(ct, JSON_CT, "{method} {path}");
        assert_eq!(
            body["error"],
            "No autorizado. Se requieren permisos de administrador.",
            "{method} {path}"
        );
    }
}

// ------------------------------------------------------------------ Leaderboard

#[tokio::test]
async fn get_leaderboard_empty_when_no_config() {
    let _env = ENV_LOCK.lock().await;
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    // Sin faceit_leaderboard_id -> {"leaderboard": []}
    let (status, ct, body) = get_req(r.clone(), "/api/admin/leaderboard", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["leaderboard"], json!([]));
}

#[tokio::test]
async fn get_leaderboard_success_with_mock_api() {
    let _env = ENV_LOCK.lock().await;
    let mock_faceit = Router::new().route(
        "/leaderboards/lb_123",
        get(|| async {
            Json(json!({
                "items": [
                    {
                        "player": {
                            "user_id": "faceit_u1",
                            "nickname": "PlayerOne",
                            "skill_level": 8
                        },
                        "points": 1850
                    },
                    {
                        "player": {
                            "user_id": "faceit_u2",
                            "nickname": "UnknownGuy",
                            "skill_level": 5
                        },
                        "points": 1300
                    }
                ]
            }))
        }),
    );
    let (addr, _server) = serve_mock(mock_faceit).await;
    std::env::set_var("FACEIT_API_BASE_URL", format!("http://{addr}"));

    let harness = TestHarness::new();
    harness.global_settings.set(Some(
        serde_json::from_value::<GlobalSettings>(json!({
            "faceitLeaderboardId": "lb_123"
        }))
        .unwrap(),
    ));

    // Sembrar usuario vinculado en FakeUsersRepo para mapear PlayerOne -> discord_111
    harness.users.insert(
        "discord_111",
        json!({
            "discordId": "discord_111",
            "faceit_nickname": "PlayerOne",
        }),
    );

    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = get_req(r.clone(), "/api/admin/leaderboard", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let items = body["leaderboard"].as_array().expect("array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], "faceit_u1");
    assert_eq!(items[0]["discord_id"], "discord_111");
    assert_eq!(items[0]["faceit_nickname"], "PlayerOne");
    assert_eq!(items[0]["faceit_level"], 8);
    assert_eq!(items[0]["faceit_elo"], 1850);

    assert_eq!(items[1]["id"], "faceit_u2");
    assert_eq!(items[1]["discord_id"], "N/A");
    assert_eq!(items[1]["faceit_nickname"], "UnknownGuy");
    assert_eq!(items[1]["faceit_level"], 5);
    assert_eq!(items[1]["faceit_elo"], 1300);

    std::env::remove_var("FACEIT_API_BASE_URL");
}

// ------------------------------------------------------------- Discord Roles

#[tokio::test]
async fn get_discord_roles_success_filters_everyone_and_sorts() {
    let _env = ENV_LOCK.lock().await;
    let mock_discord = Router::new().route(
        "/guilds/guild_default_1/roles",
        get(|| async {
            Json(json!([
                { "id": "1", "name": "@everyone", "color": 0 },
                { "id": "2", "name": "VIP", "color": 16766720 }, // #FFD700
                { "id": "3", "name": "Admin", "color": 16711680 }, // #FF0000
                { "id": "4", "name": "Beta", "color": 0 } // #000000
            ]))
        }),
    );
    let (addr, _server) = serve_mock(mock_discord).await;
    std::env::set_var("DISCORD_API_BASE_URL", format!("http://{addr}"));

    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = get_req(r.clone(), "/api/admin/discord/roles", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let roles = body["roles"].as_array().expect("array de roles");
    assert_eq!(roles.len(), 3);
    // Orden alfabético: Admin, Beta, VIP
    assert_eq!(roles[0]["name"], "Admin");
    assert_eq!(roles[0]["color"], "#FF0000");

    assert_eq!(roles[1]["name"], "Beta");
    assert_eq!(roles[1]["color"], "#000000");

    assert_eq!(roles[2]["name"], "VIP");
    assert_eq!(roles[2]["color"], "#FFD700");

    std::env::remove_var("DISCORD_API_BASE_URL");
}

#[tokio::test]
async fn get_discord_channels_keeps_text_and_voice_with_category() {
    let _env = ENV_LOCK.lock().await;
    let mock_discord = Router::new().route(
        "/guilds/guild_default_1/channels",
        get(|| async {
            Json(json!([
                { "id": "10", "name": "Competitivo", "type": 4, "position": 0 },
                { "id": "11", "name": "partidas", "type": 0, "position": 2, "parent_id": "10" },
                { "id": "12", "name": "sala-espera", "type": 2, "position": 1, "parent_id": "10" },
                { "id": "13", "name": "foro", "type": 15, "position": 3 },
                { "id": "14", "name": "general", "type": 0, "position": 4 }
            ]))
        }),
    );
    let (addr, _server) = serve_mock(mock_discord).await;
    std::env::set_var("DISCORD_API_BASE_URL", format!("http://{addr}"));

    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) =
        get_req(r.clone(), "/api/admin/discord/channels", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let ch = body["channels"].as_array().expect("array de canales");
    // Sin la categoría ni el foro; ordenados por posición.
    assert_eq!(ch.len(), 3);
    assert_eq!(ch[0]["name"], "sala-espera");
    assert_eq!(ch[0]["kind"], "voice");
    assert_eq!(ch[0]["parentName"], "Competitivo");
    assert_eq!(ch[1]["name"], "partidas");
    assert_eq!(ch[1]["kind"], "text");
    assert_eq!(ch[2]["name"], "general");
    assert!(ch[2]["parentId"].is_null());

    std::env::remove_var("DISCORD_API_BASE_URL");
}

#[tokio::test]
async fn get_discord_channels_requires_admin_session() {
    let _env = ENV_LOCK.lock().await;
    let harness = TestHarness::new();
    let r = harness.router();
    let (status, _ct, _body) = get_req(r, "/api/admin/discord/channels", None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ------------------------------------------------------------- Map Catalog

#[tokio::test]
async fn map_catalog_get_and_post_lifecycle() {
    let _env = ENV_LOCK.lock().await;
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    // 1. GET inicial: catálogo base predeterminado
    let (status, ct, body) = get_req(r.clone(), "/api/admin/map-catalog", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["activeCount"], 7); // 7 mapas oficiales premier activos por defecto
    let catalog = body["catalog"].as_array().expect("array");
    assert!(catalog.len() >= 14);

    // 2. POST con cuerpo que no es array -> 400
    let (status, _, body_err) = post_req(
        r.clone(),
        "/api/admin/map-catalog",
        Some(&g_token),
        json!({"catalog": "no-es-un-array"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_err["error"], "El catálogo debe ser un array de mapas.");

    // 3. POST con menos de 2 mapas activos -> 400
    let (status, _, body_err2) = post_req(
        r.clone(),
        "/api/admin/map-catalog",
        Some(&g_token),
        json!([
            { "id": "de_mirage", "name": "Mirage", "active": true },
            { "id": "de_dust2", "name": "Dust II", "active": false }
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body_err2["error"],
        "Debes mantener al menos 2 mapas activos para el sistema de veto."
    );

    // 4. POST con catálogo válido (3 activos)
    let new_catalog = json!([
        { "id": "de_mirage", "name": "Mirage", "active": true },
        { "id": "de_dust2", "name": "Dust II", "active": true },
        { "id": "de_inferno", "name": "Inferno", "active": true },
        { "id": "de_nuke", "name": "Nuke", "active": false },
        { "id": "custom_map_1", "name": "Aim Map", "active": false }
    ]);
    let (status, ct, body_ok) = post_req(
        r.clone(),
        "/api/admin/map-catalog",
        Some(&g_token),
        json!({ "catalog": new_catalog }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body_ok["success"], true);
    // Los mapas base que el admin no menciona conservan su estado por defecto
    // (legacy `getEffectiveMapCatalog`): 3 enviados activos + ancient, anubis y vertigo.
    assert_eq!(body_ok["activeCount"], 6);

    // Verificar en FakeDocSink que se persistió
    let saved = harness
        .doc_sink
        .get("faceit_config", "mixton_lobby")
        .expect("guardado");
    assert!(saved["mapCatalog"].is_array());

    // 5. GET posterior refleja el cambio
    harness.docs.put("faceit_config", "mixton_lobby", saved);
    let (status, _, body_after) = get_req(r.clone(), "/api/admin/map-catalog", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_after["activeCount"], 6);
}

// ------------------------------------------------------------- Users

#[tokio::test]
async fn get_users_filters_guild_and_attaches_discord_id_and_club_role() {
    let _env = ENV_LOCK.lock().await;
    let mock_discord = Router::new()
        .route(
            "/guilds/guild_1/roles",
            get(|| async {
                Json(json!([
                    { "id": "role_prem_id", "name": "PREMIUM", "color": 123 },
                    { "id": "role_other_id", "name": "Regular", "color": 0 }
                ]))
            }),
        )
        .route(
            "/guilds/guild_1/members",
            get(|| async {
                Json(json!([
                    {
                        "user": { "id": "d_100" },
                        "roles": ["role_prem_id"]
                    }
                ]))
            }),
        );
    let (addr, _server) = serve_mock(mock_discord).await;
    std::env::set_var("DISCORD_API_BASE_URL", format!("http://{addr}"));

    let harness = TestHarness::new();
    // Usuario 1 en guild_1 con rol PREMIUM en Discord
    harness.users.insert(
        "d_100",
        json!({
            "discordId": "d_100",
            "guild_id": "guild_1",
            "faceit_nickname": "ProGamer",
            "wallet_balance": 50.0
        }),
    );
    // Usuario 2 en guild_2
    harness.users.insert(
        "d_200",
        json!({
            "discordId": "d_200",
            "servidor": "guild_2",
            "faceit_nickname": "SecondUser",
        }),
    );

    let r = harness.router();
    let g_token = global_admin_token();

    // GET con filtro por guild_1
    let (status, ct, body) = get_req(
        r.clone(),
        "/api/admin/users?guildId=guild_1",
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let users = body["users"].as_array().expect("array de usuarios");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["discordId"], "d_100");
    assert_eq!(users[0]["clubRole"], "PREMIUM");
    assert_eq!(users[0]["faceit_nickname"], "ProGamer");

    std::env::remove_var("DISCORD_API_BASE_URL");
}

// ------------------------------------------------------------- Matches

#[tokio::test]
async fn get_matches_computes_server_load_avg_duration_and_cleanups() {
    let _env = ENV_LOCK.lock().await;
    let mock_discord = Router::new().route(
        "/guilds/guild_default_1/channels",
        get(|| async {
            Json(json!([
                // Canal huérfano (tipo 2 = voz, prefijo 🔊, id no activo)
                { "id": "vc_orphan", "name": "🔊 Sala Libre", "type": 2 },
                // Canal activo (id coincide con faction1_vc_id de partida m1)
                { "id": "vc_active_1", "name": "🔊 Equipo A", "type": 2 },
                // Canal de texto normal
                { "id": "ch_text", "name": "🔊 Anuncios", "type": 0 }
            ]))
        }),
    );
    let (addr, _server) = serve_mock(mock_discord).await;
    std::env::set_var("DISCORD_API_BASE_URL", format!("http://{addr}"));

    let harness = TestHarness::new();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // 2 partidas activas (carga: 2/10 = 20%)
    harness.docs.put(
        "faceit_active_matches",
        "m1",
        json!({
            "guild_id": "guild_default_1",
            "faction1_vc_id": "vc_active_1",
            "startedAt": now_ms - 120_000 // 2 minutos atrás
        }),
    );
    harness.docs.put(
        "faceit_active_matches",
        "m2",
        json!({
            "guild_id": "guild_default_1",
            "startedAt": now_ms - 60_000 // 1 minuto atrás
        }),
    );

    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = get_req(r.clone(), "/api/admin/matches", Some(&g_token), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);

    let matches = body["matches"].as_array().expect("matches array");
    assert_eq!(matches.len(), 2);

    let stats = &body["stats"];
    assert_eq!(stats["serverLoad"], 20.0);
    // Duración promedio aprox ~1m 30s
    let duration_str = stats["avgDurationStr"].as_str().expect("avgDurationStr");
    assert!(duration_str.contains('m') && duration_str.contains('s'));
    assert_eq!(stats["failedCleanups"], 1);

    std::env::remove_var("DISCORD_API_BASE_URL");
}

// ------------------------------------------------------------- Match Stats & Demo

#[tokio::test]
async fn match_stats_and_demo_mixton_and_faceit_flows() {
    let _env = ENV_LOCK.lock().await;
    let mock_faceit = Router::new()
        .route(
            "/matches/faceit_match_100/stats",
            get(|| async {
                Json(json!({
                    "rounds": [{ "round_stats": { "Score": "13-11" } }]
                }))
            }),
        )
        .route(
            "/matches/faceit_match_100",
            get(|| async {
                Json(json!({
                    "demo_url": ["https://demos.faceit.com/100.dem"],
                    "faceit_url": "https://www.faceit.com/en/cs2/room/faceit_match_100"
                }))
            }),
        );
    let (addr, _server) = serve_mock(mock_faceit).await;
    std::env::set_var("FACEIT_API_BASE_URL", format!("http://{addr}"));

    let harness = TestHarness::new();
    // Partida mixton guardada en Firestore
    harness.docs.put(
        "faceit_active_matches",
        "mixton_999",
        json!({
            "demo_url": "https://storage.example.com/mixton_999.dem"
        }),
    );

    let r = harness.router();
    let g_token = global_admin_token();

    // 1. Stats de mixton -> success: false, stats: null
    let (s, _, b_stats_m) = get_req(
        r.clone(),
        "/api/admin/matches/mixton_999/stats",
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b_stats_m["success"], false);
    assert!(b_stats_m["stats"].is_null());

    // 2. Stats de FACEIT -> success: true, stats: {...}
    let (s, _, b_stats_f) = get_req(
        r.clone(),
        "/api/admin/matches/faceit_match_100/stats",
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b_stats_f["success"], true);
    assert!(b_stats_f["stats"]["rounds"].is_array());

    // 3. Demo de mixton -> lee de Firestore
    let (s, _, b_demo_m) = get_req(
        r.clone(),
        "/api/admin/matches/mixton_999/demo",
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b_demo_m["demo_url"],
        "https://storage.example.com/mixton_999.dem"
    );

    // 4. Demo de FACEIT -> consulta API FACEIT
    let (s, _, b_demo_f) = get_req(
        r.clone(),
        "/api/admin/matches/faceit_match_100/demo",
        Some(&g_token),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b_demo_f["demo_url"], "https://demos.faceit.com/100.dem");
    assert_eq!(
        b_demo_f["faceit_url"],
        "https://www.faceit.com/en/cs2/room/faceit_match_100"
    );

    std::env::remove_var("FACEIT_API_BASE_URL");
}
