//! API publica de solo lectura (`routes/publicApi.js`, montada en `/api`):
//! `GET /api/products` (108-121) y `GET /api/reclamaciones/:code` (209-223).
//!
//! `POST /api/reclamaciones` y `POST /api/checkout/purchase` escriben `claims`/`orders` y siguen
//! en el proxy: este router no define esos metodos, el llamador pone el proxy como fallback.

use std::sync::{Arc, OnceLock};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use serde::Deserialize;
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

/// `POST /api/checkout/purchase`: compra en la tienda mediante depósito verificado en Yape.

/// `POST /api/reclamaciones`: registra un reclamo en el libro de reclamaciones virtual INDECOPI.
pub fn claims_post_router(firestore: Arc<data::FirestoreClient>) -> Router {
    Router::new()
        .route("/api/reclamaciones", post(create_claim_handler))
        .with_state(firestore)
}

pub fn checkout_router(firestore: Arc<data::FirestoreClient>) -> Router {
    Router::new()
        .route("/api/checkout/purchase", post(checkout_purchase_handler))
        .with_state(firestore)
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

/// Foto genérica de Steam cuando no se puede resolver la del jugador.
const DEFAULT_STEAM_AVATAR: &str =
    "https://avatars.steamstatic.com/fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb_medium.jpg";

/// `GET /api/lobby/steam-avatar/{steamId}` (redirige a la foto) y
/// `GET /api/lobby/steam-avatars?ids=a,b` (`{ avatars: { id: url } }`), de `routes/lobby.js`.
/// El front pone la primera como `src` de una `<img>`: tiene que ser una imagen, no un JSON.
pub fn steam_avatar_router(avatars: Arc<dyn crate::steam_avatars::SteamAvatars>) -> Router {
    Router::new()
        .route("/api/lobby/steam-avatar/{steam_id}", get(steam_avatar))
        .route("/api/lobby/steam-avatars", get(steam_avatars_batch))
        .with_state(avatars)
}

fn redirect_to(url: &str, cache_control: Option<&str>) -> Response {
    let mut res = Response::builder()
        .status(StatusCode::FOUND)
        .header(axum::http::header::LOCATION, url);
    if let Some(cc) = cache_control {
        res = res.header(axum::http::header::CACHE_CONTROL, cc);
    }
    res.body(axum::body::Body::empty())
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

async fn steam_avatar(
    State(avatars): State<Arc<dyn crate::steam_avatars::SteamAvatars>>,
    Path(steam_id): Path<String>,
) -> Response {
    let steam_id = steam_id.trim().to_owned();
    if !crate::steam_avatars::is_steam_id(&steam_id) {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({"error": "SteamID64 inválido."}),
        );
    }
    match avatars.fetch_many(&[steam_id.clone()]).await.remove(&steam_id) {
        Some(url) => redirect_to(&url, Some("public, max-age=7200")),
        None => redirect_to(DEFAULT_STEAM_AVATAR, None),
    }
}

async fn steam_avatars_batch(
    State(avatars): State<Arc<dyn crate::steam_avatars::SteamAvatars>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let ids: Vec<String> = q
        .get("ids")
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
        .unwrap_or_default();
    // Como el legacy, solo se consultan SteamID64 válidos y sin repetir.
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|id| crate::steam_avatars::is_steam_id(id) && seen.insert(id.clone()))
        .collect();
    let found = if ids.is_empty() {
        Default::default()
    } else {
        avatars.fetch_many(&ids).await
    };
    let mut res = json_response(StatusCode::OK, &json!({ "avatars": found }));
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=3600"),
    );
    res
}

/// Lo que necesita el detalle de una partida del historial.
#[derive(Clone)]
pub struct MatchDetailState {
    pub docs: Arc<dyn data::AdminDocsRepo>,
    pub users: Arc<dyn data::UsersRepo>,
    pub avatars: Arc<dyn crate::steam_avatars::SteamAvatars>,
    pub faceit: Arc<dyn crate::steam_avatars::FaceitSteamIds>,
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

    crate::steam_avatars::enrich_match(
        &mut history,
        &mut stats,
        &*st.users,
        &*st.avatars,
        &*st.faceit,
    )
    .await;

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


#[derive(Deserialize, Default)]
struct PurchaseItem {
    id: Option<String>,
    quantity: Option<Value>,
}

#[derive(Deserialize, Default)]
struct PurchaseCustomer {
    name: Option<String>,
    email: Option<String>,
    #[serde(rename = "discordId")]
    discord_id: Option<String>,
    phone: Option<String>,
}

#[derive(Deserialize, Default)]
struct PurchasePayload {
    items: Option<Vec<PurchaseItem>>,
    customer: Option<PurchaseCustomer>,
    #[serde(rename = "paymentMethod")]
    payment_method: Option<String>,
    #[serde(rename = "yapeOperationNumber")]
    yape_operation_number: Option<String>,
}

async fn checkout_purchase_handler(
    State(firestore): State<Arc<data::FirestoreClient>>,
    Json(payload): Json<PurchasePayload>,
) -> Response {
    let Some(items) = payload.items.filter(|i| !i.is_empty()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El carrito de compras está vacío." }),
        );
    };

