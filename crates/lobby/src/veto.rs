//! Módulo de Veto de Mapas con paridad exacta respecto al legado (`legacy_ref/lobby.js`).
//!
//! Referencias del legado:
//! - `generateMapBanSequence`: líneas 2797-2817
//! - `generateMapVetoPlan`: líneas 2825-2873
//! - `applyVetoMapAction`: líneas 2878-3056
//! - `autoStartMapVetoIfReady`: líneas 769-835
//! - `/mixton-start-veto`: líneas 3352-3457
//! - `/mixton-ban-map`: líneas 3460-3528
//! - `/mixton-cancel-veto`: líneas 3531-3551

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::maps::MapInfo;
use crate::state::{LobbyState, MapVetoState};

/// Paso programado dentro del plan de veto (ban o pick).
/// Referencia legado: `generateMapVetoPlan` (líneas 2834-2839, 2850-2852, 2864-2868).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VetoStep {
    #[serde(rename = "stepIndex")]
    pub step_index: usize,
    #[serde(rename = "type")]
    pub step_type: String,
    #[serde(rename = "teamId")]
    pub team_id: String,
    pub label: String,
    #[serde(
        rename = "pickNumber",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub pick_number: Option<usize>,
}

/// Plan estructurado de veto según el número de partidas (BO1, BO3, BO5).
/// Referencia legado: líneas 2825-2873.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VetoPlan {
    pub mode: String,
    pub action_type: String,
    pub steps: Vec<VetoStep>,
    pub total_picks_needed: usize,
    pub random_decider_needed: bool,
    pub random_decider_count: Option<usize>,
}

/// Genera la secuencia de turnos para el baneo de mapas entre 2 equipos.
/// Regla del legado (`legacy_ref/lobby.js` líneas 2797-2817):
/// El 1er capitán banea 1, luego el 2do banea 2, luego el 1ero banea 2,
/// y así sucesivamente hasta que solo quede exactamente 1 mapa.
pub fn generate_map_ban_sequence(
    team_a_id: &str,
    team_b_id: &str,
    total_maps: usize,
) -> Vec<String> {
    let bans_needed = if total_maps > 1 { total_maps - 1 } else { 0 };
    let mut sequence = Vec::with_capacity(bans_needed);
    let mut remaining = bans_needed;
    let mut turn = 0;

    while remaining > 0 {
        let is_team_a = turn % 2 == 0;
        let team_id = if is_team_a { team_a_id } else { team_b_id };
        let target_bans = if turn == 0 { 1 } else { 2 };
        let bans_this_turn = target_bans.min(remaining);

        for _ in 0..bans_this_turn {
            sequence.push(team_id.to_string());
            remaining -= 1;
        }
        turn += 1;
    }

    sequence
}

