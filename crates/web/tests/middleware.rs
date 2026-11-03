use std::collections::HashMap;
use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::{ConnectInfo, Request},
    http::{header, HeaderValue, Method, StatusCode},
    response::Response,
    routing::{any, get, post},
    Router,
};
use common::config::Config;
use tower::ServiceExt;
use web::middleware::apply;

fn cfg(production: bool, cors: &str) -> Config {
    let vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", "s"),
        ("BASE_URL", "http://localhost:3000"),
        ("DISCORD_CLIENT_ID", "id"),
        ("DISCORD_CLIENT_SECRET", "secret"),
        ("DISCORD_TOKEN", "token"),
        ("DISCORD_GUILD_ID", "guild"),
        ("CORS_ORIGINS", cors),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let mut c = Config::from_map(&vars).unwrap();
    c.production = production;
    c
}

async fn drain(req: Request) -> Response {
    match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(b) => Response::new(Body::from(b.len().to_string())),
        Err(_) => Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .unwrap(),
    }
}

fn base() -> Router {
    Router::new()
        .route("/", get(|| async { "ok" }))
        .route("/health", get(|| async { "up" }))
        .route("/upload", post(drain))
        .route(
            "/powered",
            get(|| async {
                (
                    [("x-powered-by", "Express"), ("x-frame-options", "DENY")],
                    "x",
                )
            }),
        )
        .route("/any", any(|| async { "any" }))
}

fn router(production: bool, cors: &str) -> Router {
    apply(base(), &cfg(production, cors))
}

fn get_req(uri: &str) -> axum::http::request::Builder {
    Request::builder().method(Method::GET).uri(uri)
}

async fn body_string(res: Response) -> String {
    let b = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(b.to_vec()).unwrap()
}

// ---------- headers ----------

