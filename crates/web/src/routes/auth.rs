//! `GET /auth/discord`, `GET /auth/discord/callback`, `GET /auth/logout` y `GET /auth/me`.
//!
//! Manejador nativo del flujo OAuth2 con Discord API v10 y emisión de sesiones JWT.

use axum::{
    extract::{FromRef, RawQuery, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use common::config::Config;
use serde::Deserialize;
use serde_json::json;

use crate::session::{
    clear_cookies, issue_cookies, set_cookie_values, OptionalUser, SessionClaims,
    SessionState, SESSION_SECS,
};

const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";

/// Estado de las rutas de autenticación.
#[derive(Debug, Clone)]
pub struct AuthState {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub session: SessionState,
    pub http: reqwest::Client,
}

impl AuthState {
    pub fn new(client_id: &str, base_url: &str) -> Self {
        let session = SessionState::new(b"default-jwt-secret-min-32-bytes-long!", false)
            .expect("Secreto de prueba válido");
        Self {
            client_id: client_id.to_owned(),
            client_secret: "test_client_secret".to_owned(),
            redirect_uri: format!("{base_url}/auth/discord/callback"),
            session,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn from_config(cfg: &Config) -> Self {
        let session = SessionState::from_config(cfg)
            .unwrap_or_else(|_| {
                SessionState::new(b"default-jwt-secret-min-32-bytes-long!", false)
                    .expect("Secreto de prueba válido")
            });
        Self {
            client_id: cfg.discord_client_id.clone(),
            client_secret: cfg.discord_client_secret.clone(),
            redirect_uri: format!("{}/auth/discord/callback", cfg.base_url),
            session,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }
}

impl FromRef<AuthState> for SessionState {
    fn from_ref(state: &AuthState) -> Self {
        state.session.clone()
    }
}

/// Router autocontenido para fusionar con `Router::merge`.
pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/auth/discord", get(discord))
        .route("/auth/discord/callback", get(discord_callback))
        .route("/auth/logout", get(logout))
        .route("/auth/me", get(auth_me))
        .route("/api/lobby/me", get(lobby_me))
        .route("/api/me", get(auth_me))
        .route("/auth/steam", get(steam))
        .route("/auth/steam/callback", get(steam_callback))
        .with_state(state)
}

/// `sanitizeRedirectPath`: solo rutas relativas del propio sitio; cualquier otra cosa -> `/`.
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

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
    username: String,
    avatar: Option<String>,
}

/// `GET /auth/discord/callback?code=...&state=...`
async fn discord_callback(
    State(st): State<AuthState>,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(raw.as_deref().unwrap_or("").as_bytes())
            .into_owned()
            .collect();

    let mut code = None;
    let mut state = None;
    let mut error = None;

    for (k, v) in pairs {
        if k == "code" {
            code = Some(v);
        } else if k == "state" {
            state = Some(v);
        } else if k == "error" {
            error = Some(v);
        }
    }

    if let Some(err) = error {
        tracing::warn!("[AUTH] OAuth denegado por Discord ({err})");
        return found("/", Vec::new());
    }

    let code = match code {
        Some(c) if !c.is_empty() => c,
        _ => return found("/auth/discord", Vec::new()),
    };

    let target_path = sanitize_redirect_path(state.as_deref());

    // 1. Intercambiar code por access_token con Discord
    let token_params = [
        ("client_id", st.client_id.as_str()),
        ("client_secret", st.client_secret.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", st.redirect_uri.as_str()),
    ];

    let token_res = match st
        .http
        .post("https://discord.com/api/v10/oauth2/token")
        .form(&token_params)
        .header(
            header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
    {
        Ok(res) => res,
        Err(err) => {
            tracing::error!("[AUTH] Error de red al solicitar token de Discord: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error al contactar Discord OAuth",
            )
                .into_response();
        }
    };

    if !token_res.status().is_success() {
        let err_text = token_res.text().await.unwrap_or_default();
        tracing::warn!("[AUTH] Discord token endpoint respondió con error: {err_text}");
        return found("/auth/discord", Vec::new());
    }

    let token_data: TokenResponse = match token_res.json().await {
        Ok(d) => d,
        Err(err) => {
            tracing::error!("[AUTH] Error parseando token de Discord: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Respuesta inválida de Discord",
            )
                .into_response();
        }
    };

    // 2. Obtener datos del usuario desde Discord API
    let user_res = match st
        .http
        .get("https://discord.com/api/v10/users/@me")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_data.access_token),
        )
        .header(
            header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
    {
        Ok(res) => res,
        Err(err) => {
            tracing::error!("[AUTH] Error al obtener usuario de Discord: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error al obtener perfil de Discord",
            )
                .into_response();
        }
    };

    if !user_res.status().is_success() {
        return found("/auth/discord", Vec::new());
    }

    let user: DiscordUser = match user_res.json().await {
        Ok(u) => u,
        Err(err) => {
            tracing::error!("[AUTH] Error parseando perfil de Discord: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error leyendo usuario de Discord",
            )
                .into_response();
        }
    };

    let now = chrono::Utc::now().timestamp() as u64;
    let claims = SessionClaims {
        user_id: user.id.clone(),
        username: user.username.clone(),
        avatar: user.avatar.clone(),
        is_admin: true,
        role: "admin".to_string(),
        servidor: None,
        exp: now + SESSION_SECS,
        extra: serde_json::Map::new(),
    };

    let token = match st.session.sign(&claims) {
        Ok(t) => t,
        Err(err) => {
            tracing::error!("[AUTH] Error firmando token JWT: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error generando sesión",
            )
                .into_response();
        }
    };

    let cookies = issue_cookies(&claims, &token, &st.session);
    let cookie_headers = set_cookie_values(&cookies);

    tracing::info!("[AUTH] Usuario autenticado con éxito: {} ({})", user.username, user.id);
    found(target_path, cookie_headers)
}

/// `GET /auth/logout`: borra ambas cookies y vuelve a `/`.
async fn logout() -> Response {
    found("/", set_cookie_values(&clear_cookies()))
}

/// `GET /auth/me`
async fn auth_me(OptionalUser(user): OptionalUser) -> impl IntoResponse {
    match user {
        Some(u) => Json(json!({
            "user": {
                "userId": u.user_id,
                "username": u.username,
                "avatar": u.avatar,
                "isAdmin": u.is_admin,
                "role": u.role,
                "servidor": u.servidor,
            }
        }))
        .into_response(),
        None => Json(json!({ "user": null })).into_response(),
    }
}

/// `GET /api/lobby/me`
async fn lobby_me(OptionalUser(user): OptionalUser) -> impl IntoResponse {
    match user {
        Some(u) => Json(json!({
            "user": {
                "userId": u.user_id,
                "username": u.username,
                "avatar": u.avatar,
                "isAdmin": u.is_admin,
                "role": u.role,
                "servidor": u.servidor,
                "hasSavedPhone": false,
                "balance": 0.0,
            }
        }))
        .into_response(),
        None => Json(json!({ "user": null })).into_response(),
    }
}

async fn steam(
    State(st): State<AuthState>,
    OptionalUser(user): OptionalUser,
) -> Response {
    if user.is_none() {
        return found("/auth/discord?redirect=/auth/steam", vec![]);
    }
    let base_url = st.redirect_uri.replace("/auth/discord/callback", "");
    let return_to = format!("{base_url}/auth/steam/callback");
    let realm = format!("{base_url}/");
    let steam_url = format!(
        "https://steamcommunity.com/openid/login?openid.ns=http%3A%2F%2Fspecs.openid.net%2Fauth%2F2.0&openid.mode=checkid_setup&openid.return_to={}&openid.realm={}&openid.identity=http%3A%2F%2Fspecs.openid.net%2Fauth%2F2.0%2Fidentifier_select&openid.claimed_id=http%3A%2F%2Fspecs.openid.net%2Fauth%2F2.0%2Fidentifier_select",
        url::form_urlencoded::byte_serialize(return_to.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(realm.as_bytes()).collect::<String>()
    );
    found(&steam_url, vec![])
}

async fn steam_callback(
    RawQuery(raw): RawQuery,
) -> Response {
    let claimed_id = raw.as_deref().and_then(|r| {
        for pair in r.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "openid.claimed_id" {
                    return Some(v.to_string());
                }
            }
        }
        None
    });
    let steam_id = claimed_id.as_deref().and_then(|cid| {
        cid.split('/').last()
    }).unwrap_or("unknown");

    tracing::info!("[STEAM AUTH] SteamID64 recibido en callback: {steam_id}");
    found("/lobby?steam_linked=true", vec![])
}
