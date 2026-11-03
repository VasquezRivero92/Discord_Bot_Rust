//! Libro de Reclamaciones (colección `claims`): modelo y repositorio de solo lectura.
//!
//! El documento lo escribe `POST /api/reclamaciones` del legacy (`routes/publicApi.js:123-207`
//! con `database.js` `recordClaim`, ~3166): el id del documento ES el `claimCode`
//! (`firestore.collection('claims').doc(claimCode).set(...)`) y `getClaim(code)` (`database.js`
//! ~3189) lo lee por id para la ruta pública.
//!
//! El panel de administración además los lista (`getClaims`, ~3220, para `GET /api/claims`,
//! `routes/api.js` ~1880): la colección **entera** (con `status`, solo
//! `where('status', '==', status)`; sin orden ni límite en Firestore), cada reclamo como
//! `{ id: doc.id, ...doc.data() }`, orden en memoria de más nuevo a más viejo por `createdAtIso`
//! (o `createdAt._seconds`) y `slice(0, limit)`. La colección contiene datos personales: el
//! listado es solo para la ruta de administración autenticada.
//!
//! Todo campo es opcional y tolerante; lo desconocido se conserva en [`Claim::extra`]. Al
//! serializar se obtiene el mismo JSON que devuelve el legacy (`res.json({ success, claim })`).

use crate::error::DataError;
use crate::firestore::{FirestoreClient, Op, Query, Value as QValue};
use crate::model::{
    created_iso_sort_key, lenient_number, lenient_object, lenient_string, node_timestamp,
    node_timestamp_verbatim, present,
};
use crate::source::{from_json, DocSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

const COLLECTION: &str = "claims";

/// Documento de `claims/{claimCode}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    /// `id`: solo en el listado (`{ id: doc.id, ...doc.data() }`); un campo `id` propio del
    /// documento, si lo hubiera, se conserva y gana. `get_by_code` no lo añade.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<Value>,
    /// `claimCode`: `LR-<año>-<seis dígitos>`; coincide con el id del documento.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub claim_code: Option<String>,
    /// `consumer`: `{ name, docType, docNumber, email, phone, address, isMinor, parent }`.
    #[serde(
        default,
        deserialize_with = "lenient_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub consumer: Option<Value>,
    /// `goods`: `{ type, amount, currency, description }`.
    #[serde(
        default,
        deserialize_with = "lenient_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub goods: Option<Value>,
    /// `claim`: `{ type, detail, request }`.
    #[serde(
        default,
        deserialize_with = "lenient_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub claim: Option<Value>,
    /// `merchant`: `{ companyName, address, email, phone }`.
    #[serde(
        default,
        deserialize_with = "lenient_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub merchant: Option<Value>,
    /// `legalNotice`.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub legal_notice: Option<String>,
    /// `submittedAt`: ISO-8601 puesto por la ruta.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub submitted_at: Option<String>,
    /// `createdAt`: `serverTimestamp()`; se serializa como `{ _seconds, _nanoseconds }`.
    #[serde(
        default,
        deserialize_with = "node_timestamp",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<Value>,
    /// `createdAtIso`.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at_iso: Option<String>,
    /// `status` (`PENDIENTE`, ...).
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub status: Option<String>,
    /// `responseDeadlineDays`.
    #[serde(
        default,
        deserialize_with = "lenient_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub response_deadline_days: Option<Value>,
    /// `updatedAt`: `serverTimestamp()` de `updateClaim`; se serializa como
    /// `{ _seconds, _nanoseconds }` (cualquier otro valor, tal cual).
    #[serde(
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<Value>,
    /// `respondedAt`: `serverTimestamp()` de `POST /claims/:code/respond`; como `updatedAt`.
    #[serde(
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub responded_at: Option<Value>,
    /// Cualquier otro campo del documento, sin perder nada.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Claim {
    fn sort_key(&self) -> i64 {
        let iso = self.created_at_iso.clone().map(Value::String);
        created_iso_sort_key(iso.as_ref(), self.created_at.as_ref())
    }
}

