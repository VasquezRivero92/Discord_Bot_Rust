//! `AdminContext` (`verifyAdminToken`), `resolve_target_guild_id`
//! (`resolveTargetGuildId`) y `ensure_manages_guild`, más la caída al proxy de
//! las rutas de `/api/admin` no migradas.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{Path, RawQuery, Request},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    routing::{any, get},
    Json, Router,
};
use common::config::Config;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;
use web::routes::admin::{
    ensure_manages_guild, resolve_target_guild_id, AdminContext, AdminError, AdminState,
};
use web::session::{new_claims, sign, SessionClaims, SessionState};
use web::{build_app, AppDeps};

const SECRET: &str = "admin-auth-test-secret";
const JSON_CT: &str = "application/json; charset=utf-8";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims(role: &str, is_admin: bool, servidor: Option<&str>) -> SessionClaims {
    new_claims(
        "42".into(),
        format!("u-{role}"),
        None,
        role.into(),
        is_admin,
        servidor.map(str::to_owned),
        now(),
    )
}

fn token(c: &SessionClaims) -> String {
    sign(c, SECRET.as_bytes()).unwrap()
}

fn ctx(c: SessionClaims) -> AdminContext {
    AdminContext { user: c }
}

/// Rutas de prueba que usan las piezas tal como lo harán las rutas reales.
fn probe_router(session: SessionState) -> Router {
    async fn probe(
        ctx: AdminContext,
        RawQuery(q): RawQuery,
        headers: HeaderMap,
    ) -> Result<Json<Value>, AdminError> {
        let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
        Ok(Json(
            json!({ "username": ctx.user.username, "target": target }),
        ))
    }
    async fn guild_settings(
        ctx: AdminContext,
        Path(guild_id): Path<String>,
    ) -> Result<Json<Value>, AdminError> {
        ensure_manages_guild(&ctx, &guild_id)?;
        Ok(Json(json!({ "ok": guild_id })))
    }
    Router::new()
        .route("/probe", get(probe))
        .route("/guilds/{guild_id}/settings", get(guild_settings))
        .with_state(AdminState { session })
}

fn session() -> SessionState {
    SessionState::new(SECRET.as_bytes(), false).unwrap()
}

