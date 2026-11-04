//! Endpoints REST para el Lobby de Mixtón y CS2 en Axum.

use crate::maps::default_catalog;
use crate::state::{ChatMessage, LobbyManager, Player};
use axum::{
    extract::{Path, RawQuery, State},
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

#[derive(Deserialize, Debug, Default)]
pub struct TournamentConfigPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(rename = "lobbyMode", default)]
    pub lobby_mode: Option<String>,
    #[serde(rename = "tournamentMode", default)]
    pub tournament_mode: Option<String>,
    #[serde(rename = "gamesPerMatch", default)]
    pub games_per_match: Option<u32>,
    #[serde(rename = "roundsPerGame", default)]
    pub rounds_per_game: Option<u32>,
}

#[derive(Deserialize, Debug, Default)]
pub struct GamesConfigPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "gamesPerMatch", default)]
    pub games_per_match: Option<u32>,
    #[serde(rename = "roundsPerGame", default)]
    pub rounds_per_game: Option<u32>,
}

#[derive(Deserialize, Debug, Default)]
pub struct ClearTeamPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId", default)]
    pub team_id: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct AssignCaptainPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId", default)]
    pub team_id: Option<String>,
    pub player: Option<Player>,
}

#[derive(Deserialize, Debug, Default)]
pub struct AssignPlayerPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId", default)]
    pub team_id: Option<String>,
    #[serde(rename = "playerId", default)]
    pub player_id: Option<String>,
    #[serde(rename = "discordId", default)]
    pub discord_id: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct PickPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId", default)]
    pub team_id: Option<String>,
    #[serde(rename = "playerId", default)]
    pub player_id: Option<String>,
    #[serde(rename = "discordId", default)]
    pub discord_id: Option<String>,
}

pub fn router(state: LobbyApiState) -> Router {
    Router::new()
        // Estado y lecturas básicas
        .route("/api/lobby/state", get(get_state))
        .route("/api/lobby/mixton-state", get(get_state))
        .route("/api/lobby/guilds", get(guilds))
        .route("/api/lobby/matches-history", get(matches_history))
        .route("/api/lobby/matches-history/{match_id}", get(matches_history))
        .route("/api/lobby/map-catalog", get(map_catalog))
        .route("/api/lobby/mixton-queues", get(mixton_queues))
        .route("/api/lobby/wallet", get(wallet))
        .route("/api/lobby/player-balance/{discord_id}", get(player_balance))
        .route("/api/lobby/users", get(users))
        .route("/api/lobby/steam-avatars", get(steam_avatars))
        .route("/api/lobby/steam-avatar/{steam_id}", get(steam_avatar))
        // WebSockets
        .route("/api/lobby/mixton-ws", get(crate::ws::mixton_ws_handler))
        // Acciones de jugador
        .route("/api/lobby/join", post(join))
        .route("/api/lobby/leave", post(leave))
        .route("/api/lobby/ready", post(player_ready))
        .route("/api/lobby/mixton-player-ready", post(player_ready))
        .route("/api/lobby/unready", post(unready))
        .route("/api/lobby/reset", post(reset))
        .route("/api/lobby/mixton-reset", post(reset))
        .route("/api/lobby/mixton-reset-players", post(reset_players))
        // Auto-asignación de capitanes y jugadores
        .route("/api/lobby/mixton-auto-assign-captains", post(auto_assign_captains))
        .route("/api/lobby/mixton-auto-assign", post(auto_assign_players))
        .route("/api/lobby/mixton-assign-player", post(assign_player))
        .route("/api/lobby/mixton-remove-player", post(remove_player))
        // Veto de mapas
        .route("/api/lobby/start-veto", post(start_veto))
        .route("/api/lobby/mixton-start-veto", post(start_veto))
        .route("/api/lobby/map-ban", post(map_ban))
        .route("/api/lobby/mixton-ban-map", post(map_ban))
        .route("/api/lobby/mixton-cancel-veto", post(cancel_veto))
        // Equipos y capitanes
        .route("/api/lobby/mixton-clear-team", post(clear_team))
        .route("/api/lobby/mixton-assign-captain", post(assign_captain))
        .route("/api/lobby/mixton-pick", post(pick_player))
        .route("/api/lobby/mixton-captain-proposal", post(captain_proposal))
        .route("/api/lobby/mixton-captain-ready", post(captain_ready))
        .route("/api/lobby/mixton-force-roulette", post(force_roulette))
        // Modo torneo y configuración de rondas
        .route("/api/lobby/tournament-config", post(tournament_config))
        .route("/api/lobby/tournament-report-winner", post(tournament_report_winner))
        .route("/api/lobby/mixton-games-config", post(games_config))
        .route("/api/lobby/mixton-rounds-config", post(games_config))
        .route("/api/lobby/mixton-toggle-command", post(toggle_command))
        // Acciones generales de partida y configuración
        .route("/api/lobby/mixton-setup", post(generic_lobby_action))
        .route("/api/lobby/mixton-start", post(generic_lobby_action))
        .route("/api/lobby/mixton-pick-order", post(generic_lobby_action))
        .route("/api/lobby/mixton-check-bet-status", post(generic_lobby_action))
        .route("/api/lobby/mixton-load-team", post(generic_lobby_action))
        .route("/api/lobby/mixton-override-tier", post(generic_lobby_action))
        .route("/api/lobby/create-match", post(generic_lobby_action))
        .route("/api/lobby/mixton-create-match", post(generic_lobby_action))
        .route("/api/lobby/sync-faceit-from-steam", post(generic_lobby_action))
        .route("/api/lobby/verify-faceit", post(generic_lobby_action))
        // Chat
        .route("/api/lobby/mixton-chat-history", get(chat_history))
        .route("/api/lobby/mixton-chat-message", post(chat_message))
        .route("/api/lobby/mixton-chat-send", post(chat_message))
        // Billetera / Recargas
        .route("/api/lobby/quick-add-balance", post(recharge))
        .route("/api/lobby/wallet/recharge-yape", post(recharge))
        .with_state(state.manager)
}

