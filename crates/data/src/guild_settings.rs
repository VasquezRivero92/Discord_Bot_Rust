//! Ajustes por servidor (colección `guild_settings`): modelo, repositorio con caché y watcher.
//!
//! Los nombres de campo son los del documento que escribe/lee el bot Node (`database.js`,
//! `setGuildSettings`/`getGuildSettings`; `config/index.js`, `getEffectiveConfigForGuild`;
//! `routes/api.js`, `GET/POST /guilds/:guildId/settings`). Todo es opcional y tolerante: un
//! campo con un tipo inesperado se lee como ausente, y los campos desconocidos se conservan en
//! [`GuildSettings::extra`].

use crate::cache::TtlCache;
use crate::error::DataError;
use crate::firestore::{ChangeEvent, FirestoreClient};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Vida de una entrada de `guild_settings` en caché (`GUILD_SETTINGS_TTL` de `database.js`).
pub const GUILD_SETTINGS_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const COLLECTION: &str = "guild_settings";
const MAX_GUILDS: u64 = 10_000;
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Un stream que aguanta más que esto se considera sano: la espera vuelve al mínimo.
const STABLE_AFTER: Duration = Duration::from_secs(60);

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

fn lenient_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .and_then(|v| u32::try_from(v).ok()),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    })
}

/// Documento de `guild_settings/{guildId}`. El nombre legado de cada campo es el de la
/// documentación (camelCase).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildSettings {
    /// `discordLinkChannelId`: canal de vinculación de cuentas.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_link_channel_id: Option<String>,
    /// `discordMatchesChannelId`: canal de partidas.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_matches_channel_id: Option<String>,
    /// `discordMixtonChannelId`: canal de Mixton.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_mixton_channel_id: Option<String>,
    /// `discordRolesChannelId`: canal de roles.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_roles_channel_id: Option<String>,
    /// `discordStatusChannelId`: canal de estado.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_status_channel_id: Option<String>,
    /// `discordAdminAlertsChannelId`: canal de alertas de administración.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_admin_alerts_channel_id: Option<String>,
    /// `discordWaitingRoomChannelId`: sala de espera.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_waiting_room_channel_id: Option<String>,
    /// `discordSanctionsChannelId`: canal de sanciones.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_sanctions_channel_id: Option<String>,
    /// `discordSupportChannelId`: canal de soporte.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_support_channel_id: Option<String>,
    /// `discordMusicChannelId`: canal de música.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_music_channel_id: Option<String>,
    /// `discordRechargeChannelId`: canal de recargas.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_recharge_channel_id: Option<String>,
    /// `discordMixtonChatChannelId`: chat de Mixton (el legado cae a `discordMixtonChannelId`).
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_mixton_chat_channel_id: Option<String>,
    /// `discordMatchAccessRoleId`: rol con acceso a partidas.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_match_access_role_id: Option<String>,
    /// `hubRoleId`: rol del hub.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub hub_role_id: Option<String>,
    /// `discordHubRoleId`: rol del hub (nombre alternativo).
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_hub_role_id: Option<String>,
    /// `faceitClubId`: club de FACEIT.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub faceit_club_id: Option<String>,
    /// `faceitHubId`: hub de FACEIT.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub faceit_hub_id: Option<String>,
    /// `faceitLeaderboardId`: leaderboard de FACEIT.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub faceit_leaderboard_id: Option<String>,
    /// `adminRoleId`: rol de administración.
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub admin_role_id: Option<String>,
    /// `adminRoleName`: nombre del rol de administración (el legado lo usa si falta el id).
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub admin_role_name: Option<String>,
    /// `matchTimeoutMinutes`: minutos de espera de partida (puede venir como texto).
    #[serde(
        default,
        deserialize_with = "lenient_u32",
        skip_serializing_if = "Option::is_none"
    )]
    pub match_timeout_minutes: Option<u32>,
    /// `gamesPerMatch`: mapas por partida (el panel valida 1, 3 o 5).
    #[serde(
        default,
        deserialize_with = "lenient_u32",
        skip_serializing_if = "Option::is_none"
    )]
    pub games_per_match: Option<u32>,
    /// `roundsPerGame`: alias legado de `gamesPerMatch` (el panel escribe ambos).
    #[serde(
        default,
        deserialize_with = "lenient_u32",
        skip_serializing_if = "Option::is_none"
    )]
    pub rounds_per_game: Option<u32>,
    /// `queues`: colas del servidor (estructura libre, se conserva tal cual).
    #[serde(
        default,
        deserialize_with = "lenient_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub queues: Vec<Value>,
    /// `updatedAt`: `serverTimestamp()` de la última escritura (texto RFC 3339 al decodificar).
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<String>,
    /// Cualquier otro campo del documento, sin perder nada.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Repositorio de ajustes por servidor.
