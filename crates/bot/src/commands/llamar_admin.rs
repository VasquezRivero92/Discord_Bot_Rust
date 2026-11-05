//! Comando `/llamar_admin` y flujo de soporte en Discord.
//!
//! Replica la lógica de `legacy_ref/llamarAdmin.js` y `legacy_ref/interactionCreate.js` (172-266, 432-560):
//! - Búsqueda de vinculación Faceit del usuario.
//! - Búsqueda de partida activa en `faceit_active_matches` mediante `roster_contains_player`.
//! - Notificación vía DM a los administradores del servidor.
//! - Soporte interactivo: apertura de modal `support_call_modal_{matchId}`, registro en
//!   `faceit_support_calls` y botón de reclamo `admin_respond_btn_{supportId}`.

use crate::commands::mixton_logic;
use common::config::Config;
use data::FirestoreClient;
use serde::{Deserialize, Serialize};
use serenity::all::*;
use tracing::{info, warn};

/// Partida activa leída de la colección `faceit_active_matches`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ActiveMatch {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub faction1_name: String,
    #[serde(default)]
    pub faction2_name: String,
    #[serde(default)]
    pub faction1_roster: Vec<serde_json::Value>,
    #[serde(default)]
    pub faction2_roster: Vec<serde_json::Value>,
    #[serde(default)]
    pub map: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

/// Comprueba si una lista de jugadores de una facción contiene al jugador indicado.
/// Soporta tanto identificadores como strings simples o como objetos `{ "id": "..." }` o `{ "player_id": "..." }`.
pub fn roster_contains_player(roster: &[serde_json::Value], player_id: &str) -> bool {
    let target = player_id.trim();
    if target.is_empty() {
        return false;
    }
    roster.iter().any(|p| match p {
        serde_json::Value::String(id) => id.trim() == target,
        serde_json::Value::Object(map) => {
            map.get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.trim() == target)
                .unwrap_or(false)
                || map
                    .get("player_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim() == target)
                    .unwrap_or(false)
        }
        _ => false,
    })
}

/// Función pura que busca la partida activa correspondiente al jugador dentro de una lista de partidas.
pub fn find_active_match_by_player_id<'a>(
    matches: &'a [ActiveMatch],
    player_id: &str,
) -> Option<&'a ActiveMatch> {
    matches.iter().find(|m| {
        roster_contains_player(&m.faction1_roster, player_id)
            || roster_contains_player(&m.faction2_roster, player_id)
    })
}

/// Errores tipados con mensajes de usuario exactos del legado para llamadas a administrador.
#[derive(Debug, PartialEq, Eq)]
pub enum AdminCallError {
    NotLinked,
    NoActiveMatch,
    GuildOnly,
    NoAdminsAvailable,
    Generic,
}

impl AdminCallError {
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NotLinked => {
                "❌ No tienes una cuenta de Faceit vinculada. Usa `/verificar_fc` o conéctala desde la web."
            }
            Self::NoActiveMatch => "❌ No estás en ninguna partida activa en este momento.",
            Self::GuildOnly => "❌ Este comando solo funciona dentro de un servidor.",
            Self::NoAdminsAvailable => "⚠️ No hay admins disponibles para atender tu solicitud.",
            Self::Generic => "❌ Error al procesar tu solicitud. Intenta de nuevo.",
        }
    }
}

/// Registro del comando slash `/llamar_admin` en Discord.
pub fn register() -> CreateCommand {
    CreateCommand::new("llamar_admin")
        .description("Solicita un admin para tu partida activa en Faceit")
}

