use common::config::Config;
use serenity::all::*;

pub fn register() -> CreateCommand {
    CreateCommand::new("info")
        .description("Muestra información del servidor y el club de Faceit")
}

pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    config: &Config,
) -> Result<(), serenity::Error> {
    let guild_name = interaction
        .guild_id
        .and_then(|g| ctx.cache.guild(g))
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Machupeek".to_string());

    let club_url = config
        .env_defaults
        .faceit_club_id
        .as_ref()
        .map(|id| format!("https://www.faceit.com/es/club/{}", id))
        .or_else(|| {
            config
                .env_defaults
                .faceit_hub_id
                .as_ref()
                .map(|id| format!("https://www.faceit.com/es/hub/{}", id))
        })
        .unwrap_or_else(|| "https://machupeek.com".to_string());

    let embed = CreateEmbed::new()
        .title(format!("🎮 Información de {}", guild_name))
        .description(format!(
            "Bienvenido a **{}**. Aquí tienes los accesos rápidos a nuestra comunidad:",
            guild_name
        ))
        .color(0x5865F2)
        .field("🔗 Club / Hub Faceit", format!("[Haz clic aquí]({})", club_url), true)
        .field("📝 Vinculación", "Usa `/verificar_fc <nickname>` para vincular tu Faceit", true)
        .field("🏆 Leaderboard", "Usa `/leaderboard` para ver el ranking del Hub", true)
        .field("📊 Estadísticas", "Usa `/stats <nickname>` para ver tus datos", true)
        .field("🎮 Mixton CS2", "Usa `/mixton_listo` o `/mixton_no_listo` para jugar", true)
        .field("🌐 Web Oficial", "[machupeek.com](https://machupeek.com)", true)
        .footer(CreateEmbedFooter::new(format!("{} • Powered by Machupeek", guild_name)));

    let builder = CreateInteractionResponseMessage::new().embed(embed);
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(builder))
        .await
}
