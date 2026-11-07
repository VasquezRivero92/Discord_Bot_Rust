use crate::commands;
use crate::faceit::FaceitClient;
use common::config::Config;
use data::FirestoreClient;
use lobby::state::LobbyManager;
use serenity::all::*;
use serenity::async_trait;
use std::sync::Arc;
use tracing::{error, info, warn};

pub struct BotHandler {
    pub config: Config,
    pub firestore: Option<FirestoreClient>,
    pub lobby: Arc<LobbyManager>,
    pub faceit: Option<FaceitClient>,
}

#[async_trait]
impl EventHandler for BotHandler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("Bot de Discord conectado como {}", ready.user.name);

        let commands = vec![
            commands::verificar::register(),
            commands::stats::register(),
            commands::leaderboard::register(),
            commands::mixton::register_listo(),
            commands::mixton::register_no_listo(),
            commands::info::register(),
            commands::sanciones::register_sancionar(),
            commands::sanciones::register_desancionar(),
            commands::panels::register_setup_recargas(),
            commands::panels::register_setup_soporte(),
            commands::desvincular::register(),
            commands::sincronizar::register(),
            commands::llamar_admin::register(),
        ];

        match Command::set_global_commands(&ctx.http, commands).await {
            Ok(registered) => {
                info!(
                    "Comandos de barra globales registrados con éxito ({} comandos)",
                    registered.len()
                );
            }
            Err(err) => {
                error!("Error al registrar comandos globales de Discord: {}", err);
            }
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(command) => {
                let name = command.data.name.as_str();
                info!("Comando recibido de <@{}>: /{}", command.user.id, name);

                let result = match name {
                    "verificar_fc" => {
                        commands::verificar::run(
                            &ctx,
                            &command,
                            self.faceit.as_ref(),
                            self.firestore.as_ref(),
                        )
                        .await
                    }
                    "stats" => commands::stats::run(&ctx, &command, self.faceit.as_ref()).await,
                    "leaderboard" => {
                        commands::leaderboard::run(
                            &ctx,
                            &command,
                            &self.config,
                            self.faceit.as_ref(),
                        )
                        .await
                    }
                    "mixton_listo" => {
                        match commands::mixton::run_listo(
                            &ctx,
                            &command.user,
                            command.guild_id,
                            &self.lobby,
                            self.firestore.as_ref(),
                        )
                        .await
                        {
                            Ok(resp) => {
                                command
                                    .create_response(
                                        &ctx.http,
                                        CreateInteractionResponse::Message(resp),
                                    )
                                    .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "mixton_no_listo" => {
                        match commands::mixton::run_no_listo(
                            &ctx,
                            &command.user,
                            command.guild_id,
                            &self.lobby,
                            self.firestore.as_ref(),
                        )
                        .await
                        {
                            Ok(resp) => {
                                command
                                    .create_response(
                                        &ctx.http,
                                        CreateInteractionResponse::Message(resp),
                                    )
                                    .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "info" => commands::info::run(&ctx, &command, &self.config).await,
                    "sancionar" => {
                        commands::sanciones::run_sancionar(&ctx, &command, &self.config).await
                    }
                    "desancionar" => {
                        commands::sanciones::run_desancionar(&ctx, &command, &self.config).await
                    }
                    "setup_recargas" => commands::panels::run_setup_recargas(&ctx, &command).await,
                    "setup_soporte" => commands::panels::run_setup_soporte(&ctx, &command).await,
                    "desvincular_fc" | "desvincular" => {
                        commands::desvincular::run(
                            &ctx,
                            &command,
                            &self.config,
                            &self.lobby,
                            self.firestore.as_ref(),
                        )
                        .await
                    }
                    "sincronizar_fc" => {
                        commands::sincronizar::run(
                            &ctx,
                            &command,
                            self.faceit.as_ref(),
                            self.firestore.as_ref(),
                        )
                        .await
                    }
                    "llamar_admin" => {
                        commands::llamar_admin::run(
                            &ctx,
                            &command,
                            &self.config,
                            self.firestore.as_ref(),
                        )
                        .await
                    }
                    unknown => {
                        warn!("Comando desconocido recibido: {}", unknown);
                        let msg = CreateInteractionResponseMessage::new()
                            .content("❌ Comando no implementado")
                            .ephemeral(true);
                        command
                            .create_response(&ctx.http, CreateInteractionResponse::Message(msg))
                            .await
                    }
                };

                if let Err(err) = result {
                    error!("Error procesando interacción de comando /{}: {}", name, err);
                }
            }

            Interaction::Component(component) => {
                let custom_id = component.data.custom_id.as_str();
                info!(
                    "Botón presionado por <@{}>: {}",
                    component.user.id, custom_id
                );

                let result = match custom_id {
                    "mixton_listo_btn" => {
                        match commands::mixton::run_listo(
                            &ctx,
                            &component.user,
                            component.guild_id,
                            &self.lobby,
                            self.firestore.as_ref(),
                        )
                        .await
                        {
                            Ok(resp) => {
                                component
                                    .create_response(
                                        &ctx.http,
                                        CreateInteractionResponse::Message(resp),
                                    )
                                    .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "mixton_no_listo_btn" => {
                        match commands::mixton::run_no_listo(
                            &ctx,
                            &component.user,
                            component.guild_id,
                            &self.lobby,
                            self.firestore.as_ref(),
                        )
                        .await
                        {
                            Ok(resp) => {
                                component
                                    .create_response(
                                        &ctx.http,
                                        CreateInteractionResponse::Message(resp),
                                    )
                                    .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "llamar_admin_btn" => {
                        commands::llamar_admin::handle_llamar_admin_button(
                            &ctx,
                            &component,
                            self.firestore.as_ref(),
                        )
                        .await
                    }
                    custom if custom.starts_with("admin_respond_btn_") => {
                        commands::llamar_admin::handle_admin_respond_button(
                            &ctx,
                            &component,
                            &self.config,
                            self.firestore.as_ref(),
                        )
                        .await
                    }
                    "iniciar_recarga_btn" => {
                        let resp = CreateInteractionResponseMessage::new()
                            .content("💜 Para recargar saldo con Yape:\n1. Escanea el QR de Yape fijado en este canal y envía el monto.\n2. Envía tu comprobante o regístralo desde el panel web en [machupeek.com](https://machupeek.com).")
                            .ephemeral(true);

                        component
                            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                            .await
                    }
                    "iniciar_verificacion_btn" => {
                        let resp = CreateInteractionResponseMessage::new()
                            .content("🔗 Para vincular tu cuenta, ejecuta el comando `/verificar_fc <tu_nickname_de_faceit>` aquí en el servidor.")
                            .ephemeral(true);

                        component
                            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                            .await
                    }
                    other => {
                        warn!("Botón no reconocido: {}", other);
                        let resp = CreateInteractionResponseMessage::new()
                            .content("Acción no reconocida")
                            .ephemeral(true);
                        component
                            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                            .await
                    }
                };

                if let Err(err) = result {
                    error!(
                        "Error procesando interacción de botón {}: {}",
                        custom_id, err
                    );
                }
            }

            Interaction::Modal(modal) => {
                let custom_id = modal.data.custom_id.as_str();
                info!("Modal enviado por <@{}>: {}", modal.user.id, custom_id);

                let result = if custom_id.starts_with("support_call_modal_") {
                    commands::llamar_admin::handle_support_call_modal(
                        &ctx,
                        &modal,
                        &self.config,
                        self.firestore.as_ref(),
                    )
                    .await
                } else {
                    warn!("Modal no reconocido: {}", custom_id);
                    let resp = CreateInteractionResponseMessage::new()
                        .content("Modal no reconocido")
                        .ephemeral(true);
                    modal
                        .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                        .await
                };

                if let Err(err) = result {
                    error!(
                        "Error procesando interacción de modal {}: {}",
                        custom_id, err
                    );
                }
            }

            _ => {}
        }
    }
}
