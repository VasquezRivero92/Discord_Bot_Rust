//! Operaciones de escritura en Firestore (crear/reemplazar, fusionar campos, borrar documentos y campos).

use crate::error::DataError;
use crate::firestore::FirestoreClient;
use async_trait::async_trait;
use firestore::gcloud_sdk::google::firestore::v1::{
    value::ValueType, ArrayValue as PbArrayValue, Document, MapValue as PbMapValue,
    Value as PbValue,
};
use firestore::gcloud_sdk::prost_types::Timestamp;
use std::collections::HashMap;
use std::sync::Mutex;

/// Construye un centinela JSON `{"$timestamp": "<RFC3339>"}` con la fecha y hora actual en UTC.
pub fn timestamp_now() -> serde_json::Value {
    serde_json::json!({
        "$timestamp": chrono::Utc::now().to_rfc3339()
    })
}

/// Convierte un valor de [`serde_json::Value`] al tipo protobuf nativo de Firestore ([`PbValue`]).
///
/// Codifica tipos estándar de Firestore:
/// - `Null` -> `NullValue(0)`
/// - `Bool` -> `BooleanValue(b)`
/// - `Number` -> `IntegerValue(i)` si cabe en entero, de lo contrario `DoubleValue(f)`
/// - `String` -> `StringValue(s)`
/// - `Array` -> `ArrayValue` recursivo
/// - `Object` -> Si coincide con el centinela `{"$timestamp": "<RFC3339>"}`, se codifica como `TimestampValue`, de lo contrario `MapValue` recursivo
pub fn json_to_firestore_value(val: &serde_json::Value) -> PbValue {
    let value_type = match val {
        serde_json::Value::Null => ValueType::NullValue(0),
        serde_json::Value::Bool(b) => ValueType::BooleanValue(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ValueType::IntegerValue(i)
            } else if let Some(u) = n.as_u64() {
                ValueType::IntegerValue(u as i64)
            } else if let Some(f) = n.as_f64() {
                ValueType::DoubleValue(f)
            } else {
                ValueType::NullValue(0)
            }
        }
        serde_json::Value::String(s) => ValueType::StringValue(s.clone()),
        serde_json::Value::Array(arr) => {
            let values = arr.iter().map(json_to_firestore_value).collect();
            ValueType::ArrayValue(PbArrayValue { values })
        }
        serde_json::Value::Object(map) => {
            if map.len() == 1 {
                if let Some(serde_json::Value::String(ts_str)) = map.get("$timestamp") {
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                        let dt_utc = dt.with_timezone(&chrono::Utc);
                        return PbValue {
                            value_type: Some(ValueType::TimestampValue(Timestamp {
                                seconds: dt_utc.timestamp(),
                                nanos: dt_utc.timestamp_subsec_nanos() as i32,
                            })),
                        };
                    }
                }
            }
            let fields = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_firestore_value(v)))
                .collect();
            ValueType::MapValue(PbMapValue { fields })
        }
    };
    PbValue {
        value_type: Some(value_type),
    }
}

/// Extrae y convierte los campos de un objeto JSON a un mapa de campos de Firestore.
fn json_to_document_fields(val: &serde_json::Value) -> Result<HashMap<String, PbValue>, DataError> {
    match val {
        serde_json::Value::Object(map) => Ok(map
            .iter()
            .map(|(k, v)| (k.clone(), json_to_firestore_value(v)))
            .collect()),
        _ => Err(DataError::Firestore(
            "El documento de Firestore debe ser un objeto JSON".into(),
        )),
    }
}

/// Trait para operaciones de escritura de documentos en Firestore.
#[async_trait]
pub trait DocSink: Send + Sync {
    /// Crea o REEMPLAZA el documento en `collection/id`.
    async fn set_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError>;

    /// Crea o fusiona solo los campos dados (update con máscara de campos; no borra los demás).
    async fn merge_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError>;

    /// Borra el documento en `collection/id`; si no existe, devuelve `Ok(())`.
    async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), DataError>;

    /// Quita solo los campos especificados del documento (equivale a `FieldValue.delete()` de firebase-admin).
    async fn delete_fields(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<(), DataError>;
}

