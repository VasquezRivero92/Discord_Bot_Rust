use crate::faceit::FaceitClient;
use chrono::Utc;
use data::FirestoreClient;
use serenity::all::*;

pub fn register() -> CreateCommand {
    CreateCommand::new("verificar_fc")
        .description("Vincula tu cuenta de Discord con tu perfil de Faceit")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::String,
                "nickname",
                "Tu nickname exacto en Faceit",
            )
            .required(true),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::String,
                "telefono",
                "Tu número de celular para premios Yape (Opcional)",
            )
            .required(false),
        )
}

pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    faceit: Option<&FaceitClient>,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    interaction.defer_ephemeral(&ctx.http).await?;

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

    let telefono = interaction
        .data
        .options
        .iter()
        .find(|opt| opt.name == "telefono")
        .and_then(|opt| opt.value.as_str());

    if nickname.is_empty() {
        let msg = EditInteractionResponse::new().content("❌ Debes ingresar un nickname válido de Faceit.");
        interaction.edit_response(&ctx.http, msg).await?;
        return Ok(());
    }

    match faceit.get_player_by_nickname(nickname).await {
        Ok(Some(player)) => {
            let discord_id = interaction.user.id.to_string();
            let elo = player
                .games
                .as_ref()
                .and_then(|g| g.cs2.as_ref().and_then(|c| c.faceit_elo).or_else(|| g.csgo.as_ref().and_then(|c| c.faceit_elo)))
                .unwrap_or(1000);

            let level = player
                .games
                .as_ref()
                .and_then(|g| g.cs2.as_ref().and_then(|c| c.skill_level).or_else(|| g.csgo.as_ref().and_then(|c| c.skill_level)))
                .unwrap_or(1);

            // Guardar vinculación en Firestore
            if let Some(fs) = firestore {
                let user_doc = serde_json::json!({
                    "discord_id": discord_id,
                    "discord_username": interaction.user.name,
                    "faceit_player_id": player.player_id,
                    "faceit_nickname": player.nickname,
                    "faceit_elo": elo,
                    "skill_level": level,
                    "phone_number": telefono,
                    "verified_at": Utc::now().to_rfc3339(),
                });

                // En firestore-rs, fluent().update().in_col("users").document_id(&discord_id)...
                // o create si no existe.
                let db = fs.db();
                let _ = db
                    .fluent()
                    .insert()
                    .into("users")
                    .document_id(&discord_id)
                    .object(&user_doc)
                    .execute::<()>()
                    .await;
            }

            let mut embed = CreateEmbed::new()
                .title("✅ ¡Cuenta de Faceit Vinculada!")
                .description(format!(
                    "Tu cuenta de Discord <@{}> ha sido vinculada exitosamente con el perfil de Faceit **{}**.",
                    discord_id, player.nickname
                ))
                .color(0x00E676)
                .field("Nickname Faceit", format!("[{}] (https://www.faceit.com/en/players/{})", player.nickname, player.nickname), true)
                .field("Nivel", format!("Nivel {}", level), true)
                .field("ELO CS2", format!("{}", elo), true)
                .footer(CreateEmbedFooter::new("Machupeek • Vinculación Oficial"));

            if let Some(avatar) = player.avatar {
                if !avatar.is_empty() {
                    embed = embed.thumbnail(avatar);
                }
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
            tracing::error!("Error vinculando Faceit: {}", err);
            let msg = EditInteractionResponse::new()
                .content("❌ Ocurrió un error al contactar la API de Faceit.");
            interaction.edit_response(&ctx.http, msg).await?;
        }
    }

    Ok(())
}
