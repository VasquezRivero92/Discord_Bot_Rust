//! Historial de partidas (colección `faceit_matches_history`): modelo y repositorio de solo lectura.
//!
//! Lo escribe `addMatchToHistory` (`database.js` ~1607; id del documento = `matchId`, con `id`,
//! `map`, `faction*_*`, `winner`, `status`, `guild_id`, `launch_mode`, `stats`, `createdAt` y
//! `updatedAt` como `serverTimestamp()`) y lo lee `getMatchHistory` (~1669) para
//! `GET /api/matches/history` (`routes/api.js` ~1486), que responde `{ history }`.
//!
//! **Una sola colección para FACEIT y Mixton.** El comentario de la ruta ("partidas de FACEIT y
//! Mixton ordenadas cronológicamente") no corresponde a dos fuentes: las dos ramas de la ruta
//! llaman a `getMatchHistory` sobre la misma colección, donde las partidas de Mixton
//! (`mixton_*`/`manual_*`, `launch_mode: 'steam'`) y las de FACEIT conviven. Sin `guild`, la
//! mezcla cronológica es simplemente `orderBy('createdAt', 'desc')` sin filtro.
//!
//! Lectura del legacy, en orden:
//! 1. `safeLimit = min(max(parseInt(limit) || 20, 1), 50)` (dentro de `database.js`).
//! 2. Consulta: con `guildId`, `where('guild_id', '==', guildId)`; `orderBy('createdAt', 'desc')`,
//!    `limit(safeLimit)`.
//! 3. Si esa consulta falla (por cualquier error; pensado para el índice compuesto que falta o se
//!    está construyendo), respaldo: `orderBy('createdAt', 'desc').limit(max(safeLimit * 4, 60))`
//!    sin filtro, filtrado en memoria por `doc.guild_id === guildId` y cortado en `safeLimit`.
//! 4. Orden en memoria por `createdAt` descendente (`toDate().getTime()`; sin fecha = 0) y corte
//!    en `safeLimit`. Cada partida sale como `doc.data()` (sin añadir el id del documento).
//! 5. Si falla también el respaldo, `[]`. Hay además una caché de 5 min que aquí no existe.

use crate::error::DataError;
use crate::firestore::{Dir, FirestoreClient, Op, Query, Value as QValue};
use crate::model::{newest_first, node_timestamp_verbatim, timestamp_millis, FakeStore};
use crate::source::{from_json, DocSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const COLLECTION: &str = "faceit_matches_history";
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;
const FALLBACK_MIN: u32 = 60;

/// Una partida del historial tal como la devuelve `GET /api/matches/history` del legacy. Todo lo
/// que no son las fechas se conserva tal cual en [`MatchHistoryEntry::extra`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MatchHistoryEntry {
    /// `createdAt`: `serverTimestamp()` del alta; se serializa como `{ _seconds, _nanoseconds }`.
    #[serde(
        rename = "createdAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<Value>,
    /// `updatedAt`: `serverTimestamp()` de la última escritura.
    #[serde(
        rename = "updatedAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<Value>,
    /// El resto de campos del documento, sin tocar.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl MatchHistoryEntry {
    /// `guild_id` si es un texto.
    pub fn guild_id(&self) -> Option<&str> {
        self.extra.get("guild_id").and_then(Value::as_str)
    }

    /// Clave del orden en memoria del legacy: milisegundos de `createdAt`, 0 si no hay fecha.
    fn sort_key(&self) -> i64 {
        self.created_at
            .as_ref()
            .and_then(timestamp_millis)
            .unwrap_or(0)
    }
}

/// Lectura del historial de partidas.
///
/// **Errores:** el respaldo del legacy para la consulta principal fallida (paso 3 del módulo) sí
/// está aquí, porque es una segunda lectura; se aplica ante cualquier error **salvo**
/// `DataError::QuotaExceeded`, que se devuelve en el acto (el respaldo gastaría más lecturas de
/// una cuota ya agotada). Si falla el respaldo se devuelve su error. El legacy convierte esos
/// errores en `[]`, así que la ruta (Tarea 7) debe responder `{ history: [] }` ante un `Err`.
#[async_trait]
pub trait MatchHistoryRepo: Send + Sync {
    /// Las partidas más recientes (por `createdAt` descendente), solo las de `guild` si se da.
    /// `limit` 0 vale 20 y se acota a 1..=50 (como `getMatchHistory`; el `parseInt(...) || 20`
    /// de la ruta y el tratamiento de negativos, que el legacy acota a 1, son de la ruta).
    async fn list(
        &self,
        limit: u32,
        guild: Option<&str>,
    ) -> Result<Vec<MatchHistoryEntry>, DataError>;
}

