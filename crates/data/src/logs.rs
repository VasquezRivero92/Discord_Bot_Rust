//! Logs del bot (colección `faceit_bot_logs`): modelo y repositorio de solo lectura.
//!
//! Los escribe `addLogEntry` (`database.js` ~2051: `{ level, message, source, guild_id,
//! createdAt: serverTimestamp() }`, con id automático) y los lee `getRecentLogs` (~2070) para
//! `GET /api/logs` (`routes/api.js` ~1596), que responde `{ logs }`.
//!
//! Lectura del legacy: `orderBy('createdAt', 'desc')`, con `guildId` además
//! `where('guild_id', '==', guildId)`, y `limit(limit)`; cada log sale como
//! `{ id: doc.id, ...doc.data() }` (si el documento trae su propio `id`, gana el del documento).

use crate::error::DataError;
use crate::firestore::{Dir, FirestoreClient, Op, Query, Value as QValue};
use crate::model::{newest_first, node_timestamp_verbatim, present, FakeStore};
use crate::source::{from_json, DocSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const COLLECTION: &str = "faceit_bot_logs";

/// Un log tal como lo devuelve `GET /api/logs` del legacy. Todo lo que no es fecha ni id se
/// conserva tal cual en [`LogEntry::extra`] (`level`, `message`, `source`, `guild_id`, ...,
/// incluidos los `null`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// `id`: el del documento salvo que el documento traiga un campo `id` propio.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<Value>,
    /// `createdAt`: `serverTimestamp()`; se serializa como `{ _seconds, _nanoseconds }`.
    #[serde(
        rename = "createdAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<Value>,
    /// El resto de campos del documento, sin tocar.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl LogEntry {
    /// `guild_id` si es un texto.
    pub fn guild_id(&self) -> Option<&str> {
        self.extra.get("guild_id").and_then(Value::as_str)
    }
}

/// Lectura de los logs recientes.
///
/// **Errores:** se devuelven tal cual (`DataError::QuotaExceeded` incluido); el repo no los
/// traga. El legacy (`getRecentLogs`) traga *cualquier* error y devuelve `[]`, así que la ruta
/// (Tarea 7) debe responder `{ logs: [] }` ante un `Err` para reproducirlo. Con `guild` la
/// consulta necesita el índice compuesto `guild_id` + `createdAt desc`, que no consta en
/// `firestore.indexes.json` (ver "Fase 2A" en `docs/riesgos.md`): sin él llega un
/// `DataError::Firestore` (`FAILED_PRECONDITION`).
#[async_trait]
pub trait LogsRepo: Send + Sync {
    /// Los `limit` logs más recientes (por `createdAt` descendente), solo los de `guild` si se da.
    /// `limit` se pasa tal cual a Firestore (el `parseInt(...) || 50` es de la ruta).
    async fn recent(&self, limit: u32, guild: Option<&str>) -> Result<Vec<LogEntry>, DataError>;
}

/// Consulta de `getRecentLogs`. Pura (sin red).
pub(crate) fn recent_query(limit: u32, guild: Option<&str>) -> Query {
    Query {
        filters: guild
            .map(|g| vec![("guild_id".into(), Op::Eq, QValue::String(g.into()))])
            .unwrap_or_default(),
        order_by: Some(("createdAt".into(), Dir::Desc)),
        limit: Some(limit),
    }
}

/// Repo sobre Firestore (o sobre cualquier [`DocSource`]), sin caché: la caché de 30 s de
/// `/logs` vive en la ruta.
pub struct FirestoreLogsRepo<S = FirestoreClient> {
    source: S,
}

impl<S: DocSource> FirestoreLogsRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

#[async_trait]
impl<S: DocSource> LogsRepo for FirestoreLogsRepo<S> {
    async fn recent(&self, limit: u32, guild: Option<&str>) -> Result<Vec<LogEntry>, DataError> {
        let docs = self
            .source
            .query(COLLECTION, recent_query(limit, guild))
            .await?;
        docs.into_iter()
            .map(|(id, doc)| {
                let mut entry: LogEntry = from_json(&id, doc)?;
                entry.id.get_or_insert(Value::String(id));
                Ok(entry)
            })
            .collect()
    }
}

