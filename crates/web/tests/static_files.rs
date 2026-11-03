//! Tests for `web::static_files`.
//!
//! Source of truth for the legacy route list (D:\Discord_Faceit\index.js, read-only):
//!
//! * 156-166  `express.static('public')`: runs FIRST. `.html` -> no-cache/no-store + Pragma + Expires;
//!   path including `assets` -> `public, max-age=31536000, immutable`.
//! * 458-465  `/admin`, `/admin/*`                       -> admin/index.html (no-store, private)
//! * 471      `/terms`, `/terms-of-service`, `/legal/terms`       -> terms.html   (public, max-age=3600)
//! * 476      `/privacy`, `/privacy-policy`, `/legal/privacy`     -> privacy.html (public, max-age=3600)
//! * 481      `/support`, `/help`, `/legal/support`               -> support.html (public, max-age=3600)
//! * 490-508  publicSpaRoutes -> index.html (no-cache/no-store + Pragma + Expires):
//!   `/` `/tienda` `/tienda/*` `/productos` `/productos/*` `/contacto` `/terminos`
//!   `/terminos-y-condiciones` `/devoluciones` `/politica-de-devoluciones` `/reclamaciones`
//!   `/libro-de-reclamaciones` `/libro-de-reclamaciones/*` `/arena` `/draft` `/mixton` `/mixton/*` `/checkout`
//! * 516      `/halloween`, `/Halloween` (+ trailing slash)       -> halloween/index.html (public, max-age=3600)
//! * 522      `/halloween/admin`, `/Halloween/admin` (+ slash)    -> halloween/admin.html (no-cache, no-store, must-revalidate)
//! * 564-571  `/halloween/user:id`, `/Halloween/user:id`, `/halloween/User:id`, `/Halloween/User:id`,
//!   `/halloween/:id([1-8])`, `/Halloween/:id([1-8])`            -> halloween/user.html (public, max-age=3600)
//! * 577-593  `GET *`: skipped (next) when lower(path) starts with `/halloween`, or path starts with
//!   `/api` `/auth` `/admin` `/webhook` `/webhooks` `/assets`, or path contains `.`;
//!   otherwise index.html (no-cache/no-store + Pragma + Expires).
//! * Express routing is case-insensitive and tolerant of one trailing slash.

use std::convert::Infallible;
use std::fs;
use std::path::Path;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use tower::util::service_fn;
use tower::ServiceExt;
use web::static_files::{routes, routes_with_fallback};

const NO_STORE: &str = "no-cache, no-store, must-revalidate";

type Resp = (StatusCode, HeaderMap, String);

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    app: Router,
}

fn marker_app(dir: &Path) -> Router {
    let fb = service_fn(|_req: Request<Body>| async {
        Ok::<_, Infallible>((StatusCode::IM_A_TEAPOT, "FALLBACK").into_response())
    });
    routes_with_fallback(dir.to_path_buf(), fb)
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let public = tmp.path().join("public");
    write(&public, "index.html", "INDEX");
    write(&public, "admin/index.html", "ADMIN");
    write(&public, "terms.html", "TERMS");
    write(&public, "privacy.html", "PRIVACY");
    write(&public, "support.html", "SUPPORT");
    write(&public, "halloween/index.html", "HALLOWEEN");
    write(&public, "halloween/admin.html", "HALLOWEEN_ADMIN");
    write(&public, "halloween/user.html", "HALLOWEEN_USER");
    write(&public, "assets/app.js", "console.log(1)");
    write(&public, "assets/style.css", "body{}");
    write(&public, "logo.svg", "<svg/>");
    write(&public, "pic.png", "PNG");
    write(&public, "data.json", "{}");
    write(&public, "robots.txt", "User-agent: *");
    write(&public, "assets/app.min.js", "min");
    write(&public, ".secret", "DOTSECRET");
    write(&public, ".hidden/x.txt", "DOTSECRET");
    write(&public, "sub/.env", "DOTSECRET");
    write(&public, ".well-known/x.txt", "DOTSECRET");
    fs::create_dir_all(public.join("emptydir")).unwrap();
    write(tmp.path(), "secret.txt", "SECRET-OUTSIDE");
    let app = marker_app(&public);
    Fixture { _tmp: tmp, app }
}

