//! Tests de `web::routes::auth` y de `web::discord` (cliente REST contra un Discord falso local).
//!
//! Fuente (legacy, solo lectura): `routes/auth.js:36-46` (`GET /auth/discord`) y `:499-503`
//! (`GET /auth/logout`). `GET /auth/discord/callback` (`:48-277`) NO esta migrado: escribe en
//! Firestore (`db.setUserRoleAndGuild`) y lee `faceit_users` y capitanes sin repo en `data`, asi
//! que sigue en el proxy. Por eso aqui no hay tests de las ramas del callback, solo uno que fija
//! que este router no lo atiende (cae al proxy) y que no emite cookies.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::Router;
use tokio::net::TcpListener;
use tower::ServiceExt;
use web::discord::testing::FakeDiscordApi;
use web::discord::{
    check_discord_admin, AdminRoleSources, DiscordApi, DiscordError, DiscordUser, GuildMember,
    RestDiscordApi, Role,
};
use web::routes::auth::{self, AuthState};

fn app() -> Router {
    auth::router(AuthState::new("123", "https://machupeek.example"))
}

async fn get(uri: &str) -> Response {
    app()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn location(res: &Response) -> String {
    res.headers()
        .get(header::LOCATION)
        .expect("location")
        .to_str()
        .unwrap()
        .to_owned()
}

fn authorize(state: &str) -> String {
    format!(
        "https://discord.com/oauth2/authorize?client_id=123&redirect_uri=https%3A%2F%2Fmachupeek.example%2Fauth%2Fdiscord%2Fcallback&response_type=code&scope=identify&state={state}&prompt=consent"
    )
}

// ---------------------------------------------------------------- GET /auth/discord

#[tokio::test]
async fn discord_redirects_to_authorize_with_exact_params() {
    let res = get("/auth/discord").await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(location(&res), authorize("%2F"));
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn discord_keeps_a_safe_redirect_path_in_state() {
    let res = get("/auth/discord?redirect=/admin/panel%3Fa%3D1").await;
    assert_eq!(location(&res), authorize("%2Fadmin%2Fpanel%3Fa%3D1"));
}

#[tokio::test]
async fn discord_authorize_params_are_exactly_the_legacy_ones() {
    let res = get("/auth/discord").await;
    let url = url::Url::parse(&location(&res)).unwrap();
    assert_eq!(url.host_str(), Some("discord.com"));
    assert_eq!(url.path(), "/oauth2/authorize");
    let keys: Vec<String> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
    assert_eq!(
        keys,
        [
            "client_id",
            "redirect_uri",
            "response_type",
            "scope",
            "state",
            "prompt"
        ]
    );
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["scope"], "identify");
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["prompt"], "consent");
    assert_eq!(
        q["redirect_uri"],
        "https://machupeek.example/auth/discord/callback"
    );
}

#[tokio::test]
async fn discord_ignores_unsafe_redirect_values() {
    // sanitizeRedirectPath: debe empezar por "/", no por "//", sin "\", <= 300, una sola vez.
    let long = format!("/{}", "a".repeat(300)); // 301 > 300
    let cases = [
        "redirect=//evil.example",
        "redirect=https://evil.example/",
        "redirect=http://evil.example",
        "redirect=evil.example",
        "redirect=%2F%5Cevil.example",
        "redirect=/%5Cevil.example",
        "redirect=javascript:alert(1)",
        "redirect=",
        "redirect=/a&redirect=/b",
    ];
    for q in cases {
        let res = get(&format!("/auth/discord?{q}")).await;
        assert_eq!(res.status(), StatusCode::FOUND, "{q}");
        assert_eq!(location(&res), authorize("%2F"), "{q}");
    }
    let res = get(&format!("/auth/discord?redirect={long}")).await;
    assert_eq!(location(&res), authorize("%2F"));
    // 300 exactos si se aceptan
    let ok = format!("/{}", "a".repeat(299));
    let res = get(&format!("/auth/discord?redirect={ok}")).await;
    assert_eq!(location(&res), authorize(&ok.replace('/', "%2F")));
}

