//! Llamadas de soporte (colección `faceit_support_calls`): modelo y repositorio de solo lectura.
//!
//! Las escribe `createSupportCall` (`database.js` ~2112: id del documento = `supportId`, con
//! `{ id, match_id, match_url, faceit_player_id, faceit_nickname, discord_id, description,
//! status: 'pendiente', claimedBy: null, claimedAt: null, guild_id, createdAt: serverTimestamp() }`)
//! y `claimSupportCall` las marca (`status: 'atendida'`, `claimedBy`, `claimedAt: Date.now()`).
//! Las lee `getSupportCalls` (~2174) para `GET /api/support-calls` (`routes/api.js` ~1731, que
//! siempre pide 50) y responde `{ calls }`.
//!
//! Lectura del legacy: `orderBy('createdAt', 'desc')`, con `guildId` además
//! `where('guild_id', '==', guildId)`, y `limit(limit)`; cada llamada sale como `doc.data()` (sin
//! añadir el id del documento: el documento ya trae su campo `id`).

use crate::error::DataError;
use crate::firestore::{Dir, FirestoreClient, Op, Query, Value as QValue};
use crate::model::{newest_first, node_timestamp_verbatim, FakeStore};
use crate::source::{from_json, DocSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const COLLECTION: &str = "faceit_support_calls";

/// Una llamada de soporte tal como la devuelve `GET /api/support-calls` del legacy. Todo lo que
/// no es `createdAt` se conserva tal cual en [`SupportCall::extra`] (incluidos `claimedBy: null`
/// y `claimedAt`, que es un número de `Date.now()`, no un `Timestamp`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SupportCall {
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

impl SupportCall {
    /// `guild_id` si es un texto.
    pub fn guild_id(&self) -> Option<&str> {
        self.extra.get("guild_id").and_then(Value::as_str)
    }
}

/// Lectura de las llamadas de soporte recientes.
///
/// **Errores:** se devuelven tal cual (`DataError::QuotaExceeded` incluido). El legacy
/// (`getSupportCalls`) traga *cualquier* error y devuelve `[]`, así que la ruta (Tarea 7) debe
/// responder `{ calls: [] }` ante un `Err`. Con `guild` la consulta necesita el índice compuesto
/// `guild_id` + `createdAt desc`, que no consta en `firestore.indexes.json` (ver "Fase 2A" en
/// `docs/riesgos.md`).
#[async_trait]
pub trait SupportCallsRepo: Send + Sync {
    /// Las `limit` llamadas más recientes (por `createdAt` descendente), solo las de `guild` si
    /// se da.
    async fn list(&self, limit: u32, guild: Option<&str>) -> Result<Vec<SupportCall>, DataError>;
}

/// Consulta de `getSupportCalls`. Pura (sin red).
pub(crate) fn list_query(limit: u32, guild: Option<&str>) -> Query {
    Query {
        filters: guild
            .map(|g| vec![("guild_id".into(), Op::Eq, QValue::String(g.into()))])
            .unwrap_or_default(),
        order_by: Some(("createdAt".into(), Dir::Desc)),
        limit: Some(limit),
    }
}

/// Repo sobre Firestore (o sobre cualquier [`DocSource`]), sin caché.
pub struct FirestoreSupportCallsRepo<S = FirestoreClient> {
    source: S,
}

impl<S: DocSource> FirestoreSupportCallsRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

#[async_trait]
impl<S: DocSource> SupportCallsRepo for FirestoreSupportCallsRepo<S> {
    async fn list(&self, limit: u32, guild: Option<&str>) -> Result<Vec<SupportCall>, DataError> {
        let docs = self
            .source
            .query(COLLECTION, list_query(limit, guild))
            .await?;
        docs.into_iter()
            .map(|(id, doc)| from_json(&id, doc))
            .collect()
    }
}

/// Doble en memoria para pruebas de otros crates: filtra, ordena y limita como Firestore.
#[derive(Default)]
pub struct FakeSupportCallsRepo {
    store: FakeStore<SupportCall>,
}

impl FakeSupportCallsRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Añade una llamada.
    pub fn push(&self, call: SupportCall) {
        self.store.push(call);
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        self.store.fail_with(error);
    }
}

#[async_trait]
impl SupportCallsRepo for FakeSupportCallsRepo {
    async fn list(&self, limit: u32, guild: Option<&str>) -> Result<Vec<SupportCall>, DataError> {
        let mut calls = self.store.snapshot()?;
        if let Some(guild) = guild {
            calls.retain(|c| c.guild_id() == Some(guild));
        }
        Ok(newest_first(
            calls,
            |c| c.created_at.as_ref(),
            Some(limit as usize),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::FakeSource;
    use serde_json::json;

    #[tokio::test]
    async fn support_calls_list_filters_by_guild_and_keeps_document_shape() {
        let doc = json!({
            "id": "SUP-1", "match_id": "m1", "match_url": "u", "faceit_player_id": "p",
            "faceit_nickname": "neo", "discord_id": "d", "description": "x",
            "status": "atendida", "claimedBy": "WebAdmin (ana)", "claimedAt": 1_793_613_600_123_i64,
            "guild_id": "g1", "createdAt": "2026-11-02T10:00:00.5Z"
        });
        let source = FakeSource::new().reply(Ok(vec![("SUP-1".into(), doc.clone())]));
        let repo = FirestoreSupportCallsRepo::new(source);
        let calls = repo.list(50, Some("g1")).await.unwrap();

        assert_eq!(
            repo.source.queries(),
            vec![(
                "faceit_support_calls".to_string(),
                Query {
                    filters: vec![("guild_id".into(), Op::Eq, QValue::String("g1".into()))],
                    order_by: Some(("createdAt".into(), Dir::Desc)),
                    limit: Some(50),
                }
            )]
        );
        let mut expected = doc;
        expected["createdAt"] = json!({"_seconds": 1_793_613_600_i64, "_nanoseconds": 500_000_000});
        assert_eq!(serde_json::to_value(&calls[0]).unwrap(), expected);
    }

    #[tokio::test]
    async fn no_id_is_added_when_the_document_has_none() {
        let source = FakeSource::new().reply(Ok(vec![(
            "SUP-2".into(),
            json!({"claimedBy": null, "guild_id": null}),
        )]));
        let calls = FirestoreSupportCallsRepo::new(source)
            .list(50, None)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&calls[0]).unwrap(),
            json!({"claimedBy": null, "guild_id": null})
        );
        assert_eq!(list_query(50, None).filters, vec![]);
    }

    #[tokio::test]
    async fn list_propagates_quota_error() {
        let source = FakeSource::new().reply(Err(DataError::QuotaExceeded));
        assert_eq!(
            FirestoreSupportCallsRepo::new(source).list(50, None).await,
            Err(DataError::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn fake_filters_orders_and_limits() {
        let fake = FakeSupportCallsRepo::new();
        for (id, created, guild) in [
            ("a", "2026-11-02T10:00:00Z", "g1"),
            ("b", "2026-11-02T10:00:01Z", "g2"),
            ("c", "2026-11-02T10:00:02Z", "g1"),
        ] {
            fake.push(
                serde_json::from_value(json!({"id": id, "guild_id": guild, "createdAt": created}))
                    .unwrap(),
            );
        }
        let ids: Vec<_> = fake
            .list(5, Some("g1"))
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.extra["id"].clone())
            .collect();
        assert_eq!(ids, vec![json!("c"), json!("a")]);
    }
}