#[async_trait]
pub trait GuildSettingsRepo: Send + Sync {
    /// Ajustes del servidor; `Ok(None)` si el documento no existe.
    async fn get(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError>;
}

/// Fuente de datos del repo con caché (costura para pruebas: Firestore real o un doble).
#[async_trait]
pub trait GuildSettingsSource: Send + Sync + 'static {
    /// Lee el documento del servidor.
    async fn fetch(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError>;
    /// Abre un stream de cambios de la colección (ver `FirestoreClient::watch_collection`).
    fn watch(&self) -> BoxStream<'static, Result<ChangeEvent, DataError>>;
}

#[async_trait]
impl GuildSettingsSource for FirestoreClient {
    async fn fetch(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError> {
        self.get_doc(COLLECTION, guild_id).await
    }

    fn watch(&self) -> BoxStream<'static, Result<ChangeEvent, DataError>> {
        self.watch_collection(COLLECTION).boxed()
    }
}

/// Tarea del watcher; soltarla (o `abort`) la detiene.
pub struct WatcherHandle {
    task: JoinHandle<()>,
}

impl WatcherHandle {
    /// Detiene el watcher.
    pub fn abort(&self) {
        self.task.abort();
    }

    /// `true` si la tarea ya terminó.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Repo con caché TTL sobre una fuente (Firestore en producción).
///
/// Un `None` (documento ausente) también se cachea. Si la recarga falla con
/// [`DataError::QuotaExceeded`] y hay un valor anterior, se sirve ese (como el legado); cualquier
/// otro error se propaga.
pub struct FirestoreGuildSettingsRepo<S: GuildSettingsSource> {
    source: Arc<S>,
    cache: TtlCache<String, Option<GuildSettings>>,
    restarts: AtomicU64,
}

impl FirestoreGuildSettingsRepo<FirestoreClient> {
    /// Repo sobre un cliente Firestore real.
    pub fn from_client(client: FirestoreClient, ttl: Duration) -> Self {
        Self::new(client, ttl)
    }
}

impl<S: GuildSettingsSource> FirestoreGuildSettingsRepo<S> {
    /// Repo sobre `source` con la vida de caché `ttl` (producción: [`GUILD_SETTINGS_TTL`]).
    pub fn new(source: S, ttl: Duration) -> Self {
        Self {
            source: Arc::new(source),
            cache: TtlCache::new(ttl, MAX_GUILDS),
            restarts: AtomicU64::new(0),
        }
    }

    /// Descarta la entrada de un servidor (p. ej. tras escribir sus ajustes).
    pub async fn invalidate(&self, guild_id: &str) {
        self.cache.invalidate(&guild_id.to_string()).await;
    }

    /// Descarta la entrada y la copia vieja de un servidor (p. ej. si se borró).
    pub async fn purge(&self, guild_id: &str) {
        self.cache.purge(&guild_id.to_string()).await;
    }

    /// Descarta todas las entradas.
    pub fn invalidate_all(&self) {
        self.cache.invalidate_all();
    }

    /// Veces que el watcher se ha reabierto tras terminar su stream.
    pub fn watcher_restarts(&self) -> u64 {
        self.restarts.load(Ordering::Relaxed)
    }

