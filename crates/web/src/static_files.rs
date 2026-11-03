//! Static files and public SPA routes, replicating the legacy Express app
//! (`D:\Discord_Faceit\index.js`: `express.static('public')` at 156-166, SPA/legal/halloween
//! routes at 458-575 and the final `GET *` at 577).
//!
//! Resolution order (same as Express):
//! 1. non-GET/HEAD requests and `/api`, `/auth`, `/webhook*` paths go to the caller's fallback;
//! 2. a file (or directory index) under `static_dir`;
//! 3. the legacy `app.get` routes (admin, legal, public SPA, halloween);
//! 4. the legacy `GET *` rule (index.html unless excluded);
//! 5. anything else goes to the caller's fallback (the proxy).

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower::{Service, ServiceExt};
use tower_http::services::{ServeDir, ServeFile};

const NO_STORE: &str = "no-cache, no-store, must-revalidate";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const PUBLIC_1H: &str = "public, max-age=3600";
const ADMIN_PRIVATE: &str = "no-store, no-cache, must-revalidate, private";

/// Cache policy a legacy route sets before `res.sendFile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Policy {
    /// `no-cache, no-store, must-revalidate` + `Pragma: no-cache` + `Expires: 0`.
    NoStoreFull,
    /// `no-cache, no-store, must-revalidate` only.
    NoStore,
    /// `no-store, no-cache, must-revalidate, private`.
    AdminPrivate,
    /// `public, max-age=3600`.
    Public1h,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    file: &'static str,
    policy: Policy,
}

const fn t(file: &'static str, policy: Policy) -> Option<Target> {
    Some(Target { file, policy })
}

/// Legacy `/halloween/user:id` and `/halloween/:id([1-8])` (index.js:564).
fn is_halloween_user(rest: &str) -> bool {
    if let Some(id) = rest.strip_prefix("user") {
        return !id.is_empty() && !id.contains('/');
    }
    matches!(rest.as_bytes(), [b'1'..=b'8'])
}

/// Which file (if any) the legacy Express routes serve for a GET path, after
/// `express.static` missed. Matching mimics Express defaults: case-insensitive and
/// one optional trailing slash; the raw (not percent-decoded) path is matched.
fn spa_target(path: &str) -> Option<Target> {
    use Policy::*;
    let lower = path.to_ascii_lowercase();
    let trimmed = if lower.len() > 1 {
        lower.strip_suffix('/').unwrap_or(&lower)
    } else {
        lower.as_str()
    };

    // index.js:458-465
    if trimmed == "/admin" || lower.starts_with("/admin/") {
        return t("admin/index.html", AdminPrivate);
    }
    // index.js:471-483
    match trimmed {
        "/terms" | "/terms-of-service" | "/legal/terms" => return t("terms.html", Public1h),
        "/privacy" | "/privacy-policy" | "/legal/privacy" => return t("privacy.html", Public1h),
        "/support" | "/help" | "/legal/support" => return t("support.html", Public1h),
        _ => {}
    }
    // index.js:490-513 (publicSpaRoutes)
    const SPA_EXACT: &[&str] = &[
        "/",
        "/tienda",
        "/productos",
        "/contacto",
        "/terminos",
        "/terminos-y-condiciones",
        "/devoluciones",
        "/politica-de-devoluciones",
        "/reclamaciones",
        "/libro-de-reclamaciones",
        "/arena",
        "/draft",
        "/mixton",
        "/checkout",
    ];
    const SPA_PREFIX: &[&str] = &[
        "/tienda/",
        "/productos/",
        "/libro-de-reclamaciones/",
        "/mixton/",
    ];
    if SPA_EXACT.contains(&trimmed) || SPA_PREFIX.iter().any(|p| lower.starts_with(p)) {
        return t("index.html", NoStoreFull);
    }
    // index.js:516-526
    if trimmed == "/halloween" {
        return t("halloween/index.html", Public1h);
    }
    if trimmed == "/halloween/admin" {
        return t("halloween/admin.html", NoStore);
    }
    // index.js:564-571
    if let Some(rest) = trimmed.strip_prefix("/halloween/") {
        if is_halloween_user(rest) {
            return t("halloween/user.html", Public1h);
        }
    }
    // index.js:577-593 (`GET *`)
    if lower.starts_with("/halloween")
        || path.starts_with("/api")
        || path.starts_with("/auth")
        || path.starts_with("/admin")
        || path.starts_with("/webhook")
        || path.starts_with("/assets")
        || path.contains('.')
    {
        return None;
    }
    t("index.html", NoStoreFull)
}