async fn send(app: &Router, method: Method, uri: &str, headers: &[(&str, &str)]) -> Resp {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = res.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn get(app: &Router, uri: &str) -> Resp {
    send(app, Method::GET, uri, &[]).await
}

fn cc(h: &HeaderMap) -> &str {
    h.get(header::CACHE_CONTROL)
        .map(|v| v.to_str().unwrap())
        .unwrap_or("")
}

fn is_fallback(r: &Resp) -> bool {
    r.0 == StatusCode::IM_A_TEAPOT && r.2 == "FALLBACK"
}

#[tokio::test]
async fn html_has_no_store_cache_control() {
    let f = fixture();
    for uri in ["/index.html", "/terms.html", "/admin/index.html", "/"] {
        let (s, h, _) = get(&f.app, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        assert_eq!(cc(&h), NO_STORE, "{uri}");
        assert_eq!(h.get("pragma").unwrap(), "no-cache", "{uri}");
        assert_eq!(h.get("expires").unwrap(), "0", "{uri}");
    }
}

#[tokio::test]
async fn assets_have_immutable_cache_control() {
    let f = fixture();
    for (uri, ct) in [
        ("/assets/app.js", "javascript"),
        ("/assets/style.css", "text/css"),
    ] {
        let (s, h, _) = get(&f.app, uri).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(cc(&h), "public, max-age=31536000, immutable", "{uri}");
        assert!(h
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains(ct));
    }
}

#[tokio::test]
async fn admin_subpath_serves_admin_index() {
    let f = fixture();
    for uri in [
        "/admin/",
        "/admin/users",
        "/admin/a/b/c",
        "/Admin/users",
        "/ADMIN/x/",
    ] {
        let (s, h, b) = get(&f.app, uri).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "ADMIN"), "{uri}");
        assert!(cc(&h).contains("no-"), "{uri}");
    }
    // Sub-paths served by the route (not by static) carry the private no-store header.
    let (_, h, _) = get(&f.app, "/admin/users").await;
    assert_eq!(cc(&h), "no-store, no-cache, must-revalidate, private");
    // `/admin` is a directory: Express static redirects to `/admin/` first
    // (tower-http answers 307, Express 301).
    let (s, h, _) = get(&f.app, "/admin").await;
    assert!(s.is_redirection());
    assert_eq!(h.get(header::LOCATION).unwrap(), "/admin/");
}

#[tokio::test]
async fn terms_aliases_serve_terms_html() {
    let f = fixture();
    let cases = [
        ("/terms", "TERMS"),
        ("/terms-of-service", "TERMS"),
        ("/legal/terms", "TERMS"),
        ("/Terms", "TERMS"),
        ("/terms/", "TERMS"),
        ("/privacy", "PRIVACY"),
        ("/privacy-policy", "PRIVACY"),
        ("/legal/privacy", "PRIVACY"),
        ("/support", "SUPPORT"),
        ("/help", "SUPPORT"),
        ("/legal/support/", "SUPPORT"),
    ];
    for (uri, body) in cases {
        let (s, h, b) = get(&f.app, uri).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, body), "{uri}");
        assert_eq!(cc(&h), "public, max-age=3600", "{uri}");
    }
}

