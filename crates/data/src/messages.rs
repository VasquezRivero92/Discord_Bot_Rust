//! Plantillas de mensajes (documento `faceit_config/messages`): repositorio de solo lectura.
//!
//! Lo escribe `setMessageTemplates` (`database.js` ~2207: `{ ...templates, updatedAt:
//! serverTimestamp() }`, reemplazando el documento) y lo lee `getMessageTemplates` (~2192) para
//! `GET /api/messages` (`routes/api.js` ~1825), que responde `{ templates }`.
//!
//! Lectura del legacy: si el documento existe, `doc.data()` tal cual; si no existe,
//! `getDefaultMessageTemplates()` (~3424). Ante un error, la caché previa o, sin ella, las mismas
//! plantillas por defecto. Hay además una caché de 6 h que aquí no existe.

use crate::error::DataError;
use crate::firestore::FirestoreClient;
use crate::source::DocSource;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Mutex;

const COLLECTION: &str = "faceit_config";
const DOC_ID: &str = "messages";

/// Campos de fecha conocidos del documento (`serverTimestamp()` de `setMessageTemplates`).
const TIMESTAMP_FIELDS: [&str; 1] = ["updatedAt"];

/// `getDefaultMessageTemplates()` del legacy. El JSON se generó evaluando esa función con Node,
/// así que el texto (emojis y saltos de línea incluidos) es idéntico.
const DEFAULT_TEMPLATES_JSON: &str = include_str!("default_message_templates.json");

/// Plantillas por defecto del legacy (`getDefaultMessageTemplates`).
pub fn default_message_templates() -> Value {
    serde_json::from_str(DEFAULT_TEMPLATES_JSON).expect("JSON de plantillas por defecto válido")
}

/// Convierte las fechas conocidas del documento al formato de Node (`{ _seconds, _nanoseconds }`).
/// El resto del documento, que son textos libres de las plantillas, no se toca.
fn normalize(mut doc: Value) -> Value {
    if let Value::Object(map) = &mut doc {
        for field in TIMESTAMP_FIELDS {
            if let Some(Value::String(s)) = map.get(field) {
                if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
                    map.insert(
                        field.to_string(),
                        serde_json::json!({
                            "_seconds": t.timestamp(),
                            "_nanoseconds": t.timestamp_subsec_nanos(),
                        }),
                    );
                }
            }
        }
    }
    doc
}

/// Lectura de las plantillas de mensajes.
///
/// **Errores:** se devuelven tal cual (`DataError::QuotaExceeded` incluido). El legacy no falla
/// nunca: ante un error responde con su caché o con [`default_message_templates`]; la ruta
/// (Tarea 7) debe hacer eso mismo ante un `Err`.
#[async_trait]
pub trait MessagesRepo: Send + Sync {
    /// El documento de plantillas, o las plantillas por defecto si no existe.
    async fn templates(&self) -> Result<Value, DataError>;
}

/// Repo sobre Firestore (o sobre cualquier [`DocSource`]), sin caché.
pub struct FirestoreMessagesRepo<S = FirestoreClient> {
    source: S,
}

impl<S: DocSource> FirestoreMessagesRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

#[async_trait]
impl<S: DocSource> MessagesRepo for FirestoreMessagesRepo<S> {
    async fn templates(&self) -> Result<Value, DataError> {
        Ok(match self.source.get(COLLECTION, DOC_ID).await? {
            Some(doc) => normalize(doc),
            None => default_message_templates(),
        })
    }
}

/// Doble en memoria para pruebas de otros crates. Sin documento devuelve las plantillas por
/// defecto, como el repo real.
#[derive(Default)]
pub struct FakeMessagesRepo {
    doc: Mutex<Option<Value>>,
    error: Mutex<Option<DataError>>,
}

impl FakeMessagesRepo {
    /// Repo sin documento.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pone (o quita, con `None`) el documento de plantillas.
    pub fn set(&self, doc: Option<Value>) {
        *self.doc.lock().expect("mutex envenenado") = doc;
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        *self.error.lock().expect("mutex envenenado") = error;
    }
}

#[async_trait]
impl MessagesRepo for FakeMessagesRepo {
    async fn templates(&self) -> Result<Value, DataError> {
        if let Some(err) = self.error.lock().expect("mutex envenenado").clone() {
            return Err(err);
        }
        Ok(self
            .doc
            .lock()
            .expect("mutex envenenado")
            .clone()
            .unwrap_or_else(default_message_templates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::FakeSource;
    use serde_json::json;

    #[tokio::test]
    async fn existing_document_is_returned_with_node_timestamps() {
        let source = FakeSource::new().doc(
            "faceit_config",
            "messages",
            Ok(Some(json!({
                "welcomeTitle": "Hola",
                "welcomeDescription": "2026-11-02T10:00:00Z",
                "updatedAt": "2026-11-02T10:00:00.25Z"
            }))),
        );
        let templates = FirestoreMessagesRepo::new(source)
            .templates()
            .await
            .unwrap();
        assert_eq!(
            templates,
            json!({
                "welcomeTitle": "Hola",
                // Un texto de plantilla con forma de fecha no se toca.
                "welcomeDescription": "2026-11-02T10:00:00Z",
                "updatedAt": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 250_000_000}
            })
        );
    }

    #[tokio::test]
    async fn missing_document_gives_legacy_defaults() {
        let templates = FirestoreMessagesRepo::new(FakeSource::new())
            .templates()
            .await
            .unwrap();
        assert_eq!(templates, default_message_templates());
        assert_eq!(
            templates["welcomeTitle"],
            json!("¡Bienvenido al Servidor! 🎉")
        );
        assert_eq!(templates["supportColor"], json!("#FF0000"));
        assert_eq!(templates.as_object().unwrap().len(), 6);
        assert!(templates["welcomeDescription"]
            .as_str()
            .unwrap()
            .contains("➡️ `/verificar_fc`\n\nEl bot"));
    }

    #[tokio::test]
    async fn list_propagates_quota_error() {
        let source =
            FakeSource::new().doc("faceit_config", "messages", Err(DataError::QuotaExceeded));
        assert_eq!(
            FirestoreMessagesRepo::new(source).templates().await,
            Err(DataError::QuotaExceeded)
        );
        let fake = FakeMessagesRepo::new();
        assert_eq!(fake.templates().await.unwrap(), default_message_templates());
        fake.fail_with(Some(DataError::QuotaExceeded));
        assert_eq!(fake.templates().await, Err(DataError::QuotaExceeded));
    }
}
