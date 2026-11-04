use crate::maps::default_catalog;
use crate::state::{ChatMessage, LobbyManager, Player};
use axum::{
    extract::{RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct LobbyApiState {
    pub manager: Arc<LobbyManager>,
}

fn extract_guild_id(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    for pair in raw.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            if k == "guildId" || k == "guild_id" {
                let decoded = url::form_urlencoded::parse(v.as_bytes())
                    .next()
                    .map(|(_, val)| val.into_owned())
                    .unwrap_or_else(|| v.to_string());
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    None
}

/// Extrae de forma segura el usuario autenticado desde las cookies de sesión (auth_token o admin_token).
fn user_from_cookies(headers: &HeaderMap) -> Option<(String, String, Option<String>)> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if pair.starts_with("auth_token=") || pair.starts_with("admin_token=") {
            let token = pair.split_once('=')?.1.trim_matches('"');
            let mut parts = token.split('.');
            let _header = parts.next()?;
            let payload_b64 = parts.next()?;
            let pad_len = (4 - (payload_b64.len() % 4)) % 4;
            let padded = format!("{}{}", payload_b64, "=".repeat(pad_len));
            let decoded = URL_SAFE_NO_PAD
                .decode(payload_b64.as_bytes())
                .or_else(|_| URL_SAFE_NO_PAD.decode(padded.as_bytes()))
                .ok()?;
            let val: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
            let user_id = val.get("userId").and_then(|v| v.as_str())?.to_string();
            let username = val
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("Jugador")
                .to_string();
            let avatar = val.get("avatar").and_then(|v| v.as_str()).map(str::to_string);
            return Some((user_id, username, avatar));
        }
    }
    None
}

#[derive(Deserialize, Debug, Default)]
pub struct JoinPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    pub player: Player,
}

#[derive(Deserialize, Debug, Default)]
pub struct UserActionPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "discordId", default)]
    pub discord_id: Option<String>,
    #[serde(default)]
    pub ready: Option<bool>,
}

#[derive(Deserialize, Debug, Default)]
pub struct MapActionPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "mapId")]
    pub map_id: String,
}

#[derive(Deserialize, Debug, Default)]
pub struct SendChatPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "authorId")]
    pub author_id: String,
    #[serde(rename = "authorName")]
    pub author_name: String,
    #[serde(rename = "authorAvatar", default)]
    pub author_avatar: Option<String>,
    pub content: String,
    #[serde(rename = "imageUrl", default)]
    pub image_url: Option<String>,
}

pub fn router(state: LobbyApiState) -> Router {
    Router::new()
        .route("/api/lobby/state", get(get_state))
        .route("/api/lobby/mixton-state", get(get_state))
        .route("/api/lobby/matches-history", get(matches_history))
        .route("/api/lobby/map-catalog", get(map_catalog))
        .route("/api/lobby/mixton-queues", get(mixton_queues))
        .route("/api/lobby/wallet", get(wallet))
        .route("/api/lobby/mixton-ws", get(crate::ws::mixton_ws_handler))
        .route("/api/lobby/join", post(join))
        .route("/api/lobby/leave", post(leave))
        .route("/api/lobby/ready", post(player_ready))
        .route("/api/lobby/mixton-player-ready", post(player_ready))
        .route("/api/lobby/unready", post(unready))
        .route("/api/lobby/reset", post(reset))
        .route("/api/lobby/mixton-reset", post(reset))
        .route("/api/lobby/start-veto", post(start_veto))
        .route("/api/lobby/mixton-start-veto", post(start_veto))
        .route("/api/lobby/map-ban", post(map_ban))
        .route("/api/lobby/mixton-ban-map", post(map_ban))
        .route("/api/lobby/mixton-chat-history", get(chat_history))
        .route("/api/lobby/mixton-chat-message", post(chat_message))
        .route("/api/lobby/mixton-chat-send", post(chat_message))
        .with_state(state.manager)
}

async fn matches_history() -> impl IntoResponse {
    Json(json!({ "history": [] }))
}

async fn map_catalog() -> impl IntoResponse {
    let catalog = default_catalog();
    let count = catalog.len();
    Json(json!({
        "success": true,
        "catalog": catalog,
        "activeCount": count
    }))
}

async fn mixton_queues() -> impl IntoResponse {
    Json(json!({
        "queues": []
    }))
}

async fn wallet() -> impl IntoResponse {
    Json(json!({
        "balance": 0.0
    }))
}

async fn get_state(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;

    Json(json!({
        "state": state,
        "availablePlayers": state.available_players,
        "commandEnabled": true,
        "guildId": guild_id
    }))
}

async fn join(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<JoinPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    let state = manager.join_queue(guild_id, payload.player).await;
    Json(json!({ "success": true, "state": state }))
}

async fn player_ready(
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());

    let (discord_id, username, avatar) = if let Some(id) = payload.discord_id {
        (id, "Jugador".to_string(), None)
    } else if let Some((id, name, av)) = user_from_cookies(&headers) {
        (id, name, av)
    } else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Debes iniciar sesión con tu cuenta de Discord." })),
        )
            .into_response();
    };

    let is_ready = payload.ready.unwrap_or(true);

    let state = if is_ready {
        let player = Player {
            discord_id: discord_id.clone(),
            nickname: username,
            faceit_nickname: None,
            steam_id_64: None,
            avatar,
            faceit_elo: 1000,
            ready: true,
            tier: "competitor".to_string(),
        };
        manager.join_queue(guild_id, player).await
    } else {
        manager.leave_queue(guild_id, &discord_id).await
    };

    Json(json!({
        "success": true,
        "ready": is_ready,
        "state": state,
        "availablePlayers": state.available_players
    }))
    .into_response()
}

async fn leave(
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());

    let discord_id = if let Some(id) = payload.discord_id {
        id
    } else if let Some((id, _, _)) = user_from_cookies(&headers) {
        id
    } else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Debes iniciar sesión con tu cuenta de Discord." })),
        )
            .into_response();
    };

    let state = manager.leave_queue(guild_id, &discord_id).await;
    Json(json!({ "success": true, "state": state })).into_response()
}

async fn unready(
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());

    let discord_id = if let Some(id) = payload.discord_id {
        id
    } else if let Some((id, _, _)) = user_from_cookies(&headers) {
        id
    } else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Debes iniciar sesión con tu cuenta de Discord." })),
        )
            .into_response();
    };

    let state = manager.set_ready(guild_id, &discord_id, false).await;
    Json(json!({ "success": true, "state": state })).into_response()
}

async fn reset(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.reset_lobby(guild_id).await;
    Json(json!({ "success": true, "state": state }))
}

async fn start_veto(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let veto = manager.start_map_veto(guild_id).await;
    Json(json!({ "success": veto.is_some(), "veto": veto }))
}

async fn map_ban(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<MapActionPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    match manager.ban_map(guild_id, &payload.team_id, &payload.map_id).await {
        Ok(state) => Json(json!({ "success": true, "state": state })).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": err })),
        )
            .into_response(),
    }
}

async fn chat_history(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let history = manager.get_chat_history(guild_id).await;
    Json(json!({
        "success": true,
        "messages": history
    }))
}

async fn chat_message(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<SendChatPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    let msg = ChatMessage {
        id: format!("msg_{}", chrono::Utc::now().timestamp_millis()),
        author_id: payload.author_id,
        author_name: payload.author_name,
        author_avatar: payload.author_avatar,
        content: payload.content,
        image_url: payload.image_url,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };
    manager.add_chat_message(guild_id, msg.clone()).await;
    Json(json!({ "success": true, "message": msg }))
}