    let Some(customer) = payload.customer else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes proporcionar tu nombre y correo electrónico." }),
        );
    };
    if customer.name.as_deref().unwrap_or("").trim().is_empty()
        || customer.email.as_deref().unwrap_or("").trim().is_empty()
    {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes proporcionar tu nombre y correo electrónico." }),
        );
    }

    let store_products = products().as_array().expect("products array");
    let mut total_amount = 0.0;
    let mut validated_items = Vec::new();

    for item in items {
        let Some(id) = item.id.filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let qty = match item.quantity {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(1),
            Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(1),
            _ => 1,
        }
        .max(1);
        if let Some(prod) = store_products
            .iter()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(&id))
        {
            let price = prod.get("price").and_then(Value::as_f64).unwrap_or(0.0);
            let name = prod.get("name").and_then(Value::as_str).unwrap_or(&id);
            let item_total = ((price * qty as f64) * 100.0).round() / 100.0;
            total_amount += item_total;
            validated_items.push(json!({
                "id": id,
                "name": name,
                "unitPrice": price,
                "quantity": qty,
                "itemTotal": item_total,
            }));
        }
    }

    if validated_items.is_empty() || total_amount <= 0.0 {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "No hay productos válidos en el pedido." }),
        );
    }
    total_amount = (total_amount * 100.0).round() / 100.0;

    if let Some(pm) = &payload.payment_method {
        if pm != "yape_transferencia" {
            return json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "El único método disponible es la verificación de depósito Yape." }),
            );
        }
    }

    let Some(yape_op) = payload
        .yape_operation_number
        .filter(|s| !s.trim().is_empty())
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Ingresa un número de operación Yape válido." }),
        );
    };

    let customer_val = json!({
        "name": customer.name,
        "email": customer.email,
        "discordId": customer.discord_id,
        "phone": customer.phone,
    });
    let items_val = json!(validated_items);

    match firestore
        .checkout_purchase(&yape_op, total_amount, &customer_val, &items_val)
        .await
    {
        Ok(order_id) => json_response(
            StatusCode::OK,
            &json!({
                "success": true,
                "orderId": order_id,
                "referenceCode": yape_op,
                "totalAmount": total_amount,
                "currency": "PEN",
                "items": validated_items,
                "customer": customer_val,
                "message": "¡Tu depósito Yape fue verificado exitosamente! Hemos enviado el comprobante a tu correo."
            }),
        ),
        Err(e) => {
            let (status, err_msg) = match e {
                data::CheckoutError::Invalid(msg) => (StatusCode::BAD_REQUEST, msg),
                data::CheckoutError::PaymentNotFound => (
                    StatusCode::BAD_REQUEST,
                    "No encontramos ese depósito. Verifica el número de operación y espera unos segundos si acabas de yapear.".to_string(),
                ),
                data::CheckoutError::PaymentNotEntry => (
                    StatusCode::BAD_REQUEST,
                    "El número de operación indicado no corresponde a un depósito recibido.".to_string(),
                ),
                data::CheckoutError::AmountMismatch(monto) => (
                    StatusCode::BAD_REQUEST,
                    format!("El depósito es de S/ {monto:.2} y no coincide con el monto solicitado."),
                ),
                data::CheckoutError::AlreadyUsed => (
                    StatusCode::CONFLICT,
                    "Este depósito Yape ya ha sido utilizado en otro pedido.".to_string(),
                ),
                data::CheckoutError::Data(err) => {
                    tracing::error!("error interno en checkout_purchase: {err}");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Error al procesar el pago. Por favor verifica tus datos e inténtalo nuevamente.".to_string(),
                    )
                }
            };
            json_response(status, &json!({ "error": err_msg }))
        }
    }
}