impl FirestoreClient {
    /// Crea o REEMPLAZA el documento en `collection/id`.
    pub async fn set_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError> {
        let doc_name = format!("{}/{}/{}", self.db.get_documents_path(), collection, id);
        let fields = json_to_document_fields(val)?;
        let doc = Document {
            name: doc_name,
            fields,
            ..Default::default()
        };
        self.db
            .fluent()
            .update()
            .in_col(collection)
            .document(doc)
            .execute()
            .await
            .map_err(DataError::from)?;
        Ok(())
    }

    /// Crea o fusiona solo los campos dados (update con máscara de campos; no borra los demás).
    pub async fn merge_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError> {
        let map = match val {
            serde_json::Value::Object(map) => map,
            _ => {
                return Err(DataError::Firestore(
                    "El documento de Firestore debe ser un objeto JSON".into(),
                ))
            }
        };
        if map.is_empty() {
            return Ok(());
        }
        let keys: Vec<String> = map.keys().cloned().collect();
        let fields = map
            .iter()
            .map(|(k, v)| (k.clone(), json_to_firestore_value(v)))
            .collect();
        let doc_name = format!("{}/{}/{}", self.db.get_documents_path(), collection, id);
        let doc = Document {
            name: doc_name,
            fields,
            ..Default::default()
        };
        self.db
            .fluent()
            .update()
            .fields(&keys)
            .in_col(collection)
            .document(doc)
            .execute()
            .await
            .map_err(DataError::from)?;
        Ok(())
    }

    /// Borra el documento en `collection/id`; si no existe, devuelve `Ok(())`.
    pub async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), DataError> {
        let res = self
            .db
            .fluent()
            .delete()
            .from(collection)
            .document_id(id)
            .execute()
            .await;
        match res {
            Ok(()) => Ok(()),
            Err(err) => match DataError::from(err) {
                DataError::NotFound => Ok(()),
                other => Err(other),
            },
        }
    }

    /// Quita solo los campos especificados del documento (equivale a `FieldValue.delete()` de firebase-admin).
    pub async fn delete_fields(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<(), DataError> {
        if fields.is_empty() {
            return Ok(());
        }
        let doc_name = format!("{}/{}/{}", self.db.get_documents_path(), collection, id);
        let doc = Document {
            name: doc_name,
            fields: HashMap::new(),
            ..Default::default()
        };
        let res = self
            .db
            .fluent()
            .update()
            .fields(fields)
            .in_col(collection)
            .document(doc)
            .execute()
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(err) => match DataError::from(err) {
                DataError::NotFound => Ok(()),
                other => Err(other),
            },
        }
    }
}

#[async_trait]
impl DocSink for FirestoreClient {
    async fn set_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError> {
        self.set_doc(collection, id, val).await
    }

    async fn merge_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError> {
        self.merge_doc(collection, id, val).await
    }

    async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), DataError> {
        self.delete_doc(collection, id).await
    }

    async fn delete_fields(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<(), DataError> {
        self.delete_fields(collection, id, fields).await
    }
}

/// Doble en memoria de [`DocSink`] para pruebas unitarias sin red.
#[derive(Default)]
pub struct FakeDocSink {
    docs: Mutex<HashMap<(String, String), serde_json::Value>>,
    error: Mutex<Option<DataError>>,
}

impl FakeDocSink {
    /// Crea un nuevo sink falso vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserta o reemplaza directamente un documento en memoria.
    pub fn insert(&self, collection: &str, id: &str, val: serde_json::Value) {
        self.docs
            .lock()
            .expect("mutex envenenado")
            .insert((collection.to_string(), id.to_string()), val);
    }

    /// Obtiene una copia del documento almacenado en memoria si existe.
    pub fn get(&self, collection: &str, id: &str) -> Option<serde_json::Value> {
        self.docs
            .lock()
            .expect("mutex envenenado")
            .get(&(collection.to_string(), id.to_string()))
            .cloned()
    }

