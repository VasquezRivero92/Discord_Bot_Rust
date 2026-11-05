use data::FirestoreClient;
use lobby::state::LobbyManager;
use serenity::all::*;
use std::sync::Arc;

pub fn register_listo() -> CreateCommand {
    CreateCommand::new("mixton_listo")
        .description("Muestra que estás disponible y listo para competir en el mixton")
}

pub fn register_no_listo() -> CreateCommand {
    CreateCommand::new("mixton_no_listo")
        .description("Quita tu perfil de la lista de listos para el mixton")
}

pub async fn run_listo(
    _ctx: &Context,
    user: &User,
    guild_id: Option<GuildId>,
    lobby: &Arc<LobbyManager>,
    firestore: Option<&FirestoreClient>,
) -> Result<CreateInteractionResponseMessage, serenity::Error> {
    let guild_id = match guild_id {
        Some(gid) => gid,
        None => {
            return Ok(CreateInteractionResponseMessage::new()
                .content("❌ Este comando solo puede usarse dentro de un servidor.")
                .ephemeral(true));
        }
    };

    let fs = match firestore {
        Some(fs) => fs,
        None => {
            return Ok(CreateInteractionResponseMessage::new()
                .content("❌ No se pudo verificar tu cuenta en este momento. Inténtalo más tarde.")
                .ephemeral(true));
        }
    };

    let discord_id = user.id.to_string();
    let guild_str = guild_id.to_string();

    // Leer doc global users/{discord_id} y doc por servidor {guild_id}_{discord_id} en faceit_users
    let global_doc = match fs
        .get_doc::<serde_json::Value>("faceit_users", &discord_id)
        .await
    {
        Ok(doc) => doc,
        Err(e) => {
            tracing::warn!(
                "Error al leer doc global de faceit_users/{}: {}",
                discord_id,
                e
            );
            None
        }
    };

    let server_doc_id = format!("{guild_str}_{discord_id}");
    let server_doc = match fs
        .get_doc::<serde_json::Value>("faceit_users", &server_doc_id)
        .await
    {
        Ok(doc) => doc,
        Err(e) => {
            tracing::warn!(
                "Error al leer doc de servidor de faceit_users/{}: {}",
                server_doc_id,
                e
            );
            None
        }
    };

    let merged = super::mixton_logic::merge_user_docs(global_doc.as_ref(), server_doc.as_ref());
    if !super::mixton_logic::is_user_verified(&merged) {
        return Ok(CreateInteractionResponseMessage::new()
            .content("❌ No estás verificado. Debes vincular tu cuenta de Faceit usando el comando `/verificar_fc <nickname>` o desde la web antes de unirte al mixton.")
            .ephemeral(true));
    }

    let display_name = user.name.clone();
    let avatar_url = user.avatar_url();

    let player =
        super::mixton_logic::build_player_from_doc(&discord_id, &display_name, avatar_url, &merged);

    let resolved_guild = lobby.resolve_guild_id(Some(&guild_str));
    let mut state = lobby.join_queue(resolved_guild, player.clone()).await;

    // Si hay >= 10 jugadores listos, faltan capitanes y el draft no está iniciado/terminado:
    if super::mixton_logic::should_auto_assign_captains(&state) {
        let ready = state.available_players.clone();
        match lobby::draft::auto_assign_captains(&mut state, ready) {
            Ok(_) => {
                lobby.cancel_roulette(resolved_guild);
                state.sanitize();
                let _ = lobby.update_state(resolved_guild, state).await;
            }
            Err(e) => {
                tracing::warn!("Error al auto-asignar capitanes en /mixton_listo: {}", e);
            }
        }
    }

    lobby.broadcast_state(resolved_guild).await;

    let embed = CreateEmbed::new()
        .title("🎮 ¡Listo para el Mixton!")
        .description(format!(
            "Hola <@{}>, te has marcado como **Disponible** para competir.\n\n\
            Ya apareces en la lista de la sala y en [machupeek.com](https://machupeek.com/lobby).\n\
            Si deseas salir, usa `/mixton_no_listo` o presiona el botón No Listo.",
            discord_id
        ))
        .color(0x15C253)
        .field("Jugador", format!("**{}**", display_name), true)
        .field(
            "Faceit Nick",
            player
                .faceit_nickname
                .unwrap_or_else(|| "No vinculado".to_string()),
            true,
        )
        .field(
            "ELO / Nivel",
            format!(
                "{} (Nivel {})",
                player.faceit_elo,
                player.faceit_level.unwrap_or(1)
            ),
            true,
        )
        .field("Tier", player.tier, true)
        .footer(CreateEmbedFooter::new("Machupeek CS2 • Sistema Mixton"));

    Ok(CreateInteractionResponseMessage::new()
        .embed(embed)
        .ephemeral(true))
}

