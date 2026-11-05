// Módulo de lógica pura y funciones auxiliares para los comandos de mixton (/mixton_listo y /mixton_no_listo).

use lobby::draft::player_tier;
use lobby::state::{elo_to_level, LobbyState, Player};
use serde_json::{Map, Value};

/// Fusiona el documento global del usuario (`faceit_users/{discord_id}`) y el documento
/// específico del servidor (`faceit_users/{guild_id}_{discord_id}`).
///
/// Los campos no nulos ni vacíos del documento del servidor complementan o sobrescriben
/// los del documento global.
pub fn merge_user_docs(global: Option<&Value>, server: Option<&Value>) -> Map<String, Value> {
    let mut map = Map::new();

    if let Some(Value::Object(g)) = global {
        for (k, v) in g {
            if !v.is_null() {
                map.insert(k.clone(), v.clone());
            }
        }
    }

    if let Some(Value::Object(s)) = server {
        for (k, v) in s {
            let is_empty_str = match v {
                Value::String(st) => st.trim().is_empty(),
                _ => false,
            };
            if !v.is_null() && !is_empty_str {
                map.insert(k.clone(), v.clone());
            }
        }
    }

    map
}

/// Determina si el usuario está vinculado y verificado en Faceit.
/// Requiere que el campo `faceit_player_id` esté presente y sea un texto no vacío.
pub fn is_user_verified(doc: &Map<String, Value>) -> bool {
    doc.get("faceit_player_id")
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Construye una estructura `Player` a partir del documento fusionado de Firestore y los datos de Discord.
/// Extrae: `faceit_player_id`, `faceit_nickname`, `faceit_elo`, `faceit_level`, `steam_id_64` y `mixton_tier_override`.
/// Calcula el `tier` del jugador usando `lobby::draft::player_tier`.
pub fn build_player_from_doc(
    discord_id: &str,
    display_name: &str,
    avatar_url: Option<String>,
    doc: &Map<String, Value>,
) -> Player {
    let faceit_nick = doc
        .get("faceit_nickname")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let elo = doc
        .get("faceit_elo")
        .and_then(|v| {
            v.as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_i64().filter(|n| *n > 0).map(|n| n as u32))
                .or_else(|| v.as_f64().filter(|n| *n > 0.0).map(|n| n as u32))
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u32>().ok()))
        })
        .unwrap_or(1000);

    let level = doc
        .get("faceit_level")
        .and_then(|v| {
            v.as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_i64().filter(|n| *n > 0).map(|n| n as u32))
                .or_else(|| v.as_f64().filter(|n| *n > 0.0).map(|n| n as u32))
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u32>().ok()))
        })
        .or_else(|| Some(elo_to_level(elo)));

    let steam_id = doc.get("steam_id_64").and_then(|v| {
        v.as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    });

    let tier_override = doc
        .get("mixton_tier_override")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty());

    let mut player = Player {
        discord_id: discord_id.to_string(),
        nickname: display_name.to_string(),
        faceit_nickname: faceit_nick,
        steam_id_64: steam_id,
        avatar: avatar_url,
        faceit_elo: elo,
        ready: true,
        tier: "C".to_string(),
        faceit_level: level,
        mixton_tier_override: tier_override,
        discord_user: None,
    };

    let calculated_tier = player_tier(&player).to_string();
    player.tier = calculated_tier;
    player.ensure_discord_user();

    player
}

/// Decide si corresponde auto-asignar capitanes en el lobby:
/// - El draft no debe haber iniciado ni terminado.
/// - Debe haber al menos 10 jugadores listos en la cola.
/// - Los 2 primeros equipos deben existir y al menos uno de ellos debe carecer de capitán.
pub fn should_auto_assign_captains(state: &LobbyState) -> bool {
    if state.draft_started || state.draft_finished {
        return false;
    }

    let ready_count = state.available_players.iter().filter(|p| p.ready).count();
    if ready_count < 10 {
        return false;
    }

    if state.teams.len() < 2 {
        return false;
    }

    state.teams.iter().take(2).any(|t| t.captain.is_none())
}