    /// Configura un error artificial para simular fallos de Firestore.
    pub fn set_error(&self, err: Option<DataError>) {
        *self.error.lock().expect("mutex envenenado") = err;
    }

    fn check_error(&self) -> Result<(), DataError> {
        if let Some(err) = self.error.lock().expect("mutex envenenado").clone() {
            return Err(err);
        }
        Ok(())
    }
}

#[async_trait]
impl DocSink for FakeDocSink {
    async fn set_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError> {
        self.check_error()?;
        if !val.is_object() {
            return Err(DataError::Firestore(
                "El documento de Firestore debe ser un objeto JSON".into(),
            ));
        }
        self.docs
            .lock()
            .expect("mutex envenenado")
            .insert((collection.to_string(), id.to_string()), val.clone());
        Ok(())
    }

    async fn merge_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), DataError> {
        self.check_error()?;
        let map = match val {
            serde_json::Value::Object(m) => m,
            _ => {
                return Err(DataError::Firestore(
                    "El documento de Firestore debe ser un objeto JSON".into(),
                ))
            }
        };
        let mut docs = self.docs.lock().expect("mutex envenenado");
        let key = (collection.to_string(), id.to_string());
        let current = docs.entry(key).or_insert_with(|| serde_json::json!({}));
        if let serde_json::Value::Object(curr_map) = current {
            for (k, v) in map {
                curr_map.insert(k.clone(), v.clone());
            }
        }
        Ok(())
    }

    async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), DataError> {
        self.check_error()?;
        self.docs
            .lock()
            .expect("mutex envenenado")
            .remove(&(collection.to_string(), id.to_string()));
        Ok(())
    }

    async fn delete_fields(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<(), DataError> {
        self.check_error()?;
        if fields.is_empty() {
            return Ok(());
        }
        let mut docs = self.docs.lock().expect("mutex envenenado");
        let key = (collection.to_string(), id.to_string());
        if let Some(serde_json::Value::Object(map)) = docs.get_mut(&key) {
            for field in fields {
                map.remove(*field);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firestore::decode_document;
    use crate::source::strip_synthetic;
    use serde_json::json;

    #[test]
    fn test_json_to_firestore_value_all_types() {
        let val = json!({
            "str": "hola mundo",
            "int": 42,
            "neg_int": -10,
            "float": 3.14159,
            "bool_t": true,
            "bool_f": false,
            "nil": null,
            "arr": [1, "dos", false, null],
            "nested": {
                "sub": "valor"
            }
        });

        let pb = json_to_firestore_value(&val);
        match pb.value_type {
            Some(ValueType::MapValue(mv)) => {
                assert!(mv.fields.contains_key("str"));
                assert!(mv.fields.contains_key("int"));
                assert!(mv.fields.contains_key("float"));
                assert!(mv.fields.contains_key("nil"));
                assert!(mv.fields.contains_key("arr"));
                assert!(mv.fields.contains_key("nested"));
            }
            _ => panic!("Esperaba MapValue para un objeto JSON"),
        }
    }

    #[test]
    fn test_roundtrip_with_decode_document() {
        let original = json!({
            "faceit_nickname": "player1",
            "faceit_elo": 1500,
            "is_active": true,
            "stats": {
                "kd": 1.25,
                "matches": 50
            },
            "tags": ["pro", "cs2"]
        });

        let fields = json_to_document_fields(&original).unwrap();
        let doc = Document {
            name: "projects/test/databases/(default)/documents/faceit_users/u123".into(),
            fields,
            create_time: None,
            update_time: None,
        };

        let (id, decoded): (String, serde_json::Value) = decode_document(&doc).unwrap();
        assert_eq!(id, "u123");
        let clean = strip_synthetic(decoded);
        assert_eq!(clean, original);
    }

    #[test]
    fn test_timestamp_sentinel_roundtrip() {
        let now_val = timestamp_now();
        let pb = json_to_firestore_value(&now_val);
        match &pb.value_type {
            Some(ValueType::TimestampValue(ts)) => {
                assert!(ts.seconds > 0);
            }
            other => panic!("Esperaba TimestampValue, se obtuvo {:?}", other),
        }

        let original = json!({
            "faceit_nickname": "player_ts",
            "updatedAt": now_val
        });

        let fields = json_to_document_fields(&original).unwrap();
        let doc = Document {
            name: "projects/test/databases/(default)/documents/faceit_users/u_ts".into(),
            fields,
            create_time: None,
            update_time: None,
        };

        let (id, decoded): (String, serde_json::Value) = decode_document(&doc).unwrap();
        assert_eq!(id, "u_ts");
        let updated_at_str = decoded["updatedAt"]
            .as_str()
            .expect("TimestampValue debe decodificarse como texto RFC3339");
        let parsed = chrono::DateTime::parse_from_rfc3339(updated_at_str).expect("RFC3339 valido");
        let orig_str = now_val["$timestamp"].as_str().unwrap();
        let orig_parsed = chrono::DateTime::parse_from_rfc3339(orig_str).unwrap();
        assert_eq!(parsed.timestamp(), orig_parsed.timestamp());
        assert_eq!(
            parsed.timestamp_subsec_nanos(),
            orig_parsed.timestamp_subsec_nanos()
        );
    }

    #[tokio::test]
    async fn test_fake_doc_sink_lifecycle() {
        let sink = FakeDocSink::new();

        // 1. set_doc
        let user = json!({
            "faceit_nickname": "rush_b",
            "faceit_elo": 1200,
            "wallet_balance": 10.0
        });
        sink.set_doc("faceit_users", "user_1", &user)
            .await
            .expect("set_doc");
        assert_eq!(sink.get("faceit_users", "user_1"), Some(user.clone()));

        // 2. set_doc reemplaza completamente
        let replacement = json!({
            "faceit_nickname": "rush_a"
        });
        sink.set_doc("faceit_users", "user_1", &replacement)
            .await
            .expect("set_doc replace");
        assert_eq!(
            sink.get("faceit_users", "user_1"),
            Some(json!({ "faceit_nickname": "rush_a" }))
        );

        // 3. merge_doc fusiona solo los campos indicados
        let patch = json!({
            "faceit_elo": 1350,
            "verified": true
        });
        sink.merge_doc("faceit_users", "user_1", &patch)
            .await
            .expect("merge_doc");
        let merged = sink.get("faceit_users", "user_1").unwrap();
        assert_eq!(merged["faceit_nickname"], "rush_a");
        assert_eq!(merged["faceit_elo"], 1350);
        assert_eq!(merged["verified"], true);

        // 4. delete_fields quita solo esos campos
        sink.delete_fields("faceit_users", "user_1", &["faceit_elo", "verified"])
            .await
            .expect("delete_fields");
        let remaining = sink.get("faceit_users", "user_1").unwrap();
        assert_eq!(remaining, json!({ "faceit_nickname": "rush_a" }));

        // 5. delete_doc borra el documento
        sink.delete_doc("faceit_users", "user_1")
            .await
            .expect("delete_doc");
        assert_eq!(sink.get("faceit_users", "user_1"), None);

        // Borrar documento inexistente devuelve Ok
        sink.delete_doc("faceit_users", "user_nonexistent")
            .await
            .expect("delete non-existent doc should be Ok");
    }

    #[tokio::test]
    async fn test_fake_doc_sink_error_propagation() {
        let sink = FakeDocSink::new();
        sink.set_error(Some(DataError::QuotaExceeded));

        let res = sink.set_doc("faceit_users", "1", &json!({"a": 1})).await;
        assert_eq!(res, Err(DataError::QuotaExceeded));

        let res = sink.merge_doc("faceit_users", "1", &json!({"a": 1})).await;
        assert_eq!(res, Err(DataError::QuotaExceeded));

        let res = sink.delete_doc("faceit_users", "1").await;
        assert_eq!(res, Err(DataError::QuotaExceeded));

        let res = sink.delete_fields("faceit_users", "1", &["a"]).await;
        assert_eq!(res, Err(DataError::QuotaExceeded));
    }
}
