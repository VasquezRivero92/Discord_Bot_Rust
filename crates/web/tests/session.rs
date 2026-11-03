use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::FromRef,
    http::{header, Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::session::{
    clear_cookies, issue_cookies, new_claims, set_cookie_values, sign, verify, AdminUser, AuthUser,
    OptionalUser, SessionClaims, SessionState,
};

const SECRET: &str = "test-secret-do-not-use-in-prod";
const VECTORS: &str = include_str!("fixtures/jwt_vectors.json");

#[derive(Clone)]
struct AppState {
    session: SessionState,
}

impl FromRef<AppState> for SessionState {
    fn from_ref(s: &AppState) -> SessionState {
        s.session.clone()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn state(secure: bool) -> SessionState {
    SessionState::new(SECRET.as_bytes(), secure).unwrap()
}

fn router() -> Router {
    async fn opt(OptionalUser(u): OptionalUser) -> impl IntoResponse {
        Json(json!({ "user": u.map(|c| c.username) }))
    }
    async fn auth(AuthUser(u): AuthUser) -> impl IntoResponse {
        Json(json!({ "username": u.username }))
    }
    async fn admin(AdminUser(u): AdminUser) -> impl IntoResponse {
        Json(json!({ "username": u.username }))
    }
    Router::new()
        .route("/opt", get(opt))
        .route("/auth", get(auth))
        .route("/admin", get(admin))
        .with_state(AppState {
            session: state(false),
        })
}

async fn call(path: &str, cookies: &[&[u8]]) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::builder().uri(path);
    for c in cookies {
        req = req.header(
            header::COOKIE,
            header::HeaderValue::from_bytes(c).expect("valid header bytes"),
        );
    }
    let res = router()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        ct,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn vector(name: &str) -> Value {
    let all: Value = serde_json::from_str(VECTORS).unwrap();
    all["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap_or_else(|| panic!("vector {name} missing"))
        .clone()
}

fn vtoken(name: &str) -> String {
    vector(name)["token"].as_str().unwrap().to_owned()
}

fn claims(role: &str, is_admin: bool, name: &str, at: u64) -> SessionClaims {
    new_claims(
        "42".into(),
        name.into(),
        None,
        role.into(),
        is_admin,
        None,
        at,
    )
}

fn token_for(c: &SessionClaims) -> String {
    sign(c, SECRET.as_bytes()).unwrap()
}

fn cookie(name: &str, value: &str) -> Vec<u8> {
    format!("{name}={value}").into_bytes()
}

const NOT_AUTH: &str = "No autenticado";
const BAD_TOKEN: &str = "Token inválido o expirado";
const FORBIDDEN: &str = "No autorizado. Se requieren permisos de administrador.";

#[test]
fn node_token_validates_in_rust() {
    let all: Value = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(all["secret"], SECRET);
    for v in all["vectors"].as_array().unwrap() {
        let token = v["token"].as_str().unwrap();
        let res = verify(token, SECRET.as_bytes());
        match v["expect"].as_str().unwrap() {
            "valid" => {
                let c = res.unwrap_or_else(|e| panic!("{} should be valid: {e}", v["name"]));
                assert_eq!(c.user_id, v["claims"]["userId"]);
                assert_eq!(c.username, v["claims"]["username"]);
                assert_eq!(c.role, v["claims"]["role"]);
                assert_eq!(c.is_admin, v["claims"]["isAdmin"]);
                assert_eq!(c.exp, v["claims"]["exp"].as_u64().unwrap());
                assert_eq!(c.is_admin_user(), v["is_admin_user"].as_bool().unwrap());
                // unknown claims (iat) are preserved
                assert_eq!(c.extra["iat"], v["claims"]["iat"]);
            }
            _ => assert!(res.is_err(), "{} should be rejected", v["name"]),
        }
    }
    let admin = verify(&vtoken("valid_admin"), SECRET.as_bytes()).unwrap();
    assert_eq!(admin.avatar.as_deref(), Some("abc123"));
    assert_eq!(admin.servidor.as_deref(), Some("222222222222222222"));
    let user = verify(&vtoken("valid_non_admin"), SECRET.as_bytes()).unwrap();
    assert_eq!(user.avatar, None);
    assert_eq!(user.servidor, None);
}

#[test]
fn claims_are_tolerant() {
    let mk = |payload: Value| serde_json::from_value::<SessionClaims>(payload);
    // numeric userId, absent isAdmin/role/avatar/servidor
    let c =
        mk(json!({"userId": 123456789012345u64, "username": "n", "exp": 4102444800u64})).unwrap();
    assert_eq!(c.user_id, "123456789012345");
    assert!(!c.is_admin);
    assert_eq!(c.role, "");
    assert!(!c.is_admin_user());
    // explicit nulls
    let c = mk(json!({"userId":"1","username":"n","avatar":null,"servidor":null,"exp":1})).unwrap();
    assert_eq!(c.avatar, None);
    assert_eq!(c.servidor, None);
    // admin by each route
    assert!(claims("global_admin", false, "a", 0).is_admin_user());
    assert!(claims("admin", false, "a", 0).is_admin_user());
    assert!(claims("user", true, "a", 0).is_admin_user());
    assert!(!claims("user", false, "a", 0).is_admin_user());
}

#[test]
fn verify_never_panics_on_junk() {
    let huge = "A".repeat(1_000_000);
    for t in ["", ".", "..", "a.b.c", "\u{0}", "é.é.é", huge.as_str()] {
        assert!(verify(t, SECRET.as_bytes()).is_err());
    }
}

#[test]
fn rust_token_validates_in_node() {
    let legacy = std::env::var("LEGACY_DIR").unwrap_or_else(|_| "D:/Discord_Faceit".into());
    let module = std::path::Path::new(&legacy)
        .join("node_modules")
        .join("jsonwebtoken");
    if !module.exists() {
        eprintln!(
            "SKIP rust_token_validates_in_node: {} not found",
            module.display()
        );
        return;
    }
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIP rust_token_validates_in_node: `node` not available");
        return;
    }
    let mut c = new_claims(
        "999".into(),
        "RustUser".into(),
        Some("av".into()),
        "admin".into(),
        true,
        Some("srv".into()),
        now(),
    );
    c.extra.insert("iat".into(), json!(now()));
    let token = token_for(&c);
    let script = "const jwt=require(process.env.JWT_MODULE);\
        const d=jwt.verify(process.env.TOKEN,process.env.SECRET);\
        process.stdout.write(JSON.stringify(d));";
    let out = Command::new("node")
        .arg("-e")
        .arg(script)
        .env("JWT_MODULE", &module)
        .env("TOKEN", &token)
        .env("SECRET", SECRET)
        .output()
        .expect("spawn node");
    assert!(
        out.status.success(),
        "node failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let decoded: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(decoded, serde_json::to_value(&c).unwrap());
    assert_eq!(decoded["userId"], "999");
    assert_eq!(decoded["isAdmin"], true);
    assert_eq!(decoded["exp"], c.exp);
}

#[tokio::test]
async fn auth_token_expired_but_admin_token_valid_authenticates() {
    let expired = vtoken("expired");
    let good = token_for(&claims("admin", true, "Good", now()));
    let (s, _, b) = call(
        "/admin",
        &[
            &cookie("auth_token", &expired),
            &cookie("admin_token", &good),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["username"], "Good");
}

#[tokio::test]
async fn admin_token_only_works() {
    let good = token_for(&claims("admin", false, "OnlyAdminCookie", now()));
    let (s, _, b) = call("/auth", &[&cookie("admin_token", &good)]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["username"], "OnlyAdminCookie");
}

#[tokio::test]
async fn auth_token_wins_when_both_valid() {
    let a = token_for(&claims("user", false, "FromAuth", now()));
    let b = token_for(&claims("user", false, "FromAdmin", now()));
    let (_, _, body) = call(
        "/auth",
        &[&cookie("admin_token", &b), &cookie("auth_token", &a)],
    )
    .await;
    assert_eq!(body["username"], "FromAuth");
}

#[tokio::test]
async fn garbage_token_is_401_never_panics() {
    for t in ["", "garbage", "a.b.c", "....", "%%%", "eyJ.eyJ.sig"] {
        let (s, ct, b) = call("/auth", &[&cookie("auth_token", t)]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "token {t:?}");
        assert_eq!(ct.as_deref(), Some("application/json"));
        // an empty value is treated as absent, like Node's falsy check
        let want = if t.is_empty() { NOT_AUTH } else { BAD_TOKEN };
        assert_eq!(b, json!({ "error": want }), "token {t:?}");
    }
}

#[tokio::test]
async fn alg_none_is_rejected() {
    let t = vtoken("alg_none");
    for path in ["/auth", "/admin"] {
        let (s, _, b) = call(path, &[&cookie("auth_token", &t)]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(b, json!({ "error": BAD_TOKEN }));
    }
    let (_, _, b) = call("/opt", &[&cookie("auth_token", &t)]).await;
    assert_eq!(b["user"], Value::Null);
}

#[tokio::test]
async fn hs384_wrong_secret_and_expired_rejected() {
    for name in ["hs384", "wrong_secret", "expired"] {
        let (s, _, b) = call("/auth", &[&cookie("auth_token", &vtoken(name))]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{name}");
        assert_eq!(b, json!({ "error": BAD_TOKEN }), "{name}");
    }
}

#[tokio::test]
async fn zero_leeway_and_exp_required() {
    let n = now();
    // exp = now - 1 must already be rejected (Node clockTolerance = 0)
    let mut c = claims("user", false, "x", n);
    c.exp = n - 1;
    let (s, _, _) = call("/auth", &[&cookie("auth_token", &token_for(&c))]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // a token without exp is rejected
    let mut v = serde_json::to_value(&c).unwrap();
    v.as_object_mut().unwrap().remove("exp");
    let t = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &v,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    let (s, _, _) = call("/auth", &[&cookie("auth_token", &t)]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn no_cookie_is_not_authenticated() {
    for path in ["/auth", "/admin"] {
        let (s, ct, b) = call(path, &[]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(ct.as_deref(), Some("application/json"));
        assert_eq!(b, json!({ "error": NOT_AUTH }));
    }
    let (s, _, b) = call("/auth", &[b"other=1; foo=bar"]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, json!({ "error": NOT_AUTH }));
}

#[tokio::test]
async fn non_admin_gets_403_on_admin_routes() {
    let t = token_for(&claims("user", false, "Plain", now()));
    let (s, ct, b) = call("/admin", &[&cookie("auth_token", &t)]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(ct.as_deref(), Some("application/json"));
    assert_eq!(b, json!({ "error": FORBIDDEN }));
    let (s, _, _) = call("/auth", &[&cookie("auth_token", &t)]).await;
    assert_eq!(s, StatusCode::OK);
    let nt = vtoken("valid_non_admin");
    let (s, _, _) = call("/admin", &[&cookie("auth_token", &nt)]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let at = vtoken("valid_admin");
    let (s, _, b) = call("/admin", &[&cookie("auth_token", &at)]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["username"], "AdminUser");
}

#[tokio::test]
async fn optional_user_never_rejects() {
    let (s, _, b) = call("/opt", &[&cookie("auth_token", "garbage")]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["user"], Value::Null);
    let (s, _, b) = call("/opt", &[]).await;
    assert_eq!((s, &b["user"]), (StatusCode::OK, &Value::Null));
    let t = token_for(&claims("user", false, "Me", now()));
    let (_, _, b) = call("/opt", &[&cookie("admin_token", &t)]).await;
    assert_eq!(b["user"], "Me");
}

#[tokio::test]
async fn huge_cookie_is_401_without_panic() {
    let huge = "A".repeat(1_000_000);
    let (s, _, b) = call("/auth", &[&cookie("auth_token", &huge)]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, json!({ "error": BAD_TOKEN }));
}

#[tokio::test]
async fn non_utf8_cookie_bytes_do_not_panic() {
    let (s, _, _) = call("/auth", &[b"auth_token=\xff\xfe\x80"]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // a non-UTF-8 header must not hide a valid cookie in another header
    let good = token_for(&claims("user", false, "Ok", now()));
    let (s, _, b) = call("/auth", &[b"junk=\xff\xfe", &cookie("auth_token", &good)]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["username"], "Ok");
}

#[tokio::test]
async fn cookie_header_parsing_is_robust() {
    let good = token_for(&claims("user", false, "Parsed", now()));
    // multiple Cookie headers
    let (s, _, b) = call("/auth", &[b"a=1; b=2", &cookie("auth_token", &good)]).await;
    assert_eq!(
        (s, b["username"].as_str()),
        (StatusCode::OK, Some("Parsed"))
    );
    // spaces and other cookies in the same header
    let line = format!("theme=dark;   auth_token={good} ;x=y");
    let (s, _, b) = call("/auth", &[line.as_bytes()]).await;
    assert_eq!(
        (s, b["username"].as_str()),
        (StatusCode::OK, Some("Parsed"))
    );
    // quoted value
    let line = format!("auth_token=\"{good}\"");
    let (s, _, _) = call("/auth", &[line.as_bytes()]).await;
    assert_eq!(s, StatusCode::OK);
    // duplicate names: first invalid, later valid -> authenticates
    let line = format!("auth_token=bad; auth_token={good}");
    let (s, _, _) = call("/auth", &[line.as_bytes()]).await;
    assert_eq!(s, StatusCode::OK);
    // a cookie whose name merely ends with auth_token is not it
    let line = format!("xauth_token={good}");
    let (s, _, b) = call("/auth", &[line.as_bytes()]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, json!({ "error": NOT_AUTH }));
}

fn find<'a>(cs: &'a [cookie::Cookie<'static>], name: &str) -> &'a cookie::Cookie<'static> {
    cs.iter().find(|c| c.name() == name).unwrap()
}

#[test]
fn cookies_have_expected_flags() {
    for secure in [false, true] {
        let st = state(secure);
        let c = claims("admin", true, "x", now());
        let t = token_for(&c);
        let cs = issue_cookies(&c, &t, &st);
        for name in ["auth_token", "admin_token"] {
            let ck = find(&cs, name);
            assert_eq!(ck.value(), t);
            assert_eq!(ck.http_only(), Some(true));
            assert_eq!(ck.same_site(), Some(cookie::SameSite::Lax));
            assert_eq!(ck.path(), Some("/"));
            assert_eq!(ck.max_age(), Some(cookie::time::Duration::seconds(28_800)));
            assert_eq!(ck.secure().unwrap_or(false), secure);
        }
        let headers = set_cookie_values(&cs);
        assert_eq!(headers.len(), 2);
        let h = headers[0].to_str().unwrap();
        assert!(h.contains("HttpOnly") && h.contains("SameSite=Lax"));
        assert!(h.contains("Max-Age=28800") && h.contains("Path=/"));
        assert_eq!(h.contains("Secure"), secure);
    }
}

#[test]
fn clear_cookies_have_expected_flags() {
    let cs = clear_cookies();
    for name in ["auth_token", "admin_token"] {
        let ck = find(&cs, name);
        assert_eq!(ck.value(), "");
        assert_eq!(ck.path(), Some("/"));
        assert_eq!(ck.max_age(), Some(cookie::time::Duration::ZERO));
        assert!(ck.expires_datetime().is_some());
    }
    let h = set_cookie_values(&cs);
    assert!(h[0].to_str().unwrap().contains("Max-Age=0"));
}

#[test]
fn new_claims_expire_in_eight_hours() {
    let c = claims("user", false, "x", 1000);
    assert_eq!(c.exp, 1000 + 8 * 3600);
}

#[test]
fn session_state_from_config_follows_production() {
    use std::collections::HashMap;
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", "s3"),
        ("BASE_URL", "https://x.example"),
        ("DISCORD_CLIENT_ID", "id"),
        ("DISCORD_CLIENT_SECRET", "secret"),
        ("DISCORD_TOKEN", "token"),
        ("DISCORD_GUILD_ID", "guild"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let cfg = common::config::Config::from_map(&vars).unwrap();
    let st = SessionState::from_config(&cfg).unwrap();
    assert!(st.secure_cookies);
}

#[test]
fn jwt_secret_with_edge_whitespace_verifies_token_signed_with_raw_secret() {
    use std::collections::HashMap;
    let raw = " s3cr3t ";
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", raw),
        ("BASE_URL", "http://localhost:15633"),
        ("DISCORD_CLIENT_ID", "id"),
        ("DISCORD_CLIENT_SECRET", "secret"),
        ("DISCORD_TOKEN", "token"),
        ("DISCORD_GUILD_ID", "guild"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let cfg = common::config::Config::from_map(&vars).unwrap();
    let st = SessionState::from_config(&cfg).unwrap();
    let t = sign(&claims("user", false, "x", now()), raw.as_bytes()).unwrap();
    assert!(st.verify(&t).is_ok());
    let trimmed = sign(&claims("user", false, "x", now()), raw.trim().as_bytes()).unwrap();
    assert!(st.verify(&trimmed).is_err());
}

#[test]
fn empty_secret_is_rejected_by_constructor() {
    assert!(SessionState::new(b"", false).is_err());
}

#[test]
fn token_expiring_exactly_now_is_rejected() {
    // Node: `clockTimestamp >= exp` ya es TokenExpiredError.
    let n = now();
    let mut c = claims("user", false, "x", n);
    c.exp = n;
    assert!(verify(&token_for(&c), SECRET.as_bytes()).is_err());
    c.exp = n + 5;
    assert!(verify(&token_for(&c), SECRET.as_bytes()).is_ok());
}

#[test]
fn token_without_iat_is_accepted_like_legacy() {
    // jsonwebtoken (npm) no exige `iat`; solo firma y `exp`.
    let c = claims("user", false, "x", now());
    assert!(!c.extra.contains_key("iat"));
    let got = verify(&token_for(&c), SECRET.as_bytes()).unwrap();
    assert!(!got.extra.contains_key("iat"));
}

#[tokio::test]
async fn verify_attempt_cap() {
    let good = token_for(&claims("user", false, "x", now()));
    // 16 candidatos inválidos agotan el tope: el válido número 17 no se prueba.
    let bad: Vec<Vec<u8>> = (0..16)
        .map(|i| cookie("auth_token", &format!("bad{i}")))
        .collect();
    let mut with_good_late: Vec<&[u8]> = bad.iter().map(|v| v.as_slice()).collect();
    let g = cookie("admin_token", &good);
    with_good_late.push(&g);
    let (s, _, _) = call("/auth", &with_good_late).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // con 15 inválidos, el válido es el 16 y entra
    let mut ok: Vec<&[u8]> = bad[..15].iter().map(|v| v.as_slice()).collect();
    ok.push(&g);
    let (s, _, _) = call("/auth", &ok).await;
    assert_eq!(s, StatusCode::OK);
}

#[test]
fn clear_cookies_header_has_expires_epoch() {
    let h = set_cookie_values(&clear_cookies());
    for v in h {
        assert!(
            v.to_str().unwrap().contains("Expires=Thu, 01 Jan 1970"),
            "{v:?}"
        );
    }
}