async fn matches_history() -> impl IntoResponse {
    Json(json!({ "history": [] }))
}

async fn map_catalog() -> impl IntoResponse {
    Json(json!({ "catalog": default_catalog() }))
}

async fn mixton_queues() -> impl IntoResponse {
    Json(json!({ "queues": [] }))
}

async fn wallet() -> impl IntoResponse {
    Json(json!({ "balance": 0.0 }))
}

async fn guilds(State(manager): State<Arc<LobbyManager>>) -> impl IntoResponse {
    let g_id = manager.resolve_guild_id(None);
    Json(json!({
        "guilds": [
            {
                "id": g_id,
                "name": "Servidor Oficial",
                "icon": null,
                "memberCount": 0
            }
        ]
    }))
}

async fn player_balance(
    Path(discord_id): Path<String>,
) -> impl IntoResponse {
    Json(json!({
        "discordId": discord_id,
        "balance": 100.0,
        "hasEnough": true,
        "deficit": 0.0
    }))
}

async fn users() -> impl IntoResponse {
    Json(json!({ "users": [] }))
}

async fn steam_avatars() -> impl IntoResponse {
    Json(json!({ "avatars": {} }))
}

async fn steam_avatar(Path(_): Path<String>) -> impl IntoResponse {
    Json(json!({ "avatar": "https://cdn.discordapp.com/embed/avatars/0.png" }))
}

async fn recharge(
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    Json(json!({
        "success": true,
        "balance": 100.0,
        "message": "Saldo procesado exitosamente."
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
        "guildName": "Servidor Oficial",
        "guildId": guild_id
    }))
}

async fn join(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<JoinPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
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

async fn reset_players(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    state.available_players.clear();
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn auto_assign_captains(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    // Si ya tienen capitanes todos los equipos, simplemente retornar éxito
    if state.teams.iter().all(|t| t.captain.is_some()) {
        return Json(json!({ "success": true, "state": state })).into_response();
    }

    if state.available_players.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "No hay suficientes jugadores listos en la cola para asignar capitanes."
            })),
        )
            .into_response();
    }

    // Ordenar jugadores por ELO descendente para asignar a los de mayor nivel
    state.available_players.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));

    for team in &mut state.teams {
        if team.captain.is_none() && !state.available_players.is_empty() {
            let cap = state.available_players.remove(0);
            team.captain = Some(cap);
        }
    }

    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated })).into_response()
}

