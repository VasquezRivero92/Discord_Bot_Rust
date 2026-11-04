//! Lógica pura de draft, selección de capitanes, propuestas de apuestas y cálculo de turnos.
//! Portado fielmente desde la implementación de referencia en Node.js (lobby.js).

use crate::state::{LobbyState, Player};

/// Convierte una propuesta JSON o número a Option<f64>.
pub fn proposal_to_f64(v: &Option<serde_json::Value>) -> Option<f64> {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Genera la secuencia ordenada de turnos de selección (snake draft) para N equipos.
/// Port exacto de `generateDraftPickSequence` (lobby.js:2772).
/// Para 2 equipos y 4 picks por equipo: A, B, B, A, A, B, B, A.
pub fn generate_draft_pick_sequence(team_ids: &[String], players_per_team: usize) -> Vec<String> {
    if team_ids.is_empty() || players_per_team == 0 {
        return Vec::new();
    }
    let mut sequence = Vec::with_capacity(team_ids.len() * players_per_team);
    for round in 0..players_per_team {
        if round % 2 == 0 {
            // Ronda par: de izquierda a derecha (Team 0 -> Team N-1)
            for id in team_ids {
                sequence.push(id.clone());
            }
        } else {
            // Ronda impar: de derecha a izquierda (Team N-1 -> Team 0)
            for id in team_ids.iter().rev() {
                sequence.push(id.clone());
            }
        }
    }
    sequence
}

/// Determina el tier de un jugador según su override o nivel de Faceit.
/// Port de lobby.js:715-731.
/// Override S/A/B/C si existe; si no, nivel 10 -> S, 8-9 -> A, 5-7 -> B, resto C.
pub fn player_tier(p: &Player) -> char {
    if let Some(ref ov) = p.mixton_tier_override {
        let trimmed = ov.trim();
        if trimmed.eq_ignore_ascii_case("s") {
            return 'S';
        } else if trimmed.eq_ignore_ascii_case("a") {
            return 'A';
        } else if trimmed.eq_ignore_ascii_case("b") {
            return 'B';
        } else if trimmed.eq_ignore_ascii_case("c") {
            return 'C';
        }
    }

    let lvl = p.faceit_level.unwrap_or(1);
    if lvl >= 10 {
        'S'
    } else if lvl >= 8 {
        'A'
    } else if lvl >= 5 {
        'B'
    } else {
        'C'
    }
}

/// Valida si un jugador es elegible para ser capitán (solo Tier S o Tier A).
/// Port de lobby.js:2448-2456.
pub fn is_captain_eligible(p: &Player) -> bool {
    if let Some(ref ov) = p.mixton_tier_override {
        let trimmed = ov.trim();
        if !trimmed.is_empty() {
            return trimmed.eq_ignore_ascii_case("s") || trimmed.eq_ignore_ascii_case("a");
        }
    }

    let lvl = p.faceit_level.unwrap_or(1);
    lvl >= 8
}

fn tier_weight(t: char) -> u8 {
    match t {
        'S' => 4,
        'A' => 3,
        'B' => 2,
        _ => 1,
    }
}

/// Auto-asigna capitanes a los equipos que carezcan de uno a partir de la cola de listos.
/// Port de `autoAssignCaptainsHelper` (lobby.js:522-624 y lobby.js:2686-2691).
///
/// Requiere al menos 2 jugadores listos y disponibles. Ordena por Tier (S > A > B > C) -> ELO desc -> Nivel desc.
/// No pisa capitanes existentes. Quita al elegido de la lista de jugadores de todos los equipos.
/// Recalcula pick_order y pick_sequence. Resetea propuestas, acuerdos, listos y ruleta.
pub fn auto_assign_captains(state: &mut LobbyState, ready: Vec<Player>) -> Result<usize, String> {
    // Si todos los equipos configurados ya tienen capitán, terminar sin cambios
    if !state.teams.is_empty() && state.teams.iter().all(|t| t.captain.is_some()) {
        return Ok(0);
    }

    if ready.len() < 2 {
        return Err(
            "No hay suficientes jugadores listos en la cola para asignar capitanes (mínimo 2 disponibles)."
                .to_string(),
        );
    }

    // Identificar discord IDs ocupados en cualquier equipo
    let mut occupied_ids = std::collections::HashSet::new();
    for t in &state.teams {
        if let Some(ref cap) = t.captain {
            occupied_ids.insert(cap.discord_id.clone());
        }
        for pl in &t.players {
            occupied_ids.insert(pl.discord_id.clone());
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut pool: Vec<Player> = ready
        .into_iter()
        .filter(|p| !occupied_ids.contains(&p.discord_id) && seen.insert(p.discord_id.clone()))
        .collect();

    if pool.is_empty() {
        return Err(
            "No hay suficientes jugadores listos en la cola para asignar capitanes (mínimo 2 disponibles)."
                .to_string(),
        );
    }

    // Ordenar pool: Tier (S > A > B > C) -> ELO desc -> Level desc
    pool.sort_by(|a, b| {
        let tier_a = tier_weight(player_tier(a));
        let tier_b = tier_weight(player_tier(b));
        tier_b
            .cmp(&tier_a)
            .then_with(|| b.faceit_elo.cmp(&a.faceit_elo))
            .then_with(|| {
                b.faceit_level
                    .unwrap_or(1)
                    .cmp(&a.faceit_level.unwrap_or(1))
            })
    });

    let mut assigned_count = 0;
    for team in &mut state.teams {
        if team.captain.is_none() && assigned_count < pool.len() {
            let mut candidate = pool[assigned_count].clone();
            candidate.ensure_discord_user();
            team.captain = Some(candidate);
            assigned_count += 1;
        }
    }

    if assigned_count > 0 {
        // Quitar de jugadores en todos los equipos a los elegidos
        let newly_assigned_ids: Vec<String> = state
            .teams
            .iter()
            .filter_map(|t| t.captain.as_ref().map(|c| c.discord_id.clone()))
            .collect();

        for t in &mut state.teams {
            t.players
                .retain(|p| !newly_assigned_ids.contains(&p.discord_id));
        }
        state
            .available_players
            .retain(|p| !newly_assigned_ids.contains(&p.discord_id));

        // Sincronizar pick_order y pick_sequence
        let current_cap_team_ids: Vec<String> = state
            .teams
            .iter()
            .filter(|t| t.captain.is_some())
            .map(|t| t.id.clone())
            .collect();
        state.pick_order = current_cap_team_ids;
        state.pick_sequence = generate_draft_pick_sequence(&state.pick_order, 4);

        // Resetear propuestas, acuerdos, listos y ruleta
        state.captain_a_proposal = None;
        state.captain_b_proposal = None;
        state.captain_a_ready = false;
        state.captain_b_ready = false;
        state.bet_agreed = false;
        state.agreed_bet_amount = None;
        state.auto_countdown_ends_at = None;
        state.roulette_winner_team_id = None;
        state.roulette_winner_name = None;
        state.roulette_spinning = false;
        state.roulette_started_at = None;
        state.roulette_slice_offset = None;

        Ok(assigned_count)
    } else {
        Err(
            "No hay suficientes jugadores listos en la cola para asignar capitanes (mínimo 2 disponibles)."
                .to_string(),
        )
    }
}

/// Asigna o remueve un capitán de un equipo.
/// Port de lobby.js:2430-2530.
///
/// - 404 "Equipo no encontrado." si no existe el equipo.
/// - Si player es None, remueve el capitán sin devolverlo a disponibles.
/// - Valida que el jugador sea Tier S o A (si no, 400 "Los capitanes solo pueden ser de Tier S o Tier A.").
/// - Quita al jugador de las listas de jugadores de todos los equipos y de capitán de otros equipos.
/// - Resetea propuestas y listos según el índice del equipo afectado.
/// - Sincroniza pick_order preservando el orden previo y pick_sequence si !draft_started.
pub fn assign_captain(
    state: &mut LobbyState,
    team_id: &str,
    player: Option<Player>,
) -> Result<(), (u16, String)> {
    let team_pos = state
        .teams
        .iter()
        .position(|t| t.id == team_id)
        .ok_or_else(|| (404, "Equipo no encontrado.".to_string()))?;

    if let Some(mut p) = player {
        if !is_captain_eligible(&p) {
            return Err((
                400,
                "Los capitanes solo pueden ser de Tier S o Tier A.".to_string(),
            ));
        }

        p.ensure_discord_user();
        let target_discord_id = p.discord_id.clone();

        state.teams[team_pos].captain = Some(p);

        // Quitar de jugadores en todos los equipos
        for t in &mut state.teams {
            t.players.retain(|pl| pl.discord_id != target_discord_id);
        }

        // Quitar de capitán de otros equipos si existía
        for (idx, t) in state.teams.iter_mut().enumerate() {
            if idx != team_pos {
                if let Some(ref cap) = t.captain {
                    if cap.discord_id == target_discord_id {
                        t.captain = None;
                    }
                }
            }
        }

        // Quitar de disponibles si estuviera presente
        state
            .available_players
            .retain(|pl| pl.discord_id != target_discord_id);
    } else {
        // Remover capitán sin devolverlo a disponibles (conforme al comportamiento del legado)
        state.teams[team_pos].captain = None;
    }

    // Resetear acuerdos y listos del capitán modificado o removido
    if team_pos == 0
        || state
            .teams
            .first()
            .and_then(|t| t.captain.as_ref())
            .is_none()
    {
        state.captain_a_proposal = None;
        state.captain_a_ready = false;
    }
    if team_pos == 1
        || state
            .teams
            .get(1)
            .and_then(|t| t.captain.as_ref())
            .is_none()
    {
        state.captain_b_proposal = None;
        state.captain_b_ready = false;
    }

    state.bet_agreed = false;
    state.agreed_bet_amount = None;
    state.auto_countdown_ends_at = None;
    state.roulette_winner_team_id = None;
    state.roulette_winner_name = None;
    state.roulette_spinning = false;
    state.roulette_started_at = None;
    state.roulette_slice_offset = None;

    // Sincronizar pick_order preservando el orden previo
    let current_cap_team_ids: Vec<String> = state
        .teams
        .iter()
        .filter(|t| t.captain.is_some())
        .map(|t| t.id.clone())
        .collect();

    let mut new_pick_order: Vec<String> = state
        .pick_order
        .iter()
        .filter(|id| current_cap_team_ids.contains(id))
        .cloned()
        .collect();

    for id in &current_cap_team_ids {
        if !new_pick_order.contains(id) {
            new_pick_order.push(id.clone());
        }
    }
    state.pick_order = new_pick_order;

    if !state.draft_started {
        state.pick_sequence = generate_draft_pick_sequence(&state.pick_order, 4);
    }

    Ok(())
}

/// Establece la propuesta de monto de apuesta de un capitán o administrador.
/// Port de lobby.js:2533-2596.
///
/// - Valida que existan al menos 2 equipos con capitanes.
/// - Valida pertenencia al equipo correspondiente o rol admin.
/// - Ajusta parsed = max(0, bet).
/// - Verifica acuerdo si ambas propuestas existen y coinciden numéricamente.
/// - Cancela auto_countdown_ends_at.
pub fn set_captain_proposal(
    state: &mut LobbyState,
    team_id: &str,
    actor_id: &str,
    is_admin: bool,
    bet: f64,
) -> Result<(), (u16, String)> {
    if state.teams.len() < 2 || state.teams[0].captain.is_none() || state.teams[1].captain.is_none()
    {
        return Err((
            400,
            "Se requieren al menos 2 equipos con capitanes asignados.".to_string(),
        ));
    }

    let team_a = &state.teams[0];
    let team_b = &state.teams[1];

    let is_captain_a = team_a
        .captain
        .as_ref()
        .map_or(false, |c| c.discord_id == actor_id);
    let is_captain_b = team_b
        .captain
        .as_ref()
        .map_or(false, |c| c.discord_id == actor_id);

    let is_target_a = team_id == team_a.id;
    let is_target_b = team_id == team_b.id;

    if !is_target_a && !is_target_b {
        return Err((400, "Equipo no válido para proponer apuesta.".to_string()));
    }

    if is_target_a && !is_captain_a && !is_admin {
        return Err((
            403,
            format!(
                "Solo el capitán de {} o un administrador pueden marcar este monto.",
                team_a.name
            ),
        ));
    }

    if is_target_b && !is_captain_b && !is_admin {
        return Err((
            403,
            format!(
                "Solo el capitán de {} o un administrador pueden marcar este monto.",
                team_b.name
            ),
        ));
    }

    let parsed = if bet.is_nan() || bet < 0.0 { 0.0 } else { bet };

    if is_target_a {
        state.captain_a_proposal = Some(serde_json::json!(parsed));
        state.captain_a_ready = false;
    }
    if is_target_b {
        state.captain_b_proposal = Some(serde_json::json!(parsed));
        state.captain_b_ready = false;
    }

    let prop_a = proposal_to_f64(&state.captain_a_proposal);
    let prop_b = proposal_to_f64(&state.captain_b_proposal);

    if let (Some(a), Some(b)) = (prop_a, prop_b) {
        // Igualdad exacta, como `Number(a) === Number(b)` del legado.
        if a == b {
            state.bet_agreed = true;
            state.agreed_bet_amount = Some(a);
        } else {
            state.bet_agreed = false;
            state.agreed_bet_amount = None;
        }
    } else {
        state.bet_agreed = false;
        state.agreed_bet_amount = None;
    }

    state.auto_countdown_ends_at = None;

    Ok(())
}

/// Actualiza el estado listo de un capitán y verifica acuerdo de apuesta.
/// Port de lobby.js:2599-2683.
///
/// - Rechaza is_ready=true si no hay acuerdo de montos con el mensaje exacto del legado.
/// - Si ambos capitanes están listos y la apuesta está acordada, fija auto_countdown_ends_at = ahora + 7000 ms y devuelve true.
/// - En cualquier otro caso limpia auto_countdown_ends_at y devuelve false.
pub fn set_captain_ready(
    state: &mut LobbyState,
    team_id: &str,
    actor_id: &str,
    is_admin: bool,
    is_ready: bool,
    bet: Option<f64>,
) -> Result<bool, (u16, String)> {
    if state.teams.len() < 2 || state.teams[0].captain.is_none() || state.teams[1].captain.is_none()
    {
        return Err((
            400,
            "Se requieren 2 equipos con capitanes asignados.".to_string(),
        ));
    }

    let team_a = &state.teams[0];
    let team_b = &state.teams[1];

    let is_captain_a = team_a
        .captain
        .as_ref()
        .map_or(false, |c| c.discord_id == actor_id);
    let is_captain_b = team_b
        .captain
        .as_ref()
        .map_or(false, |c| c.discord_id == actor_id);

    let is_target_a = team_id == team_a.id;
    let is_target_b = team_id == team_b.id;

    if !is_target_a && !is_target_b {
        return Err((400, "Equipo no válido para confirmar estado.".to_string()));
    }

    if is_target_a && !is_captain_a && !is_admin {
        return Err((
            403,
            format!(
                "Solo el capitán de {} o un administrador pueden confirmar este estado.",
                team_a.name
            ),
        ));
    }

    if is_target_b && !is_captain_b && !is_admin {
        return Err((
            403,
            format!(
                "Solo el capitán de {} o un administrador pueden confirmar este estado.",
                team_b.name
            ),
        ));
    }

    if let Some(b) = bet {
        let parsed = if b.is_nan() || b < 0.0 { 0.0 } else { b };
        if is_target_a {
            state.captain_a_proposal = Some(serde_json::json!(parsed));
        }
        if is_target_b {
            state.captain_b_proposal = Some(serde_json::json!(parsed));
        }
    }

    let prop_a = proposal_to_f64(&state.captain_a_proposal);
    let prop_b = proposal_to_f64(&state.captain_b_proposal);
    let amounts_match = match (prop_a, prop_b) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };

    if is_ready && !amounts_match {
        let val_a = prop_a
            .map(|v| format!("{:.2}", v))
            .unwrap_or_else(|| "--".to_string());
        let val_b = prop_b
            .map(|v| format!("{:.2}", v))
            .unwrap_or_else(|| "--".to_string());
        return Err((
            400,
            format!(
                "Los capitanes deben marcar el mismo monto de apuesta para acordar y confirmar. (Equipo 1: S/ {} vs Equipo 2: S/ {})",
                val_a, val_b
            ),
        ));
    }

    if is_target_a {
        state.captain_a_ready = is_ready;
    }
    if is_target_b {
        state.captain_b_ready = is_ready;
    }

    if amounts_match {
        state.bet_agreed = true;
        state.agreed_bet_amount = prop_a;
    } else {
        state.bet_agreed = false;
        state.agreed_bet_amount = None;
    }

    let both_ready = state.captain_a_ready && state.captain_b_ready;
    if both_ready && state.bet_agreed {
        let now_ms = chrono::Utc::now().timestamp_millis();
        state.auto_countdown_ends_at = Some(now_ms + 7000);
        Ok(true)
    } else {
        state.auto_countdown_ends_at = None;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LobbyTeam;

    fn make_player(id: &str, elo: u32, level: Option<u32>, tier_override: Option<&str>) -> Player {
        Player {
            discord_id: id.to_string(),
            nickname: format!("Nick_{id}"),
            faceit_nickname: Some(format!("Faceit_{id}")),
            steam_id_64: None,
            avatar: None,
            faceit_elo: elo,
            faceit_level: level,
            ready: true,
            tier: "C".to_string(),
            mixton_tier_override: tier_override.map(|s| s.to_string()),
            discord_user: None,
        }
    }

    fn sample_state() -> LobbyState {
        LobbyState {
            teams: vec![
                LobbyTeam {
                    id: "team_1".into(),
                    name: "Equipo 1".into(),
                    tag: "EQ1".into(),
                    captain: None,
                    players: Vec::new(),
                },
                LobbyTeam {
                    id: "team_2".into(),
                    name: "Equipo 2".into(),
                    tag: "EQ2".into(),
                    captain: None,
                    players: Vec::new(),
                },
            ],
            ..Default::default()
        }
    }

    // 1. generate_draft_pick_sequence: vacíos y casos base
    #[test]
    fn test_pick_sequence_empty_teams() {
        let seq = generate_draft_pick_sequence(&[], 4);
        assert!(seq.is_empty());
    }

    #[test]
    fn test_pick_sequence_zero_picks() {
        let teams = vec!["A".to_string(), "B".to_string()];
        let seq = generate_draft_pick_sequence(&teams, 0);
        assert!(seq.is_empty());
    }

    #[test]
    fn test_pick_sequence_single_team() {
        let teams = vec!["A".to_string()];
        let seq = generate_draft_pick_sequence(&teams, 3);
        assert_eq!(seq, vec!["A", "A", "A"]);
    }

    #[test]
    fn test_pick_sequence_two_teams_four_picks_snake() {
        let teams = vec!["A".to_string(), "B".to_string()];
        let seq = generate_draft_pick_sequence(&teams, 4);
        // Ronda 0 (par): A, B
        // Ronda 1 (impar): B, A
        // Ronda 2 (par): A, B
        // Ronda 3 (impar): B, A
        assert_eq!(seq, vec!["A", "B", "B", "A", "A", "B", "B", "A"]);
    }

    #[test]
    fn test_pick_sequence_three_teams_two_picks() {
        let teams = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let seq = generate_draft_pick_sequence(&teams, 2);
        assert_eq!(seq, vec!["A", "B", "C", "C", "B", "A"]);
    }

    // 2. player_tier: overrides y niveles
    #[test]
    fn test_player_tier_override_valid() {
        assert_eq!(
            player_tier(&make_player("1", 1000, Some(1), Some("S"))),
            'S'
        );
        assert_eq!(
            player_tier(&make_player("2", 1000, Some(1), Some("a"))),
            'A'
        );
        assert_eq!(
            player_tier(&make_player("3", 2500, Some(10), Some("B"))),
            'B'
        );
        assert_eq!(
            player_tier(&make_player("4", 2500, Some(10), Some("C"))),
            'C'
        );
    }

    #[test]
    fn test_player_tier_override_invalid_falls_back_to_level() {
        // Override desconocido debe ignorarse y evaluar por nivel
        let p = make_player("1", 2200, Some(10), Some("X"));
        assert_eq!(player_tier(&p), 'S');
    }

    #[test]
    fn test_player_tier_by_levels() {
        assert_eq!(player_tier(&make_player("1", 2100, Some(10), None)), 'S');
        assert_eq!(player_tier(&make_player("2", 1800, Some(9), None)), 'A');
        assert_eq!(player_tier(&make_player("3", 1600, Some(8), None)), 'A');
        assert_eq!(player_tier(&make_player("4", 1400, Some(7), None)), 'B');
        assert_eq!(player_tier(&make_player("5", 1300, Some(6), None)), 'B');
        assert_eq!(player_tier(&make_player("6", 1100, Some(5), None)), 'B');
        assert_eq!(player_tier(&make_player("7", 950, Some(4), None)), 'C');
        assert_eq!(player_tier(&make_player("8", 500, Some(1), None)), 'C');
        assert_eq!(player_tier(&make_player("9", 500, None, None)), 'C');
    }

    // 3. is_captain_eligible: reglas de capitanía
    #[test]
    fn test_is_captain_eligible_with_override() {
        assert!(is_captain_eligible(&make_player(
            "1",
            800,
            Some(1),
            Some("S")
        )));
        assert!(is_captain_eligible(&make_player(
            "2",
            800,
            Some(1),
            Some("A")
        )));
        assert!(!is_captain_eligible(&make_player(
            "3",
            2500,
            Some(10),
            Some("B")
        )));
        assert!(!is_captain_eligible(&make_player(
            "4",
            2500,
            Some(10),
            Some("C")
        )));
    }

    #[test]
    fn test_is_captain_eligible_by_level() {
        assert!(is_captain_eligible(&make_player("1", 2200, Some(10), None)));
        assert!(is_captain_eligible(&make_player("2", 1800, Some(9), None)));
        assert!(is_captain_eligible(&make_player("3", 1600, Some(8), None)));
        assert!(!is_captain_eligible(&make_player("4", 1400, Some(7), None)));
        assert!(!is_captain_eligible(&make_player("5", 500, Some(1), None)));
        assert!(!is_captain_eligible(&make_player("6", 500, None, None)));
    }

    // 4. auto_assign_captains
    #[test]
    fn test_auto_assign_captains_err_insufficient_ready() {
        let mut state = sample_state();
        let ready = vec![make_player("1", 2000, Some(10), None)];
        let res = auto_assign_captains(&mut state, ready);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            "No hay suficientes jugadores listos en la cola para asignar capitanes (mínimo 2 disponibles)."
        );
    }

    #[test]
    fn test_auto_assign_captains_already_configured() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));
        let ready = vec![
            make_player("3", 2000, Some(10), None),
            make_player("4", 2000, Some(10), None),
        ];
        let res = auto_assign_captains(&mut state, ready);
        assert_eq!(res.unwrap(), 0);
    }

    #[test]
    fn test_auto_assign_captains_pool_occupied() {
        let mut state = sample_state();
        state.teams[0]
            .players
            .push(make_player("p1", 1000, Some(1), None));
        state.teams[1]
            .players
            .push(make_player("p2", 1000, Some(1), None));
        // Los listos ya están ocupados en los equipos
        let ready = vec![
            make_player("p1", 1000, Some(1), None),
            make_player("p2", 1000, Some(1), None),
        ];
        let res = auto_assign_captains(&mut state, ready);
        assert!(res.is_err());
    }

    #[test]
    fn test_auto_assign_captains_tier_elo_level_order() {
        let mut state = sample_state();
        let ready = vec![
            make_player("p_c", 2500, Some(1), Some("C")), // ELO alto pero tier C
            make_player("p_a", 1600, Some(8), None),      // Tier A
            make_player("p_s_low", 2000, Some(10), None), // Tier S, ELO 2000
            make_player("p_s_high", 2400, Some(10), None), // Tier S, ELO 2400
        ];
        let res = auto_assign_captains(&mut state, ready);
        assert_eq!(res.unwrap(), 2);
        // Team 1 debe recibir al mejor: Tier S con mayor ELO
        assert_eq!(
            state.teams[0].captain.as_ref().unwrap().discord_id,
            "p_s_high"
        );
        // Team 2 debe recibir al segundo mejor: Tier S con menor ELO
        assert_eq!(
            state.teams[1].captain.as_ref().unwrap().discord_id,
            "p_s_low"
        );
        // Pick order y sequence recalculados
        assert_eq!(state.pick_order, vec!["team_1", "team_2"]);
        assert_eq!(state.pick_sequence.len(), 8);
        assert_eq!(state.bet_agreed, false);
    }

    #[test]
    fn test_auto_assign_captains_removes_from_team_players() {
        let mut state = sample_state();
        // El jugador estaba como jugador regular
        state.teams[1]
            .players
            .push(make_player("p1", 2000, Some(10), None));
        let ready = vec![
            make_player("p1", 2000, Some(10), None),
            make_player("p2", 1800, Some(9), None),
        ];
        // Forzamos que p1 sea considerado en ready quitándolo primero del filtro manual si no estuviera ocupado
        // Como el pool filtra occupied, vaciamos team players para ver la limpieza
        state.teams[1].players.clear();
        state.teams[1]
            .players
            .push(make_player("dummy", 1000, Some(1), None));
        let res = auto_assign_captains(&mut state, ready);
        assert!(res.is_ok());
        assert_eq!(state.teams[0].captain.as_ref().unwrap().discord_id, "p1");
        assert_eq!(state.teams[1].captain.as_ref().unwrap().discord_id, "p2");
    }

    // 5. assign_captain
    #[test]
    fn test_assign_captain_err_team_not_found() {
        let mut state = sample_state();
        let res = assign_captain(&mut state, "team_inexistente", None);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (404, "Equipo no encontrado.".to_string())
        );
    }

    #[test]
    fn test_assign_captain_remove_captain() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.captain_a_ready = true;
        state.captain_a_proposal = Some(serde_json::json!(50.0));

        let res = assign_captain(&mut state, "team_1", None);
        assert!(res.is_ok());
        assert!(state.teams[0].captain.is_none());
        assert_eq!(state.captain_a_ready, false);
        assert!(state.captain_a_proposal.is_none());
        // NO debe devolverlo a disponibles
        assert!(state.available_players.is_empty());
    }

    #[test]
    fn test_assign_captain_err_ineligible_tier() {
        let mut state = sample_state();
        let p = make_player("p_c", 1000, Some(4), None);
        let res = assign_captain(&mut state, "team_1", Some(p));
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                400,
                "Los capitanes solo pueden ser de Tier S o Tier A.".to_string()
            )
        );
    }

    #[test]
    fn test_assign_captain_success_and_cleanup() {
        let mut state = sample_state();
        let cap_a = make_player("cap_a", 2100, Some(10), None);
        // El jugador figuraba en players de team 2 y en disponibles
        state.teams[1].players.push(cap_a.clone());
        state.available_players.push(cap_a.clone());

        let res = assign_captain(&mut state, "team_1", Some(cap_a));
        assert!(res.is_ok());
        assert_eq!(state.teams[0].captain.as_ref().unwrap().discord_id, "cap_a");
        // Limpiado de players y de disponibles
        assert!(state.teams[1].players.is_empty());
        assert!(state.available_players.is_empty());
        // Sincronizó pick order
        assert_eq!(state.pick_order, vec!["team_1"]);
    }

    #[test]
    fn test_assign_captain_switches_from_other_team_captain() {
        let mut state = sample_state();
        let cap = make_player("cap_shared", 2100, Some(10), None);
        state.teams[0].captain = Some(cap.clone());

        let res = assign_captain(&mut state, "team_2", Some(cap));
        assert!(res.is_ok());
        assert!(state.teams[0].captain.is_none());
        assert_eq!(
            state.teams[1].captain.as_ref().unwrap().discord_id,
            "cap_shared"
        );
    }

    #[test]
    fn test_assign_captain_preserves_existing_pick_order() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.pick_order = vec!["team_1".to_string()];

        let cap2 = make_player("c2", 2000, Some(10), None);
        let res = assign_captain(&mut state, "team_2", Some(cap2));
        assert!(res.is_ok());
        assert_eq!(state.pick_order, vec!["team_1", "team_2"]);
    }

    // 6. set_captain_proposal
    #[test]
    fn test_set_captain_proposal_err_captains_missing() {
        let mut state = sample_state();
        let res = set_captain_proposal(&mut state, "team_1", "admin1", true, 20.0);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                400,
                "Se requieren al menos 2 equipos con capitanes asignados.".to_string()
            )
        );
    }

    #[test]
    fn test_set_captain_proposal_err_invalid_team() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        let res = set_captain_proposal(&mut state, "team_x", "c1", false, 20.0);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (400, "Equipo no válido para proponer apuesta.".to_string())
        );
    }

    #[test]
    fn test_set_captain_proposal_err_forbidden_actor() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        // Intruso intentando proponer para team 1
        let res = set_captain_proposal(&mut state, "team_1", "stranger", false, 25.0);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                403,
                "Solo el capitán de Equipo 1 o un administrador pueden marcar este monto."
                    .to_string()
            )
        );
    }

    #[test]
    fn test_set_captain_proposal_negative_clamped_to_zero() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        let res = set_captain_proposal(&mut state, "team_1", "c1", false, -15.5);
        assert!(res.is_ok());
        assert_eq!(proposal_to_f64(&state.captain_a_proposal), Some(0.0));
    }

    #[test]
    fn test_set_captain_proposal_admin_override_and_agreement() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        // Admin propone 50 para ambos
        assert!(set_captain_proposal(&mut state, "team_1", "admin_user", true, 50.0).is_ok());
        assert_eq!(state.bet_agreed, false);

        assert!(set_captain_proposal(&mut state, "team_2", "admin_user", true, 50.0).is_ok());
        assert_eq!(state.bet_agreed, true);
        assert_eq!(state.agreed_bet_amount, Some(50.0));
    }

    #[test]
    fn test_set_captain_proposal_differing_amounts_no_agreement() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        assert!(set_captain_proposal(&mut state, "team_1", "c1", false, 30.0).is_ok());
        assert!(set_captain_proposal(&mut state, "team_2", "c2", false, 40.0).is_ok());
        assert_eq!(state.bet_agreed, false);
        assert_eq!(state.agreed_bet_amount, None);
    }

    // 7. set_captain_ready
    #[test]
    fn test_set_captain_ready_err_captains_missing() {
        let mut state = sample_state();
        let res = set_captain_ready(&mut state, "team_1", "admin1", true, true, None);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                400,
                "Se requieren 2 equipos con capitanes asignados.".to_string()
            )
        );
    }

    #[test]
    fn test_set_captain_ready_err_invalid_team() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        let res = set_captain_ready(&mut state, "team_99", "c1", false, true, None);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (400, "Equipo no válido para confirmar estado.".to_string())
        );
    }

    #[test]
    fn test_set_captain_ready_err_forbidden() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        let res = set_captain_ready(&mut state, "team_2", "imposter", false, true, None);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                403,
                "Solo el capitán de Equipo 2 o un administrador pueden confirmar este estado."
                    .to_string()
            )
        );
    }

    #[test]
    fn test_set_captain_ready_err_without_agreement_formatting() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        // Uno propone 10, el otro nada
        state.captain_a_proposal = Some(serde_json::json!(10.0));
        state.captain_b_proposal = None;

        let res = set_captain_ready(&mut state, "team_1", "c1", false, true, None);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                400,
                "Los capitanes deben marcar el mismo monto de apuesta para acordar y confirmar. (Equipo 1: S/ 10.00 vs Equipo 2: S/ --)".to_string()
            )
        );
    }

    #[test]
    fn test_set_captain_ready_err_differing_amounts() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));

        state.captain_a_proposal = Some(serde_json::json!(15.0));
        state.captain_b_proposal = Some(serde_json::json!(20.0));

        let res = set_captain_ready(&mut state, "team_2", "c2", false, true, None);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap(),
            (
                400,
                "Los capitanes deben marcar el mismo monto de apuesta para acordar y confirmar. (Equipo 1: S/ 15.00 vs Equipo 2: S/ 20.00)".to_string()
            )
        );
    }

    #[test]
    fn test_set_captain_ready_unready_allowed_without_agreement() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));
        state.captain_a_ready = true;

        // Desmarcar listo (is_ready=false) debe permitirse sin error
        let res = set_captain_ready(&mut state, "team_1", "c1", false, false, None);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), false);
        assert_eq!(state.captain_a_ready, false);
        assert!(state.auto_countdown_ends_at.is_none());
    }

    #[test]
    fn test_set_captain_ready_with_bet_param_updates_proposal() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));
        state.captain_b_proposal = Some(serde_json::json!(25.0));

        // c1 se pone listo pasando su monto coincidente
        let res = set_captain_ready(&mut state, "team_1", "c1", false, true, Some(25.0));
        assert!(res.is_ok());
        assert_eq!(proposal_to_f64(&state.captain_a_proposal), Some(25.0));
        assert_eq!(state.captain_a_ready, true);
        // Sólo 1 está listo, no inicia cuenta regresiva
        assert_eq!(res.unwrap(), false);
    }

    #[test]
    fn test_set_captain_ready_both_ready_triggers_countdown() {
        let mut state = sample_state();
        state.teams[0].captain = Some(make_player("c1", 2000, Some(10), None));
        state.teams[1].captain = Some(make_player("c2", 2000, Some(10), None));
        state.captain_a_proposal = Some(serde_json::json!(100.0));
        state.captain_b_proposal = Some(serde_json::json!(100.0));

        let res_a = set_captain_ready(&mut state, "team_1", "c1", false, true, None);
        assert_eq!(res_a.unwrap(), false);
        assert!(state.auto_countdown_ends_at.is_none());

        let res_b = set_captain_ready(&mut state, "team_2", "c2", false, true, None);
        assert_eq!(res_b.unwrap(), true);
        assert!(state.auto_countdown_ends_at.is_some());
        let ends_at = state.auto_countdown_ends_at.unwrap();
        let now_ms = chrono::Utc::now().timestamp_millis();
        // Debe ser aproximadamente ahora + 7000ms
        assert!(ends_at >= now_ms + 6000 && ends_at <= now_ms + 8000);
    }
}
