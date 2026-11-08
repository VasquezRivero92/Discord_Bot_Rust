//! `POST /api/lobby/mixton-create-match`: lanza la partida del lobby en el servidor CS2 de AWS.
//!
//! Antes la atendía el legado Node (`routes/lobby.js` 4730 + `launchMatchInternal`). El orden
//! protege el dinero:
//!
//! 1. Se valida todo sin efectos (equipos completos, permisos, apuesta en soles enteros,
//!    SteamID64 de cada jugador).
//! 2. Se **cobra la apuesta** a todos, todo-o-nada (`collect_match_stakes`). El legado
//!    revisaba saldos y luego descontaba uno por uno: dos lanzamientos simultáneos podían
//!    cobrar de más o dejar saldos negativos.
//! 3. Se registra la partida activa, se configura MatchZy y se enciende el servidor.
//!    Si algo de esto falla, la partida se cancela y **se devuelve la apuesta**.
//!
//! Diferencias con el legado: solo el modo STEAM (servidor CS2 propio) y sin torneos; no se
//! crean canales de voz ni se publica el anuncio con embed. El `connect IP` llega a cada
//! jugador por mensaje privado de Discord (cola `user_notifications`).

use axum::extract::{FromRef, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use cs2::{Cs2ServerManager, MatchParams};
use data::{FirestoreClient, StakeOutcome};
use lobby::auth::{resolve_guild_for_user, RequireUser, SessionVerifier};
use lobby::{LobbyManager, LobbyState, Player};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use super::json_response;
use crate::match_discord::{
    parse_id_list, MatchAnnouncement, MatchChannelsConfig, MatchDiscord, RosterEntry,
};

/// Espectadores fijos de MatchZy (administradores), como en el legado.
const SPECTATORS: [&str; 2] = ["76561198675781576", "76561198042296985"];

#[derive(Clone)]
pub struct MatchLaunchState {
    pub lobby: Arc<LobbyManager>,
    pub cs2: Arc<Cs2ServerManager>,
    pub firestore: Arc<FirestoreClient>,
    pub auth: Arc<dyn SessionVerifier>,
    /// Servidores (guild) con un lanzamiento en curso: dos clics no lanzan dos partidas.
    pub launching: Arc<Mutex<HashSet<String>>>,
    /// Canales de voz y anuncio. `None` (sin token de bot) = la partida se lanza sin Discord.
    pub discord: Option<Arc<MatchDiscord>>,
    /// Canales por defecto del entorno (`DISCORD_MATCHES_CHANNEL_ID`...), si el servidor no
    /// tiene los suyos configurados.
    pub channels_env: MatchChannelsConfig,
}

/// Canales del servidor: los de `guild_settings`, si no los globales (`faceit_config/settings`)
/// y si no los del entorno. Listas separadas por comas, como en el legado.
async fn channels_config(st: &MatchLaunchState, guild_id: &str) -> MatchChannelsConfig {
    let guild = st
        .firestore
        .get_doc::<Value>("guild_settings", guild_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let global = st
        .firestore
        .get_doc::<Value>("faceit_config", "settings")
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let ids = |key: &str, env: &[String]| -> Vec<String> {
        for src in [&guild, &global] {
            let found = parse_id_list(src[key].as_str());
            if !found.is_empty() {
                return found;
            }
        }
        env.to_vec()
    };
    MatchChannelsConfig {
        matches_channel_ids: ids("discordMatchesChannelId", &st.channels_env.matches_channel_ids),
        waiting_room_ids: ids("discordWaitingRoomChannelId", &st.channels_env.waiting_room_ids),
        access_role_id: ids(
            "discordMatchAccessRoleId",
            st.channels_env.access_role_id.as_slice(),
        )
        .into_iter()
        .next(),
    }
}

impl FromRef<MatchLaunchState> for Arc<dyn SessionVerifier> {
    fn from_ref(state: &MatchLaunchState) -> Self {
        state.auth.clone()
    }
}

pub fn router(state: MatchLaunchState) -> Router {
    Router::new()
        .route("/api/lobby/mixton-create-match", post(create_match))
        .route("/api/lobby/create-match", post(create_match_legacy))
        .with_state(state)
}

#[derive(Deserialize, Default)]
struct CreateMatchPayload {
    #[serde(rename = "teamIdA", default)]
    team_id_a: String,
    #[serde(rename = "teamIdB", default)]
    team_id_b: String,
    #[serde(rename = "betAmount", default)]
    bet_amount: Value,
    #[serde(rename = "manualPaidDiscordIds", default)]
    manual_paid: Vec<String>,
    #[serde(rename = "matchMode", default)]
    match_mode: Option<String>,
    #[serde(rename = "guildId", default)]
    guild_id: Option<String>,
}

/// Libera el servidor al terminar el lanzamiento, también si el handler retorna antes.
struct LaunchGuard {
    set: Arc<Mutex<HashSet<String>>>,
    guild: String,
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.guild);
    }
}

