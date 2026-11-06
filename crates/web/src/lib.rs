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
pub mod steam_avatars;

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
    /// Sources for the migrated `/api/admin/*` reads (Tasks 7 and 8). `None`
    /// (Firestore disabled) leaves those routes on the proxy.
    pub admin: Option<routes::admin::AdminRepos>,
    pub cs2: Option<Arc<cs2::Cs2ServerManager>>,
    pub lobby: Option<Arc<lobby::LobbyManager>>,
}

/// Assembles the front door of machupeek.com.
///
/// Precedence, first match wins:
///
/// 1. migrated routes: `GET /api/ping`, `GET /api/products`,
///    `GET /api/reclamaciones/:code` (only with a claims source),
///    `GET /auth/discord`, `GET /auth/logout`, and the migrated
///    `/api/admin/*` routes (`routes::admin::router`: the single-collection
///    reads `claims`, `claims/:code`, `orders`, `logs`, `support-calls`,
///    `messages` and `matches/history`, and the derived reads `stats`,
///    `users/backup` and `yape/transactions`, only with admin sources);
/// 2. a wrong method on one of those paths (e.g. `POST /api/ping`) goes to
///    the proxy instead of axum's 405 (`method_not_allowed_fallback`);
/// 3. static files and SPA routes (`static_files::routes_with_fallback`);
/// 4. everything else is reverse-proxied to the Node legacy (including
///    `/auth/discord/callback`, `/auth/steam*`, `/auth/demo-login`, `/health`,
///    `/api/admin/*`, `/api/lobby/*` with its WebSocket, `/api/cs2/*`,
///    `/webhook*` and every POST of the public API).
///
/// Around all of it go the security headers, the HTTPS redirect, CORS and the
/// JSON body limit (`middleware::apply_except_rate_limit`), for Rust and
/// proxied responses alike. The rate limit is ONE `middleware::RateLimit`
/// (one bucket per client key) applied to the migrated routes and to the proxy
/// only: whatever `static_files` answers itself (files, SPA and legal entry
/// routes) is exempt, like `express.static` running before `globalLimiter` in
/// the legacy; what `static_files` hands to the proxy is limited.
///
/// Deliberate differences from the legacy: static files also go through the
/// HTTPS redirect (the legacy served `express.static` before its redirect; no
/// effect behind Cloudflare), and WebSocket upgrades now pass through CORS, the
/// HTTPS redirect and the rate limit (the legacy `server.on('upgrade')`
/// bypassed every Express middleware). See `docs/runbook-corte.md` §1. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn build_app(deps: AppDeps) -> Router {
    let cfg = &deps.config;
    let limit = middleware::RateLimit::new(cfg);
    let proxy = proxy::fallback_proxy(
        cfg.legacy_url.clone(),
        proxy::ProxyOptions {
            append_peer_to_xff: cfg.proxy_append_peer_to_xff,
            first_byte_timeout: cfg.proxy_first_byte_timeout,
            ws_idle_timeout: cfg.ws_idle_timeout,
        },
    );
    let limited_proxy = limit.layer(proxy.clone());

    // Node's panel writes (still proxied) clear the caches of the migrated
    // reads, like the legacy `invalidateAdminUsersCache`/`invalidateYapeCache`.
    let admin_caches = deps.admin.as_ref().map(|a| a.caches.clone());
    let mut migrated = routes::diagnostic::router()
        .merge(routes::public_api::products_router())
        .merge(routes::auth::router(routes::auth::AuthState::from_config(
            cfg,
        )));
    if let Some(claims) = deps.claims {
        migrated = migrated.merge(routes::public_api::claims_router(claims));
    }
    // Historial público de partidas (`/historial`), con la misma fuente que el panel admin.
    if let Some(admin) = &deps.admin {
        migrated = migrated.merge(routes::public_api::matches_history_router(
            admin.match_history.clone(),
        ));
        migrated = migrated.merge(routes::public_api::match_detail_router(
            routes::public_api::MatchDetailState {
                docs: admin.docs.clone(),
                users: admin.users.clone(),
                avatars: Arc::new(steam_avatars::HttpSteamAvatars::new()),
            },
        ));
    }
    // `/api/admin/*` migradas (solo esas: el resto sigue en el proxy). Sin
    // secreto válido (imposible con `Config::from_map`) no se monta nada.
    match session::SessionState::from_config(cfg) {
        Ok(session) => {
            migrated = migrated.merge(routes::admin::router(
                routes::admin::AdminState { session },
                deps.admin,
            ));
        }
        Err(e) => tracing::warn!("admin routes not mounted ({e}): /api/admin stays on the proxy"),
    }
    if let Some(cs2_mgr) = deps.cs2 {
        migrated = migrated
            .merge(routes::cs2_api::router(routes::cs2_api::Cs2ApiState {
                manager: cs2_mgr.clone(),
            }))
            .merge(routes::webhooks::router(routes::webhooks::WebhooksState {
                cs2_manager: cs2_mgr,
            }));
    }
    if let Some(lobby_mgr) = deps.lobby {
        // Falla cerrado: sin una sesión válida el lobby rechaza todo token. Nunca
        // se usa una clave de respaldo conocida (permitiría forjar sesiones).
        let lobby_auth: Arc<dyn lobby::auth::SessionVerifier> =
            match session::SessionState::from_config(cfg) {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    tracing::error!("lobby: sin sesión válida, se rechaza todo token: {e}");
                    Arc::new(lobby::auth::DenyAllVerifier)
                }
            };
        migrated = migrated.merge(lobby::router(lobby::LobbyApiState {
            manager: lobby_mgr,
            auth: lobby_auth,
            uploads_dir: cfg.static_dir.join("uploads").join("chat"),
        }));
    }
    // Wrong method on a migrated path -> Node instead of axum's 405. Set before
    // the rate-limit layer so each such request is counted exactly once (by the
    // layer), hence the un-limited proxy here.
    let migrated = migrated.method_not_allowed_fallback(move |req: Request| {
        let proxy = proxy.clone();
        async move {
            let res: Response = proxy.oneshot(req).await.unwrap_or_else(|e| match e {});
            res
        }
    });
    let migrated = limit.layer(migrated);

    let app = migrated.merge(static_files::routes_with_fallback(
        cfg.static_dir.clone(),
        limited_proxy,
    ));
    let app = match admin_caches {
        Some(caches) => app.layer(axum::middleware::from_fn_with_state(
            caches,
            routes::admin::invalidate_after_legacy_writes,
        )),
        None => app,
    };

    middleware::apply_except_rate_limit(app, cfg)
}
