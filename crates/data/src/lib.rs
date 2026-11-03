//! Crate `data`: acceso a Firestore (lecturas, listados y listeners de colecciones).

mod error;
mod firestore;

pub use crate::error::DataError;
pub use crate::firestore::{document_id, ChangeEvent, FirestoreClient};