fn error(status: StatusCode, msg: impl Into<String>) -> Response {
    json_response(status, &json!({ "error": msg.into() }))
}

fn guild_from_query(raw: Option<&str>) -> Option<String> {
    url::form_urlencoded::parse(raw?.as_bytes()).find_map(|(k, v)| {
        (matches!(k.as_ref(), "guildId" | "guild_id" | "guild") && !v.trim().is_empty())
            .then(|| v.trim().to_string())
    })
}

/// Monto como número o texto. `None` si no es un número finito.
fn as_amount(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if s.trim().is_empty() => 0.0,
        Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        Value::Null => 0.0,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// Apuesta en céntimos (soles enteros, ≥ 0). Un capitán no elige el monto: se usa el acordado.
fn bet_cents(state: &LobbyState, is_admin: bool, requested: &Value) -> Result<i64, String> {
    let soles = if is_admin {
        as_amount(requested).ok_or("Ingresa un monto de apuesta válido.")?
    } else if state.bet_agreed {
        state.agreed_bet_amount.unwrap_or(state.effective_bet_amount)
    } else {
        state.effective_bet_amount
    };
    if !soles.is_finite() || soles < 0.0 {
        return Err("Ingresa un monto de apuesta válido.".into());
    }
    if soles.fract() != 0.0 {
        return Err("La apuesta debe ser un monto en soles enteros (sin céntimos).".into());
    }
    Ok((soles * 100.0).round() as i64)
}

fn display_name(p: &Player) -> String {
    p.faceit_nickname
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| p.nickname.clone())
}

fn is_steam_id(s: &str) -> bool {
    s.len() == 17 && s.chars().all(|c| c.is_ascii_digit())
}

fn team_tag(name: &str, fallback: &str) -> String {
    let tag: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(4)
        .collect::<String>()
        .to_uppercase();
    if tag.is_empty() {
        fallback.into()
    } else {
        tag
    }
}

/// Id de mapa de CS2 de un valor del veto (`{ id, cs2MapId }` o texto).
fn map_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => v
            .get("cs2MapId")
            .or_else(|| v.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
    .filter(|s| !s.trim().is_empty())
}

fn chosen_maps(state: &LobbyState) -> Vec<String> {
    let Some(veto) = &state.map_veto else {
        return vec!["de_mirage".into()];
    };
    let mut maps: Vec<String> = veto.chosen_maps.iter().filter_map(map_id).collect();
    if maps.is_empty() {
        maps.extend(veto.chosen_map.as_ref().and_then(map_id));
    }
    if maps.is_empty() {
        maps.extend(veto.final_chosen_map.clone().filter(|m| !m.trim().is_empty()));
    }
    if maps.is_empty() {
        maps.push("de_mirage".into());
    }
    maps
}