#[tokio::test]
async fn public_spa_routes_serve_index() {
    let f = fixture();
    let routes = [
        "/",
        "/tienda",
        "/tienda/",
        "/tienda/a/b",
        "/productos",
        "/productos/x",
        "/contacto",
        "/terminos",
        "/terminos-y-condiciones",
        "/devoluciones",
        "/politica-de-devoluciones",
        "/reclamaciones",
        "/libro-de-reclamaciones",
        "/libro-de-reclamaciones/x",
        "/arena",
        "/draft",
        "/mixton",
        "/mixton/x/y",
        "/checkout",
        "/Tienda",
        "/CHECKOUT/",
        // GET * fallback (index.js:577): any other non-excluded path
        "/perfil",
        "/perfil/jugador/5",
        "/Assets/x",
    ];
    for uri in routes {
        let (s, h, b) = get(&f.app, uri).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "INDEX"), "{uri}");
        assert_eq!(cc(&h), NO_STORE, "{uri}");
        assert_eq!(h.get("pragma").unwrap(), "no-cache", "{uri}");
        assert_eq!(h.get("expires").unwrap(), "0", "{uri}");
    }
}

#[tokio::test]
async fn halloween_case_variants_and_trailing_slash() {
    let f = fixture();
    // `/halloween/` is answered by static (directory index), like express.static.
    for uri in ["/halloween/", "/Halloween/", "/HALLOWEEN/"] {
        let (s, _, b) = get(&f.app, uri).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "HALLOWEEN"), "{uri}");
    }
    // Without slash: static dir redirect (Express 301, tower-http 307) to the slash form.
    // Lowercase always matches the directory on disk -> redirect.
    let (s, h, _) = get(&f.app, "/halloween").await;
    assert!(s.is_redirection());
    assert_eq!(h.get(header::LOCATION).unwrap(), "/halloween/");
    // `/Halloween` depends on the filesystem: case-insensitive FS (Windows/macOS) matches the
    // directory and redirects; case-sensitive FS (Linux) misses static and the route answers 200.
    let (s, h, b) = get(&f.app, "/Halloween").await;
    let redirected = s.is_redirection() && h.get(header::LOCATION).unwrap() == "/Halloween/";
    let served = s == StatusCode::OK && b == "HALLOWEEN";
    assert!(redirected || served, "{s:?} {b}");
    // Admin: halloween/admin.html (this fixture has no halloween/admin/ directory).
    for uri in [
        "/halloween/admin",
        "/Halloween/admin",
        "/halloween/admin/",
        "/Halloween/admin/",
    ] {
        let (s, h, b) = get(&f.app, uri).await;
        assert_eq!(
            (s, b.as_str()),
            (StatusCode::OK, "HALLOWEEN_ADMIN"),
            "{uri}"
        );
        assert_eq!(cc(&h), NO_STORE, "{uri}");
    }
    // User pages.
    for uri in [
        "/halloween/1",
        "/Halloween/8",
        "/halloween/4/",
        "/halloween/user3",
        "/Halloween/User7",
        "/halloween/userabc",
    ] {
        let (s, h, b) = get(&f.app, uri).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "HALLOWEEN_USER"), "{uri}");
        assert_eq!(cc(&h), "public, max-age=3600", "{uri}");
    }
    // GET * skips every /halloween* path -> falls through (never index.html).
    for uri in [
        "/halloween/0",
        "/halloween/9",
        "/halloween/12",
        "/halloween/zzz",
        "/halloween/user",
    ] {
        assert!(is_fallback(&get(&f.app, uri).await), "{uri}");
    }
}

#[tokio::test]
async fn halloween_admin_directory_shadows_route_like_express_static() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "halloween/admin/index.html", "HADMIN_DIR");
    write(tmp.path(), "halloween/admin.html", "HADMIN_FILE");
    let app = marker_app(tmp.path());
    let (s, h, _) = get(&app, "/halloween/admin").await;
    assert!(s.is_redirection());
    assert_eq!(h.get(header::LOCATION).unwrap(), "/halloween/admin/");
    let (s, _, b) = get(&app, "/halloween/admin/").await;
    assert_eq!((s, b.as_str()), (StatusCode::OK, "HADMIN_DIR"));
}

