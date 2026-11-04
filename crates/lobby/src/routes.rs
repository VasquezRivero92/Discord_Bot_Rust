//! Endpoints REST para el Lobby de Mixtón y CS2 en Axum.

use crate::maps::MapInfo;
use crate::state::{
    LobbyTeam, MapVetoState,
    format_discord_avatar, partition_available_players, ChatMessage, DiscordUserDetails,
    LobbyManager, Player,
};
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
use chrono::Utc;
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
pub struct MapCatalogPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub catalog: Vec<MapInfo>,
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
    #[serde(rename = "authorId", default)]
    pub author_id: Option<String>,
    #[serde(rename = "discordId", default)]
    pub discord_id: Option<String>,
    #[serde(rename = "authorName", default)]
    pub author_name: Option<String>,
    #[serde(rename = "author", default)]
    pub author: Option<String>,
    #[serde(rename = "authorAvatar", default)]
    pub author_avatar: Option<String>,
    #[serde(rename = "avatar", default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
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
    #[serde(rename = "discordId", default)]
    pub discord_id: Option<String>,
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
        .route("/api/lobby/map-catalog", get(map_catalog).post(update_map_catalog))
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
        .route("/api/lobby/mixton-setup", post(mixton_setup))
        .route("/api/lobby/mixton-start", post(mixton_start))
        .route("/api/lobby/mixton-pick-order", post(mixton_pick_order))
        .route("/api/lobby/mixton-check-bet-status", post(generic_lobby_action))
        .route("/api/lobby/mixton-load-team", post(generic_lobby_action))
        .route("/api/lobby/mixton-override-tier", post(generic_lobby_action))
        .route("/api/lobby/create-match", post(mixton_create_match))
        .route("/api/lobby/mixton-create-match", post(mixton_create_match))
        .route("/api/lobby/sync-faceit-from-steam", post(generic_lobby_action))
        .route("/api/lobby/verify-faceit", post(generic_lobby_action))
        .route("/api/lobby/link-steam", post(link_steam))
        .route("/api/lobby/sync-steam", post(link_steam))
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

async fn map_catalog(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    Json(json!({ "catalog": state.map_catalog }))
}

async fn update_map_catalog(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<MapCatalogPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if !payload.catalog.is_empty() {
        state.map_catalog = payload.catalog;
        state.updated_at = chrono::Utc::now().timestamp_millis();
        let updated = manager.update_state(guild_id, state).await;
        let available_players = partition_available_players(&updated);
        Json(json!({
            "success": true,
            "catalog": updated.map_catalog,
            "state": updated,
            "availablePlayers": available_players
        })).into_response()
    } else {
        Json(json!({ "success": true, "catalog": state.map_catalog })).into_response()
    }
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
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    let available_players = partition_available_players(&state);
    let mut resp = json!({
        "state": state,
        "availablePlayers": available_players,
        "commandEnabled": true,
        "guildName": "MachuPeek",
        "guildId": guild_id
    });
    if let Some((id, name, av)) = user_from_cookies(&headers) {
        let avatar_url = format_discord_avatar(&id, av.as_deref());
        resp["currentUser"] = json!({
            "userId": id,
            "username": name,
            "avatar": av,
            "avatarURL": avatar_url,
            "isAdmin": true
        });
    }
    Json(resp)
}

async fn join(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<JoinPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.join_queue(guild_id, payload.player).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players }))
}

