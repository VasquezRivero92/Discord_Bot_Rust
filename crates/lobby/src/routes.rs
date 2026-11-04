use crate::maps::default_catalog;
use crate::state::{ChatMessage, LobbyManager, Player};
use axum::{
    extract::{RawQuery, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
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

#[derive(Deserialize)]
pub struct JoinPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    pub player: Player,
}

#[derive(Deserialize)]
pub struct UserActionPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    #[serde(rename = "discordId")]
    pub discord_id: String,
}

#[derive(Deserialize)]
pub struct MapActionPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "mapId")]
    pub map_id: String,
}

#[derive(Deserialize)]
pub struct SendChatPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    #[serde(rename = "authorId")]
    pub author_id: String,
    #[serde(rename = "authorName")]
    pub author_name: String,
    #[serde(rename = "authorAvatar")]
    pub author_avatar: Option<String>,
    pub content: String,
    #[serde(rename = "imageUrl")]
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
        .route("/api/lobby/ready", post(ready))
        .route("/api/lobby/mixton-player-ready", post(ready))
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

async fn leave(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    let state = manager.leave_queue(guild_id, &payload.discord_id).await;
    Json(json!({ "success": true, "state": state }))
}

async fn ready(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    let state = manager.set_ready(guild_id, &payload.discord_id, true).await;
    Json(json!({ "success": true, "state": state }))
}

async fn unready(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    let state = manager.set_ready(guild_id, &payload.discord_id, false).await;
    Json(json!({ "success": true, "state": state }))
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
            axum::http::StatusCode::BAD_REQUEST,
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
