//! Lecturas de administración de una sola colección (Tarea 7):
//! `GET /api/admin/{claims, claims/:code, orders, logs, support-calls, messages,
//! matches/history}` frente a los dobles de `data`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    routing::any,
    Router,
};
use common::config::Config;
use data::{
    default_message_templates, DataError, FakeDocSink, FakeLogsRepo, FakeMatchHistoryRepo,
    FakeMessagesRepo, FakeOrdersRepo, FakeSupportCallsRepo, FirestoreAdminDocsRepo,
    InMemoryClaimsRepo, InMemoryGlobalSettingsRepo, InMemoryGuildSettingsRepo, InMemoryNodeDocs,
    LogEntry, LogsRepo,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;
use web::routes::admin::{router, AdminCaches, AdminRepos, AdminState};
use web::session::{new_claims, sign, SessionClaims, SessionState};
use web::{build_app, AppDeps};

const SECRET: &str = "admin-reads-test-secret";
const JSON_CT: &str = "application/json; charset=utf-8";
const QUOTA_TEXT: &str = "Quota exceeded";

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

/// `LogsRepo` que cuenta las lecturas.
#[derive(Default)]
struct CountingLogs {
    inner: FakeLogsRepo,
    calls: AtomicUsize,
    seen: std::sync::Mutex<Vec<(u32, Option<String>)>>,
}

#[async_trait]
impl LogsRepo for CountingLogs {
    async fn recent(&self, limit: u32, guild: Option<&str>) -> Result<Vec<LogEntry>, DataError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push((limit, guild.map(str::to_owned)));
        self.inner.recent(limit, guild).await
    }
}

struct Fakes {
    claims: Arc<InMemoryClaimsRepo>,
    orders: Arc<FakeOrdersRepo>,
    logs: Arc<CountingLogs>,
    calls: Arc<FakeSupportCallsRepo>,
    history: Arc<FakeMatchHistoryRepo>,
    messages: Arc<FakeMessagesRepo>,
}

impl Fakes {
    fn new() -> Self {
        Fakes {
            claims: Arc::default(),
            orders: Arc::default(),
            logs: Arc::default(),
            calls: Arc::default(),
            history: Arc::default(),
            messages: Arc::default(),
        }
    }

