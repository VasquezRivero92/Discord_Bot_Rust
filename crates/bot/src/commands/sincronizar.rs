//! Comando `/sincronizar_fc`: sincroniza manualmente los roles de Discord y nivel actual de Faceit.
//!
//! Replica el comportamiento de `legacy_ref/sincronizar.js` y `legacy_ref/roles.js`:
//! - Defer efímero.
//! - Validación de clave de API de Faceit.
//! - Búsqueda de vinculación en Firestore (documento global y de servidor) con `mixton_logic::merge_user_docs`.
//! - Consulta del perfil en Faceit vía `get_player_by_id`.
//! - Extracción de `skill_level` y `faceit_elo` de CS2 (o CS:GO).
//! - Sincronización de roles de nivel (`Faceit Level 1`..`10`) y apodo en el servidor.
//! - Actualización en Firestore del documento `{guild}_{id}` con `merge_doc`.
//! - Registro de auditoría en `faceit_bot_logs`.
//! - Mensajes exactos del legado.

use crate::commands::mixton_logic;
use crate::faceit::FaceitClient;
use data::FirestoreClient;
use serenity::all::*;
use tracing::{info, warn};

/// Extrae el nivel y el ELO de Faceit a partir del objeto JSON de un perfil de jugador.
/// Busca primero en `games.cs2` y luego en `games.csgo`.
pub fn extract_level_and_elo(player_json: &serde_json::Value) -> (Option<u8>, u32) {
    let games = player_json.get("games");
    let cs2 = games.and_then(|g| g.get("cs2"));
    let csgo = games.and_then(|g| g.get("csgo"));

    let level = cs2
        .and_then(|c| c.get("skill_level"))
        .or_else(|| csgo.and_then(|c| c.get("skill_level")))
        .and_then(|v| {
            v.as_u64()
                .map(|n| n as u8)
                .or_else(|| v.as_i64().filter(|n| *n > 0).map(|n| n as u8))
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u8>().ok()))
        });

    let elo = cs2
        .and_then(|c| c.get("faceit_elo"))
        .or_else(|| csgo.and_then(|c| c.get("faceit_elo")))
        .and_then(|v| {
            v.as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_i64().filter(|n| *n > 0).map(|n| n as u32))
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u32>().ok()))
        })
        .unwrap_or(0);

    (level, elo)
}

/// Errores tipados con selección de mensaje de usuario exacto del legado.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncError {
    MissingApiKey,
    NotLinked,
    NoLevel(String),
    Generic(String),
}

impl SyncError {
    pub fn user_message(&self) -> String {
        match self {
            Self::MissingApiKey => {
                "❌ El administrador no ha configurado la API Key de Faceit.".to_string()
            }
            Self::NotLinked => {
                "❌ No tienes una cuenta de Faceit vinculada. Usa `/verificar_fc` primero o vincúlala desde la web.".to_string()
            }
            Self::NoLevel(nick) => {
                format!("❌ No se pudo encontrar un nivel activo en Faceit para el perfil **{nick}**.")
            }
            Self::Generic(err) => {
                format!("❌ Ocurrió un error al intentar sincronizar tu cuenta: {err}")
            }
        }
    }
}

/// Mensaje de éxito formateado con los datos sincronizados del jugador.
pub fn format_sync_success_message(
    discord_id: &str,
    faceit_nick: &str,
    level: u8,
    elo: u32,
) -> String {
    format!(
        "✅ ¡Sincronización completa!\n\nTu cuenta de Discord <@{discord_id}> está sincronizada con el perfil de Faceit **{faceit_nick}**.\n🎮 **Nivel Faceit:** Level {level}\n⚡ **ELO:** {elo} ELO"
    )
}

/// Color representativo de cada nivel de Faceit según el legado.
fn level_role_color(level: u8) -> u32 {
    match level {
        1 => 0xEEEEEE,
        2..=3 => 0x43A047,
        4..=7 => 0xFB8C00,
        8..=9 => 0xE53935,
        10 => 0xFF1744,
        _ => 0xFFFFFF,
    }
}

/// Obtiene o crea el rol `Faceit Level {level}` en el servidor de Discord.
async fn get_or_create_level_role(
    ctx: &Context,
    guild_id: GuildId,
    level: u8,
) -> Result<Option<RoleId>, serenity::Error> {
    let role_name = format!("Faceit Level {level}");
    let roles = ctx.http.get_guild_roles(guild_id).await?;

    if let Some(existing) = roles.iter().find(|r| r.name == role_name) {
        return Ok(Some(existing.id));
    }

    let builder = EditRole::new()
        .name(&role_name)
        .colour(Colour(level_role_color(level)));

    match guild_id.create_role(&ctx.http, builder).await {
        Ok(created) => {
            info!("Rol creado: {}", role_name);
            Ok(Some(created.id))
        }
        Err(err) => {
            warn!("No se pudo crear el rol {}: {}", role_name, err);
            Ok(None)
        }
    }
}

