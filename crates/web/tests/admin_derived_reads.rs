//! Lecturas de administración derivadas (Tarea 8): `GET /api/admin/{stats, users/backup,
//! yape/transactions}` sobre el origen en memoria de `data`, más el montaje (lo no migrado
//! sigue en Node) y la invalidación de cachés por las escrituras de Node que pasan por el proxy.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    routing::any,
    Router,
};
use common::config::Config;
use data::{
    DataError, FakeLogsRepo, FakeMatchHistoryRepo, FakeMessagesRepo, FakeOrdersRepo,
    FakeSupportCallsRepo, FirestoreAdminDocsRepo, GlobalSettings, InMemoryClaimsRepo,
    InMemoryGlobalSettingsRepo, InMemoryNodeDocs,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;
use web::routes::admin::{router, AdminCaches, AdminRepos, AdminState};
use web::session::{new_claims, sign, SessionClaims, SessionState};
use web::{build_app, AppDeps};

const SECRET: &str = "admin-derived-test-secret";
const JSON_CT: &str = "application/json; charset=utf-8";

const USERS: &str = "faceit_users";
const MATCHES: &str = "faceit_active_matches";
const YAPE_T: &str = "yape_transacciones";
const YAPE_O: &str = "yape_operations";
const YAPE_A: &str = "yape_audit_logs";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims(role: &str, servidor: Option<&str>) -> SessionClaims {
    new_claims(
        "42".into(),
        format!("u-{role}"),
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

fn global() -> String {
    token(&claims("global_admin", None))
}

struct Fakes {
    docs: Arc<InMemoryNodeDocs>,
    settings: Arc<InMemoryGlobalSettingsRepo>,
    repos: AdminRepos,
}

impl Fakes {
    fn new() -> Self {
        let docs = Arc::new(InMemoryNodeDocs::new());
        let settings = Arc::new(InMemoryGlobalSettingsRepo::new());
        let repos = AdminRepos {
            claims: Arc::new(InMemoryClaimsRepo::default()),
            orders: Arc::new(FakeOrdersRepo::default()),
            logs: Arc::new(FakeLogsRepo::default()),
            support_calls: Arc::new(FakeSupportCallsRepo::default()),
            match_history: Arc::new(FakeMatchHistoryRepo::default()),
            messages: Arc::new(FakeMessagesRepo::default()),
            global_settings: settings.clone(),
            docs: Arc::new(FirestoreAdminDocsRepo::new(docs.clone())),
            caches: AdminCaches::default(),
            watchers: Vec::new(),
        };
        Fakes {
            docs,
            settings,
            repos,
        }
    }

    /// Usuarios, partidas activas, ajustes y Yape de ejemplo.
    fn seeded() -> Self {
        let f = Fakes::new();
        f.docs.put(
            USERS,
            "g1_1",
            json!({"guild_id": "g1", "servidor": "g1", "faceit_nickname": "a",
                "linkedAt": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 0}}),
        );
        f.docs
            .put(USERS, "2", json!({"servidor": "g1", "discordId": "2"}));
        f.docs.put(USERS, "3", json!({"servidor": "g2"}));
        // Solo lo cuenta el respaldo del legacy (`servidor || mixton_guild_id`).
        f.docs
            .put(USERS, "4", json!({"servidor": "", "mixton_guild_id": "g1"}));
        f.docs.put(MATCHES, "m1", json!({"guild_id": "g1"}));
        f.docs.put(MATCHES, "m2", json!({"guild_id": "g2"}));
        f.docs.put(MATCHES, "mixton_3", json!({"guild_id": "g1"}));
        f.settings.set(Some(
            serde_json::from_value::<GlobalSettings>(json!({
                "autoWelcomeDm": false,
                "autoRoleSync": true,
                "autoVoiceChannels": true,
                "matchAnnouncements": false,
                "liveMatchPanel": true,
                "adminRoleId": "secreto-no-sale",
            }))
            .unwrap(),
        ));
        f.docs.put(
            YAPE_T,
            "T1",
            json!({"monto": 10, "detectedAt": "2026-11-02T10:00:00Z"}),
        );
        f.docs.put(
            YAPE_T,
            "T2",
            json!({"monto": 20, "detectedAt": "2026-11-02T11:00:00Z"}),
        );
        f.docs.put(
            YAPE_O,
            "OP1",
            json!({"status": "pending", "createdAt": {"_seconds": 100, "_nanoseconds": 0}}),
        );
        f.docs.put(
            YAPE_A,
            "A1",
            json!({"event": "credited", "createdAt": {"_seconds": 100, "_nanoseconds": 0}}),
        );
        f
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

async fn get_with(
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

async fn get(r: &Router, path: &str, cookie: &str) -> (StatusCode, Value) {
    let (s, ct, b) = get_with(r.clone(), path, Some(cookie), &[]).await;
    assert_eq!(ct, JSON_CT, "{path}");
    (s, b)
}

fn level_distribution() -> Value {
    json!({"1": 0, "2": 0, "3": 0, "4": 0, "5": 0, "6": 0, "7": 0, "8": 0, "9": 0, "10": 0})
}

const ALL: [&str; 3] = [
    "/api/admin/stats",
    "/api/admin/users/backup",
    "/api/admin/yape/transactions",
];

// ------------------------------------------------------------------- /stats

#[tokio::test]
async fn stats_counts_match_fixture() {
    let f = Fakes::seeded();
    let r = f.router();
    let (s, b) = get(&r, "/api/admin/stats", &global()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({
            "totalUsers": 4,
            "totalMatches": 3,
            "levelDistribution": level_distribution(),
            // `autoMovePlayers` no está en el documento: `undefined`, no sale.
            "features": {
                "autoWelcomeDm": false,
                "autoRoleSync": true,
                "autoVoiceChannels": true,
                "matchAnnouncements": false,
                "liveMatchPanel": true,
            },
        })
    );
    assert!(!b.to_string().contains("secreto-no-sale"));
    assert_eq!(
        f.docs.lists_of(USERS),
        0,
        "recuento nativo, sin leer documentos"
    );

    // Servidor objetivo: `where('servidor' | 'guild_id', '==', g).count()`.
    let admin = token(&claims("admin", Some("g1")));
    let (_, b) = get(&r, "/api/admin/stats", &admin).await;
    assert_eq!(
        (b["totalUsers"].clone(), b["totalMatches"].clone()),
        (json!(2), json!(2))
    );
    let (_, b) = get(&r, "/api/admin/stats?guildId=g2", &global()).await;
    assert_eq!(
        (b["totalUsers"].clone(), b["totalMatches"].clone()),
        (json!(1), json!(1))
    );
    // `guildId=` vacío: global, sin caer al `servidor` del admin.
    let (_, b) = get(&r, "/api/admin/stats?guildId=", &admin).await;
    assert_eq!(b["totalUsers"], json!(4));
}

#[tokio::test]
async fn stats_without_settings_document_uses_legacy_defaults() {
    let f = Fakes::new();
    let (s, b) = get(&f.router(), "/api/admin/stats", &global()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({
            "totalUsers": 0,
            "totalMatches": 0,
            "levelDistribution": level_distribution(),
            "features": {
                "autoWelcomeDm": true, "autoRoleSync": true, "autoVoiceChannels": true,
                "autoMovePlayers": true, "matchAnnouncements": true, "liveMatchPanel": true,
            },
        })
    );
}

/// Si falla el recuento nativo, el legacy lee las dos colecciones y cuenta con
/// `(servidor || mixton_guild_id) === g`; si eso también falla, 500.
#[tokio::test]
async fn stats_repo_error_policy_follows_the_legacy() {
    let f = Fakes::seeded();
    f.docs
        .fail_counts(Some(DataError::Firestore("count no soportado".into())));
    let (s, b) = get(&f.router(), "/api/admin/stats?guildId=g1", &global()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (b["totalUsers"].clone(), b["totalMatches"].clone()),
        (json!(3), json!(2))
    );

    let f = Fakes::seeded();
    f.docs.fail_counts(Some(DataError::QuotaExceeded));
    f.docs.fail_lists(Some(DataError::QuotaExceeded));
    let (s, b) = get(&f.router(), "/api/admin/stats", &global()).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(b, json!({"error": "Error al obtener estadísticas"}));
}

#[tokio::test(start_paused = true)]
async fn stats_second_call_within_ttl_hits_cache() {
    let f = Fakes::seeded();
    let r = f.router();
    let t = global();
    let first = get(&r, "/api/admin/stats", &t).await.1;
    let counts = f.docs.counts();
    assert_eq!(counts, 2);
    f.docs.put(USERS, "nuevo", json!({}));
    tokio::time::advance(Duration::from_millis(59_999)).await;
    assert_eq!(get(&r, "/api/admin/stats", &t).await.1, first);
    assert_eq!(f.docs.counts(), counts);
    // `guildId=global` choca con la clave global, como en el legacy.
    assert_eq!(
        get(&r, "/api/admin/stats?guildId=global", &t).await.1,
        first
    );
    // Otro servidor, otra clave.
    get(&r, "/api/admin/stats?guildId=g1", &t).await;
    assert_eq!(f.docs.counts(), counts + 2);
    tokio::time::advance(Duration::from_millis(1)).await;
    let (_, fresh) = get(&r, "/api/admin/stats", &t).await;
    assert_eq!(fresh["totalUsers"], json!(5));
}

// ------------------------------------------------------------ /users/backup

/// El legacy solo exige `requireAuth` (cualquier administrador, no solo
/// `global_admin`); el servidor objetivo filtra.
#[tokio::test]
async fn users_backup_requires_admin_session_and_filters_by_target_guild() {
    let f = Fakes::seeded();
    let r = f.router();
    let (s, b) = get(&r, "/api/admin/users/backup", &global()).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = b
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["discordId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["2", "3", "4", "g1_1"]);
    assert_eq!(
        b[3],
        json!({"discordId": "g1_1", "guild_id": "g1", "servidor": "g1", "faceit_nickname": "a",
            "linkedAt": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 0}})
    );

    let admin = token(&claims("admin", Some("g1")));
    let (s, b) = get(&r, "/api/admin/users/backup", &admin).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = b
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["discordId"].as_str().unwrap())
        .collect();
    // `guild_id || servidor || mixton_guild_id`
    assert_eq!(ids, ["2", "4", "g1_1"]);

    let user = token(&claims("usuario", None));
    let (s, _, b) = get_with(r.clone(), "/api/admin/users/backup", Some(&user), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(
        b,
        json!({"error": "No autorizado. Se requieren permisos de administrador."})
    );
}

#[tokio::test]
async fn users_backup_read_error_is_legacy_500() {
    let f = Fakes::seeded();
    f.docs.fail_lists(Some(DataError::QuotaExceeded));
    let (s, b) = get(&f.router(), "/api/admin/users/backup", &global()).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(b, json!({"error": "Error al generar backup"}));
}

// ---------------------------------------------------------- /yape/transactions

#[tokio::test]
async fn yape_transactions_have_the_legacy_shape() {
    let f = Fakes::seeded();
    let (s, b) = get(&f.router(), "/api/admin/yape/transactions", &global()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({
            "transactions": [
                {"id": "T2", "monto": 20, "detectedAt": "2026-11-02T11:00:00Z"},
                {"id": "T1", "monto": 10, "detectedAt": "2026-11-02T10:00:00Z"},
            ],
            "operations": [
                {"id": "OP1", "status": "pending", "createdAt": {"_seconds": 100, "_nanoseconds": 0}},
            ],
            "auditLogs": [
                {"id": "A1", "event": "credited", "createdAt": {"_seconds": 100, "_nanoseconds": 0}},
            ],
        })
    );
}

/// Varias peticiones mientras dura una lectura comparten esa lectura
/// (`yapeTransactionsFetchPromise`): una sola lectura de cada colección.
#[tokio::test]
async fn yape_transactions_deduplicates_concurrent_requests() {
    let f = Fakes::seeded();
    let r = f.router();
    let t = global();
    let gate = f.docs.hold().await;
    let mut pending = Vec::new();
    for _ in 0..5 {
        let r = r.clone();
        let t = t.clone();
        pending.push(tokio::spawn(async move {
            get_with(r, "/api/admin/yape/transactions", Some(&t), &[]).await
        }));
    }
    // Hasta que la lectura (retenida) haya empezado.
    while f.docs.lists_of(YAPE_T) == 0 {
        tokio::task::yield_now().await;
    }
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(pending.iter().all(|h| !h.is_finished()), "todas esperan");
    drop(gate);
    let mut bodies = Vec::new();
    for h in pending {
        let (s, _, b) = h.await.unwrap();
        assert_eq!(s, StatusCode::OK);
        bodies.push(b);
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(bodies[0]["transactions"].as_array().unwrap().len(), 2);
    for c in [YAPE_T, YAPE_O, YAPE_A] {
        assert_eq!(f.docs.lists_of(c), 1, "{c}");
    }
}

#[tokio::test(start_paused = true)]
async fn yape_cache_lasts_30_seconds_and_keeps_error_results() {
    let f = Fakes::seeded();
    let r = f.router();
    let t = global();
    // Las lecturas de `database.js` tragan el error: `[]` y se cachea.
    f.docs.fail_lists(Some(DataError::QuotaExceeded));
    let (s, b) = get(&r, "/api/admin/yape/transactions", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({"transactions": [], "operations": [], "auditLogs": []})
    );
    assert!(!b.to_string().to_lowercase().contains("quota"));
    f.docs.fail_lists(None);
    tokio::time::advance(Duration::from_millis(29_999)).await;
    let (_, b) = get(&r, "/api/admin/yape/transactions", &t).await;
    assert_eq!(b["transactions"], json!([]));
    assert_eq!(f.docs.lists_of(YAPE_T), 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    let (_, b) = get(&r, "/api/admin/yape/transactions", &t).await;
    assert_eq!(b["transactions"].as_array().unwrap().len(), 2);
    assert_eq!(f.docs.lists_of(YAPE_T), 2);
}

// ---------------------------------------------------------------- sesión

#[tokio::test]
async fn unauthorized_requests_get_legacy_401_and_403() {
    let f = Fakes::seeded();
    let user = token(&claims("usuario", None));
    for path in ALL {
        let (s, ct, b) = get_with(f.router(), path, None, &[]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(ct, JSON_CT);
        assert_eq!(b, json!({"error": "No autenticado"}));
        let (s, _, b) = get_with(f.router(), path, Some("basura"), &[]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(b, json!({"error": "Token inválido o expirado"}));
        let (s, _, b) = get_with(f.router(), path, Some(&user), &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(
            b,
            json!({"error": "No autorizado. Se requieren permisos de administrador."})
        );
    }
    assert_eq!(f.docs.counts(), 0);
    assert_eq!(f.docs.lists_of(USERS) + f.docs.lists_of(YAPE_T), 0);
}

#[tokio::test]
async fn errors_do_not_leak_firestore_text() {
    let f = Fakes::seeded();
    f.docs.fail_counts(Some(DataError::QuotaExceeded));
    f.docs
        .fail_lists(Some(DataError::Firestore("projects/secret-project".into())));
    for path in ALL {
        let (_, _, b) = get_with(f.router(), path, Some(&global()), &[]).await;
        let text = b.to_string();
        assert!(!text.contains("secret-project"), "{path}: {text}");
        assert!(!text.to_lowercase().contains("quota"), "{path}: {text}");
        assert!(!text.contains("Firestore"), "{path}: {text}");
    }
}

// --------------------------------------------------------------- montaje

async fn serve(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

fn vars(legacy: SocketAddr, static_dir: &std::path::Path) -> HashMap<String, String> {
    [
        ("ADMIN_JWT_SECRET", SECRET),
        ("BASE_URL", "http://localhost:15633"),
        ("DISCORD_CLIENT_ID", "123"),
        ("DISCORD_CLIENT_SECRET", "x"),
        ("DISCORD_TOKEN", "x"),
        ("DISCORD_GUILD_ID", "456"),
        ("LEGACY_URL", &format!("http://{legacy}")),
        ("STATIC_DIR", static_dir.to_str().unwrap()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

async fn legacy() -> SocketAddr {
    serve(Router::new().fallback(any(|req: Request| async move {
        format!("LEGACY {} {}", req.method(), req.uri().path())
    })))
    .await
}

/// Con Firestore desactivado las tres siguen en Node; con repos, las atiende
/// Rust. `/settings`, `/stats/:nickname` y `/matches/:id/demo` siguen en Node
/// siempre (Rulings de la Tarea 8): por eso no hay 404 propio de la demo.
#[tokio::test]
async fn derived_reads_mount_only_with_firestore_and_the_rest_stays_on_proxy() {
    let legacy = legacy().await;
    let static_dir = tempfile::tempdir().unwrap();
    let fakes = Fakes::seeded();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let t = global();
    for with_firestore in [false, true] {
        let front = serve(build_app(AppDeps {
            config: Config::from_map(&vars(legacy, static_dir.path())).unwrap(),
            claims: None,
            admin: with_firestore.then(|| fakes.repos.clone()),
            cs2: None,
            lobby: None,
            docs: None,
            notifier: Default::default(),
            wallet: None,
        }))
        .await;
        for path in ALL {
            let res = client
                .get(format!("http://{front}{path}"))
                .header(header::COOKIE, format!("auth_token={t}"))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200, "{path}");
            let text = res.text().await.unwrap();
            if with_firestore {
                assert!(
                    serde_json::from_str::<Value>(&text).is_ok(),
                    "{path}: {text}"
                );
            } else {
                assert_eq!(text, format!("LEGACY GET {path}"));
            }
        }
        for (method, path) in [
            ("GET", "/api/admin/settings"),
            ("GET", "/api/admin/stats/someone"),
            ("GET", "/api/admin/matches/mixton_404/demo"),
            ("GET", "/api/admin/matches/abc/demo"),
            ("POST", "/api/admin/users/restore"),
            ("POST", "/api/admin/stats"),
        ] {
            let res = client
                .request(method.parse().unwrap(), format!("http://{front}{path}"))
                .header(header::COOKIE, format!("auth_token={t}"))
                .send()
                .await
                .unwrap();
            assert_eq!(
                res.text().await.unwrap(),
                format!("LEGACY {method} {path}"),
                "{with_firestore} {method} {path}"
            );
        }
    }
}

/// `POST /users/:id/unlink`, `POST /users/verify` y `POST /yape/manual-approve`
/// siguen en Node, que vacía sus cachés; al pasar por el proxy vacían también
/// las de Rust.
#[tokio::test]
async fn legacy_panel_writes_clear_the_rust_caches() {
    let legacy = legacy().await;
    let static_dir = tempfile::tempdir().unwrap();
    let f = Fakes::seeded();
    let front = serve(build_app(AppDeps {
        config: Config::from_map(&vars(legacy, static_dir.path())).unwrap(),
        claims: None,
        admin: Some(f.repos.clone()),
        cs2: None,
        lobby: None,
        docs: None,
        notifier: Default::default(),
        wallet: None,
    }))
    .await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let t = global();
    let call = |method: &'static str, path: &'static str| {
        let client = client.clone();
        let t = t.clone();
        async move {
            client
                .request(method.parse().unwrap(), format!("http://{front}{path}"))
                .header(header::COOKIE, format!("auth_token={t}"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        }
    };

    call("GET", "/api/admin/yape/transactions").await;
    call("GET", "/api/admin/yape/transactions").await;
    assert_eq!(f.docs.lists_of(YAPE_T), 1, "cacheado");
    assert_eq!(
        call("POST", "/api/admin/yape/manual-approve").await,
        "LEGACY POST /api/admin/yape/manual-approve"
    );
    call("GET", "/api/admin/yape/transactions").await;
    assert_eq!(f.docs.lists_of(YAPE_T), 2, "invalidado por Node");

    call("GET", "/api/admin/stats").await;
    call("GET", "/api/admin/stats?guildId=g1").await;
    let counts = f.docs.counts();
    call("GET", "/api/admin/stats").await;
    assert_eq!(f.docs.counts(), counts, "cacheado");
    // `statsCache.clear()`: todas las claves (global y por servidor).
    call("POST", "/api/admin/users/123/unlink").await;
    call("GET", "/api/admin/stats").await;
    call("GET", "/api/admin/stats?guildId=g1").await;
    assert_eq!(f.docs.counts(), counts + 4);
    let before = f.docs.counts();
    call("POST", "/api/admin/users/verify").await;
    call("GET", "/api/admin/stats").await;
    assert_eq!(f.docs.counts(), before + 2);
    // Una lectura no invalida.
    call("GET", "/api/admin/users/backup").await;
    call("GET", "/api/admin/stats").await;
    assert_eq!(f.docs.counts(), before + 2);
}
