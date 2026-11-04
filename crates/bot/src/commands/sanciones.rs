use common::config::Config;
use serenity::all::*;

pub fn register_sancionar() -> CreateCommand {
    CreateCommand::new("sancionar")
        .description("Aplica y notifica una sanción a un usuario en Discord")
        .default_member_permissions(Permissions::MANAGE_MESSAGES)
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::User,
                "usuario",
                "El usuario de Discord a sancionar",
            )
            .required(true),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::String,
                "duracion",
                "Duración de la sanción (ej: 12h, 24h, 3d, 7d, permanente)",
            )
            .required(true),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::String,
                "razon",
                "Motivo de la sanción",
            )
            .required(false),
        )
}

pub fn register_desancionar() -> CreateCommand {
    CreateCommand::new("desancionar")
        .description("Levanta una sanción a un usuario")
        .default_member_permissions(Permissions::MANAGE_MESSAGES)
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::User,
                "usuario",
                "El usuario de Discord a desancionar",
            )
            .required(true),
        )
}

pub async fn run_sancionar(
    ctx: &Context,
    interaction: &CommandInteraction,
    config: &Config,
) -> Result<(), serenity::Error> {
    interaction.defer_ephemeral(&ctx.http).await?;

    let target_user = interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "usuario")
        .and_then(|opt| opt.value.as_user_id());

    let target_user = match target_user {
        Some(u) => u,
        None => {
            let msg = EditInteractionResponse::new().content("❌ Debes especificar un usuario válido.");
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let duracion = interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "duracion")
        .and_then(|opt| opt.value.as_str())
        .unwrap_or("Permanente");

    let razon = interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "razon")
        .and_then(|opt| opt.value.as_str())
        .unwrap_or("Comportamiento antideportivo / Violación de reglas del Hub");

    let embed = CreateEmbed::new()
        .title("⛔ Sanción Aplicada (Baneo)")
        .description(format!(
            "Se ha emitido una sanción para el usuario <@{}>.",
            target_user
        ))
        .color(0xFF3333)
        .field("Usuario", format!("<@{}>", target_user), true)
        .field("Duración", duracion, true)
        .field("Motivo", razon, false)
        .field("Staff Responsable", format!("<@{}>", interaction.user.id), true)
        .footer(CreateEmbedFooter::new("Machupeek • Sistema Disciplinario"));

    // Enviar al canal de sanciones si está configurado
    if let Some(chan_id_str) = &config.env_defaults.discord_sanctions_channel_id {
        if let Ok(chan_id) = chan_id_str.parse::<u64>() {
            let channel = ChannelId::new(chan_id);
            let msg = CreateMessage::new().embed(embed.clone());
            let _ = channel.send_message(&ctx.http, msg).await;
        }
    }

    let msg = EditInteractionResponse::new()
        .content(format!("✅ Sanción notificada exitosamente para <@{}>.", target_user));
    interaction.edit_response(&ctx.http, msg).await?;

    Ok(())
}

pub async fn run_desancionar(
    ctx: &Context,
    interaction: &CommandInteraction,
    config: &Config,
) -> Result<(), serenity::Error> {
    interaction.defer_ephemeral(&ctx.http).await?;

    let target_user = interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "usuario")
        .and_then(|opt| opt.value.as_user_id());

    let target_user = match target_user {
        Some(u) => u,
        None => {
            let msg = EditInteractionResponse::new().content("❌ Debes especificar un usuario válido.");
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let embed = CreateEmbed::new()
        .title("✅ Sanción Levantada (Desbaneo)")
        .description(format!(
            "La sanción registrada para el jugador <@{}> ha sido levantada exitosamente.",
            target_user
        ))
        .color(0x00E676)
        .field("Usuario", format!("<@{}>", target_user), true)
        .field("Staff Responsable", format!("<@{}>", interaction.user.id), true)
        .footer(CreateEmbedFooter::new("Machupeek • Sistema Disciplinario"));

    if let Some(chan_id_str) = &config.env_defaults.discord_sanctions_channel_id {
        if let Ok(chan_id) = chan_id_str.parse::<u64>() {
            let channel = ChannelId::new(chan_id);
            let msg = CreateMessage::new().embed(embed.clone());
            let _ = channel.send_message(&ctx.http, msg).await;
        }
    }

    let msg = EditInteractionResponse::new()
        .content(format!("✅ Sanción levantada para <@{}>.", target_user));
    interaction.edit_response(&ctx.http, msg).await?;

    Ok(())
}
