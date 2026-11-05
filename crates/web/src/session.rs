//! Sesión compatible con el legacy de Node (`jsonwebtoken` npm, HS256).
//!
//! Los tokens emitidos por Node con `ADMIN_JWT_SECRET` se validan aquí y los
//! emitidos aquí se validan en Node: mismos claims (`userId`, `username`,
//! `avatar`, `isAdmin`, `role`, `servidor`, `iat`, `exp`), mismo algoritmo,
//! mismas cookies (`auth_token` y `admin_token`).
//!
//! # Montaje (Tasks 13/14)
//!
//! Los extractores ([`OptionalUser`], [`AuthUser`], [`AdminUser`]) necesitan
//! un [`SessionState`] que salga del estado de la aplicación:
//!
//! ```ignore
//! #[derive(Clone)]
//! struct AppState { session: SessionState /* , ... */ }
//!
//! impl axum::extract::FromRef<AppState> for SessionState {
//!     fn from_ref(s: &AppState) -> SessionState { s.session.clone() }
//! }
//!
//! let app = Router::new()/* rutas */.with_state(AppState {
//!     session: SessionState::from_config(&config)?,
//! });
//! ```
//!
//! # Seguridad
//!
//! * Solo se acepta HS256; la cabecera `alg` del token nunca se usa para
//!   elegir el algoritmo (`none`, HS384, RS256... se rechazan).
//! * `exp` es obligatorio y se comprueba sin margen (Node: `clockTolerance` 0).
//! * Ninguna entrada (cookie vacía, enorme, con bytes no UTF-8) provoca panic.
//! * Mejora deliberada frente a Node: si `auth_token` está presente pero no
//!   es válido se prueba `admin_token` (un `auth_token` caducado no oculta un
//!   `admin_token` válido).

use std::sync::Arc;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cookie::{time::Duration, time::OffsetDateTime, Cookie, SameSite};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;

/// Nombre de la cookie principal.
pub const AUTH_COOKIE: &str = "auth_token";
/// Nombre de la cookie de respaldo.
pub const ADMIN_COOKIE: &str = "admin_token";
/// Duración de la sesión (8 h, como `expiresIn: '8h'` en Node).
pub const SESSION_SECS: u64 = 8 * 3600;
/// Tamaño máximo de token que se intenta verificar (las cookies reales miden
/// menos de 4 KiB); evita trabajar con valores absurdamente grandes.
const MAX_TOKEN_LEN: usize = 16 * 1024;

/// Máximo de candidatos de cookie que se verifican por petición (evita que
/// una cabecera con miles de cookies fuerce miles de verificaciones HMAC).
const MAX_VERIFY_ATTEMPTS: usize = 16;

const MSG_NOT_AUTH: &str = "No autenticado";
const MSG_BAD_TOKEN: &str = "Token inválido o expirado";
const MSG_FORBIDDEN: &str = "No autorizado. Se requieren permisos de administrador.";

/// Error de sesión (firma/verificación de tokens).
#[derive(Debug, Error)]
pub enum SessionError {
    /// El token es demasiado grande para ser legítimo.
    #[error("token too large")]
    TooLarge,
    /// El secreto de firma está vacío.
    #[error("empty secret")]
    EmptySecret,
    /// El token no es válido (firma, formato, algoritmo o caducidad).
    #[error("invalid token: {0}")]
    Invalid(#[from] jsonwebtoken::errors::Error),
}

/// Claims de la sesión, tal como los emite el legacy de Node.
///
/// Cualquier claim no listado (p. ej. `iat`) se conserva en [`Self::extra`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionClaims {
    #[serde(rename = "userId", default, deserialize_with = "lenient_string")]
    pub user_id: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub username: String,
    #[serde(default, deserialize_with = "lenient_opt_string")]
    pub avatar: Option<String>,
    #[serde(rename = "isAdmin", default, deserialize_with = "strict_true")]
    pub is_admin: bool,
    #[serde(default, deserialize_with = "lenient_string")]
    pub role: String,
    #[serde(default, deserialize_with = "lenient_opt_string")]
    pub servidor: Option<String>,
    pub exp: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Cadena, número (se pasa a texto) o cualquier otra cosa (vacío).
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(lenient_opt_string(d)?.unwrap_or_default())
}

fn lenient_opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Como `decoded.isAdmin === true` en Node: solo el booleano `true` cuenta.
fn strict_true<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(matches!(Value::deserialize(d)?, Value::Bool(true)))
}

impl SessionClaims {
    /// Admin según el legacy: `role` es `global_admin` o `admin`, o `isAdmin`.
    pub fn is_admin_user(&self) -> bool {
        self.role == "global_admin" || self.role == "admin" || self.is_admin
    }
}

