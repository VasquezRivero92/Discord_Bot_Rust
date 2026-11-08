//! Endpoints REST para el Lobby de Mixtón y CS2 en Axum.

use crate::auth::{
    resolve_guild_for_user, OptionalLobbyUser, RequireAdmin, RequireUser, SessionVerifier,
};
use crate::maps::MapInfo;
use crate::state::{
    format_discord_avatar, partition_available_players, ChatMessage, DiscordUserDetails,
    LobbyManager, LobbyTeam, Player,
};
use axum::{
    extract::{DefaultBodyLimit, RawQuery, State},
    http::{header, StatusCode},
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
    pub auth: Arc<dyn SessionVerifier>,
    pub uploads_dir: std::path::PathBuf,
}

#[derive(Clone)]
pub(crate) struct LobbyCtx {
    pub manager: Arc<LobbyManager>,
    pub auth: Arc<dyn SessionVerifier>,
    pub uploads_dir: std::path::PathBuf,
}

impl axum::extract::FromRef<LobbyCtx> for Arc<LobbyManager> {
    fn from_ref(ctx: &LobbyCtx) -> Self {
        ctx.manager.clone()
    }
}

impl axum::extract::FromRef<LobbyCtx> for Arc<dyn SessionVerifier> {
    fn from_ref(ctx: &LobbyCtx) -> Self {
        ctx.auth.clone()
    }
}

impl axum::extract::FromRef<LobbyCtx> for std::path::PathBuf {
    fn from_ref(ctx: &LobbyCtx) -> Self {
        ctx.uploads_dir.clone()
    }
}

fn extract_guild_id(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        if (k == "guildId" || k == "guild_id") && !v.is_empty() {
            return Some(v.into_owned());
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
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
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
    #[serde(rename = "playerDiscordId", default)]
    pub player_discord_id: Option<String>,
}


#[derive(Deserialize, Debug, Default)]
pub struct OverrideTierPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "playerDiscordId", default)]
    pub player_discord_id: Option<String>,
    pub tier: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct ToggleCommandPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Debug, Default)]
pub struct LoadTeamPayload {
    #[serde(rename = "guildId", default)]
    pub guild_id: Option<String>,
    #[serde(rename = "teamId", default)]
    pub team_id: Option<String>,
    #[serde(rename = "captainDiscordId", default)]
    pub captain_discord_id: Option<String>,
    #[serde(rename = "playerDiscordIds", default)]
    pub player_discord_ids: Option<Vec<String>>,
}

pub fn router(state: LobbyApiState) -> Router {
    let ctx = LobbyCtx {
        manager: state.manager,
        auth: state.auth,
        uploads_dir: state.uploads_dir,
    };
    Router::new()
        // Estado y lecturas básicas
        .route("/api/lobby/state", get(get_state))
        .route("/api/lobby/mixton-state", get(get_state))
        .route("/api/lobby/guilds", get(guilds))
        // `/api/lobby/matches-history` lo monta el crate web con el historial de Firestore; el
        // detalle `/{match_id}` sigue en el proxy (antes devolvía una lista vacía simulada).
        .route(
            "/api/lobby/map-catalog",
            get(map_catalog).post(update_map_catalog),
        )
        .route("/api/lobby/mixton-queues", get(mixton_queues))
        // Dinero (wallet, player-balance, quick-add-balance, recharge-yape): sin escritura a Firestore en Rust todavía; se dejan sin registrar para que el proxy las envíe al legado Node. NO simular saldo.
        .route("/api/lobby/users", get(users))
        // `/api/lobby/steam-avatar(s)` los sirve el crate web (`public_api::steam_avatar_router`).
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
        .route(
            "/api/lobby/mixton-auto-assign-captains",
            post(auto_assign_captains),
        )
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
        .route("/api/lobby/mixton-load-team", post(load_team))
        .route("/api/lobby/mixton-override-tier", post(override_tier))
        .route("/api/lobby/mixton-toggle-command", post(toggle_command))
        .route("/api/lobby/mixton-assign-captain", post(assign_captain))
        .route("/api/lobby/mixton-pick", post(pick_player))
        .route("/api/lobby/mixton-captain-proposal", post(captain_proposal))
        .route("/api/lobby/mixton-captain-ready", post(captain_ready))
        .route("/api/lobby/mixton-force-roulette", post(force_roulette))
        // Modo torneo y configuración de rondas
        .route("/api/lobby/tournament-config", post(tournament_config))
        .route(
            "/api/lobby/tournament-report-winner",
            post(tournament_report_winner),
        )
        .route("/api/lobby/mixton-games-config", post(games_config))
        .route("/api/lobby/mixton-rounds-config", post(games_config))
        // Acciones generales de partida y configuración
        .route("/api/lobby/mixton-setup", post(mixton_setup))
        .route("/api/lobby/mixton-start", post(mixton_start))
        .route("/api/lobby/mixton-pick-order", post(mixton_pick_order))
        // Sin implementación real en Rust todavía (no se simula éxito): mixton-check-bet-status, mixton-load-team, mixton-override-tier, sync-faceit-from-steam, verify-faceit, mixton-toggle-command, create-match. Caen al proxy del legado.
        // Chat
        .route("/api/lobby/mixton-chat-history", get(chat_history))
        .route(
            "/api/lobby/mixton-chat-message",
            post(chat_message).layer(DefaultBodyLimit::max(9 * 1024 * 1024)),
        )
        .route(
            "/api/lobby/mixton-chat-send",
            post(chat_message).layer(DefaultBodyLimit::max(9 * 1024 * 1024)),
        )
        .route("/api/lobby/mixton-chat-incoming", post(chat_incoming))
        .with_state(ctx)
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
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<MapCatalogPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
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
        }))
        .into_response()
    } else {
        Json(json!({ "success": true, "catalog": state.map_catalog })).into_response()
    }
}

