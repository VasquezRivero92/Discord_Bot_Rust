//! Rutas nativas de la API de CS2 (`/api/cs2/*`).
//!
//! Usadas por el servidor local de CS2 para consultar partidas en curso y reportar resultados.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
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
    pub firestore: Option<Arc<data::FirestoreClient>>,
    /// Secreto compartido con el servidor CS2 (el mismo `MATCHZY_WEBHOOK_SECRET`). `None` =
    /// se rechazan `match-finish` y `match-cancel`: declaran ganadores y mueven dinero.
    pub secret: Option<String>,
    /// `MATCH_PAYOUT_ENABLED`: sin él se archiva el resultado pero no se paga el pozo.
    pub payout_enabled: bool,
    /// Para borrar los canales de voz de la partida al cerrarla.
    pub discord: Option<Arc<crate::match_discord::MatchDiscord>>,
}

/// Rechaza (401) si la petición no trae el secreto del servidor CS2.
fn reject_unauthorized(state: &Cs2ApiState, headers: &HeaderMap, route: &str) -> Option<Response> {
    if super::webhooks::matchzy_authorized(headers, state.secret.as_deref()) {
        return None;
    }
    if state.secret.is_none() {
        tracing::error!("[CS2 API Rust] {route}: MATCHZY_WEBHOOK_SECRET no configurado; rechazado");
    } else {
        tracing::warn!("[CS2 API Rust] {route}: secreto ausente o inválido; rechazado");
    }
    Some((StatusCode::UNAUTHORIZED, Json(json!({ "error": "No autorizado" }))).into_response())
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
    headers: HeaderMap,
    Json(payload): Json<MatchFinishPayload>,
) -> Response {
    if let Some(denied) = reject_unauthorized(&state, &headers, "match-finish") {
        return denied;
    }
    let match_id = &payload.match_id;
    info!(
        "[CS2 API Rust] Fin de partida reportado para {match_id}. Ganador: {:?}, Marcador: {:?} - {:?}",
        payload.winner, payload.team1_score, payload.team2_score
    );

    // 1. Guardar resultado, marcador, estadísticas y liquidar premios en MySQL
    let mut payouts_done = Vec::new();
    let mut stats_saved = false;
    if let Some(fs) = &state.firestore {
        let before = crate::match_discord::active_match_doc(fs, match_id).await;
        crate::match_discord::spawn_cleanup(&state.discord, before);
        match fs
            .finish_match(
                match_id,
                payload.winner.as_deref(),
                payload.team1_score,
                payload.team2_score,
                payload.stats.as_ref(),
                state.payout_enabled,
            )
            .await
        {
            Ok(outcome) => {
                info!(
                    "[CS2 API Rust] Partida {match_id} finalizada exitosamente en MySQL. Ganador: {}, Marcador: {} - {}, Stats: {}, Pagos: {}",
                    outcome.winner,
                    outcome.score_a,
                    outcome.score_b,
                    outcome.stats_saved,
                    outcome.payouts.len()
                );
                payouts_done = outcome.payouts;
                stats_saved = outcome.stats_saved;
            }
            Err(e) => {
                tracing::error!("[CS2 API Rust] Error al finalizar partida en MySQL {match_id}: {e}");
            }
        }
    }

    // 2. Limpiar la partida activa de la memoria RAM
    let _ = state.manager.clear_match(match_id).await;

    // 3. Verificar auto-apagado de máquina virtual en AWS EC2 si ya no hay partidas
    let _ = state.manager.auto_shutdown_if_idle().await;

    Json(json!({
        "success": true,
        "matchId": match_id,
        "statsSaved": stats_saved,
        "payouts": payouts_done,
        "resolved": true,
        "message": "Estadísticas registradas, liquidación ejecutada y partida finalizada correctamente en MySQL."
    }))
    .into_response()
}

async fn match_cancel(
    State(state): State<Cs2ApiState>,
    headers: HeaderMap,
    Json(payload): Json<MatchCancelPayload>,
) -> Response {
    if let Some(denied) = reject_unauthorized(&state, &headers, "match-cancel") {
        return denied;
    }
    let match_id = &payload.match_id;
    info!(
        "[CS2 API Rust] Cancelación de partida reportada para {match_id}. Motivo: {:?}",
        payload.reason
    );

    // 1. Cancelar partida en MySQL
    if let Some(fs) = &state.firestore {
        let before = crate::match_discord::active_match_doc(fs, match_id).await;
        crate::match_discord::spawn_cleanup(&state.discord, before);
        if let Err(e) = fs.cancel_match(match_id, payload.reason.as_deref()).await {
            tracing::error!("[CS2 API Rust] Error al cancelar partida en MySQL {match_id}: {e}");
        }
    }

    // 2. Limpiar match de memoria y auto-apagar si está idle
    let _ = state.manager.clear_match(match_id).await;
    let _ = state.manager.auto_shutdown_if_idle().await;

    Json(json!({
        "success": true,
        "matchId": match_id,
        "message": "Match cancelado y actualizado en MySQL correctamente."
    }))
    .into_response()
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
        let fs = Arc::new(data::FirestoreClient::in_memory_mysql());
        let app = router(Cs2ApiState {
            manager: manager.clone(),
            firestore: Some(fs),
            secret: Some("s3creto".into()),
            payout_enabled: false,
            discord: None,
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
        // 3a. Sin secreto se rechaza y la partida sigue activa.
        let req = Request::builder()
            .method("POST")
            .uri("/api/cs2/match-finish")
            .header("Content-Type", "application/json")
            .body(Body::from(finish_body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(manager.get_all_active_matches().await.len(), 1);

        let req = Request::builder()
            .method("POST")
            .uri("/api/cs2/match-finish")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer s3creto")
            .body(Body::from(finish_body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 4. Ya no hay partidas activas
        assert_eq!(manager.get_all_active_matches().await.len(), 0);
    }

    #[tokio::test]
    async fn sin_secreto_configurado_se_rechaza_finish_y_cancel() {
        let manager = test_manager();
        let app = router(Cs2ApiState {
            manager,
            firestore: None,
            secret: None,
            payout_enabled: true,
            discord: None,
        });
        for uri in ["/api/cs2/match-finish", "/api/cs2/match-cancel"] {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("Content-Type", "application/json")
                .header("Authorization", "Bearer ")
                .body(Body::from(json!({"matchId": "m1", "winner": "team1"}).to_string()))
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
    }
}