/// Sincroniza el rol de nivel de Faceit asignado al miembro.
async fn sync_member_level_role(
    ctx: &Context,
    guild_id: GuildId,
    member_id: UserId,
    level: u8,
) -> Result<(), serenity::Error> {
    let target_role_id = match get_or_create_level_role(ctx, guild_id, level).await? {
        Some(id) => id,
        None => return Ok(()),
    };

    let member = match ctx.http.get_member(guild_id, member_id).await {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };

    let roles = ctx.http.get_guild_roles(guild_id).await.unwrap_or_default();

    // Identificar roles de nivel 1..10 a remover (todos excepto el nivel actual)
    for l in 1..=10 {
        if l == level {
            continue;
        }
        let name = format!("Faceit Level {l}");
        if let Some(r) = roles.iter().find(|role| role.name == name) {
            if member.roles.contains(&r.id) {
                let _ = ctx
                    .http
                    .remove_member_role(
                        guild_id,
                        member_id,
                        r.id,
                        Some("Sincronización de nivel Faceit"),
                    )
                    .await;
            }
        }
    }

    if !member.roles.contains(&target_role_id) {
        let _ = ctx
            .http
            .add_member_role(
                guild_id,
                member_id,
                target_role_id,
                Some("Sincronización de nivel Faceit"),
            )
            .await;
        info!("Rol Faceit Level {} asignado a <@{}>", level, member_id);
    }

    Ok(())
}

/// Sincroniza el apodo del miembro con su nickname de Faceit.
async fn sync_member_nickname(
    ctx: &Context,
    guild_id: GuildId,
    member_id: UserId,
    faceit_nickname: &str,
) {
    if faceit_nickname.trim().is_empty() {
        return;
    }

    let member = match ctx.http.get_member(guild_id, member_id).await {
        Ok(m) => m,
        Err(_) => return,
    };

    if member.nick.as_deref() == Some(faceit_nickname) {
        return;
    }

    let edit = EditMember::new().nickname(faceit_nickname);
    if let Err(err) = guild_id.edit_member(&ctx.http, member_id, edit).await {
        warn!(
            "[NICK SYNC] No se pudo cambiar el apodo de {}: {}",
            member_id, err
        );
    }
}

/// Registro del comando slash en Discord.
pub fn register() -> CreateCommand {
    CreateCommand::new("sincronizar_fc")
        .description("Sincroniza manualmente tus roles de Discord con tu nivel actual de Faceit")
}