/// Crea los claims de una sesión nueva que caduca 8 h después de `now`
/// (segundos Unix).
pub fn new_claims(
    user_id: String,
    username: String,
    avatar: Option<String>,
    role: String,
    is_admin: bool,
    servidor: Option<String>,
    now: u64,
) -> SessionClaims {
    SessionClaims {
        user_id,
        username,
        avatar,
        is_admin,
        role,
        servidor,
        exp: now.saturating_add(SESSION_SECS),
        extra: Map::new(),
    }
}

/// Firma `claims` con HS256. El llamador fija `exp` (y `iat` en `extra` si
/// quiere imitar a Node).
pub fn sign(claims: &SessionClaims, secret: &[u8]) -> Result<String, SessionError> {
    Ok(jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret),
    )?)
}

/// Verifica `token` (solo HS256, `exp` obligatorio, sin margen de reloj).
pub fn verify(token: &str, secret: &[u8]) -> Result<SessionClaims, SessionError> {
    if token.len() > MAX_TOKEN_LEN {
        return Err(SessionError::TooLarge);
    }
    let mut v = Validation::new(Algorithm::HS256);
    v.algorithms = vec![Algorithm::HS256];
    v.leeway = 0;
    v.validate_exp = true;
    v.validate_nbf = true; // jsonwebtoken (npm) también valida `nbf` si existe
    v.validate_aud = false; // npm solo valida `aud` si se le pide
    v.set_required_spec_claims(&["exp"]);
    let data = jsonwebtoken::decode::<SessionClaims>(token, &DecodingKey::from_secret(secret), &v)?;
    // Node rechaza con `clockTimestamp >= exp`; jsonwebtoken (Rust) solo con
    // `exp < now`, así que la frontera exacta se cierra aquí.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if data.claims.exp <= now {
        return Err(jsonwebtoken::errors::Error::from(
            jsonwebtoken::errors::ErrorKind::ExpiredSignature,
        )
        .into());
    }
    Ok(data.claims)
}

/// Estado necesario para los extractores y las cookies.
#[derive(Clone)]
pub struct SessionState {
    secret: Arc<[u8]>,
    /// Marca `Secure` en las cookies emitidas.
    pub secure_cookies: bool,
}

impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionState")
            .field("secret", &"<redacted>")
            .field("secure_cookies", &self.secure_cookies)
            .finish()
    }
}

impl SessionState {
    /// Falla con [`SessionError::EmptySecret`] si `secret` está vacío.
    pub fn new(secret: &[u8], secure_cookies: bool) -> Result<Self, SessionError> {
        if secret.is_empty() {
            return Err(SessionError::EmptySecret);
        }
        Ok(Self {
            secret: Arc::from(secret),
            secure_cookies,
        })
    }

    /// `Secure` según `Config::production` (que ya incluye
    /// `NODE_ENV=production` o `BASE_URL` https, como en Node). El secreto es
    /// el valor crudo (sin recortar) de `ADMIN_JWT_SECRET`.
    pub fn from_config(cfg: &common::config::Config) -> Result<Self, SessionError> {
        Self::new(cfg.admin_jwt_secret.as_bytes(), cfg.production)
    }

    pub fn verify(&self, token: &str) -> Result<SessionClaims, SessionError> {
        verify(token, &self.secret)
    }

    pub fn sign(&self, claims: &SessionClaims) -> Result<String, SessionError> {
        sign(claims, &self.secret)
    }
}

/// Cookies de sesión (`auth_token` y `admin_token`, mismo token) con las
/// opciones de Node: HttpOnly, SameSite=Lax, Path=/, Max-Age 8 h y `Secure`
/// según `state.secure_cookies`. `claims` queda para futuros usos (la duración
/// es fija, igual que en Node).
pub fn issue_cookies(
    _claims: &SessionClaims,
    token: &str,
    state: &SessionState,
) -> [Cookie<'static>; 2] {
    let build = |name: &'static str| {
        let mut b = Cookie::build((name, token.to_owned()))
            .http_only(true)
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(Duration::seconds(SESSION_SECS as i64));
        if state.secure_cookies {
            b = b.secure(true);
        }
        b.build()
    };
    [build(AUTH_COOKIE), build(ADMIN_COOKIE)]
}

/// Cookies de borrado (como `res.clearCookie(name, { path: '/' })`).
pub fn clear_cookies() -> [Cookie<'static>; 2] {
    let build = |name: &'static str| {
        Cookie::build((name, ""))
            .path("/")
            .max_age(Duration::ZERO)
            .expires(OffsetDateTime::UNIX_EPOCH)
            .build()
    };
    [build(AUTH_COOKIE), build(ADMIN_COOKIE)]
}