/// Genera el plan completo de Veto según los juegos configurados:
/// - BO1 (1 juego): Baneo alternado hasta que queda 1 mapa.
/// - BO3 (3 juegos): 1 pick cada equipo + 3er mapa decider aleatorio.
/// - BO5 (5 juegos): 2 picks cada capitán (total 4) + 5to mapa decider aleatorio.
/// Referencia legado: líneas 2825-2873.
pub fn generate_map_veto_plan(
    team_a_id: &str,
    team_b_id: &str,
    map_pool_len: usize,
    games_count: u32,
) -> VetoPlan {
    let g = match games_count {
        1 | 3 | 5 => games_count,
        _ => 1,
    };

    if g == 1 {
        let ban_seq = generate_map_ban_sequence(team_a_id, team_b_id, map_pool_len);
        let steps = ban_seq
            .into_iter()
            .enumerate()
            .map(|(idx, t_id)| VetoStep {
                step_index: idx,
                step_type: "ban".to_string(),
                team_id: t_id,
                label: format!("Baneo #{}", idx + 1),
                pick_number: None,
            })
            .collect();

        VetoPlan {
            mode: "bo1".to_string(),
            action_type: "ban".to_string(),
            steps,
            total_picks_needed: 0,
            random_decider_needed: false,
            random_decider_count: None,
        }
    } else if g == 3 {
        let steps = vec![
            VetoStep {
                step_index: 0,
                step_type: "pick".to_string(),
                team_id: team_a_id.to_string(),
                label: "Elección de mapa (Pick Equipo A)".to_string(),
                pick_number: Some(1),
            },
            VetoStep {
                step_index: 1,
                step_type: "pick".to_string(),
                team_id: team_b_id.to_string(),
                label: "Elección de mapa (Pick Equipo B)".to_string(),
                pick_number: Some(2),
            },
        ];

        VetoPlan {
            mode: "bo3".to_string(),
            action_type: "pick".to_string(),
            steps,
            total_picks_needed: 2,
            random_decider_needed: true,
            random_decider_count: Some(1),
        }
    } else {
        // BO5
        let steps = vec![
            VetoStep {
                step_index: 0,
                step_type: "pick".to_string(),
                team_id: team_a_id.to_string(),
                label: "1ra Elección de mapa (Pick 1 Equipo A)".to_string(),
                pick_number: Some(1),
            },
            VetoStep {
                step_index: 1,
                step_type: "pick".to_string(),
                team_id: team_b_id.to_string(),
                label: "1ra Elección de mapa (Pick 1 Equipo B)".to_string(),
                pick_number: Some(2),
            },
            VetoStep {
                step_index: 2,
                step_type: "pick".to_string(),
                team_id: team_a_id.to_string(),
                label: "2da Elección de mapa (Pick 2 Equipo A)".to_string(),
                pick_number: Some(3),
            },
            VetoStep {
                step_index: 3,
                step_type: "pick".to_string(),
                team_id: team_b_id.to_string(),
                label: "2da Elección de mapa (Pick 2 Equipo B)".to_string(),
                pick_number: Some(4),
            },
        ];

        VetoPlan {
            mode: "bo5".to_string(),
            action_type: "pick".to_string(),
            steps,
            total_picks_needed: 4,
            random_decider_needed: true,
            random_decider_count: Some(1),
        }
    }
}

/// Inicializa el estado `MapVetoState` validando la existencia de al menos 2 mapas activos.
/// Referencia legado: líneas 809-832 y 3415-3446.
pub fn init_map_veto_state(
    team_a_id: &str,
    team_b_id: &str,
    map_pool: Vec<MapInfo>,
    games_per_match: u32,
    bet_amount: f64,
    queue_id: &str,
    match_mode: &str,
    now_ms: i64,
) -> Result<MapVetoState, (StatusCode, &'static str)> {
    if map_pool.len() < 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Se requieren al menos 2 mapas activos para iniciar el veto. Configura los mapas en el panel administrativo.",
        ));
    }

    let normalized_games = match games_per_match {
        1 | 3 | 5 => games_per_match,
        _ => 1,
    };

    let veto_plan = generate_map_veto_plan(team_a_id, team_b_id, map_pool.len(), normalized_games);

    let ban_sequence = if veto_plan.mode == "bo1" {
        veto_plan.steps.iter().map(|s| s.team_id.clone()).collect()
    } else {
        Vec::new()
    };

    let first_turn_team = veto_plan
        .steps
        .first()
        .map(|s| s.team_id.clone())
        .unwrap_or_else(|| team_a_id.to_string());

    let state = MapVetoState {
        active: true,
        completed: false,
        team_id_a: Some(team_a_id.to_string()),
        team_id_b: Some(team_b_id.to_string()),
        pool: map_pool.clone(),
        games_per_match: normalized_games,
        veto_mode: veto_plan.mode,
        action_type: veto_plan.action_type.clone(),
        steps: veto_plan.steps,
        current_step_index: 0,
        ban_sequence,
        current_ban_index: 0,
        banned_maps: Vec::new(),
        picked_maps: Vec::new(),
        ban_history: Vec::new(),
        chosen_map: None,
        chosen_maps: Vec::new(),
        turn: first_turn_team,
        current_step: veto_plan.action_type,
        active_maps: map_pool,
        final_chosen_map: None,
        bet_amount: bet_amount.max(0.0),
        queue_id: queue_id.to_string(),
        match_mode: match_mode.to_string(),
        started_at: Some(now_ms),
        turn_ends_at: Some(now_ms + 20_000),
        auto_launch_ends_at: None,
    };

    Ok(state)
}

