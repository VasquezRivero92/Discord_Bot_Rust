use data::FirestoreClient;
use lobby::state::{LobbyManager, Player};
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
    let guild_str = guild_id
        .map(|g| g.to_string())
        .unwrap_or_else(|| "default".to_string());

    let discord_id = user.id.to_string();

    // Consultar vinculación en Firestore si está disponible
    let mut faceit_nick = None;
    let mut elo = 1000;
    let mut steam_id = None;

    if let Some(fs) = firestore {
        // En Firestore, los usuarios están indexados en la colección users
        // o podemos consultar por discord_id
        if let Ok(Some(doc)) = fs.get_doc::<serde_json::Value>("users", &discord_id).await {
            faceit_nick = doc.get("faceit_nickname").and_then(|v| v.as_str()).map(str::to_string);
            if let Some(e) = doc.get("faceit_elo").and_then(|v| v.as_u64()) {
                elo = e as u32;
            }
            steam_id = doc.get("steam_id_64").and_then(|v| v.as_str()).map(str::to_string);
        }
    }

    let display_name = user.name.clone();
    let avatar_url = user.avatar_url();

    let player = Player {
        discord_id: discord_id.clone(),
        nickname: display_name.clone(),
        faceit_nickname: faceit_nick.clone(),
        steam_id_64: steam_id,
        avatar: avatar_url.clone(),
        faceit_elo: elo,
        ready: true,
        tier: "competitor".to_string(),
    };

    let resolved_guild = lobby.resolve_guild_id(Some(&guild_str));
    lobby.join_queue(resolved_guild, player).await;
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
        .field("Faceit Nick", faceit_nick.unwrap_or_else(|| "No vinculado".to_string()), true)
        .field("ELO", format!("{}", elo), true)
        .footer(CreateEmbedFooter::new("Machupeek CS2 • Sistema Mixton"));

    Ok(CreateInteractionResponseMessage::new().embed(embed).ephemeral(true))
}

pub async fn run_no_listo(
    _ctx: &Context,
    user: &User,
    guild_id: Option<GuildId>,
    lobby: &Arc<LobbyManager>,
) -> Result<CreateInteractionResponseMessage, serenity::Error> {
    let guild_str = guild_id
        .map(|g| g.to_string())
        .unwrap_or_else(|| "default".to_string());

    let discord_id = user.id.to_string();
    let resolved_guild = lobby.resolve_guild_id(Some(&guild_str));

    lobby.leave_queue(resolved_guild, &discord_id).await;
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

    Ok(CreateInteractionResponseMessage::new().embed(embed).ephemeral(true))
}