/// Limpia el estado de negociación de capitanes y ruleta si un capitán o jugador de equipo sale.
/// Devuelve true si el estado fue modificado.
pub fn clean_lobby_on_player_leave(state: &mut LobbyState, discord_id: &str) -> bool {
    if state.draft_started || state.draft_finished {
        return false;
    }

    let mut modified = false;
    for team in &mut state.teams {
        if let Some(ref cap) = team.captain {
            if cap.discord_id == discord_id {
                team.captain = None;
                modified = true;
            }
        }
        let initial_len = team.players.len();
        team.players.retain(|p| p.discord_id != discord_id);
        if team.players.len() != initial_len {
            modified = true;
        }
    }

    if modified {
        state.captain_a_proposal = None;
        state.captain_b_proposal = None;
        state.captain_a_ready = false;
        state.captain_b_ready = false;
        state.bet_agreed = false;
        state.agreed_bet_amount = None;
        state.auto_countdown_ends_at = None;
        state.roulette_spinning = false;
        state.roulette_winner_team_id = None;
        state.roulette_winner_name = None;
        state.roulette_started_at = None;
        state.roulette_slice_offset = None;
        state.pick_order = state
            .teams
            .iter()
            .filter(|t| t.captain.is_some())
            .map(|t| t.id.clone())
            .collect();
    }

    modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_doc_sin_faceit_player_id_no_verificado() {
        let mut map = Map::new();
        assert!(!is_user_verified(&map));

        map.insert("faceit_player_id".to_string(), json!(""));
        assert!(!is_user_verified(&map));

        map.insert("faceit_player_id".to_string(), json!("   "));
        assert!(!is_user_verified(&map));

        map.insert("faceit_player_id".to_string(), Value::Null);
        assert!(!is_user_verified(&map));

        map.insert("faceit_nickname".to_string(), json!("alguien"));
        assert!(!is_user_verified(&map));

        // Con faceit_player_id válido
        map.insert(
            "faceit_player_id".to_string(),
            json!("f00b4r-1234-5678-90ab"),
        );
        assert!(is_user_verified(&map));
    }

    #[test]
    fn test_doc_global_mas_doc_servidor_fusionados() {
        let global = json!({
            "faceit_player_id": "f-player-1",
            "faceit_nickname": "GlobalNick",
            "faceit_elo": 1550,
            "steam_id_64": "76561198000000001"
        });

        let server = json!({
            "mixton_tier_override": "S",
            "faceit_nickname": "ServerNick",
            "guild_id": "12345"
        });

        let merged = merge_user_docs(Some(&global), Some(&server));

        assert_eq!(merged.get("faceit_player_id"), Some(&json!("f-player-1")));
        // Server sobrescribe nickname no vacío
        assert_eq!(merged.get("faceit_nickname"), Some(&json!("ServerNick")));
        // Conserva elo de global
        assert_eq!(merged.get("faceit_elo"), Some(&json!(1550)));
        // Conserva steam_id_64 de global
        assert_eq!(merged.get("steam_id_64"), Some(&json!("76561198000000001")));
        // Agrega mixton_tier_override de server
        assert_eq!(merged.get("mixton_tier_override"), Some(&json!("S")));
        assert_eq!(merged.get("guild_id"), Some(&json!("12345")));

        // Server con strings vacíos o null no sobreescribe campos válidos de global
        let server_empty = json!({
            "faceit_nickname": "",
            "steam_id_64": Value::Null
        });
        let merged2 = merge_user_docs(Some(&global), Some(&server_empty));
        assert_eq!(merged2.get("faceit_nickname"), Some(&json!("GlobalNick")));
        assert_eq!(
            merged2.get("steam_id_64"),
            Some(&json!("76561198000000001"))
        );

        // Sin global, solo server
        let merged3 = merge_user_docs(None, Some(&server));
        assert_eq!(merged3.get("mixton_tier_override"), Some(&json!("S")));
        assert_eq!(merged3.get("faceit_nickname"), Some(&json!("ServerNick")));
        assert!(merged3.get("faceit_player_id").is_none());
    }

    #[test]
    fn test_player_construido_con_elo_nivel_tier_correctos() {
        // Caso 1: Nivel 10 / Tier S
        let doc_tier_s = json!({
            "faceit_player_id": "p-s",
            "faceit_nickname": "ProPlayer",
            "faceit_elo": 2100,
            "steam_id_64": "76561198000000010"
        })
        .as_object()
        .cloned()
        .unwrap();

        let player_s = build_player_from_doc(
            "discord_1",
            "DiscordName1",
            Some("https://avatar.url/1.png".to_string()),
            &doc_tier_s,
        );
        assert_eq!(player_s.faceit_elo, 2100);
        assert_eq!(player_s.faceit_level, Some(10));
        assert_eq!(player_s.tier, "S");
        assert_eq!(player_s.faceit_nickname, Some("ProPlayer".to_string()));
        assert_eq!(player_s.steam_id_64, Some("76561198000000010".to_string()));

        // Caso 2: Nivel 8 / Tier A
        let doc_tier_a = json!({
            "faceit_player_id": "p-a",
            "faceit_nickname": "HighPlayer",
            "faceit_elo": 1600
        })
        .as_object()
        .cloned()
        .unwrap();

        let player_a = build_player_from_doc("discord_2", "DiscordName2", None, &doc_tier_a);
        assert_eq!(player_a.faceit_elo, 1600);
        assert_eq!(player_a.faceit_level, Some(8));
        assert_eq!(player_a.tier, "A");

        // Caso 3: Nivel 6 / Tier B
        let doc_tier_b = json!({
            "faceit_player_id": "p-b",
            "faceit_nickname": "MidPlayer",
            "faceit_elo": 1250
        })
        .as_object()
        .cloned()
        .unwrap();

        let player_b = build_player_from_doc("discord_3", "DiscordName3", None, &doc_tier_b);
        assert_eq!(player_b.faceit_elo, 1250);
        assert_eq!(player_b.faceit_level, Some(6));
        assert_eq!(player_b.tier, "B");

        // Caso 4: Nivel 3 / Tier C
        let doc_tier_c = json!({
            "faceit_player_id": "p-c",
            "faceit_nickname": "LowPlayer",
            "faceit_elo": 800
        })
        .as_object()
        .cloned()
        .unwrap();

        let player_c = build_player_from_doc("discord_4", "DiscordName4", None, &doc_tier_c);
        assert_eq!(player_c.faceit_elo, 800);
        assert_eq!(player_c.faceit_level, Some(3));
        assert_eq!(player_c.tier, "C");

        // Caso 5: Nivel bajo con override explícito
        let doc_override = json!({
            "faceit_player_id": "p-ov",
            "faceit_nickname": "OverridePlayer",
            "faceit_elo": 500,
            "faceit_level": 1,
            "mixton_tier_override": "a"
        })
        .as_object()
        .cloned()
        .unwrap();

        let player_ov = build_player_from_doc("discord_5", "DiscordName5", None, &doc_override);
        assert_eq!(player_ov.faceit_elo, 500);
        assert_eq!(player_ov.faceit_level, Some(1));
        assert_eq!(player_ov.mixton_tier_override, Some("A".to_string()));
        assert_eq!(player_ov.tier, "A");

        // Caso 6: Sin elo (default 1000)
        let doc_default = json!({
            "faceit_player_id": "p-def"
        })
        .as_object()
        .cloned()
        .unwrap();

        let player_def = build_player_from_doc("discord_6", "DiscordName6", None, &doc_default);
        assert_eq!(player_def.faceit_elo, 1000);
        assert_eq!(player_def.faceit_level, Some(4));
        assert_eq!(player_def.tier, "C");
    }

    #[test]
    fn test_auto_asignar_solo_cuando_mayor_igual_10_listos_y_faltan_capitanes_y_draft_inactivo() {
        let mut state = LobbyState::default();

        // 0 jugadores: no auto-asignar
        assert!(!should_auto_assign_captains(&state));

        // 9 jugadores listos: no auto-asignar (< 10)
        for i in 1..=9 {
            state.available_players.push(Player {
                discord_id: format!("p_{}", i),
                nickname: format!("Nick {}", i),
                faceit_nickname: None,
                steam_id_64: None,
                avatar: None,
                faceit_elo: 1000,
                ready: true,
                tier: "C".to_string(),
                faceit_level: Some(4),
                mixton_tier_override: None,
                discord_user: None,
            });
        }
        assert_eq!(state.available_players.len(), 9);
        assert!(!should_auto_assign_captains(&state));

        // Añadimos el jugador 10 pero NO listo (ready: false): no auto-asignar
        state.available_players.push(Player {
            discord_id: "p_10".to_string(),
            nickname: "Nick 10".to_string(),
            faceit_nickname: None,
            steam_id_64: None,
            avatar: None,
            faceit_elo: 1000,
            ready: false,
            tier: "C".to_string(),
            faceit_level: Some(4),
            mixton_tier_override: None,
            discord_user: None,
        });
        assert!(!should_auto_assign_captains(&state));

        // Jugador 10 se marca como listo -> Ahora son 10 listos y faltan capitanes -> auto-asignar SI
        state.available_players[9].ready = true;
        assert!(should_auto_assign_captains(&state));

        // Si el draft ya comenzó -> no auto-asignar
        state.draft_started = true;
        assert!(!should_auto_assign_captains(&state));
        state.draft_started = false;

        // Si el draft ya terminó -> no auto-asignar
        state.draft_finished = true;
        assert!(!should_auto_assign_captains(&state));
        state.draft_finished = false;

        // Si un equipo ya tiene capitán, pero el otro no: todavía faltan capitanes -> SI
        state.teams[0].captain = Some(state.available_players[0].clone());
        assert!(should_auto_assign_captains(&state));

        // Si ambos equipos ya tienen capitán: no faltan capitanes -> NO
        state.teams[1].captain = Some(state.available_players[1].clone());
        assert!(!should_auto_assign_captains(&state));
    }

    #[test]
    fn test_clean_lobby_on_player_leave() {
        let cap_a = Player {
            discord_id: "cap_a".to_string(),
            nickname: "CapA".to_string(),
            faceit_nickname: None,
            steam_id_64: None,
            avatar: None,
            faceit_elo: 2000,
            ready: true,
            tier: "S".to_string(),
            faceit_level: Some(10),
            mixton_tier_override: None,
            discord_user: None,
        };

        let mut state = LobbyState::default();
        state.teams[0].captain = Some(cap_a.clone());
        state.captain_a_proposal = Some(json!({"bet": 10}));
        state.captain_a_ready = true;
        state.bet_agreed = true;
        state.agreed_bet_amount = Some(10.0);
        state.auto_countdown_ends_at = Some(12345);
        state.roulette_spinning = true;
        state.roulette_started_at = Some(12345);
        state.roulette_slice_offset = Some(0.5);
        state.pick_order = vec!["team_1".to_string()];

        // Si sale un usuario que no era capitán ni jugador:
        let modified = clean_lobby_on_player_leave(&mut state, "random_user");
        assert!(!modified);
        assert!(state.teams[0].captain.is_some());

        // Si sale el capitán A:
        let modified = clean_lobby_on_player_leave(&mut state, "cap_a");
        assert!(modified);
        assert!(state.teams[0].captain.is_none());
        assert_eq!(state.captain_a_proposal, None);
        assert!(!state.captain_a_ready);
        assert!(!state.bet_agreed);
        assert_eq!(state.agreed_bet_amount, None);
        assert_eq!(state.auto_countdown_ends_at, None);
        assert!(!state.roulette_spinning);
        assert!(state.pick_order.is_empty());
    }
}
