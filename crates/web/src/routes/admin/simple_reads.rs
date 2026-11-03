//! Lecturas de administración de una sola colección (Tarea 7):
//!
//! | ruta | legacy (`routes/api.js`) | lectura (`database.js`) | respuesta |
//! |---|---|---|---|
//! | `GET /api/admin/claims` | ~1880 | `getClaims` ~3220 | `{ claims }` |
//! | `GET /api/admin/claims/:code` | ~1892 | `getClaim` ~3189 | `{ claim }` / 404 |
//! | `GET /api/admin/orders` | ~1956 | `getOrders` ~3268 | `{ orders }` |
//! | `GET /api/admin/logs` | ~1596 | `getRecentLogs` ~2070 | `{ logs }` (caché 30 s) |
//! | `GET /api/admin/support-calls` | ~1731 | `getSupportCalls` ~2174 | `{ calls }` |
//! | `GET /api/admin/messages` | ~1825 | `getMessageTemplates` ~2192 | `{ templates }` |
//! | `GET /api/admin/matches/history` | ~1486 | `getMatchHistory` ~1669 | `{ history }` |
//!
//! # Errores
//!
//! Todas esas funciones del legacy **tragan** cualquier error de Firestore (cuota
//! incluida) y devuelven un valor por defecto, así que el `catch` de la ruta
//! (`500 { error: 'Error al obtener ...' }`) nunca llega a ejecutarse por un
//! fallo de lectura. Aquí igual: ante un `Err` del repo se responde 200 con la
//! lista vacía (`claims`, `orders`, `logs`, `calls`, `history`), 404
//! `Reclamación no encontrada` (`getClaim` devuelve `null`) o las plantillas de
//! la última lectura correcta o las de por defecto (`messages`). El texto del
//! error solo va al log, nunca a la respuesta. El 500 con el mensaje de cada
//! ruta queda para un fallo inesperado del propio handler (serializar la
//! respuesta), que es lo que cubre el `catch` del legacy.
//!
//! # `limit`
//!
//! `parseInt(req.query.limit) || n` con la semántica exacta de JS (ver
//! [`super::js`]), y después lo que haga el legacy con ese número: `limit()` de
//! Firestore (logs), `slice(0, limit)` (claims, orders) o el `safeLimit` de
//! `getMatchHistory` (historial).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    extract::{FromRef, FromRequestParts, RawPathParams, RawQuery, State},
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use data::default_message_templates;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use super::js::{self, Radix};
use super::qs::{percent_decode_strict, query_param, QsParam};
use super::{resolve_target_guild_id, AdminContext, AdminError, AdminRepos};
use crate::routes::json_response;
use crate::session::SessionState;

/// `BOT_LOGS_TTL` (`routes/api.js:307`).
pub const BOT_LOGS_TTL: Duration = Duration::from_secs(30);
/// Tope de claves de la caché de logs (el `Map` del legacy no tiene tope).
pub const LOGS_CACHE_MAX_KEYS: usize = 1000;
/// `db.getSupportCalls(50, ...)` (`routes/api.js:1734`).
const SUPPORT_CALLS_LIMIT: u32 = 50;

const MSG_LOGS: &str = "Error al obtener logs";
const MSG_SUPPORT_CALLS: &str = "Error al obtener llamadas de soporte";
const MSG_MESSAGES: &str = "Error al obtener plantillas de mensajes";
const MSG_CLAIMS: &str = "Error al obtener reclamaciones";
const MSG_CLAIM: &str = "Error al obtener detalle de reclamación";
const MSG_CLAIM_NOT_FOUND: &str = "Reclamación no encontrada";
const MSG_ORDERS: &str = "Error al obtener órdenes";
const MSG_HISTORY: &str = "Error al obtener el historial de partidas";

/// Estado de estas rutas.
#[derive(Clone)]
pub(crate) struct ReadsState {
    session: SessionState,
    repos: AdminRepos,
    logs_cache: Arc<LogsCache>,
    /// Últimas plantillas leídas con éxito (el `messageTemplatesCache` al que
    /// el legacy recurre ante un error).
    last_templates: Arc<Mutex<Option<Value>>>,
}

impl FromRef<ReadsState> for SessionState {
    fn from_ref(s: &ReadsState) -> SessionState {
        s.session.clone()
    }
}

