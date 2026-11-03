//! Lecturas de administración derivadas (Tarea 8):
//!
//! | ruta | legacy (`routes/api.js`) | respuesta |
//! |---|---|---|
//! | `GET /api/admin/stats` | ~336 | `{ totalUsers, totalMatches, levelDistribution, features }` (caché 60 s) |
//! | `GET /api/admin/users/backup` | ~670 | `[ { discordId, ...usuario } ]` |
//! | `GET /api/admin/yape/transactions` | ~1989 | `{ transactions, operations, auditLogs }` (caché 30 s, una lectura a la vez) |
//!
//! Siguen en el proxy (dependen de la API de Faceit o del estado en memoria de Node):
//! `GET /stats/:nickname` (`fetchFaceitPlayer`, `fetchFaceitStats`, `fetchPlayerHistory`),
//! `GET /matches/:id/demo` (`getActiveMatch` lee la caché en memoria de partidas activas y el
//! resto de ids van a `open.faceit.com`) y `GET /settings` (sus valores por defecto salen de
//! `process.env`, que `POST /settings` reescribe en caliente dentro del proceso Node).
//!
//! # Errores
//!
//! `/stats` y `/users/backup` leen Firestore directamente (`db._firestore`): un error llega al
//! `catch` de la ruta y es su 500 (`Error al obtener estadísticas`, `Error al generar backup`).
//! Las tres lecturas de Yape (`database.js`) tragan el error y devuelven `[]`, y ese resultado
//! se cachea. El texto del error solo va al log.
//!
//! # Cachés y escrituras de Node
//!
//! `statsCache` (60 s por servidor) y la caché de Yape (30 s) del legacy se vacían cuando Node
//! escribe desde el panel (`invalidateAdminUsersCache` en `POST /users/:id/unlink` y
//! `POST /users/verify`; `invalidateYapeCache` en `POST /yape/manual-approve`). Esas escrituras
//! siguen en Node pero pasan por el proxy de Rust: [`invalidate_after_legacy_writes`] vacía las
//! cachés de Rust al recibir la respuesta de Node.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    extract::{FromRef, RawQuery, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::Response,
    routing::get,
    Router,
};
use data::GlobalSettingsRepo;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use super::route_cache::RouteCache;
use super::{resolve_target_guild_id, AdminContext, AdminError, AdminRepos};
use crate::routes::json_response;
use crate::session::SessionState;

/// `STATS_CACHE_TTL` (`routes/api.js:244`).
pub const STATS_CACHE_TTL: Duration = Duration::from_secs(60);
/// `YAPE_CACHE_TTL` (`routes/api.js:303`).
pub const YAPE_CACHE_TTL: Duration = Duration::from_secs(30);
/// `db.getYape*(50)` (`routes/api.js:2003-2005`).
const YAPE_LIMIT: usize = 50;

const MSG_STATS: &str = "Error al obtener estadísticas";
const MSG_BACKUP: &str = "Error al generar backup";
const MSG_YAPE: &str = "Error al obtener transacciones Yape";

/// Claves de `features` en `/stats`, en el orden del legacy.
const FEATURES: [&str; 6] = [
    "autoWelcomeDm",
    "autoRoleSync",
    "autoVoiceChannels",
    "autoMovePlayers",
    "matchAnnouncements",
    "liveMatchPanel",
];

/// Cachés de estas rutas, compartidas con [`invalidate_after_legacy_writes`].
#[derive(Clone)]
pub struct AdminCaches {
    stats: Arc<RouteCache>,
    yape: Arc<YapeCache>,
}

impl Default for AdminCaches {
    fn default() -> Self {
        Self {
            stats: Arc::new(RouteCache::new(STATS_CACHE_TTL)),
            yape: Arc::default(),
        }
    }
}

/// Estado de estas rutas.
#[derive(Clone)]
struct DerivedState {
    session: SessionState,
    repos: AdminRepos,
}

impl FromRef<DerivedState> for SessionState {
    fn from_ref(s: &DerivedState) -> SessionState {
        s.session.clone()
    }
}

/// Las tres rutas, con su estado.
pub(crate) fn router(session: SessionState, repos: AdminRepos) -> Router {
    Router::new()
        .route("/api/admin/stats", get(stats))
        .route("/api/admin/users/backup", get(users_backup))
        .route("/api/admin/yape/transactions", get(yape_transactions))
        .with_state(DerivedState { session, repos })
}

// ------------------------------------------------------------------ handlers

