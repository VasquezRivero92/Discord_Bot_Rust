//! `GET /auth/discord` y `GET /auth/logout` (legacy `routes/auth.js:36-46` y `:499-503`).
//!
//! # `GET /auth/discord/callback` NO se migra (sigue en el proxy)
//!
//! El callback del legacy (`routes/auth.js:48-277`) no puede migrarse en esta fase:
//!
//! * **Escribe en Firestore**: para los ids de `GLOBAL_ADMIN_DISCORD_IDS` llama a
//!   `db.setUserRoleAndGuild(user.id, 'global_admin', null)` (línea 137). Node es el único
//!   escritor de esas colecciones durante la fase 1.
//! * **No decide con permisos de Discord**: el rol (`usuario`/`admin`/`global_admin`) y el
//!   `servidor` salen de `db.getFullUser` (`faceit_users`, línea 139), y `isAdmin` es
//!   `rol === 'global_admin' || rol === 'admin'`; no usa `checkIsAdmin`.
//! * **Necesita datos sin repo de solo lectura en `data`**: `db.isUserCaptain` (línea 155,
//!   capitanes de salas en `faceit_active_matches`) y `faceit_users`.
//!
//! Por eso este router no atiende esa ruta y el llamador la deja caer al proxy. Los bloques de
//! construcción para la fase siguiente (`DiscordApi`, `RestDiscordApi`, `is_admin`) están en
//! [`crate::discord`].
//!
//! # Hallazgo: `state` no es protección CSRF
//!
//! El legacy usa el parámetro OAuth `state` para llevar la ruta a la que volver
//! (`sanitizeRedirectPath(req.query.redirect)`), no un valor aleatorio ligado a la sesión del
//! navegador. Se mantiene el comportamiento idéntico (paridad) y no se añade un `state`
//! anti-CSRF aquí; queda como recomendación para el usuario.
//!
//! # Seguridad
//!
//! Los destinos de redirección salen de la configuración (`BASE_URL`) o son rutas relativas
//! validadas; nada de la petición (`Host`, `X-Forwarded-*`, `next`, ...) decide a dónde se redirige.

use axum::{
    extract::{RawQuery, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use common::config::Config;

use crate::session::{clear_cookies, set_cookie_values};

const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";

/// Estado de las rutas de autenticación.
#[derive(Debug, Clone)]
pub struct AuthState {
    client_id: String,
    redirect_uri: String,
}

impl AuthState {
    /// `base_url` es `BASE_URL`; como en el legacy se le quitan las `/` finales
    /// (`config/index.js:28`).
    pub fn new(client_id: &str, base_url: &str) -> Self {
        Self {
            client_id: client_id.to_owned(),
            redirect_uri: format!("{}/auth/discord/callback", base_url.trim_end_matches('/')),
        }
    }

    pub fn from_config(cfg: &Config) -> Self {
        Self::new(&cfg.discord_client_id, &cfg.base_url)
    }
}

/// Router autocontenido (sin fallback) para fusionar con `Router::merge`.
pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/auth/discord", get(discord))
        .route("/auth/logout", get(logout))
        .with_state(state)
}

/// `sanitizeRedirectPath`: solo rutas relativas del propio sitio; cualquier otra cosa -> `/`.
/// `len` cuenta unidades UTF-16 como `String.prototype.length`.
pub(crate) fn sanitize_redirect_path(value: Option<&str>) -> &str {
    match value {
        Some(v)
            if v.starts_with('/')
                && !v.starts_with("//")
                && !v.contains('\\')
                && v.encode_utf16().count() <= 300 =>
        {
            v
        }
        _ => "/",
    }
}

fn found(location: &str, cookies: Vec<HeaderValue>) -> Response {
    let mut res = (
        StatusCode::FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("Found. Redirecting to {location}"),
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    for c in cookies {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    res
}

/// `GET /auth/discord?redirect=/ruta`
async fn discord(State(st): State<AuthState>, RawQuery(raw): RawQuery) -> Response {
    let pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(raw.as_deref().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    // Como Express: si `redirect` aparece más de una vez es un array -> no es string -> `/`.
    let mut it = pairs.iter().filter(|(k, _)| k == "redirect");
    let first = it.next().map(|(_, v)| v.as_str());
    let value = if it.next().is_some() { None } else { first };
    let redirect = sanitize_redirect_path(value);

    let mut url = url::Url::parse(AUTHORIZE_URL).expect("URL constante válida");
    url.query_pairs_mut()
        .append_pair("client_id", &st.client_id)
        .append_pair("redirect_uri", &st.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", "identify")
        .append_pair("state", redirect)
        .append_pair("prompt", "consent");
    found(url.as_str(), Vec::new())
}

/// `GET /auth/logout`: borra ambas cookies y vuelve a `/`.
async fn logout() -> Response {
    found("/", set_cookie_values(&clear_cookies()))
}