/// Las siete rutas, con su estado.
pub(crate) fn router(session: SessionState, repos: AdminRepos) -> Router {
    Router::new()
        .route("/api/admin/claims", get(claims))
        .route("/api/admin/claims/{code}", get(claim))
        .route("/api/admin/orders", get(orders))
        .route("/api/admin/logs", get(logs))
        .route("/api/admin/support-calls", get(support_calls))
        .route("/api/admin/messages", get(messages))
        .route("/api/admin/matches/history", get(match_history))
        .with_state(ReadsState {
            session,
            repos,
            logs_cache: Arc::default(),
            last_templates: Arc::default(),
        })
}

// ------------------------------------------------------------------ handlers

/// `GET /claims`: `status = req.query.status || null`,
/// `limit = parseInt(req.query.limit, 10) || 100`, `getClaims(limit, status)`.
async fn claims(
    State(st): State<ReadsState>,
    _ctx: AdminContext,
    RawQuery(q): RawQuery,
) -> Result<Response, AdminError> {
    let raw = q.as_deref().unwrap_or("");
    let status = status_param(raw)?;
    let limit = SliceLimit::new(js::or_default(limit_param(raw, Radix::Ten)?, 100.0));
    let list = match st.repos.claims.list(limit.repo, status.as_deref()).await {
        Ok(list) => limit.apply(list),
        Err(e) => {
            tracing::error!("Error al listar reclamos: {e}");
            Vec::new()
        }
    };
    Ok(body("claims", &list, MSG_CLAIMS))
}

/// `GET /claims/:code`: `getClaim(code)`; `null` (no existe o falló la
/// lectura) -> 404.
async fn claim(
    State(st): State<ReadsState>,
    ClaimCode(code): ClaimCode,
    _ctx: AdminContext,
) -> Response {
    match st.repos.claims.get_by_code(&code).await {
        Ok(Some(claim)) => body("claim", &claim, MSG_CLAIM),
        Ok(None) => not_found(),
        Err(e) => {
            tracing::error!("Error al buscar reclamo {code}: {e}");
            not_found()
        }
    }
}

/// `GET /orders`: como `/claims` sobre `getOrders`.
async fn orders(
    State(st): State<ReadsState>,
    _ctx: AdminContext,
    RawQuery(q): RawQuery,
) -> Result<Response, AdminError> {
    let raw = q.as_deref().unwrap_or("");
    let status = status_param(raw)?;
    let limit = SliceLimit::new(js::or_default(limit_param(raw, Radix::Ten)?, 100.0));
    let list = match st.repos.orders.list(limit.repo, status.as_deref()).await {
        Ok(list) => limit.apply(list),
        Err(e) => {
            tracing::error!("Error al listar órdenes: {e}");
            Vec::new()
        }
    };
    Ok(body("orders", &list, MSG_ORDERS))
}

/// `GET /logs`: `limit = parseInt(req.query.limit) || 50`, servidor objetivo,
/// caché de 30 s por `"{guild|global}:{limit}"` y `getRecentLogs(limit, guild)`.
/// Se cachea lo que se responde, también la lista vacía de un error (como el
/// legacy, que guarda el `[]` de `getRecentLogs`).
async fn logs(
    State(st): State<ReadsState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let limit = js::or_default(limit_param(q.as_deref().unwrap_or(""), Radix::Auto)?, 50.0);
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let key = format!(
        "{}:{}",
        target.as_deref().unwrap_or("global"),
        js::number_to_string(limit)
    );
    let now = Instant::now();
    if let Some(cached) = st.logs_cache.get(&key, now) {
        return Ok(logs_response(&cached));
    }
    let logs = match firestore_limit(limit) {
        Some(n) => match st.repos.logs.recent(n, target.as_deref()).await {
            Ok(entries) => match serde_json::to_value(&entries) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("Error en /logs: {e}");
                    return Ok(error_500(MSG_LOGS));
                }
            },
            Err(e) => {
                tracing::error!("Error al obtener logs: {e}");
                Value::Array(Vec::new())
            }
        },
        // Firestore (o su SDK) rechazaría ese `limit`: `getRecentLogs` da `[]`.
        None => {
            tracing::error!("Error al obtener logs: limit no válido para Firestore ({key})");
            Value::Array(Vec::new())
        }
    };
    let logs = Arc::new(logs);
    st.logs_cache.insert(key, now, logs.clone());
    Ok(logs_response(&logs))
}

