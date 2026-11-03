//! Errores de la capa de datos.

use firestore::errors::FirestoreError;
use firestore::gcloud_sdk::tonic::Status;

/// Error de la capa de datos.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataError {
    /// Cualquier otro error de Firestore o de la conexión, con su mensaje.
    #[error("error de Firestore: {0}")]
    Firestore(String),
    /// Firestore respondió `RESOURCE_EXHAUSTED` (cuota diaria o límite de tasa agotados).
    #[error("cuota de Firestore agotada")]
    QuotaExceeded,
    /// Firestore respondió `NOT_FOUND`.
    #[error("no encontrado en Firestore")]
    NotFound,
    /// El documento existe pero no se pudo decodificar al tipo pedido.
    #[error("no se pudo decodificar el documento: {0}")]
    Decode(String),
}

/// Código que el crate `firestore` guarda en `public.code` para un `RESOURCE_EXHAUSTED` de gRPC
/// (es el `Debug` de `tonic::Code::ResourceExhausted`).
const RESOURCE_EXHAUSTED_CODE: &str = "ResourceExhausted";

impl From<FirestoreError> for DataError {
    fn from(err: FirestoreError) -> Self {
        match err {
            FirestoreError::DatabaseError(ref db_err)
                if db_err.public.code == RESOURCE_EXHAUSTED_CODE =>
            {
                DataError::QuotaExceeded
            }
            FirestoreError::DataNotFoundError(_) => DataError::NotFound,
            FirestoreError::DeserializeError(ref ser_err) => {
                DataError::Decode(ser_err.message.clone())
            }
            other => DataError::Firestore(other.to_string()),
        }
    }
}

/// Mismo mapeo que aplica el crate `firestore` a un `tonic::Status`, para que una prueba (o un
/// doble de la Tarea 11) pueda fabricar errores realistas sin red.
impl From<Status> for DataError {
    fn from(status: Status) -> Self {
        DataError::from(FirestoreError::from(status))
    }
}