/// Paths owned by the Node API/auth/webhook routers: never answered here.
fn is_api_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("/api") || lower.starts_with("/auth") || lower.starts_with("/webhook")
}

fn set(res: &mut Response, name: header::HeaderName, value: &'static str) {
    res.headers_mut()
        .insert(name, HeaderValue::from_static(value));
}

fn apply_policy(res: &mut Response, policy: Policy) {
    match policy {
        Policy::NoStoreFull => {
            set(res, header::CACHE_CONTROL, NO_STORE);
            set(res, header::PRAGMA, "no-cache");
            set(res, header::EXPIRES, "0");
        }
        Policy::NoStore => set(res, header::CACHE_CONTROL, NO_STORE),
        Policy::AdminPrivate => set(res, header::CACHE_CONTROL, ADMIN_PRIVATE),
        Policy::Public1h => set(res, header::CACHE_CONTROL, PUBLIC_1H),
    }
}

/// `express.static` `setHeaders` (index.js:157-165), keyed on the request path.
fn apply_static_headers(res: &mut Response, path: &str) {
    if path.ends_with(".html") || path.ends_with('/') {
        // A trailing slash means a directory request answered with its index.html.
        apply_policy(res, Policy::NoStoreFull);
    } else if path.contains("assets") {
        set(res, header::CACHE_CONTROL, IMMUTABLE);
    }
}

fn is_ok_like(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
    )
}

/// Copy of a body-less request (GET/HEAD only ever reach here).
fn duplicate(req: &Request<Body>) -> Request<Body> {
    let mut b = Request::builder()
        .method(req.method().clone())
        .uri(req.uri().clone())
        .version(req.version());
    if let Some(h) = b.headers_mut() {
        *h = req.headers().clone();
    }
    b.body(Body::empty()).expect("valid request copy")
}

struct Inner<F> {
    dir: PathBuf,
    serve_dir: ServeDir,
    fallback: F,
}

async fn handle<F>(inner: Arc<Inner<F>>, req: Request<Body>) -> Response
where
    F: Service<Request<Body>, Response = Response, Error = Infallible> + Clone,
{
    let fallback = inner.fallback.clone();
    let path = req.uri().path().to_owned();
    let is_read = matches!(*req.method(), Method::GET | Method::HEAD);

    if !is_read || is_api_path(&path) {
        return fallback.oneshot(req).await.unwrap_or_else(|e| match e {});
    }

    // 2. static files / directory index
    let probe = duplicate(&req);
    let res = inner
        .serve_dir
        .clone()
        .oneshot(probe)
        .await
        .unwrap_or_else(|e| match e {});
    if res.status() != StatusCode::NOT_FOUND {
        let mut res = res.map(Body::new);
        if is_ok_like(res.status()) {
            apply_static_headers(&mut res, &path);
        }
        return res;
    }

    // 3/4. legacy routes + `GET *`
    if let Some(target) = spa_target(&path) {
        let res = ServeFile::new(inner.dir.join(target.file))
            .oneshot(duplicate(&req))
            .await
            .unwrap_or_else(|e| match e {});
        if res.status() != StatusCode::NOT_FOUND {
            let mut res = res.map(Body::new);
            if is_ok_like(res.status()) {
                apply_policy(&mut res, target.policy);
            }
            return res;
        }
    }

    // 5. not ours
    fallback.oneshot(req).await.unwrap_or_else(|e| match e {})
}

/// Static files and SPA routes; anything that is not ours answers `404 Not Found`.
pub fn routes(static_dir: PathBuf) -> Router {
    routes_with_fallback(
        static_dir,
        tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, Infallible>(StatusCode::NOT_FOUND.into_response())
        }),
    )
}

/// Static files and SPA routes. Requests that the legacy would not answer here
/// (non GET/HEAD, `/api*`, `/auth*`, `/webhook*`, missing files, excluded SPA paths)
/// are handed to `fallback` untouched (typically the reverse proxy to Node).
///
/// The returned router has its own fallback, so it must not be merged with another
/// router that sets one; mount the proxy through this parameter instead.
pub fn routes_with_fallback<F>(static_dir: PathBuf, fallback: F) -> Router
where
    F: Service<Request<Body>, Response = Response, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    F::Future: Send + 'static,
{
    let inner = Arc::new(Inner {
        serve_dir: ServeDir::new(&static_dir).append_index_html_on_directories(true),
        dir: static_dir,
        fallback,
    });
    Router::new().fallback(move |req: Request<Body>| {
        let inner = inner.clone();
        async move { handle(inner, req).await }
    })
}
