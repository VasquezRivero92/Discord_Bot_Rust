//! Endpoints REST para la gestión del lobby de Mixton (`/api/lobby/*`).

use crate::state::{ChatMessage, LobbyManager, Player};
use crate::ws::mixton_ws_handler;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct LobbyApiState {
    pub manager: Arc<LobbyManager>,
}

#[derive(Debug, Deserialize)]
pub struct GuildQuery {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JoinPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    pub player: Player,
}

#[derive(Debug, Deserialize)]
pub struct UserActionPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    #[serde(rename = "discordId")]
    pub discord_id: String,
}

#[derive(Debug, Deserialize)]
pub struct MapActionPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: String,
    #[serde(rename = "mapId")]
    pub map_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SendChatPayload {
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
    #[serde(rename = "authorId")]
    pub author_id: String,
    #[serde(rename = "authorName")]
    pub author_name: String,
    pub content: String,
    #[serde(rename = "authorAvatar")]
    pub author_avatar: Option<String>,
    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,
}

pub fn router(state: LobbyApiState) -> Router {
    Router::new()
        .route("/api/lobby/state", get(get_state))
        .route("/api/lobby/mixton-ws", get(mixton_ws_handler))
        .route("/api/lobby/join", post(join))
        .route("/api/lobby/leave", post(leave))
        .route("/api/lobby/ready", post(ready))
        .route("/api/lobby/unready", post(unready))
        .route("/api/lobby/reset", post(reset))
        .route("/api/lobby/start-veto", post(start_veto))
        .route("/api/lobby/map-ban", post(map_ban))
        .route("/api/lobby/mixton-chat-history", get(chat_history))
        .route("/api/lobby/mixton-chat-message", post(chat_message))
        .with_state(state.manager)
}

async fn get_state(
    Query(query): Query<GuildQuery>,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(query.guild_id.as_deref());
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
    Query(query): Query<GuildQuery>,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(query.guild_id.as_deref());
    let state = manager.reset_lobby(guild_id).await;
    Json(json!({ "success": true, "state": state }))
}

async fn start_veto(
    Query(query): Query<GuildQuery>,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(query.guild_id.as_deref());
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
    Query(query): Query<GuildQuery>,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(query.guild_id.as_deref());
    let history = manager.get_chat_history(guild_id).await;
    Json(history)
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
