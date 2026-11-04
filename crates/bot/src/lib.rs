pub mod commands;
pub mod faceit;
pub mod handler;

use common::config::Config;
use data::FirestoreClient;
use faceit::FaceitClient;
use handler::BotHandler;
use lobby::state::LobbyManager;
use serenity::all::*;
use std::sync::Arc;
use tracing::{error, info};

/// Inicia el bot de Discord en segundo plano.
pub async fn start_bot(
    config: Config,
    firestore: Option<FirestoreClient>,
    lobby: Arc<LobbyManager>,
) -> Result<(), serenity::Error> {
    let token = config.discord_token.clone();
    if token.is_empty() || token.starts_with("DISCORD_TOKEN") {
        info!("Token de Discord no configurado o de prueba; omitiendo bot.");
        return Ok(());
    }

    let faceit = config
        .faceit_api_key
        .as_ref()
        .filter(|k| !k.is_empty())
        .map(|k| FaceitClient::new(k.clone()));

    let handler = BotHandler {
        config: config.clone(),
        firestore,
        lobby,
        faceit,
    };

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_VOICE_STATES;

    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await?;

    info!("Iniciando Gateway de Discord...");
    if let Err(why) = client.start().await {
        error!("Error en el cliente de Discord: {:?}", why);
        return Err(why);
    }

    Ok(())
}