/// Valores de cabecera `Set-Cookie` para `cookies`.
pub fn set_cookie_values(cookies: &[Cookie<'static>]) -> Vec<HeaderValue> {
    cookies
        .iter()
        .filter_map(|c| HeaderValue::from_str(&c.to_string()).ok())
        .collect()
}

/// Valores no vacíos de la cookie `name` en todas las cabeceras `Cookie`,
/// en orden. Tolera espacios, comillas, duplicados y bytes no UTF-8.
fn cookie_values(parts: &Parts, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for h in parts.headers.get_all(header::COOKIE) {
        for pair in h.as_bytes().split(|&b| b == b';') {
            let Some(eq) = pair.iter().position(|&b| b == b'=') else {
                continue;
            };
            if pair[..eq].trim_ascii() != name.as_bytes() {
                continue;
            }
            let mut value = pair[eq + 1..].trim_ascii();
            if let [b'"', inner @ .., b'"'] = value {
                value = inner;
            }
            if let Ok(s) = std::str::from_utf8(value) {
                if !s.is_empty() {
                    out.push(s.to_owned());
                }
            }
        }
    }
    out
}

enum Outcome {
    /// Ninguna cookie de sesión.
    Missing,
    /// Había cookie(s) pero ninguna válida.
    Invalid,
    Valid(SessionClaims),
}

fn authenticate(parts: &Parts, state: &SessionState) -> Outcome {
    let mut any = false;
    let mut attempts = 0;
    for name in [AUTH_COOKIE, ADMIN_COOKIE] {
        for value in cookie_values(parts, name) {
            any = true;
            if attempts >= MAX_VERIFY_ATTEMPTS {
                return Outcome::Invalid;
            }
            attempts += 1;
            if let Ok(claims) = state.verify(&value) {
                return Outcome::Valid(claims);
            }
        }
    }
    if any {
        Outcome::Invalid
    } else {
        Outcome::Missing
    }
}

/// Rechazo de los extractores: respuesta JSON con el mensaje del legacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRejection {
    NotAuthenticated,
    InvalidToken,
    Forbidden,
}

impl SessionRejection {
    /// Código HTTP del legacy (401 o 403).
    pub fn status(self) -> StatusCode {
        match self {
            Self::NotAuthenticated | Self::InvalidToken => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
        }
    }

    /// Mensaje del legacy (`middleware/auth.js`), campo `error` del cuerpo.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAuthenticated => MSG_NOT_AUTH,
            Self::InvalidToken => MSG_BAD_TOKEN,
            Self::Forbidden => MSG_FORBIDDEN,
        }
    }
}

impl IntoResponse for SessionRejection {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({ "error": self.message() }))).into_response()
    }
}

/// Usuario si hay sesión válida; nunca rechaza (`optionalUserToken`).
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<SessionClaims>);

/// Sesión válida obligatoria (`verifyUserToken`): 401 si no.
#[derive(Debug, Clone)]
pub struct AuthUser(pub SessionClaims);

/// Sesión válida de administrador (`verifyAdminToken`): 401 / 403.
#[derive(Debug, Clone)]
pub struct AdminUser(pub SessionClaims);

impl<S> FromRequestParts<S> for OptionalUser
where
    S: Send + Sync,
    SessionState: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let st = SessionState::from_ref(state);
        Ok(OptionalUser(match authenticate(parts, &st) {
            Outcome::Valid(c) => Some(c),
            _ => None,
        }))
    }
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    SessionState: FromRef<S>,
{
    type Rejection = SessionRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let st = SessionState::from_ref(state);
        match authenticate(parts, &st) {
            Outcome::Valid(c) => Ok(AuthUser(c)),
            Outcome::Missing => Err(SessionRejection::NotAuthenticated),
            Outcome::Invalid => Err(SessionRejection::InvalidToken),
        }
    }
}

impl<S> FromRequestParts<S> for AdminUser
where
    S: Send + Sync,
    SessionState: FromRef<S>,
{
    type Rejection = SessionRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthUser(c) = AuthUser::from_request_parts(parts, state).await?;
        if c.is_admin_user() {
            Ok(AdminUser(c))
        } else {
            Err(SessionRejection::Forbidden)
        }
    }
}

impl lobby::auth::SessionVerifier for SessionState {
    fn verify_token(&self, token: &str) -> Option<lobby::auth::LobbyUser> {
        let claims = self.verify(token).ok()?;
        Some(lobby::auth::LobbyUser {
            user_id: claims.user_id,
            username: claims.username,
            avatar: claims.avatar,
            role: if claims.role.is_empty() {
                None
            } else {
                Some(claims.role)
            },
            servidor: claims.servidor,
            is_admin: claims.is_admin,
            nickname: claims
                .extra
                .get("nickname")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            faceit_nickname: claims
                .extra
                .get("faceit_nickname")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }
}