/// SteamID64 del jugador: el del lobby o, si falta, el guardado en su cuenta.
async fn steam_id_of(fs: &FirestoreClient, p: &Player) -> Option<String> {
    if let Some(s) = p.steam_id_64.as_deref().map(str::trim).filter(|s| is_steam_id(s)) {
        return Some(s.to_string());
    }
    let doc = fs
        .get_doc::<Value>("faceit_users", &p.discord_id)
        .await
        .ok()
        .flatten()?;
    doc.get("steam_id_64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| is_steam_id(s))
        .map(str::to_string)
}

/// Deja el lobby listo para otra partida (como `launchMatchInternal` fuera de torneo).
fn reset_after_launch(state: &mut LobbyState, team_a: &str, team_b: &str) {
    for t in state.teams.iter_mut().filter(|t| t.id == team_a || t.id == team_b) {
        t.captain = None;
        t.players.clear();
    }
    state.pick_order = state
        .teams
        .iter()
        .filter(|t| t.captain.is_some())
        .map(|t| t.id.clone())
        .collect();
    state.pick_sequence = Default::default();
    state.current_pick_index = Default::default();
    state.draft_started = false;
    state.draft_finished = false;
    state.map_veto = None;
    state.captain_a_proposal = None;
    state.captain_b_proposal = None;
    state.captain_a_ready = false;
    state.captain_b_ready = false;
    state.bet_agreed = false;
    state.agreed_bet_amount = None;
    state.effective_bet_amount = 0.0;
    state.auto_countdown_ends_at = Default::default();
    state.roulette_winner_team_id = Default::default();
    state.roulette_winner_name = Default::default();
    state.roulette_spinning = false;
}

/// Corre el lanzamiento en su propia tarea: si el navegador corta la conexión, axum suelta el
/// handler, y un lanzamiento cortado a la mitad dejaría la apuesta cobrada sin partida ni
/// reembolso. La tarea siempre llega al final (partida creada o apuesta devuelta).
async fn create_match(
    user: RequireUser,
    raw: RawQuery,
    st: State<MatchLaunchState>,
    payload: Json<CreateMatchPayload>,
) -> Response {
    match tokio::spawn(launch_match(user, raw, st, payload)).await {
        Ok(res) => res,
        Err(e) => {
            tracing::error!("[Lanzamiento] la tarea de lanzamiento falló: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Error interno al lanzar la partida.")
        }
    }
}

async fn launch_match(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(st): State<MatchLaunchState>,
    Json(payload): Json<CreateMatchPayload>,
) -> Response {
    let user = user.0;
    let mode = payload.match_mode.as_deref().unwrap_or("steam");
    if mode != "steam" {
        return error(
            StatusCode::BAD_REQUEST,
            "Por ahora solo se pueden lanzar partidas en el servidor CS2 propio (modo STEAM).",
        );
    }
    if payload.team_id_a.is_empty() || payload.team_id_b.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Debes seleccionar dos equipos.");
    }
    let requested = payload.guild_id.clone().or_else(|| guild_from_query(raw.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user, requested.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return error(StatusCode::FORBIDDEN, msg),
    };
    let guild_id = st.lobby.resolve_guild_id(guild_opt.as_deref()).to_string();
    let mut state = st.lobby.get_state(&guild_id).await;

    if state
        .tournament
        .as_ref()
        .and_then(|t| t.get("active"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "El lanzamiento de partidas de torneo todavía no está disponible en esta versión.",
        );
    }
    let find = |id: &str| state.teams.iter().find(|t| t.id == id).cloned();
    let (Some(team_a), Some(team_b)) = (find(&payload.team_id_a), find(&payload.team_id_b)) else {
        return error(StatusCode::NOT_FOUND, "Equipos no encontrados.");
    };
    if team_a.id == team_b.id {
        return error(StatusCode::BAD_REQUEST, "Debes seleccionar dos equipos distintos.");
    }

    let is_admin = user.is_admin
        || matches!(user.role.as_deref(), Some("admin") | Some("global_admin"));
    let is_captain = [&team_a, &team_b]
        .iter()
        .any(|t| t.captain.as_ref().is_some_and(|c| c.discord_id == user.user_id));
    if !is_admin && !is_captain {
        return error(
            StatusCode::FORBIDDEN,
            "Solo los capitanes del encuentro o un administrador pueden lanzar la partida.",
        );
    }
    let (Some(cap_a), Some(cap_b)) = (team_a.captain.clone(), team_b.captain.clone()) else {
        return error(StatusCode::BAD_REQUEST, "Ambos equipos deben estar completos (5 jugadores c/u).");
    };
    if team_a.players.len() != 4 || team_b.players.len() != 4 {
        return error(StatusCode::BAD_REQUEST, "Ambos equipos deben estar completos (5 jugadores c/u).");
    }

    let bet = match bet_cents(&state, is_admin, &payload.bet_amount) {
        Ok(b) => b,
        Err(msg) => return error(StatusCode::BAD_REQUEST, msg),
    };
    let roster_a: Vec<Player> = std::iter::once(cap_a.clone()).chain(team_a.players.clone()).collect();
    let roster_b: Vec<Player> = std::iter::once(cap_b.clone()).chain(team_b.players.clone()).collect();
    let all: Vec<&Player> = roster_a.iter().chain(&roster_b).collect();
    let ids: Vec<String> = all.iter().map(|p| p.discord_id.clone()).collect();
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return error(StatusCode::BAD_REQUEST, "Un jugador no puede estar en los dos equipos.");
    }
    // Solo un administrador puede marcar pagos hechos fuera de la billetera.
    let manual: Vec<String> = if is_admin {
        payload
            .manual_paid
            .iter()
            .filter(|id| ids.contains(id))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let mut steam: Vec<String> = Vec::with_capacity(all.len());
    let mut missing = Vec::new();
    for p in &all {
        match steam_id_of(&st.firestore, p).await {
            Some(s) => steam.push(s),
            None => {
                missing.push(display_name(p));
                steam.push(String::new());
            }
        }
    }
    if !missing.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "No se puede lanzar la partida. Todos los jugadores deben tener su SteamID vinculado para ingresar al servidor CS2. Falta vincular el Steam ID de: {}.",
                missing.join(", ")
            ),
        );
    }

    // Un lanzamiento a la vez por servidor.
    {
        let mut set = st.launching.lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(guild_id.clone()) {
            return error(StatusCode::CONFLICT, "Ya se está lanzando una partida en este servidor.");
        }
    }
    let _guard = LaunchGuard {
        set: st.launching.clone(),
        guild: guild_id.clone(),
    };

    let now = chrono::Utc::now();
    let match_id = format!(
        "mixton_{}_{:06x}",
        now.timestamp_millis(),
        now.timestamp_subsec_nanos() & 0xff_ffff
    );

    // 2. Cobro de la apuesta, todo-o-nada, ANTES de crear nada en el servidor.
    if bet > 0 {
        match st
            .firestore
            .collect_match_stakes(&match_id, bet, &ids, &manual)
            .await
        {
            Ok(StakeOutcome::Collected { .. }) => {}
            Ok(StakeOutcome::AlreadyCollected) => {
                return error(StatusCode::CONFLICT, "Esta partida ya fue cobrada.");
            }
            Ok(StakeOutcome::Insufficient(short)) => {
                let unpaid: Vec<Value> = short
                    .iter()
                    .map(|(id, bal)| {
                        let nick = all
                            .iter()
                            .find(|p| &p.discord_id == id)
                            .map(|p| display_name(p))
                            .unwrap_or_else(|| "Jugador".into());
                        json!({
                            "discordId": id,
                            "nickname": nick,
                            "balance": *bal as f64 / 100.0,
                            "deficit": (bet - bal) as f64 / 100.0,
                        })
                    })
                    .collect();
                let names: Vec<String> = unpaid
                    .iter()
                    .map(|u| {
                        format!(
                            "{} (Saldo: S/ {:.2}, Falta: S/ {:.2})",
                            u["nickname"].as_str().unwrap_or("Jugador"),
                            u["balance"].as_f64().unwrap_or(0.0),
                            u["deficit"].as_f64().unwrap_or(0.0)
                        )
                    })
                    .collect();
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &json!({
                        "error": format!(
                            "No se puede lanzar el encuentro: Faltan {} jugador(es) por completar su cuota: {}",
                            unpaid.len(),
                            names.join(", ")
                        ),
                        "unpaidPlayers": unpaid,
                    }),
                );
            }
            Err(e) => {
                tracing::error!("[Lanzamiento] {match_id}: cobro de apuestas: {e}");
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "No se pudo cobrar la apuesta. No se descontó nada; intenta nuevamente.",
                );
            }
        }
    }

    // 3. Registrar la partida y encender el servidor. Cualquier fallo devuelve la apuesta.
    let name_a = format!("Equipo {}", display_name(&cap_a));
    let name_b = format!("Equipo {}", display_name(&cap_b));
    let maps = chosen_maps(&state);
    let games = match state.games_per_match {
        3 | 5 => state.games_per_match,
        _ => 1,
    };
    let port = st.cs2.allocate_port(&match_id).await.unwrap_or(27015);
    let server_ip = st.cs2.get_server_ip_for_port(port);
    let total_players = all.len();
    let bet_soles = bet as f64 / 100.0;
    let db_roster = |roster: &[Player], steam: &[String]| -> Vec<Value> {
        roster
            .iter()
            .zip(steam)
            .map(|(p, s)| {
                json!({
                    "nickname": display_name(p),
                    "discord_id": p.discord_id,
                    "steam_id_64": s,
                })
            })
            .collect()
    };
    let (steam_a, steam_b) = steam.split_at(roster_a.len());
    let match_doc = json!({
        "matchId": match_id,
        "status": "ONGOING",
        "lifecycle_state": "live",
        "game": "cs2",
        "launch_mode": "steam",
        "guild_id": guild_id,
        "creator_id": user.user_id,
        "faction1_name": name_a,
        "faction2_name": name_b,
        "faction1_roster": db_roster(&roster_a, steam_a),
        "faction2_roster": db_roster(&roster_b, steam_b),
        "bet_amount_per_player": bet_soles,
        "betAmount": bet_soles,
        "total_pot": bet_soles * total_players as f64,
        "manual_paid_players": manual,
        "chosen_maps": maps,
        "games_per_match": games,
        "server_ip": server_ip,
        "assigned_port": port,
        "createdAt": data::timestamp_now(),
    });

    let fail = |why: String| {
        let fs = st.firestore.clone();
        let cs2 = st.cs2.clone();
        let match_id = match_id.clone();
        async move {
            tracing::error!("[Lanzamiento] {match_id}: {why}; se cancela y se devuelve la apuesta");
            if let Err(e) = fs.cancel_match(&match_id, Some(&why)).await {
                // Sin la cancelación, el reembolso directo (idempotente) sigue siendo seguro.
                tracing::error!("[Lanzamiento] {match_id}: cancelar: {e}");
                let _ = fs.refund_match_stakes(&match_id, &why).await;
            }
            cs2.clear_match(&match_id).await;
            error(
                StatusCode::BAD_GATEWAY,
                "No se pudo iniciar la partida en el servidor. Se devolvió la apuesta a todos los jugadores.",
            )
        }
    };

    if let Err(e) = st
        .firestore
        .set_doc("faceit_active_matches", &match_id, &match_doc)
        .await
    {
        return fail(format!("no se pudo registrar la partida: {e}")).await;
    }
    st.cs2
        .publish_match(
            match_id.clone(),
            Some(port),
            maps[0].clone(),
            bet_soles,
            total_players,
            None,
        )
        .await;
    let pairs = |roster: &[Player], steam: &[String]| -> Vec<(String, String)> {
        steam
            .iter()
            .cloned()
            .zip(roster.iter().map(display_name))
            .collect()
    };
    let spectators: Vec<(String, String)> = SPECTATORS
        .iter()
        .filter(|s| !steam.iter().any(|x| x == *s))
        .map(|s| (s.to_string(), "Admin".to_string()))
        .collect();
    let params = MatchParams {
        match_id: match_id.clone(),
        chosen_maps: maps.clone(),
        team1_name: name_a.clone(),
        team1_tag: team_tag(&display_name(&cap_a), "EQ1"),
        team1_players: pairs(&roster_a, steam_a),
        team2_name: name_b.clone(),
        team2_tag: team_tag(&display_name(&cap_b), "EQ2"),
        team2_players: pairs(&roster_b, steam_b),
        num_maps: games,
        spectators,
    };
    if let Err(e) = st.cs2.configure_match(params).await {
        return fail(format!("no se pudo configurar MatchZy: {e}")).await;
    }
    if let Err(e) = st.cs2.start_server(port).await {
        return fail(format!("no se pudo encender el servidor CS2: {e}")).await;
    }

    // 4. Discord (canales de voz, mover jugadores, anuncio). Un fallo aquí no cancela la
    //    partida: ya está cobrada y el servidor encendido; solo falta lo cosmético.
    let mut voice_of: std::collections::HashMap<String, String> = Default::default();
    let mut announcement_url = None;
    if let Some(discord) = &st.discord {
        let entry = |p: &Player| RosterEntry {
            discord_id: p.discord_id.clone(),
            nickname: display_name(p),
            elo: p.faceit_elo,
            level: p.faceit_level,
        };
        let announcement = MatchAnnouncement {
            match_id: match_id.clone(),
            guild_id: guild_id.clone(),
            team_a: name_a.clone(),
            team_b: name_b.clone(),
            roster_a: roster_a.iter().map(entry).collect(),
            roster_b: roster_b.iter().map(entry).collect(),
            bet_soles,
            server_ip: server_ip.clone(),
            map: maps[0].clone(),
            games,
            initiator: user.username.clone(),
        };
        let cfg = channels_config(&st, &guild_id).await;
        let setup = discord.setup_match(&announcement, &cfg).await;
        if let Err(e) = st
            .firestore
            .merge_doc("faceit_active_matches", &match_id, &setup.to_match_fields())
            .await
        {
            // Sin estos ids no se podrán borrar los canales al terminar: se avisa.
            tracing::error!("[Lanzamiento] {match_id}: no se guardaron los canales de Discord: {e}");
        }
        for (roster, vc) in [(&roster_a, &setup.vc_a), (&roster_b, &setup.vc_b)] {
            if let Some(vc) = vc {
                for p in roster {
                    voice_of.insert(p.discord_id.clone(), vc.clone());
                }
            }
        }
        announcement_url = setup.message_url(&guild_id);
    }

    // 5. Avisos y lobby listo para otra partida.
    for p in &all {
        let mut msg = format!(
            "Tu partida {name_a} vs {name_b} está lista. Abre CS2 y escribe en la consola: connect {server_ip}"
        );
        if let Some(vc) = voice_of.get(&p.discord_id) {
            msg.push_str(&format!("\nTu canal de voz: <#{vc}>"));
        }
        if let Some(url) = &announcement_url {
            msg.push_str(&format!("\nAnuncio: {url}"));
        }
        if let Err(e) = st.firestore.notify_user(&p.discord_id, &msg).await {
            tracing::warn!("[Lanzamiento] {match_id}: aviso a {}: {e}", p.discord_id);
        }
    }
    reset_after_launch(&mut state, &team_a.id, &team_b.id);
    st.lobby.update_state(&guild_id, state).await;

    tracing::info!(
        "[Lanzamiento] {match_id} iniciada por {} en {server_ip} (apuesta S/ {bet_soles:.2} x {total_players})",
        user.username
    );
    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "matchId": match_id,
            "faceitHubUrl": null,
            "assignedQueue": { "id": "steam", "queueId": "steam", "name": "Servidor CS2 Dedicado" },
            "gamesPerMatch": games,
            "roundsPerGame": games,
            "serverIp": server_ip,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use lobby::auth::LobbyUser;
    use lobby::LobbyTeam;
    use tower::ServiceExt;

    const GUILD: &str = "g1";

    /// Token = id de usuario; "admin" es administrador.
    struct FakeVerifier;
    impl SessionVerifier for FakeVerifier {
        fn verify_token(&self, token: &str) -> Option<LobbyUser> {
            Some(LobbyUser {
                user_id: token.to_string(),
                username: token.to_string(),
                avatar: None,
                role: (token == "admin").then(|| "global_admin".to_string()),
                servidor: None,
                is_admin: token == "admin",
                nickname: None,
                faceit_nickname: None,
            })
        }
    }

    fn player(id: &str) -> Player {
        Player {
            discord_id: id.into(),
            nickname: format!("nick_{id}"),
            steam_id_64: Some(format!("7656119800000{:04}", id.len() * 100 + id.bytes().map(u32::from).sum::<u32>() as usize % 100)),
            ..Default::default()
        }
    }

    fn team(id: &str, prefix: &str) -> LobbyTeam {
        LobbyTeam {
            id: id.into(),
            name: id.into(),
            tag: id.into(),
            captain: Some(player(&format!("{prefix}0"))),
            players: (1..=4).map(|i| player(&format!("{prefix}{i}"))).collect(),
        }
    }

    fn all_ids() -> Vec<String> {
        ["a", "b"]
            .iter()
            .flat_map(|p| (0..=4).map(move |i| format!("{p}{i}")))
            .collect()
    }

    async fn setup(bet: f64, fund: f64) -> (Router, Arc<FirestoreClient>, Arc<LobbyManager>) {
        let mut st = LobbyState::default();
        st.teams = vec![team("ta", "a"), team("tb", "b")];
        st.bet_agreed = true;
        st.agreed_bet_amount = Some(bet);
        let lobby = Arc::new(LobbyManager::new(GUILD).with_initial_state(GUILD, st));
        let fs = Arc::new(FirestoreClient::in_memory_mysql());
        for id in all_ids() {
            fs.set_doc("faceit_users", &id, &json!({ "wallet_balance": fund }))
                .await
                .unwrap();
        }
        let mut vars = std::collections::HashMap::new();
        for (k, v) in [
            ("ADMIN_JWT_SECRET", "test_secret_1234567890"),
            ("BASE_URL", "https://machupeek.com"),
            ("DISCORD_CLIENT_ID", "id"),
            ("DISCORD_CLIENT_SECRET", "secret"),
            ("DISCORD_TOKEN", "token"),
            ("DISCORD_GUILD_ID", GUILD),
            ("CS2_SERVER_IP", "127.0.0.1"),
        ] {
            vars.insert(k.to_string(), v.to_string());
        }
        let cfg = common::config::Config::from_map(&vars).unwrap();
        let app = router(MatchLaunchState {
            lobby: lobby.clone(),
            cs2: Arc::new(Cs2ServerManager::new(&cfg).offline()),
            firestore: fs.clone(),
            auth: Arc::new(FakeVerifier),
            launching: Default::default(),
            discord: None,
            channels_env: Default::default(),
        });
        (app, fs, lobby)
    }

    async fn launch(app: Router, as_user: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/lobby/mixton-create-match?guildId={GUILD}"))
            .header("Content-Type", "application/json")
            .header("Cookie", format!("auth_token={as_user}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn balance(fs: &FirestoreClient, id: &str) -> f64 {
        fs.get_doc::<Value>("faceit_users", id)
            .await
            .unwrap()
            .and_then(|u| u["wallet_balance"].as_f64())
            .unwrap_or(0.0)
    }

    #[tokio::test]
    async fn el_capitan_lanza_y_se_cobra_a_los_diez() {
        let (app, fs, lobby) = setup(5.0, 20.0).await;
        let (status, body) = launch(app, "a0", json!({"teamIdA": "ta", "teamIdB": "tb"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let match_id = body["matchId"].as_str().unwrap().to_string();
        for id in all_ids() {
            assert_eq!(balance(&fs, &id).await, 15.0, "{id}");
        }
        let stakes = fs.get_doc::<Value>(data::mysql::STAKES, &match_id).await.unwrap().unwrap();
        assert_eq!(stakes["status"], "collected");
        assert_eq!(stakes["totalCents"], 5_000);
        assert!(fs
            .get_doc::<Value>("faceit_active_matches", &match_id)
            .await
            .unwrap()
            .is_some());
        // El lobby queda libre para otra partida.
        let st = lobby.get_state(GUILD).await;
        assert!(st.teams.iter().all(|t| t.captain.is_none() && t.players.is_empty()));
        assert!(!st.bet_agreed);
    }

    #[tokio::test]
    async fn si_a_uno_le_falta_saldo_no_se_cobra_a_nadie() {
        let (app, fs, _) = setup(5.0, 20.0).await;
        fs.set_doc("faceit_users", "b3", &json!({ "wallet_balance": 2.0 }))
            .await
            .unwrap();
        let (status, body) = launch(app, "a0", json!({"teamIdA": "ta", "teamIdB": "tb"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["unpaidPlayers"][0]["discordId"], "b3");
        assert_eq!(body["unpaidPlayers"][0]["deficit"], 3.0);
        assert_eq!(balance(&fs, "a0").await, 20.0);
        assert_eq!(balance(&fs, "b3").await, 2.0);
    }

    #[tokio::test]
    async fn solo_capitanes_o_admin_y_el_capitan_no_elige_el_monto() {
        let (app, fs, _) = setup(5.0, 20.0).await;
        let (status, _) = launch(app.clone(), "a1", json!({"teamIdA": "ta", "teamIdB": "tb"})).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // El capitán envía otro monto (y pagos "a mano"): se ignoran, se cobra el acordado.
        let (status, _) = launch(
            app,
            "b0",
            json!({"teamIdA": "ta", "teamIdB": "tb", "betAmount": 1, "manualPaidDiscordIds": ["a0"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(balance(&fs, "a0").await, 15.0);
    }

    #[tokio::test]
    async fn sin_steam_id_no_se_lanza_ni_se_cobra() {
        let (_, fs, lobby) = setup(5.0, 20.0).await;
        let mut st = lobby.get_state(GUILD).await;
        st.teams[1].players[0].steam_id_64 = None;
        lobby.update_state(GUILD, st).await;
        // Reconstruir el router sobre el mismo lobby y la misma base.
        let mut vars = std::collections::HashMap::new();
        for (k, v) in [
            ("ADMIN_JWT_SECRET", "test_secret_1234567890"),
            ("BASE_URL", "https://machupeek.com"),
            ("DISCORD_CLIENT_ID", "id"),
            ("DISCORD_CLIENT_SECRET", "secret"),
            ("DISCORD_TOKEN", "token"),
            ("DISCORD_GUILD_ID", GUILD),
            ("CS2_SERVER_IP", "127.0.0.1"),
        ] {
            vars.insert(k.to_string(), v.to_string());
        }
        let cfg = common::config::Config::from_map(&vars).unwrap();
        let app = router(MatchLaunchState {
            lobby,
            cs2: Arc::new(Cs2ServerManager::new(&cfg).offline()),
            firestore: fs.clone(),
            auth: Arc::new(FakeVerifier),
            launching: Default::default(),
            discord: None,
            channels_env: Default::default(),
        });
        let (status, body) = launch(app, "admin", json!({"teamIdA": "ta", "teamIdB": "tb", "betAmount": 5})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("nick_b1"));
        assert_eq!(balance(&fs, "a0").await, 20.0);
    }

    fn state_with_bet(agreed: Option<f64>, effective: f64, bet_agreed: bool) -> LobbyState {
        let mut s = LobbyState::default();
        s.agreed_bet_amount = agreed;
        s.effective_bet_amount = effective;
        s.bet_agreed = bet_agreed;
        s
    }

    #[test]
    fn el_capitan_no_elige_el_monto_usa_el_acordado() {
        let s = state_with_bet(Some(10.0), 0.0, true);
        // Aunque el capitán envíe otro monto, se cobra el acordado.
        assert_eq!(bet_cents(&s, false, &json!(1)), Ok(1000));
        // El admin sí decide el monto.
        assert_eq!(bet_cents(&s, true, &json!(5)), Ok(500));
        assert_eq!(bet_cents(&s, true, &json!("")), Ok(0));
    }

    #[test]
    fn la_apuesta_debe_ser_en_soles_enteros_y_no_negativa() {
        let s = state_with_bet(None, 0.0, false);
        assert!(bet_cents(&s, true, &json!(10.5)).is_err());
        assert!(bet_cents(&s, true, &json!(-1)).is_err());
        assert!(bet_cents(&s, true, &json!("abc")).is_err());
        let s = state_with_bet(Some(7.5), 0.0, true);
        assert!(bet_cents(&s, false, &Value::Null).is_err());
    }

    #[test]
    fn mapas_del_veto_o_mirage() {
        let s = LobbyState::default();
        assert_eq!(chosen_maps(&s), vec!["de_mirage".to_string()]);
        assert_eq!(
            map_id(&json!({"id": "de_nuke", "cs2MapId": "de_nuke"})).as_deref(),
            Some("de_nuke")
        );
    }

    #[test]
    fn etiqueta_del_equipo() {
        assert_eq!(team_tag("Ñandú_Pro 99", "EQ1"), "ANDP");
        assert_eq!(team_tag("!!!", "EQ1"), "EQ1");
    }
}

#[derive(Deserialize, Default)]
struct LegacyCreateMatchPayload {
    #[serde(rename = "teamA", default)]
    team_a: Vec<String>,
    #[serde(rename = "teamB", default)]
    team_b: Vec<String>,
    #[serde(rename = "guildId", default)]
    guild_id: Option<String>,
}

async fn create_match_legacy(
    user: RequireUser,
    raw: RawQuery,
    State(st): State<MatchLaunchState>,
    Json(payload): Json<LegacyCreateMatchPayload>,
) -> Response {
    // Reescribe los equipos del lobby con IDs arbitrarios: si un usuario cualquiera pudiera
    // usarla, se pondría de capitán y lanzaría la partida cobrando la apuesta a otros nueve.
    let is_admin = user.0.is_admin
        || matches!(user.0.role.as_deref(), Some("admin") | Some("global_admin"));
    if !is_admin {
        return error(
            StatusCode::FORBIDDEN,
            "Solo un administrador puede crear partidas con equipos manuales.",
        );
    }
    if payload.team_a.len() != 5 || payload.team_b.len() != 5 {
        return error(
            StatusCode::BAD_REQUEST,
            "Debes proporcionar exactamente dos equipos de 5 jugadores cada uno.",
        );
    }

    let requested = payload.guild_id.clone().or_else(|| guild_from_query(raw.0.as_deref()));
    let guild_opt = match resolve_guild_for_user(&user.0, requested.as_deref(), false) {
        Ok(g) => g,
        Err(msg) => return error(StatusCode::FORBIDDEN, msg),
    };
    let guild_id = st.lobby.resolve_guild_id(guild_opt.as_deref());
    let mut state = st.lobby.get_state(guild_id).await;

    if state.teams.len() < 2 {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Lobby sin suficientes equipos configurados.",
        );
    }

    let cap_a_id = &payload.team_a[0];
    let mut cap_a = Player {
        discord_id: cap_a_id.clone(),
        nickname: "Capitán A".to_string(),
        ready: true,
        tier: "A".to_string(),
        ..Default::default()
    };
    cap_a.ensure_discord_user();
    state.teams[0].captain = Some(cap_a);
    state.teams[0].players = payload.team_a[1..5]
        .iter()
        .map(|id| {
            let mut p = Player {
                discord_id: id.clone(),
                nickname: "Jugador".to_string(),
                ready: true,
                tier: "C".to_string(),
                ..Default::default()
            };
            p.ensure_discord_user();
            p
        })
        .collect();

    let cap_b_id = &payload.team_b[0];
    let mut cap_b = Player {
        discord_id: cap_b_id.clone(),
        nickname: "Capitán B".to_string(),
        ready: true,
        tier: "A".to_string(),
        ..Default::default()
    };
    cap_b.ensure_discord_user();
    state.teams[1].captain = Some(cap_b);
    state.teams[1].players = payload.team_b[1..5]
        .iter()
        .map(|id| {
            let mut p = Player {
                discord_id: id.clone(),
                nickname: "Jugador".to_string(),
                ready: true,
                tier: "C".to_string(),
                ..Default::default()
            };
            p.ensure_discord_user();
            p
        })
        .collect();

    let team_a_id = state.teams[0].id.clone();
    let team_b_id = state.teams[1].id.clone();
    state.sanitize();
    st.lobby.update_state(guild_id, state).await;

    create_match(
        user,
        raw,
        State(st),
        Json(CreateMatchPayload {
            team_id_a: team_a_id,
            team_id_b: team_b_id,
            // El legado no cobraba apuesta en esta ruta.
            bet_amount: json!(0),
            manual_paid: Vec::new(),
            match_mode: Some("steam".to_string()),
            guild_id: payload.guild_id,
        }),
    )
    .await
}
