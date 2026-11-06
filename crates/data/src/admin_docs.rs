//! Lecturas de documentos completos del panel (Tarea 8): recuentos de `GET /api/admin/stats`,
//! la copia de usuarios de `GET /api/admin/users/backup` y las tres listas de
//! `GET /api/admin/yape/transactions`.
//!
//! Legacy:
//!
//! * `/stats` (`routes/api.js` ~336): `faceit_users` y `faceit_active_matches` con `.count()`
//!   (con servidor, `where('servidor' | 'guild_id', '==', g)`); si falla cualquiera de los dos
//!   recuentos, `get()` de las dos colecciones enteras y recuento en memoria con **otro** filtro:
//!   `(servidor || mixton_guild_id) === g` y `guild_id === g`. Si eso también falla, la ruta da
//!   500. Ver [`AdminDocsRepo::totals`].
//! * `/users/backup` (~670): `faceit_users` entera; con servidor, solo los documentos con
//!   `(guild_id || servidor || mixton_guild_id || null) === g`; cada uno sale como
//!   `{ discordId: data.discordId || doc.id, ...data }`. Un error de lectura es el 500 de la ruta.
//! * `getYapeTransactions` / `getYapeOperations` / `getYapeAuditLogs` (`database.js` ~3307-3362):
//!   colección entera, `{ id: doc.id, ...doc.data() }`, orden descendente en memoria por una
//!   clave distinta cada una y `slice(0, limit)`. El legacy traga el error y devuelve `[]`; aquí
//!   se devuelve el `Err` y la ruta aplica esa política.
//!
//! Los documentos se leen con sus tipos de Firestore ([`crate::node_docs`]), así que un
//! `Timestamp` sale como `{ _seconds, _nanoseconds }` y un texto con fecha sigue siendo texto.

use crate::error::DataError;
use crate::firestore::FirestoreClient;
use crate::model::iso_millis;
use crate::node_docs::NodeDocSource;
use async_trait::async_trait;
use serde_json::{Map, Value};

const USERS: &str = "faceit_users";
const ACTIVE_MATCHES: &str = "faceit_active_matches";
const MATCHES_HISTORY: &str = "faceit_matches_history";
const MATCH_STATS: &str = "faceit_match_stats";
const YAPE_TRANSACTIONS: &str = "yape_transacciones";
const YAPE_OPERATIONS: &str = "yape_operations";
const YAPE_AUDIT_LOGS: &str = "yape_audit_logs";
const CONFIG: &str = "faceit_config";

/// `totalUsers` y `totalMatches` de `/stats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsTotals {
    pub users: u64,
    pub matches: u64,
}

/// Lecturas de documentos completos del panel.
///
/// **Errores:** se devuelven tal cual; la política del legacy (500 en `/stats` y
/// `/users/backup`, `[]` en las de Yape) es de la ruta.
#[async_trait]
pub trait AdminDocsRepo: Send + Sync {
    /// Recuentos de `/stats` (recuento nativo y, si falla, el respaldo en memoria del legacy).
    async fn totals(&self, guild: Option<&str>) -> Result<StatsTotals, DataError>;
    /// Usuarios de `/users/backup`, ya filtrados y con `discordId`.
    async fn users_backup(&self, guild: Option<&str>) -> Result<Vec<Value>, DataError>;
    /// `getYapeTransactions(limit)`.
    async fn yape_transactions(&self, limit: usize) -> Result<Vec<Value>, DataError>;
    /// `getYapeOperations(limit)`.
    async fn yape_operations(&self, limit: usize) -> Result<Vec<Value>, DataError>;
    /// `getYapeAuditLogs(limit)`.
    async fn yape_audit_logs(&self, limit: usize) -> Result<Vec<Value>, DataError>;
    /// Partidas activas (`faceit_active_matches`), filtradas opcionalmente por guild_id.
    async fn active_matches(&self, guild: Option<&str>) -> Result<Vec<Value>, DataError>;
    /// Partida activa por id (`faceit_active_matches/{id}`).
    async fn active_match(&self, id: &str) -> Result<Option<Value>, DataError>;
    /// `getMatchHistoryById`: `faceit_matches_history/{id}` tal cual, `None` si no existe.
    async fn match_history_by_id(&self, id: &str) -> Result<Option<Value>, DataError>;
    /// `getMatchStats`: `faceit_match_stats/{id}` tal cual, `None` si no existe.
    async fn match_stats(&self, id: &str) -> Result<Option<Value>, DataError>;
    /// Estado del lobby mixton (`faceit_config/mixton_lobby` o `faceit_config/mixton_lobby_{guild}`).
    async fn mixton_lobby_state(&self, guild: Option<&str>) -> Result<Option<Value>, DataError>;
}

