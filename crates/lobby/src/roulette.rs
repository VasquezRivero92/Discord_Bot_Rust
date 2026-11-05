//! Lógica pura de la ruleta automática de Mixtón, temporización e inicio de draft.
//! Portado fielmente desde la implementación de referencia en Node.js (lobby.js:427-519).

use crate::draft::{generate_draft_pick_sequence, proposal_to_f64};
use crate::state::LobbyState;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Tiempo por defecto en milisegundos de animación de la ruleta antes de iniciar el draft.
pub const ROULETTE_DRAFT_DELAY_MS: u64 = 9500;

/// Configuración de tiempos de la ruleta (parametrizable para agilizar tests unitarios).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouletteTimings {
    pub draft_delay_ms: u64,
}

impl Default for RouletteTimings {
    fn default() -> Self {
        Self {
            draft_delay_ms: ROULETTE_DRAFT_DELAY_MS,
        }
    }
}

static RANDOM_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Genera un número pseudoaleatorio en el rango [0, 1) sin añadir dependencias externas.
pub fn random_unit() -> f64 {
    let mut hasher = RandomState::new().build_hasher();
    let count = RANDOM_COUNTER.fetch_add(1, Ordering::Relaxed);
    let elapsed = Instant::now().elapsed().as_nanos();
    hasher.write_u64(count);
    hasher.write_u128(elapsed);
    let val = hasher.finish();
    let r = (val as f64) / (u64::MAX as f64);
    if r >= 1.0 {
        0.9999999999999999
    } else if r < 0.0 {
        0.0
    } else {
        r
    }
}

/// Decide el resultado de la ruleta de prioridad entre los dos primeros equipos.
/// Función pura y totalmente determinista basada en `coin`, `slice_rand` y `now_ms`.
///
/// Port de lobby.js:436-478.
pub fn decide_roulette(
    state: &mut LobbyState,
    coin: f64,
    slice_rand: f64,
    now_ms: i64,
) -> Result<(), &'static str> {
    if state.draft_started || state.draft_finished {
        return Err("El draft ya está iniciado o concluido.");
    }

    if state.teams.len() < 2 || state.teams[0].captain.is_none() || state.teams[1].captain.is_none()
    {
        return Err("Falta al menos un capitán.");
    }

    if !state.captain_a_ready || !state.captain_b_ready {
        return Err("Los capitanes no están listos.");
    }

    let prop_a = proposal_to_f64(&state.captain_a_proposal);
    let prop_b = proposal_to_f64(&state.captain_b_proposal);
    match (prop_a, prop_b) {
        (Some(a), Some(b)) if a == b => {}
        _ => return Err("Los montos de apuesta no coinciden."),
    }

    let team0_id = state.teams[0].id.clone();
    let team1_id = state.teams[1].id.clone();

    // Orden 50/50 de prioridad
    let order = if coin < 0.5 {
        vec![team0_id, team1_id]
    } else {
        vec![team1_id, team0_id]
    };

    let winner_id = order[0].clone();
    let winner_team = state.teams.iter().find(|t| t.id == winner_id);

    let winner_name = winner_team
        .and_then(|t| t.captain.as_ref())
        .and_then(|c| {
            c.discord_user
                .as_ref()
                .map(|u| u.display_name.clone())
                .filter(|s| !s.is_empty())
                .or_else(|| c.faceit_nickname.clone().filter(|s| !s.is_empty()))
        })
        .or_else(|| {
            winner_team
                .map(|t| t.name.clone())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "Equipo".to_string());

    let r = slice_rand.clamp(0.0, 1.0);
    let bias = if r < 0.5 {
        (r * 2.0).powf(2.5) * 0.5
    } else {
        1.0 - ((1.0 - r) * 2.0).powf(2.5) * 0.5
    };
    let offset = ((0.1 + bias * 0.8) * 10_000.0).round() / 10_000.0;

    state.pick_order = order.clone();
    state.pick_sequence = generate_draft_pick_sequence(&order, 4);
    state.roulette_winner_team_id = Some(winner_id);
    state.roulette_winner_name = Some(winner_name);
    state.roulette_spinning = true;
    state.roulette_started_at = Some(now_ms);
    state.roulette_slice_offset = Some(offset);
    state.auto_countdown_ends_at = None;

    Ok(())
}

