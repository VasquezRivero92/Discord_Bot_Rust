//! Manejador de WebSockets en tiempo real (`/api/lobby/mixton-ws`) con Axum.

use crate::state::{partition_available_players, LobbyManager};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

fn extract_guild_id(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    for pair in raw.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            if k == "guildId" || k == "guild_id" {
                let decoded = url::form_urlencoded::parse(v.as_bytes())
                    .next()
                    .map(|(_, val)| val.into_owned())
                    .unwrap_or_else(|| v.to_string());
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    None
}

/// Handler de actualización a WebSocket de Axum.
pub async fn mixton_ws_handler(
    ws: WebSocketUpgrade,
    RawQuery(raw): RawQuery,
    State(manager): State<Arc<LobbyManager>>,
) -> impl IntoResponse {
    let g_opt = extract_guild_id(raw.as_deref());
    let guild_id = manager.resolve_guild_id(g_opt.as_deref()).to_string();
    ws.on_upgrade(move |socket| handle_socket(socket, manager, guild_id))
}

async fn handle_socket(socket: WebSocket, manager: Arc<LobbyManager>, guild_id: String) {
    let (mut sender, mut receiver) = socket.split();

    // 1. Enviar estado inicial
    let state = manager.get_state(&guild_id).await;
    let available_players = partition_available_players(&state);
    let initial_state = serde_json::json!({
        "type": "state_update",
        "data": {
            "state": state,
            "availablePlayers": available_players,
            "commandEnabled": true,
            "guildName": "MachuPeek",
            "guildId": guild_id
        }
    })
    .to_string();

    if let Err(e) = sender.send(Message::Text(initial_state.into())).await {
        error!("[WS] Error enviando estado inicial a {guild_id}: {e}");
        return;
    }

    // 2. Enviar historial reciente del chat
    let history = manager.get_chat_history(&guild_id).await;
    if !history.is_empty() {
        let history_msg = serde_json::json!({
            "type": "chat_history",
            "data": history
        })
        .to_string();
        let _ = sender.send(Message::Text(history_msg.into())).await;
    }

    // Canal interno para coordinar respuestas salientes (Pongs, mensajes directos) hacia el sender
    let (tx_out, mut rx_out) = mpsc::channel::<Message>(64);

    // 3. Suscribirse a las difusiones del guild
    let rx = manager.get_broadcast_channel(&guild_id).await;
    let mut broadcast_rx = rx.subscribe();

    // 4. Bucle emisor (mensajes directos, difusiones y heartbeat Ping cada 20 segundos)
    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Some(msg) = rx_out.recv() => {
                    if sender.send(msg).await.is_err() {
                        break;
                    }
                }
                Ok(broadcast_msg) = broadcast_rx.recv() => {
                    if sender.send(Message::Text(broadcast_msg.into())).await.is_err() {
                        break;
                    }
                }
                _ = ping_interval.tick() => {
                    // Enviar Ping frame para evitar que Cloudflare o el navegador cierren el túnel por inactividad
                    if sender.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 5. Bucle receptor: procesa Pings entrantes de Cloudflare/navegador y responde con Pong
    let tx_out_clone = tx_out.clone();
    let gid_clone = guild_id.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg_res) = receiver.next().await {
            match msg_res {
                Ok(Message::Ping(payload)) => {
                    // RFC 6455: Todo Ping debe recibir su correspondiente Pong
                    if tx_out_clone.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Pong(_)) => {
                    // Pong recibido satisfactoriamente
                }
                Ok(Message::Text(t)) => {
                    if t == "ping" {
                        let _ = tx_out_clone.send(Message::Text("pong".into())).await;
                    } else {
                        debug!("[WS] Mensaje de cliente en {gid_clone}: {t}");
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!("[WS] Cliente cerró el WebSocket en {gid_clone}");
                    break;
                }
                Ok(Message::Binary(_)) => {}
                Err(e) => {
                    warn!("[WS] Conexión cerrada o error de lectura en {gid_clone}: {e}");
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }
}