async fn mixton_queues(_user: RequireUser) -> impl IntoResponse {
    Json(json!({ "queues": [] }))
}

async fn guilds(_user: RequireUser, State(manager): State<Arc<LobbyManager>>) -> impl IntoResponse {
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

async fn users(_user: RequireUser) -> impl IntoResponse {
    Json(json!({ "users": [] }))
}

async fn get_state(
    OptionalLobbyUser(user_opt): OptionalLobbyUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let effective_guild = if let Some(ref u) = user_opt {
        match resolve_guild_for_user(u, g_opt.as_deref(), false) {
            Ok(g) => g,
            Err(_) => g_opt,
        }
    } else {
        g_opt
    };
    let guild_id = manager.resolve_guild_id(effective_guild.as_deref());
    let state = manager.get_state(guild_id).await;
    let available_players = partition_available_players(&state);
    let mut resp = json!({
        "state": state,
        "availablePlayers": available_players,
        "commandEnabled": true,
        "guildName": "MachuPeek",
        "guildId": guild_id
    });
    if let Some(user) = user_opt {
        let avatar_url = format_discord_avatar(&user.user_id, user.avatar.as_deref());
        resp["currentUser"] = json!({
            "userId": user.user_id,
            "username": user.username,
            "avatar": user.avatar,
            "avatarURL": avatar_url,
            "isAdmin": user.is_admin_like()
        });
    }
    Json(resp).into_response()
}

async fn join(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<JoinPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let player = if user.is_admin_like() {
        payload.player
    } else {
        own_player(&user, payload.player)
    };
    let state = manager.join_queue(guild_id, player).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players }))
        .into_response()
}

/// Lo que un usuario sin permisos de admin puede poner en la cola: solo a sí mismo, con la
/// identidad de su sesión. Del cuerpo no se confía la SteamID (el lanzamiento la toma de su
/// cuenta vinculada), ni el ELO, el nivel o el tier, que deciden su lugar en el draft.
fn own_player(user: &RequireUser, requested: Player) -> Player {
    let username = user.username.clone();
    let mut player = Player {
        discord_id: user.user_id.clone(),
        nickname: username.clone(),
        faceit_nickname: Some(username.clone()),
        steam_id_64: None,
        avatar: user.avatar.clone(),
        faceit_elo: 1000,
        faceit_level: Some(1),
        ready: requested.ready,
        tier: "C".to_string(),
        mixton_tier_override: None,
        discord_user: Some(DiscordUserDetails {
            username: username.clone(),
            display_name: username,
            avatar_url: format_discord_avatar(&user.user_id, user.avatar.as_deref()),
        }),
    };
    player.ensure_discord_user();
    player
}

async fn player_ready(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());

    let discord_id = payload.discord_id.unwrap_or_else(|| user.user_id.clone());
    let username = user.username.clone();
    let avatar = user.avatar.clone();

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
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let discord_id = payload.discord_id.unwrap_or_else(|| user.user_id.clone());

    let state = manager.leave_queue(guild_id, &discord_id).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players }))
        .into_response()
}

async fn unready(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<UserActionPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let discord_id = payload.discord_id.unwrap_or_else(|| user.user_id.clone());

    let state = manager.set_ready(guild_id, &discord_id, false).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players }))
        .into_response()
}

async fn reset(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    manager.cancel_roulette(guild_id);
    let state = manager.reset_lobby(guild_id).await;
    Json(json!({ "success": true, "state": state })).into_response()
}

