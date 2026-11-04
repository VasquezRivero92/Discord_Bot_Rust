//! Tests de `web::routes::{diagnostic, public_api}`.
//!
//! Fuente (legacy, solo lectura): `index.js:399-408` (`/api/ping`), `routes/publicApi.js:108-121`
//! (`GET /products`) y `:209-223` (`GET /reclamaciones/:code`). `/health` (index.js:280) lee el
//! estado del gateway de Discord, asi que sigue en el proxy y no tiene ruta propia aqui.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::routing::any;
use axum::Router;
use data::{Claim, ClaimsRepo, DataError, InMemoryClaimsRepo};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;
use web::routes::{diagnostic, public_api};

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        String::from_utf8(bytes.to_vec()).unwrap(),
    )
}

async fn get_json(
    app: &Router,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let (s, h, b) = call(app, Method::GET, uri, headers).await;
    let v = serde_json::from_str(&b).unwrap_or_else(|e| panic!("cuerpo no es JSON ({e}): {b}"));
    (s, h, v)
}

// ---------------------------------------------------------------- /api/ping

#[tokio::test]
async fn ping_lists_cookie_names_and_has_admin_token_flag() {
    let app = diagnostic::router();
    let (status, headers, v) = get_json(
        &app,
        "/api/ping",
        &[(
            "cookie",
            "auth_token=aaa.bbb; admin_token=secretvalue; theme=dark; auth_token=again",
        )],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    assert_eq!(v["ok"], true);
    assert_eq!(v["cookies"], json!(["auth_token", "admin_token", "theme"]));
    assert_eq!(v["hasAdminToken"], true);
    assert_eq!(v["secure"], false);
    assert_eq!(v["protocol"], "http");
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "cookies",
            "hasAdminToken",
            "ok",
            "protocol",
            "secure",
            "timestamp"
        ]
    );

    let (_, _, v) = get_json(&app, "/api/ping", &[("cookie", "theme=dark; admin_token=")]).await;
    assert_eq!(v["hasAdminToken"], false);
    assert_eq!(v["cookies"], json!(["theme", "admin_token"]));

    let (_, _, v) = get_json(&app, "/api/ping", &[]).await;
    assert_eq!(v["cookies"], json!([]));
    assert_eq!(v["hasAdminToken"], false);
}

#[tokio::test]
async fn ping_orders_integer_like_cookie_names_first_like_js_objects() {
    let app = diagnostic::router();
    let (_, _, v) = get_json(&app, "/api/ping", &[("cookie", "b=1; 10=x; a=2; 2=y")]).await;
    assert_eq!(v["cookies"], json!(["2", "10", "b", "a"]));
}

#[tokio::test]
async fn ping_joins_multiple_cookie_headers() {
    let app = diagnostic::router();
    let (_, _, v) = get_json(
        &app,
        "/api/ping",
        &[("cookie", "a=1"), ("cookie", "admin_token=z")],
    )
    .await;
    assert_eq!(v["cookies"], json!(["a", "admin_token"]));
    assert_eq!(v["hasAdminToken"], true);
}

#[tokio::test]
async fn ping_protocol_and_secure_follow_x_forwarded_proto() {
    let app = diagnostic::router();
    for (hdr, proto, secure) in [
        (Some("https"), "https", true),
        (Some("https, http"), "https", true),
        (Some(" HTTPS "), "https", true),
        (Some("http, https"), "http", false),
        (Some("http"), "http", false),
        (None, "http", false),
    ] {
        let headers: Vec<(&str, &str)> = hdr
            .map(|h| vec![("x-forwarded-proto", h)])
            .unwrap_or_default();
        let (_, _, v) = get_json(&app, "/api/ping", &headers).await;
        assert_eq!(v["protocol"], proto, "hdr={hdr:?}");
        assert_eq!(v["secure"], secure, "hdr={hdr:?}");
    }
}