async fn call_with(
    router: Router,
    path: &str,
    cookies: &[(&str, &str)],
) -> (StatusCode, String, Value) {
    let mut req = axum::http::Request::builder().uri(path);
    for (name, value) in cookies {
        req = req.header(header::COOKIE, format!("{name}={value}"));
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
    (
        status,
        ct,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(path: &str, cookies: &[(&str, &str)]) -> (StatusCode, String, Value) {
    call_with(probe_router(session()), path, cookies).await
}

fn error(msg: &str) -> Value {
    json!({ "error": msg })
}

// ------------------------------------------------------------- AdminContext

#[tokio::test]
async fn no_token_is_401_with_legacy_body() {
    for cookies in [
        vec![],
        vec![("auth_token", "")],
        vec![("other", "x")],
        vec![("auth_token", ""), ("admin_token", "")],
    ] {
        let (s, ct, b) = call("/probe", &cookies).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{cookies:?}");
        assert_eq!(ct, JSON_CT);
        assert_eq!(b, error("No autenticado"));
    }
}

#[tokio::test]
async fn tampered_token_is_401() {
    let t = token(&claims("global_admin", true, None));
    // Cambia un carácter de la firma (y otro del payload).
    let mut sig = t.clone().into_bytes();
    let last = sig.len() - 2;
    sig[last] = if sig[last] == b'A' { b'B' } else { b'A' };
    let sig = String::from_utf8(sig).unwrap();
    let parts: Vec<&str> = t.split('.').collect();
    let payload = format!("{}.{}x.{}", parts[0], parts[1], parts[2]);
    for bad in [sig.as_str(), payload.as_str(), "garbage", "a.b.c"] {
        let (s, ct, b) = call("/probe", &[("auth_token", bad)]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{bad}");
        assert_eq!(ct, JSON_CT);
        assert_eq!(b, error("Token inválido o expirado"));
    }
}

#[tokio::test]
async fn expired_token_is_401() {
    let mut c = claims("global_admin", true, None);
    c.exp = now() - 1;
    let (s, _, b) = call("/probe", &[("auth_token", &token(&c))]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, error("Token inválido o expirado"));
}

#[tokio::test]
async fn wrong_secret_is_401() {
    let t = sign(&claims("global_admin", true, None), b"another-secret").unwrap();
    let (s, _, b) = call("/probe", &[("admin_token", &t)]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, error("Token inválido o expirado"));
}

#[tokio::test]
async fn secret_with_edge_whitespace_is_used_verbatim() {
    let raw = "  spaced-secret \t";
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", raw),
        ("BASE_URL", "http://localhost"),
        ("DISCORD_CLIENT_ID", "1"),
        ("DISCORD_CLIENT_SECRET", "x"),
        ("DISCORD_TOKEN", "x"),
        ("DISCORD_GUILD_ID", "1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let cfg = Config::from_map(&vars).unwrap();
    let st = SessionState::from_config(&cfg).unwrap();
    let c = claims("global_admin", true, None);

    let good = sign(&c, raw.as_bytes()).unwrap();
    let (s, _, _) = call_with(probe_router(st.clone()), "/probe", &[("auth_token", &good)]).await;
    assert_eq!(s, StatusCode::OK);

    let trimmed = sign(&c, raw.trim().as_bytes()).unwrap();
    let (s, _, b) = call_with(probe_router(st), "/probe", &[("auth_token", &trimmed)]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, error("Token inválido o expirado"));
}

#[tokio::test]
async fn non_admin_role_is_403_with_legacy_body() {
    for role in ["usuario", "", "Admin", "global_admin "] {
        let t = token(&claims(role, false, Some("g1")));
        let (s, ct, b) = call("/probe", &[("auth_token", &t)]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{role:?}");
        assert_eq!(ct, JSON_CT);
        assert_eq!(
            b,
            error("No autorizado. Se requieren permisos de administrador.")
        );
    }
}

#[tokio::test]
async fn admin_roles_and_is_admin_flag_are_accepted() {
    for (role, is_admin) in [("global_admin", false), ("admin", false), ("usuario", true)] {
        let t = token(&claims(role, is_admin, None));
        let (s, _, b) = call("/probe", &[("auth_token", &t)]).await;
        assert_eq!(s, StatusCode::OK, "{role}");
        assert_eq!(b["username"], format!("u-{role}"));
    }
}

#[tokio::test]
async fn token_sources_auth_token_admin_token_and_precedence() {
    let a = token(&claims("global_admin", false, None));
    let b = token(&claims("admin", false, None));

    let (s, _, body) = call("/probe", &[("auth_token", &a)]).await;
    assert_eq!(
        (s, body["username"].clone()),
        (StatusCode::OK, json!("u-global_admin"))
    );

    let (s, _, body) = call("/probe", &[("admin_token", &b)]).await;
    assert_eq!(
        (s, body["username"].clone()),
        (StatusCode::OK, json!("u-admin"))
    );

    // Ambas válidas: gana `auth_token` (`req.cookies.auth_token || req.cookies.admin_token`).
    let (_, _, body) = call("/probe", &[("admin_token", &b), ("auth_token", &a)]).await;
    assert_eq!(body["username"], "u-global_admin");

    // `auth_token` vacía es falsy en Node: se usa `admin_token`.
    let (_, _, body) = call("/probe", &[("auth_token", ""), ("admin_token", &b)]).await;
    assert_eq!(body["username"], "u-admin");

    // La cabecera Authorization no es una fuente en el legacy.
    let req = axum::http::Request::builder()
        .uri("/probe")
        .header(header::AUTHORIZATION, format!("Bearer {a}"))
        .body(Body::empty())
        .unwrap();
    let res = probe_router(session()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Diferencia deliberada heredada de `session.rs` (documentada en el informe):
    // `auth_token` inválida + `admin_token` válida autentica (Node: 401).
    let (s, _, body) = call("/probe", &[("auth_token", "garbage"), ("admin_token", &b)]).await;
    assert_eq!(
        (s, body["username"].clone()),
        (StatusCode::OK, json!("u-admin"))
    );
}

// -------------------------------------------------- servidor (403) y destino

#[tokio::test]
async fn admin_role_with_other_servidor_is_403() {
    let own = token(&claims("admin", true, Some("g1")));
    let (s, ct, b) = call("/guilds/g2/settings", &[("auth_token", &own)]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(ct, JSON_CT);
    assert_eq!(b, error("No autorizado para gestionar este servidor."));

    let (s, _, b) = call("/guilds/g1/settings", &[("auth_token", &own)]).await;
    assert_eq!((s, b), (StatusCode::OK, json!({"ok": "g1"})));

    // `admin` sin servidor: `undefined !== guildId` -> 403.
    let none = token(&claims("admin", true, None));
    let (s, _, _) = call("/guilds/g1/settings", &[("auth_token", &none)]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Solo `isAdmin` (rol `usuario`): la regla no aplica.
    let flag = token(&claims("usuario", true, Some("g1")));
    let (s, _, _) = call("/guilds/g2/settings", &[("auth_token", &flag)]).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn global_admin_may_target_any_guild() {
    let t = token(&claims("global_admin", true, Some("g1")));
    for g in ["g1", "g2", "999"] {
        let (s, _, _) = call(&format!("/guilds/{g}/settings"), &[("auth_token", &t)]).await;
        assert_eq!(s, StatusCode::OK, "{g}");
        let (s, _, b) = call(&format!("/probe?guildId={g}"), &[("auth_token", &t)]).await;
        assert_eq!((s, b["target"].clone()), (StatusCode::OK, json!(g)));
    }
    // Sin guildId: vista global (no hay respaldo a `servidor` para global_admin).
    let (_, _, b) = call("/probe", &[("auth_token", &t)]).await;
    assert_eq!(b["target"], Value::Null);
}

#[tokio::test]
async fn admin_role_query_guild_is_not_restricted_by_resolver() {
    // Paridad: `resolveTargetGuildId` no comprueba permisos.
    let t = token(&claims("admin", true, Some("g1")));
    let (s, _, b) = call("/probe?guildId=g2", &[("auth_token", &t)]).await;
    assert_eq!((s, b["target"].clone()), (StatusCode::OK, json!("g2")));
    let (_, _, b) = call("/probe", &[("auth_token", &t)]).await;
    assert_eq!(b["target"], "g1");
}

#[tokio::test]
async fn ambiguous_query_guild_is_400() {
    let t = token(&claims("global_admin", true, None));
    for q in ["guildId=1&guildId=2", "guildId[]=1", "guildId%5Bx%5D=1"] {
        let (s, ct, b) = call(&format!("/probe?{q}"), &[("auth_token", &t)]).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{q}");
        assert_eq!(ct, JSON_CT);
        assert_eq!(b, error("guildId inválido"));
    }
}

fn headers(x_guild: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(v) = x_guild {
        h.insert("x-guild-id", HeaderValue::from_str(v).unwrap());
    }
    h
}

fn resolve(
    c: &SessionClaims,
    q: Option<&str>,
    body: Option<Value>,
    x_guild: Option<&str>,
) -> Option<String> {
    resolve_target_guild_id(&ctx(c.clone()), q, body.as_ref(), &headers(x_guild)).unwrap()
}

#[test]
fn precedence_query_empty_string_is_none_even_with_header_and_body() {
    let admin = claims("admin", true, Some("srv"));
    let body = Some(json!({"guildId": "body"}));
    assert_eq!(
        resolve(&admin, Some("guildId="), body.clone(), Some("hdr")),
        None
    );
    assert_eq!(resolve(&admin, Some("guildId"), None, Some("hdr")), None);
    assert_eq!(
        resolve(&admin, Some("guildId=q"), body, Some("hdr")).as_deref(),
        Some("q")
    );
}

#[test]
fn precedence_body_then_header() {
    let admin = claims("admin", true, Some("srv"));
    let b = |v: Value| Some(json!({ "guildId": v }));
    assert_eq!(
        resolve(&admin, Some("x=1"), b(json!("body")), Some("hdr")).as_deref(),
        Some("body")
    );
    // Presente pero falsy: null, sin pasar a la cabecera ni al servidor.
    assert_eq!(resolve(&admin, None, b(json!("")), Some("hdr")), None);
    assert_eq!(resolve(&admin, None, b(Value::Null), Some("hdr")), None);
    // Cuerpo sin guildId: cabecera.
    assert_eq!(
        resolve(&admin, None, Some(json!({"a": 1})), Some("hdr")).as_deref(),
        Some("hdr")
    );
    assert_eq!(
        resolve(&admin, Some("other=1"), None, Some("hdr")).as_deref(),
        Some("hdr")
    );
}

#[test]
fn precedence_servidor_fallback_only_for_admin_role() {
    let admin = claims("admin", true, Some("srv"));
    assert_eq!(resolve(&admin, None, None, None).as_deref(), Some("srv"));
    // Cabecera vacía: `'' || null` -> null -> se usa el servidor.
    assert_eq!(
        resolve(&admin, None, None, Some("")).as_deref(),
        Some("srv")
    );
    assert_eq!(
        resolve(&claims("admin", true, Some("")), None, None, None),
        None
    );
    assert_eq!(
        resolve(&claims("admin", true, None), None, None, None),
        None
    );
    for role in ["global_admin", "usuario"] {
        assert_eq!(
            resolve(&claims(role, true, Some("srv")), None, None, None),
            None
        );
    }
}

#[test]
fn non_string_body_guild_is_rejected() {
    let admin = claims("global_admin", true, None);
    for v in [json!(5), json!(true), json!(["a"]), json!({"a": "b"})] {
        let r = resolve_target_guild_id(
            &ctx(admin.clone()),
            None,
            Some(&json!({ "guildId": v })),
            &HeaderMap::new(),
        );
        assert_eq!(r, Err(AdminError::InvalidGuildId));
    }
}

// --------------------------------------------------------------- proxy

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

#[tokio::test]
async fn unmigrated_admin_route_falls_through_to_proxy() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    let legacy = serve(Router::new().fallback(any(move |req: Request| {
        let log = log.clone();
        async move {
            let line = format!(
                "{} {}",
                req.method(),
                req.uri().path_and_query().map_or("/", |pq| pq.as_str())
            );
            log.lock().unwrap().push(line.clone());
            format!("LEGACY {line}")
        }
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
    let front = serve(build_app(AppDeps {
        config: Config::from_map(&vars).unwrap(),
        claims: None,
        admin: None,
        cs2: None,
        lobby: None,
    }))
    .await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let t = token(&claims("global_admin", true, None));
    for (method, path) in [
        ("POST", "/api/admin/orders/x/status"),
        ("GET", "/api/admin/me"),
        ("GET", "/api/admin/claims?status=open"),
        ("DELETE", "/api/admin/claims/LR-1"),
    ] {
        let res = client
            .request(method.parse().unwrap(), format!("http://{front}{path}"))
            .header(header::COOKIE, format!("auth_token={t}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(if method == "GET" { "" } else { "{}" })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{method} {path}");
        assert_eq!(res.text().await.unwrap(), format!("LEGACY {method} {path}"));
    }
    // Sin cookie también llega a Node (Rust no responde 401 por rutas no migradas).
    let res = client
        .post(format!("http://{front}/api/admin/orders/x/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(seen.lock().unwrap().len(), 5);
}