    /// Lanza la tarea que mantiene la caché al día con los cambios de la colección.
    ///
    /// `Added`/`Modified` invalidan la clave (conservan la copia vieja para la cuota); `Removed`
    /// la purga (entrada y copia vieja); `Reset` y cualquier error invalidan todo (`Reset` es
    /// idempotente y puede llegar varias veces). Al abrir (y reabrir) el stream se invalida todo,
    /// y otra vez con el `Reset` que emite el listener cuando su instantánea inicial es actual
    /// (cubre lo cacheado y borrado entre la apertura y ese momento). Cuando el stream termina
    /// (error o fin) se invalida todo y se reabre con espera exponencial (1 s, el doble cada vez,
    /// tope 60 s; vuelve a 1 s tras un stream que duró más de 60 s). El TTL sigue siendo la única
    /// cota si el listener cae sin avisar.
    pub fn spawn_watcher(self: Arc<Self>) -> WatcherHandle {
        let task = tokio::spawn(async move { self.watch_loop().await });
        WatcherHandle { task }
    }

    async fn watch_loop(&self) {
        let mut backoff = BACKOFF_MIN;
        let mut first = true;
        loop {
            if !first {
                self.restarts.fetch_add(1, Ordering::Relaxed);
            }
            first = false;
            let opened = Instant::now();
            let mut stream = self.source.watch();
            // Al (re)abrir también se invalida todo: un borrado durante la espera no emite evento.
            self.cache.invalidate_all();
            while let Some(item) = stream.next().await {
                match item {
                    Ok(ChangeEvent::Added(id)) | Ok(ChangeEvent::Modified(id)) => {
                        self.invalidate(&id).await
                    }
                    // Borrado: se descarta también la copia vieja (no servirla ante cuota).
                    Ok(ChangeEvent::Removed(id)) => self.cache.purge(&id).await,
                    Ok(ChangeEvent::Reset) => self.cache.invalidate_all(),
                    Err(_) => {
                        self.cache.invalidate_all();
                        break;
                    }
                }
            }
            drop(stream);
            self.cache.invalidate_all();
            if opened.elapsed() > STABLE_AFTER {
                backoff = BACKOFF_MIN;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

#[async_trait]
impl<S: GuildSettingsSource> GuildSettingsRepo for FirestoreGuildSettingsRepo<S> {
    async fn get(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError> {
        let source = self.source.clone();
        let id = guild_id.to_string();
        let load_id = id.clone();
        self.cache
            .get_or_load_stale_on(
                id,
                move || async move { source.fetch(&load_id).await },
                |err| matches!(err, DataError::QuotaExceeded),
            )
            .await
    }
}

/// Repo en memoria para pruebas de otros crates.
#[derive(Default)]
pub struct InMemoryGuildSettingsRepo {
    docs: Mutex<HashMap<String, GuildSettings>>,
}

impl InMemoryGuildSettingsRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Crea o reemplaza los ajustes de un servidor.
    pub fn insert(&self, guild_id: &str, settings: GuildSettings) {
        self.docs
            .lock()
            .expect("mutex envenenado")
            .insert(guild_id.to_string(), settings);
    }

    /// Borra los ajustes de un servidor.
    pub fn remove(&self, guild_id: &str) {
        self.docs.lock().expect("mutex envenenado").remove(guild_id);
    }
}

#[async_trait]
impl GuildSettingsRepo for InMemoryGuildSettingsRepo {
    async fn get(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError> {
        Ok(self
            .docs
            .lock()
            .expect("mutex envenenado")
            .get(guild_id)
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firestore::decode_document;
    use firestore::gcloud_sdk::google::firestore::v1::value::ValueType;
    use firestore::gcloud_sdk::google::firestore::v1::{Document, Value as FsValue};
    use firestore::gcloud_sdk::prost_types::Timestamp;

    fn val(v: ValueType) -> FsValue {
        FsValue {
            value_type: Some(v),
        }
    }

    #[test]
    fn firestore_document_decodes_with_unknown_and_timestamp_fields() {
        let mut fields = HashMap::new();
        fields.insert(
            "adminRoleId".into(),
            val(ValueType::StringValue("9".into())),
        );
        fields.insert("gamesPerMatch".into(), val(ValueType::IntegerValue(3)));
        fields.insert("customFlag".into(), val(ValueType::BooleanValue(true)));
        fields.insert(
            "updatedAt".into(),
            val(ValueType::TimestampValue(Timestamp {
                seconds: 1_793_613_600,
                nanos: 0,
            })),
        );
        let doc = Document {
            name: "projects/p/databases/(default)/documents/guild_settings/g1".into(),
            fields,
            ..Default::default()
        };
        let (id, s): (String, GuildSettings) = decode_document(&doc).expect("decodifica");
        assert_eq!(id, "g1");
        assert_eq!(s.admin_role_id.as_deref(), Some("9"));
        assert_eq!(s.games_per_match, Some(3));
        assert!(s.updated_at.unwrap().starts_with("2026-11-02T10:00:00"));
        assert_eq!(s.extra["customFlag"], Value::Bool(true));
    }

    // --- Repo + traducción real del listener (WatchState) -------------------------------------

    use crate::firestore::WatchState;
    use firestore::gcloud_sdk::google::firestore::v1::target_change::TargetChangeType;
    use firestore::gcloud_sdk::google::firestore::v1::TargetChange;
    use firestore::FirestoreListenEvent;
    use tokio::sync::mpsc;

    const TARGET: i32 = 1;
    type Docs = Arc<Mutex<HashMap<String, GuildSettings>>>;

    /// Fuente cuyo `watch()` pasa eventos crudos del listener por [`WatchState`], como
    /// `FirestoreClient::watch_collection` (sin red).
    struct RawSource {
        docs: Docs,
        events: Mutex<Option<mpsc::UnboundedReceiver<FirestoreListenEvent>>>,
    }

    #[async_trait]
    impl GuildSettingsSource for RawSource {
        async fn fetch(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError> {
            Ok(self.docs.lock().unwrap().get(guild_id).cloned())
        }

        fn watch(&self) -> BoxStream<'static, Result<ChangeEvent, DataError>> {
            match self.events.lock().unwrap().take() {
                Some(rx) => futures::stream::unfold(
                    (rx, WatchState::new(TARGET)),
                    |(mut rx, mut state)| async move {
                        loop {
                            let event = rx.recv().await?;
                            if let Some(change) = state.on_event(event) {
                                return Some((Ok(change), (rx, state)));
                            }
                        }
                    },
                )
                .boxed(),
                None => futures::stream::pending().boxed(),
            }
        }
    }

    fn target_change(kind: TargetChangeType) -> FirestoreListenEvent {
        FirestoreListenEvent::TargetChange(TargetChange {
            target_change_type: kind as i32,
            target_ids: vec![TARGET],
            ..Default::default()
        })
    }

    async fn settle() {
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
    }

    /// Ventana residual de la fase 1: un documento cacheado tras abrir el stream y borrado antes
    /// de que la instantánea inicial sea actual no llega como evento (la instantánea ya no lo
    /// incluye). El `Reset` del primer `CURRENT` invalida la caché y la lectura recarga.
    #[tokio::test(start_paused = true)]
    async fn doc_deleted_before_initial_snapshot_is_not_served() {
        let docs: Docs = Arc::default();
        docs.lock().unwrap().insert(
            "g1".into(),
            GuildSettings {
                admin_role_id: Some("r".into()),
                ..Default::default()
            },
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let source = RawSource {
            docs: docs.clone(),
            events: Mutex::new(Some(rx)),
        };
        let repo = Arc::new(FirestoreGuildSettingsRepo::new(
            source,
            Duration::from_secs(3600),
        ));
        let _watcher = repo.clone().spawn_watcher();
        settle().await; // stream abierto e invalidate_all de apertura ya hecho
        tx.send(target_change(TargetChangeType::Add)).unwrap();
        settle().await;

        assert!(
            repo.get("g1").await.unwrap().is_some(),
            "cargado y cacheado"
        );
        // Borrado antes de la instantánea: Firestore no manda ningún evento del documento.
        docs.lock().unwrap().remove("g1");
        tx.send(target_change(TargetChangeType::Current)).unwrap();
        settle().await;

        assert_eq!(
            repo.get("g1").await.unwrap(),
            None,
            "tras el Reset del CURRENT la lectura recarga y ve que no existe"
        );
    }
}
