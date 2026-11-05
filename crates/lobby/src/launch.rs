//! Módulo para validación y preparación del lanzamiento de partidas CS2 en modo STEAM.
//!
//! Contiene funciones puras para validar requisitos de lanzamiento ([`validate_launch`]),
//! y reiniciar el estado del lobby posterior al lanzamiento ([`reset_lobby_after_launch`]).

use crate::auth::LobbyUser;
use crate::state::{LobbyState, Player};
use serde::{Deserialize, Deserializer, Serialize};

fn deserialize_bet_amount<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(f64),
        Str(String),
        Null,
    }
    match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(n) => Ok(n),
        NumOrStr::Str(s) => Ok(s.trim().parse::<f64>().unwrap_or(0.0)),
        NumOrStr::Null => Ok(0.0),
    }
}

fn default_match_mode() -> String {
    "steam".to_string()
}

/// Solicitud de lanzamiento de partida proveniente del frontend o llamada de administración.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    #[serde(default)]
    pub team_id_a: Option<String>,
    #[serde(default)]
    pub team_id_b: Option<String>,
    #[serde(default, deserialize_with = "deserialize_bet_amount")]
    pub bet_amount: f64,
    #[serde(default)]
    pub manual_paid_discord_ids: Vec<String>,
    #[serde(default = "default_match_mode")]
    pub match_mode: String,
    #[serde(default)]
    pub chosen_map: Option<serde_json::Value>,
    #[serde(default)]
    pub queue_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
}

impl Default for LaunchRequest {
    fn default() -> Self {
        Self {
            team_id_a: None,
            team_id_b: None,
            bet_amount: 0.0,
            manual_paid_discord_ids: Vec::new(),
            match_mode: default_match_mode(),
            chosen_map: None,
            queue_id: None,
            guild_id: None,
        }
    }
}

/// Plan de lanzamiento preparado con todas las validaciones aprobadas.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchPlan {
    pub team_a_id: String,
    pub team_b_id: String,
    pub team_a_name: String,
    pub team_a_tag: String,
    pub team_a_roster: Vec<(String, String)>, // (steam_id_64, nick)
    pub team_b_name: String,
    pub team_b_tag: String,
    pub team_b_roster: Vec<(String, String)>, // (steam_id_64, nick)
    pub chosen_map: serde_json::Value,
    pub chosen_maps: Vec<serde_json::Value>,
    pub map_name: String,
    pub map_id: String,
    pub bet_per_player: f64,
    pub manual_paid: Vec<String>,
    pub games_per_match: u32,
    pub full_roster_a: Vec<Player>,
    pub full_roster_b: Vec<Player>,
}

impl LaunchPlan {
    /// Extrae los identificadores de mapas para la configuración de CS2 MatchZy.
    pub fn chosen_map_ids(&self) -> Vec<String> {
        self.chosen_maps
            .iter()
            .map(|m| {
                if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                    id.to_string()
                } else if let Some(id) = m.get("cs2MapId").and_then(|v| v.as_str()) {
                    id.to_string()
                } else if let Some(s) = m.as_str() {
                    s.to_string()
                } else {
                    "de_mirage".to_string()
                }
            })
            .collect()
    }
}

/// Retorna el nombre visible del jugador según la jerarquía del legado:
/// `faceit_nickname` -> `discord_user.display_name` -> `nickname` -> `discord_id`.
pub fn player_display_name(p: &Player) -> String {
    if let Some(ref fnick) = p.faceit_nickname {
        if !fnick.trim().is_empty() {
            return fnick.clone();
        }
    }
    if let Some(ref du) = p.discord_user {
        if !du.display_name.trim().is_empty() {
            return du.display_name.clone();
        }
    }
    if !p.nickname.trim().is_empty() {
        return p.nickname.clone();
    }
    p.discord_id.clone()
}

/// Genera el tag del equipo a partir del nombre del capitán:
/// caracteres alfanuméricos en mayúsculas (máximo 4 letras), o un fallback ("EQ1"/"EQ2").
pub fn derive_team_tag(name: &str, fallback: &str) -> String {
    let alphanumeric: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(4)
        .collect();
    let upper = alphanumeric.to_ascii_uppercase();
    if upper.is_empty() {
        fallback.to_string()
    } else {
        upper
    }
}