#[tokio::test]
async fn unknown_api_path_falls_to_proxy_not_spa() {
    let f = fixture();
    for uri in [
        "/api/x",
        "/api",
        "/api/",
        "/API/x",
        "/apix",
        "/api/halloween/status",
        "/auth/login",
        "/auth",
        "/webhook/faceit",
        "/webhooks/faceit",
        "/api/index.html",
    ] {
        assert!(is_fallback(&get(&f.app, uri).await), "{uri}");
    }
    // Non-GET/HEAD is never answered here, whatever the path.
    for (m, uri) in [
        (Method::POST, "/api/x"),
        (Method::POST, "/terms"),
        (Method::PUT, "/admin/x"),
        (Method::DELETE, "/index.html"),
        (Method::POST, "/"),
        (Method::PATCH, "/assets/app.js"),
        (Method::OPTIONS, "/tienda"),
    ] {
        assert!(
            is_fallback(&send(&f.app, m.clone(), uri, &[]).await),
            "{m} {uri}"
        );
    }
}

#[tokio::test]
async fn missing_files_and_dotted_paths_fall_through() {
    let f = fixture();
    for uri in [
        "/nope.txt",
        "/assets/missing.js",
        "/assets/",
        "/assets/zz",
        "/a.b/c",
        "/favicon.ico",
    ] {
        let r = get(&f.app, uri).await;
        assert!(is_fallback(&r), "{uri}: {:?}", r.0);
    }
}

#[tokio::test]
async fn directory_without_slash_redirects_like_express_static() {
    // serve-static redirects any directory to `dir/` (301; tower-http uses 307).
    let f = fixture();
    for (uri, loc) in [("/assets", "/assets/"), ("/emptydir", "/emptydir/")] {
        let (s, h, _) = get(&f.app, uri).await;
        assert!(s.is_redirection(), "{uri}");
        assert_eq!(h.get(header::LOCATION).unwrap(), loc);
    }
}

