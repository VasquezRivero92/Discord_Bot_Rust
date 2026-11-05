//! Crate `data`: acceso a Firestore (lecturas, listados y listeners de colecciones).

mod admin_docs;
mod cache;
mod claims;
mod error;
mod firestore;
mod global_settings;
mod guild_settings;
mod logs;
mod match_history;
mod messages;
mod model;
mod node_docs;
mod orders;
mod source;
mod support_calls;
mod users;
mod watch;
mod write;

pub use crate::admin_docs::{AdminDocsRepo, FirestoreAdminDocsRepo, StatsTotals};
pub use crate::cache::TtlCache;
pub use crate::claims::{Claim, ClaimsRepo, FirestoreClaimsRepo, InMemoryClaimsRepo};
pub use crate::error::DataError;
pub use crate::firestore::{document_id, ChangeEvent, Dir, FirestoreClient, Op, Query, Value};
pub use crate::global_settings::{
    effective_config_for_guild, global_admin_role, is_default_guild, js_parse_int, EffectiveConfig,
    FirestoreGlobalSettingsRepo, GlobalSettings, GlobalSettingsRepo, GlobalSettingsSource,
    InMemoryGlobalSettingsRepo, GLOBAL_SETTINGS_TTL,
};
pub use crate::guild_settings::{
    FirestoreGuildSettingsRepo, GuildSettings, GuildSettingsRepo, GuildSettingsSource,
    InMemoryGuildSettingsRepo, GUILD_SETTINGS_TTL,
};
pub use crate::logs::{FakeLogsRepo, FirestoreLogsRepo, LogEntry, LogsRepo};
pub use crate::match_history::{
    FakeMatchHistoryRepo, FirestoreMatchHistoryRepo, MatchHistoryEntry, MatchHistoryRepo,
};
pub use crate::messages::{
    default_message_templates, FakeMessagesRepo, FirestoreMessagesRepo, MessagesRepo,
};
pub use crate::node_docs::{InMemoryNodeDocs, NodeDocSource};
pub use crate::orders::{FakeOrdersRepo, FirestoreOrdersRepo, Order, OrdersRepo};
pub use crate::source::DocSource;
pub use crate::support_calls::{
    FakeSupportCallsRepo, FirestoreSupportCallsRepo, SupportCall, SupportCallsRepo,
};
pub use crate::users::{
    FaceitUser, FakeUsersRepo, FirestoreUsersRepo, UsersRepo, USERS_ALL_TTL, USER_TTL,
};
pub use crate::watch::WatcherHandle;
pub use crate::write::{json_to_firestore_value, timestamp_now, DocSink, FakeDocSink};