#[tokio::test]
async fn sets_permissions_policy_header() {
    let res = router(false, "")
        .oneshot(get_req("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()["permissions-policy"],
        "camera=(), microphone=(), geolocation=()"
    );
}

#[tokio::test]
async fn sets_helmet_defaults_without_csp_when_absent() {
    let res = router(false, "")
        .oneshot(get_req("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let h = res.headers();
    assert_eq!(
        h["strict-transport-security"],
        "max-age=31536000; includeSubDomains"
    );
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["x-frame-options"], "SAMEORIGIN");
    assert_eq!(h["x-dns-prefetch-control"], "off");
    assert_eq!(h["x-download-options"], "noopen");
    assert_eq!(h["x-permitted-cross-domain-policies"], "none");
    assert_eq!(h["x-xss-protection"], "0");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert_eq!(h["cross-origin-opener-policy"], "same-origin");
    assert_eq!(h["cross-origin-resource-policy"], "same-origin");
    assert_eq!(h["origin-agent-cluster"], "?1");
    assert!(h.get("content-security-policy").is_none());
}

#[tokio::test]
async fn existing_headers_untouched_and_x_powered_by_removed() {
    let res = router(false, "")
        .oneshot(get_req("/powered").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.headers().get_all("x-frame-options").iter().count(), 1);
    assert_eq!(res.headers()["x-frame-options"], "DENY");
    assert!(res.headers().get("x-powered-by").is_none());
}

// ---------- https redirect ----------

#[tokio::test]
async fn redirects_http_to_https_in_production() {
    let res = router(true, "")
        .oneshot(
            get_req("/health?x=1")
                .header(header::HOST, "example.com")
                .header("x-forwarded-proto", "http")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        res.headers()[header::LOCATION],
        "https://example.com/health?x=1"
    );
}

#[tokio::test]
async fn missing_forwarded_proto_in_production_redirects() {
    let res = router(true, "")
        .oneshot(
            get_req("/")
                .header(header::HOST, "example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);
}

#[tokio::test]
async fn no_redirect_when_forwarded_proto_https() {
    let res = router(true, "")
        .oneshot(
            get_req("/")
                .header(header::HOST, "example.com")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn no_redirect_outside_production() {
    let res = router(false, "")
        .oneshot(
            get_req("/")
                .header(header::HOST, "x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// ---------- cors ----------

#[tokio::test]
async fn cors_rejects_unlisted_origin() {
    let res = router(false, "https://ok.example")
        .oneshot(
            get_req("/")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_string(res).await,
        r#"{"error":"Origen CORS no permitido."}"#
    );
}

#[tokio::test]
async fn cors_allows_listed_origin_with_credentials() {
    let res = router(false, "https://ok.example, https://b.example")
        .oneshot(
            get_req("/")
                .header(header::ORIGIN, "https://b.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()["access-control-allow-origin"],
        "https://b.example"
    );
    assert_eq!(res.headers()["access-control-allow-credentials"], "true");
    assert_eq!(res.headers()["vary"], "Origin");
}

#[tokio::test]
async fn cors_without_origin_or_with_empty_list_allows() {
    let res = router(false, "https://ok.example")
        .oneshot(get_req("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get("access-control-allow-origin").is_none());

    let res = router(false, "")
        .oneshot(
            get_req("/")
                .header(header::ORIGIN, "https://any.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()["access-control-allow-origin"],
        "https://any.example"
    );
}

#[tokio::test]
async fn cors_preflight_is_answered() {
    let res = router(false, "https://ok.example")
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/upload")
                .header(header::ORIGIN, "https://ok.example")
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type,x-custom")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let h = res.headers();
    assert_eq!(h["access-control-allow-origin"], "https://ok.example");
    assert_eq!(h["access-control-allow-credentials"], "true");
    assert_eq!(
        h["access-control-allow-methods"],
        "GET,HEAD,PUT,PATCH,POST,DELETE"
    );
    assert_eq!(h["access-control-allow-headers"], "content-type,x-custom");
}

// ---------- rate limit ----------

fn with_cf(ip: &str) -> Request {
    get_req("/")
        .header("cf-connecting-ip", ip)
        .body(Body::empty())
        .unwrap()
}

async fn status_of(app: &Router, req: Request) -> StatusCode {
    app.clone().oneshot(req).await.unwrap().status()
}

async fn exhaust(app: &Router, mk: impl Fn() -> Request) {
    for i in 0..100 {
        assert_eq!(status_of(app, mk()).await, StatusCode::OK, "request {i}");
    }
}

#[tokio::test]
async fn rate_limit_returns_429_with_spanish_message_after_100() {
    let app = router(false, "");
    exhaust(&app, || with_cf("1.1.1.1")).await;
    let res = app.clone().oneshot(with_cf("1.1.1.1")).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(res.headers()["ratelimit-limit"], "100");
    assert_eq!(
        body_string(res).await,
        r#"{"error":"Demasiadas peticiones. Intenta de nuevo más tarde."}"#
    );
}

#[tokio::test]
async fn rate_limit_does_not_exempt_health() {
    let app = router(false, "");
    for _ in 0..100 {
        status_of(&app, get_req("/health").body(Body::empty()).unwrap()).await;
    }
    assert_eq!(
        status_of(&app, get_req("/health").body(Body::empty()).unwrap()).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn rate_limit_keys_on_cf_connecting_ip() {
    let app = router(false, "");
    exhaust(&app, || with_cf("1.1.1.1")).await;
    assert_eq!(
        status_of(&app, with_cf("1.1.1.1")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(status_of(&app, with_cf("2.2.2.2")).await, StatusCode::OK);
}

#[tokio::test]
async fn invalid_cf_connecting_ip_is_ignored() {
    let app = router(false, "");
    // clave inválida: cae a X-Forwarded-For
    let mk = || {
        get_req("/")
            .header("cf-connecting-ip", "not-an-ip")
            .header("x-forwarded-for", "9.9.9.9")
            .body(Body::empty())
            .unwrap()
    };
    exhaust(&app, mk).await;
    assert_eq!(status_of(&app, mk()).await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        status_of(
            &app,
            get_req("/")
                .header("x-forwarded-for", "9.9.9.9")
                .body(Body::empty())
                .unwrap()
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "same key as the invalid-CF requests"
    );
}

#[tokio::test]
async fn rate_limit_uses_rightmost_xff_and_spoofing_does_not_reset() {
    let app = router(false, "");
    let mk = |spoof: String| {
        get_req("/")
            .header("x-forwarded-for", format!("{spoof}, 7.7.7.7"))
            .body(Body::empty())
            .unwrap()
    };
    for i in 0..100 {
        assert_eq!(
            status_of(&app, mk(format!("10.0.0.{i}"))).await,
            StatusCode::OK
        );
    }
    assert_eq!(
        status_of(&app, mk("10.1.1.1".into())).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // otro cliente real no se ve afectado
    assert_eq!(
        status_of(
            &app,
            get_req("/")
                .header("x-forwarded-for", "10.1.1.1, 8.8.8.8")
                .body(Body::empty())
                .unwrap()
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn rate_limit_falls_back_to_peer_address() {
    let app = router(false, "");
    let mk = |a: &str| {
        let mut r = get_req("/").body(Body::empty()).unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(a.parse::<SocketAddr>().unwrap()));
        r
    };
    for _ in 0..100 {
        status_of(&app, mk("5.5.5.5:1000")).await;
    }
    assert_eq!(
        status_of(&app, mk("5.5.5.5:2000")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(status_of(&app, mk("6.6.6.6:1000")).await, StatusCode::OK);
}

#[tokio::test]
async fn rate_limiters_are_independent_per_apply() {
    let a = router(false, "");
    exhaust(&a, || with_cf("1.1.1.1")).await;
    let b = router(false, "");
    assert_eq!(status_of(&b, with_cf("1.1.1.1")).await, StatusCode::OK);
}

// ---------- body limit ----------

fn json_post(len_hdr: bool, size: usize, chunked: bool) -> Request {
    let data = vec![b'a'; size];
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/upload")
        .header(header::CONTENT_TYPE, "application/json");
    let body = if chunked {
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
            data.chunks(8192).map(|c| Ok(c.to_vec())).collect();
        Body::from_stream(futures_util::stream::iter(chunks))
    } else {
        if len_hdr {
            b = b.header(header::CONTENT_LENGTH, size);
        }
        Body::from(data)
    };
    b.body(body).unwrap()
}

#[tokio::test]
async fn body_over_64kb_returns_413() {
    let app = router(false, "");
    let res = app
        .clone()
        .oneshot(json_post(true, 65 * 1024, false))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body_string(res).await,
        r#"{"error":"Cuerpo demasiado grande"}"#
    );
}

#[tokio::test]
async fn body_over_64kb_without_content_length_returns_413() {
    let app = router(false, "");
    let res = app
        .clone()
        .oneshot(json_post(false, 65 * 1024, true))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body_string(res).await,
        r#"{"error":"Cuerpo demasiado grande"}"#
    );
    // sin cabecera Content-Length aunque el cuerpo no vaya en chunks
    let res = app
        .oneshot(json_post(false, 65 * 1024, false))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn json_body_within_limit_passes() {
    let app = router(false, "");
    let res = app
        .oneshot(json_post(true, 64 * 1024, false))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, (64 * 1024).to_string());
}

#[tokio::test]
async fn large_non_json_body_is_not_limited() {
    let size = 8 * 1024 * 1024;
    let app = router(false, "");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/upload")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(vec![0u8; size]))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, size.to_string());

    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        (0..size / 65536).map(|_| Ok(vec![0u8; 65536])).collect();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/upload")
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("multipart/form-data; boundary=x"),
        )
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, size.to_string());
}