async fn reset_players(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    state.available_players.clear();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

async fn auto_assign_captains(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let ready = state.available_players.clone();
    match crate::draft::auto_assign_captains(&mut state, ready) {
        Ok(_) => {
            manager.cancel_roulette(guild_id);
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err(msg) => (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response(),
    }
}

/// Autocompleta equipos de forma balanceada (Admin).
/// Port exacto de lobby.js:3207-3300.
async fn auto_assign_players(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    match crate::draft::auto_balance(&mut state) {
        Ok(_count) => {
            manager.cancel_roulette(guild_id);
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

async fn assign_player(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignPlayerPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let p_id = payload.player_id.or(payload.discord_id);
    if let (Some(tid), Some(did)) = (payload.team_id, p_id) {
        let mut player = if let Some(pos) = state
            .available_players
            .iter()
            .position(|p| p.discord_id == did)
        {
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

        let all_done = state.teams.len() >= 2
            && state
                .teams
                .iter()
                .all(|t| t.captain.is_some() && t.players.len() == 4);
        if all_done {
            state.draft_finished = true;
            state.draft_started = false;
            state.auto_start_veto_if_ready();
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

async fn remove_player(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignPlayerPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
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
                            if let Some(mut cap) = t.captain.take() {
                                cap.ensure_discord_user();
                                state.available_players.push(cap);
                            }
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
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

async fn clear_team(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<ClearTeamPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
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
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

async fn assign_captain(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<AssignCaptainPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let team_id = match payload.team_id.as_deref() {
        Some(tid) if !tid.is_empty() => tid,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Equipo no encontrado." })),
            )
                .into_response();
        }
    };

    let did_opt = payload
        .discord_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let player_opt = match did_opt {
        None => None,
        Some(did) => {
            let found = state
                .available_players
                .iter()
                .find(|p| p.discord_id == did)
                .cloned()
                .or_else(|| {
                    for t in &state.teams {
                        if let Some(p) = t.players.iter().find(|p| p.discord_id == did) {
                            return Some(p.clone());
                        }
                        if let Some(ref c) = t.captain {
                            if c.discord_id == did {
                                return Some(c.clone());
                            }
                        }
                    }
                    None
                })
                .or_else(|| payload.player.filter(|p| p.discord_id == did));

            match found {
                Some(p) => Some(p),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({ "error": "Jugador no registrado." })),
                    )
                        .into_response();
                }
            }
        }
    };

    match crate::draft::assign_captain(&mut state, team_id, player_opt) {
        Ok(()) => {
            manager.cancel_roulette(guild_id);
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

/// Realiza una selección de jugador en el draft (Capitán de turno o Admin).
/// Port exacto de lobby.js:3116-3205.
async fn pick_player(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<PickPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());

    let target_discord_id = payload
        .player_discord_id
        .or(payload.player_id)
        .or(payload.discord_id)
        .unwrap_or_default();

    let mut state = manager.get_state(guild_id).await;
    let actor_id = &user.user_id;
    let is_admin = user.is_admin_like();

    match crate::draft::pick_player(&mut state, actor_id, is_admin, &target_discord_id) {
        Ok(()) => {
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

async fn captain_proposal(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = payload
        .get("guildId")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let team_id = match payload.get("teamId").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Equipo no válido para proponer apuesta." })),
            )
                .into_response();
        }
    };

    let bet = match payload.get("betAmount") {
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(serde_json::Value::String(s)) => s.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };

    let actor_id = &user.user_id;
    let is_admin = user.is_admin_like();

    match crate::draft::set_captain_proposal(&mut state, team_id, actor_id, is_admin, bet) {
        Ok(()) => {
            manager.cancel_roulette(guild_id);
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

async fn captain_ready(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = payload
        .get("guildId")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let team_id = match payload.get("teamId").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Equipo no válido para confirmar estado." })),
            )
                .into_response();
        }
    };

    let is_ready = payload
        .get("isReady")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let bet = match payload.get("betAmount") {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    };

    let actor_id = &user.user_id;
    let is_admin = user.is_admin_like();

    match crate::draft::set_captain_ready(&mut state, team_id, actor_id, is_admin, is_ready, bet) {
        Ok(should_start_roulette) => {
            if should_start_roulette {
                manager.schedule_roulette(
                    guild_id,
                    7000,
                    crate::roulette::RouletteTimings::default(),
                );
            } else {
                manager.cancel_roulette(guild_id);
            }
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

async fn force_roulette(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if state.teams.len() < 2 || state.teams[0].captain.is_none() || state.teams[1].captain.is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Se requieren al menos 2 equipos con capitanes asignados." })),
        )
            .into_response();
    }

    state.captain_a_ready = true;
    state.captain_b_ready = true;

    let prop_a = crate::draft::proposal_to_f64(&state.captain_a_proposal);
    let prop_b = crate::draft::proposal_to_f64(&state.captain_b_proposal);

    let differing = match (prop_a, prop_b) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    };

    let default_bet = if differing {
        let chosen = prop_a.or(prop_b).unwrap_or(0.0);
        state.captain_a_proposal = Some(json!(chosen));
        state.captain_b_proposal = Some(json!(chosen));
        chosen
    } else {
        prop_a.unwrap_or(0.0)
    };

    state.bet_agreed = true;
    state.agreed_bet_amount = Some(default_bet);

    state.sanitize();
    manager.update_state(guild_id, state).await;
    manager.schedule_roulette(guild_id, 400, crate::roulette::RouletteTimings::default());

    Json(json!({ "success": true, "message": "Ruleta iniciada de inmediato." })).into_response()
}

async fn tournament_config(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<TournamentConfigPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
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
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

async fn tournament_report_winner(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let state = manager.get_state(guild_id).await;
    let available_players = partition_available_players(&state);
    Json(json!({ "success": true, "state": state, "availablePlayers": available_players }))
        .into_response()
}

async fn games_config(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<GamesConfigPayload>,
) -> impl IntoResponse {
    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    if let Some(games) = payload.games_per_match.or(payload.rounds_per_game) {
        state.games_per_match = games;
        state.rounds_per_game = games;
    }

    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

async fn mixton_setup(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    manager.cancel_roulette(guild_id);
    let mut state = manager.get_state(guild_id).await;

    let num_teams = payload
        .get("numTeams")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .clamp(2, 8) as usize;

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
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}

/// Inicia manualmente el draft de jugadores (Admin).
/// Port exacto de lobby.js:3059-3115.
async fn mixton_start(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let pick_order = payload
        .get("pickOrder")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<String>>()
        });

    let bet_amount = payload.get("betAmount").and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    });

    match crate::draft::start_draft(&mut state, pick_order, bet_amount) {
        Ok(()) => {
            manager.cancel_roulette(guild_id);
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

/// Guarda el orden de elección de capitanes (Admin).
/// Port exacto de lobby.js:2730-2757.
async fn mixton_pick_order(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let orders_arr = match payload.get("pickOrder").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "El orden debe ser un array." })),
            )
                .into_response();
        }
    };

    let pick_order: Vec<String> = orders_arr
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();

    match crate::draft::set_pick_order(&mut state, &pick_order) {
        Ok(()) => {
            state.sanitize();
            let updated = manager.update_state(guild_id, state).await;
            let available_players = partition_available_players(&updated);
            Json(
                json!({ "success": true, "state": updated, "availablePlayers": available_players }),
            )
            .into_response()
        }
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

async fn start_veto(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let team_a_id = payload
        .get("teamIdA")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let team_b_id = payload
        .get("teamIdB")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if team_a_id.is_empty() || team_b_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Debes seleccionar dos equipos para el encuentro." })),
        )
            .into_response();
    }

    let team_a = match state.teams.iter().find(|t| t.id == team_a_id) {
        Some(t) => t.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Equipos no encontrados." })),
            )
                .into_response();
        }
    };
    let team_b = match state.teams.iter().find(|t| t.id == team_b_id) {
        Some(t) => t.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Equipos no encontrados." })),
            )
                .into_response();
        }
    };

    let role_str = user.role.as_deref().unwrap_or("");
    let is_admin = role_str == "global_admin" || role_str == "admin" || user.is_admin;
    let is_captain_of_match = team_a
        .captain
        .as_ref()
        .map(|c| c.discord_id == user.user_id)
        .unwrap_or(false)
        || team_b
            .captain
            .as_ref()
            .map(|c| c.discord_id == user.user_id)
            .unwrap_or(false);

    if !is_admin && !is_captain_of_match {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "Solo los capitanes de la partida o un administrador pueden iniciar la fase de veto."
            })),
        )
            .into_response();
    }

    let a_complete = team_a.captain.is_some() && team_a.players.len() == 4;
    let b_complete = team_b.captain.is_some() && team_b.players.len() == 4;
    if !a_complete || !b_complete {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Ambos equipos deben estar completos (5 jugadores c/u)." })),
        )
            .into_response();
    }

    let active_maps: Vec<MapInfo> = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    if active_maps.len() < 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Se requieren al menos 2 mapas activos para iniciar el veto. Configura los mapas en el panel administrativo."
            })),
        )
            .into_response();
    }

    let raw_games = payload
        .get("gamesPerMatch")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(state.games_per_match);
    let games_per_match = match raw_games {
        1 | 3 | 5 => raw_games,
        _ => 1,
    };

    let bet = payload
        .get("betAmount")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let queue_id = payload
        .get("queueId")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    let match_mode = payload
        .get("matchMode")
        .and_then(|v| v.as_str())
        .unwrap_or("steam");

    let veto = match crate::veto::init_map_veto_state(
        &team_a_id,
        &team_b_id,
        active_maps,
        games_per_match,
        bet,
        queue_id,
        match_mode,
        chrono::Utc::now().timestamp_millis(),
    ) {
        Ok(v) => v,
        Err((code, msg)) => return (code, Json(json!({ "error": msg }))).into_response(),
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
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&user, g_opt.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let map_id = payload.get("mapId").and_then(|v| v.as_str()).unwrap_or("");
    if map_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Debes indicar el mapa a banear." })),
        )
            .into_response();
    }

    let veto = match state.map_veto.as_ref() {
        Some(v) if v.active && !v.completed => v,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "No hay una fase de veto de mapas activa." })),
            )
                .into_response();
        }
    };

    let is_pick_mode =
        veto.action_type == "pick" || veto.veto_mode == "bo3" || veto.veto_mode == "bo5";
    if !is_pick_mode && veto.banned_maps.contains(&map_id.to_string()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Este mapa ya fue baneado." })),
        )
            .into_response();
    }
    if is_pick_mode
        && veto
            .picked_maps
            .iter()
            .any(|p| p.get("mapId").and_then(|v| v.as_str()) == Some(map_id))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Este mapa ya fue seleccionado por un equipo." })),
        )
            .into_response();
    }

    let pool = if !veto.pool.is_empty() {
        veto.pool.clone()
    } else {
        state
            .map_catalog
            .iter()
            .filter(|m| m.active)
            .cloned()
            .collect()
    };
    if !pool.iter().any(|m| m.id == map_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Mapa no válido en el pool." })),
        )
            .into_response();
    }

    let role_str = user.role.as_deref().unwrap_or("");
    let is_admin = role_str == "global_admin" || role_str == "admin" || user.is_admin;
    if let Err((code, msg)) =
        crate::veto::verify_veto_turn(&state, veto, &user.user_id, role_str, is_admin)
    {
        return (code, Json(json!({ "error": msg }))).into_response();
    }

    let actor_nick = user.username.as_str();
    let now_ms = chrono::Utc::now().timestamp_millis();
    if let Err((code, msg)) =
        crate::veto::apply_veto_map_action(&mut state, map_id, Some(actor_nick), now_ms, None)
    {
        return (code, Json(json!({ "error": msg }))).into_response();
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
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    crate::veto::cancel_veto(&mut state);
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
    OptionalLobbyUser(user_opt): OptionalLobbyUser,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let effective_guild = if let Some(ref u) = user_opt {
        match resolve_guild_for_user(u, g_opt.as_deref(), false) {
            Ok(g) => g,
            Err(_) => g_opt,
        }
    } else {
        g_opt
    };
    let guild_id = manager.resolve_guild_id(effective_guild.as_deref());
    let mut history = manager.get_chat_history(guild_id).await;

    if history.is_empty() {
        let legacy_base = std::env::var("LEGACY_URL").unwrap_or_else(|_| "http://bot:3001".to_string());
        let client = reqwest::Client::new();
        let url = format!("{legacy_base}/api/lobby/mixton-chat-history?guildId={guild_id}");
        if let Ok(res) = client.get(&url).send().await {
            if let Ok(val) = res.json::<serde_json::Value>().await {
                if let Some(msgs) = val.get("messages").and_then(|m| m.as_array()) {
                    for m in msgs {
                        if let Ok(chat_msg) = serde_json::from_value::<ChatMessage>(m.clone()) {
                            manager.add_chat_message(guild_id, chat_msg).await;
                        }
                    }
                    history = manager.get_chat_history(guild_id).await;
                }
            }
        }
    }

    Json(json!({
        "success": true,
        "messages": history
    }))
    .into_response()
}