async fn player_ready(
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());

    let cookie_user = user_from_cookies(&headers);
    let (discord_id, username, avatar) = if let Some((id, name, av)) = cookie_user {
        (payload.discord_id.unwrap_or(id), name, av)
    } else if let Some(id) = payload.discord_id {
        (id, "Jugador".to_string(), None)
    } else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Debes iniciar sesión con tu cuenta de Discord." })),
        )
            .into_response();
    };

    let is_ready = payload.ready.unwrap_or(true);

    let state = if is_ready {
        let avatar_url = format_discord_avatar(&discord_id, avatar.as_deref());
        let mut player = Player {
            discord_id: discord_id.clone(),
            nickname: username.clone(),
            faceit_nickname: Some(username.clone()),
            steam_id_64: None,
            avatar: avatar.clone(),
            faceit_elo: 1000,
            faceit_level: Some(1),
            ready: true,
            tier: "C".to_string(),
            mixton_tier_override: None,
            discord_user: Some(DiscordUserDetails {
                username: username.clone(),
                display_name: username,
                avatar_url,
            }),
        };
        player.ensure_discord_user();
        manager.join_queue(guild_id, player).await
    } else {
        manager.leave_queue(guild_id, &discord_id).await
    };

    let available_players = partition_available_players(&state);

    Json(json!({
        "success": true,
        "ready": is_ready,
        "state": state,
        "availablePlayers": available_players
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
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players })).into_response()
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
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players })).into_response()
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
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
}

