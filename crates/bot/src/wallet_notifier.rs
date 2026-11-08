//! Envía por mensaje privado de Discord los avisos de billetera encolados en `user_notifications`
//! (recargas, retiros, reembolsos, cobro de apuestas, premios) y las alertas de administración
//! al canal `WALLET_ALERTS_CHANNEL_ID`.
//!
//! La cola se escribe en la misma transacción que el movimiento, así que cada movimiento
//! confirmado tiene su aviso y uno que no se confirmó no avisa nada. Un aviso se marca como
//! enviado solo cuando Discord lo acepta; si falla se reintenta (hasta 5 veces, p. ej. si el
//! usuario tiene los mensajes privados cerrados).

use data::{FirestoreClient, WalletNotification};
use serenity::all::{ChannelId, CreateMessage, Http, UserId};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, warn};

const POLL_EVERY: Duration = Duration::from_secs(5);
const BATCH: u32 = 25;

/// Arranca el envío periódico de avisos en segundo plano.
pub fn spawn(http: Arc<Http>, fs: FirestoreClient) {
    let alerts_channel = std::env::var("WALLET_ALERTS_CHANNEL_ID")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|id| *id != 0)
        .map(ChannelId::new);
    if alerts_channel.is_none() {
        warn!("WALLET_ALERTS_CHANNEL_ID no definido: las alertas de billetera solo quedan en el log");
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(POLL_EVERY);
        loop {
            tick.tick().await;
            let pending = match fs.pending_notifications(BATCH).await {
                Ok(p) => p,
                Err(e) => {
                    error!("avisos de billetera: no se pudo leer la cola: {e}");
                    continue;
                }
            };
            for n in pending {
                let delivered = deliver(&http, alerts_channel, &n).await;
                if let Err(e) = fs.mark_notification(n.id, delivered).await {
                    error!("avisos de billetera: no se pudo marcar {}: {e}", n.id);
                }
            }
        }
    });
}

async fn deliver(http: &Http, alerts: Option<ChannelId>, n: &WalletNotification) -> bool {
    match &n.discord_id {
        Some(id) => {
            let Ok(uid) = id.trim().parse::<u64>() else {
                // Id que no es de Discord: no hay a quién avisar; se descarta.
                warn!("aviso {} para un id no válido ({id}); se descarta", n.id);
                return true;
            };
            let msg = CreateMessage::new().content(format!("💳 **MachuPeek** — {}", n.message));
            match UserId::new(uid).direct_message(http, msg).await {
                Ok(_) => true,
                Err(e) => {
                    warn!("aviso {} a {uid} no entregado: {e}", n.id);
                    false
                }
            }
        }
        None => {
            let Some(ch) = alerts else {
                warn!("ALERTA DE BILLETERA (sin canal configurado): {}", n.message);
                return true;
            };
            match ch.say(http, format!("🚨 **Billetera** — {}", n.message)).await {
                Ok(_) => true,
                Err(e) => {
                    error!("alerta {} no entregada al canal: {e}", n.id);
                    false
                }
            }
        }
    }
}
