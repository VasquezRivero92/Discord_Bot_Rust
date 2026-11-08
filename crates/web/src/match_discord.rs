//! Lado Discord de una partida lanzada desde el lobby (antes en `launchMatchInternal` del legado):
//! canales de voz por equipo, mover a los jugadores, anuncio en el canal de partidas, y la
//! limpieza al terminar (devolver a cada uno a su canal y borrar los de la partida).
//!
//! Va por la API REST de Discord con el token del bot. Nada de esto toca dinero: un fallo de
//! Discord **nunca** cancela una partida ya cobrada (el legado cobraba y luego podía fallar por
//! no encontrar el canal de anuncios, dejando el dinero descontado sin partida).
//!
//! Los ids que se interpolan en rutas se validan (solo dígitos) para impedir inyección de ruta.

use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::time::Duration;
use tokio::sync::OnceCell;

const API: &str = "https://discord.com/api/v10";

// Bits de permisos de Discord.
const VIEW_CHANNEL: u64 = 1 << 10;
const CONNECT: u64 = 1 << 20;
const SPEAK: u64 = 1 << 21;
const MANAGE_CHANNELS: u64 = 1 << 4;
const MOVE_MEMBERS: u64 = 1 << 24;

const GUILD_TEXT: u64 = 0;
const GUILD_VOICE: u64 = 2;
const GUILD_CATEGORY: u64 = 4;

/// Jugador tal como aparece en el anuncio.
#[derive(Debug, Clone)]
pub struct RosterEntry {
    pub discord_id: String,
    pub nickname: String,
    pub elo: u32,
    pub level: Option<u32>,
}

/// Lo que hace falta para preparar Discord para una partida.
#[derive(Debug, Clone)]
pub struct MatchAnnouncement {
    pub match_id: String,
    pub guild_id: String,
    pub team_a: String,
    pub team_b: String,
    pub roster_a: Vec<RosterEntry>,
    pub roster_b: Vec<RosterEntry>,
    pub bet_soles: f64,
    pub server_ip: String,
    pub map: String,
    pub games: u32,
    pub initiator: String,
}

/// Canales configurados para el servidor (listas separadas por comas, como en el legado).
#[derive(Debug, Clone, Default)]
pub struct MatchChannelsConfig {
    pub matches_channel_ids: Vec<String>,
    pub waiting_room_ids: Vec<String>,
    pub access_role_id: Option<String>,
}

/// Resultado: se guarda en la partida activa para poder limpiarla al terminar.
#[derive(Debug, Clone, Default)]
pub struct DiscordSetup {
    pub vc_a: Option<String>,
    pub vc_b: Option<String>,
    pub matches_channel_id: Option<String>,
    pub message_id: Option<String>,
    pub waiting_room_id: Option<String>,
    /// `discordId -> canal de voz en el que estaba antes de moverlo`.
    pub original_voice_states: Map<String, Value>,
}

impl DiscordSetup {
    /// Campos para la partida activa (mismos nombres que el legado).
    pub fn to_match_fields(&self) -> Value {
        json!({
            "faction1_vc_id": self.vc_a,
            "faction2_vc_id": self.vc_b,
            "matches_channel_id": self.matches_channel_id,
            "message_id": self.message_id,
            "waiting_room_id": self.waiting_room_id,
            "original_voice_states": self.original_voice_states,
        })
    }

    /// Enlace al anuncio, si se publicó.
    pub fn message_url(&self, guild_id: &str) -> Option<String> {
        Some(format!(
            "https://discord.com/channels/{guild_id}/{}/{}",
            self.matches_channel_id.as_ref()?,
            self.message_id.as_ref()?
        ))
    }
}

fn is_snowflake(id: &str) -> bool {
    (15..=21).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit())
}

/// Ids de una lista "a,b,c" (solo los que parecen ids de Discord).
pub fn parse_id_list(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| is_snowflake(s))
        .map(str::to_string)
        .collect()
}

/// Cliente REST mínimo para el lado Discord de las partidas.
pub struct MatchDiscord {
    http: reqwest::Client,
    token: String,
    api: String,
    bot_id: OnceCell<Option<String>>,
}

impl std::fmt::Debug for MatchDiscord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchDiscord").field("token", &"<redacted>").finish()
    }
}