/// Transiciona el estado del lobby al draft una vez culminada la animación de la ruleta.
///
/// Port de lobby.js:489-502.
pub fn start_draft_after_roulette(state: &mut LobbyState) -> bool {
    if !state.draft_started && state.roulette_spinning {
        state.roulette_spinning = false;
        state.roulette_started_at = None;
        state.roulette_slice_offset = None;
        state.draft_started = true;
        state.draft_finished = false;
        state.current_pick_index = 0;
        state.effective_bet_amount = state.agreed_bet_amount.unwrap_or(0.0);
        for team in &mut state.teams {
            team.players.clear();
        }
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DiscordUserDetails, LobbyManager, LobbyTeam, Player};
    use serde_json::json;

    fn make_valid_ready_state() -> LobbyState {
        let mut state = LobbyState::default();
        state.teams = vec![
            LobbyTeam {
                id: "team_1".to_string(),
                name: "Equipo 1".to_string(),
                tag: "EQ1".to_string(),
                captain: Some(Player {
                    discord_id: "cap_1".to_string(),
                    nickname: "CapitanUno".to_string(),
                    faceit_nickname: Some("CapitanUno".to_string()),
                    faceit_level: Some(10),
                    faceit_elo: 2200,
                    ready: true,
                    tier: "S".to_string(),
                    discord_user: Some(DiscordUserDetails {
                        username: "CapitanUno".to_string(),
                        display_name: "Capitan Uno Disp".to_string(),
                        avatar_url: "".to_string(),
                    }),
                    ..Default::default()
                }),
                players: vec![Player {
                    discord_id: "p_1".to_string(),
                    nickname: "Player1".to_string(),
                    ..Default::default()
                }],
            },
            LobbyTeam {
                id: "team_2".to_string(),
                name: "Equipo 2".to_string(),
                tag: "EQ2".to_string(),
                captain: Some(Player {
                    discord_id: "cap_2".to_string(),
                    nickname: "CapitanDos".to_string(),
                    faceit_nickname: Some("CapitanDos".to_string()),
                    faceit_level: Some(8),
                    faceit_elo: 1800,
                    ready: true,
                    tier: "A".to_string(),
                    discord_user: Some(DiscordUserDetails {
                        username: "CapitanDos".to_string(),
                        display_name: "Capitan Dos Disp".to_string(),
                        avatar_url: "".to_string(),
                    }),
                    ..Default::default()
                }),
                players: vec![],
            },
        ];
        state.captain_a_proposal = Some(json!(15.0));
        state.captain_b_proposal = Some(json!(15.0));
        state.captain_a_ready = true;
        state.captain_b_ready = true;
        state.bet_agreed = true;
        state.agreed_bet_amount = Some(15.0);
        state
    }

    #[test]
    fn test_decide_roulette_rejections() {
        // 1. draft_started -> Err
        let mut state = make_valid_ready_state();
        state.draft_started = true;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("El draft ya está iniciado o concluido.")
        );

        // 2. draft_finished -> Err
        let mut state = make_valid_ready_state();
        state.draft_finished = true;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("El draft ya está iniciado o concluido.")
        );

        // 3. Falta capitán en equipo 0 -> Err
        let mut state = make_valid_ready_state();
        state.teams[0].captain = None;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Falta al menos un capitán.")
        );

        // 4. Falta capitán en equipo 1 -> Err
        let mut state = make_valid_ready_state();
        state.teams[1].captain = None;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Falta al menos un capitán.")
        );

        // 5. Menos de 2 equipos -> Err
        let mut state = make_valid_ready_state();
        state.teams.pop();
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Falta al menos un capitán.")
        );

        // 6. !captain_a_ready -> Err
        let mut state = make_valid_ready_state();
        state.captain_a_ready = false;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Los capitanes no están listos.")
        );

        // 7. !captain_b_ready -> Err
        let mut state = make_valid_ready_state();
        state.captain_b_ready = false;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Los capitanes no están listos.")
        );

        // 8. Propuesta A ausente -> Err
        let mut state = make_valid_ready_state();
        state.captain_a_proposal = None;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Los montos de apuesta no coinciden.")
        );

        // 9. Propuesta B ausente -> Err
        let mut state = make_valid_ready_state();
        state.captain_b_proposal = None;
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Los montos de apuesta no coinciden.")
        );

        // 10. Propuestas distintas -> Err
        let mut state = make_valid_ready_state();
        state.captain_b_proposal = Some(json!(20.0));
        assert_eq!(
            decide_roulette(&mut state, 0.2, 0.5, 1000),
            Err("Los montos de apuesta no coinciden.")
        );
    }

    #[test]
    fn test_decide_roulette_coin_branches_and_winner_name() {
        // Rama coin < 0.5 -> Team 1 gana
        let mut state = make_valid_ready_state();
        let res = decide_roulette(&mut state, 0.49, 0.5, 12345);
        assert!(res.is_ok());
        assert_eq!(state.pick_order, vec!["team_1", "team_2"]);
        assert_eq!(state.roulette_winner_team_id.as_deref(), Some("team_1"));
        assert_eq!(
            state.roulette_winner_name.as_deref(),
            Some("Capitan Uno Disp")
        );
        assert!(state.roulette_spinning);
        assert_eq!(state.roulette_started_at, Some(12345));
        assert_eq!(state.auto_countdown_ends_at, None);
        assert_eq!(
            state.pick_sequence,
            generate_draft_pick_sequence(&["team_1".to_string(), "team_2".to_string()], 4)
        );

        // Rama coin >= 0.5 -> Team 2 gana
        let mut state = make_valid_ready_state();
        let res = decide_roulette(&mut state, 0.50, 0.5, 67890);
        assert!(res.is_ok());
        assert_eq!(state.pick_order, vec!["team_2", "team_1"]);
        assert_eq!(state.roulette_winner_team_id.as_deref(), Some("team_2"));
        assert_eq!(
            state.roulette_winner_name.as_deref(),
            Some("Capitan Dos Disp")
        );
        assert!(state.roulette_spinning);
        assert_eq!(state.roulette_started_at, Some(67890));
        assert_eq!(state.auto_countdown_ends_at, None);
        assert_eq!(
            state.pick_sequence,
            generate_draft_pick_sequence(&["team_2".to_string(), "team_1".to_string()], 4)
        );
    }

    #[test]
    fn test_decide_roulette_slice_offset_within_bounds() {
        let test_r_values = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0];
        for &r in &test_r_values {
            let mut state = make_valid_ready_state();
            let res = decide_roulette(&mut state, 0.1, r, 1000);
            assert!(res.is_ok());
            let offset = state.roulette_slice_offset.unwrap();
            assert!(
                (0.1..=0.9).contains(&offset),
                "Offset {} debe estar en [0.1, 0.9] para r={}",
                offset,
                r
            );
        }
    }

    #[test]
    fn test_start_draft_after_roulette_transitions_properly() {
        let mut state = make_valid_ready_state();
        decide_roulette(&mut state, 0.3, 0.5, 1000).unwrap();
        assert!(state.roulette_spinning);
        assert!(!state.teams[0].players.is_empty());

        let changed = start_draft_after_roulette(&mut state);
        assert!(changed);
        assert!(!state.roulette_spinning);
        assert_eq!(state.roulette_started_at, None);
        assert_eq!(state.roulette_slice_offset, None);
        assert!(state.draft_started);
        assert!(!state.draft_finished);
        assert_eq!(state.current_pick_index, 0);
        assert_eq!(state.effective_bet_amount, 15.0);
        assert!(state.teams[0].players.is_empty());

        // Segunda llamada no debe modificar nada y devolver false
        let changed2 = start_draft_after_roulette(&mut state);
        assert!(!changed2);
    }

    #[test]
    fn test_start_draft_after_roulette_noop_when_not_spinning() {
        let mut state = make_valid_ready_state();
        state.roulette_spinning = false;
        state.draft_started = false;
        assert!(!start_draft_after_roulette(&mut state));
    }

    #[test]
    fn test_random_unit_range() {
        for _ in 0..100 {
            let val = random_unit();
            assert!((0.0..1.0).contains(&val));
        }
    }

    #[tokio::test]
    async fn test_lobby_manager_schedule_roulette_flow() {
        let manager = LobbyManager::new("guild_test");
        let initial_state = make_valid_ready_state();
        manager.update_state("guild_test", initial_state).await;

        let timings = RouletteTimings { draft_delay_ms: 10 };
        manager.schedule_roulette("guild_test", 10, timings);

        let mut completed = false;
        for _ in 0..100 {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            let s = manager.get_state("guild_test").await;
            if s.draft_started {
                completed = true;
                break;
            }
        }
        assert!(completed, "El draft no se inició tras esperar la ruleta");

        let state = manager.get_state("guild_test").await;
        assert!(state.draft_started);
        assert!(!state.roulette_spinning);
        assert_eq!(state.current_pick_index, 0);
        assert_eq!(state.effective_bet_amount, 15.0);
        assert!(state.teams[0].players.is_empty());
    }

    #[tokio::test]
    async fn test_lobby_manager_cancel_roulette_prevents_change() {
        let manager = LobbyManager::new("guild_test_cancel");
        let initial_state = make_valid_ready_state();
        manager
            .update_state("guild_test_cancel", initial_state)
            .await;

        let timings = RouletteTimings { draft_delay_ms: 80 };
        manager.schedule_roulette("guild_test_cancel", 80, timings);

        // Cancelar inmediatamente
        manager.cancel_roulette("guild_test_cancel");

        // Esperar tiempo suficiente para asegurar que no se ejecute
        tokio::time::sleep(tokio::time::Duration::from_millis(120)).await;

        let state = manager.get_state("guild_test_cancel").await;
        assert!(!state.draft_started);
        assert!(!state.roulette_spinning);
    }
}