/// `safeLimit` de `getMatchHistory` para un `limit` ya no negativo.
pub(crate) fn safe_limit(limit: u32) -> u32 {
    if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.min(MAX_LIMIT)
    }
}

/// Consulta principal. Pura (sin red).
pub(crate) fn primary_query(safe_limit: u32, guild: Option<&str>) -> Query {
    Query {
        filters: guild
            .map(|g| vec![("guild_id".into(), Op::Eq, QValue::String(g.into()))])
            .unwrap_or_default(),
        order_by: Some(("createdAt".into(), Dir::Desc)),
        limit: Some(safe_limit),
    }
}

/// Consulta de respaldo (sin filtro: solo el índice automático de `createdAt`). Pura.
pub(crate) fn fallback_query(safe_limit: u32) -> Query {
    Query {
        filters: vec![],
        order_by: Some(("createdAt".into(), Dir::Desc)),
        limit: Some((safe_limit * 4).max(FALLBACK_MIN)),
    }
}

/// Repo sobre Firestore (o sobre cualquier [`DocSource`]), sin caché.
pub struct FirestoreMatchHistoryRepo<S = FirestoreClient> {
    source: S,
}

impl<S: DocSource> FirestoreMatchHistoryRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }

    async fn fetch(&self, q: Query) -> Result<Vec<MatchHistoryEntry>, DataError> {
        self.source
            .query(COLLECTION, q)
            .await?
            .into_iter()
            .map(|(id, doc)| from_json(&id, doc))
            .collect()
    }
}

#[async_trait]
impl<S: DocSource> MatchHistoryRepo for FirestoreMatchHistoryRepo<S> {
    async fn list(
        &self,
        limit: u32,
        guild: Option<&str>,
    ) -> Result<Vec<MatchHistoryEntry>, DataError> {
        let safe = safe_limit(limit);
        let mut history = match self.fetch(primary_query(safe, guild)).await {
            Ok(history) => history,
            Err(DataError::QuotaExceeded) => return Err(DataError::QuotaExceeded),
            Err(_) => {
                let mut docs = self.fetch(fallback_query(safe)).await?;
                if let Some(guild) = guild {
                    docs.retain(|d| d.guild_id() == Some(guild));
                }
                docs.truncate(safe as usize);
                docs
            }
        };
        // Verificación del legacy: orden estable por `createdAt` descendente.
        history.sort_by_key(|d| std::cmp::Reverse(d.sort_key()));
        history.truncate(safe as usize);
        Ok(history)
    }
}

/// Doble en memoria para pruebas de otros crates: acota el límite, filtra y ordena como la
/// lectura real (sin el respaldo, que no hace falta en memoria).
#[derive(Default)]
pub struct FakeMatchHistoryRepo {
    store: FakeStore<MatchHistoryEntry>,
}

impl FakeMatchHistoryRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Añade una partida.
    pub fn push(&self, entry: MatchHistoryEntry) {
        self.store.push(entry);
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        self.store.fail_with(error);
    }
}

