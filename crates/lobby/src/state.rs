//! Estado en memoria RAM y gestión en tiempo real de las salas de Mixton.

use crate::maps::{default_catalog, MapInfo};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Player {
    #[serde(rename = "discordId")]
    pub discord_id: String,
    pub nickname: String,
    #[serde(rename = "faceit_nickname")]
    pub faceit_nickname: Option<String>,
    #[serde(rename = "steam_id_64")]
    pub steam_id_64: Option<String>,
    pub avatar: Option<String>,
    #[serde(rename = "faceit_elo")]
    pub faceit_elo: u32,
    pub ready: bool,
    pub tier: String,
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
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
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
        let payload = serde_json::json!({
            "type": "state_update",
            "data": {
                "state": state,
                "availablePlayers": state.available_players,
                "commandEnabled": true,
                "guildId": guild_id
            }
        })
        .to_string();

        let tx = self.get_broadcast_channel(guild_id).await;
        let _ = tx.send(payload);
    }

    /// Jugador ingresa a la sala de Mixton.
    pub async fn join_queue(&self, guild_id: &str, player: Player) -> LobbyState {
        let mut states = self.states.write().await;
        let state = states.entry(guild_id.to_string()).or_default();

        // Evitar duplicados
        state.available_players.retain(|p| p.discord_id != player.discord_id);
        state.available_players.push(player);
        state.updated_at = Utc::now().timestamp_millis();

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
