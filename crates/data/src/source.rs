//! Origen de documentos de los repositorios de lectura: una costura entre los repos y
//! [`FirestoreClient`] para poder probar su lógica (consulta que construyen, respaldos, orden en
//! memoria, propagación de errores) con un origen falso y sin red.

use crate::error::DataError;
use crate::firestore::{FirestoreClient, Query};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Campos sintéticos que el deserializador del crate `firestore` añade a todo documento
/// (`firestore_document_to_serializable`). No están en Firestore ni en lo que devuelve el legacy:
/// si no se quitan acaban en el `extra` de los modelos y salen en el JSON (el `_full_id` lleva
/// además la ruta completa con el id del proyecto).
const SYNTHETIC_FIELDS: [&str; 4] = [
    "_firestore_id",
    "_firestore_full_id",
    "_firestore_created",
    "_firestore_updated",
];

/// Quita de `doc` los campos sintéticos del crate (ver [`SYNTHETIC_FIELDS`]).
pub(crate) fn strip_synthetic(mut doc: Value) -> Value {
    if let Value::Object(map) = &mut doc {
        for field in SYNTHETIC_FIELDS {
            map.remove(field);
        }
    }
    doc
}

/// Decodifica un documento ya en JSON al modelo `T`.
pub(crate) fn from_json<T: DeserializeOwned>(id: &str, doc: Value) -> Result<T, DataError> {
    serde_json::from_value(doc).map_err(|err| DataError::Decode(format!("{id}: {err}")))
}

/// Lecturas de documentos como JSON (sin los campos sintéticos del crate). Los `Timestamp` llegan
/// como texto RFC 3339, igual que en [`FirestoreClient`].
#[async_trait]
pub trait DocSource: Send + Sync {
    /// [`FirestoreClient::query`] como `(id, documento)`.
    async fn query(&self, collection: &str, q: Query) -> Result<Vec<(String, Value)>, DataError>;
    /// [`FirestoreClient::get_doc`]: `Ok(None)` si el documento no existe.
    async fn get(&self, collection: &str, id: &str) -> Result<Option<Value>, DataError>;
}

#[async_trait]
impl DocSource for FirestoreClient {
    async fn query(&self, collection: &str, q: Query) -> Result<Vec<(String, Value)>, DataError> {
        let docs: Vec<(String, Value)> = FirestoreClient::query(self, collection, q).await?;
        Ok(docs
            .into_iter()
            .map(|(id, doc)| (id, strip_synthetic(doc)))
            .collect())
    }

    async fn get(&self, collection: &str, id: &str) -> Result<Option<Value>, DataError> {
        let doc: Option<Value> = self.get_doc(collection, id).await?;
        Ok(doc.map(strip_synthetic))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! Origen falso: devuelve respuestas encoladas y apunta cada consulta recibida.

    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    type Reply = Result<Vec<(String, Value)>, DataError>;
    /// `(colección, id, respuesta de get)`.
    type DocReply = (String, String, Result<Option<Value>, DataError>);

    #[derive(Default)]
    pub(crate) struct FakeSource {
        replies: Mutex<VecDeque<Reply>>,
        queries: Mutex<Vec<(String, Query)>>,
        docs: Mutex<Vec<DocReply>>,
    }

    impl FakeSource {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Encola la respuesta de la próxima consulta.
        pub(crate) fn reply(self, reply: Reply) -> Self {
            self.replies.lock().unwrap().push_back(reply);
            self
        }

        /// Respuesta de `get(collection, id)`.
        pub(crate) fn doc(
            self,
            collection: &str,
            id: &str,
            reply: Result<Option<Value>, DataError>,
        ) -> Self {
            self.docs
                .lock()
                .unwrap()
                .push((collection.into(), id.into(), reply));
            self
        }

        /// Consultas recibidas, en orden.
        pub(crate) fn queries(&self) -> Vec<(String, Query)> {
            self.queries.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DocSource for FakeSource {
        async fn query(
            &self,
            collection: &str,
            q: Query,
        ) -> Result<Vec<(String, Value)>, DataError> {
            self.queries.lock().unwrap().push((collection.into(), q));
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("consulta inesperada: no hay respuesta encolada")
        }

        async fn get(&self, collection: &str, id: &str) -> Result<Option<Value>, DataError> {
            self.docs
                .lock()
                .unwrap()
                .iter()
                .find(|(c, i, _)| c == collection && i == id)
                .map(|(_, _, reply)| reply.clone())
                .unwrap_or(Ok(None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firestore::decode_document;
    use firestore::gcloud_sdk::google::firestore::v1::value::ValueType;
    use firestore::gcloud_sdk::google::firestore::v1::{Document, Value as FsValue};
    use firestore::gcloud_sdk::prost_types::Timestamp;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn decoded_documents_lose_the_crate_synthetic_fields() {
        let mut fields = HashMap::new();
        fields.insert(
            "level".to_string(),
            FsValue {
                value_type: Some(ValueType::StringValue("error".into())),
            },
        );
        let doc = Document {
            name: "projects/p/databases/(default)/documents/faceit_bot_logs/a1".into(),
            fields,
            create_time: Some(Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            update_time: Some(Timestamp {
                seconds: 2,
                nanos: 0,
            }),
        };
        let (id, raw): (String, Value) = decode_document(&doc).unwrap();
        assert_eq!(id, "a1");
        // El crate sí los añade...
        assert!(raw.get("_firestore_full_id").is_some(), "{raw}");
        // ...y aquí se quitan.
        assert_eq!(strip_synthetic(raw), json!({"level": "error"}));
    }
}