/// Información del turno actual requerida para validar la acción.
#[derive(Debug, Clone, PartialEq)]
pub struct VetoTurnContext {
    pub team_id: String,
    pub team_name: String,
    pub captain_nickname: String,
    pub is_pick_mode: bool,
}

/// Verifica si es el turno del usuario o si posee rol de administrador.
/// Referencia legado: líneas 3474-3504.
pub fn verify_veto_turn(
    state: &LobbyState,
    veto: &MapVetoState,
    user_id: &str,
    user_role: &str,
    is_admin: bool,
) -> Result<VetoTurnContext, (StatusCode, String)> {
    let is_pick_mode =
        veto.action_type == "pick" || veto.veto_mode == "bo3" || veto.veto_mode == "bo5";

    let current_turn_team_id = if is_pick_mode {
        veto.steps
            .get(veto.current_step_index)
            .map(|s| s.team_id.as_str())
            .or(veto.team_id_a.as_deref())
            .unwrap_or("team_1")
    } else {
        veto.ban_sequence
            .get(veto.current_ban_index)
            .map(|s| s.as_str())
            .or_else(|| {
                veto.steps
                    .get(veto.current_step_index)
                    .map(|s| s.team_id.as_str())
            })
            .or(veto.team_id_a.as_deref())
            .unwrap_or("team_1")
    };

    let team_in_turn = state
        .teams
        .iter()
        .find(|t| t.id == current_turn_team_id)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Equipo o capitán en turno no encontrado.".to_string(),
            )
        })?;

    let captain = team_in_turn.captain.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Equipo o capitán en turno no encontrado.".to_string(),
        )
    })?;

    let is_captain = captain.discord_id == user_id;
    let admin_flag = user_role == "global_admin" || user_role == "admin" || is_admin;

    if !is_captain && !admin_flag {
        let action_verb = if is_pick_mode {
            "seleccionar tu mapa"
        } else {
            "banear"
        };
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "No es tu turno para {action_verb}. Es turno de {}.",
                team_in_turn.name
            ),
        ));
    }

    let cap_nick = captain
        .discord_user
        .as_ref()
        .map(|u| u.display_name.clone())
        .or_else(|| captain.faceit_nickname.clone())
        .unwrap_or_else(|| captain.nickname.clone());

    Ok(VetoTurnContext {
        team_id: current_turn_team_id.to_string(),
        team_name: team_in_turn.name.clone(),
        captain_nickname: cap_nick,
        is_pick_mode,
    })
}

/// Resultado de aplicar una acción de veto.
#[derive(Debug, Clone, PartialEq)]
pub struct VetoActionResult {
    pub completed: bool,
}

