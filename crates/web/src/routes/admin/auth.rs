//! `requireAuth` del panel (`verifyAdminToken`, `middleware/auth.js:5-31`).
//!
//! El legacy hace exactamente esto y nada más:
//!
//! 1. token = cookie `auth_token` || cookie `admin_token`; sin token:
//!    401 `{"error":"No autenticado"}`;
//! 2. `jwt.verify(token, ADMIN_JWT_SECRET)`; si falla (firma, formato,
//!    caducidad): 401 `{"error":"Token inválido o expirado"}`;
//! 3. `role === 'global_admin' || role === 'admin' || isAdmin === true`; si no:
//!    403 `{"error":"No autorizado. Se requieren permisos de administrador."}`;
//! 4. `req.user = decoded`.
//!
//! No consulta Discord ni Firestore (no llama a `checkIsAdmin`): aquí tampoco.
//! Los pasos 1-3 son los de [`AdminUser`]; las diferencias deliberadas de ese
//! extractor frente a Node (si `auth_token` no es válido se prueba
//! `admin_token`; se prueban cookies repetidas hasta un tope) están
//! documentadas en [`crate::session`].

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{request::Parts, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::routes::json_response;
use crate::session::{AdminUser, SessionClaims, SessionRejection, SessionState};

/// Mensaje (nuevo, sin equivalente en el legacy) para un `guildId` que Express
/// convertiría en array u objeto o que en el cuerpo JSON no es una cadena.
pub const MSG_INVALID_GUILD_ID: &str = "guildId inválido";

/// Sesión de administrador válida (`req.user` tras `requireAuth`).
#[derive(Debug, Clone)]
pub struct AdminContext {
    pub user: SessionClaims,
}

/// Error de las rutas de administración, con el cuerpo JSON del legacy
/// (`{"error": "..."}`, `Content-Type: application/json; charset=utf-8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminError {
    /// 401/403 de `verifyAdminToken`.
    Session(SessionRejection),
    /// 403 con el mensaje concreto de cada ruta del legacy.
    Forbidden(&'static str),
    /// 400: `guildId` no representable como una sola cadena (ver
    /// [`super::resolve_target_guild_id`]).
    InvalidGuildId,
}

impl AdminError {
    pub fn status(self) -> StatusCode {
        match self {
            Self::Session(r) => r.status(),
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::InvalidGuildId => StatusCode::BAD_REQUEST,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Session(r) => r.message(),
            Self::Forbidden(m) => m,
            Self::InvalidGuildId => MSG_INVALID_GUILD_ID,
        }
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        json_response(self.status(), &json!({ "error": self.message() }))
    }
}

impl<S> FromRequestParts<S> for AdminContext
where
    S: Send + Sync,
    SessionState: FromRef<S>,
{
    type Rejection = AdminError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match AdminUser::from_request_parts(parts, state).await {
            Ok(AdminUser(user)) => Ok(AdminContext { user }),
            Err(rejection) => {
                log_rejection(parts, rejection);
                Err(AdminError::Session(rejection))
            }
        }
    }
}

/// Como los `logger.warn/error` del legacy, sin token ni query (solo método y ruta).
fn log_rejection(parts: &Parts, rejection: SessionRejection) {
    let method = &parts.method;
    let path = parts.uri.path();
    match rejection {
        SessionRejection::NotAuthenticated => {
            // El legacy no avisa del sondeo de sesión `GET /me`.
            if !(method == Method::GET && path == "/api/admin/me") {
                tracing::warn!("[AUTH] No se encontró token para {method} {path}");
            }
        }
        SessionRejection::InvalidToken => {
            tracing::warn!("[AUTH] Token inválido o expirado para {method} {path}");
        }
        SessionRejection::Forbidden => {
            tracing::warn!("[AUTH] Intento de acceso no autorizado a ruta admin: {method} {path}");
        }
    }
}