    fn repos(&self) -> AdminRepos {
        let vars: HashMap<String, String> = [
            ("ADMIN_JWT_SECRET", SECRET),
            ("BASE_URL", "http://localhost:15633"),
            ("DISCORD_CLIENT_ID", "123"),
            ("DISCORD_CLIENT_SECRET", "test-secret"),
            ("DISCORD_TOKEN", "test-token"),
            ("DISCORD_GUILD_ID", "456"),
            ("LEGACY_URL", "http://localhost:3000"),
            ("STATIC_DIR", "."),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let cfg = Arc::new(Config::from_map(&vars).unwrap());

        AdminRepos {
            claims: self.claims.clone(),
            orders: self.orders.clone(),
            logs: self.logs.clone(),
            support_calls: self.calls.clone(),
            match_history: self.history.clone(),
            messages: self.messages.clone(),
            global_settings: Arc::new(InMemoryGlobalSettingsRepo::new()),
            guild_settings: Arc::new(InMemoryGuildSettingsRepo::new()),
            docs: Arc::new(FirestoreAdminDocsRepo::new(Arc::new(
                InMemoryNodeDocs::new(),
            ))),
            doc_sink: Arc::new(FakeDocSink::new()),
            users: Arc::new(data::FakeUsersRepo::new()),
            config: cfg,
            firestore: None,
            caches: AdminCaches::default(),
            watchers: Vec::new(),
        }
    }

    fn router(&self) -> Router {
        router(
            AdminState {
                session: SessionState::new(SECRET.as_bytes(), false).unwrap(),
            },
            Some(self.repos()),
        )
    }

    fn fail_all(&self, err: DataError) {
        self.claims.fail_with(Some(err.clone()));
        self.orders.fail_with(Some(err.clone()));
        self.logs.inner.fail_with(Some(err.clone()));
        self.calls.fail_with(Some(err.clone()));
        self.history.fail_with(Some(err.clone()));
        self.messages.fail_with(Some(err));
    }

    /// Datos de ejemplo en todas las colecciones.
    fn seeded() -> Self {
        let f = Fakes::new();
        for (code, status, iso) in [
            ("LR-2026-000001", "PENDIENTE", "2026-11-02T10:00:00.000Z"),
            ("LR-2026-000002", "ATENDIDO", "2026-11-02T11:00:00.000Z"),
            ("LR-2026-000003", "PENDIENTE", "2026-11-02T12:00:00.000Z"),
        ] {
            f.claims.insert(
                code,
                serde_json::from_value(
                    json!({"claimCode": code, "status": status, "createdAtIso": iso}),
                )
                .unwrap(),
            );
        }
        for (id, status, iso) in [
            ("ORD-1", "COMPLETADA", "2026-11-02T10:00:00.000Z"),
            ("ORD-2", "PENDIENTE", "2026-11-02T11:00:00.000Z"),
            ("ORD-3", "COMPLETADA", "2026-11-02T12:00:00.000Z"),
        ] {
            f.orders.push(
                serde_json::from_value(
                    json!({"id": id, "orderId": id, "status": status, "createdAtIso": iso}),
                )
                .unwrap(),
            );
        }
        for (id, created, guild) in [
            ("l1", "2026-11-02T10:00:00Z", "g1"),
            ("l2", "2026-11-02T10:00:01Z", "g2"),
            ("l3", "2026-11-02T10:00:02Z", "g1"),
        ] {
            f.logs.inner.push(
                serde_json::from_value(json!({"id": id, "level": "info", "message": "m",
                    "source": "bot", "guild_id": guild, "createdAt": created}))
                .unwrap(),
            );
        }
        for (id, created, guild) in [
            ("SUP-1", "2026-11-02T10:00:00Z", "g1"),
            ("SUP-2", "2026-11-02T10:00:01Z", "g2"),
        ] {
            f.calls.push(
                serde_json::from_value(json!({"id": id, "status": "pendiente", "claimedBy": null,
                    "guild_id": guild, "createdAt": created}))
                .unwrap(),
            );
        }
        for (id, created, guild) in [
            ("m1", "2026-11-02T10:00:00Z", "g1"),
            ("mixton_2", "2026-11-02T11:00:00Z", "g2"),
            ("m3", "2026-11-02T12:00:00Z", "g1"),
        ] {
            f.history.push(
                serde_json::from_value(json!({"id": id, "guild_id": guild, "createdAt": created}))
                    .unwrap(),
            );
        }
        f
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

async fn get(f: &Fakes, path: &str, cookie: &str) -> (StatusCode, Value) {
    let (s, ct, b) = get_with(f.router(), path, Some(cookie), &[]).await;
    if s != StatusCode::BAD_REQUEST || ct.starts_with("application/json") {
        assert_eq!(ct, JSON_CT, "{path}");
    }
    (s, b)
}

fn ids(v: &Value, key: &str, field: &str) -> Vec<String> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} no es un array: {v}"))
        .iter()
        .map(|e| e[field].as_str().unwrap().to_owned())
        .collect()
}

const ALL: [(&str, &str); 7] = [
    ("/api/admin/claims", "claims"),
    ("/api/admin/claims/LR-2026-000001", "claim"),
    ("/api/admin/orders", "orders"),
    ("/api/admin/logs", "logs"),
    ("/api/admin/support-calls", "calls"),
    ("/api/admin/messages", "templates"),
    ("/api/admin/matches/history", "history"),
];

// --------------------------------------------------------------- happy paths

#[tokio::test]
async fn each_route_answers_with_the_legacy_shape() {
    let f = Fakes::seeded();
    let t = global();

    let (s, b) = get(&f, "/api/admin/claims", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        ids(&b, "claims", "id"),
        ["LR-2026-000003", "LR-2026-000002", "LR-2026-000001"]
    );
    assert_eq!(b["claims"][0]["claimCode"], "LR-2026-000003");

    let (s, b) = get(&f, "/api/admin/claims/LR-2026-000002", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({"claim": {"claimCode": "LR-2026-000002", "status": "ATENDIDO",
            "createdAtIso": "2026-11-02T11:00:00.000Z"}})
    );

    let (s, b) = get(&f, "/api/admin/orders", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ids(&b, "orders", "id"), ["ORD-3", "ORD-2", "ORD-1"]);

    let (s, b) = get(&f, "/api/admin/logs", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ids(&b, "logs", "id"), ["l3", "l2", "l1"]);
    assert_eq!(
        b["logs"][0]["createdAt"],
        json!({"_seconds": 1_793_613_602_i64, "_nanoseconds": 0})
    );
    assert_eq!(f.logs.seen.lock().unwrap()[0], (50, None));

    let (s, b) = get(&f, "/api/admin/support-calls", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ids(&b, "calls", "id"), ["SUP-2", "SUP-1"]);
    assert_eq!(b["calls"][0]["claimedBy"], Value::Null);

    let (s, b) = get(&f, "/api/admin/messages", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, json!({"templates": default_message_templates()}));
    f.messages.set(Some(json!({"welcomeTitle": "Hola"})));
    let (_, b) = get(&f, "/api/admin/messages", &t).await;
    assert_eq!(b, json!({"templates": {"welcomeTitle": "Hola"}}));

    let (s, b) = get(&f, "/api/admin/matches/history", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ids(&b, "history", "id"), ["m3", "mixton_2", "m1"]);
}

#[tokio::test]
async fn claims_filters_by_status() {
    let f = Fakes::seeded();
    let t = global();
    let (s, b) = get(&f, "/api/admin/claims?status=PENDIENTE", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        ids(&b, "claims", "id"),
        ["LR-2026-000003", "LR-2026-000001"]
    );
    // `status=` vacío es `null` (`req.query.status || null`): sin filtro.
    let (_, b) = get(&f, "/api/admin/claims?status=", &t).await;
    assert_eq!(ids(&b, "claims", "id").len(), 3);
    let (_, b) = get(&f, "/api/admin/claims?status=NADA", &t).await;
    assert_eq!(b, json!({"claims": []}));
    // Igual en las órdenes.
    let (_, b) = get(&f, "/api/admin/orders?status=COMPLETADA", &t).await;
    assert_eq!(ids(&b, "orders", "id"), ["ORD-3", "ORD-1"]);
    let (_, b) = get(&f, "/api/admin/orders?status=", &t).await;
    assert_eq!(ids(&b, "orders", "id").len(), 3);
    // Un array de `qs` no se reproduce.
    let (s, b) = get(&f, "/api/admin/claims?status=A&status=B", &t).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error": "status inválido"}));
}

#[tokio::test]
async fn claims_and_orders_limit_slices_like_the_legacy() {
    let f = Fakes::seeded();
    let t = global();
    for (q, want) in [
        ("limit=2", vec!["LR-2026-000003", "LR-2026-000002"]),
        (
            "limit=abc",
            vec!["LR-2026-000003", "LR-2026-000002", "LR-2026-000001"],
        ),
        (
            "limit=0",
            vec!["LR-2026-000003", "LR-2026-000002", "LR-2026-000001"],
        ),
        ("limit=1x", vec!["LR-2026-000003"]),
        // `slice(0, -1)` quita el último.
        ("limit=-1", vec!["LR-2026-000003", "LR-2026-000002"]),
        ("limit=-5", vec![]),
        // `parseInt(x, 10)`: `0x2` es 0 -> 100.
        (
            "limit=0x2",
            vec!["LR-2026-000003", "LR-2026-000002", "LR-2026-000001"],
        ),
        (
            "limit=99999999999999999999999",
            vec!["LR-2026-000003", "LR-2026-000002", "LR-2026-000001"],
        ),
    ] {
        let (s, b) = get(&f, &format!("/api/admin/claims?{q}"), &t).await;
        assert_eq!(s, StatusCode::OK, "{q}");
        assert_eq!(ids(&b, "claims", "id"), want, "{q}");
    }
    let (_, b) = get(&f, "/api/admin/orders?limit=-2&status=COMPLETADA", &t).await;
    assert_eq!(b, json!({"orders": []}));
    let (_, b) = get(&f, "/api/admin/orders?limit=-1", &t).await;
    assert_eq!(ids(&b, "orders", "id"), ["ORD-3", "ORD-2"]);
}

#[tokio::test]
async fn logs_and_history_limits_reach_the_repo_like_the_legacy() {
    let f = Fakes::seeded();
    let t = global();
    for (q, want) in [
        ("limit=abc", Some(50)),
        ("limit=0", Some(50)),
        ("limit=7x", Some(7)),
        ("limit=0x10", Some(16)),
        ("limit=%2B3", Some(3)),
        // Firestore rechaza un límite negativo y `getRecentLogs` da `[]`.
        ("limit=-5", None),
    ] {
        let before = f.logs.calls.load(Ordering::SeqCst);
        let (s, b) = get(&f, &format!("/api/admin/logs?{q}"), &t).await;
        assert_eq!(s, StatusCode::OK, "{q}");
        match want {
            Some(n) => assert_eq!(f.logs.seen.lock().unwrap().last().unwrap().0, n, "{q}"),
            None => {
                assert_eq!(f.logs.calls.load(Ordering::SeqCst), before, "{q}");
                assert_eq!(b, json!({"logs": []}));
            }
        }
    }

    for (q, n) in [
        ("", 3),
        ("limit=-5", 1),
        ("limit=1", 1),
        ("limit=2", 2),
        ("limit=abc", 3),
    ] {
        let (_, b) = get(&f, &format!("/api/admin/matches/history?{q}"), &t).await;
        assert_eq!(ids(&b, "history", "id").len(), n, "{q}");
    }
}

// --------------------------------------------------------------- servidor

#[tokio::test]
async fn admin_role_falls_back_to_servidor_for_logs() {
    let f = Fakes::seeded();
    let admin = token(&claims("admin", Some("g1")));
    let (s, b) = get(&f, "/api/admin/logs", &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ids(&b, "logs", "id"), ["l3", "l1"]);
    assert_eq!(
        f.logs.seen.lock().unwrap().last().unwrap(),
        &(50, Some("g1".into()))
    );
    // Igual en las llamadas de soporte y el historial.
    let (_, b) = get(&f, "/api/admin/support-calls", &admin).await;
    assert_eq!(ids(&b, "calls", "id"), ["SUP-1"]);
    let (_, b) = get(&f, "/api/admin/matches/history", &admin).await;
    assert_eq!(ids(&b, "history", "id"), ["m3", "m1"]);
    // Pero claims/orders no filtran por servidor (el legacy no lo hace).
    let (_, b) = get(&f, "/api/admin/claims", &admin).await;
    assert_eq!(ids(&b, "claims", "id").len(), 3);
}

#[tokio::test]
async fn empty_guild_id_query_means_global() {
    let f = Fakes::seeded();
    let admin = token(&claims("admin", Some("g1")));
    // `?guildId=` existe y es falsy: `null`, sin caer al `servidor`.
    let (_, b) = get(&f, "/api/admin/logs?guildId=", &admin).await;
    assert_eq!(ids(&b, "logs", "id"), ["l3", "l2", "l1"]);
    assert_eq!(f.logs.seen.lock().unwrap().last().unwrap(), &(50, None));
    let (_, b) = get(&f, "/api/admin/support-calls?guildId=", &admin).await;
    assert_eq!(ids(&b, "calls", "id").len(), 2);
    let (_, b) = get(&f, "/api/admin/matches/history?guildId=", &admin).await;
    assert_eq!(ids(&b, "history", "id").len(), 3);
    // Un `guildId` explícito gana, también la cabecera.
    let (_, b) = get(&f, "/api/admin/logs?guildId=g2", &admin).await;
    assert_eq!(ids(&b, "logs", "id"), ["l2"]);
    let (_, _, b) = get_with(
        f.router(),
        "/api/admin/support-calls",
        Some(&admin),
        &[("x-guild-id", "g2")],
    )
    .await;
    assert_eq!(ids(&b, "calls", "id"), ["SUP-2"]);
    let (s, b) = get(&f, "/api/admin/logs?guildId=1&guildId=2", &admin).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error": "guildId inválido"}));
}

// ------------------------------------------------------------------ caché

#[tokio::test(start_paused = true)]
async fn logs_second_call_within_ttl_hits_cache() {
    let f = Fakes::seeded();
    let r = f.router();
    let t = global();
    let call = |path: &'static str| {
        let r = r.clone();
        let t = t.clone();
        async move { get_with(r, path, Some(&t), &[]).await.2 }
    };
    let first = call("/api/admin/logs").await;
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 1);

    // Un log nuevo no se ve mientras dura la caché.
    f.logs.inner.push(
        serde_json::from_value(json!({"id": "l4", "guild_id": "g1",
            "createdAt": "2026-11-02T10:00:09Z"}))
        .unwrap(),
    );
    tokio::time::advance(Duration::from_millis(29_999)).await;
    assert_eq!(call("/api/admin/logs").await, first);
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 1);

    // Otra clave (`limit`, servidor) es otra entrada.
    call("/api/admin/logs?limit=2").await;
    call("/api/admin/logs?guildId=g1").await;
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 3);
    // `limit=abc` y `limit=50` comparten la clave `global:50`.
    call("/api/admin/logs?limit=abc").await;
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 3);
    // El servidor literal `global` choca con la clave global, como en el legacy.
    assert_eq!(call("/api/admin/logs?guildId=global").await, first);
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 3);

    // A los 30 s se vuelve a leer.
    tokio::time::advance(Duration::from_millis(1)).await;
    let fresh = call("/api/admin/logs").await;
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 4);
    assert_eq!(ids(&fresh, "logs", "id")[0], "l4");
}

