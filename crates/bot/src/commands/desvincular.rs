//! Comando `/desvincular_fc`: desvincula la cuenta de Faceit de un usuario en Discord.
//!
//! Replica la lógica de `legacy_ref/desvincular.js`:
//! - Verificación de permisos de administrador (`checkIsAdmin`).
//! - Verificación de canal configurado (`discord_link_channel_id`).
//! - Desvinculación en Firestore usando [`data::DocSink`] (`delete_doc` del doc `{guild}_{id}`,
//!   un solo documento, como `db.unlinkAccount` del legado).
//! - Abandono de la cola del lobby (`lobby.leave_queue`) y limpieza con [`mixton_logic::clean_lobby_on_player_leave`].
//! - Remoción de roles asociados de Faceit mediante la API REST de Discord.
//! - Respuesta diferida y efímera con los mensajes de éxito y error exactos del legado.

use common::config::Config;
use data::DocSink;
use data::FirestoreClient;
use lobby::state::LobbyManager;
use serenity::all::*;
use std::sync::Arc;

/// Plan de desvinculación: igual que `db.unlinkAccount` del legado, borra UN solo documento de
/// `faceit_users`. Nunca se tocan los demás servidores ni el documento global si hay servidor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkPlan {
    /// Id del documento a borrar: `{guild_id}_{discord_id}`, o `discord_id` sin servidor.
    pub doc_id: String,
}

/// Función pura que determina qué documento se borra al desvincular.
pub fn compute_unlink_plan(guild_id: Option<&str>, discord_id: &str) -> UnlinkPlan {
    let doc_id = match guild_id {
        Some(gid) => format!("{gid}_{discord_id}"),
        None => discord_id.to_string(),
    };
    UnlinkPlan { doc_id }
}

/// Ejecuta la desvinculación sobre una implementación de [`data::DocSink`].
pub async fn execute_unlink_on_sink<S: DocSink + ?Sized>(
    sink: &S,
    plan: &UnlinkPlan,
) -> Result<(), data::DataError> {
    sink.delete_doc("faceit_users", &plan.doc_id).await
}

/// Valida si el canal donde se ejecutó el comando es el canal de vinculación permitido.
pub fn validate_link_channel(
    raw_channel_config: Option<&str>,
    current_channel_id: &str,
) -> Result<(), String> {
    let raw = match raw_channel_config {
        Some(s) if !s.trim().is_empty() && !s.starts_with("ID_") && !s.starts_with("ID-") => {
            s.trim()
        }
        _ => {
            return Err(
                "❌ El canal de vinculación no está configurado por el administrador en este servidor."
                    .to_string(),
            );
        }
    };

    let allowed_channels: Vec<&str> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with("ID_") && !s.starts_with("ID-"))
        .collect();

    if allowed_channels.is_empty() {
        return Err(
            "❌ El canal de vinculación configurado no pertenece a este servidor o no está disponible."
                .to_string(),
        );
    }

    if !allowed_channels.contains(&current_channel_id) {
        let mentions = allowed_channels
            .iter()
            .map(|id| format!("<#{id}>"))
            .collect::<Vec<_>>()
            .join(" o ");
        return Err(format!(
            "❌ Este comando solo se puede usar en el canal de vinculación de este servidor: {mentions}."
        ));
    }

    Ok(())
}

