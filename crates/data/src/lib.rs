//! Crate `data`: acceso a Firestore (lecturas, listados y listeners de colecciones).

mod cache;
mod error;
mod firestore;
mod guild_settings;

pub use crate::cache::TtlCache;
pub use crate::error::DataError;
pub use crate::firestore::{document_id, ChangeEvent, FirestoreClient};
pub use crate::guild_settings::{
    FirestoreGuildSettingsRepo, GuildSettings, GuildSettingsRepo, GuildSettingsSource,
    InMemoryGuildSettingsRepo, WatcherHandle, GUILD_SETTINGS_TTL,
};