#[tokio::test(start_paused = true)]
async fn logs_error_result_is_cached_like_the_legacy() {
    let f = Fakes::seeded();
    let r = f.router();
    let t = global();
    f.logs.inner.fail_with(Some(DataError::QuotaExceeded));
    let (_, _, b) = get_with(r.clone(), "/api/admin/logs", Some(&t), &[]).await;
    assert_eq!(b, json!({"logs": []}));
    // `getRecentLogs` devolvió `[]` y la ruta lo guardó: 30 s de lista vacía.
    f.logs.inner.fail_with(None);
    let (_, _, b) = get_with(r.clone(), "/api/admin/logs", Some(&t), &[]).await;
    assert_eq!(b, json!({"logs": []}));
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(30)).await;
    let (_, _, b) = get_with(r, "/api/admin/logs", Some(&t), &[]).await;
    assert_eq!(ids(&b, "logs", "id").len(), 3);
}

// ---------------------------------------------------------------- errores

/// El legacy traga los errores de Firestore en `database.js`: el 500 del
/// `catch` de cada ruta no llega por un fallo de lectura.
#[tokio::test]
async fn repo_error_maps_to_legacy_body() {
    let f = Fakes::seeded();
    let t = global();
    // Plantillas leídas con éxito antes del fallo (mismo router: mismo estado).
    let r = f.router();
    f.messages.set(Some(json!({"welcomeTitle": "Antes"})));
    get_with(r.clone(), "/api/admin/messages", Some(&t), &[]).await;
    f.fail_all(DataError::Firestore("FAILED_PRECONDITION: index".into()));
    for (path, want_status, want) in [
        ("/api/admin/claims", 200, json!({"claims": []})),
        (
            "/api/admin/claims/LR-2026-000001",
            404,
            json!({"error": "Reclamación no encontrada"}),
        ),
        ("/api/admin/orders", 200, json!({"orders": []})),
        ("/api/admin/logs", 200, json!({"logs": []})),
        ("/api/admin/support-calls", 200, json!({"calls": []})),
        (
            "/api/admin/messages",
            200,
            json!({"templates": {"welcomeTitle": "Antes"}}),
        ),
        ("/api/admin/matches/history", 200, json!({"history": []})),
    ] {
        let (s, _, b) = get_with(r.clone(), path, Some(&t), &[]).await;
        assert_eq!(s.as_u16(), want_status, "{path}");
        assert_eq!(b, want, "{path}");
    }
    // Sin plantillas previas: las de por defecto.
    let fresh = Fakes::new();
    fresh.fail_all(DataError::QuotaExceeded);
    let (s, b) = get(&fresh, "/api/admin/messages", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, json!({"templates": default_message_templates()}));
}