/// Ejecución principal del comando `/llamar_admin`.
pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    config: &Config,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    // 1. Defer efímero
    interaction.defer_ephemeral(&ctx.http).await?;

    let guild_id = match interaction.guild_id {
        Some(gid) => gid,
        None => {
            let msg =
                EditInteractionResponse::new().content(AdminCallError::GuildOnly.user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let fs = match firestore {
        Some(f) => f,
        None => {
            let msg =
                EditInteractionResponse::new().content(AdminCallError::Generic.user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let discord_id = interaction.user.id.to_string();
    let guild_str = guild_id.to_string();

    // 2. Obtener faceit_player_id del usuario desde Firestore
    let global_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &discord_id)
        .await
        .ok()
        .flatten();

    let server_doc_id = format!("{guild_str}_{discord_id}");
    let server_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &server_doc_id)
        .await
        .ok()
        .flatten();

    let merged = mixton_logic::merge_user_docs(global_doc.as_ref(), server_doc.as_ref());
    if !mixton_logic::is_user_verified(&merged) {
        let msg = EditInteractionResponse::new().content(AdminCallError::NotLinked.user_message());
        interaction.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    let faceit_player_id = match merged.get("faceit_player_id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim(),
        _ => {
            let msg =
                EditInteractionResponse::new().content(AdminCallError::NotLinked.user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let faceit_nick = merged
        .get("faceit_nickname")
        .and_then(|v| v.as_str())
        .unwrap_or("Desconocido");

    // 3. Buscar partida activa del jugador en faceit_active_matches
    let active_matches_raw = match fs
        .list_docs::<serde_json::Value>("faceit_active_matches")
        .await
    {
        Ok(m) => m,
        Err(err) => {
            warn!("Error al listar faceit_active_matches: {}", err);
            Vec::new()
        }
    };

    let matches: Vec<ActiveMatch> = active_matches_raw
        .into_iter()
        .filter_map(|(id, val)| {
            let mut match_obj: ActiveMatch = serde_json::from_value(val).ok()?;
            if match_obj.id.is_empty() {
                match_obj.id = id;
            }
            Some(match_obj)
        })
        .collect();

    let active_match = match find_active_match_by_player_id(&matches, faceit_player_id) {
        Some(m) => m.clone(),
        None => {
            let msg = EditInteractionResponse::new()
                .content(AdminCallError::NoActiveMatch.user_message());
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    // 4. Buscar administradores en el servidor
    let guild_roles = ctx.http.get_guild_roles(guild_id).await.unwrap_or_default();
    let guild_members = ctx
        .http
        .get_guild_members(guild_id, Some(1000), None)
        .await
        .unwrap_or_default();

    let target_admin_role = if let Some(ref role_cfg) = config.admin_role_id {
        guild_roles
            .iter()
            .find(|r| r.id.to_string() == *role_cfg || r.name.eq_ignore_ascii_case(role_cfg))
    } else {
        None
    };

    let admin_members: Vec<&Member> = if let Some(role) = target_admin_role {
        let members: Vec<&Member> = guild_members
            .iter()
            .filter(|m| m.roles.contains(&role.id))
            .collect();
        if !members.is_empty() {
            members
        } else {
            guild_members
                .iter()
                .filter(|m| {
                    m.permissions
                        .map_or(false, |p| p.contains(Permissions::ADMINISTRATOR))
                        || guild_roles
                            .iter()
                            .filter(|r| m.roles.contains(&r.id))
                            .any(|r| r.permissions.contains(Permissions::ADMINISTRATOR))
                })
                .collect()
        }
    } else {
        guild_members
            .iter()
            .filter(|m| {
                m.permissions
                    .map_or(false, |p| p.contains(Permissions::ADMINISTRATOR))
                    || guild_roles
                        .iter()
                        .filter(|r| m.roles.contains(&r.id))
                        .any(|r| r.permissions.contains(Permissions::ADMINISTRATOR))
            })
            .collect()
    };

    if admin_members.is_empty() {
        let msg = EditInteractionResponse::new()
            .content(AdminCallError::NoAdminsAvailable.user_message());
        interaction.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    // 5. Enviar DM a cada administrador con el embed exacto del legado
    let guild_name = ctx
        .cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Servidor".to_string());
    let guild_icon = ctx.cache.guild(guild_id).and_then(|g| g.icon_url());

    let match_url = format!("https://www.faceit.com/en/cs2/room/{}", active_match.id);
    let map_name = active_match.map.as_deref().unwrap_or("N/A");
    let location = active_match
        .location
        .as_deref()
        .unwrap_or("N/A")
        .to_uppercase();

    let user_tag = format!("{} ({})", interaction.user.name, interaction.user.id);
    let mut embed = CreateEmbed::new()
        .title(format!("🚨 Solicitud de Admin — {guild_name}"))
        .color(0xFF0000)
        .description(format!(
            "El jugador **{user_tag}** ({faceit_nick}) ha solicitado un admin en su partida en el servidor **{guild_name}**."
        ))
        .field("🏛️ Servidor", &guild_name, true)
        .field("👤 Jugador", format!("<@{}>", interaction.user.id), true)
        .field("💬 Canal", format!("<#{}>", interaction.channel_id), true)
        .field(
            "🔗 Partida",
            format!("[Hacer clic aquí]({match_url})"),
            false,
        )
        .field(
            "📋 Detalles",
            format!(
                "**{}** vs **{}**\n🗺️ Mapa: `{map_name}`\n📍 Servidor: `{location}`",
                active_match.faction1_name, active_match.faction2_name
            ),
            false,
        )
        .footer(CreateEmbedFooter::new(format!(
            "Solicitud originada en: {guild_name}"
        )))
        .timestamp(Timestamp::now());

    if let Some(ref icon) = guild_icon {
        embed = embed.author(CreateEmbedAuthor::new(&guild_name).icon_url(icon));
    } else {
        embed = embed.author(CreateEmbedAuthor::new(&guild_name));
    }

    let mut sent_count = 0;
    for admin in &admin_members {
        let dm_msg = CreateMessage::new().embed(embed.clone());
        if admin.user.direct_message(&ctx.http, dm_msg).await.is_ok() {
            sent_count += 1;
        }
    }

    info!("Solicitud de admin enviada a {} admins.", sent_count);

    // 6. Registro de advertencia en faceit_bot_logs
    let log_entry = serde_json::json!({
        "level": "warn",
        "message": format!(
            "Solicitud de admin de {} en partida {}. DMs enviados: {}/{}",
            user_tag, active_match.id, sent_count, admin_members.len()
        ),
        "source": "bot",
        "guild_id": Some(guild_str),
        "createdAt": data::timestamp_now(),
    });
    let log_id = format!(
        "{}_{}",
        chrono::Utc::now().timestamp_millis(),
        interaction.user.id
    );
    let _ = fs.set_doc("faceit_bot_logs", &log_id, &log_entry).await;

    // 7. Responder al usuario
    let msg = EditInteractionResponse::new().content(format!(
        "✅ Se ha notificado a **{} admin(s)**. Te atenderán pronto.",
        sent_count
    ));
    interaction.edit_response(&ctx.http, msg).await?;

    Ok(())
}

// -----------------------------------------------------------------------------
// PARTE 3 — Flujo de Soporte Interactivo (Modal, Botón de Reclamo y Alertas)
// -----------------------------------------------------------------------------

/// Verifica si el usuario tiene alguna solicitud de soporte pendiente en `faceit_support_calls`.
async fn has_pending_support_call(fs: &FirestoreClient, discord_id: &str) -> bool {
    let calls = match fs
        .list_docs::<serde_json::Value>("faceit_support_calls")
        .await
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    calls.iter().any(|(_, call)| {
        let matches_user = call
            .get("discord_id")
            .and_then(|v| v.as_str())
            .map(|id| id == discord_id)
            .unwrap_or(false);
        let is_pending = call
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s == "pendiente")
            .unwrap_or(false);
        matches_user && is_pending
    })
}

/// Comprueba si un miembro tiene permisos de administración o el rol configurado.
fn check_member_is_admin(member: &Member, guild_roles: &[Role], config: &Config) -> bool {
    if let Some(perms) = member.permissions {
        if perms.contains(Permissions::ADMINISTRATOR) {
            return true;
        }
    }

    if guild_roles
        .iter()
        .filter(|r| member.roles.contains(&r.id))
        .any(|r| r.permissions.contains(Permissions::ADMINISTRATOR))
    {
        return true;
    }

    if let Some(ref r_cfg) = config.admin_role_id {
        if let Ok(id) = r_cfg.parse::<u64>() {
            if member.roles.contains(&RoleId::new(id)) {
                return true;
            }
        }
        if guild_roles
            .iter()
            .filter(|r| member.roles.contains(&r.id))
            .any(|r| r.name.eq_ignore_ascii_case(r_cfg))
        {
            return true;
        }
    }

    false
}

/// Manejador del botón `llamar_admin_btn`: verifica estado y abre el modal `support_call_modal_{matchId}`.
pub async fn handle_llamar_admin_button(
    ctx: &Context,
    component: &ComponentInteraction,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    let fs = match firestore {
        Some(f) => f,
        None => {
            let resp = CreateInteractionResponseMessage::new()
                .content("❌ No se pudo conectar a la base de datos.")
                .ephemeral(true);
            return component
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    let discord_id = component.user.id.to_string();

    // 1. Verificar si ya tiene una solicitud pendiente
    if has_pending_support_call(fs, &discord_id).await {
        let resp = CreateInteractionResponseMessage::new()
            .content("❌ Ya tienes una solicitud de soporte pendiente. Por favor, espera a que un administrador te atienda.")
            .ephemeral(true);
        return component
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
    }

    // 2. Obtener vinculación Faceit del usuario
    let guild_str = component.guild_id.map(|g| g.to_string());
    let global_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &discord_id)
        .await
        .ok()
        .flatten();

    let server_doc_id = match guild_str.as_deref() {
        Some(gid) => format!("{gid}_{discord_id}"),
        None => discord_id.clone(),
    };

    let server_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &server_doc_id)
        .await
        .ok()
        .flatten();

    let merged = mixton_logic::merge_user_docs(global_doc.as_ref(), server_doc.as_ref());
    if !mixton_logic::is_user_verified(&merged) {
        let resp = CreateInteractionResponseMessage::new()
            .content(
                "❌ No tienes una cuenta de Faceit vinculada. Usa `/verificar_fc` para vincularla.",
            )
            .ephemeral(true);
        return component
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
    }

    let faceit_player_id = match merged.get("faceit_player_id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim(),
        _ => {
            let resp = CreateInteractionResponseMessage::new()
                .content("❌ No tienes una cuenta de Faceit vinculada. Usa `/verificar_fc` para vincularla.")
                .ephemeral(true);
            return component
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    // 3. Buscar partida activa
    let active_matches_raw = match fs
        .list_docs::<serde_json::Value>("faceit_active_matches")
        .await
    {
        Ok(m) => m,
        Err(_) => Vec::new(),
    };

    let matches: Vec<ActiveMatch> = active_matches_raw
        .into_iter()
        .filter_map(|(id, val)| {
            let mut m: ActiveMatch = serde_json::from_value(val).ok()?;
            if m.id.is_empty() {
                m.id = id;
            }
            Some(m)
        })
        .collect();

    let active_match = match find_active_match_by_player_id(&matches, faceit_player_id) {
        Some(m) => m,
        None => {
            let resp = CreateInteractionResponseMessage::new()
                .content("❌ No estás en ninguna partida activa en este momento.")
                .ephemeral(true);
            return component
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    // 4. Crear y mostrar el modal
    let modal_id = format!("support_call_modal_{}", active_match.id);
    let input = CreateInputText::new(
        InputTextStyle::Paragraph,
        "Describe tu problema en la partida",
        "problem_description",
    )
    .placeholder("Ej: Jugador ausente (AFK), actitudes tóxicas, etc.")
    .min_length(10)
    .max_length(500)
    .required(true);

    let row = CreateActionRow::InputText(input);
    let modal = CreateModal::new(modal_id, "Llamar Administrador").components(vec![row]);

    component
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await
}

/// Manejador de envío de formulario modal `support_call_modal_{matchId}`.
pub async fn handle_support_call_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    config: &Config,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    let match_id = modal
        .data
        .custom_id
        .trim_start_matches("support_call_modal_")
        .to_string();

    let mut description = String::new();
    for row in &modal.data.components {
        for comp in &row.components {
            if let ActionRowComponent::InputText(it) = comp {
                if it.custom_id == "problem_description" {
                    if let Some(ref val) = it.value {
                        description = val.clone();
                    }
                }
            }
        }
    }

    modal.defer_ephemeral(&ctx.http).await?;

    let fs = match firestore {
        Some(f) => f,
        None => {
            let msg = EditInteractionResponse::new()
                .content("❌ Error al procesar tu solicitud. Intenta de nuevo.");
            modal.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let discord_id = modal.user.id.to_string();
    let guild_id = match modal.guild_id {
        Some(g) => g,
        None => {
            let msg = EditInteractionResponse::new()
                .content("❌ Este comando solo funciona dentro de un servidor.");
            modal.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };
    let guild_str = guild_id.to_string();

    // 1. Obtener datos de Faceit
    let global_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &discord_id)
        .await
        .ok()
        .flatten();

    let server_doc_id = format!("{guild_str}_{discord_id}");
    let server_doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &server_doc_id)
        .await
        .ok()
        .flatten();

    let merged = mixton_logic::merge_user_docs(global_doc.as_ref(), server_doc.as_ref());
    if !mixton_logic::is_user_verified(&merged) {
        let msg =
            EditInteractionResponse::new().content("❌ No tienes una cuenta de Faceit vinculada.");
        modal.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    let faceit_player_id = match merged.get("faceit_player_id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim(),
        _ => {
            let msg = EditInteractionResponse::new()
                .content("❌ No tienes una cuenta de Faceit vinculada.");
            modal.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let faceit_nick = merged
        .get("faceit_nickname")
        .and_then(|v| v.as_str())
        .unwrap_or("Desconocido");

    // 2. Obtener datos de la partida
    let active_match_raw = fs
        .get_doc::<serde_json::Value>("faceit_active_matches", &match_id)
        .await
        .ok()
        .flatten();

    let active_match: ActiveMatch = match active_match_raw {
        Some(v) => {
            let mut m: ActiveMatch = serde_json::from_value(v).unwrap_or_default();
            if m.id.is_empty() {
                m.id = match_id.clone();
            }
            m
        }
        None => {
            let msg =
                EditInteractionResponse::new().content("❌ La partida ya no se encuentra activa.");
            modal.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let match_url = format!("https://www.faceit.com/en/cs2/room/{}", active_match.id);

    // 3. Registrar llamada de soporte en faceit_support_calls
    let support_id = format!(
        "call_{}_{}",
        active_match.id,
        chrono::Utc::now().timestamp_millis()
    );
    let support_call_doc = serde_json::json!({
        "id": support_id,
        "match_id": active_match.id,
        "match_url": match_url,
        "faceit_player_id": faceit_player_id,
        "faceit_nickname": faceit_nick,
        "discord_id": discord_id,
        "description": description,
        "status": "pendiente",
        "claimedBy": null,
        "claimedAt": null,
        "guild_id": guild_str,
        "createdAt": data::timestamp_now(),
    });

    if let Err(err) = fs
        .set_doc("faceit_support_calls", &support_id, &support_call_doc)
        .await
    {
        warn!("Error al registrar llamada de soporte: {}", err);
        let msg = EditInteractionResponse::new()
            .content("❌ Error al procesar tu solicitud. Intenta de nuevo.");
        modal.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    // 4. Enviar alerta al canal de Staff
    let admin_alerts_channel_id = config
        .env_defaults
        .discord_admin_alerts_channel_id
        .as_deref()
        .filter(|s| !s.trim().is_empty() && !s.starts_with("ID_") && !s.starts_with("ID-"));

    let alerts_chan_id = match admin_alerts_channel_id.and_then(|s| s.parse::<u64>().ok()) {
        Some(id) => ChannelId::new(id),
        None => {
            let msg = EditInteractionResponse::new()
                .content("❌ El canal de alertas del staff no está configurado en este servidor.");
            modal.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let guild_name = ctx
        .cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Servidor".to_string());
    let guild_icon = ctx.cache.guild(guild_id).and_then(|g| g.icon_url());

    let map_name = active_match.map.as_deref().unwrap_or("N/A");
    let location = active_match
        .location
        .as_deref()
        .unwrap_or("N/A")
        .to_uppercase();

    let mut staff_embed = CreateEmbed::new()
        .title(format!("🚨 Solicitud de Admin (Soporte) — {guild_name}"))
        .color(0xFF0000)
        .description(format!(
            "El jugador <@{}> (**{}**) ha solicitado asistencia desde el panel de soporte de **{}**.",
            modal.user.id, faceit_nick, guild_name
        ))
        .field("🏛️ Servidor", &guild_name, true)
        .field("👤 Jugador", format!("<@{}>", modal.user.id), true)
        .field("💬 Canal de origen", format!("<#{}>", modal.channel_id), true)
        .field(
            "🔗 Partida (Enlace Directo)",
            format!("[Hacer clic aquí]({match_url})\n`{match_url}`"),
            false,
        )
        .field(
            "📝 Problema Reportado",
            format!("```\n{}\n```", description),
            false,
        )
        .field(
            "📋 Detalles de Partida",
            format!(
                "**{}** vs **{}**\n🗺️ Mapa: `{map_name}`\n📍 Servidor: `{location}`",
                active_match.faction1_name, active_match.faction2_name
            ),
            false,
        )
        .field("⚙️ Estado", "⏳ **Pendiente**", true)
        .footer(CreateEmbedFooter::new(format!("Servidor: {guild_name}")))
        .timestamp(Timestamp::now());

    if let Some(ref icon) = guild_icon {
        staff_embed = staff_embed.author(CreateEmbedAuthor::new(&guild_name).icon_url(icon));
    } else {
        staff_embed = staff_embed.author(CreateEmbedAuthor::new(&guild_name));
    }

    let claim_button = CreateButton::new(format!("admin_respond_btn_{}", support_id))
        .label("Atender Solicitud")
        .style(ButtonStyle::Success)
        .emoji('✅');

    let row = CreateActionRow::Buttons(vec![claim_button]);

    let ping_mention = if let Some(ref r_cfg) = config.admin_role_id {
        format!("<@&{r_cfg}>")
    } else {
        "@here".to_string()
    };

    let alert_msg = CreateMessage::new()
        .content(format!("⚠️ Atención {}", ping_mention))
        .embed(staff_embed)
        .components(vec![row]);

    if let Err(err) = alerts_chan_id.send_message(&ctx.http, alert_msg).await {
        warn!(
            "No se pudo enviar alerta de soporte al canal staff: {}",
            err
        );
        let msg = EditInteractionResponse::new()
            .content("❌ Error: El canal de alertas del staff no está accesible en este servidor.");
        modal.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    // 5. Responder al usuario
    let msg = EditInteractionResponse::new().content(
        "✅ **Solicitud enviada**. Nos estamos poniendo en contacto con un administrador. Te atenderán lo antes posible.",
    );
    modal.edit_response(&ctx.http, msg).await?;

    Ok(())
}

/// Manejador del botón `admin_respond_btn_{supportId}` para reclamar y atender una solicitud.
pub async fn handle_admin_respond_button(
    ctx: &Context,
    component: &ComponentInteraction,
    config: &Config,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    let support_id = component
        .data
        .custom_id
        .trim_start_matches("admin_respond_btn_")
        .to_string();

    let member = match component.member {
        Some(ref m) => m,
        None => {
            let resp = CreateInteractionResponseMessage::new()
                .content("❌ Solo los administradores pueden atender solicitudes.")
                .ephemeral(true);
            return component
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    let guild_id = match component.guild_id {
        Some(g) => g,
        None => {
            let resp = CreateInteractionResponseMessage::new()
                .content("❌ Solo se puede atender solicitudes dentro de un servidor.")
                .ephemeral(true);
            return component
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    let guild_roles = ctx.http.get_guild_roles(guild_id).await.unwrap_or_default();
    if !check_member_is_admin(member, &guild_roles, config) {
        let resp = CreateInteractionResponseMessage::new()
            .content("❌ Solo los administradores pueden atender solicitudes.")
            .ephemeral(true);
        return component
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
    }

    component.defer_ephemeral(&ctx.http).await?;

    let fs = match firestore {
        Some(f) => f,
        None => {
            let msg = EditInteractionResponse::new()
                .content("❌ No se pudo conectar a la base de datos.");
            component.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    // Obtener la solicitud desde Firestore
    let call_data = match fs
        .get_doc::<serde_json::Value>("faceit_support_calls", &support_id)
        .await
    {
        Ok(Some(doc)) => doc,
        _ => {
            let msg = EditInteractionResponse::new()
                .content("❌ No se encontró la información de esta solicitud en la base de datos.");
            component.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    if call_data
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s == "atendida")
        .unwrap_or(false)
    {
        let claimed_by = call_data
            .get("claimedBy")
            .and_then(|v| v.as_str())
            .unwrap_or("otro administrador");
        let msg = EditInteractionResponse::new().content(format!(
            "⚠️ Esta solicitud ya está siendo atendida por <@{claimed_by}>."
        ));
        component.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    // Actualizar en Firestore
    let patch = serde_json::json!({
        "status": "atendida",
        "claimedBy": component.user.id.to_string(),
        "claimedAt": chrono::Utc::now().timestamp_millis(),
    });
    let _ = fs
        .merge_doc("faceit_support_calls", &support_id, &patch)
        .await;

    // Eliminar el mensaje de alerta para que desaparezca
    let _ = component.message.delete(&ctx.http).await;

    let match_url = call_data
        .get("match_url")
        .and_then(|v| v.as_str())
        .unwrap_or("N/A");
    let description = call_data
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("No especificado");

    // Responder de forma efímera al administrador con el enlace de la partida
    let msg = EditInteractionResponse::new().content(format!(
        "✅ Has tomado la solicitud de soporte.\n🔗 **Enlace de la partida:** {match_url}\n📝 **Problema:** {description}"
    ));
    component.edit_response(&ctx.http, msg).await?;

    // Notificar al jugador por DM
    if let Some(player_discord_id) = call_data.get("discord_id").and_then(|v| v.as_str()) {
        if let Ok(player_id_u64) = player_discord_id.parse::<u64>() {
            let player_user_id = UserId::new(player_id_u64);
            if let Ok(player_user) = ctx.http.get_user(player_user_id).await {
                let server_name = ctx
                    .cache
                    .guild(guild_id)
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|| "Servidor".to_string());
                let admin_tag = format!("{} ({})", component.user.name, component.user.id);
                let dm_embed = CreateEmbed::new()
                    .title(format!("✅ Solicitud de Soporte Aceptada — {server_name}"))
                    .description(format!(
                        "Tu solicitud de soporte en el servidor **{server_name}** ha sido aceptada por el administrador **{admin_tag}**.\n\nSe pondrá en contacto contigo o se unirá al canal de voz de tu partida a la brevedad."
                    ))
                    .color(0x15C253)
                    .footer(CreateEmbedFooter::new(format!("Servidor: {server_name}")))
                    .timestamp(Timestamp::now());

                let dm_msg = CreateMessage::new().embed(dm_embed);
                let _ = player_user.direct_message(&ctx.http, dm_msg).await;
            }
        }
    }

    // Enviar log a faceit_bot_logs
    let admin_tag = format!("{} ({})", component.user.name, component.user.id);
    let log_entry = serde_json::json!({
        "level": "info",
        "message": format!("Solicitud {support_id} atendida por {admin_tag}"),
        "source": "bot",
        "guild_id": Some(guild_id.to_string()),
        "createdAt": data::timestamp_now(),
    });
    let log_id = format!(
        "{}_{}",
        chrono::Utc::now().timestamp_millis(),
        component.user.id
    );
    let _ = fs.set_doc("faceit_bot_logs", &log_id, &log_entry).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_roster_contains_player_strings_and_objects() {
        let roster = vec![
            json!("player_1"),
            json!({ "id": "player_2", "nickname": "p2" }),
            json!({ "player_id": "player_3" }),
        ];

        assert!(roster_contains_player(&roster, "player_1"));
        assert!(roster_contains_player(&roster, "player_2"));
        assert!(roster_contains_player(&roster, "player_3"));
        assert!(!roster_contains_player(&roster, "player_4"));
        assert!(!roster_contains_player(&roster, ""));
    }

    #[test]
    fn test_find_active_match_by_player_id() {
        let matches = vec![
            ActiveMatch {
                id: "match_100".into(),
                faction1_name: "Team A".into(),
                faction2_name: "Team B".into(),
                faction1_roster: vec![json!("p_alpha"), json!("p_beta")],
                faction2_roster: vec![json!("p_gamma")],
                map: Some("de_mirage".into()),
                location: Some("us".into()),
            },
            ActiveMatch {
                id: "match_200".into(),
                faction1_name: "Team C".into(),
                faction2_name: "Team D".into(),
                faction1_roster: vec![json!({"id": "p_delta"})],
                faction2_roster: vec![json!({"player_id": "p_omega"})],
                map: Some("de_inferno".into()),
                location: Some("br".into()),
            },
        ];

        let found_alpha = find_active_match_by_player_id(&matches, "p_alpha");
        assert_eq!(found_alpha.map(|m| m.id.as_str()), Some("match_100"));

        let found_gamma = find_active_match_by_player_id(&matches, "p_gamma");
        assert_eq!(found_gamma.map(|m| m.id.as_str()), Some("match_100"));

        let found_omega = find_active_match_by_player_id(&matches, "p_omega");
        assert_eq!(found_omega.map(|m| m.id.as_str()), Some("match_200"));

        let found_none = find_active_match_by_player_id(&matches, "p_unknown");
        assert!(found_none.is_none());
    }

    #[test]
    fn test_admin_call_error_user_messages() {
        assert_eq!(
            AdminCallError::NotLinked.user_message(),
            "❌ No tienes una cuenta de Faceit vinculada. Usa `/verificar_fc` o conéctala desde la web."
        );
        assert_eq!(
            AdminCallError::NoActiveMatch.user_message(),
            "❌ No estás en ninguna partida activa en este momento."
        );
        assert_eq!(
            AdminCallError::GuildOnly.user_message(),
            "❌ Este comando solo funciona dentro de un servidor."
        );
        assert_eq!(
            AdminCallError::NoAdminsAvailable.user_message(),
            "⚠️ No hay admins disponibles para atender tu solicitud."
        );
        assert_eq!(
            AdminCallError::Generic.user_message(),
            "❌ Error al procesar tu solicitud. Intenta de nuevo."
        );
    }
}