/// Comprueba si el usuario que ejecutó la interacción tiene permisos de administrador.
fn check_is_admin(interaction: &CommandInteraction, config: &Config) -> bool {
    if let Some(ref member) = interaction.member {
        if let Some(perms) = member.permissions {
            if perms.contains(Permissions::ADMINISTRATOR) {
                return true;
            }
        }

        // Revisar si el miembro tiene algún rol que coincida con el rol de administración configurado
        let mut admin_role_candidates = Vec::new();
        if let Some(ref r) = config.env_defaults.faceit_hub_role_id {
            admin_role_candidates.push(r.as_str());
        }

        for candidate in admin_role_candidates {
            if let Ok(id) = candidate.parse::<u64>() {
                if member.roles.contains(&RoleId::new(id)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Retira los roles asociados a Faceit del miembro objetivo en el servidor.
pub async fn remove_faceit_roles(
    ctx: &Context,
    guild_id: GuildId,
    member: &Member,
    config: &Config,
) -> u32 {
    let mut removed = 0;
    let mut role_ids_to_remove: Vec<RoleId> = Vec::new();

    // Roles configurados en variables de entorno / config
    if let Some(ref r) = config.env_defaults.discord_hub_role_id {
        if let Ok(id) = r.parse::<u64>() {
            role_ids_to_remove.push(RoleId::new(id));
        }
    }
    if let Some(ref r) = config.env_defaults.faceit_hub_role_id {
        if let Ok(id) = r.parse::<u64>() {
            role_ids_to_remove.push(RoleId::new(id));
        }
    }
    if let Some(ref r) = config.env_defaults.discord_match_access_role_id {
        if let Ok(id) = r.parse::<u64>() {
            role_ids_to_remove.push(RoleId::new(id));
        }
    }
    for q in &config.env_defaults.queues {
        if let Ok(id) = q.discord_hub_role_id.parse::<u64>() {
            role_ids_to_remove.push(RoleId::new(id));
        }
    }

    // Consultar todos los roles del servidor para detectar roles por nombre (Niveles de Faceit, Verificado)
    if let Ok(guild_roles) = ctx.http.get_guild_roles(guild_id).await {
        for role in guild_roles {
            let name_lower = role.name.to_lowercase();
            if name_lower.starts_with("nivel ")
                || name_lower.starts_with("level ")
                || name_lower == "verificado"
                || name_lower == "faceit"
                || name_lower.contains("club faceit")
            {
                role_ids_to_remove.push(role.id);
            }
        }
    }

    for role_id in role_ids_to_remove {
        if member.roles.contains(&role_id) {
            if ctx
                .http
                .remove_member_role(
                    guild_id,
                    member.user.id,
                    role_id,
                    Some("Desvinculación de Faceit"),
                )
                .await
                .is_ok()
            {
                removed += 1;
            }
        }
    }

    removed
}

/// Registro del comando slash en Discord.
pub fn register() -> CreateCommand {
    CreateCommand::new("desvincular_fc")
        .description("Desvincula a un usuario de su perfil de Faceit (Solo ADMIN)")
        .default_member_permissions(Permissions::ADMINISTRATOR)
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::User,
                "usuario",
                "El usuario de Discord a desvincular",
            )
            .required(true),
        )
}

/// Ejecución principal del comando `/desvincular_fc`.
pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    config: &Config,
    lobby: &Arc<LobbyManager>,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    // 1. Verificación de canal configurado (si está definido en configuración)
    if let Some(ref link_cfg) = config.env_defaults.discord_link_channel_id {
        if !link_cfg.starts_with("ID_") && !link_cfg.starts_with("ID-") {
            let channel_str = interaction.channel_id.to_string();
            if let Err(err_msg) = validate_link_channel(Some(link_cfg.as_str()), &channel_str) {
                let resp = CreateInteractionResponseMessage::new()
                    .content(err_msg)
                    .ephemeral(true);
                return interaction
                    .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                    .await;
            }
        }
    }

    // 2. Verificación de permisos de administrador
    if !check_is_admin(interaction, config) {
        let resp = CreateInteractionResponseMessage::new()
            .content("❌ No tienes permisos para usar este comando. Se requiere el rol **ADMIN**.")
            .ephemeral(true);
        return interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
    }

    // 3. Verificación de Firestore configurado
    let fs = match firestore {
        Some(f) => f,
        None => {
            let resp = CreateInteractionResponseMessage::new()
                .content(
                    "❌ No se pudo completar la operación en este momento. Inténtalo más tarde.",
                )
                .ephemeral(true);
            return interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    // 4. Obtener el usuario objetivo
    let target_user_id = match interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "usuario")
        .and_then(|opt| opt.value.as_user_id())
    {
        Some(u) => u,
        None => {
            let resp = CreateInteractionResponseMessage::new()
                .content("❌ Debes especificar un usuario de Discord válido.")
                .ephemeral(true);
            return interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    };

    // 5. Defer efímero para operaciones asíncronas
    interaction.defer_ephemeral(&ctx.http).await?;

    let guild_str = interaction.guild_id.map(|g| g.to_string());
    let target_id_str = target_user_id.to_string();

    // 6. Ejecutar desvinculación en Firestore
    let plan = compute_unlink_plan(guild_str.as_deref(), &target_id_str);
    if let Err(err) = execute_unlink_on_sink(fs, &plan).await {
        tracing::error!("Error en el comando /desvincular_fc: {}", err);
        let msg = EditInteractionResponse::new().content(format!(
            "❌ Ocurrió un error al intentar desvincular al usuario: {}",
            err
        ));
        interaction.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    // Registrar en faceit_bot_logs (equivalente a db.addLogEntry del legado)
    let log_entry = serde_json::json!({
        "level": "warn",
        "message": format!(
            "El administrador {} desvinculó la cuenta de {}",
            interaction.user.name, target_id_str
        ),
        "source": "bot",
        "guild_id": guild_str,
        "createdAt": chrono::Utc::now().to_rfc3339(),
    });
    let log_id = format!(
        "{}_{}",
        chrono::Utc::now().timestamp_millis(),
        target_id_str
    );
    let _ = fs.set_doc("faceit_bot_logs", &log_id, &log_entry).await;

    // 7. Sacar al jugador de la cola del lobby y limpiar el estado de negociación/ruleta
    let resolved_guild = lobby.resolve_guild_id(guild_str.as_deref());
    lobby.leave_queue(resolved_guild, &target_id_str).await;
    let mut state = lobby.get_state(resolved_guild).await;
    let modified = super::mixton_logic::clean_lobby_on_player_leave(&mut state, &target_id_str);
    if modified {
        lobby.cancel_roulette(resolved_guild);
        if super::mixton_logic::should_auto_assign_captains(&state) {
            let ready = state.available_players.clone();
            if let Ok(_) = lobby::draft::auto_assign_captains(&mut state, ready) {
                lobby.cancel_roulette(resolved_guild);
                state.sanitize();
            }
        }
    }
    lobby.update_state(resolved_guild, state).await;
    lobby.broadcast_state(resolved_guild).await;

    // 8. Retirar roles de Discord si el miembro está presente en el servidor
    if let Some(gid) = interaction.guild_id {
        if let Ok(target_member) = ctx.http.get_member(gid, target_user_id).await {
            let removed_count = remove_faceit_roles(ctx, gid, &target_member, config).await;
            if removed_count > 0 {
                tracing::info!(
                    "Roles de Faceit removidos para el usuario desvinculado: <@{}> (total: {})",
                    target_user_id,
                    removed_count
                );
            }
        }
    }

    // 9. Respuesta de éxito idéntica al legado
    let msg = EditInteractionResponse::new().content(format!(
        "✅ El usuario <@{}> ha sido desvinculado de Faceit con éxito y sus roles asociados fueron removidos.",
        target_user_id
    ));
    interaction.edit_response(&ctx.http, msg).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use data::FakeDocSink;
    use serde_json::json;

    #[test]
    fn test_compute_unlink_plan_with_guild() {
        let plan = compute_unlink_plan(Some("guild_99"), "user_42");
        assert_eq!(plan.doc_id, "guild_99_user_42");
    }

    #[test]
    fn test_compute_unlink_plan_without_guild() {
        let plan = compute_unlink_plan(None, "user_42");
        assert_eq!(plan.doc_id, "user_42");
    }

    #[tokio::test]
    async fn test_execute_unlink_on_fake_sink() {
        let sink = FakeDocSink::new();

        // Estado inicial en Firestore:
        // Doc global con saldo, steam, phone y datos de Faceit
        let global_user = json!({
            "discord_id": "u1",
            "wallet_balance": 50.0,
            "steam_id_64": "76561198000000000",
            "phoneNumber": "999888777",
            "faceit_player_id": "faceit_uuid_123",
            "faceit_nickname": "ProGamer",
            "faceit_elo": 2100,
            "skill_level": 10,
            "mixton_ready": true,
            "mixton_guild_id": "guild_1"
        });
        sink.set_doc("faceit_users", "u1", &global_user)
            .await
            .unwrap();

        // Doc de servidor específico
        let server_user = json!({
            "faceit_nickname": "ProGamer_Servidor",
            "guild_id": "guild_1"
        });
        sink.set_doc("faceit_users", "guild_1_u1", &server_user)
            .await
            .unwrap();

        // Ejecutar desvinculación
        let plan = compute_unlink_plan(Some("guild_1"), "u1");
        execute_unlink_on_sink(&sink, &plan).await.unwrap();

        // 1. El doc de servidor se borra por completo.
        assert_eq!(sink.get("faceit_users", "guild_1_u1"), None);

        // 2. El doc global queda INTACTO (como en el legado): otros servidores no se ven afectados.
        let remaining_global = sink.get("faceit_users", "u1").expect("global doc exists");
        assert_eq!(remaining_global, global_user);
    }

    #[test]
    fn test_validate_link_channel_rules() {
        // No configurado
        assert!(validate_link_channel(None, "123").is_err());
        assert!(validate_link_channel(Some(""), "123").is_err());
        assert!(validate_link_channel(Some("ID_CANAL"), "123").is_err());
        assert!(validate_link_channel(Some("ID-CANAL"), "123").is_err());

        // Configurado un solo canal
        assert!(validate_link_channel(Some("1001"), "1001").is_ok());
        assert!(validate_link_channel(Some("1001"), "2002").is_err());

        // Configurado múltiples canales separados por coma
        assert!(validate_link_channel(Some("1001, 1002, 1003"), "1002").is_ok());
        assert!(validate_link_channel(Some("1001, 1002, 1003"), "9999").is_err());
    }
}
