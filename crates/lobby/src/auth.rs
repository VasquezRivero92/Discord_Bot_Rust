//! Módulo de autenticación y autorización para el crate `lobby`.
//!
//! Proporciona tipos de usuario verificado ([`LobbyUser`]), el trait [`SessionVerifier`]
//! y extractores de Axum ([`RequireUser`], [`RequireAdmin`], [`OptionalLobbyUser`])
//! para proteger los endpoints REST del lobby Mixton.

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Representación del usuario autenticado en el contexto del lobby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyUser {
    pub user_id: String,
    pub username: String,
    pub avatar: Option<String>,
    pub role: Option<String>,
    pub servidor: Option<String>,
    pub is_admin: bool,
}

impl LobbyUser {
    /// Determina si el usuario tiene privilegios administrativos equivalentes a admin
    /// según la regla de negocio del legado: `role == "global_admin" || role == "admin" || is_admin`.
    pub fn is_admin_like(&self) -> bool {
        matches!(self.role.as_deref(), Some("global_admin") | Some("admin")) || self.is_admin
    }
}

/// Trait para verificar firmas de tokens de sesión sin acoplar `lobby` a la implementación
/// criptográfica concreta de JWT.
pub trait SessionVerifier: Send + Sync {
    fn verify_token(&self, token: &str) -> Option<LobbyUser>;
}

/// Verificador que rechaza todo token. Se usa cuando no hay una sesión válida
/// configurada: el lobby falla cerrado en lugar de aceptar una clave de respaldo.
pub struct DenyAllVerifier;

impl SessionVerifier for DenyAllVerifier {
    fn verify_token(&self, _token: &str) -> Option<LobbyUser> {
        None
    }
}

/// Extrae de forma segura los valores de una cookie por su nombre a partir de las cabeceras HTTP.
fn extract_cookie_values(headers: &HeaderMap, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for h in headers.get_all(header::COOKIE) {
        let Ok(s) = h.to_str() else { continue };
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim() == name {
                    let v = v.trim().trim_matches('"');
                    if !v.is_empty() {
                        out.push(v.to_string());
                    }
                }
            }
        }
    }
    out
}

enum AuthOutcome {
    Missing,
    Invalid,
    Success(LobbyUser),
}

/// Intenta autenticar la petición contra el verificador de sesión.
/// Regla: cookie `auth_token` primero; si no existe o no es válida, `admin_token`.
fn authenticate(headers: &HeaderMap, verifier: &dyn SessionVerifier) -> AuthOutcome {
    let auth_tokens = extract_cookie_values(headers, "auth_token");
    let admin_tokens = extract_cookie_values(headers, "admin_token");

    if auth_tokens.is_empty() && admin_tokens.is_empty() {
        return AuthOutcome::Missing;
    }

    for token in &auth_tokens {
        if let Some(user) = verifier.verify_token(token) {
            return AuthOutcome::Success(user);
        }
    }

    for token in &admin_tokens {
        if let Some(user) = verifier.verify_token(token) {
            return AuthOutcome::Success(user);
        }
    }

    AuthOutcome::Invalid
}

/// Rechazos emitidos por los extractores de autenticación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyAuthRejection {
    NotAuthenticated,
    InvalidToken,
    Forbidden,
}

impl IntoResponse for LobbyAuthRejection {
    fn into_response(self) -> Response {
        match self {
            Self::NotAuthenticated => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "No autenticado" })),
            )
                .into_response(),
            Self::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Token inválido o expirado" })),
            )
                .into_response(),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Se requieren permisos de administrador." })),
            )
                .into_response(),
        }
    }
}

/// Extractor opcional de usuario: no rechaza peticiones no autenticadas ni con tokens inválidos.
#[derive(Debug, Clone)]
pub struct OptionalLobbyUser(pub Option<LobbyUser>);

impl std::ops::Deref for OptionalLobbyUser {
    type Target = Option<LobbyUser>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for OptionalLobbyUser
where
    S: Send + Sync,
    Arc<dyn SessionVerifier>: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let verifier = Arc::<dyn SessionVerifier>::from_ref(state);
        let outcome = authenticate(&parts.headers, verifier.as_ref());
        Ok(OptionalLobbyUser(match outcome {
            AuthOutcome::Success(user) => Some(user),
            _ => None,
        }))
    }
}