/// Ejecución principal del comando `/sincronizar_fc`.
pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    faceit: Option<&FaceitClient>,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    // 1. Defer efímero
    interaction.defer_ephemeral(&ctx.http).await?;

    let discord_id = interaction.user.id.to_string();
    let guild_str = interaction.guild_id.map(|g| g.to_string());

    // 2. Verificar FaceitClient configurado
    let faceit = match faceit {
        Some(f) => f,
        None => {
            let msg =
                EditInteractionResponse::new().content(SyncError::MissingApiKey.user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    // 3. Verificar FirestoreClient configurado
    let fs = match firestore {
        Some(f) => f,
        None => {
            let msg = EditInteractionResponse::new()
                .content("❌ No se pudo conectar a la base de datos.");
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    // 4. Buscar vinculación global del jugador
    let global_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &discord_id)
        .await
        .ok()
        .flatten();

    let merged = mixton_logic::merge_user_docs(global_doc.as_ref(), None);
    if !mixton_logic::is_user_verified(&merged) {
        let msg = EditInteractionResponse::new().content(SyncError::NotLinked.user_message());
        interaction.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    let faceit_player_id = match merged.get("faceit_player_id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim(),
        _ => {
            let msg = EditInteractionResponse::new().content(SyncError::NotLinked.user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let faceit_nick = merged
        .get("faceit_nickname")
        .and_then(|v| v.as_str())
        .unwrap_or("Desconocido");

    // 5. Consultar perfil en Faceit
    let player = match faceit.get_player_by_id(faceit_player_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            let msg = EditInteractionResponse::new()
                .content(SyncError::NoLevel(faceit_nick.to_string()).user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
        Err(err) => {
            let msg = EditInteractionResponse::new()
                .content(SyncError::Generic(err.to_string()).user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    // 6. Extraer nivel y elo
    let player_val = match serde_json::to_value(&player) {
        Ok(v) => v,
        Err(err) => {
            let msg = EditInteractionResponse::new()
                .content(SyncError::Generic(err.to_string()).user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let (level_opt, elo) = extract_level_and_elo(&player_val);
    let level = match level_opt {
        Some(l) if l > 0 => l,
        _ => {
            let msg = EditInteractionResponse::new()
                .content(SyncError::NoLevel(faceit_nick.to_string()).user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let player_actual_nick = if player.nickname.trim().is_empty() {
        faceit_nick
    } else {
        player.nickname.as_str()
    };

    // 7. Sincronizar roles y apodo en Discord si se ejecuta dentro de un servidor
    if let Some(gid) = interaction.guild_id {
        let _ = sync_member_level_role(ctx, gid, interaction.user.id, level).await;
        sync_member_nickname(ctx, gid, interaction.user.id, player_actual_nick).await;
    }

    // 8. Actualizar documento global del jugador con merge_doc
    let patch = serde_json::json!({
        "faceit_level": level,
        "faceit_elo": elo,
        "updatedAt": data::timestamp_now(),
    });
    if let Err(err) = fs.merge_doc("faceit_users", &discord_id, &patch).await {
        warn!(
            "Error al actualizar stats para {}: {}",
            discord_id, err
        );
    }

    // 9. Registrar en faceit_bot_logs
    let user_tag = format!("{} ({})", interaction.user.name, interaction.user.id);
    let log_entry = serde_json::json!({
        "level": "info",
        "message": format!(
            "Usuario {} sincronizó manualmente su nivel Faceit ({}) y roles.",
            user_tag, level
        ),
        "source": "bot",
        "guild_id": guild_str,
        "createdAt": data::timestamp_now(),
    });
    let log_id = format!(
        "{}_{}",
        chrono::Utc::now().timestamp_millis(),
        interaction.user.id
    );
    let _ = fs.set_doc("faceit_bot_logs", &log_id, &log_entry).await;

    // 10. Responder con mensaje de éxito exacto
    let msg = EditInteractionResponse::new().content(format_sync_success_message(
        &discord_id,
        player_actual_nick,
        level,
        elo,
    ));
    interaction.edit_response(&ctx.http, msg).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_level_and_elo_cs2() {
        let player = json!({
            "games": {
                "cs2": {
                    "skill_level": 8,
                    "faceit_elo": 1850
                }
            }
        });
        let (lvl, elo) = extract_level_and_elo(&player);
        assert_eq!(lvl, Some(8));
        assert_eq!(elo, 1850);
    }

    #[test]
    fn test_extract_level_and_elo_csgo_fallback() {
        let player = json!({
            "games": {
                "csgo": {
                    "skill_level": 5,
                    "faceit_elo": 1200
                }
            }
        });
        let (lvl, elo) = extract_level_and_elo(&player);
        assert_eq!(lvl, Some(5));
        assert_eq!(elo, 1200);
    }

    #[test]
    fn test_extract_level_and_elo_missing() {
        let player = json!({
            "games": {}
        });
        let (lvl, elo) = extract_level_and_elo(&player);
        assert_eq!(lvl, None);
        assert_eq!(elo, 0);
    }

    #[test]
    fn test_sync_error_user_messages() {
        assert_eq!(
            SyncError::MissingApiKey.user_message(),
            "❌ El administrador no ha configurado la API Key de Faceit."
        );
        assert_eq!(
            SyncError::NotLinked.user_message(),
            "❌ No tienes una cuenta de Faceit vinculada. Usa `/verificar_fc` primero o vincúlala desde la web."
        );
        assert_eq!(
            SyncError::NoLevel("shroud".into()).user_message(),
            "❌ No se pudo encontrar un nivel activo en Faceit para el perfil **shroud**."
        );
        assert_eq!(
            SyncError::Generic("timeout".into()).user_message(),
            "❌ Ocurrió un error al intentar sincronizar tu cuenta: timeout"
        );
    }

    #[test]
    fn test_format_sync_success_message() {
        let msg = format_sync_success_message("12345", "s1mple", 10, 3100);
        let expected = "✅ ¡Sincronización completa!\n\nTu cuenta de Discord <@12345> está sincronizada con el perfil de Faceit **s1mple**.\n🎮 **Nivel Faceit:** Level 10\n⚡ **ELO:** 3100 ELO";
        assert_eq!(msg, expected);
    }
}
