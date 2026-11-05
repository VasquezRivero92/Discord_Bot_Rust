//! Notificador de partidas CS2 en Discord (anuncio en canal y mensajes directos a jugadores).
//!
//! Tarea B14:
//! - Anuncio con embed en el canal de partidas configurado.
//! - Mensaje directo con comando `connect <ip>` a cada uno de los 10 jugadores.

use lobby::notify::{MatchLaunchInfo, MatchNotifier, TeamNotice};
use serenity::all::{ChannelId, CreateEmbed, CreateMessage, UserId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Máximo de caracteres permitidos en el título de un embed en Discord.
const MAX_TITLE_LEN: usize = 256;
/// Máximo de caracteres permitidos en la descripción de un embed en Discord.
const MAX_DESCRIPTION_LEN: usize = 4096;
/// Máximo de caracteres permitidos en el nombre de un campo de embed.
const MAX_FIELD_NAME_LEN: usize = 256;
/// Máximo de caracteres permitidos en el valor de un campo de embed.
const MAX_FIELD_VALUE_LEN: usize = 1024;
/// Máximo de caracteres permitidos en el contenido de texto de un mensaje en Discord.
const MAX_CONTENT_LEN: usize = 2000;

/// Recorta una cadena de texto según la cantidad máxima de caracteres Unicode.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        s.chars().take(max_len).collect()
    }
}

/// Extrae el primer identificador numérico de canal de una lista separada por comas.
///
/// Descarta fragmentos vacíos y valores comodín o no resueltos que comiencen por `"ID_"`.
pub fn pick_channel_id(raw: &str) -> Option<String> {
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed.starts_with("ID_") {
            continue;
        }
        if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Formatea la etiqueta legible de cantidad de juegos según el estándar del legado.
pub fn format_games_label(games_per_match: u32) -> String {
    if games_per_match <= 1 {
        "1 Juego (Al mejor de 1)".to_string()
    } else {
        let needed = (games_per_match / 2) + 1;
        format!("{games_per_match} Juegos (Al mejor de {games_per_match} - Gana {needed})")
    }
}

/// Genera la línea de comando para la consola de CS2 enviada por mensaje directo.
pub fn build_dm_connect_line(ip: &str) -> String {
    format!("connect {ip}")
}

/// Estructura desacoplada de serenity con los datos preparados para el anuncio de partida.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncementContent {
    /// Contenido de texto con menciones a todos los jugadores participantes.
    pub content: String,
    /// Título del embed.
    pub title: String,
    /// Descripción del embed.
    pub description: String,
    /// Color del embed (hexadecimal numérico).
    pub color: u32,
    /// Campos del embed en pares (nombre, valor).
    pub fields: Vec<(String, String)>,
}