/// Extractor de usuario obligatorio: 401 si falta o falla el token.
#[derive(Debug, Clone)]
pub struct RequireUser(pub LobbyUser);

impl std::ops::Deref for RequireUser {
    type Target = LobbyUser;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for RequireUser
where
    S: Send + Sync,
    Arc<dyn SessionVerifier>: FromRef<S>,
{
    type Rejection = LobbyAuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let verifier = Arc::<dyn SessionVerifier>::from_ref(state);
        match authenticate(&parts.headers, verifier.as_ref()) {
            AuthOutcome::Success(user) => Ok(RequireUser(user)),
            AuthOutcome::Missing => Err(LobbyAuthRejection::NotAuthenticated),
            AuthOutcome::Invalid => Err(LobbyAuthRejection::InvalidToken),
        }
    }
}

/// Extractor de administrador obligatorio: 401 si falta/falla el token, 403 si no es admin-like.
#[derive(Debug, Clone)]
pub struct RequireAdmin(pub LobbyUser);

impl std::ops::Deref for RequireAdmin {
    type Target = LobbyUser;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
    Arc<dyn SessionVerifier>: FromRef<S>,
{
    type Rejection = LobbyAuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let verifier = Arc::<dyn SessionVerifier>::from_ref(state);
        match authenticate(&parts.headers, verifier.as_ref()) {
            AuthOutcome::Success(user) => {
                if user.is_admin_like() {
                    Ok(RequireAdmin(user))
                } else {
                    Err(LobbyAuthRejection::Forbidden)
                }
            }
            AuthOutcome::Missing => Err(LobbyAuthRejection::NotAuthenticated),
            AuthOutcome::Invalid => Err(LobbyAuthRejection::InvalidToken),
        }
    }
}