/// Lectura de reclamos.
///
/// **Errores:** se devuelven tal cual (`DataError::QuotaExceeded` incluido). En `list`, el
/// legacy (`getClaims`) traga *cualquier* error y devuelve `[]`, así que la ruta (Tarea 7) debe
/// responder `{ claims: [] }` ante un `Err`.
#[async_trait]
pub trait ClaimsRepo: Send + Sync {
    /// Lee `claims/{code}`. `Ok(None)` si no existe (o si `code` no puede ser un id válido de
    /// Firestore: el legacy lo trata igual, `getClaim` devuelve `null`).
    async fn get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError>;

    /// Los `limit` reclamos más recientes, solo los de estado `status` si se da. Lee la colección
    /// entera (o todos los de ese estado) y ordena en memoria, como el legacy; no necesita
    /// índices compuestos.
    async fn list(&self, limit: u32, status: Option<&str>) -> Result<Vec<Claim>, DataError>;
}

/// Consulta de `getClaims`. Pura (sin red).
pub(crate) fn list_query(status: Option<&str>) -> Query {
    Query {
        filters: status
            .map(|s| vec![("status".into(), Op::Eq, QValue::String(s.into()))])
            .unwrap_or_default(),
        order_by: None,
        limit: None,
    }
}

/// Orden en memoria y corte del legacy (estable: los empates conservan el orden de llegada).
fn newest_first(mut claims: Vec<Claim>, limit: u32) -> Vec<Claim> {
    claims.sort_by_key(|c| std::cmp::Reverse(c.sort_key()));
    claims.truncate(limit as usize);
    claims
}

/// ¿Puede `code` ser el id de un documento de Firestore? (no vacío, sin `/`, distinto de `.` y
/// `..`, sin la forma reservada `__.*__`, como máximo 1500 bytes).
fn is_valid_document_id(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 1500
        && !code.contains('/')
        && code != "."
        && code != ".."
        && !(code.len() > 4 && code.starts_with("__") && code.ends_with("__"))
}

/// Repo sobre Firestore (o sobre cualquier [`DocSource`]): lee la colección `claims`, sin caché.
pub struct FirestoreClaimsRepo<S = FirestoreClient> {
    source: S,
}

impl<S: DocSource> FirestoreClaimsRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

#[async_trait]
impl<S: DocSource> ClaimsRepo for FirestoreClaimsRepo<S> {
    async fn get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError> {
        if !is_valid_document_id(code) {
            return Ok(None);
        }
        self.source
            .get(COLLECTION, code)
            .await?
            .map(|doc| from_json(code, doc))
            .transpose()
    }

    async fn list(&self, limit: u32, status: Option<&str>) -> Result<Vec<Claim>, DataError> {
        let docs = self.source.query(COLLECTION, list_query(status)).await?;
        let claims = docs
            .into_iter()
            .map(|(id, doc)| {
                let mut claim: Claim = from_json(&id, doc)?;
                claim.id.get_or_insert(Value::String(id));
                Ok(claim)
            })
            .collect::<Result<Vec<_>, DataError>>()?;
        Ok(newest_first(claims, limit))
    }
}

/// Repo en memoria para pruebas de otros crates (el doble de [`ClaimsRepo`]). El listado
/// recorre los reclamos por código, como Firestore por id, y añade `id` como el repo real.
#[derive(Default)]
pub struct InMemoryClaimsRepo {
    docs: Mutex<BTreeMap<String, Claim>>,
    error: Mutex<Option<DataError>>,
}

