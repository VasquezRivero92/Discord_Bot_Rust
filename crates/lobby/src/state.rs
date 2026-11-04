//! Estado en memoria RAM y gestión en tiempo real de las salas de Mixton.

use crate::maps::{default_catalog, MapInfo};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DiscordUserDetails {
    pub username: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "avatarURL")]
    pub avatar_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Player {
    #[serde(rename = "discordId")]
    pub discord_id: String,
    pub nickname: String,
    #[serde(rename = "faceit_nickname", default)]
    pub faceit_nickname: Option<String>,
    #[serde(rename = "steam_id_64", default)]
    pub steam_id_64: Option<String>,
    pub avatar: Option<String>,
    #[serde(rename = "faceit_elo", default = "default_elo")]
    pub faceit_elo: u32,
    #[serde(rename = "faceit_level", default)]
    pub faceit_level: Option<u32>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(rename = "mixton_tier_override", default)]
    pub mixton_tier_override: Option<String>,
    #[serde(rename = "discordUser", default)]
    pub discord_user: Option<DiscordUserDetails>,
}

fn default_elo() -> u32 {
    1000
}

fn default_tier() -> String {
    "C".to_string()
}

pub fn format_discord_avatar(user_id: &str, avatar: Option<&str>) -> String {
    match avatar {
        Some(av) if av.starts_with("http") => av.to_string(),
        Some(av) if !av.is_empty() => {
            let ext = if av.starts_with("a_") { "gif" } else { "png" };
            format!("https://cdn.discordapp.com/avatars/{user_id}/{av}.{ext}")
        }
        _ => "https://cdn.discordapp.com/embed/avatars/0.png".to_string(),
    }
}

pub fn elo_to_level(elo: u32) -> u32 {
    match elo {
        0..=500 => 1,
        501..=750 => 2,
        751..=900 => 3,
        901..=1050 => 4,
        1051..=1200 => 5,
        1201..=1350 => 6,
        1351..=1530 => 7,
        1531..=1750 => 8,
        1751..=2000 => 9,
        _ => 10,
    }
}