/// Construye el contenido estructurado del anuncio de partida en el canal oficial.
pub fn build_announcement(info: &MatchLaunchInfo) -> AnnouncementContent {
    let games_label = format_games_label(info.games_per_match);

    let color = if info.bet_per_player > 0.0 {
        0x15C253 // Verde del legado si hay apuesta
    } else {
        0xFF5500 // Naranja del legado si es partida competitiva estándar
    };

    // Menciones en el texto exterior para que Discord notifique a los 10 jugadores
    let mut all_pings = Vec::with_capacity(10);
    for (id, _) in &info.team_a.players {
        all_pings.push(format!("<@{id}>"));
    }
    for (id, _) in &info.team_b.players {
        all_pings.push(format!("<@{id}>"));
    }
    let pings_str = all_pings.join(" ");

    let raw_content = format!(
        "📢 **¡Partida Mixtón Iniciada!** ({games_label})\nJugadores: {pings_str}\n⚡ **Conexión CS2:** `connect {}`",
        info.server_ip
    );
    let content = truncate_str(&raw_content, MAX_CONTENT_LEN);

    let raw_desc = format!(
        "Lanzada por **{}**.\n🖥️ **Servidor CS2:** `connect {}`\n🗺️ **Mapa:** **{}** • 🎮 **Juegos:** **{}**",
        info.initiator_username, info.server_ip, info.map_name, games_label
    );
    let description = truncate_str(&raw_desc, MAX_DESCRIPTION_LEN);

    let title = truncate_str("📢 ¡Partida Mixtón Iniciada!", MAX_TITLE_LEN);

    let mut fields = Vec::new();

    fields.push((
        truncate_str("🌐 Formato", MAX_FIELD_NAME_LEN),
        truncate_str(&format!("Mixtón ({games_label})"), MAX_FIELD_VALUE_LEN),
    ));

    fields.push((
        truncate_str("🗺️ Mapa", MAX_FIELD_NAME_LEN),
        truncate_str(&format!("`{}`", info.map_name), MAX_FIELD_VALUE_LEN),
    ));

    if info.bet_per_player > 0.0 {
        let total_players = (info.team_a.players.len() + info.team_b.players.len()) as f64;
        let total_pool = info.bet_per_player * total_players;
        let bet_str = format!(
            "💵 **Cuota:** S/ {:.2} c/u\n🏆 **Pozo Total:** S/ {:.2}",
            info.bet_per_player, total_pool
        );
        fields.push((
            truncate_str("💰 Apuesta y Pozo", MAX_FIELD_NAME_LEN),
            truncate_str(&bet_str, MAX_FIELD_VALUE_LEN),
        ));
    }

    let cs2_server_value = format!(
        "⚡ **Comando de Conexión:**\n```connect {}```\n🗺️ **Mapa Oficial:** `{}` • 🎮 **Juegos:** `{}`",
        info.server_ip, info.map_name, games_label
    );
    fields.push((
        truncate_str("🖥️ Servidor CS2 Dedicado", MAX_FIELD_NAME_LEN),
        truncate_str(&cs2_server_value, MAX_FIELD_VALUE_LEN),
    ));

    // Formatear alineación del Equipo A
    let mut team_a_lines = Vec::new();
    for (id, nick) in &info.team_a.players {
        let is_cap = id == &info.team_a.captain_discord_id;
        let prefix = if is_cap { "👑" } else { "•" };
        team_a_lines.push(format!("{prefix} <@{id}> (`{nick}`)"));
    }
    let team_a_value = team_a_lines.join("\n");
    fields.push((
        truncate_str(&format!("👥 {}", info.team_a.name), MAX_FIELD_NAME_LEN),
        truncate_str(&team_a_value, MAX_FIELD_VALUE_LEN),
    ));

    // Formatear alineación del Equipo B
    let mut team_b_lines = Vec::new();
    for (id, nick) in &info.team_b.players {
        let is_cap = id == &info.team_b.captain_discord_id;
        let prefix = if is_cap { "👑" } else { "•" };
        team_b_lines.push(format!("{prefix} <@{id}> (`{nick}`)"));
    }
    let team_b_value = team_b_lines.join("\n");
    fields.push((
        truncate_str(&format!("👥 {}", info.team_b.name), MAX_FIELD_NAME_LEN),
        truncate_str(&team_b_value, MAX_FIELD_VALUE_LEN),
    ));

    AnnouncementContent {
        content,
        title,
        description,
        color,
        fields,
    }
}

/// Estructura desacoplada para el embed de mensaje directo enviado a cada jugador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmEmbedContent {
    /// Título del embed.
    pub title: String,
    /// Descripción del embed.
    pub description: String,
    /// Color del embed.
    pub color: u32,
    /// Campos informativos (mapa, equipo, capitán).
    pub fields: Vec<(String, String)>,
}

