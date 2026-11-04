//! Rutas nativas de la API de CS2 (`/api/cs2/*`).
//!
//! Usadas por el servidor local de CS2 para consultar partidas en curso y reportar resultados.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cs2::Cs2ServerManager;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct Cs2ApiState {
    pub manager: Arc<Cs2ServerManager>,
}

#[derive(Debug, Deserialize)]
pub struct MatchFinishPayload {
    #[serde(rename = "matchId")]
    pub match_id: String,
    pub winner: Option<String>,
    pub team1_score: Option<u32>,
    pub team2_score: Option<u32>,
    pub stats: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct MatchCancelPayload {
    #[serde(rename = "matchId")]
    pub match_id: String,
    pub reason: Option<String>,
}

pub fn router(state: Cs2ApiState) -> Router {
    Router::new()
        .route("/api/cs2/server-info", get(server_info))
        .route("/api/cs2/current-match", get(current_match))
        .route("/api/cs2/match-finish", post(match_finish))
        .route("/api/cs2/match-cancel", post(match_cancel))
        .with_state(state)
}

async fn server_info(State(state): State<Cs2ApiState>) -> impl IntoResponse {
    let all = state.manager.get_all_active_matches().await;
    let first = all.first().cloned();
    Json(json!({
        "serverIp": state.manager.server_ip(),
        "activeMatch": {
            "active": !all.is_empty(),
            "match": first,
            "matches": all
        }
    }))
}

async fn current_match(State(state): State<Cs2ApiState>) -> impl IntoResponse {
    let all = state.manager.get_all_active_matches().await;
    let active = !all.is_empty();
    let primary = all.first().cloned();
    let count = all.len();

    Json(json!({
        "active": active,
        "match": primary,
        "matches": all,
        "count": count
    }))
}

async fn match_finish(
    State(state): State<Cs2ApiState>,
    Json(payload): Json<MatchFinishPayload>,
) -> impl IntoResponse {
    let match_id = &payload.match_id;
    info!(
        "[CS2 API Rust] Fin de partida reportado para {match_id}. Ganador: {:?}, Marcador: {:?} - {:?}",
        payload.winner, payload.team1_score, payload.team2_score
    );

    // 1. Limpiar la partida activa de la memoria RAM
    let _ = state.manager.clear_match(match_id).await;

    // 2. Verificar auto-apagado de máquina virtual en AWS EC2 si ya no hay partidas
    let _ = state.manager.auto_shutdown_if_idle().await;

    Json(json!({
        "success": true,
        "matchId": match_id,
        "statsSaved": payload.stats.is_some(),
        "resolved": true,
        "message": "Estadísticas registradas y match finalizado correctamente en Rust."
    }))
}

async fn match_cancel(
    State(state): State<Cs2ApiState>,
    Json(payload): Json<MatchCancelPayload>,
) -> impl IntoResponse {
    let match_id = &payload.match_id;
    info!(
        "[CS2 API Rust] Cancelación de partida reportada para {match_id}. Motivo: {:?}",
        payload.reason
    );

    let _ = state.manager.clear_match(match_id).await;
    let _ = state.manager.auto_shutdown_if_idle().await;

    Json(json!({
        "success": true,
        "matchId": match_id,
        "message": "Match cancelado y limpiado de la memoria en Rust."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use common::config::Config;
    use tower::ServiceExt;

    fn test_manager() -> Arc<Cs2ServerManager> {
        let mut vars = std::collections::HashMap::new();
        vars.insert("ADMIN_JWT_SECRET".into(), "test_secret_1234567890".into());
        vars.insert("BASE_URL".into(), "https://machupeek.com".into());
        vars.insert("DISCORD_CLIENT_ID".into(), "id".into());
        vars.insert("DISCORD_CLIENT_SECRET".into(), "secret".into());
        vars.insert("DISCORD_TOKEN".into(), "token".into());
        vars.insert("DISCORD_GUILD_ID".into(), "guild".into());

        let cfg = Config::from_map(&vars).unwrap();
        Arc::new(Cs2ServerManager::new(&cfg))
    }

    #[tokio::test]
    async fn server_info_and_current_match_endpoints() {
        let manager = test_manager();
        let app = router(Cs2ApiState {
            manager: manager.clone(),
        });

        // 1. Inicialmente sin partidas
        let req = Request::builder()
            .uri("/api/cs2/current-match")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 2. Registrar una partida
        manager
            .publish_match("match_test_1".into(), None, "de_mirage".into(), 10.0, 10, None)
            .await;

        let req = Request::builder()
            .uri("/api/cs2/current-match")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 3. Finalizar partida
        let finish_body = json!({
            "matchId": "match_test_1",
            "winner": "team1",
            "team1_score": 13,
            "team2_score": 7
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/cs2/match-finish")
            .header("Content-Type", "application/json")
            .body(Body::from(finish_body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 4. Ya no hay partidas activas
        assert_eq!(manager.get_all_active_matches().await.len(), 0);
    }
}
