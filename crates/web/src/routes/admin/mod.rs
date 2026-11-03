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
//! aquí y cae al proxy hacia Node. En esta tarea aún no hay ninguna ruta.

pub mod auth;
pub mod target_guild;

use axum::{extract::FromRef, Router};

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

/// Rutas migradas de `/api/admin/*` (rutas completas, sin `nest`), sin
/// fallback propio: el llamador lo fusiona antes del proxy.
pub fn router(state: AdminState) -> Router {
    routes().with_state(state)
}

/// Las rutas se añaden aquí a medida que se migran (Tareas 7 y 8).
fn routes() -> Router<AdminState> {
    Router::new()
}