impl InMemoryClaimsRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Crea o reemplaza el reclamo `code`.
    pub fn insert(&self, code: &str, claim: Claim) {
        self.docs
            .lock()
            .expect("mutex envenenado")
            .insert(code.to_string(), claim);
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        *self.error.lock().expect("mutex envenenado") = error;
    }

    fn check(&self) -> Result<(), DataError> {
        match self.error.lock().expect("mutex envenenado").clone() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl ClaimsRepo for InMemoryClaimsRepo {
    async fn get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError> {
        self.check()?;
        Ok(self
            .docs
            .lock()
            .expect("mutex envenenado")
            .get(code)
            .cloned())
    }

    async fn list(&self, limit: u32, status: Option<&str>) -> Result<Vec<Claim>, DataError> {
        self.check()?;
        let claims = self
            .docs
            .lock()
            .expect("mutex envenenado")
            .iter()
            .filter(|(_, c)| status.is_none() || c.status.as_deref() == status)
            .map(|(code, c)| {
                let mut c = c.clone();
                c.id.get_or_insert_with(|| Value::String(code.clone()));
                c
            })
            .collect();
        Ok(newest_first(claims, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firestore::decode_document;
    use firestore::gcloud_sdk::google::firestore::v1::value::ValueType;
    use firestore::gcloud_sdk::google::firestore::v1::{
        ArrayValue, Document, MapValue, Value as FsValue,
    };
    use firestore::gcloud_sdk::prost_types::Timestamp;
    use serde_json::json;
    use std::collections::HashMap;

    fn val(v: ValueType) -> FsValue {
        FsValue {
            value_type: Some(v),
        }
    }

    fn map(entries: Vec<(&str, FsValue)>) -> FsValue {
        val(ValueType::MapValue(MapValue {
            fields: entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }))
    }

    #[test]
    fn decodes_firestore_document_with_timestamp_nested_null_and_unknown_fields() {
        let mut fields = HashMap::new();
        fields.insert(
            "claimCode".into(),
            val(ValueType::StringValue("LR-2026-1".into())),
        );
        fields.insert(
            "consumer".into(),
            map(vec![
                ("name", val(ValueType::StringValue("Ana".into()))),
                ("isMinor", val(ValueType::BooleanValue(false))),
                ("parent", val(ValueType::NullValue(0))),
            ]),
        );
        fields.insert(
            "goods".into(),
            map(vec![("amount", val(ValueType::DoubleValue(0.0)))]),
        );
        fields.insert(
            "createdAt".into(),
            val(ValueType::TimestampValue(Timestamp {
                seconds: 1_793_613_600,
                nanos: 5_000_000,
            })),
        );
        fields.insert(
            "responseDeadlineDays".into(),
            val(ValueType::IntegerValue(15)),
        );
        fields.insert(
            "tags".into(),
            val(ValueType::ArrayValue(ArrayValue {
                values: vec![val(ValueType::StringValue("x".into()))],
            })),
        );
        let doc = Document {
            name: "projects/p/databases/(default)/documents/claims/LR-2026-1".into(),
            fields,
            ..Default::default()
        };
        let (id, c): (String, Claim) = decode_document(&doc).expect("decodifica");
        assert_eq!(id, "LR-2026-1");
        assert_eq!(c.claim_code.as_deref(), Some("LR-2026-1"));
        assert_eq!(c.consumer.as_ref().unwrap()["parent"], Value::Null);
        assert_eq!(
            c.created_at,
            Some(json!({"_seconds": 1_793_613_600_i64, "_nanoseconds": 5_000_000}))
        );
        assert_eq!(c.response_deadline_days, Some(json!(15)));
        assert_eq!(c.extra["tags"], json!(["x"]));
    }

    #[test]
    fn tolerates_missing_and_wrong_typed_fields() {
        let c: Claim = serde_json::from_value(json!({
            "claimCode": 123,
            "consumer": "no soy objeto",
            "status": ["x"],
            "createdAt": "no es fecha",
            "responseDeadlineDays": "15"
        }))
        .unwrap();
        assert_eq!(c.claim_code.as_deref(), Some("123"));
        assert!(c.consumer.is_none() && c.status.is_none());
        assert!(c.created_at.is_none() && c.response_deadline_days.is_none());

        let empty: Claim = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, Claim::default());
        assert_eq!(serde_json::to_value(&empty).unwrap(), json!({}));
    }

    #[test]
    fn extra_fields_survive_a_round_trip() {
        let doc = json!({"claimCode": "A", "mystery": {"a": [1, null]}, "status": "PENDIENTE"});
        let c: Claim = serde_json::from_value(doc.clone()).unwrap();
        assert_eq!(c.extra["mystery"], json!({"a": [1, null]}));
        assert_eq!(serde_json::to_value(&c).unwrap(), doc);
    }

    #[test]
    fn rejects_ids_firestore_cannot_hold() {
        for bad in ["", ".", "..", "a/b", "__x__", &"a".repeat(1501)] {
            assert!(!is_valid_document_id(bad), "{bad:?}");
        }
        for ok in ["LR-2026-123456", "__x", "x__", "a.b"] {
            assert!(is_valid_document_id(ok), "{ok:?}");
        }
    }

    #[tokio::test]
    async fn in_memory_repo_reads_by_code() {
        let repo = InMemoryClaimsRepo::new();
        assert!(repo.get_by_code("A").await.unwrap().is_none());
        repo.insert("A", Claim::default());
        assert!(repo.get_by_code("A").await.unwrap().is_some());
    }

    use crate::source::fake::FakeSource;

    #[tokio::test]
    async fn claims_list_filters_by_status_sorts_in_memory_and_adds_id() {
        let source = FakeSource::new().reply(Ok(vec![
            (
                "LR-2026-1".into(),
                json!({"claimCode": "LR-2026-1", "status": "PENDIENTE",
                       "createdAtIso": "2026-11-02T10:00:00.000Z",
                       "createdAt": "2026-11-02T10:00:00Z"}),
            ),
            (
                "LR-2026-2".into(),
                json!({"claimCode": "LR-2026-2", "status": "PENDIENTE",
                       "createdAtIso": "2026-11-02T11:00:00.000Z",
                       "respondedAt": "2026-11-02T12:00:00Z",
                       "updatedAt": "2026-11-02T12:00:00Z"}),
            ),
            (
                "LR-2026-3".into(),
                json!({"claimCode": "LR-2026-3", "status": "PENDIENTE",
                       "createdAtIso": "2026-11-02T09:00:00.000Z"}),
            ),
        ]));
        let repo = FirestoreClaimsRepo::new(source);
        let claims = repo.list(2, Some("PENDIENTE")).await.unwrap();

        assert_eq!(
            repo.source.queries(),
            vec![(
                "claims".to_string(),
                Query {
                    filters: vec![("status".into(), Op::Eq, QValue::String("PENDIENTE".into()))],
                    order_by: None,
                    limit: None,
                }
            )]
        );
        assert_eq!(
            serde_json::to_value(&claims).unwrap(),
            json!([
                {"id": "LR-2026-2", "claimCode": "LR-2026-2", "status": "PENDIENTE",
                 "createdAtIso": "2026-11-02T11:00:00.000Z",
                 "respondedAt": {"_seconds": 1_793_620_800_i64, "_nanoseconds": 0},
                 "updatedAt": {"_seconds": 1_793_620_800_i64, "_nanoseconds": 0}},
                {"id": "LR-2026-1", "claimCode": "LR-2026-1", "status": "PENDIENTE",
                 "createdAtIso": "2026-11-02T10:00:00.000Z",
                 "createdAt": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 0}}
            ])
        );
        assert_eq!(list_query(None), Query::default());
    }

    #[tokio::test]
    async fn get_by_code_reads_through_the_source_without_adding_id() {
        let source = FakeSource::new().doc(
            "claims",
            "LR-1",
            Ok(Some(json!({"claimCode": "LR-1", "status": "PENDIENTE"}))),
        );
        let claim = FirestoreClaimsRepo::new(source)
            .get_by_code("LR-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&claim).unwrap(),
            json!({"claimCode": "LR-1", "status": "PENDIENTE"})
        );
    }

    #[tokio::test]
    async fn list_propagates_quota_error() {
        let source = FakeSource::new().reply(Err(DataError::QuotaExceeded));
        assert_eq!(
            FirestoreClaimsRepo::new(source).list(100, None).await,
            Err(DataError::QuotaExceeded)
        );
        let fake = InMemoryClaimsRepo::new();
        fake.fail_with(Some(DataError::QuotaExceeded));
        assert_eq!(fake.list(100, None).await, Err(DataError::QuotaExceeded));
    }

    #[tokio::test]
    async fn in_memory_repo_lists_like_the_real_one() {
        let repo = InMemoryClaimsRepo::new();
        for (code, status, iso) in [
            ("A", "PENDIENTE", "2026-11-02T10:00:00.000Z"),
            ("B", "ATENDIDO", "2026-11-02T11:00:00.000Z"),
            ("C", "PENDIENTE", "2026-11-02T12:00:00.000Z"),
        ] {
            repo.insert(
                code,
                serde_json::from_value(json!({"status": status, "createdAtIso": iso})).unwrap(),
            );
        }
        let ids: Vec<_> = repo
            .list(100, Some("PENDIENTE"))
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id.unwrap())
            .collect();
        assert_eq!(ids, vec![json!("C"), json!("A")]);
        assert_eq!(repo.list(1, None).await.unwrap().len(), 1);
    }
}
