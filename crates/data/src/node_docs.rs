//! Documentos completos tal como los devuelve `res.json(...)` del legacy.
//!
//! Algunas rutas del panel reenvían documentos enteros (`{ id: doc.id, ...doc.data() }`) sin
//! modelo. El deserializador serde del crate `firestore` entrega los `Timestamp` como texto RFC
//! 3339, indistinguibles de un texto con fecha que el bot guardó como cadena; aquí se convierte
//! directamente desde los tipos de Firestore a lo que produce `JSON.stringify` en Node con
//! firebase-admin:
//!
//! | Firestore | JSON de Node |
//! |---|---|
//! | `Timestamp` | `{ "_seconds": s, "_nanoseconds": n }` |
//! | `GeoPoint` | `{ "_latitude": a, "_longitude": b }` |
//! | bytes (`Buffer`) | `{ "type": "Buffer", "data": [..] }` |
//! | entero | número (un entero de más de 2^53 pierde precisión, como en JS) |
//! | doble | número; `NaN`/`±Infinity` -> `null` |
//! | referencia | **texto con la ruta del documento** (diferencia deliberada: Node volcaría el objeto interno `DocumentReference`, con el cliente de Firestore dentro) |
//! | mapa / array / null / bool / texto | igual |

use crate::error::DataError;
use crate::firestore::{FirestoreClient, Query};
use async_trait::async_trait;
use firestore::gcloud_sdk::google::firestore::v1::{value::ValueType, Value as PbValue};
use firestore::FirestoreDocument;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Mayor entero que un `Number` de JS representa sin pérdida (2^53).
const JS_MAX_SAFE: f64 = 9_007_199_254_740_992.0;

