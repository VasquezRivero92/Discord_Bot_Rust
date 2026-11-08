use std::sync::Arc;

use axum::extract::Request;
use axum::response::Response;
use axum::Router;
use common::config::Config;
use data::{ClaimsRepo, DocSink};
use tower::ServiceExt;

pub mod discord;
pub mod match_discord;
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
    /// Cliente de Firestore para `POST /api/lobby/wallet/recharge-yape` y vinculación de
    /// cuentas FACEIT/Steam. `None` (Firestore deshabilitado) deja esas rutas en el proxy.
    pub firestore: Option<Arc<data::FirestoreClient>>,
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
    let halloween_sink: Option<Arc<dyn DocSink>> = deps
        .admin
        .as_ref()
        .map(|a| a.doc_sink.clone())
        .or_else(|| deps.firestore.clone().map(|fs| fs as Arc<dyn DocSink>));

    let mut migrated = routes::diagnostic::router()
        .merge(routes::public_api::products_router())
        .merge(routes::matchmaking::router())
        .merge(routes::halloween::router(halloween_sink))
        .merge(routes::auth::router(
            routes::auth::AuthState::from_config(cfg).with_firestore(deps.firestore.clone()),
        ));
    if let Some(claims) = deps.claims {
        migrated = migrated.merge(routes::public_api::claims_router(claims));
    }
    if let Some(fs) = &deps.firestore {
        migrated = migrated
            .merge(routes::public_api::checkout_router(fs.clone()))
            .merge(routes::public_api::claims_post_router(fs.clone()));
    }
    // Fotos de Steam del historial: una sola caché para el detalle y para `/api/lobby/steam-avatar(s)`.
    let steam_avatars: Arc<dyn steam_avatars::SteamAvatars> =
        Arc::new(steam_avatars::HttpSteamAvatars::new());
    migrated = migrated.merge(routes::public_api::steam_avatar_router(
        steam_avatars.clone(),
    ));
    // Historial público de partidas (`/historial`), con la misma fuente que el panel admin.
    if let Some(admin) = &deps.admin {
        migrated = migrated.merge(routes::public_api::matches_history_router(
            admin.match_history.clone(),
        ));
        migrated = migrated.merge(routes::public_api::match_detail_router(
            routes::public_api::MatchDetailState {
                docs: admin.docs.clone(),
                users: admin.users.clone(),
                avatars: steam_avatars,
                faceit: Arc::new(steam_avatars::HttpFaceitSteamIds::new(
                    cfg.faceit_api_key.clone(),
                )),
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
    // Canales de voz y anuncio de las partidas (sin token de bot: `None`, no se toca Discord).
    let match_discord = match_discord::MatchDiscord::new(&cfg.discord_token).map(Arc::new);
    let env = &cfg.env_defaults;
    let channels_env = match_discord::MatchChannelsConfig {
        matches_channel_ids: match_discord::parse_id_list(env.discord_matches_channel_id.as_deref()),
        waiting_room_ids: match_discord::parse_id_list(
            env.discord_waiting_room_channel_id.as_deref(),
        ),
        access_role_id: match_discord::parse_id_list(env.discord_match_access_role_id.as_deref())
            .into_iter()
            .next(),
    };
    // Lanzamiento de partidas con cobro de apuestas (antes en Node): necesita el servidor CS2,
    // el lobby, MySQL (cobro atómico) y una sesión válida. Sin alguno, la ruta sigue en el proxy.
    if let (Some(cs2_mgr), Some(lobby_mgr), Some(fs), Ok(session)) = (
        deps.cs2.clone(),
        deps.lobby.clone(),
        deps.firestore.clone(),
        session::SessionState::from_config(cfg),
    ) {
        migrated = migrated.merge(routes::match_launch::router(
            routes::match_launch::MatchLaunchState {
                lobby: lobby_mgr,
                cs2: cs2_mgr,
                firestore: fs,
                auth: Arc::new(session),
                launching: Default::default(),
                discord: match_discord.clone(),
                channels_env,
            },
        ));
    }
    if let Some(cs2_mgr) = deps.cs2 {
        // Mismo interruptor de pagos y mismo secreto para MatchZy y para el plugin CS2: ambos
        // pueden declarar ganadores.
        let payout_enabled = std::env::var("MATCH_PAYOUT_ENABLED").is_ok_and(|v| v == "1");
        let cs2_secret = std::env::var("MATCHZY_WEBHOOK_SECRET")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        migrated = migrated
            .merge(routes::cs2_api::router(routes::cs2_api::Cs2ApiState {
                manager: cs2_mgr.clone(),
                firestore: deps.firestore.clone(),
                secret: cs2_secret.clone(),
                payout_enabled,
                discord: match_discord.clone(),
            }))
            .merge(routes::webhooks::router(routes::webhooks::WebhooksState {
                cs2_manager: cs2_mgr,
                firestore: deps.firestore.clone(),
                payout_enabled,
                matchzy_secret: cs2_secret,
                discord: match_discord,
            }));
    }
    if let Some(lobby_mgr) = deps.lobby.clone() {
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
    // Recarga Yape y vinculación de cuentas FACEIT/Steam (antes en Node). Mismo verificador de
    // sesión que el lobby; sin sesión válida no se monta y la ruta sigue en el proxy.
    if let Some(firestore) = deps.firestore {
        match session::SessionState::from_config(cfg) {
            Ok(s) => {
                let auth: Arc<dyn lobby::auth::SessionVerifier> = Arc::new(s.clone());
                migrated = migrated
                    .merge(routes::wallet::router(routes::wallet::WalletState {
                        firestore: firestore.clone(),
                        auth,
                        lobby: deps.lobby.clone(),
                        default_guild_id: cfg.discord_guild_id.clone(),
                    }))
                    .merge(routes::account::router(routes::account::AccountState::new(
                        firestore,
                        s,
                        cfg.faceit_api_key.clone(),
                        cfg.base_url.clone(),
                        cfg.discord_guild_id.clone(),
                    )));
            }
            Err(e) => tracing::warn!("wallet/account routes not mounted ({e}): stay on the proxy"),
        }
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