pub async fn run_no_listo(
    _ctx: &Context,
    user: &User,
    guild_id: Option<GuildId>,
    lobby: &Arc<LobbyManager>,
    firestore: Option<&FirestoreClient>,
) -> Result<CreateInteractionResponseMessage, serenity::Error> {
    let guild_id = match guild_id {
        Some(gid) => gid,
        None => {
            return Ok(CreateInteractionResponseMessage::new()
                .content("❌ Este comando solo puede usarse dentro de un servidor.")
                .ephemeral(true));
        }
    };

    let discord_id = user.id.to_string();
    let guild_str = guild_id.to_string();

    // Si Firestore está configurado, verificar vinculación
    if let Some(fs) = firestore {
        let global_doc = match fs
            .get_doc::<serde_json::Value>("faceit_users", &discord_id)
            .await
        {
            Ok(doc) => doc,
            Err(e) => {
                tracing::warn!(
                    "Error al leer doc global de faceit_users/{}: {}",
                    discord_id,
                    e
                );
                None
            }
        };

        let server_doc_id = format!("{guild_str}_{discord_id}");
        let server_doc = match fs
            .get_doc::<serde_json::Value>("faceit_users", &server_doc_id)
            .await
        {
            Ok(doc) => doc,
            Err(e) => {
                tracing::warn!(
                    "Error al leer doc de servidor de faceit_users/{}: {}",
                    server_doc_id,
                    e
                );
                None
            }
        };

        let merged = super::mixton_logic::merge_user_docs(global_doc.as_ref(), server_doc.as_ref());
        if !super::mixton_logic::is_user_verified(&merged) {
            return Ok(CreateInteractionResponseMessage::new()
                .content("❌ No estás verificado. Este comando es solo para miembros registrados.")
                .ephemeral(true));
        }
    }

    let resolved_guild = lobby.resolve_guild_id(Some(&guild_str));
    let mut state = lobby.get_state(resolved_guild).await;

    let modified = super::mixton_logic::clean_lobby_on_player_leave(&mut state, &discord_id);
    if modified {
        lobby.cancel_roulette(resolved_guild);
    }

    state
        .available_players
        .retain(|p| p.discord_id != discord_id);
    state.updated_at = chrono::Utc::now().timestamp_millis();

    if modified && super::mixton_logic::should_auto_assign_captains(&state) {
        let ready = state.available_players.clone();
        match lobby::draft::auto_assign_captains(&mut state, ready) {
            Ok(_) => {
                lobby.cancel_roulette(resolved_guild);
                state.sanitize();
            }
            Err(e) => {
                tracing::warn!(
                    "Error al auto-asignar capitanes tras /mixton_no_listo: {}",
                    e
                );
            }
        }
    }

    lobby.update_state(resolved_guild, state).await;
    lobby.broadcast_state(resolved_guild).await;

    let embed = CreateEmbed::new()
        .title("🎮 ¡Fuera del Mixton!")
        .description(format!(
            "Hola <@{}>, te has marcado como **No Disponible**.\n\n\
            Ya no aparecerás en la lista de jugadores listos en la web.\n\
            Usa `/mixton_listo` cuando estés listo para jugar.",
            discord_id
        ))
        .color(0xFF3E3E)
        .footer(CreateEmbedFooter::new("Machupeek CS2 • Sistema Mixton"));

    Ok(CreateInteractionResponseMessage::new()
        .embed(embed)
        .ephemeral(true))
}