/// Aplica una acción de veto (ban o pick) sobre el mapa indicado.
/// Referencia legado: líneas 2878-3056 (`applyVetoMapAction`).
pub fn apply_veto_map_action(
    state: &mut LobbyState,
    map_id: &str,
    actor_nickname: Option<&str>,
    now_ms: i64,
    random_decider_index: Option<usize>,
) -> Result<VetoActionResult, (StatusCode, String)> {
    let veto = match state.map_veto.as_mut() {
        Some(v) if v.active && !v.completed => v,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "No hay una fase de veto de mapas activa.".to_string(),
            ))
        }
    };

    if map_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Debes indicar el mapa a banear.".to_string(),
        ));
    }

    let is_pick_mode =
        veto.action_type == "pick" || veto.veto_mode == "bo3" || veto.veto_mode == "bo5";

    if !is_pick_mode && veto.banned_maps.contains(&map_id.to_string()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Este mapa ya fue baneado.".to_string(),
        ));
    }

    if is_pick_mode
        && veto
            .picked_maps
            .iter()
            .any(|p| p.get("mapId").and_then(|v| v.as_str()) == Some(map_id))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Este mapa ya fue seleccionado por un equipo.".to_string(),
        ));
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

    let map_item = pool
        .iter()
        .find(|m| m.id == map_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Mapa no válido en el pool.".to_string(),
            )
        })?;

    // 1. MODO BO1 (BANEO HASTA QUE QUEDE 1 MAPA)
    // Referencia legado: líneas 2891-2940.
    if !is_pick_mode {
        let current_turn_team_id = veto
            .ban_sequence
            .get(veto.current_ban_index)
            .cloned()
            .unwrap_or_else(|| {
                veto.team_id_a
                    .clone()
                    .unwrap_or_else(|| "team_1".to_string())
            });

        let team_in_turn = state.teams.iter().find(|t| t.id == current_turn_team_id);
        let team_name = team_in_turn
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "Equipo".to_string());
        let cap_nick = team_in_turn.and_then(|t| {
            t.captain.as_ref().and_then(|c| {
                c.discord_user
                    .as_ref()
                    .map(|u| u.display_name.clone())
                    .or_else(|| c.faceit_nickname.clone())
            })
        });

        let banned_by_nickname = actor_nickname
            .map(|s| s.to_string())
            .or(cap_nick)
            .unwrap_or_else(|| "Capitán".to_string());

        veto.banned_maps.push(map_id.to_string());
        veto.ban_history.push(json!({
            "mapId": map_id,
            "mapName": map_item.name,
            "teamId": current_turn_team_id,
            "teamName": team_name,
            "bannedByNickname": banned_by_nickname,
            "isPick": false,
            "timestamp": now_ms
        }));

        veto.current_ban_index += 1;
        veto.current_step_index += 1;
        veto.active_maps.retain(|m| m.id != map_id);

        let remaining_maps: Vec<MapInfo> = pool
            .into_iter()
            .filter(|m| !veto.banned_maps.contains(&m.id))
            .collect();

        if remaining_maps.len() == 1 {
            let final_map = &remaining_maps[0];
            veto.chosen_map = Some(json!(final_map));
            veto.chosen_maps = vec![json!(final_map)];
            veto.final_chosen_map = Some(final_map.id.clone());
            veto.active = false;
            veto.completed = true;
            veto.turn_ends_at = None;
            veto.auto_launch_ends_at = Some(now_ms + 4500);

            Ok(VetoActionResult { completed: true })
        } else {
            veto.turn_ends_at = Some(now_ms + 20_000);
            if let Some(next_team) = veto.ban_sequence.get(veto.current_ban_index) {
                veto.turn = next_team.clone();
            }
            Ok(VetoActionResult { completed: false })
        }
    } else {
        // 2. MODO BO3 / BO5 (PICKS DE EQUIPOS + MAPA DECIDER ALEATORIO)
        // Referencia legado: líneas 2943-3056.
        if veto.current_step_index >= veto.steps.len() {
            return Err((
                StatusCode::BAD_REQUEST,
                "No hay más pasos de selección programados.".to_string(),
            ));
        }

        let current_step = veto.steps[veto.current_step_index].clone();
        let team_in_turn_id = current_step.team_id.clone();
        let team_in_turn = state.teams.iter().find(|t| t.id == team_in_turn_id);
        let team_name = team_in_turn
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "Equipo".to_string());
        let cap_nick = team_in_turn.and_then(|t| {
            t.captain.as_ref().and_then(|c| {
                c.discord_user
                    .as_ref()
                    .map(|u| u.display_name.clone())
                    .or_else(|| c.faceit_nickname.clone())
            })
        });

        let order_num = veto.picked_maps.len() + 1;
        let picked_by_nickname = actor_nickname
            .map(|s| s.to_string())
            .or(cap_nick)
            .unwrap_or_else(|| "Capitán".to_string());

        let picked_entry = json!({
            "mapId": map_item.id,
            "mapName": map_item.name,
            "image": map_item.image,
            "icon": map_item.icon,
            "teamId": team_in_turn_id,
            "teamName": team_name,
            "pickedByNickname": picked_by_nickname,
            "order": order_num,
            "isRandom": false,
            "timestamp": now_ms
        });

        veto.picked_maps.push(picked_entry);
        veto.ban_history.push(json!({
            "mapId": map_item.id,
            "mapName": map_item.name,
            "teamId": team_in_turn_id,
            "teamName": team_name,
            "bannedByNickname": format!("{picked_by_nickname} (Pick #{order_num})"),
            "isPick": true,
            "isRandom": false,
            "timestamp": now_ms
        }));

        veto.current_step_index += 1;

        if veto.current_step_index >= veto.steps.len() {
            // Completados todos los picks de capitanes -> sorteo del decider aleatorio
            let current_picked_ids: Vec<String> = veto
                .picked_maps
                .iter()
                .filter_map(|p| p.get("mapId").and_then(|v| v.as_str()).map(str::to_string))
                .collect();

            let available_pool: Vec<MapInfo> = pool
                .iter()
                .filter(|m| !current_picked_ids.contains(&m.id))
                .cloned()
                .collect();

            if !available_pool.is_empty() {
                let decider_idx = random_decider_index.unwrap_or(0) % available_pool.len();
                let random_decider = &available_pool[decider_idx];
                let decider_order = veto.picked_maps.len() + 1;

                let decider_entry = json!({
                    "mapId": random_decider.id,
                    "mapName": random_decider.name,
                    "image": random_decider.image,
                    "icon": random_decider.icon,
                    "teamId": "random_decider",
                    "teamName": "Sorteo Aleatorio",
                    "pickedByNickname": "Sistema (Aleatorio)",
                    "order": decider_order,
                    "isRandom": true,
                    "timestamp": now_ms
                });

                veto.picked_maps.push(decider_entry);
                veto.ban_history.push(json!({
                    "mapId": random_decider.id,
                    "mapName": random_decider.name,
                    "teamId": "random_decider",
                    "teamName": "Sorteo Aleatorio",
                    "bannedByNickname": format!("Sorteo Aleatorio (Decider #{decider_order})"),
                    "isPick": true,
                    "isRandom": true,
                    "timestamp": now_ms
                }));
            }

            veto.chosen_maps = veto
                .picked_maps
                .iter()
                .map(|p| {
                    let map_id = p.get("mapId").and_then(|v| v.as_str()).unwrap_or("");
                    let orig = pool
                        .iter()
                        .find(|m| m.id == map_id)
                        .cloned()
                        .unwrap_or_else(|| MapInfo {
                            id: map_id.to_string(),
                            name: p
                                .get("mapName")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            image: p
                                .get("image")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            icon: p
                                .get("icon")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            active: true,
                            is_official_premier: true,
                        });

                    json!({
                        "id": orig.id,
                        "name": orig.name,
                        "image": orig.image,
                        "icon": orig.icon,
                        "active": orig.active,
                        "pickedByTeamId": p.get("teamId"),
                        "teamName": p.get("teamName"),
                        "isRandom": p.get("isRandom"),
                        "order": p.get("order")
                    })
                })
                .collect();

            veto.chosen_map = veto.chosen_maps.first().cloned();
            veto.final_chosen_map = veto
                .chosen_maps
                .first()
                .and_then(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string));
            veto.active = false;
            veto.completed = true;
            veto.turn_ends_at = None;
            veto.auto_launch_ends_at = Some(now_ms + 4500);

            Ok(VetoActionResult { completed: true })
        } else {
            veto.turn_ends_at = Some(now_ms + 20_000);
            if let Some(next_step) = veto.steps.get(veto.current_step_index) {
                veto.turn = next_step.team_id.clone();
            }
            Ok(VetoActionResult { completed: false })
        }
    }
}