fn logs_response(logs: &Value) -> Response {
    let mut m = Map::new();
    m.insert("logs".into(), logs.clone());
    json_response(StatusCode::OK, &Value::Object(m))
}

/// `GET /support-calls`: servidor objetivo y `getSupportCalls(50, guild)`.
async fn support_calls(
    State(st): State<ReadsState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let calls = match st
        .repos
        .support_calls
        .list(SUPPORT_CALLS_LIMIT, target.as_deref())
        .await
    {
        Ok(calls) => calls,
        Err(e) => {
            tracing::error!("Error al obtener llamadas de soporte de Firestore: {e}");
            Vec::new()
        }
    };
    Ok(body("calls", &calls, MSG_SUPPORT_CALLS))
}

/// `GET /messages`: `getMessageTemplates()`; ante un error, las últimas
/// plantillas leídas con éxito o las de por defecto.
async fn messages(State(st): State<ReadsState>, _ctx: AdminContext) -> Response {
    let templates = match st.repos.messages.templates().await {
        Ok(t) => {
            *st.last_templates.lock().expect("mutex envenenado") = Some(t.clone());
            t
        }
        Err(e) => {
            tracing::error!("Error al obtener plantillas de mensajes: {e}");
            st.last_templates
                .lock()
                .expect("mutex envenenado")
                .clone()
                .unwrap_or_else(default_message_templates)
        }
    };
    body("templates", &templates, MSG_MESSAGES)
}

/// `GET /matches/history`: `limit = parseInt(req.query.limit) || 20`, servidor
/// objetivo y `getMatchHistory(limit, guild)`.
async fn match_history(
    State(st): State<ReadsState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let limit = js::or_default(limit_param(q.as_deref().unwrap_or(""), Radix::Auto)?, 20.0);
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let history = match st
        .repos
        .match_history
        .list(history_safe_limit(limit), target.as_deref())
        .await
    {
        Ok(history) => history,
        Err(e) => {
            tracing::error!("Error al obtener historial de partidas de Firestore: {e}");
            Vec::new()
        }
    };
    Ok(body("history", &history, MSG_HISTORY))
}

// ------------------------------------------------------------------- helpers

/// `{ key: value }` con 200, o el 500 de la ruta si no se puede serializar.
fn body<T: Serialize + ?Sized>(key: &str, value: &T, msg_500: &'static str) -> Response {
    match serde_json::to_value(value) {
        Ok(v) => {
            let mut m = Map::new();
            m.insert(key.to_owned(), v);
            json_response(StatusCode::OK, &Value::Object(m))
        }
        Err(e) => {
            tracing::error!("{msg_500}: {e}");
            error_500(msg_500)
        }
    }
}

fn error_500(msg: &'static str) -> Response {
    json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({ "error": msg }))
}

fn not_found() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        &json!({ "error": MSG_CLAIM_NOT_FOUND }),
    )
}

/// `parseInt(req.query.limit[, 10])`: `NaN` sin `limit`. Un array de `qs`
/// pasa por `String(array)` (`join(',')`), como en JS.
fn limit_param(raw: &str, radix: Radix) -> Result<f64, AdminError> {
    match query_param(raw, "limit") {
        QsParam::Absent => Ok(f64::NAN),
        QsParam::One(s) => Ok(js::parse_int(&s, radix)),
        QsParam::Many(v) => Ok(js::parse_int(&v.join(","), radix)),
        QsParam::Unsupported => Err(AdminError::InvalidQuery("limit inválido")),
    }
}

/// `req.query.status || null`. Un array u objeto de `qs` no se reproduce
/// (el legacy filtraría `status == [array]`): 400.
fn status_param(raw: &str) -> Result<Option<String>, AdminError> {
    match query_param(raw, "status") {
        QsParam::Absent => Ok(None),
        QsParam::One(s) => Ok((!s.is_empty()).then_some(s)),
        QsParam::Many(_) | QsParam::Unsupported => Err(AdminError::InvalidQuery("status inválido")),
    }
}

/// `query.limit(limit)` de Firestore en Node para un `limit` ya no falsy:
/// `validateInteger` rechaza `±Infinity` en el cliente; protobufjs codifica el
/// `int32` con `| 0` y el servidor rechaza un límite negativo. `None` = la
/// consulta fallaría (y `getRecentLogs` daría `[]`); también para un `0` tras
/// el `| 0`, que no devolvería ningún documento.
pub(crate) fn firestore_limit(limit: f64) -> Option<u32> {
    if !limit.is_finite() {
        return None;
    }
    u32::try_from(js::to_int32(limit)).ok().filter(|n| *n > 0)
}