/// Doble en memoria para pruebas de otros crates: filtra, ordena y limita como Firestore.
#[derive(Default)]
pub struct FakeLogsRepo {
    store: FakeStore<LogEntry>,
}

impl FakeLogsRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Añade un log.
    pub fn push(&self, entry: LogEntry) {
        self.store.push(entry);
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        self.store.fail_with(error);
    }
}

#[async_trait]
impl LogsRepo for FakeLogsRepo {
    async fn recent(&self, limit: u32, guild: Option<&str>) -> Result<Vec<LogEntry>, DataError> {
        let mut entries = self.store.snapshot()?;
        if let Some(guild) = guild {
            entries.retain(|e| e.guild_id() == Some(guild));
        }
        Ok(newest_first(
            entries,
            |e| e.created_at.as_ref(),
            Some(limit as usize),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::FakeSource;
    use serde_json::json;

    fn log(created: &str, guild: Option<&str>) -> Value {
        json!({"level": "error", "message": "m", "source": "bot", "guild_id": guild, "createdAt": created})
    }

    #[tokio::test]
    async fn logs_recent_filters_by_guild_and_limits() {
        let source = FakeSource::new().reply(Ok(vec![
            ("b".into(), log("2026-11-02T10:00:01Z", Some("g1"))),
            ("a".into(), log("2026-11-02T10:00:00Z", Some("g1"))),
        ]));
        let repo = FirestoreLogsRepo::new(source);
        let logs = repo.recent(2, Some("g1")).await.unwrap();

        let queries = repo.source.queries();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].0, "faceit_bot_logs");
        assert_eq!(
            queries[0].1,
            Query {
                filters: vec![("guild_id".into(), Op::Eq, QValue::String("g1".into()))],
                order_by: Some(("createdAt".into(), Dir::Desc)),
                limit: Some(2),
            }
        );
        assert_eq!(logs.len(), 2);
        assert_eq!(
            serde_json::to_value(&logs[0]).unwrap(),
            json!({
                "id": "b", "level": "error", "message": "m", "source": "bot", "guild_id": "g1",
                "createdAt": {"_seconds": 1_793_613_601_i64, "_nanoseconds": 0}
            })
        );

        // El doble hace lo mismo en memoria.
        let fake = FakeLogsRepo::new();
        for (id, created, guild) in [
            ("1", "2026-11-02T10:00:00Z", Some("g1")),
            ("2", "2026-11-02T10:00:02Z", Some("g2")),
            ("3", "2026-11-02T10:00:03Z", Some("g1")),
            ("4", "2026-11-02T10:00:04Z", Some("g1")),
        ] {
            let mut e: LogEntry = serde_json::from_value(log(created, guild)).unwrap();
            e.id = Some(json!(id));
            fake.push(e);
        }
        let got: Vec<_> = fake
            .recent(2, Some("g1"))
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.id.unwrap())
            .collect();
        assert_eq!(got, vec![json!("4"), json!("3")]);
        assert_eq!(fake.recent(10, None).await.unwrap().len(), 4);
    }

    #[test]
    fn without_guild_only_orders_by_created_at() {
        assert_eq!(
            recent_query(50, None),
            Query {
                filters: vec![],
                order_by: Some(("createdAt".into(), Dir::Desc)),
                limit: Some(50),
            }
        );
    }

    #[tokio::test]
    async fn document_id_field_wins_over_document_name_and_nulls_survive() {
        let source = FakeSource::new().reply(Ok(vec![(
            "auto".into(),
            json!({"id": "propio", "guild_id": null, "createdAt": null}),
        )]));
        let logs = FirestoreLogsRepo::new(source)
            .recent(5, None)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&logs[0]).unwrap(),
            json!({"id": "propio", "guild_id": null, "createdAt": null})
        );
    }

    #[tokio::test]
    async fn list_propagates_quota_error() {
        let source = FakeSource::new().reply(Err(DataError::QuotaExceeded));
        let err = FirestoreLogsRepo::new(source)
            .recent(50, Some("g1"))
            .await
            .unwrap_err();
        assert_eq!(err, DataError::QuotaExceeded);

        let fake = FakeLogsRepo::new();
        fake.fail_with(Some(DataError::QuotaExceeded));
        assert_eq!(fake.recent(1, None).await, Err(DataError::QuotaExceeded));
    }
}