impl MatchDiscord {
    /// `None` sin un token con forma de token de bot (tres partes separadas por puntos): así
    /// las pruebas y los tokens de ejemplo nunca llaman a Discord.
    pub fn new(bot_token: &str) -> Option<Self> {
        let token = bot_token.trim();
        if token.len() < 50 || token.split('.').count() != 3 {
            return None;
        }
        Some(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("DiscordBot (https://machupeek.com, 1.0)")
                .build()
                .unwrap_or_default(),
            token: token.to_string(),
            api: API.to_string(),
            bot_id: OnceCell::new(),
        })
    }

    /// Petición con un reintento si Discord responde 429 (límite de velocidad).
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        reason: Option<&str>,
    ) -> Result<(StatusCode, Value), String> {
        for attempt in 0..2 {
            let mut req = self
                .http
                .request(method.clone(), format!("{}{path}", self.api))
                .header("Authorization", format!("Bot {}", self.token));
            if let Some(r) = reason {
                req = req.header("X-Audit-Log-Reason", r);
            }
            if let Some(b) = body {
                req = req.json(b);
            }
            let res = req.send().await.map_err(|e| format!("Discord: {e}"))?;
            let status = res.status();
            let value: Value = res.json().await.unwrap_or(Value::Null);
            if status == StatusCode::TOO_MANY_REQUESTS && attempt == 0 {
                let wait = value["retry_after"].as_f64().unwrap_or(1.0).clamp(0.1, 5.0);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }
            return Ok((status, value));
        }
        Err("Discord: demasiadas peticiones".into())
    }

    async fn bot_user_id(&self) -> Option<String> {
        self.bot_id
            .get_or_init(|| async {
                match self.call(reqwest::Method::GET, "/users/@me", None, None).await {
                    Ok((s, v)) if s.is_success() => v["id"].as_str().map(str::to_string),
                    _ => None,
                }
            })
            .await
            .clone()
    }

    /// Canal de voz en el que está ahora el usuario (o `None`).
    async fn voice_channel_of(&self, guild: &str, user: &str) -> Option<String> {
        if !is_snowflake(guild) || !is_snowflake(user) {
            return None;
        }
        match self
            .call(reqwest::Method::GET, &format!("/guilds/{guild}/voice-states/{user}"), None, None)
            .await
        {
            Ok((s, v)) if s.is_success() => v["channel_id"].as_str().map(str::to_string),
            _ => None,
        }
    }

    async fn is_member(&self, guild: &str, user: &str) -> bool {
        is_snowflake(user)
            && matches!(
                self.call(reqwest::Method::GET, &format!("/guilds/{guild}/members/{user}"), None, None)
                    .await,
                Ok((s, _)) if s.is_success()
            )
    }

    async fn move_member(&self, guild: &str, user: &str, channel: &str) -> bool {
        is_snowflake(user)
            && matches!(
                self.call(
                    reqwest::Method::PATCH,
                    &format!("/guilds/{guild}/members/{user}"),
                    Some(&json!({ "channel_id": channel })),
                    Some("Mover al canal de voz de su equipo CS2"),
                )
                .await,
                Ok((s, _)) if s.is_success()
            )
    }

    /// Crea un canal de voz con los reintentos del legado: con categoría y permisos, sin
    /// categoría, y por último con permisos estándar.
    async fn create_voice(
        &self,
        guild: &str,
        name: &str,
        parent: Option<&str>,
        overwrites: &Value,
    ) -> Option<String> {
        let base = json!({ "name": name, "type": GUILD_VOICE, "user_limit": 5 });
        let mut attempts = Vec::new();
        if let Some(p) = parent {
            attempts.push(json!({ "parent_id": p, "permission_overwrites": overwrites }));
        }
        attempts.push(json!({ "permission_overwrites": overwrites }));
        if let Some(p) = parent {
            attempts.push(json!({ "parent_id": p }));
        }
        attempts.push(json!({}));
        for extra in attempts {
            let mut body = base.clone();
            if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
                b.extend(e.clone());
            }
            match self
                .call(
                    reqwest::Method::POST,
                    &format!("/guilds/{guild}/channels"),
                    Some(&body),
                    Some("Partida Mixton CS2"),
                )
                .await
            {
                Ok((s, v)) if s.is_success() => return v["id"].as_str().map(str::to_string),
                Ok((s, _)) => tracing::warn!("[Discord] crear canal \"{name}\": HTTP {s}; se reintenta"),
                Err(e) => tracing::warn!("[Discord] crear canal \"{name}\": {e}"),
            }
        }
        None
    }

    /// Prepara Discord para la partida. Nunca falla: lo que no se pueda hacer queda en `None`
    /// y en el log.
    pub async fn setup_match(&self, m: &MatchAnnouncement, cfg: &MatchChannelsConfig) -> DiscordSetup {
        let mut out = DiscordSetup::default();
        let guild = m.guild_id.as_str();
        if !is_snowflake(guild) {
            tracing::warn!("[Discord] {}: servidor sin id válido; se omite Discord", m.match_id);
            return out;
        }
        let channels = match self
            .call(reqwest::Method::GET, &format!("/guilds/{guild}/channels"), None, None)
            .await
        {
            Ok((s, Value::Array(a))) if s.is_success() => a,
            other => {
                tracing::warn!("[Discord] {}: no se pudieron leer los canales: {other:?}", m.match_id);
                Vec::new()
            }
        };
        out.matches_channel_id = pick_channel(
            &channels,
            &cfg.matches_channel_ids,
            GUILD_TEXT,
            &["partidas", "mixton", "partida", "general"],
        );
        out.waiting_room_id = pick_channel(
            &channels,
            &cfg.waiting_room_ids,
            GUILD_VOICE,
            &["espera", "waiting", "general", "lobby", "mixton"],
        );
        let parent = parent_category(&channels, out.waiting_room_id.as_deref());

        // Canales de voz por equipo, solo con los jugadores (y el rol de acceso) dentro.
        let bot = self.bot_user_id().await;
        let mut members_a = Vec::new();
        for p in &m.roster_a {
            if self.is_member(guild, &p.discord_id).await {
                members_a.push(p.discord_id.clone());
            }
        }
        let mut members_b = Vec::new();
        for p in &m.roster_b {
            if self.is_member(guild, &p.discord_id).await {
                members_b.push(p.discord_id.clone());
            }
        }
        let ow_a = build_overwrites(guild, bot.as_deref(), cfg.access_role_id.as_deref(), &members_a);
        let ow_b = build_overwrites(guild, bot.as_deref(), cfg.access_role_id.as_deref(), &members_b);
        out.vc_a = self
            .create_voice(guild, &format!("🔊 {}", m.team_a), parent.as_deref(), &ow_a)
            .await;
        out.vc_b = self
            .create_voice(guild, &format!("🔊 {}", m.team_b), parent.as_deref(), &ow_b)
            .await;

        // Mover a quien ya esté en un canal de voz, recordando de dónde venía.
        for (members, vc) in [(&members_a, &out.vc_a), (&members_b, &out.vc_b)] {
            let Some(vc) = vc else { continue };
            for uid in members {
                if let Some(prev) = self.voice_channel_of(guild, uid).await {
                    if self.move_member(guild, uid, vc).await {
                        out.original_voice_states.insert(uid.clone(), json!(prev));
                    }
                }
            }
        }

        // Anuncio en el canal de partidas.
        if let Some(ch) = out.matches_channel_id.clone() {
            let msg = build_announcement(m, out.vc_a.as_deref(), out.vc_b.as_deref());
            match self
                .call(reqwest::Method::POST, &format!("/channels/{ch}/messages"), Some(&msg), None)
                .await
            {
                Ok((s, v)) if s.is_success() => out.message_id = v["id"].as_str().map(str::to_string),
                Ok((s, _)) => tracing::warn!("[Discord] {}: anuncio no publicado (HTTP {s})", m.match_id),
                Err(e) => tracing::warn!("[Discord] {}: anuncio no publicado: {e}", m.match_id),
            }
        } else {
            tracing::warn!("[Discord] {}: sin canal de anuncios configurado ni de respaldo", m.match_id);
        }
        out
    }

    /// Limpieza al terminar o cancelar: devuelve a cada jugador a su canal de origen (si sigue
    /// en el de la partida) y borra los canales de la partida. Idempotente.
    pub async fn cleanup_match(&self, match_doc: &Value) {
        let guild = match_doc["guild_id"].as_str().unwrap_or_default();
        if !is_snowflake(guild) {
            return;
        }
        let vcs: Vec<String> = ["faction1_vc_id", "faction2_vc_id"]
            .iter()
            .filter_map(|k| match_doc[*k].as_str())
            .filter(|id| is_snowflake(id))
            .map(str::to_string)
            .collect();
        if vcs.is_empty() {
            return;
        }
        if let Some(states) = match_doc["original_voice_states"].as_object() {
            for (uid, prev) in states {
                let Some(prev) = prev.as_str().filter(|p| is_snowflake(p)) else { continue };
                if let Some(now) = self.voice_channel_of(guild, uid).await {
                    if vcs.contains(&now) {
                        self.move_member(guild, uid, prev).await;
                    }
                }
            }
        }
        for vc in vcs {
            match self
                .call(reqwest::Method::DELETE, &format!("/channels/{vc}"), None, Some("Partida finalizada"))
                .await
            {
                Ok((s, _)) if s.is_success() || s == StatusCode::NOT_FOUND => {}
                Ok((s, _)) => tracing::warn!("[Discord] borrar canal {vc}: HTTP {s}"),
                Err(e) => tracing::warn!("[Discord] borrar canal {vc}: {e}"),
            }
        }
    }
}

