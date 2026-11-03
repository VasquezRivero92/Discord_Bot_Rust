//! API publica de solo lectura (`routes/publicApi.js`, montada en `/api`):
//! `GET /api/products` (108-121) y `GET /api/reclamaciones/:code` (209-223).
//!
//! `POST /api/reclamaciones` y `POST /api/checkout/purchase` escriben `claims`/`orders` y siguen
//! en el proxy: este router no define esos metodos, el llamador pone el proxy como fallback.

use std::sync::{Arc, OnceLock};

use axum::extract::{Path, State};
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
    Router::new()
        .route("/api/products", get(list_products))
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