#[tokio::test]
async fn discord_never_redirects_to_request_controlled_hosts() {
    // `next`, `return_to`, Host, X-Forwarded-Host... no influyen en el destino.
    let res = app()
        .oneshot(
            Request::builder()
                .uri("/auth/discord?next=https://evil.example&return_to=//evil.example&redirect_uri=https://evil.example")
                .header(header::HOST, "evil.example")
                .header("x-forwarded-host", "evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(location(&res), authorize("%2F"));
}

#[tokio::test]
async fn discord_state_with_control_chars_cannot_split_headers() {
    let res = get("/auth/discord?redirect=/a%0d%0aSet-Cookie:x=1").await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert!(location(&res).starts_with("https://discord.com/oauth2/authorize?"));
    assert!(!location(&res).contains('\r') && !location(&res).contains('\n'));
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn base_url_trailing_slashes_are_stripped_like_the_legacy() {
    // La normalizacion vive en `Config` (una sola vez); `from_config` la consume.
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", "s"),
        ("BASE_URL", "https://machupeek.example//"),
        ("DISCORD_CLIENT_ID", "123"),
        ("DISCORD_CLIENT_SECRET", "s"),
        ("DISCORD_TOKEN", "t"),
        ("DISCORD_GUILD_ID", "g"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let cfg = common::config::Config::from_map(&vars).unwrap();
    let app = auth::router(AuthState::from_config(&cfg));
    let res = app
        .oneshot(Request::get("/auth/discord").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(location(&res), authorize("%2F"));
}

// ---------------------------------------------------------------- callback (proxy)

#[tokio::test]
async fn standalone_router_does_not_handle_proxied_auth_routes() {
    for uri in [
        "/auth/discord/callback",
        "/auth/discord/callback?code=abc",
        "/auth/steam",
        "/auth/demo-login",
    ] {
        let res = get(uri).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
        assert!(res.headers().get(header::SET_COOKIE).is_none(), "{uri}");
    }
}

// ---------------------------------------------------------------- GET /auth/logout

#[tokio::test]
async fn logout_clears_both_cookies_and_redirects() {
    let res = get("/auth/logout").await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(location(&res), "/");
    let cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(cookies.len(), 2, "{cookies:?}");
    for name in ["auth_token", "admin_token"] {
        let c = cookies
            .iter()
            .find(|c| c.starts_with(&format!("{name}=;")))
            .unwrap_or_else(|| panic!("falta {name}: {cookies:?}"));
        assert!(c.contains("Path=/"), "{c}");
        assert!(c.contains("Max-Age=0"), "{c}");
        assert!(c.contains("1970"), "{c}");
    }
}

#[tokio::test]
async fn logout_ignores_redirect_params_and_works_with_a_session() {
    let res = app()
        .oneshot(
            Request::get("/auth/logout?redirect=https://evil.example&next=//evil.example")
                .header(header::COOKIE, "auth_token=abc; admin_token=def")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(location(&res), "/");
}

// ---------------------------------------------------------------- RestDiscordApi vs Discord falso

type Seen = Arc<Mutex<Vec<(String, String, HeaderMap, String)>>>;
type Script = Arc<Mutex<HashMap<String, (u16, Vec<(&'static str, String)>, String)>>>;

struct FakeServer {
    addr: SocketAddr,
    seen: Seen,
    script: Script,
}

impl FakeServer {
    async fn start() -> Self {
        let seen: Seen = Arc::default();
        let script: Script = Arc::default();
        let (s2, c2) = (seen.clone(), script.clone());
        let app = Router::new().fallback(move |req: Request| {
            let (seen, script) = (s2.clone(), c2.clone());
            async move {
                let (parts, body) = req.into_parts();
                let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
                let key = format!("{} {}", parts.method, parts.uri.path());
                seen.lock().unwrap().push((
                    parts.method.to_string(),
                    parts.uri.path().to_owned(),
                    parts.headers.clone(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
                let (status, headers, body) = script
                    .lock()
                    .unwrap()
                    .get(&key)
                    .cloned()
                    .unwrap_or((404, vec![], "{}".into()));
                let mut b = Response::builder().status(status);
                for (k, v) in headers {
                    b = b.header(k, v);
                }
                b.body(Body::from(body)).unwrap()
            }
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { addr, seen, script }
    }

    fn on(&self, key: &str, status: u16, headers: Vec<(&'static str, String)>, body: &str) {
        self.script
            .lock()
            .unwrap()
            .insert(key.to_owned(), (status, headers, body.to_owned()));
    }

    fn api(&self) -> RestDiscordApi {
        RestDiscordApi::new(
            "cid",
            "SECRET-CLIENT",
            "SECRET-BOT",
            "https://machupeek.example",
        )
        .with_base_url(&format!("http://{}/api/v10/", self.addr))
    }

    fn last(&self) -> (String, String, HeaderMap, String) {
        self.seen.lock().unwrap().last().cloned().expect("request")
    }
}

#[tokio::test]
async fn rest_member_sends_bot_auth_and_legacy_user_agent() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/members/2",
        200,
        vec![("content-type", "application/json".into())],
        r#"{"user":{"id":"2","username":"x"},"roles":["5","6"],"nick":null}"#,
    );
    let m = s.api().guild_member("1", "2").await.unwrap().unwrap();
    assert_eq!(
        m,
        GuildMember {
            user_id: "2".into(),
            roles: vec!["5".into(), "6".into()]
        }
    );
    let (_, _, h, _) = s.last();
    assert_eq!(h[header::AUTHORIZATION], "Bot SECRET-BOT");
    assert_eq!(
        h[header::USER_AGENT],
        "DiscordBot (https://machupeek.example, 1.0.0)"
    );
}

#[tokio::test]
async fn rest_member_404_is_none() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/members/2",
        404,
        vec![],
        r#"{"message":"Unknown Member","code":10007}"#,
    );
    assert_eq!(s.api().guild_member("1", "2").await, Ok(None));
}

#[tokio::test]
async fn rest_429_is_rate_limited_with_retry_after() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/roles",
        429,
        vec![("retry-after", "2.5".into())],
        r#"{"message":"You are being rate limited."}"#,
    );
    assert_eq!(
        s.api().guild_roles("1").await,
        Err(DiscordError::RateLimited {
            retry_after: Duration::from_millis(2500)
        })
    );
}

#[tokio::test]
async fn rest_429_without_header_defaults_and_garbage_header_is_safe() {
    let s = FakeServer::start().await;
    s.on("GET /api/v10/guilds/1/roles", 429, vec![], "{}");
    assert!(matches!(
        s.api().guild_roles("1").await,
        Err(DiscordError::RateLimited { .. })
    ));
    s.on(
        "GET /api/v10/guilds/1/roles",
        429,
        vec![("retry-after", "NaN".into())],
        "{}",
    );
    assert!(matches!(
        s.api().guild_roles("1").await,
        Err(DiscordError::RateLimited { .. })
    ));
}

#[tokio::test]
async fn rest_roles_parse_decimal_string_permissions() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/roles",
        200,
        vec![],
        r#"[{"id":"1","name":"@everyone","permissions":"1071698660929"},{"id":"9","name":"Admin","permissions":"8"}]"#,
    );
    let roles = s.api().guild_roles("1").await.unwrap();
    assert_eq!(roles.len(), 2);
    assert_eq!(roles[1].permissions, "8");
}

#[tokio::test]
async fn rest_owner_id() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1",
        200,
        vec![],
        r#"{"id":"1","owner_id":"77"}"#,
    );
    assert_eq!(s.api().guild_owner_id("1").await.unwrap(), "77");
}

#[tokio::test]
async fn rest_malformed_json_is_an_error_that_does_not_leak() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/roles",
        200,
        vec![],
        "<html>SECRET-BOT leak</html>",
    );
    let err = s.api().guild_roles("1").await.unwrap_err();
    assert_eq!(err, DiscordError::Malformed);
    let text = format!("{err} {err:?}");
    assert!(!text.contains("SECRET") && !text.contains("html"), "{text}");
}

#[tokio::test]
async fn rest_server_error_does_not_leak_body() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/roles",
        500,
        vec![],
        "boom SECRET-BOT",
    );
    let err = s.api().guild_roles("1").await.unwrap_err();
    assert_eq!(err, DiscordError::Status(500));
    assert!(!format!("{err:?}").contains("SECRET"));
}