/// Repo sobre Firestore (o sobre cualquier [`NodeDocSource`]), sin caché (las cachés de
/// `/stats` y de Yape son de la ruta).
pub struct FirestoreAdminDocsRepo<S = FirestoreClient> {
    source: S,
}

impl<S: NodeDocSource> FirestoreAdminDocsRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

#[async_trait]
impl<S: NodeDocSource> AdminDocsRepo for FirestoreAdminDocsRepo<S> {
    async fn totals(&self, guild: Option<&str>) -> Result<StatsTotals, DataError> {
        let counted = futures::try_join!(
            self.source.count(USERS, guild.map(|g| ("servidor", g))),
            self.source
                .count(ACTIVE_MATCHES, guild.map(|g| ("guild_id", g))),
        );
        if let Ok((users, matches)) = counted {
            return Ok(StatsTotals { users, matches });
        }
        // `catch {}` del legacy: las dos colecciones enteras.
        let (users, matches) = futures::try_join!(
            self.source.list_all(USERS),
            self.source.list_all(ACTIVE_MATCHES),
        )?;
        Ok(match guild {
            None => StatsTotals {
                users: users.len() as u64,
                matches: matches.len() as u64,
            },
            Some(g) => StatsTotals {
                // `(data.servidor || data.mixton_guild_id) === targetGuildId`
                users: users
                    .iter()
                    .filter(|(_, d)| is_str(js_or(d, &["servidor", "mixton_guild_id"]), g))
                    .count() as u64,
                // `doc.data().guild_id === targetGuildId`
                matches: matches
                    .iter()
                    .filter(|(_, d)| is_str(d.get("guild_id"), g))
                    .count() as u64,
            },
        })
    }

    async fn users_backup(&self, guild: Option<&str>) -> Result<Vec<Value>, DataError> {
        let docs = self.source.list_all(USERS).await?;
        Ok(docs
            .into_iter()
            .filter(|(_, d)| match guild {
                // `(guild_id || servidor || mixton_guild_id || null) !== targetGuildId` -> fuera
                Some(g) => is_str(js_or(d, &["guild_id", "servidor", "mixton_guild_id"]), g),
                None => true,
            })
            .map(|(id, data)| {
                // `{ discordId: data.discordId || doc.id, ...data }`: si el documento trae
                // `discordId` (aunque sea falsy) gana el suyo.
                let mut out = Map::new();
                out.insert("discordId".into(), Value::String(id));
                out.extend(data);
                Value::Object(out)
            })
            .collect())
    }

    async fn yape_transactions(&self, limit: usize) -> Result<Vec<Value>, DataError> {
        // `a.detectedAt ? new Date(a.detectedAt).getTime() : (a.createdAt || 0)`
        self.sorted(YAPE_TRANSACTIONS, limit, |d| match d.get("detectedAt") {
            Some(at) if js_truthy(at) => js_date_time(at),
            _ => match d.get("createdAt") {
                Some(c) if js_truthy(c) => js_to_number(c),
                _ => 0.0,
            },
        })
        .await
    }

    async fn yape_operations(&self, limit: usize) -> Result<Vec<Value>, DataError> {
        // `a.createdAt ? (a.createdAt._seconds ? a.createdAt._seconds * 1000
        //                 : new Date(a.createdAt).getTime()) : 0`
        self.sorted(YAPE_OPERATIONS, limit, |d| match d.get("createdAt") {
            Some(c) if js_truthy(c) => match c.get("_seconds") {
                Some(s) if js_truthy(s) => js_to_number(s) * 1000.0,
                _ => js_date_time(c),
            },
            _ => 0.0,
        })
        .await
    }

