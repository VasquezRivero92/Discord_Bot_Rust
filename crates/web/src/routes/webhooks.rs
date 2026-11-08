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
    /// Secreto compartido con MatchZy (`MATCHZY_WEBHOOK_SECRET`). `None` = se rechaza todo
    /// evento: sin él cualquiera podría declarar un ganador y cobrar el pozo.
    pub matchzy_secret: Option<String>,
    /// Para borrar los canales de voz de la partida al terminar la serie.
    pub discord: Option<Arc<crate::match_discord::MatchDiscord>>,
}

/// Compara sin cortar en el primer byte distinto (no filtra el secreto por tiempo).
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// El evento trae el secreto en `Authorization` (`Bearer <secreto>` o el secreto solo), que es
/// lo que envía MatchZy con `matchzy_remote_log_header_key "Authorization"`.
pub(crate) fn matchzy_authorized(headers: &HeaderMap, secret: Option<&str>) -> bool {
    let Some(secret) = secret.filter(|s| !s.is_empty()) else {
        return false;
    };
    // `x-matchzy-token` es la cabecera que el legado dejó configurada en el servidor CS2
    // (`matchzy_remote_log_header_key`); `Authorization` queda para configuraciones nuevas.
    let Some(value) = headers
        .get("x-matchzy-token")
        .or_else(|| headers.get(axum::http::header::AUTHORIZATION))
        .and_then(|h| h.to_str().ok())
    else {
        return false;
    };
    let token = value.strip_prefix("Bearer ").unwrap_or(value).trim();
    constant_time_eq(token.as_bytes(), secret.as_bytes())
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
    headers: HeaderMap,
    Json(payload): Json<MatchZyPayload>,
) -> axum::response::Response {
    if !matchzy_authorized(&headers, state.matchzy_secret.as_deref()) {
        if state.matchzy_secret.is_none() {
            tracing::error!("[MatchZy Webhook Rust] MATCHZY_WEBHOOK_SECRET no configurado; evento rechazado");
        } else {
            tracing::warn!("[MatchZy Webhook Rust] evento con secreto ausente o inválido; rechazado");
        }
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "No autorizado" })),
        )
            .into_response();
    }
    let event = payload.event.as_deref().unwrap_or("unknown");
    let match_id = payload.matchid.as_deref().unwrap_or("unknown");

    info!("[MatchZy Webhook Rust] Evento: '{event}' para match: '{match_id}'");

    // El premio se paga solo al terminar la serie, nunca en `map_end`. Se paga antes de limpiar
    // la partida de memoria.
    if event == "series_end" {
        if let Some(fs) = &state.firestore {
            let before = crate::match_discord::active_match_doc(fs, match_id).await;
            crate::match_discord::spawn_cleanup(&state.discord, before);
        }
        pay_series_winners(&state, match_id, &payload.winner).await;
        if let Some(fs) = &state.firestore {
            let winner_team = payload
                .winner
                .as_ref()
                .and_then(|w| w.get("team"))
                .and_then(serde_json::Value::as_str);
            let s1 = payload.team1_series_score;
            let s2 = payload.team2_series_score;
            // El premio ya lo liquidó `pay_series_winners` (con su interruptor): aquí solo se
            // archiva la partida, sin volver a pagar.
            let _ = fs.finish_match(match_id, winner_team, s1, s2, None, false).await;
        }
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
    .into_response()
}

async fn yape_transaction(
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    // No se registra la firma ni el cuerpo: traen datos personales del pago. Este receptor no
    // escribe nada; las recargas se verifican contra `yape_transacciones`.
    let signed = headers.contains_key("x-yape-signature");
    let has_body = body.is_some();
    info!("[Yape Webhook Rust] Notificación Yape recibida (firma: {signed}, cuerpo: {has_body})");

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
        let app = router(state(mgr, Some("s3creto")));

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

        let app = router(state(mgr.clone(), Some("s3creto")));

        let res = app
            .oneshot(series_end("test_match_99", Some("Bearer s3creto")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Se limpió el match
        assert_eq!(mgr.get_all_active_matches().await.len(), 0);
    }

    fn state(mgr: Arc<Cs2ServerManager>, secret: Option<&str>) -> WebhooksState {
        WebhooksState {
            cs2_manager: mgr,
            firestore: None,
            payout_enabled: false,
            matchzy_secret: secret.map(str::to_string),
            discord: None,
        }
    }

    fn series_end(match_id: &str, auth: Option<&str>) -> Request<Body> {
        let payload = json!({
            "event": "series_end",
            "matchid": match_id,
            "winner": "team1",
            "team1_series_score": 13,
            "team2_series_score": 5
        });
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/matchzy/event")
            .header("Content-Type", "application/json");
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        req.body(Body::from(payload.to_string())).unwrap()
    }

    #[tokio::test]
    async fn matchzy_event_sin_secreto_valido_se_rechaza_y_no_toca_la_partida() {
        let mgr = test_manager();
        mgr.publish_match("m_atacada".into(), None, "de_nuke".into(), 5.0, 10, None)
            .await;

        for (secret, auth) in [
            (Some("s3creto"), None),
            (Some("s3creto"), Some("Bearer otro")),
            (Some("s3creto"), Some("Bearer s3cretoX")),
            // Sin secreto configurado se rechaza todo, incluso sin cabecera.
            (None, None),
            (None, Some("Bearer ")),
        ] {
            let app = router(state(mgr.clone(), secret));
            let res = app.oneshot(series_end("m_atacada", auth)).await.unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{secret:?} / {auth:?}");
        }
        assert_eq!(mgr.get_all_active_matches().await.len(), 1);
    }

    #[test]
    fn acepta_el_secreto_con_o_sin_bearer() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "s3creto".parse().unwrap());
        assert!(matchzy_authorized(&h, Some("s3creto")));
        h.insert("authorization", "Bearer s3creto".parse().unwrap());
        assert!(matchzy_authorized(&h, Some("s3creto")));
        assert!(!matchzy_authorized(&h, Some("")));
        assert!(!matchzy_authorized(&h, None));
    }

    #[test]
    fn acepta_la_cabecera_x_matchzy_token_del_legado() {
        let mut h = HeaderMap::new();
        h.insert("x-matchzy-token", "s3creto".parse().unwrap());
        assert!(matchzy_authorized(&h, Some("s3creto")));
        h.insert("x-matchzy-token", "otro".parse().unwrap());
        assert!(!matchzy_authorized(&h, Some("s3creto")));
    }
}