#[tokio::test]
async fn rest_rejects_ids_that_are_not_digits_without_calling_out() {
    let s = FakeServer::start().await;
    let api = s.api();
    for bad in ["", "../x", "1/2", "1?x=1", "a", "1 2", "%2e%2e"] {
        assert_eq!(
            api.guild_member(bad, "2").await,
            Err(DiscordError::InvalidArgument),
            "{bad:?}"
        );
        assert_eq!(
            api.guild_roles(bad).await,
            Err(DiscordError::InvalidArgument)
        );
        assert_eq!(
            api.guild_owner_id(bad).await,
            Err(DiscordError::InvalidArgument)
        );
    }
    assert_eq!(
        api.guild_member("1", "../2").await,
        Err(DiscordError::InvalidArgument)
    );
    assert!(s.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rest_does_not_follow_redirects() {
    let s = FakeServer::start().await;
    s.on(
        "GET /api/v10/guilds/1/roles",
        302,
        vec![("location", "http://127.0.0.1:1/steal".into())],
        "",
    );
    assert_eq!(
        s.api().guild_roles("1").await,
        Err(DiscordError::Status(302))
    );
    assert_eq!(s.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rest_transport_failure_is_generic() {
    let api = RestDiscordApi::new("cid", "SECRET-CLIENT", "SECRET-BOT", "https://x.example")
        .with_base_url("http://127.0.0.1:1");
    let err = api.guild_roles("1").await.unwrap_err();
    assert_eq!(err, DiscordError::Transport);
    assert!(!format!("{err}").contains("127.0.0.1"));
}

#[tokio::test]
async fn rest_exchange_code_posts_form_and_fetches_user_with_bearer() {
    let s = FakeServer::start().await;
    s.on(
        "POST /api/v10/oauth2/token",
        200,
        vec![],
        r#"{"access_token":"ACCESS-XYZ","token_type":"Bearer","expires_in":604800}"#,
    );
    s.on(
        "GET /api/v10/users/@me",
        200,
        vec![],
        r#"{"id":"42","username":"neo","global_name":"Neo","avatar":"abc","discriminator":"0"}"#,
    );
    let user = s
        .api()
        .exchange_code(
            "the code&x=1",
            "https://machupeek.example/auth/discord/callback",
        )
        .await
        .unwrap();
    assert_eq!(
        user,
        DiscordUser {
            id: "42".into(),
            username: "neo".into(),
            global_name: Some("Neo".into()),
            avatar: Some("abc".into())
        }
    );
    let seen = s.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    let (_, _, h, body) = &seen[0];
    assert_eq!(h[header::CONTENT_TYPE], "application/x-www-form-urlencoded");
    assert_eq!(
        h[header::USER_AGENT],
        "DiscordBot (https://machupeek.example, 1.0.0)"
    );
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(form["client_id"], "cid");
    assert_eq!(form["client_secret"], "SECRET-CLIENT");
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "the code&x=1");
    assert_eq!(
        form["redirect_uri"],
        "https://machupeek.example/auth/discord/callback"
    );
    let (_, _, h, _) = &seen[1];
    assert_eq!(h[header::AUTHORIZATION], "Bearer ACCESS-XYZ");
    assert_eq!(
        h[header::USER_AGENT],
        "DiscordBot (https://machupeek.example, 1.0.0)"
    );
}

#[tokio::test]
async fn rest_exchange_code_error_kinds() {
    let s = FakeServer::start().await;
    let ex = || async { s.api().exchange_code("c", "https://r").await };
    s.on(
        "POST /api/v10/oauth2/token",
        400,
        vec![],
        r#"{"error":"invalid_grant","error_description":"Invalid \"code\" in request."}"#,
    );
    assert_eq!(ex().await, Err(DiscordError::InvalidGrant));
    s.on(
        "POST /api/v10/oauth2/token",
        429,
        vec![("retry-after", "9".into())],
        "<html><div id=\"cf-wrapper\">error code: 1015</div></html>",
    );
    assert_eq!(ex().await, Err(DiscordError::CloudflareBlocked));
    s.on(
        "POST /api/v10/oauth2/token",
        429,
        vec![("retry-after", "9".into())],
        r#"{"message":"rate limited"}"#,
    );
    assert_eq!(
        ex().await,
        Err(DiscordError::RateLimited {
            retry_after: Duration::from_secs(9)
        })
    );
    s.on(
        "POST /api/v10/oauth2/token",
        401,
        vec![],
        r#"{"error":"invalid_client"}"#,
    );
    assert_eq!(ex().await, Err(DiscordError::Status(401)));
    s.on("POST /api/v10/oauth2/token", 200, vec![], "not json ACCESS");
    assert_eq!(ex().await, Err(DiscordError::Malformed));
}

#[tokio::test]
async fn rest_exchange_code_user_endpoint_failure_does_not_leak_token() {
    let s = FakeServer::start().await;
    s.on(
        "POST /api/v10/oauth2/token",
        200,
        vec![],
        r#"{"access_token":"ACCESS-XYZ"}"#,
    );
    s.on(
        "GET /api/v10/users/@me",
        401,
        vec![],
        r#"{"message":"401: Unauthorized"}"#,
    );
    let err = s.api().exchange_code("c", "https://r").await.unwrap_err();
    assert_eq!(err, DiscordError::Status(401));
    assert!(!format!("{err} {err:?}").contains("ACCESS"));
}

// ---------------------------------------------------------------- check_discord_admin con el doble

fn role(id: &str, name: &str, perms: &str) -> Role {
    Role {
        id: id.into(),
        name: name.into(),
        permissions: perms.into(),
    }
}

fn member(roles: &[&str]) -> GuildMember {
    GuildMember {
        user_id: "42".into(),
        roles: roles.iter().map(|r| r.to_string()).collect(),
    }
}

#[tokio::test]
async fn check_admin_via_administrator_role() {
    let api = FakeDiscordApi::new()
        .with_member("1", "42", Ok(Some(member(&["9"]))))
        .with_roles(
            "1",
            Ok(vec![role("1", "@everyone", "0"), role("9", "Boss", "8")]),
        )
        .with_owner("1", Ok("7".into()));
    assert_eq!(
        check_discord_admin(&api, "1", "42", &AdminRoleSources::default()).await,
        Ok(true)
    );
    assert_eq!(
        api.calls(),
        ["guild_member:1:42", "guild_roles:1", "guild_owner_id:1"]
    );
}

#[tokio::test]
async fn check_admin_owner_and_non_member_and_errors() {
    let owner = FakeDiscordApi::new()
        .with_member("1", "42", Ok(Some(member(&[]))))
        .with_roles("1", Ok(vec![]))
        .with_owner("1", Ok("42".into()));
    assert_eq!(
        check_discord_admin(&owner, "1", "42", &AdminRoleSources::default()).await,
        Ok(true)
    );

    let absent = FakeDiscordApi::new().with_member("1", "42", Ok(None));
    assert_eq!(
        check_discord_admin(&absent, "1", "42", &AdminRoleSources::default()).await,
        Ok(false)
    );
    assert_eq!(absent.calls(), ["guild_member:1:42"]);

    let failing = FakeDiscordApi::new().with_member("1", "42", Err(DiscordError::Transport));
    assert_eq!(
        check_discord_admin(&failing, "1", "42", &AdminRoleSources::default()).await,
        Err(DiscordError::Transport)
    );
}

#[tokio::test]
async fn fake_api_is_usable_as_trait_object() {
    let api: Arc<dyn DiscordApi> = Arc::new(FakeDiscordApi::new().with_user(Ok(DiscordUser {
        id: "1".into(),
        username: "u".into(),
        global_name: None,
        avatar: None,
    })));
    assert_eq!(api.exchange_code("c", "r").await.unwrap().id, "1");
}

// ---------------------------------------------------------------- composicion con el proxy

async fn serve(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Compone como la Tarea 17: rutas de Rust + el proxy como fallback unico. El Node simulado
/// contesta `LEGACY <metodo> <ruta>` y registra cada peticion que recibe.
#[tokio::test]
async fn auth_routes_compose_with_proxy_callback_and_steam_reach_legacy() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = seen.clone();
    let legacy = Router::new().fallback(axum::routing::any(move |req: Request| {
        let s = s2.clone();
        async move {
            let line = format!("LEGACY {} {}", req.method(), req.uri().path());
            s.lock().unwrap().push(line.clone());
            line
        }
    }));
    let legacy_addr = serve(legacy).await;
    let proxy = web::proxy::fallback_proxy(format!("http://{legacy_addr}"), Default::default());
    let front = serve(app().merge(proxy)).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Estas llegan al legacy.
    for (method, path_q, path) in [
        (
            reqwest::Method::GET,
            "/auth/discord/callback?code=x&state=/",
            "/auth/discord/callback",
        ),
        (reqwest::Method::GET, "/auth/steam", "/auth/steam"),
        (
            reqwest::Method::GET,
            "/auth/steam/callback",
            "/auth/steam/callback",
        ),
        (
            reqwest::Method::POST,
            "/auth/demo-login",
            "/auth/demo-login",
        ),
    ] {
        let res = client
            .request(method.clone(), format!("http://{front}{path_q}"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{path_q}");
        assert_eq!(
            res.text().await.unwrap(),
            format!("LEGACY {method} {path}"),
            "{path_q}"
        );
    }
    assert_eq!(seen.lock().unwrap().len(), 4);

    // Estas las contesta Rust y el legacy no ve nada.
    let res = client
        .get(format!("http://{front}/auth/discord"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 302);
    assert!(res.headers()["location"]
        .to_str()
        .unwrap()
        .starts_with("https://discord.com/oauth2/authorize?"));

    let res = client
        .get(format!("http://{front}/auth/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 302);
    assert_eq!(res.headers()["location"], "/");
    assert_eq!(res.headers().get_all("set-cookie").iter().count(), 2);

    assert_eq!(
        seen.lock().unwrap().len(),
        4,
        "el legacy no debe ver /auth/discord ni /auth/logout"
    );
}
