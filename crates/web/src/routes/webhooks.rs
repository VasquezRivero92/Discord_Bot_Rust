//! Receptores oficiales de Webhooks (MatchZy CS2, Faceit y pasarelas de pago).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cs2::Cs2ServerManager;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct WebhooksState {
    pub cs2_manager: Arc<Cs2ServerManager>,
    /// Firestore para pagar premios en `series_end`. `None` = no se paga.
    pub firestore: Option<Arc<data::FirestoreClient>>,
    /// Interruptor (`MATCH_PAYOUT_ENABLED=1`): mientras el Node también pueda pagar, apagado.
    pub payout_enabled: bool,
}

/// Equipo ganador del payload de MatchZy: `"team1"` o `{ "team": "team1" }`.
fn winner_team(winner: &Option<serde_json::Value>) -> Option<String> {
    match winner.as_ref()? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(o) => o.get("team")?.as_str().map(str::to_string),
        _ => None,
    }
}

/// Paga el pozo de `match_id` a los ganadores. Falla cerrado: sin datos claros no se paga y
/// se deja un error en el log para resolverlo a mano. Es idempotente (`match_payouts`).
async fn pay_series_winners(
    state: &WebhooksState,
    match_id: &str,
    winner: &Option<serde_json::Value>,
) {
    let (true, Some(fs)) = (state.payout_enabled, state.firestore.as_ref()) else {
        return;
    };
    let Some(team) = winner_team(winner) else {
        tracing::error!("[Payout] {match_id}: series_end sin equipo ganador; no se paga");
        return;
    };
    let mut doc = None;
    for col in ["faceit_active_matches", "faceit_matches_history"] {
        match fs.get_doc::<serde_json::Value>(col, match_id).await {
            Ok(Some(d)) => {
                doc = Some(d);
                break;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!("[Payout] {match_id}: no se pudo leer {col}: {e}");
                return;
            }
        }
    }
    let Some(doc) = doc else {
        tracing::error!("[Payout] {match_id}: partida no encontrada; no se paga");
        return;
    };
    let plan = match data::plan_payout(&doc, &team) {
        Ok(p) => p,
        Err(why) => {
            tracing::error!("[Payout] {match_id}: {why}; no se paga");
            return;
        }
    };
    match fs
        .pay_match_winners(
            match_id,
            plan.bet_cents,
            plan.total_players,
            &plan.winners,
            "matchzy",
        )
        .await
    {
        Ok(data::PayoutOutcome::Paid(shares)) => {
            info!("[Payout] {match_id}: pagado a {} ganadores {shares:?}", shares.len())
        }
        Ok(data::PayoutOutcome::AlreadyPaid) => info!("[Payout] {match_id}: ya estaba pagada"),
        Ok(data::PayoutOutcome::NothingToPay) => {
            info!("[Payout] {match_id}: sin apuesta, nada que pagar")
        }
        Err(e) => tracing::error!("[Payout] {match_id}: error al pagar: {e}"),
    }
}

#[derive(Debug, Deserialize)]
pub struct MatchZyWinner {
    pub team: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MatchZyPayload {
    pub event: Option<String>,
    pub matchid: Option<String>,
    pub winner: Option<serde_json::Value>,
    pub team1_series_score: Option<u32>,
    pub team2_series_score: Option<u32>,
}

pub fn router(state: WebhooksState) -> Router {
    Router::new()
        .route("/webhook/faceit", get(faceit_verify).post(faceit_event))
        .route("/webhooks/faceit", get(faceit_verify).post(faceit_event))
        .route("/api/matchzy/event", post(matchzy_event))
        .route("/webhook/yape-transaction", post(yape_transaction))
        .with_state(state)
}

async fn faceit_verify() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "message": "Endpoint de Webhook de FACEIT activo. Este endpoint está listo para recibir eventos POST de FACEIT.",
        "endpoint": "/webhook/faceit"
    }))
}

async fn faceit_event(
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let signature = headers
        .get("x-faceit-signature")
        .or_else(|| headers.get("x-webhook-signature"))
        .and_then(|h| h.to_str().ok());

    info!(
        "[Faceit Webhook Rust] Recibido evento POST con firma: {:?}",
        signature
    );

    if let Some(Json(payload)) = body {
        if let Some(event) = payload.get("event").and_then(|e| e.as_str()) {
            info!("[Faceit Webhook Rust] Procesando evento: {event}");
        }
    }

    (StatusCode::OK, "OK")
}

async fn matchzy_event(
    State(state): State<WebhooksState>,
    Json(payload): Json<MatchZyPayload>,
) -> impl IntoResponse {
    let event = payload.event.as_deref().unwrap_or("unknown");
    let match_id = payload.matchid.as_deref().unwrap_or("unknown");

    info!("[MatchZy Webhook Rust] Evento: '{event}' para match: '{match_id}'");

    // El premio se paga solo al terminar la serie, nunca en `map_end`. Se paga antes de limpiar
    // la partida de memoria.
    if event == "series_end" {
        pay_series_winners(&state, match_id, &payload.winner).await;
    }

    if event == "series_end" || event == "map_end" {
        info!("[MatchZy Webhook Rust] Serie o mapa finalizado. Resolviendo partida {match_id}...");

        // 1. Limpiar match de la memoria RAM del servidor CS2
        state.cs2_manager.clear_match(match_id).await;

        // 2. Comprobar auto-apagado de máquina virtual en AWS EC2 (Ahorro de costos)
        let _ = state.cs2_manager.auto_shutdown_if_idle().await;
    }

    Json(json!({
        "success": true,
        "event": event,
        "matchId": match_id
    }))
}

async fn yape_transaction(
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let auth = headers.get("x-yape-signature").and_then(|h| h.to_str().ok());
    info!("[Yape Webhook Rust] Recibida notificación de transacción Yape. Auth: {:?}", auth);

    if let Some(Json(b)) = body {
        info!("[Yape Webhook Rust] Datos recibidos de transacción: {:?}", b);
    }

    Json(json!({ "success": true, "status": "processed" }))
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
    async fn faceit_verify_get_returns_200() {
        let mgr = test_manager();
        let app = router(WebhooksState { cs2_manager: mgr, firestore: None, payout_enabled: false });

        let req = Request::builder()
            .uri("/webhook/faceit")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn matchzy_event_handles_series_end() {
        let mgr = test_manager();
        mgr.publish_match("test_match_99".into(), None, "de_nuke".into(), 5.0, 10, None)
            .await;
        assert_eq!(mgr.get_all_active_matches().await.len(), 1);

        let app = router(WebhooksState { cs2_manager: mgr.clone(), firestore: None, payout_enabled: false });

        let payload = json!({
            "event": "series_end",
            "matchid": "test_match_99",
            "winner": "team1",
            "team1_series_score": 13,
            "team2_series_score": 5
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/matchzy/event")
            .header("Content-Type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Se limpió el match
        assert_eq!(mgr.get_all_active_matches().await.len(), 0);
    }
}