/// `GET /stats`: servidor objetivo, caché de 60 s por `guild || 'global'`, `getSettings()` y
/// los recuentos.
async fn stats(
    State(st): State<DerivedState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let key = target.as_deref().unwrap_or("global").to_owned();
    let now = Instant::now();
    let cache = &st.repos.caches.stats;
    if let Some(cached) = cache.get(&key, now) {
        return Ok(json_response(StatusCode::OK, &cached));
    }
    let features = features(st.repos.global_settings.as_ref()).await;
    let totals = match st.repos.docs.totals(target.as_deref()).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Error en /stats: {e}");
            return Ok(error_500(MSG_STATS));
        }
    };
    let level_distribution: Map<String, Value> =
        (1..=10).map(|i| (i.to_string(), json!(0))).collect();
    let body = Arc::new(json!({
        "totalUsers": totals.users,
        "totalMatches": totals.matches,
        "levelDistribution": level_distribution,
        "features": features,
    }));
    cache.insert(key, now, body.clone());
    Ok(json_response(StatusCode::OK, &body))
}

/// `features` de `/stats`: `settings.<clave>` de `getSettings()`; una clave ausente no sale
/// (`undefined` en `JSON.stringify`).
async fn features(settings: &dyn GlobalSettingsRepo) -> Value {
    let settings = match serde_json::to_value(settings.get().await) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    Value::Object(
        FEATURES
            .iter()
            .filter_map(|k| settings.get(*k).map(|v| ((*k).to_owned(), v.clone())))
            .collect(),
    )
}

/// `GET /users/backup`: servidor objetivo y todos sus usuarios (todos sin servidor).
async fn users_backup(
    State(st): State<DerivedState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    Ok(match st.repos.docs.users_backup(target.as_deref()).await {
        Ok(users) => json_response(StatusCode::OK, &Value::Array(users)),
        Err(e) => {
            tracing::error!("Error al generar backup de usuarios: {e}");
            error_500(MSG_BACKUP)
        }
    })
}

/// `GET /yape/transactions`: la caché de 30 s o la lectura en curso, o una nueva.
async fn yape_transactions(State(st): State<DerivedState>, _ctx: AdminContext) -> Response {
    match st.repos.caches.yape.get_or_fetch(&st.repos).await {
        Ok(data) => json_response(StatusCode::OK, &data),
        Err(()) => {
            tracing::error!("Error en GET /yape/transactions: la lectura no terminó");
            error_500(MSG_YAPE)
        }
    }
}

fn error_500(msg: &'static str) -> Response {
    json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({ "error": msg }))
}

// -------------------------------------------------------------- caché Yape

type YapeFetch = Shared<BoxFuture<'static, Result<Arc<Value>, ()>>>;

/// `yapeTransactionsCache` + `yapeTransactionsFetchPromise` del legacy.
#[derive(Default)]
pub(crate) struct YapeCache {
    state: Mutex<YapeState>,
}

#[derive(Default)]
struct YapeState {
    /// Datos y el instante en que **terminó** su lectura (`Date.now()` tras el `Promise.all`).
    data: Option<(Instant, Arc<Value>)>,
    /// Lectura en curso, compartida por todas las peticiones que llegan mientras dura.
    inflight: Option<YapeFetch>,
}

impl YapeCache {
    /// Como el handler del legacy: datos con menos de 30 s, o esperar la lectura en curso, o
    /// lanzar una. La lectura corre en su propia tarea (como la promesa de Node, sigue aunque
    /// el cliente se vaya) y al terminar guarda el resultado y suelta `inflight` (`finally`).
    async fn get_or_fetch(self: &Arc<Self>, repos: &AdminRepos) -> Result<Arc<Value>, ()> {
        let fetch = {
            let mut st = self.state.lock().expect("mutex envenenado");
            if let Some((at, data)) = &st.data {
                if Instant::now().saturating_duration_since(*at) < YAPE_CACHE_TTL {
                    return Ok(data.clone());
                }
            }
            match &st.inflight {
                Some(f) => f.clone(),
                None => {
                    let cache = self.clone();
                    let repos = repos.clone();
                    let task = tokio::spawn(async move {
                        let data = Arc::new(read_yape(&repos).await);
                        let mut st = cache.state.lock().expect("mutex envenenado");
                        st.data = Some((Instant::now(), data.clone()));
                        st.inflight = None;
                        data
                    });
                    let f: YapeFetch = async move { task.await.map_err(|_| ()) }.boxed().shared();
                    st.inflight = Some(f.clone());
                    f
                }
            }
        };
        fetch.await
    }

    /// `invalidateYapeCache()`: sin datos ni lectura en curso (la que estuviera en marcha
    /// termina y, como en el legacy, deja su resultado).
    fn invalidate(&self) {
        let mut st = self.state.lock().expect("mutex envenenado");
        st.data = None;
        st.inflight = None;
    }
}

