//! Pruebas de integración para las rutas de escritura de administración (Tanda 3):
//! - POST `/api/admin/features/:key`
//! - POST `/api/admin/features-batch`
//! - POST `/api/admin/users/restore`
//! - DELETE `/api/admin/matches/history/:id`

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
    FakeSupportCallsRepo, FakeUsersRepo, FirestoreAdminDocsRepo, InMemoryClaimsRepo,
    InMemoryGlobalSettingsRepo, InMemoryGuildSettingsRepo, InMemoryNodeDocs,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::routes::admin::{router, AdminCaches, AdminRepos, AdminState};
use web::session::{new_claims, sign, SessionClaims, SessionState};

const SECRET: &str = "admin-writes-test-secret";
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

#[allow(dead_code)]
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
            users: Arc::new(FakeUsersRepo::new()),
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

async fn delete_req(
    router: Router,
    path: &str,
    cookie: Option<&str>,
) -> (StatusCode, String, Value) {
    let mut req = axum::http::Request::builder()
        .method("DELETE")
        .uri(path);
    if let Some(t) = cookie {
        req = req.header(header::COOKIE, format!("auth_token={t}"));
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

// ------------------------------------------------------------------- tests

#[tokio::test]
async fn unauthenticated_returns_401() {
    let harness = TestHarness::new();
    let r = harness.router();

    let post_paths = [
        "/api/admin/features/musicBot",
        "/api/admin/features-batch",
        "/api/admin/users/restore",
    ];

    for path in post_paths {
        let (status, ct, body) = post_req(r.clone(), path, None, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(ct, JSON_CT, "{path}");
        assert_eq!(body["error"], "No autenticado", "{path}");
    }

    let (status, ct, body) =
        delete_req(r.clone(), "/api/admin/matches/history/match_123", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "No autenticado");
}

#[tokio::test]
async fn invalid_token_returns_401() {
    let harness = TestHarness::new();
    let r = harness.router();

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/musicBot",
        Some("invalid-token"),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Token inválido o expirado");
}

#[tokio::test]
async fn forbidden_for_non_admins() {
    let harness = TestHarness::new();
    let r = harness.router();
    let u_token = user_token();

    let post_paths = [
        "/api/admin/features/musicBot",
        "/api/admin/features-batch",
        "/api/admin/users/restore",
    ];

    for path in post_paths {
        let (status, ct, body) = post_req(r.clone(), path, Some(&u_token), json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(ct, JSON_CT, "{path}");
        assert_eq!(
            body["error"],
            "No autorizado. Se requieren permisos de administrador.",
            "{path}"
        );
    }

    let (status, ct, body) = delete_req(
        r.clone(),
        "/api/admin/matches/history/match_123",
        Some(&u_token),
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
async fn features_global_requires_global_admin() {
    let harness = TestHarness::new();
    let r = harness.router();
    let a_token = admin_token(None);

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/musicBot",
        Some(&a_token),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(
        body["error"],
        "Solo el administrador global puede modificar la configuración global."
    );

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features-batch",
        Some(&a_token),
        json!({"updates": {"musicBot": true}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(
        body["error"],
        "Solo el administrador global puede modificar la configuración global."
    );
}

#[tokio::test]
async fn features_toggle_global_happy_path() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    // Activar musicBot global
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/musicBot",
        Some(&g_token),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["key"], "musicBot");
    assert_eq!(body["enabled"], true);

    let saved = harness
        .doc_sink
        .get("faceit_config", "settings")
        .expect("guardado en faceit_config/settings");
    assert_eq!(saved["musicBot"], true);
    assert!(saved["updatedAt"].is_object());

    // Desactivar musicBot global
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/musicBot",
        Some(&g_token),
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["key"], "musicBot");
    assert_eq!(body["enabled"], false);

    let saved = harness
        .doc_sink
        .get("faceit_config", "settings")
        .expect("guardado en faceit_config/settings");
    assert_eq!(saved["musicBot"], false);
}

#[tokio::test]
async fn features_toggle_per_guild_happy_path() {
    let harness = TestHarness::new();
    let r = harness.router();
    let a_token = admin_token(Some("guild_alpha"));

    // Admin de servidor modifica feature flag de su servidor (resuelto por claims)
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/musicBot",
        Some(&a_token),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["key"], "musicBot");
    assert_eq!(body["enabled"], true);

    let saved = harness
        .doc_sink
        .get("guild_settings", "guild_alpha")
        .expect("guardado en guild_settings/guild_alpha");
    assert_eq!(saved["musicBot"], true);
    assert!(saved["updatedAt"].is_object());

    // Global admin modifica feature flag por query param
    let g_token = global_admin_token();
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/autoWelcomeDm?guildId=guild_beta",
        Some(&g_token),
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["key"], "autoWelcomeDm");
    assert_eq!(body["enabled"], false);

    let saved = harness
        .doc_sink
        .get("guild_settings", "guild_beta")
        .expect("guardado en guild_settings/guild_beta");
    assert_eq!(saved["autoWelcomeDm"], false);
}

#[tokio::test]
async fn features_batch_validation_error() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let invalid_bodies = [
        json!({}),
        json!({"updates": null}),
        json!({"updates": "not_an_object"}),
        json!({"updates": 123}),
    ];

    for body in invalid_bodies {
        let (status, ct, resp) = post_req(
            r.clone(),
            "/api/admin/features-batch",
            Some(&g_token),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(ct, JSON_CT);
        assert_eq!(
            resp["error"],
            "El cuerpo de la petición debe contener un objeto \"updates\" con los pares servicio/estado."
        );
    }
}

#[tokio::test]
async fn features_batch_global_and_guild_happy_path() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    // Actualización batch global
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features-batch",
        Some(&g_token),
        json!({
            "updates": {
                "musicBot": true,
                "autoWelcomeDm": false,
                "command_say": true
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);

    let saved = harness
        .doc_sink
        .get("faceit_config", "settings")
        .expect("guardado global");
    assert_eq!(saved["musicBot"], true);
    assert_eq!(saved["autoWelcomeDm"], false);
    assert_eq!(saved["command_say"], true);
    assert!(saved["updatedAt"].is_object());

    // Actualización batch por servidor
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features-batch?guildId=guild_gamma",
        Some(&g_token),
        json!({
            "updates": {
                "autoMovePlayers": false
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);

    let saved = harness
        .doc_sink
        .get("guild_settings", "guild_gamma")
        .expect("guardado por servidor");
    assert_eq!(saved["autoMovePlayers"], false);
    assert!(saved["updatedAt"].is_object());
}

#[tokio::test]
async fn users_restore_validation_error() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let invalid_bodies = [
        json!({}),
        json!({"users": null}),
        json!({"users": "not_an_array"}),
        json!({"users": 123}),
    ];

    for body in invalid_bodies {
        let (status, ct, resp) = post_req(
            r.clone(),
            "/api/admin/users/restore",
            Some(&g_token),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(ct, JSON_CT);
        assert_eq!(
            resp["error"],
            "El formato de datos de backup no es válido"
        );
    }
}

#[tokio::test]
async fn users_restore_global_happy_path() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let users = json!({
        "users": [
            {
                "discordId": "111222333",
                "faceit_nickname": "PlayerPro",
                "faceit_level": 10,
                "linkedAt": {
                    "_seconds": 1700000000,
                    "_nanoseconds": 0
                }
            },
            {
                // Sin discordId, debe ignorarse sin error
                "faceit_nickname": "MissingId"
            }
        ]
    });

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/users/restore",
        Some(&g_token),
        users,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(
        body["message"],
        "Backup importado con éxito: 1 vinculaciones restauradas."
    );

    let saved = harness
        .doc_sink
        .get("faceit_users", "111222333")
        .expect("usuario restaurado en faceit_users");
    assert_eq!(saved["faceit_nickname"], "PlayerPro");
    assert_eq!(saved["faceit_level"], 10);
    assert!(saved.get("discordId").is_none());
}

#[tokio::test]
async fn users_restore_guild_happy_path() {
    let harness = TestHarness::new();
    let r = harness.router();
    let a_token = admin_token(Some("guild_target_1"));

    let users = json!({
        "users": [
            {
                "discordId": "guild_old_999888",
                "faceit_nickname": "GuildHero",
                "servidor": "guild_old",
                "mixton_guild_id": "guild_old",
                "linkedAt": "2026-01-01T12:00:00Z"
            }
        ]
    });

    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/users/restore",
        Some(&a_token),
        users,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(
        body["message"],
        "Backup importado con éxito: 1 vinculaciones restauradas."
    );

    // Debe extraer el raw discordId y prefijar con la guild
    let saved = harness
        .doc_sink
        .get("faceit_users", "guild_target_1_999888")
        .expect("usuario restaurado en faceit_users con prefijo de guild");
    assert_eq!(saved["faceit_nickname"], "GuildHero");
    assert_eq!(saved["guild_id"], "guild_target_1");
    assert_eq!(saved["servidor"], "guild_target_1");
    assert_eq!(saved["mixton_guild_id"], "guild_target_1");
}

#[tokio::test]
async fn delete_match_history_happy_path() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    // Precargar partida en fake doc_sink
    harness.doc_sink.insert(
        "faceit_matches_history",
        "match_del_01",
        json!({"id": "match_del_01", "status": "finished"}),
    );
    assert!(harness
        .doc_sink
        .get("faceit_matches_history", "match_del_01")
        .is_some());

    let (status, ct, body) = delete_req(
        r.clone(),
        "/api/admin/matches/history/match_del_01",
        Some(&g_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["success"], true);
    assert_eq!(body["message"], "Partida eliminada del historial con éxito");

    // Verificar que se eliminó de FakeDocSink
    assert!(harness
        .doc_sink
        .get("faceit_matches_history", "match_del_01")
        .is_none());
}

#[tokio::test]
async fn delete_match_history_empty_id_returns_404() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    let (status, ct, body) = delete_req(
        r.clone(),
        "/api/admin/matches/history/%20",
        Some(&g_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ct, JSON_CT);
    assert_eq!(
        body["error"],
        "No se encontró la partida en el historial o no pudo eliminarse"
    );
}

#[tokio::test]
async fn doc_sink_error_handling() {
    let harness = TestHarness::new();
    let r = harness.router();
    let g_token = global_admin_token();

    // Simular error de Firestore en doc_sink
    harness
        .doc_sink
        .set_error(Some(DataError::Firestore("Disk failure".into())));

    // DELETE /matches/history/:id
    let (status, ct, body) = delete_req(
        r.clone(),
        "/api/admin/matches/history/match_fail",
        Some(&g_token),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Error al eliminar la partida del historial");

    // POST /features/:key
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features/musicBot",
        Some(&g_token),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Error al actualizar feature flag");

    // POST /features-batch
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/features-batch",
        Some(&g_token),
        json!({"updates": {"musicBot": true}}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Error al actualizar servicios por lote");

    // POST /users/restore
    let (status, ct, body) = post_req(
        r.clone(),
        "/api/admin/users/restore",
        Some(&g_token),
        json!({"users": [{"discordId": "111"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(ct, JSON_CT);
    assert_eq!(body["error"], "Error al restaurar backup");
}