/// Partida activa tal como está antes de cerrarla (al cerrarla se borra de las activas).
pub async fn active_match_doc(fs: &data::FirestoreClient, match_id: &str) -> Option<Value> {
    fs.get_doc::<Value>("faceit_active_matches", match_id)
        .await
        .ok()
        .flatten()
}

/// Lanza en segundo plano la limpieza de Discord de una partida cerrada, para no demorar la
/// respuesta al servidor CS2.
pub fn spawn_cleanup(discord: &Option<std::sync::Arc<MatchDiscord>>, doc: Option<Value>) {
    if let (Some(d), Some(doc)) = (discord.clone(), doc) {
        tokio::spawn(async move { d.cleanup_match(&doc).await });
    }
}

/// Canal configurado que exista en el servidor; si no, el primero cuyo nombre contenga alguna
/// pista (en orden), y si no, el primero del tipo.
pub fn pick_channel(channels: &[Value], configured: &[String], kind: u64, hints: &[&str]) -> Option<String> {
    let of_kind: Vec<&Value> = channels
        .iter()
        .filter(|c| c["type"].as_u64() == Some(kind))
        .collect();
    let id_of = |c: &&Value| c["id"].as_str().map(str::to_string);
    if let Some(found) = configured
        .iter()
        .find(|id| of_kind.iter().any(|c| c["id"].as_str() == Some(id.as_str())))
    {
        return Some(found.clone());
    }
    for hint in hints {
        if let Some(c) = of_kind.iter().find(|c| {
            c["name"]
                .as_str()
                .is_some_and(|n| n.to_lowercase().contains(hint))
        }) {
            return id_of(c);
        }
    }
    of_kind.first().and_then(id_of)
}