    async fn yape_audit_logs(&self, limit: usize) -> Result<Vec<Value>, DataError> {
        // `a.timestamp || 0` (los registros guardan `createdAt`, no `timestamp`: en la práctica
        // todas las claves son 0 y queda el orden de Firestore).
        self.sorted(YAPE_AUDIT_LOGS, limit, |d| match d.get("timestamp") {
            Some(t) if js_truthy(t) => js_to_number(t),
            _ => 0.0,
        })
        .await
    }

    async fn active_matches(&self, guild: Option<&str>) -> Result<Vec<Value>, DataError> {
        let docs = self.source.list_all(ACTIVE_MATCHES).await?;
        Ok(docs
            .into_iter()
            .filter(|(_, d)| match guild {
                Some(g) => is_str(d.get("guild_id"), g),
                None => true,
            })
            .map(|(id, mut data)| {
                if !data.contains_key("id") {
                    data.insert("id".to_string(), Value::String(id));
                }
                Value::Object(data)
            })
            .collect())
    }

    async fn active_match(&self, id: &str) -> Result<Option<Value>, DataError> {
        let doc = self.source.get(ACTIVE_MATCHES, id).await?;
        Ok(doc.map(|mut data| {
            if !data.contains_key("id") {
                data.insert("id".to_string(), Value::String(id.to_string()));
            }
            Value::Object(data)
        }))
    }

    async fn match_history_by_id(&self, id: &str) -> Result<Option<Value>, DataError> {
        Ok(self
            .source
            .get(MATCHES_HISTORY, id)
            .await?
            .map(Value::Object))
    }

    async fn match_stats(&self, id: &str) -> Result<Option<Value>, DataError> {
        Ok(self.source.get(MATCH_STATS, id).await?.map(Value::Object))
    }

    async fn mixton_lobby_state(&self, guild: Option<&str>) -> Result<Option<Value>, DataError> {
        let doc_id = match guild {
            Some(g) if !g.is_empty() => format!("mixton_lobby_{g}"),
            _ => "mixton_lobby".to_string(),
        };
        let doc = self.source.get(CONFIG, &doc_id).await?;
        Ok(doc.map(Value::Object))
    }
}

impl<S: NodeDocSource> FirestoreAdminDocsRepo<S> {
    /// `{ id: doc.id, ...doc.data() }` de toda la colección, `sort((a, b) => key(b) - key(a))`
    /// y `slice(0, limit)`.
    async fn sorted(
        &self,
        collection: &str,
        limit: usize,
        key: impl Fn(&Map<String, Value>) -> f64,
    ) -> Result<Vec<Value>, DataError> {
        let docs = self.source.list_all(collection).await?;
        let mut keyed: Vec<(f64, Value)> = docs
            .into_iter()
            .map(|(id, data)| {
                let k = key(&data);
                let mut out = Map::new();
                out.insert("id".into(), Value::String(id));
                out.extend(data);
                (k, Value::Object(out))
            })
            .collect();
        sort_desc_like_js(&mut keyed);
        keyed.truncate(limit);
        Ok(keyed.into_iter().map(|(_, v)| v).collect())
    }
}

/// `arr.sort((a, b) => kb - ka)`: orden estable descendente. Con claves numéricas (no `NaN`)
/// coincide con V8 (TimSort, estable). Con un `NaN` el comparador de JS es incoherente y el
/// resultado depende del algoritmo de V8; aquí un `NaN` cuenta como igual a todo.
fn sort_desc_like_js<T>(items: &mut [(f64, T)]) {
    items.sort_by(|(ka, _), (kb, _)| kb.partial_cmp(ka).unwrap_or(std::cmp::Ordering::Equal));
}

// --- Semántica de JS sobre valores leídos de Firestore ---------------------------------------

/// Veracidad de JS (`if (x)`).
pub(crate) fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `d[a] || d[b] || ...`: el primer valor verdadero o, si no hay, el último (que puede faltar).
fn js_or<'a>(d: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    let mut last = None;
    for key in keys {
        last = d.get(*key);
        if last.is_some_and(js_truthy) {
            return last;
        }
    }
    last
}