/// Cancela la fase de veto activa.
/// Referencia legado: líneas 3531-3551 (`mixton-cancel-veto`).
pub fn cancel_veto(state: &mut LobbyState) {
    state.map_veto = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::default_catalog;
    use crate::state::{LobbyTeam, Player};

    fn make_test_player(id: &str, _is_captain: bool) -> Player {
        Player {
            discord_id: id.to_string(),
            nickname: format!("Nick_{id}"),
            faceit_player_id: None,
            faceit_nickname: Some(format!("Faceit_{id}")),
            steam_id_64: None,
            avatar: None,
            faceit_elo: 1500,
            faceit_level: Some(7),
            ready: true,
            tier: "A".to_string(),
            mixton_tier_override: None,
            discord_user: None,
        }
    }

    fn make_test_state_with_teams() -> LobbyState {
        let mut state = LobbyState::default();
        let team_a = LobbyTeam {
            id: "team_1".to_string(),
            name: "Equipo Alfa".to_string(),
            tag: "ALFA".to_string(),
            captain: Some(make_test_player("cap_1", true)),
            players: vec![
                make_test_player("p1_1", false),
                make_test_player("p1_2", false),
                make_test_player("p1_3", false),
                make_test_player("p1_4", false),
            ],
        };
        let team_b = LobbyTeam {
            id: "team_2".to_string(),
            name: "Equipo Beta".to_string(),
            tag: "BETA".to_string(),
            captain: Some(make_test_player("cap_2", true)),
            players: vec![
                make_test_player("p2_1", false),
                make_test_player("p2_2", false),
                make_test_player("p2_3", false),
                make_test_player("p2_4", false),
            ],
        };
        state.teams = vec![team_a, team_b];
        state.map_catalog = default_catalog();
        state
    }

    #[test]
    fn test_generate_map_ban_sequence_bo1_7_maps() {
        // Para 7 mapas, se requieren 6 baneos:
        // Turn 0 (Equipo A): 1 ban
        // Turn 1 (Equipo B): 2 bans
        // Turn 2 (Equipo A): 2 bans
        // Turn 3 (Equipo B): 1 ban (restante)
        let seq = generate_map_ban_sequence("A", "B", 7);
        assert_eq!(seq, vec!["A", "B", "B", "A", "A", "B"]);
        assert_eq!(seq.len(), 6);
    }

    #[test]
    fn test_generate_map_veto_plan_bo1() {
        let plan = generate_map_veto_plan("team_A", "team_B", 7, 1);
        assert_eq!(plan.mode, "bo1");
        assert_eq!(plan.action_type, "ban");
        assert_eq!(plan.steps.len(), 6);
        assert_eq!(plan.steps[0].team_id, "team_A");
        assert_eq!(plan.steps[0].label, "Baneo #1");
        assert_eq!(plan.steps[1].team_id, "team_B");
        assert_eq!(plan.steps[2].team_id, "team_B");
        assert_eq!(plan.steps[3].team_id, "team_A");
        assert_eq!(plan.steps[4].team_id, "team_A");
        assert_eq!(plan.steps[5].team_id, "team_B");
    }

    #[test]
    fn test_generate_map_veto_plan_bo3() {
        let plan = generate_map_veto_plan("team_A", "team_B", 7, 3);
        assert_eq!(plan.mode, "bo3");
        assert_eq!(plan.action_type, "pick");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].team_id, "team_A");
        assert_eq!(plan.steps[0].pick_number, Some(1));
        assert_eq!(plan.steps[1].team_id, "team_B");
        assert_eq!(plan.steps[1].pick_number, Some(2));
        assert!(plan.random_decider_needed);
        assert_eq!(plan.random_decider_count, Some(1));
    }

    #[test]
    fn test_generate_map_veto_plan_bo5() {
        let plan = generate_map_veto_plan("team_A", "team_B", 7, 5);
        assert_eq!(plan.mode, "bo5");
        assert_eq!(plan.action_type, "pick");
        assert_eq!(plan.steps.len(), 4);
        assert_eq!(plan.steps[0].team_id, "team_A");
        assert_eq!(plan.steps[1].team_id, "team_B");
        assert_eq!(plan.steps[2].team_id, "team_A");
        assert_eq!(plan.steps[3].team_id, "team_B");
        assert!(plan.random_decider_needed);
    }

    #[test]
    fn test_init_map_veto_error_insufficient_maps() {
        let maps = vec![MapInfo {
            id: "de_mirage".to_string(),
            name: "Mirage".to_string(),
            image: "".to_string(),
            icon: "".to_string(),
            active: true,
            is_official_premier: true,
        }];
        let res = init_map_veto_state("A", "B", maps, 1, 0.0, "auto", "steam", 1000);
        assert!(res.is_err());
        let (code, msg) = res.unwrap_err();
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert_eq!(
            msg,
            "Se requieren al menos 2 mapas activos para iniciar el veto. Configura los mapas en el panel administrativo."
        );
    }

    #[test]
    fn test_verify_veto_turn_actor_validation() {
        let state = make_test_state_with_teams();
        let maps = state
            .map_catalog
            .iter()
            .filter(|m| m.active)
            .cloned()
            .collect();
        let veto =
            init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000).unwrap();

        // Turno inicial de team_1 (capitán cap_1).
        // 1. Capitán ajeno (cap_2) intenta actuar fuera de turno -> 403
        let err = verify_veto_turn(&state, &veto, "cap_2", "user", false).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(
            err.1,
            "No es tu turno para banear. Es turno de Equipo Alfa."
        );

        // 2. Capitán correcto (cap_1) -> Ok
        let ok = verify_veto_turn(&state, &veto, "cap_1", "user", false).unwrap();
        assert_eq!(ok.team_id, "team_1");

        // 3. Admin actuando por el equipo -> Ok
        let admin_ok = verify_veto_turn(&state, &veto, "admin_user", "admin", false).unwrap();
        assert_eq!(admin_ok.team_id, "team_1");
    }

    #[test]
    fn test_apply_veto_map_action_bo1_full_flow() {
        let mut state = make_test_state_with_teams();
        let maps: Vec<MapInfo> = state
            .map_catalog
            .iter()
            .filter(|m| m.active)
            .cloned()
            .collect();
        let map_count = maps.len(); // 7 mapas en premier
        assert_eq!(map_count, 7);

        state.map_veto = Some(
            init_map_veto_state(
                "team_1",
                "team_2",
                maps.clone(),
                1,
                0.0,
                "auto",
                "steam",
                1000,
            )
            .unwrap(),
        );

        // Ban 1: de_mirage (team_1)
        let r1 = apply_veto_map_action(&mut state, "de_mirage", Some("Cap1"), 1100, None).unwrap();
        assert!(!r1.completed);
        let v = state.map_veto.as_ref().unwrap();
        assert_eq!(v.banned_maps, vec!["de_mirage"]);
        assert_eq!(v.current_ban_index, 1);
        assert_eq!(v.current_step_index, 1);
        assert_eq!(v.turn, "team_2");

        // Error al intentar banear mapa repetido
        let rep_err =
            apply_veto_map_action(&mut state, "de_mirage", Some("Cap2"), 1200, None).unwrap_err();
        assert_eq!(rep_err.0, StatusCode::BAD_REQUEST);
        assert_eq!(rep_err.1, "Este mapa ya fue baneado.");

        // Error mapa fuera del pool
        let not_found_err =
            apply_veto_map_action(&mut state, "de_invalid_map", Some("Cap2"), 1200, None)
                .unwrap_err();
        assert_eq!(not_found_err.0, StatusCode::NOT_FOUND);
        assert_eq!(not_found_err.1, "Mapa no válido en el pool.");

        // Ban 2: de_inferno (team_2)
        apply_veto_map_action(&mut state, "de_inferno", Some("Cap2"), 1300, None).unwrap();
        // Ban 3: de_nuke (team_2)
        apply_veto_map_action(&mut state, "de_nuke", Some("Cap2"), 1400, None).unwrap();
        // Ban 4: de_ancient (team_1)
        apply_veto_map_action(&mut state, "de_ancient", Some("Cap1"), 1500, None).unwrap();
        // Ban 5: de_anubis (team_1)
        apply_veto_map_action(&mut state, "de_anubis", Some("Cap1"), 1600, None).unwrap();

        // Ban 6: de_dust2 (team_2) -> Debe completar el veto y quedar de_vertigo
        let r6 = apply_veto_map_action(&mut state, "de_dust2", Some("Cap2"), 1700, None).unwrap();
        assert!(r6.completed);

        let final_v = state.map_veto.as_ref().unwrap();
        assert!(final_v.completed);
        assert!(!final_v.active);
        assert_eq!(final_v.banned_maps.len(), 6);
        assert_eq!(final_v.final_chosen_map.as_deref(), Some("de_vertigo"));
        assert_eq!(
            final_v
                .chosen_map
                .as_ref()
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str()),
            Some("de_vertigo")
        );
        assert_eq!(final_v.turn_ends_at, None);
        assert_eq!(final_v.auto_launch_ends_at, Some(1700 + 4500));
    }

    #[test]
    fn test_apply_veto_map_action_bo3_picks_and_random_decider() {
        let mut state = make_test_state_with_teams();
        let maps: Vec<MapInfo> = state
            .map_catalog
            .iter()
            .filter(|m| m.active)
            .cloned()
            .collect();

        state.map_veto = Some(
            init_map_veto_state("team_1", "team_2", maps, 3, 0.0, "auto", "steam", 2000).unwrap(),
        );

        // Pick 1 por team_1: de_mirage
        let p1 = apply_veto_map_action(&mut state, "de_mirage", Some("Cap1"), 2100, None).unwrap();
        assert!(!p1.completed);

        // Intento de re-pickear mapa ya seleccionado
        let rep_pick =
            apply_veto_map_action(&mut state, "de_mirage", Some("Cap2"), 2200, None).unwrap_err();
        assert_eq!(rep_pick.0, StatusCode::BAD_REQUEST);
        assert_eq!(rep_pick.1, "Este mapa ya fue seleccionado por un equipo.");

        // Pick 2 por team_2: de_inferno con random_decider fijado al índice 0 del pool disponible
        let p2 =
            apply_veto_map_action(&mut state, "de_inferno", Some("Cap2"), 2300, Some(0)).unwrap();
        assert!(p2.completed);

        let final_v = state.map_veto.as_ref().unwrap();
        assert!(final_v.completed);
        // Debe haber 3 pickedMaps (Pick 1, Pick 2, Decider aleatorio)
        assert_eq!(final_v.picked_maps.len(), 3);
        assert_eq!(final_v.chosen_maps.len(), 3);
        assert_eq!(final_v.picked_maps[0]["mapId"], "de_mirage");
        assert_eq!(final_v.picked_maps[1]["mapId"], "de_inferno");
        assert_eq!(final_v.picked_maps[2]["isRandom"], true);
        assert_eq!(final_v.picked_maps[2]["teamId"], "random_decider");
    }

    #[test]
    fn test_cancel_veto_clears_state() {
        let mut state = make_test_state_with_teams();
        let maps: Vec<MapInfo> = state
            .map_catalog
            .iter()
            .filter(|m| m.active)
            .cloned()
            .collect();
        state.map_veto = Some(
            init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000).unwrap(),
        );
        assert!(state.map_veto.is_some());

        cancel_veto(&mut state);
        assert!(state.map_veto.is_none());
    }
}