/// `Promise.all([getYapeTransactions(50), getYapeOperations(50), getYapeAuditLogs(50)])`; cada
/// una traga su error y da `[]`.
async fn read_yape(repos: &AdminRepos) -> Value {
    let docs = &repos.docs;
    let (transactions, operations, audit_logs) = tokio::join!(
        docs.yape_transactions(YAPE_LIMIT),
        docs.yape_operations(YAPE_LIMIT),
        docs.yape_audit_logs(YAPE_LIMIT),
    );
    let or_empty = |r: Result<Vec<Value>, data::DataError>, what: &str| match r {
        Ok(list) => Value::Array(list),
        Err(e) => {
            tracing::error!("Error al listar {what}: {e}");
            Value::Array(Vec::new())
        }
    };
    json!({
        "transactions": or_empty(transactions, "transacciones Yape"),
        "operations": or_empty(operations, "operaciones Yape"),
        "auditLogs": or_empty(audit_logs, "logs de auditoría Yape"),
    })
}

// ------------------------------------------------- escrituras de Node (proxy)

/// Caché que vacía una escritura del panel atendida por Node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyWrite {
    /// `invalidateAdminUsersCache()`: `statsCache.clear()` (y cachés de `/users`, que siguen
    /// en Node).
    Users,
    /// `invalidateYapeCache()`.
    Yape,
}

/// Escritura del legacy que invalida una caché de estas rutas, según la ruta de Express
/// (sin distinguir mayúsculas y con una `/` final opcional, como el router por defecto).
pub(crate) fn legacy_write(method: &Method, path: &str) -> Option<LegacyWrite> {
    if method != Method::POST {
        return None;
    }
    let path = path.strip_suffix('/').unwrap_or(path).to_ascii_lowercase();
    // El montaje `app.use('/api/admin', router)` deja `/resto`, y su `trim_prefix`
    // admite una `/` extra (`/api/admin//users/verify` llega al handler).
    let rest = path.strip_prefix("/api/admin/")?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let segments: Vec<&str> = rest.split('/').collect();
    match segments.as_slice() {
        ["users", id, "unlink"] if !id.is_empty() => Some(LegacyWrite::Users),
        ["users", "verify"] => Some(LegacyWrite::Users),
        ["yape", "manual-approve"] => Some(LegacyWrite::Yape),
        _ => None,
    }
}

/// Capa sobre toda la aplicación: tras la respuesta (de Node, vía proxy) a una escritura de
/// [`legacy_write`], vacía la caché correspondiente. Se vacía con cualquier estado de
/// respuesta: el legacy invalida a mitad del handler y vaciar de más solo cuesta una lectura.
pub async fn invalidate_after_legacy_writes(
    State(caches): State<AdminCaches>,
    req: Request,
    next: Next,
) -> Response {
    let write = legacy_write(req.method(), req.uri().path());
    let res = next.run(req).await;
    match write {
        Some(LegacyWrite::Users) => caches.stats.clear(),
        Some(LegacyWrite::Yape) => caches.yape.invalidate(),
        None => {}
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_writes_match_like_express() {
        let post = Method::POST;
        for (path, want) in [
            ("/api/admin/users/123/unlink", Some(LegacyWrite::Users)),
            ("/api/admin/users/123/unlink/", Some(LegacyWrite::Users)),
            ("/API/Admin/Users/abc/UNLINK", Some(LegacyWrite::Users)),
            ("/api/admin/users/verify", Some(LegacyWrite::Users)),
            ("/api/admin/yape/manual-approve", Some(LegacyWrite::Yape)),
            ("/api/admin/users//unlink", None),
            ("/api/admin/users/1/2/unlink", None),
            ("/api/admin/users/backup", None),
            ("/api/admin/yape/transactions", None),
            ("/api/lobby/yape/manual-approve", None),
            // Express 4.22 (sondeado): el `trim_prefix` del montaje se traga una `/`
            // extra tras `/api/admin`; con tres, 404.
            ("/api/admin//users/verify", Some(LegacyWrite::Users)),
            ("/api/admin//users/x/unlink", Some(LegacyWrite::Users)),
            ("/api/admin//yape/manual-approve", Some(LegacyWrite::Yape)),
            ("/api/admin///users/verify", None),
            ("/API/ADMIN/USERS/VERIFY", Some(LegacyWrite::Users)),
            ("/api/admin/yape/manual-approve/", Some(LegacyWrite::Yape)),
            ("/api/admin/users/verify//", None),
            // Los literales no se decodifican; el `:id` admite `%2F`.
            ("/api/admin/%75sers/verify", None),
            ("/api/admin/users/ver%69fy", None),
            ("/api/admin/users/a%2Fb/unlink", Some(LegacyWrite::Users)),
            ("/api/admin%2Fusers/verify", None),
            ("/api/admin/./users/verify", None),
            ("//api/admin/users/verify", None),
            ("/api//admin/users/verify", None),
        ] {
            assert_eq!(legacy_write(&post, path), want, "{path}");
        }
        assert_eq!(legacy_write(&Method::GET, "/api/admin/users/verify"), None);
    }
}