#[derive(Deserialize, Default)]
struct CreateClaimPayload {
    #[serde(rename = "consumerName", default)]
    consumer_name: String,
    #[serde(rename = "consumerDocType", default)]
    consumer_doc_type: String,
    #[serde(rename = "consumerDocNumber", default)]
    consumer_doc_number: String,
    #[serde(rename = "consumerEmail", default)]
    consumer_email: String,
    #[serde(rename = "consumerPhone", default)]
    consumer_phone: String,
    #[serde(rename = "consumerAddress", default)]
    consumer_address: String,
    #[serde(rename = "isMinor", default)]
    is_minor: bool,
    #[serde(rename = "parentName", default)]
    parent_name: String,
    #[serde(rename = "parentDocNumber", default)]
    parent_doc_number: String,
    #[serde(rename = "contractedType", default)]
    contracted_type: String,
    #[serde(rename = "claimedAmount", default)]
    claimed_amount: Value,
    #[serde(rename = "goodsDescription", default)]
    goods_description: String,
    #[serde(rename = "claimType", default)]
    claim_type: String,
    #[serde(rename = "claimDetail", default)]
    claim_detail: String,
    #[serde(rename = "consumerRequest", default)]
    consumer_request: String,
}

fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    if let Some((user, domain)) = email.split_once('@') {
        !user.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
    } else {
        false
    }
}

async fn create_claim_handler(
    State(firestore): State<Arc<data::FirestoreClient>>,
    Json(payload): Json<CreateClaimPayload>,
) -> Response {
    let consumer_name = payload.consumer_name.trim();
    let consumer_doc_number = payload.consumer_doc_number.trim();
    let consumer_email = payload.consumer_email.trim();
    let claim_detail = payload.claim_detail.trim();
    let consumer_request = payload.consumer_request.trim();

    if consumer_name.is_empty()
        || consumer_doc_number.is_empty()
        || consumer_email.is_empty()
        || claim_detail.is_empty()
        || consumer_request.is_empty()
    {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Por favor completa todos los campos obligatorios requeridos por INDECOPI."
            }),
        );
    }

    if !is_valid_email(consumer_email) {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "Ingresa un correo electrónico válido."
            }),
        );
    }

    let year = chrono::Utc::now().format("%Y").to_string();
    let ts = chrono::Utc::now().timestamp_millis();
    let nanos = chrono::Utc::now().timestamp_subsec_nanos();
    let rand_num = 1000 + (nanos % 9000);
    let claim_code = format!("LR-{}-{:04}{:02}", year, ts % 10000, rand_num % 100);

    let doc_type = if payload.consumer_doc_type.trim().is_empty() {
        "DNI".to_string()
    } else {
        payload.consumer_doc_type.trim().to_string()
    };

    let parent = if payload.is_minor {
        json!({
            "name": payload.parent_name.trim(),
            "docNumber": payload.parent_doc_number.trim(),
        })
    } else {
        Value::Null
    };

    let amount = match payload.claimed_amount {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().replace(',', ".").parse().unwrap_or(0.0),
        _ => 0.0,
    };

    let contracted_type = if payload.contracted_type.trim().to_uppercase() == "PRODUCTO" {
        "PRODUCTO"
    } else {
        "SERVICIO"
    };

    let claim_type = if payload.claim_type.trim().to_uppercase() == "QUEJA" {
        "QUEJA"
    } else {
        "RECLAMO"
    };

    let now_iso = chrono::Utc::now().to_rfc3339();

    let clean_claim = json!({
        "claimCode": claim_code,
        "consumer": {
            "name": consumer_name,
            "docType": doc_type,
            "docNumber": consumer_doc_number,
            "email": consumer_email,
            "phone": payload.consumer_phone.trim(),
            "address": payload.consumer_address.trim(),
            "isMinor": payload.is_minor,
            "parent": parent,
        },
        "goods": {
            "type": contracted_type,
            "amount": amount,
            "currency": "PEN",
            "description": payload.goods_description.trim(),
        },
        "claim": {
            "type": claim_type,
            "detail": claim_detail,
            "request": consumer_request,
        },
        "merchant": {
            "companyName": "MachuPeek",
            "address": "Online",
            "email": "machupeek@gmail.com",
            "phone": "+51 949 165 677"
        },
        "legalNotice": "Conforme al Código de Protección y Defensa del Consumidor (Ley N° 29571) y el D.S. 011-2011-PCM, el proveedor responderá en un plazo máximo de quince (15) días hábiles.",
        "submittedAt": now_iso,
        "createdAt": now_iso,
        "createdAtIso": now_iso,
        "status": "PENDIENTE",
        "responseDeadlineDays": 15,
    });

    if let Err(e) = firestore.set_doc("claims", &claim_code, &clean_claim).await {
        tracing::error!("Error al guardar reclamación {claim_code}: {e}");
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({
                "error": "Ocurrió un error al registrar la reclamación. Por favor inténtalo de nuevo."
            }),
        );
    }

    json_response(
        StatusCode::CREATED,
        &json!({
            "success": true,
            "claimCode": claim_code,
            "claim": clean_claim,
            "deadlineDays": 15,
            "message": "Tu reclamación ha sido registrada en nuestro Libro de Reclamaciones Virtual. Se ha generado tu Hoja de Reclamación con valor legal."
        }),
    )
}
