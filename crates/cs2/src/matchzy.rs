//! Generación y formateo de configuraciones oficiales de MatchZy (`match.json` y `whitelist.cfg`).

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatchCvars {
    pub mp_maxrounds: String,
    pub mp_overtime_enable: String,
    pub mp_overtime_maxrounds: String,
}

impl Default for MatchCvars {
    fn default() -> Self {
        Self {
            mp_maxrounds: "24".into(),
            mp_overtime_enable: "1".into(),
            mp_overtime_maxrounds: "6".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatchTeam {
    pub name: String,
    pub tag: String,
    pub players: BTreeMap<String, String>, // steam_id_64 -> nick
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatchSpectators {
    pub players: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatchConfig {
    pub matchid: String,
    pub num_maps: u32,
    pub maplist: Vec<String>,
    pub skip_veto: bool,
    pub players_per_team: u32,
    pub min_players_to_ready: u32,
    pub min_spectators_to_ready: u32,
    pub cvars: MatchCvars,
    pub team1: MatchTeam,
    pub team2: MatchTeam,
    pub spectators: MatchSpectators,
}

/// Parámetros de entrada para construir la partida en MatchZy.
#[derive(Debug, Clone)]
pub struct MatchParams {
    pub match_id: String,
    pub chosen_maps: Vec<String>,
    pub team1_name: String,
    pub team1_tag: String,
    pub team1_players: Vec<(String, String)>, // (steam_id_64, nick)
    pub team2_name: String,
    pub team2_tag: String,
    pub team2_players: Vec<(String, String)>,
    pub num_maps: u32,
    pub spectators: Vec<(String, String)>,
}

/// Resultado de generar los archivos para MatchZy.
pub struct GeneratedMatchZy {
    pub config: MatchConfig,
    pub match_json: String,
    pub whitelist_cfg: String,
    pub initial_map: String,
    pub players_count: usize,
}

impl GeneratedMatchZy {
    /// Genera la configuración completa con listas deduplicadas y formato compatible.
    pub fn build(params: MatchParams) -> Self {
        let maps = if params.chosen_maps.is_empty() {
            vec!["de_mirage".to_owned()]
        } else {
            params.chosen_maps
        };
        let initial_map = maps.first().cloned().unwrap_or_else(|| "de_mirage".into());

        let mut t1_players = BTreeMap::new();
        for (s64, nick) in params.team1_players {
            let clean = s64.trim().to_string();
            if clean.len() == 17 && clean.chars().all(|c| c.is_ascii_digit()) {
                t1_players.insert(clean, nick);
            }
        }
        if t1_players.is_empty() {
            t1_players.insert("76561198675781576".into(), params.team1_name.clone());
        }

        let mut t2_players = BTreeMap::new();
        for (s64, nick) in params.team2_players {
            let clean = s64.trim().to_string();
            if clean.len() == 17 && clean.chars().all(|c| c.is_ascii_digit()) {
                t2_players.insert(clean, nick);
            }
        }
        if t2_players.is_empty() {
            t2_players.insert("76561198042296985".into(), params.team2_name.clone());
        }

        // Espectadores: si alguien está jugando, se excluye de espectadores
        let mut specs = BTreeMap::new();
        for (s64, name) in params.spectators {
            let clean = s64.trim().to_string();
            if !t1_players.contains_key(&clean) && !t2_players.contains_key(&clean) {
                specs.insert(clean, name);
            }
        }

        let max_players = t1_players.len().max(t2_players.len()).max(1) as u32;

        let num_clean: String = params.match_id.chars().filter(|c| c.is_ascii_digit()).collect();
        let matchid = if num_clean.is_empty() || num_clean.len() > 9 {
            chrono::Utc::now().timestamp_millis().to_string().chars().rev().take(8).collect::<String>().chars().rev().collect()
        } else {
            num_clean
        };

        let num_maps = match params.num_maps {
            1 | 3 | 5 => params.num_maps,
            _ => 1,
        };

        let config = MatchConfig {
            matchid,
            num_maps,
            maplist: maps,
            skip_veto: true,
            players_per_team: max_players,
            min_players_to_ready: 0,
            min_spectators_to_ready: 0,
            cvars: MatchCvars::default(),
            team1: MatchTeam {
                name: params.team1_name,
                tag: params.team1_tag,
                players: t1_players.clone(),
            },
            team2: MatchTeam {
                name: params.team2_name,
                tag: params.team2_tag,
                players: t2_players.clone(),
            },
            spectators: MatchSpectators {
                players: specs.clone(),
            },
        };

        let mut whitelist_set = BTreeSet::new();
        whitelist_set.extend(t1_players.keys().cloned());
        whitelist_set.extend(t2_players.keys().cloned());
        whitelist_set.extend(specs.keys().cloned());

        let whitelist_cfg = whitelist_set.into_iter().collect::<Vec<_>>().join("\n");
        let match_json = serde_json::to_string_pretty(&config).unwrap_or_default();
        let players_count = t1_players.len() + t2_players.len();

        Self {
            config,
            match_json,
            whitelist_cfg,
            initial_map,
            players_count,
        }
    }

    /// Genera la línea de comando remota para sincronizar los archivos vía SSH en base64.
    pub fn build_remote_sync_command(&self) -> String {
        let b64_match = base64::engine::general_purpose::STANDARD.encode(&self.match_json);
        let b64_white = base64::engine::general_purpose::STANDARD.encode(&self.whitelist_cfg);
        format!(
            "mkdir -p /home/ubuntu/cs2_server/game/csgo/cfg/MatchZy && \
             echo \"{b64_match}\" | base64 -d > /home/ubuntu/cs2_server/game/csgo/cfg/MatchZy/match.json && \
             echo \"{b64_white}\" | base64 -d > /home/ubuntu/cs2_server/game/csgo/cfg/MatchZy/whitelist.cfg"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_matchzy_json_and_whitelist() {
        let params = MatchParams {
            match_id: "match_123456".into(),
            chosen_maps: vec!["de_inferno".into(), "de_mirage".into()],
            team1_name: "Equipo Alfa".into(),
            team1_tag: "ALFA".into(),
            team1_players: vec![
                ("76561198000000001".into(), "Player1".into()),
                ("76561198000000002".into(), "Player2".into()),
            ],
            team2_name: "Equipo Beta".into(),
            team2_tag: "BETA".into(),
            team2_players: vec![
                ("76561198000000003".into(), "Player3".into()),
                ("76561198000000004".into(), "Player4".into()),
            ],
            num_maps: 1,
            spectators: vec![("76561198000000099".into(), "Admin".into())],
        };

        let generated = GeneratedMatchZy::build(params);
        assert_eq!(generated.config.matchid, "123456");
        assert_eq!(generated.initial_map, "de_inferno");
        assert_eq!(generated.players_count, 4);
        assert!(generated.whitelist_cfg.contains("76561198000000001"));
        assert!(generated.whitelist_cfg.contains("76561198000000099"));

        let remote_cmd = generated.build_remote_sync_command();
        assert!(remote_cmd.contains("base64 -d > /home/ubuntu/cs2_server/game/csgo/cfg/MatchZy/match.json"));
    }
}