/// Un número JSON como lo escribiría `JSON.stringify` para el `Number` `f`.
pub(crate) fn js_number_json(f: f64) -> Value {
    if !f.is_finite() {
        Value::Null
    } else if f.fract() == 0.0 && f.abs() <= JS_MAX_SAFE {
        // `1.0` sale como `1` y `-0` como `0` en JS.
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// Valor de Firestore -> JSON de Node (ver la tabla del módulo).
pub(crate) fn node_json(v: &PbValue) -> Value {
    let Some(t) = &v.value_type else {
        return Value::Null;
    };
    match t {
        ValueType::NullValue(_) => Value::Null,
        ValueType::BooleanValue(b) => Value::Bool(*b),
        ValueType::IntegerValue(n) => {
            if (*n as f64).abs() <= JS_MAX_SAFE {
                json!(n)
            } else {
                js_number_json(*n as f64)
            }
        }
        ValueType::DoubleValue(f) => js_number_json(*f),
        ValueType::TimestampValue(ts) => {
            json!({ "_seconds": ts.seconds, "_nanoseconds": ts.nanos })
        }
        ValueType::StringValue(s) => Value::String(s.clone()),
        ValueType::BytesValue(b) => json!({ "type": "Buffer", "data": b }),
        ValueType::ReferenceValue(r) => Value::String(reference_path(r).to_owned()),
        ValueType::GeoPointValue(g) => {
            json!({ "_latitude": js_number_json(g.latitude), "_longitude": js_number_json(g.longitude) })
        }
        ValueType::ArrayValue(a) => Value::Array(a.values.iter().map(node_json).collect()),
        ValueType::MapValue(m) => Value::Object(node_fields(&m.fields)),
        // Tipos que solo existen en consultas/pipelines, nunca en documentos guardados.
        _ => Value::Null,
    }
}

/// `projects/p/databases/d/documents/col/id` -> `col/id`.
fn reference_path(r: &str) -> &str {
    r.split_once("/documents/").map_or(r, |(_, path)| path)
}

fn node_fields(fields: &HashMap<String, PbValue>) -> Map<String, Value> {
    fields
        .iter()
        .map(|(k, v)| (k.clone(), node_json(v)))
        .collect()
}

/// `(id, doc.data())` de un documento crudo.
pub(crate) fn node_document(
    doc: &FirestoreDocument,
) -> Result<(String, Map<String, Value>), DataError> {
    let id = crate::firestore::document_id(&doc.name)
        .ok_or_else(|| DataError::Decode(format!("nombre de documento sin id: {:?}", doc.name)))?
        .to_string();
    Ok((id, node_fields(&doc.fields)))
}

/// Lecturas de documentos completos (como JSON de Node) y recuentos. Costura entre los repos
/// del panel y [`FirestoreClient`] para probarlos sin red ([`InMemoryNodeDocs`]).
#[async_trait]
pub trait NodeDocSource: Send + Sync {
    /// `collection(c).get()`: todos los documentos `(id, doc.data())`, en el orden de Firestore
    /// (por id).
    async fn list_all(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, Map<String, Value>)>, DataError>;
    /// `collection(c)[.where(field, '==', value)].count().get()`.
    async fn count(&self, collection: &str, eq: Option<(&str, &str)>) -> Result<u64, DataError>;
}

#[async_trait]
impl NodeDocSource for FirestoreClient {
    async fn list_all(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, Map<String, Value>)>, DataError> {
        self.list_raw_docs(collection)
            .await?
            .iter()
            .map(node_document)
            .collect()
    }

    async fn count(&self, collection: &str, eq: Option<(&str, &str)>) -> Result<u64, DataError> {
        let filters = eq
            .map(|(field, value)| {
                vec![(
                    field.to_string(),
                    crate::firestore::Op::Eq,
                    crate::firestore::Value::String(value.to_string()),
                )]
            })
            .unwrap_or_default();
        FirestoreClient::count(
            self,
            collection,
            Query {
                filters,
                ..Query::default()
            },
        )
        .await
    }
}

#[async_trait]
impl<T: NodeDocSource + ?Sized> NodeDocSource for Arc<T> {
    async fn list_all(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, Map<String, Value>)>, DataError> {
        (**self).list_all(collection).await
    }

    async fn count(&self, collection: &str, eq: Option<(&str, &str)>) -> Result<u64, DataError> {
        (**self).count(collection, eq).await
    }
}

/// Documentos de una colección por id.
type Collection = BTreeMap<String, Map<String, Value>>;

/// Origen en memoria para pruebas (también de otros crates): colecciones ordenadas por id,
/// errores inyectables por separado para listados y recuentos, contador de listados por
/// colección y una compuerta para retener las lecturas (pruebas de concurrencia sin esperas).
#[derive(Default)]
pub struct InMemoryNodeDocs {
    collections: Mutex<BTreeMap<String, Collection>>,
    list_error: Mutex<Option<DataError>>,
    count_error: Mutex<Option<DataError>>,
    lists: Mutex<HashMap<String, usize>>,
    counts: Mutex<usize>,
    gate: tokio::sync::RwLock<()>,
}

impl InMemoryNodeDocs {
    /// Origen vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Crea o reemplaza `collection/id` (`doc` debe ser un objeto JSON).
    pub fn put(&self, collection: &str, id: &str, doc: Value) {
        let Value::Object(map) = doc else {
            panic!("el documento debe ser un objeto: {doc}");
        };
        self.collections
            .lock()
            .expect("mutex envenenado")
            .entry(collection.to_string())
            .or_default()
            .insert(id.to_string(), map);
    }

    /// Con `Some(err)` todo listado falla con `err`.
    pub fn fail_lists(&self, err: Option<DataError>) {
        *self.list_error.lock().expect("mutex envenenado") = err;
    }

    /// Con `Some(err)` todo recuento falla con `err`.
    pub fn fail_counts(&self, err: Option<DataError>) {
        *self.count_error.lock().expect("mutex envenenado") = err;
    }

    /// Listados recibidos de `collection`.
    pub fn lists_of(&self, collection: &str) -> usize {
        self.lists
            .lock()
            .expect("mutex envenenado")
            .get(collection)
            .copied()
            .unwrap_or(0)
    }

    /// Recuentos recibidos.
    pub fn counts(&self) -> usize {
        *self.counts.lock().expect("mutex envenenado")
    }

    /// Retiene todas las lecturas hasta soltar el guardia devuelto.
    pub async fn hold(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.gate.write().await
    }
}

#[async_trait]
impl NodeDocSource for InMemoryNodeDocs {
    async fn list_all(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, Map<String, Value>)>, DataError> {
        *self
            .lists
            .lock()
            .expect("mutex envenenado")
            .entry(collection.to_string())
            .or_default() += 1;
        let _open = self.gate.read().await;
        if let Some(err) = self.list_error.lock().expect("mutex envenenado").clone() {
            return Err(err);
        }
        Ok(self
            .collections
            .lock()
            .expect("mutex envenenado")
            .get(collection)
            .map(|docs| docs.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default())
    }

    async fn count(&self, collection: &str, eq: Option<(&str, &str)>) -> Result<u64, DataError> {
        *self.counts.lock().expect("mutex envenenado") += 1;
        let _open = self.gate.read().await;
        if let Some(err) = self.count_error.lock().expect("mutex envenenado").clone() {
            return Err(err);
        }
        let collections = self.collections.lock().expect("mutex envenenado");
        let Some(docs) = collections.get(collection) else {
            return Ok(0);
        };
        Ok(docs
            .values()
            .filter(|d| match eq {
                None => true,
                Some((field, value)) => d.get(field).and_then(Value::as_str) == Some(value),
            })
            .count() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use firestore::gcloud_sdk::google::firestore::v1::{ArrayValue, MapValue};
    use firestore::gcloud_sdk::google::r#type::LatLng;
    use firestore::gcloud_sdk::prost_types::Timestamp;

    fn v(t: ValueType) -> PbValue {
        PbValue {
            value_type: Some(t),
        }
    }

    #[test]
    fn firestore_types_become_node_json() {
        let ts = v(ValueType::TimestampValue(Timestamp {
            seconds: 1_793_613_600,
            nanos: 5_000,
        }));
        assert_eq!(
            node_json(&ts),
            json!({"_seconds": 1_793_613_600_i64, "_nanoseconds": 5_000})
        );
        // Un texto con forma de fecha sigue siendo texto.
        assert_eq!(
            node_json(&v(ValueType::StringValue("2026-11-02T10:00:00Z".into()))),
            json!("2026-11-02T10:00:00Z")
        );
        assert_eq!(node_json(&v(ValueType::DoubleValue(1.0))), json!(1));
        assert_eq!(node_json(&v(ValueType::DoubleValue(-0.0))), json!(0));
        assert_eq!(node_json(&v(ValueType::DoubleValue(2.5))), json!(2.5));
        assert_eq!(node_json(&v(ValueType::DoubleValue(f64::NAN))), Value::Null);
        assert_eq!(
            node_json(&v(ValueType::DoubleValue(f64::INFINITY))),
            Value::Null
        );
        assert_eq!(node_json(&v(ValueType::IntegerValue(-7))), json!(-7));
        assert_eq!(node_json(&v(ValueType::NullValue(0))), Value::Null);
        assert_eq!(node_json(&PbValue { value_type: None }), Value::Null);
        assert_eq!(
            node_json(&v(ValueType::BytesValue(vec![1, 2]))),
            json!({"type": "Buffer", "data": [1, 2]})
        );
        assert_eq!(
            node_json(&v(ValueType::GeoPointValue(LatLng {
                latitude: -12.5,
                longitude: -77.0
            }))),
            json!({"_latitude": -12.5, "_longitude": -77})
        );
        assert_eq!(
            node_json(&v(ValueType::ReferenceValue(
                "projects/p/databases/(default)/documents/faceit_users/1".into()
            ))),
            json!("faceit_users/1")
        );
        let mut fields = HashMap::new();
        fields.insert("at".to_string(), ts.clone());
        let nested = v(ValueType::ArrayValue(ArrayValue {
            values: vec![
                v(ValueType::MapValue(MapValue { fields })),
                v(ValueType::BooleanValue(true)),
            ],
        }));
        assert_eq!(
            node_json(&nested),
            json!([{"at": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 5_000}}, true])
        );
    }

    #[test]
    fn documents_keep_every_field_and_no_synthetic_ones() {
        let mut fields = HashMap::new();
        fields.insert("level".to_string(), v(ValueType::StringValue("x".into())));
        let doc = FirestoreDocument {
            name: "projects/p/databases/(default)/documents/yape_operations/OP%201".into(),
            fields,
            create_time: Some(Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            update_time: None,
        };
        let (id, data) = node_document(&doc).unwrap();
        assert_eq!(id, "OP%201");
        assert_eq!(Value::Object(data), json!({"level": "x"}));
    }

    #[tokio::test]
    async fn in_memory_source_lists_by_id_and_counts_with_equality() {
        let s = InMemoryNodeDocs::new();
        s.put("c", "b", json!({"g": "1"}));
        s.put("c", "a", json!({"g": "2"}));
        s.put("c", "c", json!({"g": 1}));
        let ids: Vec<String> = s
            .list_all("c")
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.0)
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(s.count("c", None).await, Ok(3));
        assert_eq!(s.count("c", Some(("g", "1"))).await, Ok(1), "solo el texto");
        assert_eq!(s.count("otra", None).await, Ok(0));
        assert_eq!(s.lists_of("c"), 1);
        s.fail_counts(Some(DataError::QuotaExceeded));
        assert_eq!(s.count("c", None).await, Err(DataError::QuotaExceeded));
        assert!(s.list_all("c").await.is_ok());
    }
}
