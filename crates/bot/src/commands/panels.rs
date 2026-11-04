use serenity::all::*;

pub fn register_setup_recargas() -> CreateCommand {
    CreateCommand::new("setup_recargas")
        .description("Publica el panel interactivo de recargas con Yape en este canal")
        .default_member_permissions(Permissions::ADMINISTRATOR)
}

pub fn register_setup_soporte() -> CreateCommand {
    CreateCommand::new("setup_soporte")
        .description("Crea el panel de soporte para llamar administradores")
        .default_member_permissions(Permissions::ADMINISTRATOR)
}

pub async fn run_setup_recargas(
    ctx: &Context,
    interaction: &CommandInteraction,
) -> Result<(), serenity::Error> {
    let guild_name = interaction
        .guild_id
        .and_then(|g| ctx.cache.guild(g))
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Machupeek".to_string());

    let embed = CreateEmbed::new()
        .title(format!("💰 Billetera y Recargas — {}", guild_name))
        .description(
            "¡Recarga saldo a tu cuenta de forma instantánea usando **Yape** para participar en las partidas y cuotas de apuestas de **Machupeek**!\n\n\
            **📌 ¿Cómo recargar?**\n\
            1️⃣ Realiza tu Yape al número oficial de la comunidad.\n\
            2️⃣ Presiona el botón de abajo e ingresa el monto, tu celular y el código de comprobante.\n\
            3️⃣ Tu saldo se acreditará y tu celular quedará registrado para recibir premios si ganas.\n\n\
            🔒 *Nunca te pediremos tus claves ni códigos de acceso.*"
        )
        .color(0x742284) // Púrpura Yape
        .footer(CreateEmbedFooter::new(format!("{} • Billetera Yape", guild_name)));

    let button = CreateButton::new("iniciar_recarga_btn")
        .label("Recargar con Yape")
        .style(ButtonStyle::Primary)
        .emoji(ReactionType::Unicode("💜".to_string()));

    let row = CreateActionRow::Buttons(vec![button]);

    let response = CreateInteractionResponseMessage::new()
        .embed(embed)
        .components(vec![row]);

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
        .await
}

pub async fn run_setup_soporte(
    ctx: &Context,
    interaction: &CommandInteraction,
) -> Result<(), serenity::Error> {
    let guild_name = interaction
        .guild_id
        .and_then(|g| ctx.cache.guild(g))
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Machupeek".to_string());

    let embed = CreateEmbed::new()
        .title("🚨 Soporte y Llamado de Admin")
        .description(
            "¿Tienes algún problema en tu partida activa o necesitas asistencia del staff?\n\n\
            Presiona el botón de abajo para solicitar la presencia inmediata de un administrador.\n\n\
            ⚠️ **Notas importantes**:\n\
            - Debes tener tu cuenta de Faceit vinculada con `/verificar_fc`.\n\
            - Al presionar el botón se notificará a los administradores de guardia."
        )
        .color(0xFF0000)
        .footer(CreateEmbedFooter::new(format!("{} • Centro de Soporte", guild_name)));

    let button = CreateButton::new("llamar_admin_btn")
        .label("Llamar Administrador")
        .style(ButtonStyle::Danger)
        .emoji(ReactionType::Unicode("🚨".to_string()));

    let row = CreateActionRow::Buttons(vec![button]);

    let response = CreateInteractionResponseMessage::new()
        .embed(embed)
        .components(vec![row]);

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
        .await
}