/// Construye el contenido del embed directo personalizado para un jugador.
pub fn build_player_dm(
    info: &MatchLaunchInfo,
    team: &TeamNotice,
    player_discord_id: &str,
) -> DmEmbedContent {
    let is_cap = player_discord_id == team.captain_discord_id;
    let games_label = format_games_label(info.games_per_match);

    let cap_desc = if is_cap {
        "👑 Eres el **Capitán/Representante**.".to_string()
    } else {
        format!("👑 Tu capitán es **<@{}>**.", team.captain_discord_id)
    };

    let raw_desc = format!(
        "{cap_desc}\n\n🗺️ **Mapa Oficial:** `{}`\n🎮 **Juegos por Enfrentamiento:** `{}`\n\n⚡ **Conexión al Servidor:**\nAbre la consola de CS2 (`~`) y ejecuta:\n```connect {}```",
        info.map_name, games_label, info.server_ip
    );
    let description = truncate_str(&raw_desc, MAX_DESCRIPTION_LEN);

    let cap_val = if is_cap {
        "Tú eres el capitán".to_string()
    } else {
        format!("<@{}>", team.captain_discord_id)
    };

    let title = truncate_str(
        &format!("🎮 ¡Partida CS2 Lista — {}!", team.name),
        MAX_TITLE_LEN,
    );

    DmEmbedContent {
        title,
        description,
        color: 0x15C253,
        fields: vec![
            (
                truncate_str("🗺️ Mapa", MAX_FIELD_NAME_LEN),
                truncate_str(&info.map_name, MAX_FIELD_VALUE_LEN),
            ),
            (
                truncate_str("👥 Equipo", MAX_FIELD_NAME_LEN),
                truncate_str(&team.name, MAX_FIELD_VALUE_LEN),
            ),
            (
                truncate_str("👑 Capitán", MAX_FIELD_NAME_LEN),
                truncate_str(&cap_val, MAX_FIELD_VALUE_LEN),
            ),
        ],
    }
}

/// Implementación real del notificador de partidas que interactúa con la API de Discord.
pub struct DiscordMatchNotifier {
    /// Cliente HTTP de Serenity para enviar mensajes y abrir canales DM.
    pub http: Arc<serenity::http::Http>,
    /// Identificador numérico opcional del canal de partidas.
    pub matches_channel_id: Option<String>,
}

impl DiscordMatchNotifier {
    /// Crea un nuevo notificador con el cliente HTTP y el canal de partidas opcional.
    pub fn new(http: Arc<serenity::http::Http>, matches_channel_id: Option<String>) -> Self {
        Self {
            http,
            matches_channel_id,
        }
    }

    /// Ejecuta las notificaciones de anuncio y mensajes directos.
    async fn send_notifications(&self, info: &MatchLaunchInfo) {
        // (a) Enviar anuncio al canal de partidas si está configurado
        if let Some(ref chan_str) = self.matches_channel_id {
            if let Ok(chan_num) = chan_str.parse::<u64>() {
                let channel_id = ChannelId::new(chan_num);
                let announcement = build_announcement(info);

                let mut embed = CreateEmbed::new()
                    .title(&announcement.title)
                    .description(&announcement.description)
                    .color(announcement.color);

                for (name, val) in &announcement.fields {
                    embed = embed.field(name, val, false);
                }

                let msg = CreateMessage::new()
                    .content(&announcement.content)
                    .embed(embed);

                if let Err(e) = channel_id.send_message(&self.http, msg).await {
                    tracing::warn!(
                        "[DiscordMatchNotifier] Error enviando anuncio de partida al canal {}: {e}",
                        chan_num
                    );
                }
            } else {
                tracing::warn!(
                    "[DiscordMatchNotifier] ID de canal de partidas no numérico: {chan_str}"
                );
            }
        }

        // (b) Enviar mensaje directo a cada uno de los 10 jugadores
        let teams = [&info.team_a, &info.team_b];
        let connect_line = build_dm_connect_line(&info.server_ip);

        for team in teams {
            for (discord_id, _nick) in &team.players {
                let uid = match discord_id.parse::<u64>() {
                    Ok(id) => id,
                    Err(_) => {
                        tracing::warn!(
                            "[DiscordMatchNotifier] Discord ID no numérico para DM: {discord_id}"
                        );
                        continue;
                    }
                };

                let user_id = UserId::new(uid);
                let dm_channel = match user_id.create_dm_channel(&self.http).await {
                    Ok(ch) => ch,
                    Err(e) => {
                        tracing::warn!(
                            "[DiscordMatchNotifier] No se pudo abrir canal DM con usuario {discord_id}: {e}"
                        );
                        continue;
                    }
                };

                let dm_content = build_player_dm(info, team, discord_id);
                let mut embed = CreateEmbed::new()
                    .title(&dm_content.title)
                    .description(&dm_content.description)
                    .color(dm_content.color);

                for (name, val) in &dm_content.fields {
                    embed = embed.field(name, val, true);
                }

                let msg = CreateMessage::new().content(&connect_line).embed(embed);

                if let Err(e) = dm_channel.send_message(&self.http, msg).await {
                    tracing::warn!(
                        "[DiscordMatchNotifier] Error enviando mensaje directo a {discord_id}: {e}"
                    );
                }
            }
        }
    }
}