/// Categoría de la sala de espera; si no tiene, una categoría de partidas.
fn parent_category(channels: &[Value], waiting_room: Option<&str>) -> Option<String> {
    let categories: Vec<&Value> = channels
        .iter()
        .filter(|c| c["type"].as_u64() == Some(GUILD_CATEGORY))
        .collect();
    if let Some(parent) = waiting_room
        .and_then(|w| channels.iter().find(|c| c["id"].as_str() == Some(w)))
        .and_then(|c| c["parent_id"].as_str())
    {
        if categories.iter().any(|c| c["id"].as_str() == Some(parent)) {
            return Some(parent.to_string());
        }
    }
    for hint in ["partida", "mixton", "cs2", "faceit", "voz", "voice"] {
        if let Some(c) = categories.iter().find(|c| {
            c["name"]
                .as_str()
                .is_some_and(|n| n.to_lowercase().contains(hint))
        }) {
            return c["id"].as_str().map(str::to_string);
        }
    }
    None
}

/// Permisos del canal de un equipo: nadie más puede conectarse; el bot lo administra; los
/// jugadores del equipo (y el rol de acceso, si hay) entran y hablan.
pub fn build_overwrites(
    guild: &str,
    bot: Option<&str>,
    access_role: Option<&str>,
    members: &[String],
) -> Value {
    let mut ow = vec![json!({ "id": guild, "type": 0, "allow": "0", "deny": CONNECT.to_string() })];
    let player = (VIEW_CHANNEL | CONNECT | SPEAK).to_string();
    if let Some(b) = bot {
        ow.push(json!({
            "id": b, "type": 1, "deny": "0",
            "allow": (VIEW_CHANNEL | CONNECT | SPEAK | MANAGE_CHANNELS | MOVE_MEMBERS).to_string(),
        }));
    }
    if let Some(r) = access_role.filter(|r| is_snowflake(r)) {
        ow.push(json!({ "id": r, "type": 0, "allow": player, "deny": "0" }));
    }
    for m in members {
        ow.push(json!({ "id": m, "type": 1, "allow": player, "deny": "0" }));
    }
    Value::Array(ow)
}

