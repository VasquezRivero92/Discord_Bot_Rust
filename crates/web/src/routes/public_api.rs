//! API publica de solo lectura (`routes/publicApi.js`, montada en `/api`):
//! `GET /api/products` (108-121) y `GET /api/reclamaciones/:code` (209-223).
//!
//! `POST /api/reclamaciones` y `POST /api/checkout/purchase` escriben `claims`/`orders` y siguen
//! en el proxy: este router no define esos metodos, el llamador pone el proxy como fallback.

use std::sync::{Arc, OnceLock};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use data::ClaimsRepo;
use serde_json::{json, Value};

use super::json_response;

/// `STORE_PRODUCTS` de `routes/publicApi.js`. Regenerar con `node tools/gen-products.mjs`.
const PRODUCTS_JSON: &str = include_str!("products.json");

fn products() -> &'static Value {
    static PRODUCTS: OnceLock<Value> = OnceLock::new();
    PRODUCTS
        .get_or_init(|| serde_json::from_str(PRODUCTS_JSON).expect("products.json es JSON valido"))
}

/// Rutas de la API publica; las reclamaciones se leen con `claims`.
pub fn router(claims: Arc<dyn ClaimsRepo>) -> Router {
    products_router().merge(claims_router(claims))
}

/// Solo `GET /api/products` (no necesita Firestore).
pub fn products_router() -> Router {
    Router::new().route("/api/products", get(list_products))
}

/// Servidores cuyo historial comparten (MachuPeek y el hub de FACEIT anterior): el legacy los
/// une cuando se pide cualquiera de los dos.
const HISTORY_GUILDS: [&str; 2] = ["1274598531759345674", "1472731390675255410"];

/// `GET /api/lobby/matches-history` (y `/match-history`): últimas partidas del historial de
/// Firestore como `{ history }`. `limit` se acota a 1..=50 (20 si falta o no es un número).
/// El detalle `/{match_id}` no se monta aquí y sigue en el proxy.
pub fn matches_history_router(repo: Arc<dyn data::MatchHistoryRepo>) -> Router {
    Router::new()
        .route("/api/lobby/matches-history", get(list_matches_history))
        .route("/api/lobby/match-history", get(list_matches_history))
        .with_state(repo)
}

async fn list_matches_history(
    State(repo): State<Arc<dyn data::MatchHistoryRepo>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let limit = q
        .get("limit")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n != 0)
        .map(|n| n.clamp(1, 50) as u32)
        .unwrap_or(20);
    let guild = q
        .get("guildId")
        .map(String::as_str)
        .filter(|g| !g.is_empty());

    let guilds: Vec<Option<&str>> = match guild {
        Some(g) if HISTORY_GUILDS.contains(&g) => HISTORY_GUILDS.iter().map(|g| Some(*g)).collect(),
        other => vec![other],
    };

    let mut history = Vec::new();
    for g in guilds {
        match repo.list(limit, g).await {
            Ok(mut rows) => history.append(&mut rows),
            Err(e) => {
                tracing::error!("Error al obtener historial de partidas de Firestore: {e}");
            }
        }
    }

    let created_secs = |e: &data::MatchHistoryEntry| {
        e.created_at
            .as_ref()
            .and_then(|v| v.get("_seconds"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    history.sort_by_key(|e| std::cmp::Reverse(created_secs(e)));

    // Sin duplicados por `id`; las filas sin `id` se descartan, como en el legacy.
    let mut seen = std::collections::HashSet::new();
    history.retain(|e| match e.extra.get("id").and_then(Value::as_str) {
        Some(id) => seen.insert(id.to_owned()),
        None => false,
    });
    history.truncate(limit as usize);

    json_response(StatusCode::OK, &json!({ "history": history }))
}

/// Lo que necesita el detalle de una partida del historial.
#[derive(Clone)]
pub struct MatchDetailState {
    pub docs: Arc<dyn data::AdminDocsRepo>,
    pub users: Arc<dyn data::UsersRepo>,
    pub avatars: Arc<dyn crate::steam_avatars::SteamAvatars>,
}

/// `GET /api/lobby/matches-history/{id}` (y `/match-history/{id}`): la partida con sus
/// estadísticas y las fotos de perfil de Steam de quienes jugaron.
pub fn match_detail_router(state: MatchDetailState) -> Router {
    Router::new()
        .route("/api/lobby/matches-history/{match_id}", get(match_detail))
        .route("/api/lobby/match-history/{match_id}", get(match_detail))
        .with_state(state)
}

async fn match_detail(State(st): State<MatchDetailState>, Path(match_id): Path<String>) -> Response {
    // Un error de lectura es "no encontrada", como el legacy (`getMatchHistoryById` devuelve null).
    let Some(mut history) = st.docs.match_history_by_id(&match_id).await.ok().flatten() else {
        return json_response(
            StatusCode::NOT_FOUND,
            &json!({"error": "Partida no encontrada."}),
        );
    };
    let mut stats = st.docs.match_stats(&match_id).await.ok().flatten();
    if stats.is_none() {
        stats = history.get("stats").filter(|s| !s.is_null()).cloned();
    }

    crate::steam_avatars::enrich_match(&mut history, &mut stats, &*st.users, &*st.avatars).await;

    let mut body = json!({ "history": history });
    if let Some(stats) = stats {
        body["stats"] = stats;
    }
    json_response(StatusCode::OK, &body)
}

/// Solo `GET /api/reclamaciones/:code`. Sin fuente de reclamaciones no se monta y la
/// ruta sigue llegando a Node por el proxy.
pub fn claims_router(claims: Arc<dyn ClaimsRepo>) -> Router {
    Router::new()
        .route("/api/reclamaciones/{code}", get(get_claim))
        .with_state(claims)
}

async fn list_products() -> Response {
    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "products": products(),
            "currency": "PEN",
            "merchant": {
                "name": "MachuPeek",
                "address": "Online",
                "phone": "+51 949 165 677",
                "email": "machupeek@gmail.com",
            },
        }),
    )
}

async fn get_claim(
    State(claims): State<Arc<dyn ClaimsRepo>>,
    Path(code): Path<String>,
) -> Response {
    match claims.get_by_code(&code).await {
        Ok(Some(claim)) => match serde_json::to_value(&claim) {
            Ok(claim) => json_response(StatusCode::OK, &json!({"success": true, "claim": claim})),
            Err(_) => lookup_failed(),
        },
        Ok(None) => json_response(
            StatusCode::NOT_FOUND,
            &json!({"error": "No se encontró ninguna reclamación con el código especificado."}),
        ),
        // El detalle del error no sale nunca en la respuesta.
        Err(_) => lookup_failed(),
    }
}

fn lookup_failed() -> Response {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &json!({"error": "Error al consultar la reclamación."}),
    )
}