impl MatchNotifier for DiscordMatchNotifier {
    fn match_launched<'a>(
        &'a self,
        info: &'a MatchLaunchInfo,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.send_notifications(info).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_channel_id_various_cases() {
        // Vacío y espacios
        assert_eq!(pick_channel_id(""), None);
        assert_eq!(pick_channel_id("   "), None);
        assert_eq!(pick_channel_id(",, , "), None);

        // Con prefijo ID_
        assert_eq!(pick_channel_id("ID_MATCHES"), None);
        assert_eq!(pick_channel_id("ID_12345"), None);

        // Lista con ID_ al inicio y numérico después
        assert_eq!(
            pick_channel_id("ID_MATCHES, 123456789012345678"),
            Some("123456789012345678".to_string())
        );

        // Varios números válidos: toma el primero
        assert_eq!(
            pick_channel_id("111222333, 444555666"),
            Some("111222333".to_string())
        );

        // Espacios alrededor de los valores
        assert_eq!(
            pick_channel_id("  ID_CANAL  ,  987654321  ,  12345 "),
            Some("987654321".to_string())
        );

        // Valores alfanuméricos no numéricos ignorados
        assert_eq!(
            pick_channel_id("canal_general, 555666777"),
            Some("555666777".to_string())
        );
    }

    #[test]
    fn test_build_dm_connect_line() {
        assert_eq!(
            build_dm_connect_line("192.168.1.50:27015"),
            "connect 192.168.1.50:27015"
        );
        assert_eq!(
            build_dm_connect_line("cs2.machupeek.com:27020"),
            "connect cs2.machupeek.com:27020"
        );
    }

    #[test]
    fn test_format_games_label() {
        assert_eq!(format_games_label(1), "1 Juego (Al mejor de 1)");
        assert_eq!(format_games_label(3), "3 Juegos (Al mejor de 3 - Gana 2)");
        assert_eq!(format_games_label(5), "5 Juegos (Al mejor de 5 - Gana 3)");
    }

    fn sample_match_info(bet: f64) -> MatchLaunchInfo {
        MatchLaunchInfo {
            guild_id: "guild_123".to_string(),
            match_id: "mixton_99999_abc".to_string(),
            server_ip: "144.22.50.1:27015".to_string(),
            map_name: "Mirage".to_string(),
            games_per_match: 1,
            bet_per_player: bet,
            initiator_username: "AdminUser".to_string(),
            team_a: TeamNotice {
                name: "Equipo Alfa".to_string(),
                captain_discord_id: "1001".to_string(),
                players: vec![
                    ("1001".to_string(), "CapitanA".to_string()),
                    ("1002".to_string(), "PlayerA2".to_string()),
                    ("1003".to_string(), "PlayerA3".to_string()),
                    ("1004".to_string(), "PlayerA4".to_string()),
                    ("1005".to_string(), "PlayerA5".to_string()),
                ],
            },
            team_b: TeamNotice {
                name: "Equipo Beta".to_string(),
                captain_discord_id: "2001".to_string(),
                players: vec![
                    ("2001".to_string(), "CapitanB".to_string()),
                    ("2002".to_string(), "PlayerB2".to_string()),
                    ("2003".to_string(), "PlayerB3".to_string()),
                    ("2004".to_string(), "PlayerB4".to_string()),
                    ("2005".to_string(), "PlayerB5".to_string()),
                ],
            },
        }
    }

    #[test]
    fn test_build_announcement_without_bet() {
        let info = sample_match_info(0.0);
        let announcement = build_announcement(&info);

        assert_eq!(announcement.title, "📢 ¡Partida Mixtón Iniciada!");
        assert_eq!(announcement.color, 0xFF5500); // Naranja sin apuesta

        // No debe contener el campo de apuesta
        assert!(!announcement
            .fields
            .iter()
            .any(|(name, _)| name == "💰 Apuesta y Pozo"));

        // Campos requeridos
        let field_names: Vec<&str> = announcement
            .fields
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(field_names.contains(&"🌐 Formato"));
        assert!(field_names.contains(&"🗺️ Mapa"));
        assert!(field_names.contains(&"🖥️ Servidor CS2 Dedicado"));
        assert!(field_names.contains(&"👥 Equipo Alfa"));
        assert!(field_names.contains(&"👥 Equipo Beta"));

        // Verificación de menciones en el contenido
        for id in [
            "1001", "1002", "1003", "1004", "1005", "2001", "2002", "2003", "2004", "2005",
        ] {
            assert!(announcement.content.contains(&format!("<@{id}>")));
        }

        // Bloque connect en la descripción y campo CS2
        assert!(announcement
            .description
            .contains("connect 144.22.50.1:27015"));
        let cs2_field = announcement
            .fields
            .iter()
            .find(|(n, _)| n == "🖥️ Servidor CS2 Dedicado")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(cs2_field.contains("connect 144.22.50.1:27015"));

        // Capitanes con corona y miembros con viñeta
        let team_a_field = announcement
            .fields
            .iter()
            .find(|(n, _)| n == "👥 Equipo Alfa")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(team_a_field.contains("👑 <@1001> (`CapitanA`)"));
        assert!(team_a_field.contains("• <@1002> (`PlayerA2`)"));
    }

    #[test]
    fn test_build_announcement_with_bet() {
        let info = sample_match_info(5.0);
        let announcement = build_announcement(&info);

        assert_eq!(announcement.color, 0x15C253); // Verde con apuesta

        let bet_field = announcement
            .fields
            .iter()
            .find(|(n, _)| n == "💰 Apuesta y Pozo")
            .map(|(_, v)| v.as_str());
        assert!(bet_field.is_some());
        let val = bet_field.unwrap();
        assert!(val.contains("Cuota:** S/ 5.00"));
        assert!(val.contains("Pozo Total:** S/ 50.00"));
    }

    #[test]
    fn test_build_announcement_length_limits() {
        let mut info = sample_match_info(10.0);
        info.initiator_username = "X".repeat(5000);
        info.map_name = "Y".repeat(2000);
        info.team_a.name = "Z".repeat(500);

        let announcement = build_announcement(&info);
        assert!(announcement.title.chars().count() <= MAX_TITLE_LEN);
        assert!(announcement.description.chars().count() <= MAX_DESCRIPTION_LEN);
        assert!(announcement.content.chars().count() <= MAX_CONTENT_LEN);

        for (name, val) in &announcement.fields {
            assert!(name.chars().count() <= MAX_FIELD_NAME_LEN);
            assert!(val.chars().count() <= MAX_FIELD_VALUE_LEN);
        }
    }

    #[test]
    fn test_build_player_dm() {
        let info = sample_match_info(0.0);

        // Para el capitán
        let dm_cap = build_player_dm(&info, &info.team_a, "1001");
        assert!(dm_cap.title.contains("Equipo Alfa"));
        assert!(dm_cap
            .description
            .contains("👑 Eres el **Capitán/Representante**."));
        assert!(dm_cap.description.contains("connect 144.22.50.1:27015"));
        let cap_field = dm_cap
            .fields
            .iter()
            .find(|(n, _)| n == "👑 Capitán")
            .unwrap();
        assert_eq!(cap_field.1, "Tú eres el capitán");

        // Para un jugador regular
        let dm_player = build_player_dm(&info, &info.team_a, "1002");
        assert!(dm_player
            .description
            .contains("👑 Tu capitán es **<@1001>**."));
        let cap_field_reg = dm_player
            .fields
            .iter()
            .find(|(n, _)| n == "👑 Capitán")
            .unwrap();
        assert_eq!(cap_field_reg.1, "<@1001>");
    }
}