/// Resuelve la URL del avatar de Discord según la lógica del legado de Mixtón:
/// 1. Si `avatar` empieza por `http://` o `https://`, se devuelve tal cual.
/// 2. Si `avatar` es hexadecimal/guion bajo (`^[0-9a-fA-F_]+$`) y hay `user_id`, devuelve CDN de Discord.
/// 3. Si hay `user_id` numérico, calcula el avatar por defecto `(user_id >> 22) % 6`.
/// 4. En cualquier otro caso, devuelve avatar por defecto 0.
pub fn resolve_discord_avatar_url(user_id: Option<&str>, avatar: Option<&str>) -> String {
    if let Some(av) = avatar {
        if av.starts_with("http://") || av.starts_with("https://") {
            return av.to_string();
        }
        if !av.is_empty() && av.chars().all(|c| c.is_ascii_hexdigit() || c == '_') {
            if let Some(uid) = user_id {
                let uid = uid.trim();
                if !uid.is_empty() {
                    return format!("https://cdn.discordapp.com/avatars/{uid}/{av}.png?size=128");
                }
            }
        }
    }

    if let Some(uid) = user_id {
        let uid = uid.trim();
        if let Ok(num) = uid.parse::<u128>() {
            let default_index = ((num >> 22) % 6) as u32;
            return format!("https://cdn.discordapp.com/embed/avatars/{default_index}.png");
        }
    }

    "https://cdn.discordapp.com/embed/avatars/0.png".to_string()
}