#[tokio::test]
async fn quota_error_does_not_leak_text() {
    let f = Fakes::seeded();
    f.fail_all(DataError::QuotaExceeded);
    let t = global();
    for (path, _) in ALL {
        let (_, _, b) = get_with(f.router(), path, Some(&t), &[]).await;
        let text = b.to_string();
        assert!(!text.contains(QUOTA_TEXT), "{path}: {text}");
        assert!(!text.to_lowercase().contains("quota"), "{path}: {text}");
        assert!(!text.contains("Firestore"), "{path}: {text}");
    }
}

#[tokio::test]
async fn missing_claim_is_404() {
    let f = Fakes::seeded();
    let t = global();
    let (s, b) = get(&f, "/api/admin/claims/LR-2026-999999", &t).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b, json!({"error": "Reclamación no encontrada"}));
    // Un `/` codificado no puede ser un id de documento: `getClaim` da `null`.
    let (s, _) = get(&f, "/api/admin/claims/a%2Fb", &t).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Decodificado como `decodeURIComponent`.
    f.claims.insert(
        "LR ñ",
        serde_json::from_value(json!({"status": "X"})).unwrap(),
    );
    let (s, b) = get(&f, "/api/admin/claims/LR%20%C3%B1", &t).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, json!({"claim": {"status": "X"}}));
}

