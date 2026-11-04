//! Manejador de WebSockets en tiempo real (`/api/lobby/mixton-ws`) con Axum.

use crate::state::LobbyManager;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tracing::{error, info};

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
    let initial_state = serde_json::json!({
        "type": "state_update",
        "data": {
            "state": state,
            "availablePlayers": state.available_players,
            "commandEnabled": true,
            "guildId": guild_id
        }
    })
    .to_string();

    if let Err(e) = sender.send(Message::Text(initial_state.into())).await {
        error!("[WS] Error enviando estado inicial: {e}");
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

    // 3. Suscribirse a las difusiones del guild
    let rx = manager.get_broadcast_channel(&guild_id).await;
    let mut broadcast_rx = rx.subscribe();

    // 4. Bucle bidireccional
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Close(_) => break,
                Message::Ping(_) => {}
                Message::Pong(_) => {}
                Message::Text(t) => {
                    info!("[WS] Mensaje recibido del cliente en {guild_id}: {t}");
                }
                Message::Binary(_) => {}
            }
        }
    });

    // Si una de las dos tareas finaliza (p. ej. el cliente cerró la pestaña), abortamos la otra limpiamente
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }
}
