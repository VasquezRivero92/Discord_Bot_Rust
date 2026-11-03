use std::sync::Arc;

use axum::extract::Request;
use axum::response::Response;
use axum::Router;
use common::config::Config;
use data::ClaimsRepo;
use tower::ServiceExt;

pub mod discord;
pub mod middleware;
pub mod proxy;
pub mod routes;
pub mod session;
pub mod static_files;

/// Builds the application router. Unknown routes return 404.
pub fn app() -> Router {
    Router::new()
}

/// Builds the application router with every unmatched route reverse-proxied
/// to the Node legacy at `legacy_url`.
pub fn app_with_proxy(legacy_url: String) -> Router {
    app_with_proxy_options(legacy_url, proxy::ProxyOptions::default())
}

/// Like [`app_with_proxy`] with explicit [`proxy::ProxyOptions`].
pub fn app_with_proxy_options(legacy_url: String, options: proxy::ProxyOptions) -> Router {
    Router::new().fallback_service(proxy::fallback_proxy(legacy_url, options))
}

/// Everything [`build_app`] needs. Built in `main` (or in tests with fakes).
#[derive(Clone)]
pub struct AppDeps {
    pub config: Config,
    /// Source for `GET /api/reclamaciones/:code`. `None` (Firestore disabled)
    /// leaves that route on the proxy: no failing handler is registered.
    pub claims: Option<Arc<dyn ClaimsRepo>>,
}

/// Assembles the front door of machupeek.com.
///
/// Precedence, first match wins:
///
/// 1. migrated routes: `GET /api/ping`, `GET /api/products`,
///    `GET /api/reclamaciones/:code` (only with a claims source),
///    `GET /auth/discord`, `GET /auth/logout`;
/// 2. a wrong method on one of those paths (e.g. `POST /api/ping`) goes to
///    the proxy instead of axum's 405 (`method_not_allowed_fallback`);
/// 3. static files and SPA routes (`static_files::routes_with_fallback`);
/// 4. everything else is reverse-proxied to the Node legacy (including
///    `/auth/discord/callback`, `/auth/steam*`, `/auth/demo-login`, `/health`,
///    `/api/admin/*`, `/api/lobby/*` with its WebSocket, `/api/cs2/*`,
///    `/webhook*` and every POST of the public API).
///
/// `middleware::apply` wraps all of it (outermost), so security headers, the
/// HTTPS redirect, CORS, the rate limit and the JSON body limit apply to Rust
/// and proxied responses alike. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn build_app(deps: AppDeps) -> Router {
    let cfg = &deps.config;
    let proxy = proxy::fallback_proxy(
        cfg.legacy_url.clone(),
        proxy::ProxyOptions {
            append_peer_to_xff: cfg.proxy_append_peer_to_xff,
        },
    );

    let mut migrated = routes::diagnostic::router()
        .merge(routes::public_api::products_router())
        .merge(routes::auth::router(routes::auth::AuthState::from_config(
            cfg,
        )));
    if let Some(claims) = deps.claims {
        migrated = migrated.merge(routes::public_api::claims_router(claims));
    }

    let to_legacy = proxy.clone();
    let app = migrated
        .merge(static_files::routes_with_fallback(
            cfg.static_dir.clone(),
            proxy,
        ))
        // Applies to the routes registered so far, so it goes after every merge.
        .method_not_allowed_fallback(move |req: Request| {
            let to_legacy = to_legacy.clone();
            async move {
                let res: Response = to_legacy.oneshot(req).await.unwrap_or_else(|e| match e {});
                res
            }
        });

    middleware::apply(app, cfg)
}
