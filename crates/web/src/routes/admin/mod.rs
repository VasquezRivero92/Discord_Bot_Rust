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
//! ([`simple_reads`], Tarea 7) solo se montan con [`AdminRepos`], es decir, con
//! Firestore configurado; sin él siguen en el proxy.

pub mod auth;
mod js;
mod qs;
pub mod simple_reads;
pub mod target_guild;

use std::sync::Arc;

use axum::{extract::FromRef, Router};
use data::{
    ClaimsRepo, FirestoreClaimsRepo, FirestoreClient, FirestoreLogsRepo, FirestoreMatchHistoryRepo,
    FirestoreMessagesRepo, FirestoreOrdersRepo, FirestoreSupportCallsRepo, LogsRepo,
    MatchHistoryRepo, MessagesRepo, OrdersRepo, SupportCallsRepo,
};

use crate::session::SessionState;

pub use auth::{AdminContext, AdminError};
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

/// Fuentes de datos de las rutas de administración migradas.
#[derive(Clone)]
pub struct AdminRepos {
    pub claims: Arc<dyn ClaimsRepo>,
    pub orders: Arc<dyn OrdersRepo>,
    pub logs: Arc<dyn LogsRepo>,
    pub support_calls: Arc<dyn SupportCallsRepo>,
    pub match_history: Arc<dyn MatchHistoryRepo>,
    pub messages: Arc<dyn MessagesRepo>,
}

impl AdminRepos {
    /// Todos los repos sobre una misma conexión a Firestore.
    pub fn firestore(client: &FirestoreClient) -> Self {
        Self {
            claims: Arc::new(FirestoreClaimsRepo::new(client.clone())),
            orders: Arc::new(FirestoreOrdersRepo::new(client.clone())),
            logs: Arc::new(FirestoreLogsRepo::new(client.clone())),
            support_calls: Arc::new(FirestoreSupportCallsRepo::new(client.clone())),
            match_history: Arc::new(FirestoreMatchHistoryRepo::new(client.clone())),
            messages: Arc::new(FirestoreMessagesRepo::new(client.clone())),
        }
    }
}

/// Rutas migradas de `/api/admin/*` (rutas completas, sin `nest`), sin
/// fallback propio: el llamador lo fusiona antes del proxy. Sin `repos`
/// (Firestore desactivado) no se monta ninguna lectura.
pub fn router(state: AdminState, repos: Option<AdminRepos>) -> Router {
    let mut router = routes().with_state(state.clone());
    if let Some(repos) = repos {
        router = router.merge(simple_reads::router(state.session, repos));
    }
    router
}

/// Rutas que solo necesitan la sesión (ninguna todavía; Tarea 8).
fn routes() -> Router<AdminState> {
    Router::new()
}