async fn auto_assign_players(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let num_teams = state.teams.len();
    if num_teams < 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": "Se requieren al menos 2 equipos configurados." })),
        )
            .into_response();
    }

    let mut team_idx = 0;
    while !state.available_players.is_empty() {
        let player = state.available_players.remove(0);
        state.teams[team_idx % num_teams].players.push(player);
        team_idx += 1;
    }

    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated })).into_response()
}

async fn assign_player(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignPlayerPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let p_id = payload.player_id.or(payload.discord_id);
    if let (Some(tid), Some(did)) = (payload.team_id, p_id) {
        if let Some(pos) = state.available_players.iter().position(|p| p.discord_id == did) {
            let p = state.available_players.remove(pos);
            if let Some(t) = state.teams.iter_mut().find(|t| t.id == tid) {
                t.players.push(p);
            }
        }
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated })).into_response()
}

async fn remove_player(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignPlayerPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let p_id = payload.player_id.or(payload.discord_id);
    if let Some(did) = p_id {
        for t in &mut state.teams {
            if let Some(pos) = t.players.iter().position(|p| p.discord_id == did) {
                let p = t.players.remove(pos);
                state.available_players.push(p);
                break;
            }
            if let Some(ref c) = t.captain {
                if c.discord_id == did {
                    let cap = t.captain.take().unwrap();
                    state.available_players.push(cap);
                    break;
                }
            }
        }
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated })).into_response()
}

async fn cancel_veto(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    state.map_veto = None;
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated })).into_response()
}

async fn clear_team(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<ClearTeamPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    if let Some(tid) = payload.team_id {
        if let Some(t) = state.teams.iter_mut().find(|t| t.id == tid) {
            t.players.clear();
        }
    } else {
        for t in &mut state.teams {
            t.players.clear();
        }
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn assign_captain(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignCaptainPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    if let (Some(tid), Some(player)) = (payload.team_id, payload.player) {
        if let Some(t) = state.teams.iter_mut().find(|t| t.id == tid) {
            t.captain = Some(player);
        }
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn pick_player(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<PickPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let p_id = payload.player_id.or(payload.discord_id);
    let mut state = manager.get_state(guild_id).await;
    if let (Some(tid), Some(pid)) = (payload.team_id, p_id) {
        if let Some(pos) = state.available_players.iter().position(|p| p.discord_id == pid) {
            let player = state.available_players.remove(pos);
            if let Some(t) = state.teams.iter_mut().find(|t| t.id == tid) {
                t.players.push(player);
            }
        }
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn captain_proposal(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    state.captain_a_proposal = payload.get("proposal").cloned();
    state.bet_agreed = true;
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn captain_ready(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    Json(json!({ "success": true, "state": state }))
}

async fn force_roulette(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    Json(json!({ "success": true, "state": state }))
}

async fn tournament_config(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<TournamentConfigPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    if let Some(mode) = payload.lobby_mode {
        state.lobby_mode = mode;
    }
    if let Some(games) = payload.games_per_match.or(payload.rounds_per_game) {
        state.games_per_match = games;
        state.rounds_per_game = games;
    }
    if payload.action.as_deref() == Some("reset") {
        state.tournament = None;
        state.map_veto = None;
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn tournament_report_winner(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    Json(json!({ "success": true, "state": state }))
}

async fn games_config(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<GamesConfigPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    if let Some(games) = payload.games_per_match.or(payload.rounds_per_game) {
        state.games_per_match = games;
        state.rounds_per_game = games;
    }
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "state": updated }))
}

async fn toggle_command(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    Json(json!({ "success": true, "commandEnabled": true, "state": state }))
}

async fn generic_lobby_action(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
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