/// Genera una cadena alfanumérica pseudoaleatoria reutilizando `random_unit()` sin dependencias nuevas.
pub fn random_alphanumeric(len: usize) -> String {
    const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = String::with_capacity(len);
    for _ in 0..len {
        let u = crate::roulette::random_unit();
        let idx = (u * CHARSET.len() as f64).floor() as usize % CHARSET.len();
        result.push(CHARSET[idx] as char);
    }
    result
}

async fn chat_message(
    user: RequireUser,
    State(manager): State<Arc<LobbyManager>>,
    State(uploads_dir): State<std::path::PathBuf>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    use axum::extract::FromRequest;

    let (parts, body) = req.into_parts();
    let query_guild = extract_guild_id(parts.uri.query());

    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let is_multipart = content_type.starts_with("multipart/form-data");

    let mut message_text = String::new();
    let mut body_guild_id: Option<String> = None;
    let mut saved_image_filename: Option<String> = None;
    let mut saved_image_path: Option<std::path::PathBuf> = None;

    if is_multipart {
        let req_again = axum::extract::Request::from_parts(parts, body);
        let mut multipart = match axum::extract::Multipart::from_request(req_again, &manager).await
        {
            Ok(m) => m,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "Solo se permiten archivos de imagen (PNG, JPG, WEBP, GIF)." })),
                ).into_response();
            }
        };

        loop {
            let field_opt = match multipart.next_field().await {
                Ok(f) => f,
                Err(_) => {
                    // Si falla por límite de tamaño de Axum o error de red
                    if let Some(ref path) = saved_image_path {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "El archivo supera el máximo de 8 MB." })),
                    )
                        .into_response();
                }
            };

            let mut field = match field_opt {
                Some(f) => f,
                None => break,
            };

            let name = field.name().unwrap_or("").to_string();
            if name == "message" || name == "content" {
                match field.text().await {
                    Ok(text) => message_text = text,
                    Err(_) => {
                        if let Some(ref path) = saved_image_path {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": "El mensaje no puede superar los 400 caracteres." })),
                        ).into_response();
                    }
                }
            } else if name == "guildId" || name == "guild_id" {
                if let Ok(gid) = field.text().await {
                    body_guild_id = Some(gid);
                }
            } else if name == "image" {
                let file_name = field.file_name().unwrap_or("").to_string();
                let field_ct = field.content_type().unwrap_or("").to_string();

                // Si no hay nombre de archivo ni content-type, puede ser un campo file vacío del formulario
                if file_name.trim().is_empty() && field_ct.is_empty() {
                    continue;
                }

                // 1. Content-type debe empezar por image/
                if !field_ct.starts_with("image/") {
                    if let Some(ref path) = saved_image_path {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "Solo se permiten archivos de imagen (PNG, JPG, WEBP, GIF)." })),
                    ).into_response();
                }

                // 2. Extensión debe ser png, jpg, jpeg, webp o gif (por defecto png)
                let ext = std::path::Path::new(&file_name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_else(|| "png".to_string());

                if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
                    if let Some(ref path) = saved_image_path {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "Solo se permiten archivos de imagen (PNG, JPG, WEBP, GIF)." })),
                    ).into_response();
                }

                // 3. Tamaño máximo 8 MiB (8 * 1024 * 1024)
                const MAX_IMAGE_SIZE: usize = 8 * 1024 * 1024;
                let mut bytes = Vec::new();
                let mut exceeded = false;

                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            if bytes.len() + chunk.len() > MAX_IMAGE_SIZE {
                                exceeded = true;
                                break;
                            }
                            bytes.extend_from_slice(&chunk);
                        }
                        Ok(None) => break,
                        Err(_) => {
                            exceeded = true;
                            break;
                        }
                    }
                }

                if exceeded {
                    if let Some(ref path) = saved_image_path {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "El archivo supera el máximo de 8 MB." })),
                    )
                        .into_response();
                }

                if !bytes.is_empty() {
                    if let Err(e) = tokio::fs::create_dir_all(&uploads_dir).await {
                        tracing::error!("Error al crear uploads_dir: {e}");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": "Error interno del servidor al procesar el archivo." })),
                        ).into_response();
                    }

                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let rand_suffix = random_alphanumeric(6);
                    let safe_name = format!("chat_{}_{}.{}", now_ms, rand_suffix, ext);
                    let target = uploads_dir.join(&safe_name);

                    if let Err(e) = tokio::fs::write(&target, &bytes).await {
                        tracing::error!("Error al escribir imagen en disco: {e}");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": "Error interno del servidor al guardar el archivo." })),
                        ).into_response();
                    }

                    if let Some(ref prev_path) = saved_image_path {
                        let _ = tokio::fs::remove_file(prev_path).await;
                    }

                    saved_image_path = Some(target);
                    saved_image_filename = Some(safe_name);
                }
            }
        }
    } else {
        let bytes = match axum::body::to_bytes(body, 1024 * 64).await {
            Ok(b) => b,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "Cuerpo de solicitud inválido." })),
                )
                    .into_response();
            }
        };

        if let Ok(payload) = serde_json::from_slice::<SendChatPayload>(&bytes) {
            message_text = payload.message.or(payload.content).unwrap_or_default();
            body_guild_id = payload.guild_id;
        }
    }

    let text = message_text.trim();
    let has_image = saved_image_filename.is_some();

    // Validación de texto o imagen
    if text.is_empty() && !has_image {
        if let Some(ref path) = saved_image_path {
            let _ = tokio::fs::remove_file(path).await;
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "El mensaje debe contener texto o una imagen." })),
        )
            .into_response();
    }

    // Validación de longitud máxima (conteo de caracteres, no bytes)
    if text.chars().count() > 400 {
        if let Some(ref path) = saved_image_path {
            let _ = tokio::fs::remove_file(path).await;
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "El mensaje no puede superar los 400 caracteres." })),
        )
            .into_response();
    }

    // Límite de frecuencia: máximo 1 mensaje por segundo por usuario
    if !manager
        .check_and_update_chat_rate_limit(&user.user_id)
        .await
    {
        if let Some(ref path) = saved_image_path {
            let _ = tokio::fs::remove_file(path).await;
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Por favor espera un segundo antes de enviar otro mensaje." })),
        )
            .into_response();
    }

    // Resolución del servidor/guild
    let effective_gid = body_guild_id.or(query_guild);
    let guild_opt = match resolve_guild_for_user(&user, effective_gid.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => {
            if let Some(ref path) = saved_image_path {
                let _ = tokio::fs::remove_file(path).await;
            }
            return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response();
        }
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());

    // Identidad asegurada desde el usuario autenticado
    let did = user.user_id.clone();
    let author_name = if let Some(nick) = user.nickname.as_deref().filter(|s| !s.trim().is_empty())
    {
        nick.to_string()
    } else if let Some(fnick) = user
        .faceit_nickname
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        fnick.to_string()
    } else if !user.username.trim().is_empty() {
        user.username.clone()
    } else {
        "Jugador".to_string()
    };

    let avatar_url = resolve_discord_avatar_url(Some(&did), user.avatar.as_deref());
    let now = chrono::Utc::now().timestamp_millis();
    let id = format!("web_{}_{}", now, random_alphanumeric(5));
    let image_url = saved_image_filename.map(|f| format!("/uploads/chat/{f}"));
    let is_admin = user.is_admin_like();

    let target_channel_id = std::env::var("DISCORD_MIXTON_CHAT_CHANNEL_ID")
        .or_else(|_| std::env::var("DISCORD_MIXTON_CHANNEL_ID"))
        .unwrap_or_else(|_| "1563654582452756571".to_string());

    forward_chat_to_discord(
        target_channel_id,
        author_name.clone(),
        avatar_url.clone(),
        text.to_string(),
        saved_image_path.clone(),
    );

    let msg = ChatMessage {
        id,
        discord_id: did,
        author: author_name,
        avatar: Some(avatar_url),
        is_admin,
        source: "web".to_string(),
        content: text.to_string(),
        image_url,
        timestamp: now,
    };

    manager.add_chat_message(guild_id, msg.clone()).await;
    Json(json!({ "success": true, "message": msg })).into_response()
}