#[tokio::test]
async fn ping_never_prints_cookie_values() {
    let app = diagnostic::router();
    let (_, _, body) = call(
        &app,
        Method::GET,
        "/api/ping",
        &[(
            "cookie",
            "admin_token=TOP-SECRET-VALUE; auth_token=OTHER-SECRET",
        )],
    )
    .await;
    assert!(!body.contains("SECRET"), "{body}");
}

#[tokio::test]
async fn ping_timestamp_is_js_iso_string() {
    let app = diagnostic::router();
    let (_, _, v) = get_json(&app, "/api/ping", &[]).await;
    let ts = v["timestamp"].as_str().unwrap();
    let b = ts.as_bytes();
    // YYYY-MM-DDTHH:MM:SS.mmmZ
    assert_eq!(ts.len(), 24, "{ts}");
    for (i, c) in b.iter().enumerate() {
        match i {
            4 | 7 => assert_eq!(*c, b'-', "{ts}"),
            10 => assert_eq!(*c, b'T', "{ts}"),
            13 | 16 => assert_eq!(*c, b':', "{ts}"),
            19 => assert_eq!(*c, b'.', "{ts}"),
            23 => assert_eq!(*c, b'Z', "{ts}"),
            _ => assert!(c.is_ascii_digit(), "{ts}"),
        }
    }
}

// ------------------------------------------------------------ /api/products

fn embedded_products() -> Value {
    let text = include_str!("../src/routes/products.json");
    serde_json::from_str(text).unwrap()
}

fn repo_app() -> (Router, Arc<InMemoryClaimsRepo>) {
    let repo = Arc::new(InMemoryClaimsRepo::new());
    (public_api::router(repo.clone()), repo)
}