#[async_trait]
impl MatchHistoryRepo for FakeMatchHistoryRepo {
    async fn list(
        &self,
        limit: u32,
        guild: Option<&str>,
    ) -> Result<Vec<MatchHistoryEntry>, DataError> {
        let mut entries = self.store.snapshot()?;
        if let Some(guild) = guild {
            entries.retain(|e| e.guild_id() == Some(guild));
        }
        Ok(newest_first(
            entries,
            |e| e.created_at.as_ref(),
            Some(safe_limit(limit) as usize),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::FakeSource;
    use serde_json::json;

    fn m(id: &str, created: &str, guild: Option<&str>) -> (String, Value) {
        (
            id.to_string(),
            json!({"id": id, "guild_id": guild, "createdAt": created, "updatedAt": created}),
        )
    }

    fn ids(history: &[MatchHistoryEntry]) -> Vec<Value> {
        history.iter().map(|h| h.extra["id"].clone()).collect()
    }

    fn missing_index() -> DataError {
        DataError::Firestore("FAILED_PRECONDITION: The query requires an index".into())
    }

    #[tokio::test]
    async fn match_history_without_guild_mixes_sources_in_chronological_order() {
        // FACEIT y Mixton en la misma colección; se entregan desordenados para comprobar
        // también el orden en memoria del legacy.
        let source = FakeSource::new().reply(Ok(vec![
            m("1-faceit-a", "2026-11-02T10:00:00Z", Some("g1")),
            m("mixton_9", "2026-11-02T12:00:00Z", Some("g2")),
            m("1-faceit-b", "2026-11-02T11:00:00Z", None),
            m("manual_3", "2026-11-02T09:00:00Z", Some("g1")),
        ]));
        let repo = FirestoreMatchHistoryRepo::new(source);
        let history = repo.list(20, None).await.unwrap();

        assert_eq!(
            repo.source.queries(),
            vec![(
                "faceit_matches_history".to_string(),
                Query {
                    filters: vec![],
                    order_by: Some(("createdAt".into(), Dir::Desc)),
                    limit: Some(20),
                }
            )]
        );
        assert_eq!(
            ids(&history),
            vec![
                json!("mixton_9"),
                json!("1-faceit-b"),
                json!("1-faceit-a"),
                json!("manual_3")
            ]
        );
        assert_eq!(
            serde_json::to_value(&history[0]).unwrap(),
            json!({
                "id": "mixton_9", "guild_id": "g2",
                "createdAt": {"_seconds": 1_793_620_800_i64, "_nanoseconds": 0},
                "updatedAt": {"_seconds": 1_793_620_800_i64, "_nanoseconds": 0}
            })
        );
    }

    #[tokio::test]
    async fn with_guild_filters_in_firestore() {
        let source = FakeSource::new().reply(Ok(vec![m("a", "2026-11-02T10:00:00Z", Some("g1"))]));
        let repo = FirestoreMatchHistoryRepo::new(source);
        assert_eq!(
            ids(&repo.list(5, Some("g1")).await.unwrap()),
            vec![json!("a")]
        );
        assert_eq!(
            repo.source.queries()[0].1,
            Query {
                filters: vec![("guild_id".into(), Op::Eq, QValue::String("g1".into()))],
                order_by: Some(("createdAt".into(), Dir::Desc)),
                limit: Some(5),
            }
        );
    }

    #[tokio::test]
    async fn missing_index_falls_back_to_created_at_scan_filtered_in_memory() {
        let source = FakeSource::new().reply(Err(missing_index())).reply(Ok(vec![
            m("c", "2026-11-02T12:00:00Z", Some("g2")),
            m("b", "2026-11-02T11:00:00Z", Some("g1")),
            m("x", "2026-11-02T10:30:00Z", None),
            m("a", "2026-11-02T10:00:00Z", Some("g1")),
            m("z", "2026-11-02T09:00:00Z", Some("g1")),
        ]));
        let repo = FirestoreMatchHistoryRepo::new(source);
        let history = repo.list(2, Some("g1")).await.unwrap();

        let queries = repo.source.queries();
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[1].1, fallback_query(2));
        assert_eq!(queries[1].1.limit, Some(60), "max(2 * 4, 60)");
        assert!(queries[1].1.filters.is_empty());
        assert_eq!(ids(&history), vec![json!("b"), json!("a")]);
    }

    #[test]
    fn limits_follow_get_match_history() {
        assert_eq!(safe_limit(0), 20);
        assert_eq!(safe_limit(1), 1);
        assert_eq!(safe_limit(50), 50);
        assert_eq!(safe_limit(500), 50);
        assert_eq!(fallback_query(20).limit, Some(80));
        assert_eq!(fallback_query(10).limit, Some(60));
        assert_eq!(primary_query(safe_limit(999), None).limit, Some(50));
    }

    #[tokio::test]
    async fn list_propagates_quota_error_without_fallback() {
        let source = FakeSource::new().reply(Err(DataError::QuotaExceeded));
        let repo = FirestoreMatchHistoryRepo::new(source);
        assert_eq!(
            repo.list(20, Some("g1")).await,
            Err(DataError::QuotaExceeded)
        );
        assert_eq!(repo.source.queries().len(), 1, "no se intenta el respaldo");
    }

    #[tokio::test]
    async fn fallback_error_is_returned() {
        let source = FakeSource::new()
            .reply(Err(missing_index()))
            .reply(Err(DataError::QuotaExceeded));
        assert_eq!(
            FirestoreMatchHistoryRepo::new(source).list(20, None).await,
            Err(DataError::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn fake_clamps_filters_and_orders() {
        let fake = FakeMatchHistoryRepo::new();
        for i in 0..60 {
            let (_, doc) = m(
                &format!("m{i:02}"),
                &format!("2026-11-02T10:{i:02}:00Z"),
                Some("g1"),
            );
            fake.push(serde_json::from_value(doc).unwrap());
        }
        let all = fake.list(0, Some("g1")).await.unwrap();
        assert_eq!(all.len(), 20);
        assert_eq!(all[0].extra["id"], json!("m59"));
        assert_eq!(fake.list(999, None).await.unwrap().len(), 50);
        assert!(fake.list(5, Some("g2")).await.unwrap().is_empty());
    }
}