/// `x === 'texto'`.
fn is_str(v: Option<&Value>, s: &str) -> bool {
    v.and_then(Value::as_str) == Some(s)
}

/// `{ _seconds, _nanoseconds }` de un `Timestamp` (no se distingue de un mapa con esas dos
/// claves numéricas, que en Firestore no aparece).
fn as_timestamp(v: &Value) -> Option<(i64, i64)> {
    let o = v.as_object()?;
    if o.len() != 2 {
        return None;
    }
    Some((
        o.get("_seconds")?.as_i64()?,
        o.get("_nanoseconds")?.as_i64()?,
    ))
}

/// `Number(x)` de JS. Un `Timestamp` pasa por su `valueOf()` (`segundos - MIN_SECONDS` con 12
/// cifras, un punto y los nanos con 9); un mapa es `NaN`.
pub(crate) fn js_to_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => js_string_to_number(s),
        Value::Array(a) => match a.as_slice() {
            [] => 0.0,
            [one @ (Value::String(_) | Value::Number(_))] => js_to_number(one),
            _ => f64::NAN,
        },
        Value::Object(_) => match as_timestamp(v) {
            Some((s, n)) => format!("{:012}.{:09}", s + 62_135_596_800, n)
                .parse()
                .unwrap_or(f64::NAN),
            None => f64::NAN,
        },
    }
}

/// `Number('texto')`: espacios fuera, `''` es 0, decimal, `0x`/`0o`/`0b` y `Infinity`.
fn js_string_to_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    let radix = |p: &str, r: u32| u64::from_str_radix(p, r).map_or(f64::NAN, |n| n as f64);
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return radix(h, 16);
    }
    if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return radix(o, 8);
    }
    if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return radix(b, 2);
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // Rust acepta "inf", "nan", "infinity": JS no.
    if t.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        t.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// `new Date(x).getTime()`. Texto: RFC 3339 / ISO con zona o `YYYY-MM-DD` (UTC); cualquier otro