#[tokio::test]
async fn products_matches_node_catalog() {
    let (app, _) = repo_app();
    let (status, headers, v) = get_json(&app, "/api/products", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    assert_eq!(v["success"], true);
    assert_eq!(v["currency"], "PEN");
    assert_eq!(
        v["merchant"],
        json!({"name": "MachuPeek", "address": "Online", "phone": "+51 949 165 677", "email": "machupeek@gmail.com"})
    );
    assert_eq!(v["products"], embedded_products());
    assert_eq!(v.as_object().unwrap().len(), 4);

    // Contra una evaluacion fresca del array legado (si esta disponible).
    let legacy = std::path::Path::new("D:/Discord_Faceit/routes/publicApi.js");
    if !legacy.exists() {
        eprintln!(
            "SKIPPED (loud): {} no existe; solo se comparo con products.json",
            legacy.display()
        );
        return;
    }
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-products.mjs");
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("fresh.json");
    let status = std::process::Command::new("node")
        .arg(&script)
        .arg(legacy)
        .arg(&out)
        .status()
        .expect("node disponible");
    assert!(status.success());
    let fresh: Value = serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
    assert_eq!(
        v["products"], fresh,
        "products.json desactualizado: node tools/gen-products.mjs"
    );
}

// ---------------------------------------------------- /api/reclamaciones/:code

fn claim_doc() -> Value {
    json!({
        "claimCode": "LR-2026-123456",
        "consumer": {"name": "Ana", "docType": "DNI", "docNumber": "123", "email": "a@b.co",
                     "phone": "", "address": "", "isMinor": false, "parent": null},
        "goods": {"type": "SERVICIO", "amount": 0.0, "currency": "PEN", "description": "x"},
        "claim": {"type": "RECLAMO", "detail": "d", "request": "r"},
        "merchant": {"companyName": "MachuPeek", "address": "Online",
                     "email": "machupeek@gmail.com", "phone": "+51 949 165 677"},
        "legalNotice": "aviso",
        "submittedAt": "2026-11-02T10:00:00.000Z",
        "createdAt": {"_seconds": 1793613600, "_nanoseconds": 5000000},
        "createdAtIso": "2026-11-02T10:00:00.005Z",
        "status": "PENDIENTE",
        "responseDeadlineDays": 15,
        "futureField": {"k": [1, 2]}
    })
}

#[tokio::test]
async fn claim_by_code_returns_404_json_for_unknown() {
    let (app, _) = repo_app();
    let (status, headers, v) = get_json(&app, "/api/reclamaciones/LR-0000-000000", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    assert_eq!(
        v,
        json!({"error": "No se encontró ninguna reclamación con el código especificado."})
    );
}

#[tokio::test]
async fn claim_by_code_returns_public_fields_only() {
    let (app, repo) = repo_app();
    let claim: Claim = serde_json::from_value(claim_doc()).unwrap();
    repo.insert("LR-2026-123456", claim);
    repo.insert(
        "LR-2026-OTHER",
        serde_json::from_value(json!({"claimCode": "LR-2026-OTHER"})).unwrap(),
    );
    let (status, headers, v) = get_json(&app, "/api/reclamaciones/LR-2026-123456", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    assert!(headers.get(header::CACHE_CONTROL).is_none());
    // Exactamente `{ success: true, claim }` con el documento tal cual (database.js getClaim
    // devuelve doc.data()), sin campos anadidos ni de otros reclamos.
    assert_eq!(v.as_object().unwrap().len(), 2);
    assert_eq!(v["success"], true);
    assert_eq!(v["claim"], claim_doc());
}

#[tokio::test]
async fn claim_lookup_error_maps_to_legacy_500_without_internal_text() {
    struct Failing;
    #[async_trait]
    impl ClaimsRepo for Failing {
        async fn get_by_code(&self, _code: &str) -> Result<Option<Claim>, DataError> {
            Err(DataError::Firestore("INTERNAL-SECRET-DETAIL".into()))
        }
        async fn list(&self, _limit: u32, _status: Option<&str>) -> Result<Vec<Claim>, DataError> {
            Err(DataError::Firestore("INTERNAL-SECRET-DETAIL".into()))
        }
    }
    let app = public_api::router(Arc::new(Failing));
    let (status, _, body) = call(&app, Method::GET, "/api/reclamaciones/X", &[]).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!body.contains("SECRET"), "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"error": "Error al consultar la reclamación."})
    );
}

// ---------------------------------------------------------- proxied writes

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
async fn post_reclamaciones_is_proxied() {
    // Node simulado: contesta a todo con el metodo y la ruta que recibio.
    let legacy = Router::new().fallback(any(|req: Request| async move {
        format!("LEGACY {} {}", req.method(), req.uri().path())
    }));
    let legacy_addr = serve(legacy).await;

    // Composicion como la hara la Tarea 17: rutas de Rust + el proxy como fallback unico.
    let repo = Arc::new(InMemoryClaimsRepo::new());
    repo.insert(
        "LR-1",
        serde_json::from_value(json!({"claimCode": "LR-1"})).unwrap(),
    );
    let proxy = web::proxy::fallback_proxy(format!("http://{legacy_addr}"), Default::default());
    let app = public_api::router(repo)
        .merge(diagnostic::router())
        .merge(proxy);
    let front = serve(app).await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let post = client
        .post(format!("http://{front}/api/reclamaciones"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 200);
    assert_eq!(post.text().await.unwrap(), "LEGACY POST /api/reclamaciones");

    let post = client
        .post(format!("http://{front}/api/checkout/purchase"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(
        post.text().await.unwrap(),
        "LEGACY POST /api/checkout/purchase"
    );

    let get = client
        .get(format!("http://{front}/api/reclamaciones/LR-1"))
        .send()
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&get.text().await.unwrap()).unwrap();
    assert_eq!(v["claim"]["claimCode"], "LR-1");

    let ping = client
        .get(format!("http://{front}/api/ping"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let ping: Value = serde_json::from_str(&ping).unwrap();
    assert_eq!(ping["ok"], true);

    // Lo que Rust no migra sigue yendo al legado, incluido /health.
    let health = client
        .get(format!("http://{front}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.text().await.unwrap(), "LEGACY GET /health");
}