#[derive(Deserialize)]
struct IncomingChatPayload {
    #[serde(rename = "guildId")]
    guild_id: Option<String>,
    message: ChatMessage,
}

async fn chat_incoming(
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<IncomingChatPayload>,
) -> impl IntoResponse {
    let guild_id = manager.resolve_guild_id(payload.guild_id.as_deref());
    manager.add_chat_message(guild_id, payload.message).await;
    Json(serde_json::json!({ "success": true })).into_response()
}

fn forward_chat_to_discord(
    channel_id: String,
    author_name: String,
    avatar_url: String,
    content: String,
    image_path: Option<std::path::PathBuf>,
) {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .user_agent("DiscordBot (https://machupeek.com, 1.0)")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        // La URL del webhook es un secreto: solo desde el entorno, nunca en el código.
        let webhook_url = std::env::var("DISCORD_MIXTON_WEBHOOK_URL")
            .ok()
            .filter(|url| !url.trim().is_empty());

        let target_webhook = if let Some(wh) = webhook_url {
            Some(wh)
        } else if let Ok(token) = std::env::var("DISCORD_TOKEN") {
            let url = format!("https://discord.com/api/v10/channels/{channel_id}/webhooks");
            let resp = client.get(&url)
                .header("Authorization", format!("Bot {token}"))
                .send()
                .await;
            if let Ok(r) = resp {
                if let Ok(webhooks) = r.json::<Vec<serde_json::Value>>().await {
                    webhooks.into_iter().find_map(|w| {
                        let id = w.get("id")?.as_str()?;
                        let tok = w.get("token")?.as_str()?;
                        Some(format!("https://discord.com/api/webhooks/{id}/{tok}"))
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(wh_url) = target_webhook {
            let payload_json = serde_json::json!({
                "content": content,
                "username": author_name,
                "avatar_url": avatar_url
            });

            let mut has_image = false;
            let mut form = reqwest::multipart::Form::new();
            if let Some(ref path) = image_path {
                if path.exists() {
                    if let Ok(bytes) = tokio::fs::read(path).await {
                        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image.png").to_string();
                        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
                        form = form.part("files[0]", part);
                        form = form.text("payload_json", payload_json.to_string());
                        has_image = true;
                    }
                }
            }

            let res = if has_image {
                client.post(&wh_url).multipart(form).send().await
            } else {
                client.post(&wh_url).json(&payload_json).send().await
            };

            if let Ok(r) = res {
                if r.status().is_success() {
                    tracing::info!("Mensaje de chat reenviado a webhook Discord con éxito");
                    return;
                } else {
                    tracing::warn!("Webhook Discord respondió con error: status={}", r.status());
                }
            }
        }

        // Fallback al bot directo en el canal
        if let Ok(token) = std::env::var("DISCORD_TOKEN") {
            let msg_url = format!("https://discord.com/api/v10/channels/{channel_id}/messages");
            let body = serde_json::json!({
                "content": format!("💬 **{}**: {}", author_name, content)
            });
            let _ = client.post(&msg_url)
                .header("Authorization", format!("Bot {token}"))
                .json(&body)
                .send()
                .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_usuario_solo_se_une_a_si_mismo_sin_steam_ni_elo_del_cuerpo() {
        let user = RequireUser(crate::auth::LobbyUser {
            user_id: "111".into(),
            username: "yo".into(),
            avatar: None,
            role: None,
            servidor: None,
            is_admin: false,
            nickname: None,
            faceit_nickname: None,
        });
        let requested = Player {
            discord_id: "222".into(),
            nickname: "otro".into(),
            steam_id_64: Some("76561198000000001".into()),
            faceit_elo: 3000,
            tier: "A".into(),
            mixton_tier_override: Some("A".into()),
            ready: true,
            ..Default::default()
        };
        let p = own_player(&user, requested);
        assert_eq!(p.discord_id, "111");
        assert_eq!(p.nickname, "yo");
        assert_eq!(p.steam_id_64, None);
        assert_eq!(p.faceit_elo, 1000);
        assert_eq!(p.tier, "C");
        assert_eq!(p.mixton_tier_override, None);
        assert!(p.ready);
    }

    #[test]
    fn test_random_alphanumeric_length_and_chars() {
        let s = random_alphanumeric(6);
        assert_eq!(s.len(), 6);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));

        let s5 = random_alphanumeric(5);
        assert_eq!(s5.len(), 5);
        assert!(s5.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_resolve_discord_avatar_url_all_branches() {
        // 1. HTTP / HTTPS
        assert_eq!(
            resolve_discord_avatar_url(Some("1"), Some("https://example.com/a.png")),
            "https://example.com/a.png"
        );
        assert_eq!(
            resolve_discord_avatar_url(Some("1"), Some("http://example.com/a.png")),
            "http://example.com/a.png"
        );

        // 2. Hex/underscore con user_id
        assert_eq!(
            resolve_discord_avatar_url(Some("123"), Some("a_feedface_01")),
            "https://cdn.discordapp.com/avatars/123/a_feedface_01.png?size=128"
        );

        // 3. user_id numérico sin avatar
        // (803378680482594817 >> 22) % 6 = 1
        assert_eq!(
            resolve_discord_avatar_url(Some("803378680482594817"), None),
            "https://cdn.discordapp.com/embed/avatars/1.png"
        );

        // 4. Fallback default 0
        assert_eq!(
            resolve_discord_avatar_url(None, None),
            "https://cdn.discordapp.com/embed/avatars/0.png"
        );
        assert_eq!(
            resolve_discord_avatar_url(Some("no_es_numero"), None),
            "https://cdn.discordapp.com/embed/avatars/0.png"
        );
    }
}


async fn override_tier(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<OverrideTierPayload>,
) -> impl IntoResponse {
    let p_id = match payload.player_discord_id.filter(|s| !s.trim().is_empty()) {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Falta el ID del jugador." })),
            )
                .into_response()
        }
    };

    if let Some(ref t) = payload.tier {
        if !matches!(t.to_uppercase().as_str(), "S" | "A" | "B" | "C") {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Tier inválido. Debe ser S, A, B, C o null." })),
            )
                .into_response();
        }
    }

    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let norm_tier = payload.tier.as_ref().map(|t| t.to_uppercase());

    let mut found = false;
    for p in &mut state.available_players {
        if p.discord_id == p_id {
            p.mixton_tier_override = norm_tier.clone();
            if let Some(ref t) = norm_tier {
                p.tier = t.clone();
            }
            found = true;
            break;
        }
    }
    if !found {
        for t in &mut state.teams {
            if let Some(ref mut c) = t.captain {
                if c.discord_id == p_id {
                    c.mixton_tier_override = norm_tier.clone();
                    if let Some(ref nt) = norm_tier {
                        c.tier = nt.clone();
                    }
                    break;
                }
            }
            for p in &mut t.players {
                if p.discord_id == p_id {
                    p.mixton_tier_override = norm_tier.clone();
                    if let Some(ref nt) = norm_tier {
                        p.tier = nt.clone();
                    }
                    break;
                }
            }
        }
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({
        "success": true,
        "tier": norm_tier,
        "state": updated,
        "availablePlayers": available_players,
    }))
    .into_response()
}

async fn toggle_command(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<ToggleCommandPayload>,
) -> impl IntoResponse {
    let enabled = match payload.enabled {
        Some(e) => e,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "El valor de \"enabled\" debe ser un booleano." })),
            )
                .into_response()
        }
    };

    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;
    state.command_enabled = enabled;
    let updated = manager.update_state(guild_id, state).await;
    Json(json!({ "success": true, "commandEnabled": enabled, "state": updated })).into_response()
}