/// Función pura para resolver el servidor/guild efectivo para un usuario y validar permisos.
///
/// Si `user.role == "admin"` y `requested` es `Some` distinto de `user.servidor`,
/// devuelve un error descriptivo con el mensaje del legado:
/// - En ruta de administración: `"No tienes permisos para gestionar este servidor."`
/// - En ruta de acceso común: `"No tienes permisos para acceder a este servidor."`
///
/// Si `user.role == "admin"`, el guild efectivo forzado es `user.servidor`.
pub fn resolve_guild_for_user(
    user: &LobbyUser,
    requested: Option<&str>,
    admin_route: bool,
) -> Result<Option<String>, &'static str> {
    if user.role.as_deref() == Some("admin") {
        if let Some(req) = requested {
            if user.servidor.as_deref() != Some(req) {
                return Err(if admin_route {
                    "No tienes permisos para gestionar este servidor."
                } else {
                    "No tienes permisos para acceder a este servidor."
                });
            }
        }
        Ok(user.servidor.clone())
    } else {
        Ok(requested.map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockVerifier;
    impl SessionVerifier for MockVerifier {
        fn verify_token(&self, token: &str) -> Option<LobbyUser> {
            match token {
                "user-token" => Some(LobbyUser {
                    user_id: "u1".to_string(),
                    username: "NormalUser".to_string(),
                    avatar: None,
                    role: None,
                    servidor: None,
                    is_admin: false,
                }),
                "admin-guild-token" => Some(LobbyUser {
                    user_id: "u2".to_string(),
                    username: "GuildAdmin".to_string(),
                    avatar: None,
                    role: Some("admin".to_string()),
                    servidor: Some("guild-123".to_string()),
                    is_admin: false,
                }),
                "global-admin-token" => Some(LobbyUser {
                    user_id: "u3".to_string(),
                    username: "GlobalAdmin".to_string(),
                    avatar: None,
                    role: Some("global_admin".to_string()),
                    servidor: None,
                    is_admin: false,
                }),
                _ => None,
            }
        }
    }

    #[test]
    fn deny_all_verifier_rejects_every_token() {
        // Falla cerrado: aunque el token tenga forma válida, nunca autentica.
        let v = DenyAllVerifier;
        assert!(v.verify_token("").is_none());
        assert!(v.verify_token("a.b.c").is_none());
        assert!(v.verify_token("valid-looking-token").is_none());
    }

    #[test]
    fn test_is_admin_like() {
        let user_normal = LobbyUser {
            user_id: "1".into(),
            username: "A".into(),
            avatar: None,
            role: None,
            servidor: None,
            is_admin: false,
        };
        assert!(!user_normal.is_admin_like());

        let user_bool_admin = LobbyUser {
            is_admin: true,
            ..user_normal.clone()
        };
        assert!(user_bool_admin.is_admin_like());

        let user_role_admin = LobbyUser {
            role: Some("admin".into()),
            ..user_normal.clone()
        };
        assert!(user_role_admin.is_admin_like());

        let user_global_admin = LobbyUser {
            role: Some("global_admin".into()),
            ..user_normal.clone()
        };
        assert!(user_global_admin.is_admin_like());
    }

    #[test]
    fn test_resolve_guild_for_user_admin_role() {
        let admin_user = LobbyUser {
            user_id: "a1".into(),
            username: "Admin".into(),
            avatar: None,
            role: Some("admin".into()),
            servidor: Some("guild_official".into()),
            is_admin: false,
        };

        // Mismo servidor que servidor asignado -> Ok
        assert_eq!(
            resolve_guild_for_user(&admin_user, Some("guild_official"), true),
            Ok(Some("guild_official".into()))
        );
        assert_eq!(
            resolve_guild_for_user(&admin_user, Some("guild_official"), false),
            Ok(Some("guild_official".into()))
        );

        // Sin guild solicitado -> adopta servidor del usuario
        assert_eq!(
            resolve_guild_for_user(&admin_user, None, true),
            Ok(Some("guild_official".into()))
        );

        // Servidor distinto en ruta admin -> error de gestión
        assert_eq!(
            resolve_guild_for_user(&admin_user, Some("other_guild"), true),
            Err("No tienes permisos para gestionar este servidor.")
        );

        // Servidor distinto en ruta de acceso -> error de acceso
        assert_eq!(
            resolve_guild_for_user(&admin_user, Some("other_guild"), false),
            Err("No tienes permisos para acceder a este servidor.")
        );
    }

    #[test]
    fn test_resolve_guild_for_user_non_admin_and_global_admin() {
        let normal_user = LobbyUser {
            user_id: "u1".into(),
            username: "User".into(),
            avatar: None,
            role: None,
            servidor: None,
            is_admin: false,
        };
        assert_eq!(
            resolve_guild_for_user(&normal_user, Some("any_guild"), false),
            Ok(Some("any_guild".into()))
        );
        assert_eq!(resolve_guild_for_user(&normal_user, None, false), Ok(None));

        let global_admin = LobbyUser {
            user_id: "g1".into(),
            username: "Global".into(),
            avatar: None,
            role: Some("global_admin".into()),
            servidor: None,
            is_admin: false,
        };
        assert_eq!(
            resolve_guild_for_user(&global_admin, Some("any_guild"), true),
            Ok(Some("any_guild".into()))
        );
    }

    #[test]
    fn test_authenticate_cookie_priority() {
        let verifier = MockVerifier;

        // Sin cabeceras -> Missing
        let headers = HeaderMap::new();
        assert!(matches!(
            authenticate(&headers, &verifier),
            AuthOutcome::Missing
        ));

        // Token inválido -> Invalid
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "auth_token=invalid".parse().unwrap());
        assert!(matches!(
            authenticate(&headers, &verifier),
            AuthOutcome::Invalid
        ));

        // auth_token válido -> Success
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "auth_token=user-token".parse().unwrap());
        assert!(matches!(
            authenticate(&headers, &verifier),
            AuthOutcome::Success(u) if u.username == "NormalUser"
        ));

        // auth_token inválido pero admin_token válido -> Success
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "auth_token=invalid; admin_token=admin-guild-token"
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            authenticate(&headers, &verifier),
            AuthOutcome::Success(u) if u.username == "GuildAdmin"
        ));
    }
}
