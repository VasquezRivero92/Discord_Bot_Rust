use crate::faceit::FaceitClient;
use serenity::all::*;

pub fn register() -> CreateCommand {
    CreateCommand::new("stats")
        .description("Muestra estadísticas de un jugador de Faceit")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::String,
                "nickname",
                "Nickname exacto en Faceit",
            )
            .required(true),
        )
}

pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
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

    let nickname = interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "nickname")
        .and_then(|opt| opt.value.as_str())
        .unwrap_or_default();

    if nickname.is_empty() {
        let msg = EditInteractionResponse::new().content("❌ Debes ingresar un nickname válido.");
        interaction.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    match faceit.get_player_by_nickname(nickname).await {
        Ok(Some(player)) => {
            let stats = faceit.get_player_stats(&player.player_id).await.ok().flatten();
            let elo = player
                .games
                .as_ref()
                .and_then(|g| g.cs2.as_ref().and_then(|c| c.faceit_elo).or_else(|| g.csgo.as_ref().and_then(|c| c.faceit_elo)))
                .unwrap_or(0);

            let level = player
                .games
                .as_ref()
                .and_then(|g| g.cs2.as_ref().and_then(|c| c.skill_level).or_else(|| g.csgo.as_ref().and_then(|c| c.skill_level)))
                .unwrap_or(1);

            let color = match level {
                1..=3 => 0xC0C0C0,
                4..=7 => 0x00FF85,
                8..=9 => 0xFF9E00,
                10 => 0xFE0000,
                _ => 0x5865F2,
            };

            let mut embed = CreateEmbed::new()
                .title(format!("📊 Estadísticas de {}", player.nickname))
                .url(format!("https://www.faceit.com/en/players/{}", player.nickname))
                .color(color)
                .field("Nivel Faceit", format!("Nivel {}", level), true)
                .field("ELO CS2", format!("{}", elo), true)
                .field("País", player.country.clone().unwrap_or_else(|| "N/A".to_string()), true);

            if let Some(avatar) = player.avatar {
                if !avatar.is_empty() {
                    embed = embed.thumbnail(avatar);
                }
            }

            if let Some(stats) = stats.and_then(|s| s.lifetime) {
                embed = embed
                    .field("K/D Ratio", stats.kd_ratio.unwrap_or_else(|| "N/A".to_string()), true)
                    .field("Win Rate %", format!("{}%", stats.win_rate.unwrap_or_else(|| "N/A".to_string())), true)
                    .field("Partidas", stats.matches.unwrap_or_else(|| "N/A".to_string()), true);
            }

            let msg = EditInteractionResponse::new().embed(embed);
            interaction.edit_response(&ctx.http, msg).await?;
        }
        Ok(None) => {
            let msg = EditInteractionResponse::new()
                .content(format!("❌ No se encontró ningún jugador con el nickname **{}** en Faceit.", nickname));
            interaction.edit_response(&ctx.http, msg).await?;
        }
        Err(err) => {
            tracing::error!("Error consultando stats de Faceit: {}", err);
            let msg = EditInteractionResponse::new()
                .content("❌ Ocurrió un error al consultar la API de Faceit.");
            interaction.edit_response(&ctx.http, msg).await?;
        }
    }

    Ok(())
}