/// `claims.slice(0, limit)` (y `orders`) a través de `list(limit)` del repo,
/// que solo sabe cortar por delante: un `limit` negativo pide todo y quita
/// los `-limit` últimos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SliceLimit {
    /// `limit` para el repo.
    pub(crate) repo: u32,
    /// Elementos a quitar del final.
    pub(crate) drop_last: usize,
}

impl SliceLimit {
    /// Para un `limit` ya no falsy (entero o `±Infinity`).
    pub(crate) fn new(limit: f64) -> Self {
        if limit > 0.0 {
            Self {
                // `as` satura (`Infinity` -> `u32::MAX`): pedir más que la
                // colección entera da la colección entera.
                repo: limit as u32,
                drop_last: 0,
            }
        } else {
            Self {
                repo: u32::MAX,
                drop_last: (-limit) as usize,
            }
        }
    }

    fn apply<T>(self, mut list: Vec<T>) -> Vec<T> {
        let keep = list.len().saturating_sub(self.drop_last);
        list.truncate(keep);
        list
    }
}

/// `safeLimit = min(max(parseInt(limit, 10) || 20, 1), 50)` de
/// `getMatchHistory` sobre el número que le pasa la ruta (`parseInt(...) || 20`,
/// ya no falsy). `parseInt` de un número pasa por `String(n)`: desde `1e21`
/// queda la primera cifra (`"1e+21"` -> 1) e `Infinity` es `NaN`.
pub(crate) fn history_safe_limit(limit: f64) -> u32 {
    let reparsed = js::parse_int(&js::number_to_string(limit), Radix::Ten);
    let n = js::or_default(reparsed, 20.0);
    n.clamp(1.0, 50.0) as u32
}

/// `req.params.code`: el segmento crudo decodificado como `decodeURIComponent`
/// (Express lo hace al casar la ruta, antes de `requireAuth`). Si JS lanzaría
/// `URIError`, Express responde 400 antes de autenticar: aquí también.
struct ClaimCode(String);

impl<S: Send + Sync> FromRequestParts<S> for ClaimCode {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let raw = params
            .iter()
            .find(|(k, _)| *k == "code")
            .map(|(_, v)| v)
            .unwrap_or_default();
        percent_decode_strict(raw)
            .map(ClaimCode)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Bad Request").into_response())
    }
}

/// La caché de `/logs` del legacy (`botLogsCache`): cada clave guarda la lista
/// y el instante en que **empezó** la petición que la leyó; vale mientras
/// `now - instante < 30 s`. Sin carga única por clave (el legacy tampoco la
/// tiene). Con [`LOGS_CACHE_MAX_KEYS`] claves, al meter una nueva se purgan
/// las caducadas y, si siguen sin caber, se vacía.
#[derive(Default)]
pub(crate) struct LogsCache {
    entries: Mutex<HashMap<String, (Instant, Arc<Value>)>>,
}

impl LogsCache {
    fn get(&self, key: &str, now: Instant) -> Option<Arc<Value>> {
        let entries = self.entries.lock().expect("mutex envenenado");
        entries
            .get(key)
            .filter(|(at, _)| now.saturating_duration_since(*at) < BOT_LOGS_TTL)
            .map(|(_, logs)| logs.clone())
    }