/// Valida si un SteamID64 es válido (exactamente 17 dígitos numéricos).
fn is_valid_steam_id_64(s: Option<&str>) -> bool {
    match s {
        Some(val) => {
            let trimmed = val.trim();
            trimmed.len() == 17 && trimmed.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// Valida de forma pura los requisitos de lanzamiento de partida en modo STEAM.
/// Retorna `Ok(LaunchPlan)` si pasa todas las reglas, o `Err((status_code, mensaje))` en caso contrario.
pub fn validate_launch(
    state: &LobbyState,
    req: &LaunchRequest,
    user: &LobbyUser,
) -> Result<LaunchPlan, (u16, String)> {
    // 1. Falta teamIdA o teamIdB -> 400 "Debes seleccionar dos equipos."
    let team_a_id = match req
        .team_id_a
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(id) => id,
        None => return Err((400, "Debes seleccionar dos equipos.".to_string())),
    };
    let team_b_id = match req
        .team_id_b
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(id) => id,
        None => return Err((400, "Debes seleccionar dos equipos.".to_string())),
    };

    // 2. matchMode no es "faceit" ni "steam" -> 400 "El modo de partida debe ser FACEIT o STEAM."
    let match_mode_lower = req.match_mode.to_ascii_lowercase();
    if match_mode_lower != "faceit" && match_mode_lower != "steam" {
        return Err((
            400,
            "El modo de partida debe ser FACEIT o STEAM.".to_string(),
        ));
    }

    // 3. Equipos no encontrados -> 404 "Equipos no encontrados."
    let team_a = match state.teams.iter().find(|t| t.id == team_a_id) {
        Some(t) => t,
        None => return Err((404, "Equipos no encontrados.".to_string())),
    };
    let team_b = match state.teams.iter().find(|t| t.id == team_b_id) {
        Some(t) => t,
        None => return Err((404, "Equipos no encontrados.".to_string())),
    };

    // 4. No admin y no capitán de ninguno de los dos -> 403 "Solo los capitanes del encuentro o un administrador pueden lanzar la partida."
    let is_admin = user.is_admin_like();
    let is_cap_a = team_a
        .captain
        .as_ref()
        .map_or(false, |c| c.discord_id == user.user_id);
    let is_cap_b = team_b
        .captain
        .as_ref()
        .map_or(false, |c| c.discord_id == user.user_id);
    if !is_admin && !is_cap_a && !is_cap_b {
        return Err((
            403,
            "Solo los capitanes del encuentro o un administrador pueden lanzar la partida."
                .to_string(),
        ));
    }

    // 5. Equipos incompletos (cada uno capitán + 4 jugadores) -> 400 "Ambos equipos deben estar completos (5 jugadores c/u)."
    if team_a.captain.is_none()
        || team_a.players.len() != 4
        || team_b.captain.is_none()
        || team_b.players.len() != 4
    {
        return Err((
            400,
            "Ambos equipos deben estar completos (5 jugadores c/u).".to_string(),
        ));
    }

    // 6. matchMode == "faceit" -> 400 "El modo FACEIT aún no está disponible en esta versión. Usa el modo STEAM."
    if match_mode_lower == "faceit" {
        return Err((
            400,
            "El modo FACEIT aún no está disponible en esta versión. Usa el modo STEAM.".to_string(),
        ));
    }

    // Rosters completos (capitán + 4 jugadores)
    let mut roster_a = Vec::with_capacity(5);
    if let Some(ref cap) = team_a.captain {
        roster_a.push(cap.clone());
    }
    roster_a.extend(team_a.players.iter().cloned());

    let mut roster_b = Vec::with_capacity(5);
    if let Some(ref cap) = team_b.captain {
        roster_b.push(cap.clone());
    }
    roster_b.extend(team_b.players.iter().cloned());

    // 7. Apuesta efectiva: admin -> max(0, bet_amount); no admin -> apuesta acordada
    // (state.bet_agreed && agreed_bet_amount.is_some() -> agreed; si no effective_bet_amount; si no veto.bet_amount; si no 0).
    // Si apuesta > 0 y NO todos los 10 jugadores están en manual_paid_discord_ids (solo cuenta si es admin) -> 400
    // "Las apuestas con saldo aún no están disponibles en esta versión. Un administrador debe marcar a todos los jugadores como pagados manualmente o lanzar sin apuesta."
    let effective_bet = if is_admin {
        req.bet_amount.max(0.0)
    } else if state.bet_agreed && state.agreed_bet_amount.is_some() {
        state.agreed_bet_amount.unwrap_or(0.0).max(0.0)
    } else if state.effective_bet_amount > 0.0 {
        state.effective_bet_amount.max(0.0)
    } else if let Some(ref veto) = state.map_veto {
        veto.bet_amount.max(0.0)
    } else {
        0.0
    };

    let manual_paid_set: std::collections::HashSet<&str> = if is_admin {
        req.manual_paid_discord_ids
            .iter()
            .map(|s| s.as_str())
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    if effective_bet > 0.0 {
        let all_paid = roster_a
            .iter()
            .chain(roster_b.iter())
            .all(|p| manual_paid_set.contains(p.discord_id.as_str()));
        if !all_paid {
            return Err((
                400,
                "Las apuestas con saldo aún no están disponibles en esta versión. Un administrador debe marcar a todos los jugadores como pagados manualmente o lanzar sin apuesta."
                    .to_string(),
            ));
        }
    }

    // 8. SteamID: cada uno de los 10 jugadores debe tener steam_id_64 de 17 dígitos (Player.steam_id_64); si faltan -> 400
    // "No se puede lanzar la partida. Todos los jugadores deben tener su SteamID vinculado para ingresar al servidor CS2. Falta vincular el Steam ID de: {nombres separados por ', '}."
    let mut missing_steam = Vec::new();
    for p in roster_a.iter().chain(roster_b.iter()) {
        if !is_valid_steam_id_64(p.steam_id_64.as_deref()) {
            missing_steam.push(player_display_name(p));
        }
    }
    if !missing_steam.is_empty() {
        return Err((
            400,
            format!(
                "No se puede lanzar la partida. Todos los jugadores deben tener su SteamID vinculado para ingresar al servidor CS2. Falta vincular el Steam ID de: {}.",
                missing_steam.join(", ")
            ),
        ));
    }

    // 9. Mapa final: chosen_map del request, o state.map_veto.chosen_map, o el primer mapa de map_catalog (o "de_mirage" si no hay).
    let chosen_map_value = req
        .chosen_map
        .clone()
        .filter(|v| !v.is_null())
        .or_else(|| {
            state
                .map_veto
                .as_ref()
                .and_then(|v| v.chosen_map.clone())
                .filter(|v| !v.is_null())
        })
        .or_else(|| {
            state.map_catalog.first().map(|m| {
                serde_json::to_value(m).unwrap_or_else(|_| {
                    serde_json::json!({
                        "id": m.id,
                        "name": m.name,
                    })
                })
            })
        })
        .unwrap_or_else(|| {
            serde_json::json!({
                "id": "de_mirage",
                "name": "Mirage"
            })
        });

    let chosen_maps_value = if let Some(ref veto) = state.map_veto {
        if !veto.chosen_maps.is_empty() {
            veto.chosen_maps.clone()
        } else {
            vec![chosen_map_value.clone()]
        }
    } else {
        vec![chosen_map_value.clone()]
    };

    let map_name = if let Some(name) = chosen_map_value.get("name").and_then(|v| v.as_str()) {
        name.to_string()
    } else if let Some(id) = chosen_map_value.get("id").and_then(|v| v.as_str()) {
        id.to_string()
    } else if let Some(s) = chosen_map_value.as_str() {
        s.to_string()
    } else {
        "Mirage".to_string()
    };

    let map_id = if let Some(id) = chosen_map_value.get("id").and_then(|v| v.as_str()) {
        id.to_string()
    } else if let Some(cs2_id) = chosen_map_value.get("cs2MapId").and_then(|v| v.as_str()) {
        cs2_id.to_string()
    } else if let Some(s) = chosen_map_value.as_str() {
        s.to_string()
    } else {
        "de_mirage".to_string()
    };

    // 10. gamesPerMatch: state.games_per_match o rounds_per_game si es 1/3/5, si no 1.
    let games_per_match = if [1, 3, 5].contains(&state.games_per_match) {
        state.games_per_match
    } else if [1, 3, 5].contains(&state.rounds_per_game) {
        state.rounds_per_game
    } else {
        1
    };

    // Nombres y tags de equipo
    let cap_a_name = team_a
        .captain
        .as_ref()
        .map(player_display_name)
        .unwrap_or_else(|| "Jugador A".to_string());
    let team_a_name = format!("Equipo {cap_a_name}");
    let team_a_tag = derive_team_tag(&cap_a_name, "EQ1");

    let cap_b_name = team_b
        .captain
        .as_ref()
        .map(player_display_name)
        .unwrap_or_else(|| "Jugador B".to_string());
    let team_b_name = format!("Equipo {cap_b_name}");
    let team_b_tag = derive_team_tag(&cap_b_name, "EQ2");

    // Rosters en formato (steam_id_64, nick)
    let team_a_roster = roster_a
        .iter()
        .map(|p| {
            (
                p.steam_id_64.clone().unwrap_or_default(),
                player_display_name(p),
            )
        })
        .collect();
    let team_b_roster = roster_b
        .iter()
        .map(|p| {
            (
                p.steam_id_64.clone().unwrap_or_default(),
                player_display_name(p),
            )
        })
        .collect();

    let manual_paid = if is_admin {
        req.manual_paid_discord_ids.clone()
    } else {
        Vec::new()
    };

    Ok(LaunchPlan {
        team_a_id: team_a_id.to_string(),
        team_b_id: team_b_id.to_string(),
        team_a_name,
        team_a_tag,
        team_a_roster,
        team_b_name,
        team_b_tag,
        team_b_roster,
        chosen_map: chosen_map_value,
        chosen_maps: chosen_maps_value,
        map_name,
        map_id,
        bet_per_player: effective_bet,
        manual_paid,
        games_per_match,
        full_roster_a: roster_a,
        full_roster_b: roster_b,
    })
}

/// Reinicia el estado del lobby después de lanzar la partida en CS2,
/// replicando fielmente `lobby.js:4697-4718`.
pub fn reset_lobby_after_launch(state: &mut LobbyState, team_a_id: &str, team_b_id: &str) {
    let mut launched_ids = std::collections::HashSet::new();
    for team in &state.teams {
        if team.id == team_a_id || team.id == team_b_id {
            if let Some(ref cap) = team.captain {
                launched_ids.insert(cap.discord_id.clone());
            }
            for p in &team.players {
                launched_ids.insert(p.discord_id.clone());
            }
        }
    }

    for team in &mut state.teams {
        if team.id == team_a_id || team.id == team_b_id {
            team.captain = None;
            team.players.clear();
            let num: String = team.id.chars().filter(|c| c.is_ascii_digit()).collect();
            if !num.is_empty() {
                team.name = format!("Equipo {num}");
                team.tag = format!("EQ{num}");
            }
        }
    }

    state.current_pick_index = 0;
    state.draft_started = false;
    state.draft_finished = false;
    state.pick_order = state
        .teams
        .iter()
        .filter(|t| t.captain.is_some())
        .map(|t| t.id.clone())
        .collect();
    state.pick_sequence.clear();
    state.map_veto = None;
    state.captain_a_proposal = None;
    state.captain_b_proposal = None;
    state.captain_a_ready = false;
    state.captain_b_ready = false;
    state.bet_agreed = false;
    state.agreed_bet_amount = None;
    state.effective_bet_amount = 0.0;
    state.auto_countdown_ends_at = None;
    state.roulette_winner_team_id = None;
    state.roulette_winner_name = None;
    state.roulette_spinning = false;
    state.roulette_started_at = None;
    state.roulette_slice_offset = None;

    state
        .available_players
        .retain(|p| !launched_ids.contains(&p.discord_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::MapInfo;
    use crate::state::{DiscordUserDetails, MapVetoState};

    fn dummy_player(id: &str, steam: Option<&str>) -> Player {
        Player {
            discord_id: id.to_string(),
            nickname: format!("Nick_{id}"),
            faceit_player_id: None,
            faceit_nickname: Some(format!("FC_{id}")),
            steam_id_64: steam.map(str::to_string),
            avatar: None,
            faceit_elo: 1500,
            faceit_level: Some(7),
            ready: true,
            tier: "A".to_string(),
            mixton_tier_override: None,
            discord_user: Some(DiscordUserDetails {
                username: format!("User_{id}"),
                display_name: format!("Display_{id}"),
                avatar_url: "".to_string(),
            }),
        }
    }

    fn complete_state_with_steams() -> LobbyState {
        let mut state = LobbyState::default();
        let team_a = &mut state.teams[0];
        team_a.captain = Some(dummy_player("cap_a", Some("76561198000000001")));
        team_a.players = vec![
            dummy_player("a1", Some("76561198000000002")),
            dummy_player("a2", Some("76561198000000003")),
            dummy_player("a3", Some("76561198000000004")),
            dummy_player("a4", Some("76561198000000005")),
        ];

        let team_b = &mut state.teams[1];
        team_b.captain = Some(dummy_player("cap_b", Some("76561198000000006")));
        team_b.players = vec![
            dummy_player("b1", Some("76561198000000007")),
            dummy_player("b2", Some("76561198000000008")),
            dummy_player("b3", Some("76561198000000009")),
            dummy_player("b4", Some("76561198000000010")),
        ];

        state
    }

    fn admin_user() -> LobbyUser {
        LobbyUser {
            user_id: "admin_1".to_string(),
            username: "admin".to_string(),
            avatar: None,
            role: Some("admin".to_string()),
            servidor: None,
            is_admin: true,
            nickname: None,
            faceit_nickname: None,
        }
    }

    fn captain_user() -> LobbyUser {
        LobbyUser {
            user_id: "cap_a".to_string(),
            username: "cap_a".to_string(),
            avatar: None,
            role: None,
            servidor: None,
            is_admin: false,
            nickname: None,
            faceit_nickname: None,
        }
    }

    fn random_user() -> LobbyUser {
        LobbyUser {
            user_id: "random_guy".to_string(),
            username: "random".to_string(),
            avatar: None,
            role: None,
            servidor: None,
            is_admin: false,
            nickname: None,
            faceit_nickname: None,
        }
    }

    #[test]
    fn validate_launch_fails_when_team_id_missing() {
        let state = complete_state_with_steams();
        let user = admin_user();

        let req = LaunchRequest {
            team_id_a: None,
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(err, (400, "Debes seleccionar dos equipos.".to_string()));

        let req2 = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("   ".into()),
            ..Default::default()
        };
        let err2 = validate_launch(&state, &req2, &user).unwrap_err();
        assert_eq!(err2, (400, "Debes seleccionar dos equipos.".to_string()));
    }

    #[test]
    fn validate_launch_fails_when_match_mode_invalid() {
        let state = complete_state_with_steams();
        let user = admin_user();

        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            match_mode: "custom".into(),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(
            err,
            (
                400,
                "El modo de partida debe ser FACEIT o STEAM.".to_string()
            )
        );
    }

    #[test]
    fn validate_launch_fails_when_teams_not_found() {
        let state = complete_state_with_steams();
        let user = admin_user();

        let req = LaunchRequest {
            team_id_a: Some("team_nonexistent".into()),
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(err, (404, "Equipos no encontrados.".to_string()));
    }

    #[test]
    fn validate_launch_fails_when_user_unauthorized() {
        let state = complete_state_with_steams();
        let user = random_user();

        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(
            err,
            (
                403,
                "Solo los capitanes del encuentro o un administrador pueden lanzar la partida."
                    .to_string()
            )
        );
    }

    #[test]
    fn validate_launch_allows_captain_and_admin() {
        let state = complete_state_with_steams();
        let cap = captain_user();
        let admin = admin_user();

        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };
        assert!(validate_launch(&state, &req, &cap).is_ok());
        assert!(validate_launch(&state, &req, &admin).is_ok());
    }

    #[test]
    fn validate_launch_fails_when_teams_incomplete() {
        let mut state = complete_state_with_steams();
        state.teams[0].players.pop(); // Solo 3 jugadores
        let user = admin_user();

        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(
            err,
            (
                400,
                "Ambos equipos deben estar completos (5 jugadores c/u).".to_string()
            )
        );
    }

    #[test]
    fn validate_launch_fails_when_match_mode_is_faceit() {
        let state = complete_state_with_steams();
        let user = admin_user();

        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            match_mode: "faceit".into(),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(
            err,
            (
                400,
                "El modo FACEIT aún no está disponible en esta versión. Usa el modo STEAM."
                    .to_string()
            )
        );
    }

    #[test]
    fn validate_launch_fails_when_steam_id_invalid_or_missing() {
        let mut state = complete_state_with_steams();
        state.teams[0].players[0].steam_id_64 = None; // FC_a1
        state.teams[1].players[1].steam_id_64 = Some("invalid_steam_id".into()); // FC_b2
        let user = admin_user();

        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };
        let err = validate_launch(&state, &req, &user).unwrap_err();
        assert_eq!(
            err,
            (
                400,
                "No se puede lanzar la partida. Todos los jugadores deben tener su SteamID vinculado para ingresar al servidor CS2. Falta vincular el Steam ID de: FC_a1, FC_b2.".to_string()
            )
        );
    }

    #[test]
    fn validate_launch_bet_without_and_with_manual_paid() {
        let state = complete_state_with_steams();
        let admin = admin_user();
        let cap = captain_user();

        // Admin lanza con apuesta pero sin manual paid completo -> 400
        let req1 = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            bet_amount: 5.0,
            manual_paid_discord_ids: vec!["cap_a".into()],
            ..Default::default()
        };
        let err1 = validate_launch(&state, &req1, &admin).unwrap_err();
        assert_eq!(
            err1,
            (
                400,
                "Las apuestas con saldo aún no están disponibles en esta versión. Un administrador debe marcar a todos los jugadores como pagados manualmente o lanzar sin apuesta.".to_string()
            )
        );

        // No-admin lanza con apuesta acordada pero manual_paid_discord_ids no le cuenta -> 400
        let mut state_with_agreed_bet = state.clone();
        state_with_agreed_bet.bet_agreed = true;
        state_with_agreed_bet.agreed_bet_amount = Some(10.0);
        let req2 = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            bet_amount: 10.0,
            manual_paid_discord_ids: vec![
                "cap_a".into(),
                "a1".into(),
                "a2".into(),
                "a3".into(),
                "a4".into(),
                "cap_b".into(),
                "b1".into(),
                "b2".into(),
                "b3".into(),
                "b4".into(),
            ],
            ..Default::default()
        };
        let err2 = validate_launch(&state_with_agreed_bet, &req2, &cap).unwrap_err();
        assert_eq!(
            err2,
            (
                400,
                "Las apuestas con saldo aún no están disponibles en esta versión. Un administrador debe marcar a todos los jugadores como pagados manualmente o lanzar sin apuesta.".to_string()
            )
        );

        // Admin marca a los 10 jugadores como pagados -> Ok
        let res3 = validate_launch(&state, &req2, &admin).unwrap();
        assert_eq!(res3.bet_per_player, 10.0);
        assert_eq!(res3.manual_paid.len(), 10);
    }

    #[test]
    fn validate_launch_map_priority() {
        let mut state = complete_state_with_steams();
        let admin = admin_user();

        // 1. chosen_map en LaunchRequest
        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            chosen_map: Some(serde_json::json!({ "id": "de_inferno", "name": "Inferno" })),
            ..Default::default()
        };
        let plan = validate_launch(&state, &req, &admin).unwrap();
        assert_eq!(plan.map_name, "Inferno");
        assert_eq!(plan.map_id, "de_inferno");

        // 2. state.map_veto.chosen_map
        let veto = MapVetoState {
            active: false,
            completed: true,
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            games_per_match: 1,
            veto_mode: "bo1".into(),
            action_type: "ban".into(),
            pool: vec![],
            steps: vec![],
            current_step_index: 0,
            current_ban_index: 0,
            banned_maps: vec![],
            picked_maps: vec![],
            ban_history: vec![],
            chosen_map: Some(serde_json::json!({ "id": "de_dust2", "name": "Dust II" })),
            chosen_maps: vec![serde_json::json!({ "id": "de_dust2", "name": "Dust II" })],
            ban_sequence: vec![],
            turn: "".into(),
            current_step: "".into(),
            active_maps: vec![],
            final_chosen_map: None,
            bet_amount: 0.0,
            queue_id: "auto".into(),
            match_mode: "steam".into(),
            started_at: None,
            turn_ends_at: None,
            auto_launch_ends_at: None,
        };
        state.map_veto = Some(veto.clone());
        let req2 = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            chosen_map: None,
            ..Default::default()
        };
        let plan2 = validate_launch(&state, &req2, &admin).unwrap();
        assert_eq!(plan2.map_name, "Dust II");
        assert_eq!(plan2.map_id, "de_dust2");

        // 3. state.map_catalog.first()
        state.map_veto = None;
        state.map_catalog = vec![MapInfo {
            id: "de_nuke".into(),
            name: "Nuke".into(),
            image: "".into(),
            icon: "".into(),
            active: true,
            is_official_premier: true,
        }];
        let plan3 = validate_launch(&state, &req2, &admin).unwrap();
        assert_eq!(plan3.map_name, "Nuke");
        assert_eq!(plan3.map_id, "de_nuke");

        // 4. Fallback de_mirage cuando catalog vacío
        state.map_catalog.clear();
        let plan4 = validate_launch(&state, &req2, &admin).unwrap();
        assert_eq!(plan4.map_name, "Mirage");
        assert_eq!(plan4.map_id, "de_mirage");
    }

    #[test]
    fn validate_launch_games_per_match() {
        let mut state = complete_state_with_steams();
        let admin = admin_user();
        let req = LaunchRequest {
            team_id_a: Some("team_1".into()),
            team_id_b: Some("team_2".into()),
            ..Default::default()
        };

        state.games_per_match = 3;
        assert_eq!(
            validate_launch(&state, &req, &admin)
                .unwrap()
                .games_per_match,
            3
        );

        state.games_per_match = 7; // No permitido -> 1
        assert_eq!(
            validate_launch(&state, &req, &admin)
                .unwrap()
                .games_per_match,
            1
        );
    }

    #[test]
    fn reset_lobby_after_launch_clears_all_fields_and_removes_players() {
        let mut state = complete_state_with_steams();
        // Agregar a los 10 jugadores en available_players más 2 extras
        for team in &state.teams {
            if let Some(ref cap) = team.captain {
                state.available_players.push(cap.clone());
            }
            state.available_players.extend(team.players.iter().cloned());
        }
        let extra1 = dummy_player("extra_1", None);
        let extra2 = dummy_player("extra_2", None);
        state.available_players.push(extra1.clone());
        state.available_players.push(extra2.clone());

        assert_eq!(state.available_players.len(), 12);

        state.current_pick_index = 4;
        state.draft_started = true;
        state.draft_finished = true;
        state.bet_agreed = true;
        state.agreed_bet_amount = Some(5.0);
        state.effective_bet_amount = 5.0;
        state.roulette_spinning = true;

        reset_lobby_after_launch(&mut state, "team_1", "team_2");

        // Equipos vacíos
        assert!(state.teams[0].captain.is_none());
        assert!(state.teams[0].players.is_empty());
        assert!(state.teams[1].captain.is_none());
        assert!(state.teams[1].players.is_empty());

        // Campos reseteados
        assert_eq!(state.current_pick_index, 0);
        assert!(!state.draft_started);
        assert!(!state.draft_finished);
        assert!(state.pick_sequence.is_empty());
        assert!(state.map_veto.is_none());
        assert!(state.captain_a_proposal.is_none());
        assert!(state.captain_b_proposal.is_none());
        assert!(!state.captain_a_ready);
        assert!(!state.captain_b_ready);
        assert!(!state.bet_agreed);
        assert!(state.agreed_bet_amount.is_none());
        assert_eq!(state.effective_bet_amount, 0.0);
        assert!(state.auto_countdown_ends_at.is_none());
        assert!(state.roulette_winner_team_id.is_none());
        assert!(state.roulette_winner_name.is_none());
        assert!(!state.roulette_spinning);
        assert!(state.roulette_started_at.is_none());
        assert!(state.roulette_slice_offset.is_none());

        // Solo quedaron los 2 jugadores extras
        assert_eq!(state.available_players.len(), 2);
        assert_eq!(state.available_players[0].discord_id, "extra_1");
        assert_eq!(state.available_players[1].discord_id, "extra_2");
    }
}
