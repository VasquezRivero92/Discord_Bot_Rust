//! Libro de Reclamaciones (colección `claims`): modelo y repositorio de solo lectura.
//!
//! El documento lo escribe `POST /api/reclamaciones` del legacy (`routes/publicApi.js:123-207`
//! con `database.js` `recordClaim`, ~3166): el id del documento ES el `claimCode`
//! (`firestore.collection('claims').doc(claimCode).set(...)`) y `getClaim(code)` (`database.js`
//! ~3189) lo lee por id. Aquí no hay consultas por campo ni listados: la colección contiene datos
//! personales y solo se puede leer un reclamo si se conoce su código.
//!
//! Todo campo es opcional y tolerante; lo desconocido se conserva en [`Claim::extra`]. Al
//! serializar se obtiene el mismo JSON que devuelve el legacy (`res.json({ success, claim })`).

use crate::error::DataError;
use crate::firestore::FirestoreClient;
use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;

const COLLECTION: &str = "claims";

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

/// Marca de tiempo de Firestore tal como la serializa `res.json` en Node con firebase-admin:
/// `{ "_seconds": n, "_nanoseconds": n }`. El cliente de Rust entrega las marcas como texto
/// RFC 3339, que aquí se convierte de vuelta; un objeto ya con esa forma se conserva.
fn node_timestamp<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => chrono::DateTime::parse_from_rfc3339(&s).ok().map(|t| {
            json!({
                "_seconds": t.timestamp(),
                "_nanoseconds": t.timestamp_subsec_nanos(),
            })
        }),
        Some(obj @ Value::Object(_)) => Some(obj),
        _ => None,
    })
}

fn lenient_object<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(obj @ Value::Object(_)) => Some(obj),
        _ => None,
    })
}

fn lenient_number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(n @ Value::Number(_)) => Some(n),
        _ => None,
    })
}

/// Documento de `claims/{claimCode}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
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
    /// Cualquier otro campo del documento, sin perder nada.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Lectura de reclamos por código.
#[async_trait]
pub trait ClaimsRepo: Send + Sync {
    /// Lee `claims/{code}`. `Ok(None)` si no existe (o si `code` no puede ser un id válido de
    /// Firestore: el legacy lo trata igual, `getClaim` devuelve `null`).
    async fn get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError>;
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

/// Repo sobre Firestore: lee la colección `claims` por ID de documento, sin caché.
pub struct FirestoreClaimsRepo {
    client: FirestoreClient,
}

impl FirestoreClaimsRepo {
    /// Repo sobre `client`.
    pub fn new(client: FirestoreClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ClaimsRepo for FirestoreClaimsRepo {
    async fn get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError> {
        if !is_valid_document_id(code) {
            return Ok(None);
        }
        self.client.get_doc(COLLECTION, code).await
    }
}

/// Repo en memoria para pruebas de otros crates.
#[derive(Default)]
pub struct InMemoryClaimsRepo {
    docs: Mutex<HashMap<String, Claim>>,
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
}

#[async_trait]
impl ClaimsRepo for InMemoryClaimsRepo {
    async fn get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError> {
        Ok(self
            .docs
            .lock()
            .expect("mutex envenenado")
            .get(code)
            .cloned())
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
}