#[tokio::test]
async fn malformed_claim_code_is_400_before_auth() {
    let f = Fakes::seeded();
    for path in ["/api/admin/claims/%FF", "/api/admin/claims/%ZZ"] {
        for cookie in [None, Some(global())] {
            let (s, _, _) = get_with(f.router(), path, cookie.as_deref(), &[]).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{path}");
        }
    }
}

// --------------------------------------------------------------- sesión

#[tokio::test]
async fn unauthorized_requests_get_legacy_401_and_403() {
    let f = Fakes::seeded();
    let user = token(&claims("usuario", None));
    for (path, _) in ALL {
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
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 0);
}

/// `isAdmin === true` y `role === 'admin'` del legacy son estrictos: un token
/// bien firmado con `isAdmin: "true"`, `isAdmin: 1` o un `role` que no es una
/// cadena recibe 403.
#[tokio::test]
async fn non_strict_admin_claims_get_403() {
    let f = Fakes::seeded();
    let base = json!({"userId": "42", "username": "x", "exp": now() + 3600});
    let variants = [
        json!({"isAdmin": "true", "role": "usuario"}),
        json!({"isAdmin": 1}),
        json!({"isAdmin": [true]}),
        json!({"role": ["admin"]}),
        json!({"role": {"name": "global_admin"}}),
        json!({"role": 1}),
    ];
    for extra in variants {
        let mut claims = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            claims[k] = v.clone();
        }
        let t = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();
        let (s, _, b) = get_with(f.router(), "/api/admin/logs", Some(&t), &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{claims}");
        assert_eq!(
            b,
            json!({"error": "No autorizado. Se requieren permisos de administrador."})
        );
    }
    // Control: `isAdmin: true` sí pasa.
    let mut ok = base.clone();
    ok["isAdmin"] = json!(true);
    let t = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &ok,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    let (s, _, _) = get_with(f.router(), "/api/admin/logs", Some(&t), &[]).await;
    assert_eq!(s, StatusCode::OK);
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

/// Con Firestore desactivado (`admin: None`) las lecturas siguen en Node; con
/// repos, las atiende Rust y lo no migrado sigue en Node.
#[tokio::test]
async fn reads_mount_only_with_firestore() {
    let legacy = serve(Router::new().fallback(any(|req: Request| async move {
        format!("LEGACY {} {}", req.method(), req.uri().path())
    })))
    .await;
    let static_dir = tempfile::tempdir().unwrap();
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", SECRET),
        ("BASE_URL", "http://localhost:15633"),
        ("DISCORD_CLIENT_ID", "123"),
        ("DISCORD_CLIENT_SECRET", "x"),
        ("DISCORD_TOKEN", "x"),
        ("DISCORD_GUILD_ID", "456"),
        ("LEGACY_URL", &format!("http://{legacy}")),
        ("STATIC_DIR", static_dir.path().to_str().unwrap()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let fakes = Fakes::seeded();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let t = global();

    for with_firestore in [false, true] {
        let front = serve(build_app(AppDeps {
            config: Config::from_map(&vars).unwrap(),
            claims: None,
            admin: with_firestore.then(|| fakes.repos()),
            cs2: None,
        lobby: None,
        firestore: None,
        }))
        .await;
        for (path, key) in ALL {
            let res = client
                .get(format!("http://{front}{path}"))
                .header(header::COOKIE, format!("auth_token={t}"))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200, "{path}");
            let text = res.text().await.unwrap();
            if with_firestore {
                let v: Value = serde_json::from_str(&text).unwrap();
                assert!(v.get(key).is_some(), "{path}: {text}");
            } else {
                assert_eq!(text, format!("LEGACY GET {path}"));
            }
        }
        // Si no hay firestore/admin, todo cae al proxy Node.
        // Si hay admin montado, las rutas no existentes caen a Node.
        if !with_firestore {
            for (method, path) in [
                ("DELETE", "/api/admin/claims/LR-2026-000001"),
                ("POST", "/api/admin/messages"),
                ("GET", "/api/admin/claims/LR-1/extra"),
                ("GET", "/api/admin/steam/status"),
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
        } else {
            for (method, path) in [
                ("GET", "/api/admin/claims/LR-1/extra"),
                ("GET", "/api/admin/unmigrated-unknown-route"),
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
}
