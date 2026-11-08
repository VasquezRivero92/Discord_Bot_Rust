//! API de Matchmaking (Steam Bot Bridge).
//!
//! Rutas migradas:
//! - `GET /api/matchmaking/status`: estado del bot de Steam, protegido por `STEAM_BOT_API_KEY`.
//! - `POST /api/matchmaking/launch`: lanza invitaciones para 10 SteamID64 únicos en un servidor IP:puerto.

use std::collections::HashSet;
use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use crate::routes::json_response;

#[derive(Debug, Deserialize)]
pub struct MatchmakingLaunchPayload {
    #[serde(rename = "serverIp")]
    pub server_ip: Option<Value>,
    pub players: Option<Value>,
}

pub fn router() -> Router {
    Router::new()
        .route("/api/matchmaking/status", get(matchmaking_status))
        .route("/api/matchmaking/launch", post(matchmaking_launch))
        .route_layer(middleware::from_fn(auth_steam_api_key))
}

async fn auth_steam_api_key(req: Request<Body>, next: Next) -> Response {
    let api_key = match std::env::var("STEAM_BOT_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            return json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &json!({
                    "error": "Servicio no disponible",
                    "message": "La API de matchmaking no esta configurada en este servidor."
                }),
            );
        }
    };

    let token = req
        .headers()
        .get("x-api-key")
        .or_else(|| req.headers().get("authorization"))
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            if let Some(bearer) = s.strip_prefix("Bearer ") {
                bearer.trim()
            } else {
                s.trim()
            }
        });

    if token != Some(&api_key) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            &json!({
                "error": "No autorizado",
                "message": "API Key invalida o no proporcionada en la cabecera x-api-key o Authorization."
            }),
        );
    }

    next.run(req).await
}

async fn matchmaking_status() -> Response {
    json_response(
        StatusCode::OK,
        &json!({
            "status": "online",
            "steamBot": {
                "connected": false,
                "steamID": Value::Null,
                "personaState": 0
            },
            "uptime": 0,
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
}

fn parse_port(endpoint: &str) -> Option<u16> {
    let parts: Vec<&str> = endpoint.rsplitn(2, ':').collect();
    if parts.len() == 2 {
        parts[0].parse::<u16>().ok().filter(|&p| p > 0)
    } else {
        None
    }
}

async fn matchmaking_launch(Json(payload): Json<MatchmakingLaunchPayload>) -> Response {
    let Some(server_ip_val) = payload.server_ip else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Parametro faltante",
                "message": "El campo \"serverIp\" es obligatorio."
            }),
        );
    };

    let Some(server_ip) = server_ip_val.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Parametro faltante",
                "message": "El campo \"serverIp\" es obligatorio."
            }),
        );
    };

    let port = parse_port(server_ip);
    if port.is_none() {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Formato IP invalido",
                "message": "El campo \"serverIp\" debe tener un formato IP:PUERTO o DOMINIO:PUERTO valido y un puerto entre 1 y 65535."
            }),
        );
    }

    let Some(players_val) = payload.players else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Estructura invalida",
                "message": "El campo \"players\" debe ser un array."
            }),
        );
    };

    let Some(players_arr) = players_val.as_array() else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Estructura invalida",
                "message": "El campo \"players\" debe ser un array."
            }),
        );
    };

    if players_arr.len() != 10 {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Cantidad incorrecta de jugadores",
                "message": format!(
                    "Se requieren exactamente 10 jugadores para un match 5v5 de CS2. Recibidos: {}.",
                    players_arr.len()
                )
            }),
        );
    }

    let mut invalid_players = Vec::new();
    let mut valid_players = Vec::new();

    for p in players_arr {
        let is_valid = match p.as_str() {
            Some(s) => s.len() == 17 && s.chars().all(|c| c.is_ascii_digit()),
            None => false,
        };
        if is_valid {
            valid_players.push(p.as_str().unwrap().to_string());
        } else {
            invalid_players.push(p.clone());
        }
    }

    if !invalid_players.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "SteamIDs invalidos",
                "message": "Todos los jugadores deben ser SteamID64 validos como strings numericos de 17 digitos.",
                "invalidPlayers": invalid_players
            }),
        );
    }

    let mut seen = HashSet::new();
    let mut duplicates = Vec::new();
    for p in &valid_players {
        if !seen.insert(p.clone()) {
            duplicates.push(p.clone());
        }
    }

    if !duplicates.is_empty() {
        let unique_dups: Vec<String> = {
            let mut s = HashSet::new();
            duplicates.into_iter().filter(|x| s.insert(x.clone())).collect()
        };
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "SteamIDs duplicados",
                "message": "La lista de jugadores no puede contener SteamID64 repetidos.",
                "duplicatePlayers": unique_dups
            }),
        );
    }

    let lobby_id = format!("lobby_{}", Utc::now().timestamp_millis());
    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Matchmaking lanzado con exito. Se enviaron invitaciones a los jugadores.",
            "lobbyId": lobby_id,
            "lobby": {
                "lobbyId": lobby_id,
                "serverIp": server_ip,
                "players": valid_players
            }
        }),
    )
}