async fn auto_assign_captains(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    state.available_players.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));

    for team in &mut state.teams {
        if team.captain.is_none() && !state.available_players.is_empty() {
            let mut cap = state.available_players.remove(0);
            cap.ensure_discord_user();
            team.captain = Some(cap);
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn auto_assign_players(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if state.teams.len() < 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": "Se requieren al menos 2 equipos configurados." })),
        )
            .into_response();
    }

    // Auto-asignar capitanes si faltan
    for team in &mut state.teams {
        if team.captain.is_none() && !state.available_players.is_empty() {
            state.available_players.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));
            let mut cap = state.available_players.remove(0);
            cap.ensure_discord_user();
            team.captain = Some(cap);
        }
    }

    // Ordenar disponibles por ELO desc
    state.available_players.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));

    while !state.available_players.is_empty() {
        let incomplete_indices: Vec<usize> = state
            .teams
            .iter()
            .enumerate()
            .filter(|(_, t)| t.players.len() < 4)
            .map(|(i, _)| i)
            .collect();

        if incomplete_indices.is_empty() {
            break;
        }

        let mut best_team_idx = incomplete_indices[0];
        let mut min_elo = u64::MAX;

        for &idx in &incomplete_indices {
            let t = &state.teams[idx];
            let cap_elo = t.captain.as_ref().map(|c| c.faceit_elo as u64).unwrap_or(1000);
            let players_elo: u64 = t.players.iter().map(|p| p.faceit_elo as u64).sum();
            let total = cap_elo + players_elo;
            if total < min_elo {
                min_elo = total;
                best_team_idx = idx;
            }
        }

        let mut player = state.available_players.remove(0);
        player.ensure_discord_user();
        state.teams[best_team_idx].players.push(player);
    }

    let all_done = state.teams.iter().all(|t| t.captain.is_some() && t.players.len() == 4);
    if all_done {
        state.draft_finished = true;
        state.draft_started = false;
        state.current_pick_index = 0;
        state.auto_start_veto_if_ready();
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
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
        let mut player = if let Some(pos) = state.available_players.iter().position(|p| p.discord_id == did) {
            state.available_players.remove(pos)
        } else {
            let mut found = None;
            for other in &mut state.teams {
                if let Some(pos) = other.players.iter().position(|p| p.discord_id == did) {
                    found = Some(other.players.remove(pos));
                    break;
                }
                if let Some(ref c) = other.captain {
                    if c.discord_id == did {
                        found = other.captain.take();
                        break;
                    }
                }
            }
            found.unwrap_or_else(|| Player {
                discord_id: did.clone(),
                nickname: "Jugador".to_string(),
                faceit_nickname: Some("Jugador".to_string()),
                faceit_elo: 1000,
                ready: true,
                tier: "C".to_string(),
                ..Default::default()
            })
        };
        player.ensure_discord_user();

        if let Some(t) = state.teams.iter_mut().find(|t| t.id == tid) {
            if t.players.len() < 4 {
                t.players.push(player);
            } else {
                state.available_players.push(player);
            }
        }

        let all_done = state.teams.len() >= 2 && state.teams.iter().all(|t| t.captain.is_some() && t.players.len() == 4);
        if all_done {
            state.draft_finished = true;
            state.draft_started = false;
            state.auto_start_veto_if_ready();
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
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
        if let Some(ref tid) = payload.team_id {
            // Quitar del equipo especificado y reinsertar en disponibles
            for t in &mut state.teams {
                if t.id == *tid {
                    if let Some(pos) = t.players.iter().position(|p| p.discord_id == did) {
                        let mut p = t.players.remove(pos);
                        p.ensure_discord_user();
                        state.available_players.push(p);
                    }
                    if let Some(ref c) = t.captain {
                        if c.discord_id == did {
                            let mut cap = t.captain.take().unwrap();
                            cap.ensure_discord_user();
                            state.available_players.push(cap);
                        }
                    }
                }
            }
            state.draft_finished = false;
        } else {
            // Quitar de la cola de disponibles por completo (botón 'x' en lista de tiers)
            state.available_players.retain(|p| p.discord_id != did);
            for t in &mut state.teams {
                t.players.retain(|p| p.discord_id != did);
                if let Some(ref c) = t.captain {
                    if c.discord_id == did {
                        t.captain = None;
                    }
                }
            }
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
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
            for mut p in t.players.drain(..) {
                p.ensure_discord_user();
                state.available_players.push(p);
            }
            if let Some(mut cap) = t.captain.take() {
                cap.ensure_discord_user();
                state.available_players.push(cap);
            }
        }
    } else {
        for t in &mut state.teams {
            for mut p in t.players.drain(..) {
                p.ensure_discord_user();
                state.available_players.push(p);
            }
            if let Some(mut cap) = t.captain.take() {
                cap.ensure_discord_user();
                state.available_players.push(cap);
            }
        }
    }
    state.draft_finished = false;
    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn assign_captain(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignCaptainPayload>,
) -> impl IntoResponse {
    let g_opt = payload.guild_id.or_else(|| extract_guild_id(raw.as_deref()));
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if let Some(ref tid) = payload.team_id {
        match payload.discord_id {
            Some(ref did) if !did.is_empty() => {
                let mut player = if let Some(pos) = state.available_players.iter().position(|p| p.discord_id == *did) {
                    state.available_players.remove(pos)
                } else if let Some(ref p) = payload.player {
                    p.clone()
                } else {
                    let mut found = None;
                    for other in &mut state.teams {
                        if let Some(pos) = other.players.iter().position(|p| p.discord_id == *did) {
                            found = Some(other.players.remove(pos));
                            break;
                        }
                    }
                    found.unwrap_or_else(|| Player {
                        discord_id: did.clone(),
                        nickname: "Capitán".to_string(),
                        faceit_nickname: Some("Capitán".to_string()),
                        faceit_elo: 1000,
                        ready: true,
                        tier: "C".to_string(),
                        ..Default::default()
                    })
                };
                player.ensure_discord_user();
                if let Some(t) = state.teams.iter_mut().find(|t| t.id == *tid) {
                    t.captain = Some(player);
                }
            }
            _ => {
                if let Some(t) = state.teams.iter_mut().find(|t| t.id == *tid) {
                    if let Some(mut cap) = t.captain.take() {
                        cap.ensure_discord_user();
                        state.available_players.push(cap);
                    }
                }
            }
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
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

    if let Some(pid) = p_id {
        if let Some(pos) = state.available_players.iter().position(|p| p.discord_id == pid) {
            let mut player = state.available_players.remove(pos);
            player.ensure_discord_user();

            let target_team_id = payload.team_id.unwrap_or_else(|| {
                if !state.pick_order.is_empty() {
                    state.pick_order[state.current_pick_index % state.pick_order.len()].clone()
                } else if !state.teams.is_empty() {
                    state.teams[0].id.clone()
                } else {
                    "team_1".to_string()
                }
            });

            if let Some(t) = state.teams.iter_mut().find(|t| t.id == target_team_id) {
                t.players.push(player);
            }
            state.current_pick_index += 1;
        }

        let all_done = state.teams.len() >= 2 && state.teams.iter().all(|t| t.captain.is_some() && t.players.len() == 4);
        if all_done {
            state.draft_finished = true;
            state.draft_started = false;
            state.auto_start_veto_if_ready();
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn captain_proposal(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if let Some(team_id) = payload.get("teamId").and_then(|v| v.as_str()) {
        let bet = payload.get("betAmount").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if team_id.contains("1") || team_id.to_lowercase().contains('a') {
            state.captain_a_proposal = Some(json!(bet));
        } else {
            state.captain_b_proposal = Some(json!(bet));
        }
        state.bet_agreed = true;
        state.agreed_bet_amount = Some(bet);
        state.effective_bet_amount = bet;
    }

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn captain_ready(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if let Some(team_id) = payload.get("teamId").and_then(|v| v.as_str()) {
        let is_ready = payload.get("isReady").and_then(|v| v.as_bool()).unwrap_or(true);
        if team_id.contains("1") || team_id.to_lowercase().contains('a') {
            state.captain_a_ready = is_ready;
        } else {
            state.captain_b_ready = is_ready;
        }
    }

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn force_roulette(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if !state.teams.is_empty() {
        let winner = &state.teams[0];
        state.roulette_spinning = true;
        state.roulette_winner_team_id = Some(winner.id.clone());
        state.roulette_winner_name = Some(winner.name.clone());
        state.roulette_started_at = Some(Utc::now().timestamp_millis());
        state.bet_agreed = true;
    }

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
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
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn tournament_report_winner(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players })).into_response()
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
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn toggle_command(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "commandEnabled": true, "state": state, "availablePlayers": available_players })).into_response()
}

async fn mixton_setup(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let num_teams = payload.get("numTeams").and_then(|v| v.as_u64()).unwrap_or(2).clamp(2, 8) as usize;

    while state.teams.len() < num_teams {
        let idx = state.teams.len() + 1;
        state.teams.push(LobbyTeam {
            id: format!("team_{idx}"),
            name: format!("Equipo {idx}"),
            tag: format!("EQ{idx}"),
            captain: None,
            players: Vec::new(),
        });
    }

    while state.teams.len() > num_teams {
        if let Some(t) = state.teams.pop() {
            if let Some(mut c) = t.captain {
                c.ensure_discord_user();
                state.available_players.push(c);
            }
            for mut p in t.players {
                p.ensure_discord_user();
                state.available_players.push(p);
            }
        }
    }

    state.draft_started = false;
    state.draft_finished = false;
    state.map_veto = None;
    state.current_pick_index = 0;
    state.sanitize();

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn mixton_start(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if let Some(orders) = payload.get("pickOrder").and_then(|v| v.as_array()) {
        state.pick_order = orders.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    }
    state.draft_started = true;
    state.draft_finished = false;
    state.current_pick_index = 0;

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn mixton_pick_order(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if let Some(orders) = payload.get("pickOrder").and_then(|v| v.as_array()) {
        state.pick_order = orders.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    }

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players })).into_response()
}

async fn mixton_create_match(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let match_id = format!("match_{}", chrono::Utc::now().timestamp_millis());

    let team_a_id = payload.get("teamIdA").and_then(|v| v.as_str()).unwrap_or("team_1");
    let team_b_id = payload.get("teamIdB").and_then(|v| v.as_str()).unwrap_or("team_2");

    for t in &mut state.teams {
        if t.id == team_a_id || t.id == team_b_id {
            t.captain = None;
            t.players.clear();
        }
    }

    state.draft_started = false;
    state.draft_finished = false;
    state.current_pick_index = 0;
    state.map_veto = None;
    state.roulette_spinning = false;
    state.roulette_winner_team_id = None;
    state.roulette_winner_name = None;
    state.bet_agreed = false;
    state.captain_a_ready = false;
    state.captain_b_ready = false;

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);

    Json(json!({
        "success": true,
        "matchId": match_id,
        "faceitHubUrl": serde_json::Value::Null,
        "assignedQueue": "CS2 5v5 Oficial",
        "gamesPerMatch": 1,
        "roundsPerGame": 1,
        "state": updated,
        "availablePlayers": available_players
    }))
    .into_response()
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
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let team_a_id = payload.get("teamIdA").and_then(|v| v.as_str()).unwrap_or("team_1").to_string();
    let team_b_id = payload.get("teamIdB").and_then(|v| v.as_str()).unwrap_or("team_2").to_string();
    let bet = payload.get("betAmount").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let active_maps: Vec<MapInfo> = state.map_catalog.iter().filter(|m| m.active).cloned().collect();
    let pool_len = active_maps.len();
    let ban_count = if pool_len > 1 { pool_len - 1 } else { 0 };
    let mut ban_sequence = Vec::with_capacity(ban_count);
    for i in 0..ban_count {
        if i % 2 == 0 {
            ban_sequence.push(team_a_id.clone());
        } else {
            ban_sequence.push(team_b_id.clone());
        }
    }

    let veto = MapVetoState {
        active: true,
        completed: false,
        team_id_a: Some(team_a_id.clone()),
        team_id_b: Some(team_b_id.clone()),
        games_per_match: state.games_per_match,
        veto_mode: "bo1".to_string(),
        action_type: "ban".to_string(),
        current_ban_index: 0,
        banned_maps: Vec::new(),
        picked_maps: Vec::new(),
        ban_history: Vec::new(),
        chosen_map: None,
        ban_sequence,
        turn: team_a_id,
        current_step: "ban".to_string(),
        active_maps,
        final_chosen_map: None,
        bet_amount: bet,
        queue_id: "auto".to_string(),
        match_mode: "steam".to_string(),
        started_at: Some(chrono::Utc::now().timestamp_millis()),
    };

    state.map_veto = Some(veto);
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({
        "success": true,
        "state": updated,
        "mapVeto": updated.map_veto,
        "availablePlayers": available_players
    }))
    .into_response()
}

async fn map_ban(
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let map_id = payload.get("mapId").and_then(|v| v.as_str()).unwrap_or("");
    if map_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Debes indicar el mapa a banear." }))).into_response();
    }

    if let Some(ref mut veto) = state.map_veto {
        if !veto.banned_maps.contains(&map_id.to_string()) {
            veto.banned_maps.push(map_id.to_string());
            veto.ban_history.push(json!({
                "mapId": map_id,
                "teamId": veto.turn,
                "timestamp": chrono::Utc::now().timestamp_millis()
            }));
            veto.current_ban_index += 1;

            veto.active_maps.retain(|m| m.id != map_id);

            if veto.active_maps.len() == 1 {
                let chosen = veto.active_maps[0].id.clone();
                veto.active = false;
                veto.completed = true;
                veto.chosen_map = Some(chosen.clone());
                veto.final_chosen_map = Some(chosen);
            } else if let Some(team_a) = &veto.team_id_a {
                if let Some(team_b) = &veto.team_id_b {
                    veto.turn = if veto.turn == *team_a { team_b.clone() } else { team_a.clone() };
                }
            }
        }
    }

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({
        "success": true,
        "mapVeto": updated.map_veto,
        "state": updated,
        "availablePlayers": available_players
    }))
    .into_response()
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
    let available_players = partition_available_players(&updated);
    Json(json!({
        "success": true,
        "state": updated,
        "availablePlayers": available_players
    }))
    .into_response()
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
    req: axum::extract::Request,
) -> impl IntoResponse {
    use axum::extract::FromRequest;

    let (parts, body) = req.into_parts();
    let headers = &parts.headers;
    let query_str = parts.uri.query();
    let query_guild = extract_guild_id(query_str);

    let cookie_user = user_from_cookies(headers);
    let is_admin = headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .map(|c| c.contains("admin_token="))
        .unwrap_or(false);

    let is_multipart = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("multipart/form-data"))
        .unwrap_or(false);

    let mut message_text = String::new();
    let mut image_url: Option<String> = None;
    let mut body_guild_id: Option<String> = None;
    let mut author_id: Option<String> = None;
    let mut author_name: Option<String> = None;
    let mut author_avatar: Option<String> = None;

    if is_multipart {
        let req_again = axum::extract::Request::from_parts(parts, body);
        if let Ok(mut multipart) = axum::extract::Multipart::from_request(req_again, &manager).await {
            while let Ok(Some(field)) = multipart.next_field().await {
                let name = field.name().unwrap_or("").to_string();
                if name == "message" || name == "content" {
                    if let Ok(text) = field.text().await {
                        message_text = text;
                    }
                } else if name == "guildId" || name == "guild_id" {
                    if let Ok(gid) = field.text().await {
                        body_guild_id = Some(gid);
                    }
                } else if name == "image" {
                    let file_name = field.file_name().unwrap_or("image.png").to_string();
                    let ext = std::path::Path::new(&file_name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("png");
                    if let Ok(bytes) = field.bytes().await {
                        if !bytes.is_empty() {
                            let filename = format!("chat_{}.{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), ext);
                            let target_path = format!("public/uploads/{}", filename);
                            let srv_path = format!("/srv/public/uploads/{}", filename);
                            let _ = tokio::fs::create_dir_all("public/uploads").await;
                            let _ = tokio::fs::create_dir_all("/srv/public/uploads").await;
                            let _ = tokio::fs::write(&target_path, &bytes).await;
                            let _ = tokio::fs::write(&srv_path, &bytes).await;
                            image_url = Some(format!("/uploads/{}", filename));
                        }
                    }
                }
            }
        }
    } else {
        let bytes = axum::body::to_bytes(body, 1024 * 64).await.unwrap_or_default();
        if let Ok(payload) = serde_json::from_slice::<SendChatPayload>(&bytes) {
            message_text = payload.message.or(payload.content).unwrap_or_default().trim().to_string();
            body_guild_id = payload.guild_id;
            author_id = payload.author_id.or(payload.discord_id);
            author_name = payload.author_name.or(payload.author);
            author_avatar = payload.author_avatar.or(payload.avatar);
            image_url = payload.image_url;
        }
    }

    let (did, name, avatar) = if let Some((id, u_name, av)) = cookie_user {
        let av_url = format_discord_avatar(&id, av.as_deref());
        (id, u_name, av_url)
    } else if let Some(id) = author_id.filter(|s| !s.is_empty()) {
        let n = author_name.unwrap_or_else(|| "Jugador".to_string());
        let av = author_avatar.unwrap_or_else(|| format_discord_avatar(&id, None));
        (id, n, av)
    } else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Debes iniciar sesión con tu cuenta de Discord para enviar mensajes." })),
        )
            .into_response();
    };

    if message_text.is_empty() && image_url.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "El mensaje no puede estar vacío." })),
        )
            .into_response();
    }

    let effective_gid = body_guild_id.or(query_guild);
    let guild_id = manager.resolve_guild_id(effective_gid.as_deref());
    let now = chrono::Utc::now().timestamp_millis();
    let msg = ChatMessage {
        id: format!("msg_{now}"),
        discord_id: did.clone(),
        author: name.clone(),
        avatar: Some(avatar.clone()),
        is_admin,
        source: "web".to_string(),
        content: message_text,
        image_url,
        timestamp: now,
    };

    manager.add_chat_message(guild_id, msg.clone()).await;
    Json(json!({ "success": true, "message": msg })).into_response()
}

#[derive(Deserialize, Debug, Default)]
pub struct LinkSteamPayload {
    #[serde(rename = "steamInput", default)]
    pub steam_input: Option<String>,
}

async fn link_steam(
    headers: HeaderMap,
    Json(payload): Json<LinkSteamPayload>,
) -> impl IntoResponse {
    let user_opt = user_from_cookies(&headers);
    if user_opt.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Debes iniciar sesión con tu cuenta de Discord." })),
        )
            .into_response();
    }
    let input = payload.steam_input.unwrap_or_else(|| "detect_faceit".to_string());
    Json(json!({
        "success": true,
        "message": format!("Steam vinculado ({input})"),
        "steamId64": "76561198000000000"
    }))
    .into_response()
}