async fn load_team(
    admin: RequireAdmin,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
    Json(payload): Json<LoadTeamPayload>,
) -> impl IntoResponse {
    let (team_id, cap_id, player_ids) = match (
        payload.team_id.filter(|s| !s.trim().is_empty()),
        payload.captain_discord_id.filter(|s| !s.trim().is_empty()),
        payload.player_discord_ids,
    ) {
        (Some(tid), Some(cid), Some(pids)) => (tid, cid, pids),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Faltan parámetros (teamId, captainDiscordId, playerDiscordIds)." })),
            )
                .into_response()
        }
    };

    let g_opt = payload
        .guild_id
        .or_else(|| extract_guild_id(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&admin, g_opt.as_deref(), true) {
        Ok(g) => g,
        Err(msg) => return (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    };
    let guild_id = manager.resolve_guild_id(guild_opt.as_deref());
    let mut state = manager.get_state(guild_id).await;

    let team_pos = match state.teams.iter().position(|t| t.id == team_id) {
        Some(pos) => pos,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Equipo no encontrado." })),
            )
                .into_response()
        }
    };

    let all_roster_ids: std::collections::HashSet<String> =
        std::iter::once(cap_id.clone())
            .chain(player_ids.iter().cloned())
            .collect();

    state
        .available_players
        .retain(|p| !all_roster_ids.contains(&p.discord_id));

    for other_team in &mut state.teams {
        if let Some(ref c) = other_team.captain {
            if all_roster_ids.contains(&c.discord_id) {
                other_team.captain = None;
            }
        }
        other_team
            .players
            .retain(|p| !all_roster_ids.contains(&p.discord_id));
    }

    let mut cap_player = Player {
        discord_id: cap_id.clone(),
        nickname: "Capitán".to_string(),
        faceit_nickname: Some("Capitán".to_string()),
        faceit_elo: 1000,
        ready: true,
        tier: "A".to_string(),
        ..Default::default()
    };
    cap_player.ensure_discord_user();
    state.teams[team_pos].captain = Some(cap_player);

    state.teams[team_pos].players.clear();
    for pid in player_ids.iter().take(4) {
        let mut p = Player {
            discord_id: pid.clone(),
            nickname: "Jugador".to_string(),
            faceit_nickname: Some("Jugador".to_string()),
            faceit_elo: 1000,
            ready: true,
            tier: "C".to_string(),
            ..Default::default()
        };
        p.ensure_discord_user();
        state.teams[team_pos].players.push(p);
    }

    let all_done = state.teams.len() >= 2
        && state
            .teams
            .iter()
            .all(|t| t.captain.is_some() && t.players.len() == 4);
    if all_done {
        state.draft_finished = true;
        state.draft_started = false;
        state.auto_start_veto_if_ready();
    }

    state.sanitize();
    let updated = manager.update_state(guild_id, state).await;
    let available_players = partition_available_players(&updated);
    Json(json!({ "success": true, "state": updated, "availablePlayers": available_players }))
        .into_response()
}