fn level_emoji(level: Option<u32>) -> &'static str {
    match level.unwrap_or(1) {
        10 => "🔴",
        8..=9 => "🟠",
        5..=7 => "🔵",
        _ => "⚪",
    }
}

fn games_label(games: u32) -> String {
    if games <= 1 {
        "1 Juego (Al mejor de 1)".into()
    } else {
        format!("{games} Juegos (Al mejor de {games} - Gana {})", games / 2 + 1)
    }
}

/// Mensaje del anuncio (contenido con menciones + embed), como el del legado sin los botones
/// de resultado (el bot de Rust todavía no los atiende).
pub fn build_announcement(m: &MatchAnnouncement, vc_a: Option<&str>, vc_b: Option<&str>) -> Value {
    let roster_lines = |roster: &[RosterEntry]| -> String {
        roster
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "{} {} <@{}> (`{}` - ELO: {})",
                    if i == 0 { "👑" } else { "•" },
                    level_emoji(p.level),
                    p.discord_id,
                    p.nickname,
                    p.elo
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let avg = |roster: &[RosterEntry]| -> u32 {
        let n = roster.len().max(1) as u32;
        roster.iter().map(|p| p.elo).sum::<u32>() / n
    };
    let games = games_label(m.games);
    let total_players = m.roster_a.len() + m.roster_b.len();
    let mut fields = vec![
        json!({ "name": "🌐 Formato", "value": format!("Mixtón ({games})"), "inline": true }),
        json!({ "name": "🗺️ Mapa", "value": format!("`{}`", m.map), "inline": true }),
    ];
    if let (Some(a), Some(b)) = (vc_a, vc_b) {
        fields.push(json!({ "name": "📍 Canales de Voz", "value": format!("🔊 <#{a}>\n🔊 <#{b}>"), "inline": true }));
    }
    if m.bet_soles > 0.0 {
        fields.push(json!({
            "name": "💰 Apuesta y Pozo",
            "value": format!(
                "💵 **Cuota:** S/ {:.2} c/u\n🏆 **Pozo Total:** S/ {:.2}",
                m.bet_soles,
                m.bet_soles * total_players as f64
            ),
            "inline": true,
        }));
    }
    fields.push(json!({
        "name": "🖥️ Servidor CS2 Dedicado",
        "value": format!("⚡ **Comando de Conexión:**\n```connect {}```", m.server_ip),
        "inline": false,
    }));
    fields.push(json!({ "name": format!("👥 {} [ELO: {}]", m.team_a, avg(&m.roster_a)), "value": roster_lines(&m.roster_a), "inline": false }));
    fields.push(json!({ "name": format!("👥 {} [ELO: {}]", m.team_b, avg(&m.roster_b)), "value": roster_lines(&m.roster_b), "inline": false }));

    let title = if m.bet_soles > 0.0 {
        format!("🎮 ¡Partida Mixton con Apuesta (S/ {:.2})!", m.bet_soles)
    } else {
        "🎮 ¡Partida Mixton Competitiva!".to_string()
    };
    let pings: Vec<String> = m
        .roster_a
        .iter()
        .chain(&m.roster_b)
        .map(|p| format!("<@{}>", p.discord_id))
        .collect();
    json!({
        "content": format!(
            "📢 **¡Partida Mixtón Iniciada!** ({games})\nJugadores: {}\n⚡ **Conexión CS2:** `connect {}`",
            pings.join(" "),
            m.server_ip
        ),
        "embeds": [{
            "title": title,
            "color": if m.bet_soles > 0.0 { 0x15C253 } else { 0xFF5500 },
            "description": format!(
                "Lanzada por **{}**.\n🖥️ **Servidor CS2:** `connect {}`\n🗺️ **Mapa:** **{}** • 🎮 **Juegos:** **{games}**",
                m.initiator, m.server_ip, m.map
            ),
            "fields": fields,
            "footer": { "text": format!("Partida {}", m.match_id) },
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }],
        "allowed_mentions": { "parse": ["users"] },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "100000000000000001";

    fn entry(id: &str, elo: u32) -> RosterEntry {
        RosterEntry { discord_id: id.into(), nickname: format!("n{id}"), elo, level: Some(10) }
    }

    #[test]
    fn el_canal_solo_deja_entrar_a_su_equipo() {
        let ow = build_overwrites(G, Some("200000000000000002"), None, &["300000000000000003".into()]);
        let ow = ow.as_array().unwrap();
        // @everyone sin Connect.
        assert_eq!(ow[0]["id"], G);
        assert_eq!(ow[0]["deny"], CONNECT.to_string());
        // El jugador entra y habla; no administra.
        let p = ow.iter().find(|o| o["id"] == "300000000000000003").unwrap();
        let allow: u64 = p["allow"].as_str().unwrap().parse().unwrap();
        assert_eq!(allow & CONNECT, CONNECT);
        assert_eq!(allow & MANAGE_CHANNELS, 0);
    }

    #[test]
    fn elige_el_canal_configurado_o_por_nombre() {
        let chans = vec![
            json!({"id": "111111111111111111", "type": 0, "name": "general"}),
            json!({"id": "222222222222222222", "type": 0, "name": "partidas-mixton"}),
            json!({"id": "333333333333333333", "type": 2, "name": "Sala de espera"}),
        ];
        assert_eq!(
            pick_channel(&chans, &["111111111111111111".into()], GUILD_TEXT, &["partidas"]).as_deref(),
            Some("111111111111111111")
        );
        // Configurado pero inexistente: se cae a la pista por nombre.
        assert_eq!(
            pick_channel(&chans, &["999999999999999999".into()], GUILD_TEXT, &["partidas"]).as_deref(),
            Some("222222222222222222")
        );
        assert_eq!(
            pick_channel(&chans, &[], GUILD_VOICE, &["espera"]).as_deref(),
            Some("333333333333333333")
        );
    }

    #[test]
    fn el_anuncio_trae_pozo_conexion_y_menciones() {
        let m = MatchAnnouncement {
            match_id: "mixton_1".into(),
            guild_id: G.into(),
            team_a: "Equipo A".into(),
            team_b: "Equipo B".into(),
            roster_a: vec![entry("400000000000000004", 2000), entry("400000000000000005", 1000)],
            roster_b: vec![entry("500000000000000005", 1500)],
            bet_soles: 5.0,
            server_ip: "1.2.3.4:27015".into(),
            map: "de_mirage".into(),
            games: 3,
            initiator: "cap".into(),
        };
        let msg = build_announcement(&m, Some("600000000000000006"), Some("700000000000000007"));
        let content = msg["content"].as_str().unwrap();
        assert!(content.contains("<@400000000000000004>") && content.contains("connect 1.2.3.4:27015"));
        let fields = msg["embeds"][0]["fields"].as_array().unwrap();
        let pot = fields.iter().find(|f| f["name"] == "💰 Apuesta y Pozo").unwrap();
        assert!(pot["value"].as_str().unwrap().contains("S/ 15.00"));
        assert!(fields.iter().any(|f| f["name"] == "👥 Equipo A [ELO: 1500]"));
        assert!(fields[2]["value"].as_str().unwrap().contains("<#600000000000000006>"));
        // Solo menciona usuarios: nada de @everyone.
        assert_eq!(msg["allowed_mentions"]["parse"], json!(["users"]));
        assert_eq!(games_label(3), "3 Juegos (Al mejor de 3 - Gana 2)");
    }

    #[test]
    fn sin_token_real_no_hay_cliente() {
        for t in ["", "token", "x", "test-bot-token", "DISCORD_TOKEN_AQUI"] {
            assert!(MatchDiscord::new(t).is_none(), "{t}");
        }
        let fake_shape = format!("{}.{}.{}", "A".repeat(24), "B".repeat(6), "C".repeat(38));
        assert!(MatchDiscord::new(&fake_shape).is_some());
    }

    #[test]
    fn ids_validos() {
        assert_eq!(parse_id_list(Some(" 123456789012345678 , x, ../1 ")), vec!["123456789012345678"]);
        assert!(!is_snowflake("12/34"));
    }
}
