//! Panel de administración: `/api/admin/*` (legacy `routes/api.js`, montado en
//! `index.js:447` con `app.use('/api/admin', createApiRouter(...))`).
//!
//! Piezas comunes que usan las rutas migradas:
//!
//! * [`AdminContext`]: equivalente de `requireAuth = verifyAdminToken(ADMIN_JWT_SECRET)`
//!   (`routes/api.js:21`, `middleware/auth.js:5-31`).
//! * [`resolve_target_guild_id`]: equivalente de `resolveTargetGuildId(req)`
//!   (`routes/api.js:29-43`) junto con el `router.use` que rellena `req.guildId`
//!   (`routes/api.js:24-27`).
//! * [`ensure_manages_guild`]: la comprobación de servidor de
//!   `GET/POST /guilds/:guildId/settings` (`routes/api.js:2066` y `:2082`).
//!
//! # Rutas
//!
//! [`router`] solo define rutas ya migradas; todo lo demás bajo `/api/admin`
//! (escrituras, rutas que dependen de Discord o del estado de Node, ...) no casa
//! aquí y cae al proxy hacia Node. Las lecturas de una colección
//! ([`simple_reads`], Tarea 7) y las derivadas ([`derived_reads`], Tarea 8) solo
//! se montan con [`AdminRepos`], es decir, con Firestore configurado; sin él
//! siguen en el proxy.

pub mod auth;
pub mod derived_reads;
mod js;
mod qs;
mod route_cache;
pub mod settings;
pub mod simple_reads;
pub mod target_guild;

use std::sync::Arc;

use axum::{extract::FromRef, response::IntoResponse, routing::get, Json, Router};
use common::config::Config;
use data::{
    AdminDocsRepo, ClaimsRepo, DocSink, FirestoreAdminDocsRepo, FirestoreClaimsRepo,
    FirestoreClient, FirestoreGlobalSettingsRepo, FirestoreGuildSettingsRepo, FirestoreLogsRepo,
    FirestoreMatchHistoryRepo, FirestoreMessagesRepo, FirestoreOrdersRepo,
    FirestoreSupportCallsRepo, GlobalSettingsRepo, GuildSettingsRepo, LogsRepo, MatchHistoryRepo,
    MessagesRepo, OrdersRepo, SupportCallsRepo, WatcherHandle, GLOBAL_SETTINGS_TTL,
    GUILD_SETTINGS_TTL,
};

use crate::session::SessionState;

pub use auth::{AdminContext, AdminError};
pub use derived_reads::{invalidate_after_legacy_writes, AdminCaches};
pub use target_guild::{ensure_manages_guild, resolve_target_guild_id};

/// Estado de las rutas de administración.
#[derive(Debug, Clone)]
pub struct AdminState {
    pub session: SessionState,
}

impl FromRef<AdminState> for SessionState {
    fn from_ref(s: &AdminState) -> SessionState {
        s.session.clone()
    }
}

/// Fuentes de datos de las rutas de administración migradas, y las cachés
/// que deben compartir con la capa [`invalidate_after_legacy_writes`].
#[derive(Clone)]
pub struct AdminRepos {
    pub claims: Arc<dyn ClaimsRepo>,
    pub orders: Arc<dyn OrdersRepo>,
    pub logs: Arc<dyn LogsRepo>,
    pub support_calls: Arc<dyn SupportCallsRepo>,
    pub match_history: Arc<dyn MatchHistoryRepo>,
    pub messages: Arc<dyn MessagesRepo>,
    /// `getSettings()` (para `features` de `/stats`).
    pub global_settings: Arc<dyn GlobalSettingsRepo>,
    pub guild_settings: Arc<dyn GuildSettingsRepo>,
    /// Documentos completos: `/stats`, `/users/backup`, `/yape/transactions`.
    pub docs: Arc<dyn AdminDocsRepo>,
    pub doc_sink: Arc<dyn DocSink>,
    pub config: Arc<Config>,
    /// Cachés de `/stats` y `/yape/transactions`.
    pub caches: AdminCaches,
    /// Tareas que deben vivir tanto como los repos (watcher de los ajustes
    /// globales); soltar el último clon las detiene.
    pub watchers: Vec<Arc<WatcherHandle>>,
}

impl AdminRepos {
    /// Todos los repos sobre una misma conexión a Firestore. Lanza los watchers
    /// de `faceit_config/settings` y `guild_settings` (necesita un runtime de tokio).
    pub fn firestore(client: &FirestoreClient, config: Arc<Config>) -> Self {
        let global = Arc::new(FirestoreGlobalSettingsRepo::from_client(
            client.clone(),
            GLOBAL_SETTINGS_TTL,
        ));
        let global_watcher = Arc::new(global.clone().spawn_watcher());
        let guild_settings = Arc::new(FirestoreGuildSettingsRepo::from_client(
            client.clone(),
            GUILD_SETTINGS_TTL,
        ));
        let guild_watcher = Arc::new(guild_settings.clone().spawn_watcher());
        Self {
            claims: Arc::new(FirestoreClaimsRepo::new(client.clone())),
            orders: Arc::new(FirestoreOrdersRepo::new(client.clone())),
            logs: Arc::new(FirestoreLogsRepo::new(client.clone())),
            support_calls: Arc::new(FirestoreSupportCallsRepo::new(client.clone())),
            match_history: Arc::new(FirestoreMatchHistoryRepo::new(client.clone())),
            messages: Arc::new(FirestoreMessagesRepo::new(client.clone())),
            global_settings: global,
            guild_settings,
            docs: Arc::new(FirestoreAdminDocsRepo::new(client.clone())),
            doc_sink: Arc::new(client.clone()),
            config,
            caches: AdminCaches::default(),
            watchers: vec![global_watcher, guild_watcher],
        }
    }
}

/// Rutas migradas de `/api/admin/*` (rutas completas, sin `nest`), sin
/// fallback propio: el llamador lo fusiona antes del proxy. Sin `repos`
/// (Firestore desactivado) no se monta ninguna lectura.
pub fn router(state: AdminState, repos: Option<AdminRepos>) -> Router {
    let mut router = routes().with_state(state.clone());
    if let Some(repos) = repos {
        router = router
            .merge(simple_reads::router(state.session.clone(), repos.clone()))
            .merge(derived_reads::router(state.session.clone(), repos.clone()))
            .merge(settings::router(state.session, repos));
    }
    router
}

/// Rutas que solo necesitan la sesión.
fn routes() -> Router<AdminState> {
    Router::new().route("/api/admin/me", get(admin_me))
}

/// `GET /api/admin/me`: el panel lo usa para saber si hay sesión de administrador.
async fn admin_me(ctx: AdminContext) -> impl IntoResponse {
    Json(serde_json::json!({
        "user": {
            "userId": ctx.user.user_id,
            "username": ctx.user.username,
            "avatar": ctx.user.avatar,
            "role": ctx.user.role,
            "isAdmin": true,
            "servidor": ctx.user.servidor,
        }
    }))
}