    fn insert(&self, key: String, at: Instant, logs: Arc<Value>) {
        let mut entries = self.entries.lock().expect("mutex envenenado");
        if entries.len() >= LOGS_CACHE_MAX_KEYS && !entries.contains_key(&key) {
            entries.retain(|_, (t, _)| at.saturating_duration_since(*t) < BOT_LOGS_TTL);
            if entries.len() >= LOGS_CACHE_MAX_KEYS {
                entries.clear();
            }
        }
        entries.insert(key, (at, logs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(q: &str, radix: Radix, default: f64) -> f64 {
        js::or_default(limit_param(q, radix).unwrap(), default)
    }

    /// `parseInt(x) || default` frente a Node para las cuatro rutas con `limit`.
    #[test]
    fn limit_falls_back_like_parse_int_or_default() {
        for (q, logs, claims) in [
            ("", 50.0, 100.0),
            ("limit=abc", 50.0, 100.0),
            ("limit=0", 50.0, 100.0),
            ("limit=-5", -5.0, -5.0),
            ("limit=7x", 7.0, 7.0),
            ("limit=", 50.0, 100.0),
            ("limit", 50.0, 100.0),
            ("limit=3.9", 3.0, 3.0),
            ("limit=%20%205", 5.0, 5.0),
            ("limit=+5", 5.0, 5.0),
            ("limit=%2B5", 5.0, 5.0),
            ("limit=007", 7.0, 7.0),
            ("limit=0x10", 16.0, 100.0),
            ("limit=-0", 50.0, 100.0),
            ("limit=99999999999", 99_999_999_999.0, 99_999_999_999.0),
            ("limit=4&limit=9", 4.0, 4.0),
            ("limit[]=6", 6.0, 6.0),
        ] {
            assert_eq!(limit(q, Radix::Auto, 50.0), logs, "logs {q}");
            assert_eq!(limit(q, Radix::Ten, 100.0), claims, "claims {q}");
        }
        assert_eq!(
            limit_param("limit[a]=5", Radix::Auto),
            Err(AdminError::InvalidQuery("limit inválido"))
        );
    }

    #[test]
    fn logs_limit_follows_the_firestore_sdk() {
        assert_eq!(firestore_limit(50.0), Some(50));
        assert_eq!(firestore_limit(7.0), Some(7));
        assert_eq!(firestore_limit(-5.0), None);
        assert_eq!(firestore_limit(f64::INFINITY), None);
        assert_eq!(firestore_limit(2_147_483_647.0), Some(2_147_483_647));
        assert_eq!(firestore_limit(3_000_000_000.0), None, "| 0 negativo");
        assert_eq!(firestore_limit(4_294_967_301.0), Some(5), "| 0 = 5");
        assert_eq!(firestore_limit(4_294_967_296.0), None, "| 0 = 0");
    }

    #[test]
    fn slice_limit_reproduces_negative_slices() {
        let s = |l: f64| SliceLimit::new(l).apply(vec![1, 2, 3]);
        assert_eq!(s(100.0), vec![1, 2, 3]);
        assert_eq!(SliceLimit::new(2.0).repo, 2);
        assert_eq!(s(-1.0), vec![1, 2]);
        assert_eq!(s(-3.0), Vec::<i32>::new());
        assert_eq!(s(-50.0), Vec::<i32>::new());
        assert_eq!(s(f64::NEG_INFINITY), Vec::<i32>::new());
        assert_eq!(SliceLimit::new(f64::INFINITY).repo, u32::MAX);
        assert_eq!(SliceLimit::new(1e300).repo, u32::MAX);
    }

    #[test]
    fn history_limit_follows_get_match_history() {
        assert_eq!(history_safe_limit(20.0), 20);
        assert_eq!(history_safe_limit(7.0), 7);
        assert_eq!(history_safe_limit(-5.0), 1);
        assert_eq!(history_safe_limit(500.0), 50);
        assert_eq!(history_safe_limit(1e21), 1);
        assert_eq!(history_safe_limit(7.5e22), 7);
        assert_eq!(history_safe_limit(-1e21), 1);
        assert_eq!(history_safe_limit(f64::INFINITY), 20);
        assert_eq!(history_safe_limit(f64::NEG_INFINITY), 20);
    }

    #[test]
    fn status_maps_empty_to_none() {
        assert_eq!(status_param(""), Ok(None));
        assert_eq!(status_param("status="), Ok(None));
        assert_eq!(status_param("status=ATENDIDO"), Ok(Some("ATENDIDO".into())));
        assert_eq!(
            status_param("status=a&status=b"),
            Err(AdminError::InvalidQuery("status inválido"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cache_entry_expires_after_ttl_and_is_bounded() {
        let c = LogsCache::default();
        let t0 = Instant::now();
        c.insert("global:50".into(), t0, Arc::new(json!([1])));
        assert!(c
            .get("global:50", t0 + Duration::from_millis(29_999))
            .is_some());
        assert!(c.get("global:50", t0 + BOT_LOGS_TTL).is_none());
        for i in 0..LOGS_CACHE_MAX_KEYS {
            c.insert(format!("k{i}"), t0, Arc::new(json!([])));
        }
        assert!(c.entries.lock().unwrap().len() <= LOGS_CACHE_MAX_KEYS);
    }
}