#[tokio::test]
async fn head_requests_work() {
    let f = fixture();
    let (s, h, b) = send(&f.app, Method::HEAD, "/assets/app.js", &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(cc(&h), "public, max-age=31536000, immutable");
    assert!(b.is_empty());
    let (s, h, b) = send(&f.app, Method::HEAD, "/terms", &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(cc(&h), "public, max-age=3600");
    assert!(b.is_empty());
    let (s, _, b) = send(&f.app, Method::HEAD, "/perfil", &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.is_empty());
}

#[tokio::test]
async fn path_traversal_never_escapes_static_dir() {
    let f = fixture();
    for uri in [
        "/..%2fsecret.txt",
        "/..%2f..%2fsecret.txt",
        "/%2e%2e/secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/..%5csecret.txt",
        "/%2e%2e%5csecret.txt",
        "/assets/../../secret.txt",
        "/assets/..%2f..%2fsecret.txt",
        "/../secret.txt",
        "/assets/%2e%2e/%2e%2e/secret.txt",
        "/secret.txt",
        "/%00secret.txt",
    ] {
        let r = get(&f.app, uri).await;
        assert!(!r.2.contains("SECRET-OUTSIDE"), "{uri} leaked");
        assert!(is_fallback(&r), "{uri} -> {:?}", r.0);
    }
}

#[tokio::test]
async fn directory_without_index_is_not_listed() {
    let f = fixture();
    // No index: static misses; the legacy `GET *` then answers with the SPA index.html
    // (path has no dot and is not excluded), never with a listing.
    let r = get(&f.app, "/emptydir/").await;
    assert_eq!((r.0, r.2.as_str()), (StatusCode::OK, "INDEX"));
    let r = get(&f.app, "/assets/").await;
    assert!(is_fallback(&r));
    assert!(!r.2.contains("app.js"));
}

#[tokio::test]
async fn conditional_get_returns_304() {
    let f = fixture();
    let (s, h, _) = get(&f.app, "/assets/app.js").await;
    assert_eq!(s, StatusCode::OK);
    let lm = h
        .get(header::LAST_MODIFIED)
        .expect("Last-Modified")
        .to_str()
        .unwrap()
        .to_owned();
    let (s, h2, b) = send(
        &f.app,
        Method::GET,
        "/assets/app.js",
        &[("if-modified-since", &lm)],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_MODIFIED);
    assert!(b.is_empty());
    assert_eq!(cc(&h2), "public, max-age=31536000, immutable");

    let (_, h, _) = get(&f.app, "/terms.html").await;
    let lm = h
        .get(header::LAST_MODIFIED)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (s, h2, _) = send(
        &f.app,
        Method::GET,
        "/terms.html",
        &[("if-modified-since", &lm)],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_MODIFIED);
    assert_eq!(cc(&h2), NO_STORE);

    // SPA route files support it as well.
    let (_, h, _) = get(&f.app, "/terms").await;
    let lm = h
        .get(header::LAST_MODIFIED)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (s, _, _) = send(&f.app, Method::GET, "/terms", &[("if-modified-since", &lm)]).await;
    assert_eq!(s, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn content_types() {
    let f = fixture();
    for (uri, ct) in [
        ("/assets/app.js", "javascript"),
        ("/assets/style.css", "text/css"),
        ("/logo.svg", "image/svg+xml"),
        ("/pic.png", "image/png"),
        ("/data.json", "application/json"),
        ("/robots.txt", "text/plain"),
        ("/index.html", "text/html"),
        ("/terms", "text/html"),
        ("/perfil", "text/html"),
    ] {
        let (s, h, _) = get(&f.app, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        let v = h.get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(v.contains(ct), "{uri}: {v}");
    }
}

#[tokio::test]
async fn routes_default_fallback_is_404() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "index.html", "INDEX");
    let app = routes(tmp.path().to_path_buf());
    let (s, _, _) = get(&app, "/api/x").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, b) = get(&app, "/tienda").await;
    assert_eq!((s, b.as_str()), (StatusCode::OK, "INDEX"));
}

#[tokio::test]
async fn dotfiles_are_ignored_and_fall_through() {
    let f = fixture();
    for uri in [
        "/.secret",
        "/%2esecret",
        "/%2Esecret",
        "/.hidden/x.txt",
        "/%2ehidden/x.txt",
        "/sub/.env",
        "/sub/%2eenv",
        "/.well-known/x.txt",
    ] {
        let r = get(&f.app, uri).await;
        assert!(!r.2.contains("DOTSECRET"), "{uri} leaked");
        assert!(is_fallback(&r), "{uri} -> {:?}", r.0);
    }
    let (s, _, b) = get(&f.app, "/robots.txt").await;
    assert_eq!((s, b.as_str()), (StatusCode::OK, "User-agent: *"));
    let (s, h, b) = get(&f.app, "/assets/app.min.js").await;
    assert_eq!((s, b.as_str()), (StatusCode::OK, "min"));
    assert_eq!(cc(&h), "public, max-age=31536000, immutable");
}

#[tokio::test]
async fn health_is_not_shadowed_by_the_spa_catch_all() {
    // index.js:280 `app.get('/health')` runs before `GET *` (577), so Node answers it.
    // Express matching: case-insensitive, one optional trailing slash.
    let f = fixture();
    for uri in ["/health", "/health/", "/HEALTH", "/Health/", "/health?x=1"] {
        assert!(is_fallback(&get(&f.app, uri).await), "{uri}");
    }
    // Only that exact route: deeper paths still get the SPA like the legacy `GET *`.
    let r = get(&f.app, "/healthz").await;
    assert_eq!(r.2, "INDEX");
    let r = get(&f.app, "/health/x").await;
    assert_eq!(r.2, "INDEX");
}