/// formato que `Date.parse` de V8 aceptaría da `NaN` aquí. Un número pasa por `TimeClip`. Un
/// objeto (también un `Timestamp`, cuyo `valueOf()` no es una fecha) es `NaN`.
pub(crate) fn js_date_time(v: &Value) -> f64 {
    let clip = |f: f64| {
        if f.is_finite() && f.abs() <= 8.64e15 {
            f.trunc() + 0.0
        } else {
            f64::NAN
        }
    };
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => clip(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => date_parse(s),
        Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

fn date_parse(s: &str) -> f64 {
    if let Some(ms) = iso_millis(s) {
        return ms as f64;
    }
    match chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        Ok(d) if s.len() == 10 => d
            .and_hms_opt(0, 0, 0)
            .map_or(f64::NAN, |t| t.and_utc().timestamp_millis() as f64),
        _ => f64::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_docs::InMemoryNodeDocs;
    use serde_json::json;
    use std::sync::Arc;

    fn ids(v: &[Value], key: &str) -> Vec<String> {
        v.iter()
            .map(|e| e[key].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn js_coercions_match_node() {
        // Valores comprobados con node -e (ver informe de la Tarea 8).
        let ts = json!({"_seconds": 1_793_613_600_i64, "_nanoseconds": 5_000});
        assert_eq!(js_to_number(&ts), 63_929_210_400.000_01);
        assert!(js_date_time(&ts).is_nan());
        assert_eq!(js_to_number(&json!(" 12 ")), 12.0);
        assert_eq!(js_to_number(&json!("0x10")), 16.0);
        assert_eq!(js_to_number(&json!("")), 0.0);
        assert_eq!(js_to_number(&json!("1e3")), 1000.0);
        assert!(js_to_number(&json!("inf")).is_nan());
        assert_eq!(js_to_number(&json!([])), 0.0);
        assert_eq!(js_to_number(&json!([5])), 5.0);
        assert!(js_to_number(&json!({})).is_nan());
        assert_eq!(js_date_time(&json!("2026-11-02")), 1_793_577_600_000.0);
        assert_eq!(
            js_date_time(&json!("2026-11-02T10:00:00.005Z")),
            1_793_613_600_005.0
        );
        assert!(js_date_time(&json!(" 2026-11-02T10:00:00Z")).is_nan());
        assert_eq!(
            js_date_time(&json!(1_700_000_000_000.7_f64)),
            1_700_000_000_000.0
        );
        assert!(js_date_time(&json!(9e15)).is_nan());
        for falsy in [json!(null), json!(false), json!(0), json!(""), json!(-0.0)] {
            assert!(!js_truthy(&falsy), "{falsy}");
        }
        for truthy in [json!("0"), json!([]), json!({}), json!(-1)] {
            assert!(js_truthy(&truthy), "{truthy}");
        }
    }

    fn repo(s: &Arc<InMemoryNodeDocs>) -> FirestoreAdminDocsRepo<Arc<InMemoryNodeDocs>> {
        FirestoreAdminDocsRepo::new(s.clone())
    }

    #[tokio::test]
    async fn totals_count_natively_and_fall_back_with_the_legacy_filter() {
        let s = Arc::new(InMemoryNodeDocs::new());
        s.put(USERS, "u1", json!({"servidor": "g1"}));
        s.put(USERS, "u2", json!({"servidor": "g2"}));
        // Solo cuenta en el respaldo (`servidor || mixton_guild_id`).
        s.put(
            USERS,
            "u3",
            json!({"servidor": "", "mixton_guild_id": "g1"}),
        );
        s.put(ACTIVE_MATCHES, "m1", json!({"guild_id": "g1"}));
        s.put(ACTIVE_MATCHES, "m2", json!({"guild_id": "g2"}));
        let r = repo(&s);
        assert_eq!(
            r.totals(None).await,
            Ok(StatsTotals {
                users: 3,
                matches: 2
            })
        );
        assert_eq!(
            r.totals(Some("g1")).await,
            Ok(StatsTotals {
                users: 1,
                matches: 1
            })
        );
        assert_eq!(s.lists_of(USERS), 0, "sin respaldo");

        s.fail_counts(Some(DataError::Firestore("count".into())));
        assert_eq!(
            r.totals(Some("g1")).await,
            Ok(StatsTotals {
                users: 2,
                matches: 1
            })
        );
        assert_eq!(
            r.totals(None).await,
            Ok(StatsTotals {
                users: 3,
                matches: 2
            })
        );
        s.fail_lists(Some(DataError::QuotaExceeded));
        assert_eq!(r.totals(None).await, Err(DataError::QuotaExceeded));
    }

    #[tokio::test]
    async fn users_backup_filters_by_guild_chain_and_adds_discord_id() {
        let s = Arc::new(InMemoryNodeDocs::new());
        s.put(
            USERS,
            "g1_1",
            json!({"guild_id": "g1", "linkedAt": {"_seconds": 5, "_nanoseconds": 0}}),
        );
        s.put(USERS, "2", json!({"servidor": "g1", "discordId": "2"}));
        s.put(
            USERS,
            "3",
            json!({"guild_id": "", "servidor": null, "mixton_guild_id": "g1"}),
        );
        s.put(USERS, "4", json!({"guild_id": "g2", "servidor": "g1"}));
        s.put(USERS, "5", json!({"discordId": ""}));
        let r = repo(&s);
        let all = r.users_backup(None).await.unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(
            all[0],
            json!({"discordId": "2", "servidor": "g1"}),
            "orden por id de documento"
        );
        assert_eq!(
            all[3],
            json!({"discordId": ""}),
            "el falsy del documento gana"
        );
        let g1 = r.users_backup(Some("g1")).await.unwrap();
        assert_eq!(ids(&g1, "discordId"), ["2", "3", "g1_1"]);
        assert_eq!(
            g1[2],
            json!({"discordId": "g1_1", "guild_id": "g1",
                "linkedAt": {"_seconds": 5, "_nanoseconds": 0}})
        );
        s.fail_lists(Some(DataError::QuotaExceeded));
        assert_eq!(r.users_backup(None).await, Err(DataError::QuotaExceeded));
    }

    #[tokio::test]
    async fn yape_lists_sort_like_database_js_and_slice() {
        let s = Arc::new(InMemoryNodeDocs::new());
        // transacciones: detectedAt (fecha) o createdAt (número)
        s.put(
            YAPE_TRANSACTIONS,
            "t1",
            json!({"detectedAt": "2026-11-02T10:00:00Z"}),
        );
        s.put(
            YAPE_TRANSACTIONS,
            "t2",
            json!({"createdAt": 1_793_613_601_000_i64}),
        );
        s.put(
            YAPE_TRANSACTIONS,
            "t3",
            json!({"detectedAt": "2026-11-02T10:00:02Z", "id": "propio"}),
        );
        s.put(YAPE_TRANSACTIONS, "t4", json!({}));
        // operaciones: createdAt Timestamp (_seconds) o texto
        s.put(
            YAPE_OPERATIONS,
            "o1",
            json!({"createdAt": {"_seconds": 10, "_nanoseconds": 0}}),
        );
        s.put(
            YAPE_OPERATIONS,
            "o2",
            json!({"createdAt": "1970-01-01T00:00:20Z"}),
        );
        s.put(YAPE_OPERATIONS, "o3", json!({"status": "pending"}));
        // auditoría: sin `timestamp` -> todas 0 -> orden de Firestore
        for id in ["a3", "a1", "a2"] {
            s.put(
                YAPE_AUDIT_LOGS,
                id,
                json!({"createdAt": {"_seconds": 99, "_nanoseconds": 0}}),
            );
        }
        s.put(YAPE_AUDIT_LOGS, "a0", json!({"timestamp": 5}));
        let r = repo(&s);
        let t = r.yape_transactions(50).await.unwrap();
        assert_eq!(ids(&t, "id"), ["propio", "t2", "t1", "t4"]);
        assert_eq!(
            t[0]["detectedAt"],
            json!("2026-11-02T10:00:02Z"),
            "texto, no Timestamp"
        );
        assert_eq!(
            ids(&r.yape_transactions(2).await.unwrap(), "id"),
            ["propio", "t2"]
        );
        let o = r.yape_operations(50).await.unwrap();
        assert_eq!(ids(&o, "id"), ["o2", "o1", "o3"]);
        assert_eq!(
            o[1]["createdAt"],
            json!({"_seconds": 10, "_nanoseconds": 0})
        );
        let a = r.yape_audit_logs(50).await.unwrap();
        assert_eq!(ids(&a, "id"), ["a0", "a1", "a2", "a3"]);
        s.fail_lists(Some(DataError::QuotaExceeded));
        assert_eq!(r.yape_audit_logs(50).await, Err(DataError::QuotaExceeded));
    }

    #[tokio::test]
    async fn active_matches_and_lobby_state_read_from_source() {
        let s = Arc::new(InMemoryNodeDocs::new());
        s.put(ACTIVE_MATCHES, "m1", json!({"guild_id": "g1", "demo_url": "url1"}));
        s.put(ACTIVE_MATCHES, "m2", json!({"guild_id": "g2"}));
        s.put(CONFIG, "mixton_lobby", json!({"mapCatalog": [{"id": "de_mirage"}]}));
        s.put(CONFIG, "mixton_lobby_g1", json!({"mapCatalog": [{"id": "de_nuke"}]}));

        let r = repo(&s);
        let all = r.active_matches(None).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(ids(&all, "id"), ["m1", "m2"]);

        let g1 = r.active_matches(Some("g1")).await.unwrap();
        assert_eq!(g1.len(), 1);
        assert_eq!(g1[0]["id"], "m1");

        let m1 = r.active_match("m1").await.unwrap().expect("match m1");
        assert_eq!(m1["id"], "m1");
        assert_eq!(m1["demo_url"], "url1");

        assert!(r.active_match("m999").await.unwrap().is_none());

        let lob_global = r.mixton_lobby_state(None).await.unwrap().expect("global lobby");
        assert_eq!(lob_global["mapCatalog"][0]["id"], "de_mirage");

        let lob_g1 = r.mixton_lobby_state(Some("g1")).await.unwrap().expect("g1 lobby");
        assert_eq!(lob_g1["mapCatalog"][0]["id"], "de_nuke");
    }
}