impl Player {
    pub fn ensure_discord_user(&mut self) {
        if self.faceit_level.is_none() {
            self.faceit_level = Some(elo_to_level(self.faceit_elo));
        }
        if self.discord_user.is_none() {
            let avatar = format_discord_avatar(&self.discord_id, self.avatar.as_deref());
            let name = self.faceit_nickname.clone().unwrap_or_else(|| self.nickname.clone());
            self.discord_user = Some(DiscordUserDetails {
                username: self.nickname.clone(),
                display_name: name,
                avatar_url: avatar,
            });
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TieredAvailablePlayers {
    #[serde(rename = "S")]
    pub s: Vec<Player>,
    #[serde(rename = "A")]
    pub a: Vec<Player>,
    #[serde(rename = "B")]
    pub b: Vec<Player>,
    #[serde(rename = "C")]
    pub c: Vec<Player>,
}

pub fn partition_available_players(state: &LobbyState) -> TieredAvailablePlayers {
    let mut tiered = TieredAvailablePlayers::default();
    let occupied: std::collections::HashSet<&str> = state
        .teams
        .iter()
        .flat_map(|t| {
            let mut ids = Vec::new();
            if let Some(ref c) = t.captain {
                ids.push(c.discord_id.as_str());
            }
            for p in &t.players {
                ids.push(p.discord_id.as_str());
            }
            ids
        })
        .collect();

    for p in &state.available_players {
        if occupied.contains(p.discord_id.as_str()) {
            continue;
        }
        let mut player = p.clone();
        player.ensure_discord_user();

        let target_tier = if let Some(ref ov) = player.mixton_tier_override {
            ov.to_uppercase()
        } else if matches!(player.tier.to_uppercase().as_str(), "S" | "A" | "B" | "C") {
            player.tier.to_uppercase()
        } else if let Some(lvl) = player.faceit_level {
            if lvl >= 10 {
                "S".to_string()
            } else if lvl >= 8 {
                "A".to_string()
            } else if lvl >= 5 {
                "B".to_string()
            } else {
                "C".to_string()
            }
        } else if player.faceit_elo >= 2000 {
            "S".to_string()
        } else if player.faceit_elo >= 1700 {
            "A".to_string()
        } else if player.faceit_elo >= 1300 {
            "B".to_string()
        } else {
            "C".to_string()
        };

        player.tier = target_tier.clone();
        match target_tier.as_str() {
            "S" => tiered.s.push(player),
            "A" => tiered.a.push(player),
            "B" => tiered.b.push(player),
            _ => tiered.c.push(player),
        }
    }

    tiered.s.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));
    tiered.a.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));
    tiered.b.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));
    tiered.c.sort_by(|a, b| b.faceit_elo.cmp(&a.faceit_elo));

    tiered
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LobbyTeam {
    pub id: String,
    pub name: String,
    pub tag: String,
    pub captain: Option<Player>,
    pub players: Vec<Player>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MapVetoState {
    pub active_maps: Vec<MapInfo>,
    pub banned_maps: Vec<String>,
    pub picked_maps: Vec<String>,
    pub turn: String, // "team_1" o "team_2"
    pub current_step: String, // "ban" o "pick"
    pub final_chosen_map: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    #[serde(rename = "authorId")]
    pub author_id: String,
    #[serde(rename = "authorName")]
    pub author_name: String,
    #[serde(rename = "authorAvatar")]
    pub author_avatar: Option<String>,
    pub content: String,
    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LobbyState {
    #[serde(rename = "lobbyMode")]
    pub lobby_mode: String,
    #[serde(rename = "gamesPerMatch")]
    pub games_per_match: u32,
    #[serde(rename = "roundsPerGame")]
    pub rounds_per_game: u32,
    pub teams: Vec<LobbyTeam>,
    #[serde(rename = "availablePlayers")]
    pub available_players: Vec<Player>,
    #[serde(rename = "mapVeto")]
    pub map_veto: Option<MapVetoState>,
    #[serde(rename = "draftStarted")]
    pub draft_started: bool,
    #[serde(rename = "draftFinished")]
    pub draft_finished: bool,
    #[serde(rename = "pickOrder", default)]
    pub pick_order: Vec<String>,
    #[serde(rename = "pickSequence", default)]
    pub pick_sequence: Vec<String>,
    #[serde(rename = "currentPickIndex", default)]
    pub current_pick_index: usize,
    #[serde(rename = "captainAProposal", default)]
    pub captain_a_proposal: Option<serde_json::Value>,
    #[serde(rename = "captainBProposal", default)]
    pub captain_b_proposal: Option<serde_json::Value>,
    #[serde(rename = "captainAReady", default)]
    pub captain_a_ready: bool,
    #[serde(rename = "captainBReady", default)]
    pub captain_b_ready: bool,
    #[serde(rename = "betAgreed", default)]
    pub bet_agreed: bool,
    #[serde(rename = "agreedBetAmount", default)]
    pub agreed_bet_amount: Option<f64>,
    #[serde(rename = "effectiveBetAmount", default)]
    pub effective_bet_amount: f64,
    #[serde(rename = "autoCountdownEndsAt", default)]
    pub auto_countdown_ends_at: Option<i64>,
    #[serde(rename = "rouletteWinnerTeamId", default)]
    pub roulette_winner_team_id: Option<String>,
    #[serde(rename = "rouletteWinnerName", default)]
    pub roulette_winner_name: Option<String>,
    #[serde(rename = "rouletteSpinning", default)]
    pub roulette_spinning: bool,
    #[serde(rename = "rouletteStartedAt", default)]
    pub roulette_started_at: Option<i64>,
    #[serde(rename = "rouletteSliceOffset", default)]
    pub roulette_slice_offset: Option<f64>,
    #[serde(default)]
    pub tournament: Option<serde_json::Value>,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

impl LobbyState {
    pub fn sanitize(&mut self) {
        for team in &mut self.teams {
            if let Some(ref mut cap) = team.captain {
                cap.ensure_discord_user();
            }
            for p in &mut team.players {
                p.ensure_discord_user();
            }
        }
        for p in &mut self.available_players {
            p.ensure_discord_user();
        }
    }
}

impl Default for LobbyState {
    fn default() -> Self {
        Self {
            lobby_mode: "auto".into(),
            games_per_match: 1,
            rounds_per_game: 1,
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
            available_players: Vec::new(),
            map_veto: None,
            draft_started: false,
            draft_finished: false,
            pick_order: Vec::new(),
            pick_sequence: Vec::new(),
            current_pick_index: 0,
            captain_a_proposal: None,
            captain_b_proposal: None,
            captain_a_ready: false,
            captain_b_ready: false,
            bet_agreed: false,
            agreed_bet_amount: None,
            effective_bet_amount: 0.0,
            auto_countdown_ends_at: None,
            roulette_winner_team_id: None,
            roulette_winner_name: None,
            roulette_spinning: false,
            roulette_started_at: None,
            roulette_slice_offset: None,
            tournament: None,
            updated_at: Utc::now().timestamp_millis(),
        }
    }
}

/// Gestor del lobby en memoria para todas las comunidades/servidores de Discord.
#[derive(Clone)]
pub struct LobbyManager {
    states: Arc<RwLock<HashMap<String, LobbyState>>>,
    chats: Arc<RwLock<HashMap<String, Vec<ChatMessage>>>>,
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<String>>>>,
    default_guild_id: String,
}

impl LobbyManager {
    pub fn new(default_guild_id: impl Into<String>) -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
            chats: Arc::new(RwLock::new(HashMap::new())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            default_guild_id: default_guild_id.into(),
        }
    }

    pub fn resolve_guild_id<'a>(&'a self, requested: Option<&'a str>) -> &'a str {
        match requested {
            Some(g) if !g.trim().is_empty() => g.trim(),
            _ => &self.default_guild_id,
        }
    }

    /// Obtiene o inicializa el canal de difusión WebSocket para el guild.
    pub async fn get_broadcast_channel(&self, guild_id: &str) -> broadcast::Sender<String> {
        let mut chs = self.channels.write().await;
        chs.entry(guild_id.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(100);
                tx
            })
            .clone()
    }

    /// Retorna una copia del estado actual del lobby en memoria.
    /// Actualiza el estado del lobby y lo difunde a los WebSockets.
    pub async fn update_state(&self, guild_id: &str, mut new_state: LobbyState) -> LobbyState {
        let mut states = self.states.write().await;
        new_state.updated_at = Utc::now().timestamp_millis();
        states.insert(guild_id.to_string(), new_state.clone());
        drop(states);

        self.broadcast_state(guild_id).await;
        new_state
    }

    pub async fn get_state(&self, guild_id: &str) -> LobbyState {
        let states = self.states.read().await;
        states
            .get(guild_id)
            .cloned()
            .unwrap_or_else(LobbyState::default)
    }

    /// Difunde el estado actual por WebSockets a todos los navegadores conectados.
    pub async fn broadcast_state(&self, guild_id: &str) {
        let state = self.get_state(guild_id).await;
        let available_players = partition_available_players(&state);
        let payload = serde_json::json!({
            "type": "state_update",
            "data": {
                "state": state,
                "availablePlayers": available_players,
                "commandEnabled": true,
                "guildName": "MachuPeek",
                "guildId": guild_id
            }
        })
        .to_string();

        let tx = self.get_broadcast_channel(guild_id).await;
        let _ = tx.send(payload);
    }

    /// Jugador ingresa a la sala de Mixton.
    pub async fn join_queue(&self, guild_id: &str, mut player: Player) -> LobbyState {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();

        player.ensure_discord_user();
        if let Some(pos) = state.available_players.iter().position(|p| p.discord_id == player.discord_id) {
            let existing = &state.available_players[pos];
            if player.faceit_elo == 1000 && existing.faceit_elo != 1000 {
                player.faceit_elo = existing.faceit_elo;
                player.faceit_level = existing.faceit_level;
                player.tier = existing.tier.clone();
            }
            if player.faceit_nickname.is_none() && existing.faceit_nickname.is_some() {
                player.faceit_nickname = existing.faceit_nickname.clone();
            }
            state.available_players.remove(pos);
        }
        state.available_players.push(player);
        state.updated_at = Utc::now().timestamp_millis();
        state.sanitize();

        let updated = state.clone();
        drop(states);

        self.broadcast_state(guild_id).await;
        updated
    }

    /// Jugador abandona la sala de Mixton.
    pub async fn leave_queue(&self, guild_id: &str, discord_id: &str) -> LobbyState {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();

        state.available_players.retain(|p| p.discord_id != discord_id);
        for t in &mut state.teams {
            t.players.retain(|p| p.discord_id != discord_id);
            if let Some(ref c) = t.captain {
                if c.discord_id == discord_id {
                    t.captain = None;
                }
            }
        }
        state.updated_at = Utc::now().timestamp_millis();

        let updated = state.clone();
        drop(states);

        self.broadcast_state(guild_id).await;
        updated
    }

    /// Marca o desmarca al jugador como "Listo".
    pub async fn set_ready(&self, guild_id: &str, discord_id: &str, ready: bool) -> LobbyState {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();

        if let Some(p) = state.available_players.iter_mut().find(|p| p.discord_id == discord_id) {
            p.ready = ready;
        }
        for t in &mut state.teams {
            if let Some(p) = t.players.iter_mut().find(|p| p.discord_id == discord_id) {
                p.ready = ready;
            }
            if let Some(ref mut c) = t.captain {
                if c.discord_id == discord_id {
                    c.ready = ready;
                }
            }
        }
        state.updated_at = Utc::now().timestamp_millis();

        let updated = state.clone();
        drop(states);

        self.broadcast_state(guild_id).await;
        updated
    }

    /// Reinicia la sala de Mixton al estado inicial.
    pub async fn reset_lobby(&self, guild_id: &str) -> LobbyState {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();
        *state = LobbyState::default();
        let updated = state.clone();
        drop(states);

        self.broadcast_state(guild_id).await;
        updated
    }

    /// Inicia el veto de mapas oficial de CS2.
    pub async fn start_map_veto(&self, guild_id: &str) -> Option<MapVetoState> {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();

        let catalog = default_catalog();
        let active_maps: Vec<MapInfo> = catalog.into_iter().filter(|m| m.active).collect();

        let veto = MapVetoState {
            active_maps,
            banned_maps: Vec::new(),
            picked_maps: Vec::new(),
            turn: "team_1".into(),
            current_step: "ban".into(),
            final_chosen_map: None,
        };

        state.map_veto = Some(veto.clone());
        state.updated_at = Utc::now().timestamp_millis();
        drop(states);

        self.broadcast_state(guild_id).await;
        Some(veto)
    }

    /// Veto de mapa (Ban).
    pub async fn ban_map(&self, guild_id: &str, team_id: &str, map_id: &str) -> Result<LobbyState, String> {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();

        let veto = state.map_veto.as_mut().ok_or("No hay proceso de veto activo")?;
        if veto.turn != team_id {
            return Err("No es el turno de tu equipo para vetar".into());
        }
        if veto.banned_maps.contains(&map_id.to_string()) {
            return Err("Este mapa ya ha sido descartado".into());
        }

        veto.banned_maps.push(map_id.to_string());
        veto.active_maps.retain(|m| m.id != map_id);

        // Si solo queda 1 mapa, ese es el mapa final
        if veto.active_maps.len() == 1 {
            veto.final_chosen_map = Some(veto.active_maps[0].id.clone());
            info!("[Lobby] Veto completado. Mapa elegido: {:?}", veto.final_chosen_map);
        } else {
            // Alternar turno
            veto.turn = if team_id == "team_1" { "team_2".into() } else { "team_1".into() };
        }

        state.updated_at = Utc::now().timestamp_millis();
        let updated = state.clone();
        drop(states);

        self.broadcast_state(guild_id).await;
        Ok(updated)
    }

    /// Historial de chat del lobby.
    pub async fn get_chat_history(&self, guild_id: &str) -> Vec<ChatMessage> {
        let chats = self.chats.read().await;
        chats.get(guild_id).cloned().unwrap_or_default()
    }

    /// Añade un mensaje de chat y lo difunde a los WebSockets.
    pub async fn add_chat_message(&self, guild_id: &str, msg: ChatMessage) {
        let mut chats = self.chats.write().await;
        let history = chats.entry(guild_id.to_string()).or_default();
        history.push(msg.clone());
        if history.len() > 100 {
            history.remove(0);
        }
        drop(chats);

        let payload = serde_json::json!({
            "type": "chat_message",
            "data": msg
        })
        .to_string();

        let tx = self.get_broadcast_channel(guild_id).await;
        let _ = tx.send(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lobby_join_ready_and_leave_lifecycle() {
        let mgr = LobbyManager::new("guild_123");
        let player = Player {
            discord_id: "user_1".into(),
            nickname: "Gamer1".into(),
            faceit_nickname: Some("gamer_1".into()),
            steam_id_64: Some("76561198000000001".into()),
            avatar: None,
            faceit_elo: 1500,
            ready: false,
            tier: "tier1".into(),
            faceit_level: Some(7),
            mixton_tier_override: None,
            discord_user: None,
        };

        // 1. Join
        let s = mgr.join_queue("guild_123", player.clone()).await;
        assert_eq!(s.available_players.len(), 1);
        assert!(!s.available_players[0].ready);

        // 2. Ready
        let s = mgr.set_ready("guild_123", "user_1", true).await;
        assert!(s.available_players[0].ready);

        // 3. Leave
        let s = mgr.leave_queue("guild_123", "user_1").await;
        assert_eq!(s.available_players.len(), 0);
    }

    #[tokio::test]
    async fn map_veto_flow() {
        let mgr = LobbyManager::new("guild_123");
        mgr.start_map_veto("guild_123").await;

        let s = mgr.ban_map("guild_123", "team_1", "de_mirage").await.unwrap();
        let veto = s.map_veto.unwrap();
        assert!(veto.banned_maps.contains(&"de_mirage".to_string()));
        assert_eq!(veto.turn, "team_2");
    }
}
