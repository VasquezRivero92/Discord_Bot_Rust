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

pub const MODAL_ID: &str = "verificar_modal";

fn input(
    style: InputTextStyle,
    label: &str,
    custom_id: &str,
    placeholder: &str,
    max: u16,
    required: bool,
) -> CreateActionRow {
    CreateActionRow::InputText(
        CreateInputText::new(style, label, custom_id)
            .placeholder(placeholder)
            .max_length(max)
            .required(required),
    )
}

/// Abre el formulario de vinculación al pulsar el botón "Verificar Cuenta".
pub async fn open_modal(
    ctx: &Context,
    component: &ComponentInteraction,
) -> Result<(), serenity::Error> {
    let modal = CreateModal::new(MODAL_ID, "Verificar cuenta de Faceit").components(vec![
        input(
            InputTextStyle::Short,
            "Nickname exacto en Faceit",
            "nickname",
            "Ej: s1mple",
            64,
            true,
        ),
        input(
            InputTextStyle::Short,
            "Celular para premios Yape (opcional)",
            "telefono",
            "Ej: 987654321",
            9,
            false,
        ),
    ]);
    component
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await
}

fn field(modal: &ModalInteraction, id: &str) -> String {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|c| match c {
            ActionRowComponent::InputText(it) if it.custom_id == id => it.value.clone(),
            _ => None,
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub async fn run(
    ctx: &Context,
    interaction: &CommandInteraction,
    faceit: Option<&FaceitClient>,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    interaction.defer_ephemeral(&ctx.http).await?;

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

    let msg = link_account(&interaction.user, nickname, telefono, faceit, firestore).await;
    interaction.edit_response(&ctx.http, msg).await?;
    Ok(())
}

pub async fn handle_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    faceit: Option<&FaceitClient>,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    modal.defer_ephemeral(&ctx.http).await?;

    let nickname = field(modal, "nickname");
    let telefono = field(modal, "telefono");
    let telefono = (!telefono.is_empty()).then_some(telefono.as_str());

    let msg = link_account(&modal.user, &nickname, telefono, faceit, firestore).await;
    modal.edit_response(&ctx.http, msg).await?;
    Ok(())
}

/// Vincula Discord con Faceit y devuelve la respuesta para el usuario.
async fn link_account(
    user: &User,
    nickname: &str,
    telefono: Option<&str>,
    faceit: Option<&FaceitClient>,
    firestore: Option<&FirestoreClient>,
) -> EditInteractionResponse {
    let Some(faceit) = faceit else {
        return EditInteractionResponse::new()
            .content("❌ FACEIT_API_KEY no está configurado en el servidor.");
    };

    if nickname.is_empty() {
        return EditInteractionResponse::new()
            .content("❌ Debes ingresar un nickname válido de Faceit.");
    }

    match faceit.get_player_by_nickname(nickname).await {
        Ok(Some(player)) => {
            let discord_id = user.id.to_string();
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

            // Guardar vinculación en MySQL / base de datos en tiempo real
            if let Some(fs) = firestore {
                let user_doc = serde_json::json!({
                    "discordId": discord_id,
                    "discord_id": discord_id,
                    "discord_username": user.name,
                    "faceit_player_id": player.player_id,
                    "faceit_nickname": player.nickname,
                    "faceit_nickname_lower": player.nickname.to_lowercase(),
                    "faceit_elo": elo,
                    "skill_level": level,
                    "phoneNumber": telefono,
                    "phone_number": telefono,
                    "verified_at": Utc::now().to_rfc3339(),
                });

                let _ = fs.merge_doc("faceit_users", &discord_id, &user_doc).await;
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

            EditInteractionResponse::new().embed(embed)
        }
        Ok(None) => EditInteractionResponse::new().content(format!(
            "❌ No se encontró ningún jugador con el nickname **{}** en Faceit.",
            nickname
        )),
        Err(err) => {
            tracing::error!("Error vinculando Faceit: {}", err);
            EditInteractionResponse::new().content("❌ Ocurrió un error al contactar la API de Faceit.")
        }
    }
}
