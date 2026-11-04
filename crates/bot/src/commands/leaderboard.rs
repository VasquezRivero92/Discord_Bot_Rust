use crate::faceit::FaceitClient;
use common::config::Config;
use serenity::all::*;

pub fn register() -> CreateCommand {
    CreateCommand::new("leaderboard")
        .description("Muestra el ranking del Club de Faceit de la season actual")
}

pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    config: &Config,
    faceit: Option<&FaceitClient>,
) -> Result<(), serenity::Error> {
    interaction.defer(&ctx.http).await?;

    let faceit = match faceit {
        Some(f) => f,
        None => {
            let msg = EditInteractionResponse::new()
                .content("❌ FACEIT_API_KEY no está configurado en el servidor.");
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    let lb_id = config
        .env_defaults
        .faceit_leaderboard_id
        .as_deref()
        .or(config.env_defaults.faceit_hub_id.as_deref());

    let lb_id = match lb_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            let msg = EditInteractionResponse::new()
                .content("❌ El ID de Leaderboard o Hub de Faceit no está configurado.");
            interaction.edit_response(&ctx.http, msg).await?;
            return Ok(());
        }
    };

    match faceit.get_leaderboard(lb_id).await {
        Ok(Some(lb)) if !lb.items.is_empty() => {
            let lines: Vec<String> = lb
                .items
                .iter()
                .take(10)
                .enumerate()
                .map(|(i, item)| {
                    let medal = match i {
                        0 => "🥇",
                        1 => "🥈",
                        2 => "🥉",
                        _ => "🔹",
                    };
                    let lvl = item.player.skill_level.map(|l| format!("(Lvl {})", l)).unwrap_or_default();
                    let pts = item.points.map(|p| format!("{} pts", p)).unwrap_or_default();
                    format!("{medal} **{}** {} [{}]", item.player.nickname, lvl, pts)
                })
                .collect();

            let embed = CreateEmbed::new()
                .title("🏆 Leaderboard del Club — Season Actual")
                .description(format!(
                    "Ranking de los mejores jugadores sincronizado con Faceit:\n\n{}",
                    lines.join("\n")
                ))
                .color(0xFFD700)
                .footer(CreateEmbedFooter::new("Machupeek • Leaderboard en Vivo"));

            let msg = EditInteractionResponse::new().embed(embed);
            interaction.edit_response(&ctx.http, msg).await?;
        }
        Ok(_) => {
            let msg = EditInteractionResponse::new()
                .content("⚠️ No se encontraron posiciones en el leaderboard de Faceit actual.");
            interaction.edit_response(&ctx.http, msg).await?;
        }
        Err(err) => {
            tracing::error!("Error consultando leaderboard de Faceit: {}", err);
            let msg = EditInteractionResponse::new()
                .content("❌ Ocurrió un error al obtener el leaderboard de Faceit.");
            interaction.edit_response(&ctx.http, msg).await?;
        }
    }

    Ok(())
}
