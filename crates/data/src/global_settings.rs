//! Ajustes globales (documento `faceit_config/settings`): modelo, repositorio con caché y
//! watcher, y la configuración efectiva por servidor.
//!
//! Legacy:
//!
//! * `database.js` `getSettings` (~1950): lee `faceit_config/settings`; si no existe devuelve
//!   `getDefaultSettings()` (~3365); ante **cualquier** error devuelve la copia en memoria o, si no
//!   hay, los valores por defecto (nunca falla). Caché de 6 h (`SETTINGS_TTL`), refrescada por el
//!   `onSnapshot` del documento (~805).
//! * `config/index.js` `getEffectiveConfig` (~128) y `getEffectiveConfigForGuild` (~178): mezclan
//!   entorno, ajustes globales y ajustes del servidor; ver [`effective_config_for_guild`].

use crate::cache::TtlCache;
use crate::error::DataError;
use crate::firestore::{ChangeEvent, FirestoreClient, WatchScope};
use crate::guild_settings::GuildSettings;
use crate::model::lenient_string;
use crate::source::{from_json, strip_synthetic, DocSource};
use crate::watch::{keep_cache_in_sync, WatcherHandle};
use async_trait::async_trait;
use common::config::{Config, EnvQueue};
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Vida de los ajustes globales en caché (`SETTINGS_TTL` de `database.js`, 6 h).
pub const GLOBAL_SETTINGS_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const COLLECTION: &str = "faceit_config";
const DOC_ID: &str = "settings";

/// Documento `faceit_config/settings`.
///
/// Tiene los mismos campos que [`GuildSettings`] (en [`GlobalSettings::fields`], aplanados en el
/// JSON) más `discordGuildId`, el servidor por defecto. Los interruptores de funciones
/// (`autoWelcomeDm`, `command_*`, ...) y todo lo demás quedan en `fields.extra`, sin perder nada.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSettings {
    /// `discordGuildId`: servidor por defecto (si falta, el de `DISCORD_GUILD_ID`).
    #[serde(
        default,
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub discord_guild_id: Option<String>,
    /// Campos comunes con los ajustes por servidor (y el resto en `extra`).
    #[serde(flatten)]
    pub fields: GuildSettings,
}

/// `getDefaultSettings()` de `database.js` (~3365), literal.
const LEGACY_DEFAULTS: &str = r#"{
    "discordLinkChannelId": "",
    "discordMatchesChannelId": "",
    "discordMixtonChannelId": "",
    "discordRolesChannelId": "",
    "discordStatusChannelId": "",
    "discordWaitingRoomChannelId": "",
    "discordAdminAlertsChannelId": "",
    "discordSupportChannelId": "",
    "discordMusicChannelId": "",
    "discordSanctionsChannelId": "",
    "autoWelcomeDm": true,
    "autoRoleSync": true,
    "autoNickSync": true,
    "autoVoiceChannels": true,
    "autoMovePlayers": true,
    "matchAnnouncements": true,
    "serverStatusAnnouncements": true,
    "liveMatchPanel": true,
    "webhookValidation": false,
    "adminCallNotifications": true,
    "hubBansNotifications": true,
    "musicBot": true,
    "rechargeWalletSystem": true,
    "mixtonReadyPanel": true,
    "command_verificar": true,
    "command_desvincular": true,
    "command_leaderboard": true,
    "command_stats": true,
    "command_info": true,
    "command_sincronizar": true,
    "command_partidas": true,
    "command_crear_sala": true,
    "command_mixton_listo": true,
    "command_mixton_no_listo": true,
    "command_cs2_play": true,
    "command_sancionar": true,
    "command_desancionar": true,
    "command_say": true,
    "command_recargar": true,
    "command_setup_recargas": true,
    "command_setup_soporte": true,
    "command_llamar_admin": true,
    "faceitHubId": "",
    "adminRoleId": "",
    "matchTimeoutMinutes": 120,
    "gamesPerMatch": 1,
    "roundsPerGame": 1,
    "updatedAt": null
}"#;

/// [`LEGACY_DEFAULTS`] como JSON.
fn legacy_defaults_json() -> Value {
    serde_json::from_str(LEGACY_DEFAULTS).expect("JSON de valores por defecto")
}

impl GlobalSettings {
    /// Lo que devuelve `getSettings` cuando el documento no existe o la lectura falla sin copia
    /// en memoria (`getDefaultSettings()` de `database.js`).
    pub fn legacy_defaults() -> GlobalSettings {
        static DEFAULTS: OnceLock<GlobalSettings> = OnceLock::new();
        DEFAULTS
            .get_or_init(|| {
                serde_json::from_value(legacy_defaults_json()).expect("valores por defecto")
            })
            .clone()
    }
}

/// Decodifica `faceit_config/settings` ya en JSON, sin los campos sintéticos `_firestore_*`.
pub(crate) fn decode_global(doc: Value) -> Result<GlobalSettings, DataError> {
    from_json(DOC_ID, strip_synthetic(doc))
}

/// Repositorio de los ajustes globales.
#[async_trait]
pub trait GlobalSettingsRepo: Send + Sync {
    /// Ajustes globales. Como `getSettings` del legacy nunca falla: documento ausente ->
    /// [`GlobalSettings::legacy_defaults`]; error de lectura -> la última copia leída o, si no
    /// hay, los valores por defecto.
    async fn get(&self) -> GlobalSettings;
}

/// Fuente de datos del repo con caché (costura para pruebas).
#[async_trait]
pub trait GlobalSettingsSource: Send + Sync + 'static {
    /// Lee `faceit_config/settings`; `Ok(None)` si no existe.
    async fn fetch(&self) -> Result<Option<GlobalSettings>, DataError>;
    /// Stream de cambios del documento `faceit_config/settings` (en producción, un listener de
    /// ese único documento: [`settings_watch_scope`]). El watcher solo mira la clave `settings`,
    /// así que un evento de otro id tampoco tocaría su entrada.
    fn watch(&self) -> BoxStream<'static, Result<ChangeEvent, DataError>>;
}

/// Lo que observa el watcher de producción: solo `faceit_config/settings`, como el
/// `faceit_config.doc('settings').onSnapshot` del legacy (`database.js` ~805). Escuchar la
/// colección entera traería (y cobraría) cada cambio de `mixton_chat_<guild>`,
/// `mixton_lobby_<guild>`, `panel_*`, `status_panel`, `messages`..., que Node escribe sin parar.
pub(crate) fn settings_watch_scope() -> WatchScope {
    WatchScope::Document {
        collection: COLLECTION.to_string(),
        id: DOC_ID.to_string(),
    }
}

#[async_trait]
impl GlobalSettingsSource for FirestoreClient {
    async fn fetch(&self) -> Result<Option<GlobalSettings>, DataError> {
        DocSource::get(self, COLLECTION, DOC_ID)
            .await?
            .map(decode_global)
            .transpose()
    }

    fn watch(&self) -> BoxStream<'static, Result<ChangeEvent, DataError>> {
        FirestoreClient::watch(self, settings_watch_scope()).boxed()
    }
}

/// Repo con caché TTL (una sola clave) sobre una fuente, con el mismo watcher que
/// `FirestoreGuildSettingsRepo` ([`crate::watch::keep_cache_in_sync`]).
pub struct FirestoreGlobalSettingsRepo<S: GlobalSettingsSource> {
    source: Arc<S>,
    cache: TtlCache<String, Option<GlobalSettings>>,
    restarts: AtomicU64,
}

impl FirestoreGlobalSettingsRepo<FirestoreClient> {
    /// Repo sobre un cliente Firestore real.
    pub fn from_client(client: FirestoreClient, ttl: Duration) -> Self {
        Self::new(client, ttl)
    }
}

impl<S: GlobalSettingsSource> FirestoreGlobalSettingsRepo<S> {
    /// Repo sobre `source` con la vida de caché `ttl` (producción: [`GLOBAL_SETTINGS_TTL`]).
    pub fn new(source: S, ttl: Duration) -> Self {
        Self {
            source: Arc::new(source),
            cache: TtlCache::new(ttl, 4),
            restarts: AtomicU64::new(0),
        }
    }

    /// Descarta la entrada (p. ej. tras escribir los ajustes); conserva la copia vieja.
    pub async fn invalidate(&self) {
        self.cache.invalidate(&DOC_ID.to_string()).await;
    }

    /// Veces que el watcher se ha reabierto tras terminar su stream.
    pub fn watcher_restarts(&self) -> u64 {
        self.restarts.load(Ordering::Relaxed)
    }

    /// Lanza el watcher de `faceit_config/settings` (mismo bucle que el de `guild_settings`:
    /// `Added`/`Modified` de `settings` invalidan, `Removed` purga también la copia vieja, `Reset`
    /// invalida, `Synced` sin `settings` purga la copia vieja; reapertura con espera 1 s -> 60 s).
    pub fn spawn_watcher(self: Arc<Self>) -> WatcherHandle {
        WatcherHandle::new(tokio::spawn(async move {
            keep_cache_in_sync(&self.cache, &self.restarts, || self.source.watch()).await
        }))
    }
}

#[async_trait]
impl<S: GlobalSettingsSource> GlobalSettingsRepo for FirestoreGlobalSettingsRepo<S> {
    async fn get(&self) -> GlobalSettings {
        let source = self.source.clone();
        let loaded = self
            .cache
            .get_or_load_stale_on(
                DOC_ID.to_string(),
                move || async move { source.fetch().await },
                // `getSettings`: ante cualquier error, la copia en memoria.
                |_| true,
            )
            .await;
        match loaded {
            Ok(Some(settings)) => settings,
            Ok(None) | Err(_) => GlobalSettings::legacy_defaults(),
        }
    }
}

/// Repo en memoria para pruebas de otros crates.
#[derive(Default)]
pub struct InMemoryGlobalSettingsRepo {
    doc: Mutex<Option<GlobalSettings>>,
}

impl InMemoryGlobalSettingsRepo {
    /// Repo sin documento (devuelve los valores por defecto).
    pub fn new() -> Self {
        Self::default()
    }

    /// Crea o reemplaza el documento (`None` lo borra).
    pub fn set(&self, settings: Option<GlobalSettings>) {
        *self.doc.lock().expect("mutex envenenado") = settings;
    }
}

#[async_trait]
impl GlobalSettingsRepo for InMemoryGlobalSettingsRepo {
    async fn get(&self) -> GlobalSettings {
        self.doc
            .lock()
            .expect("mutex envenenado")
            .clone()
            .unwrap_or_else(GlobalSettings::legacy_defaults)
    }
}

// --- Configuración efectiva -------------------------------------------------------------------

/// Resultado de `getEffectiveConfig` / `getEffectiveConfigForGuild`.
///
/// Los campos tipados son los que el legacy calcula; `None` en un `Option<String>` es un
/// `undefined` de JS (no sale en el JSON). Todo lo demás de los documentos (`...settings`,
/// `...guildSettings`: interruptores, `adminRoleName`, `updatedAt`, ...) va en
/// [`EffectiveConfig::extra`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_guild_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_link_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_matches_channel_id: Option<String>,
    pub discord_mixton_channel_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_roles_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_status_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_admin_alerts_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_waiting_room_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_sanctions_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_support_channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_music_channel_id: Option<String>,
    pub discord_recharge_channel_id: String,
    pub discord_mixton_chat_channel_id: String,
    pub discord_match_access_role_id: String,
    pub hub_role_id: String,
    pub discord_hub_role_id: String,
    pub faceit_club_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faceit_hub_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faceit_leaderboard_id: Option<String>,
    /// Rol de administración efectivo ('' si no hay).
    pub admin_role_id: String,
    /// `parseInt(...)`; `None` es `NaN` (sale como `null`, igual que `JSON.stringify(NaN)`).
    pub match_timeout_minutes: Option<i64>,
    /// 1, 3 o 5 (cualquier otro valor pasa a 1).
    pub games_per_match: u32,
    /// Igual que `games_per_match` (alias legado).
    pub rounds_per_game: u32,
    pub queues: Vec<Value>,
    /// El resto de campos de los documentos (servidor sobre global).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Claves que la configuración efectiva calcula (se quitan de `extra` para no duplicarlas).
const COMPUTED_KEYS: [&str; 24] = [
    "discordGuildId",
    "discordLinkChannelId",
    "discordMatchesChannelId",
    "discordMixtonChannelId",
    "discordRolesChannelId",
    "discordStatusChannelId",
    "discordAdminAlertsChannelId",
    "discordWaitingRoomChannelId",
    "discordSanctionsChannelId",
    "discordSupportChannelId",
    "discordMusicChannelId",
    "discordRechargeChannelId",
    "discordMixtonChatChannelId",
    "discordMatchAccessRoleId",
    "hubRoleId",
    "discordHubRoleId",
    "faceitClubId",
    "faceitHubId",
    "faceitLeaderboardId",
    "adminRoleId",
    "matchTimeoutMinutes",
    "gamesPerMatch",
    "roundsPerGame",
    "queues",
];

/// Valor no vacío (el `||` de JS trata `''` como falso).
fn nz(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

/// Primer valor no vacío de la cadena `a || b || ...`.
fn first(chain: &[Option<&str>]) -> Option<String> {
    chain.iter().find_map(|v| nz(*v)).map(str::to_owned)
}

/// `parseInt(s, 10)` de JS: espacios iniciales, signo opcional y el prefijo de dígitos; `None`
/// es `NaN` (sin dígitos, o un número que no cabe en `i64`).
pub fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: &str = &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    Some(if neg { -n } else { n })
}

/// `Number(s)` de JS para los valores que llegan aquí (decimales): espacios fuera, `''` es 0;
/// `None` es `NaN`. No reconoce hexadecimal/binario/octal (`0x3`), que el legacy sí acepta.
fn js_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    t.parse::<f64>().ok().filter(|f| !f.is_nan())
}

/// `[1, 3, 5].includes(n) ? n : 1`.
fn games_or_one(n: Option<f64>) -> u32 {
    match n {
        Some(f) if f == 1.0 || f == 3.0 || f == 5.0 => f as u32,
        _ => 1,
    }
}

fn env_queue_json(q: &EnvQueue) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(q.id));
    m.insert("queueId".into(), json!(q.queue_id));
    m.insert("name".into(), json!(q.name));
    m.insert(
        "discordMatchesChannelId".into(),
        json!(q.discord_matches_channel_id),
    );
    m.insert(
        "discordWaitingRoomChannelId".into(),
        json!(q.discord_waiting_room_channel_id),
    );
    m.insert(
        "discordMixtonChannelId".into(),
        json!(q.discord_mixton_channel_id),
    );
    m.insert(
        "discordMatchAccessRoleId".into(),
        json!(q.discord_match_access_role_id),
    );
    if let Some(hub) = &q.hub_role_id {
        m.insert("hubRoleId".into(), json!(hub));
    }
    m.insert("discordHubRoleId".into(), json!(q.discord_hub_role_id));
    Value::Object(m)
}

/// Mapa JSON de un documento sin las claves calculadas.
fn spread<T: Serialize>(doc: &T) -> Map<String, Value> {
    let mut map = match serde_json::to_value(doc) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for key in COMPUTED_KEYS {
        map.remove(key);
    }
    map
}

/// Capa global del rol de administración: `settings.adminRoleId || settings.adminRoleName ||
/// process.env.ADMIN_ROLE_ID || env.ADMIN_ROLE_ID` (`config/index.js` ~154; `env.ADMIN_ROLE_ID`
/// es `ADMIN_ROLE_ID || ADMIN_ROLE_NAME`, o sea [`Config::admin_role_id`]). `None` si queda vacía.
///
/// Es exactamente lo que `web::discord::AdminRoleSources::global_admin_role` espera ya resuelto
/// (y coincide con `adminRoleId` de `effective_config_for_guild(None, ...)`).
pub fn global_admin_role(global: &GlobalSettings, env: &Config) -> Option<String> {
    first(&[
        global.fields.admin_role_id.as_deref(),
        global.fields.admin_role_name.as_deref(),
        env.admin_role_id.as_deref(),
    ])
}

/// `globalConfig.discordGuildId = settings.discordGuildId || DISCORD_GUILD_ID`.
fn default_guild_id<'a>(global: &'a GlobalSettings, env: &'a Config) -> &'a str {
    nz(global.discord_guild_id.as_deref()).unwrap_or(env.discord_guild_id.as_str())
}

/// `Boolean(globalConfig.discordGuildId && guildId === globalConfig.discordGuildId)`
/// (`config/index.js` ~135 y ~192): el servidor por defecto es `settings.discordGuildId` o, si
/// falta o está vacío, `DISCORD_GUILD_ID`; un id vacío nunca es el servidor por defecto.
pub fn is_default_guild(guild_id: &str, global: &GlobalSettings, env: &Config) -> bool {
    let id = default_guild_id(global, env);
    !id.is_empty() && guild_id == id
}

/// `getEffectiveConfig()` (sin servidor).
fn global_effective(global: &GlobalSettings, env: &Config) -> EffectiveConfig {
    let s = &global.fields;
    let e = &env.env_defaults;
    let pick = |a: &Option<String>, b: &Option<String>| first(&[a.as_deref(), b.as_deref()]);
    let games = s
        .games_per_match
        .or(s.rounds_per_game)
        .map(f64::from)
        .or_else(|| {
            e.games_per_match
                .as_deref()
                .or(e.rounds_per_game.as_deref())
                .map(|v| js_number(v).unwrap_or(f64::NAN))
        });
    let games = games_or_one(games.filter(|f| !f.is_nan()));
    EffectiveConfig {
        discord_guild_id: nz(Some(default_guild_id(global, env))).map(str::to_owned),
        discord_link_channel_id: pick(&s.discord_link_channel_id, &e.discord_link_channel_id),
        discord_matches_channel_id: pick(
            &s.discord_matches_channel_id,
            &e.discord_matches_channel_id,
        ),
        discord_mixton_channel_id: pick(&s.discord_mixton_channel_id, &e.discord_mixton_channel_id)
            .unwrap_or_default(),
        discord_roles_channel_id: pick(&s.discord_roles_channel_id, &e.discord_roles_channel_id),
        discord_status_channel_id: pick(&s.discord_status_channel_id, &e.discord_status_channel_id),
        discord_admin_alerts_channel_id: pick(
            &s.discord_admin_alerts_channel_id,
            &e.discord_admin_alerts_channel_id,
        ),
        discord_waiting_room_channel_id: pick(
            &s.discord_waiting_room_channel_id,
            &e.discord_waiting_room_channel_id,
        ),
        discord_sanctions_channel_id: pick(
            &s.discord_sanctions_channel_id,
            &e.discord_sanctions_channel_id,
        ),
        discord_support_channel_id: pick(
            &s.discord_support_channel_id,
            &e.discord_support_channel_id,
        ),
        discord_music_channel_id: pick(&s.discord_music_channel_id, &e.discord_music_channel_id),
        discord_recharge_channel_id: pick(
            &s.discord_recharge_channel_id,
            &e.discord_recharge_channel_id,
        )
        .unwrap_or_default(),
        discord_mixton_chat_channel_id: first(&[
            s.discord_mixton_chat_channel_id.as_deref(),
            e.discord_mixton_chat_channel_id.as_deref(),
            s.discord_mixton_channel_id.as_deref(),
            e.discord_mixton_channel_id.as_deref(),
        ])
        .unwrap_or_default(),
        discord_match_access_role_id: pick(
            &s.discord_match_access_role_id,
            &e.discord_match_access_role_id,
        )
        .unwrap_or_default(),
        hub_role_id: pick(&s.hub_role_id, &e.faceit_hub_role_id).unwrap_or_default(),
        discord_hub_role_id: pick(&s.discord_hub_role_id, &e.discord_hub_role_id)
            .unwrap_or_default(),
        faceit_club_id: pick(&s.faceit_club_id, &e.faceit_club_id).unwrap_or_default(),
        faceit_hub_id: pick(&s.faceit_hub_id, &e.faceit_hub_id),
        faceit_leaderboard_id: pick(&s.faceit_leaderboard_id, &e.faceit_leaderboard_id),
        admin_role_id: global_admin_role(global, env).unwrap_or_default(),
        // parseInt(settings.matchTimeoutMinutes || MATCH_TIMEOUT_MINUTES || '120', 10)
        match_timeout_minutes: match s.match_timeout_minutes.filter(|n| *n != 0) {
            Some(n) => Some(i64::from(n)),
            None => js_parse_int(e.match_timeout_minutes.as_deref().unwrap_or("120")),
        },
        games_per_match: games,
        rounds_per_game: games,
        queues: if s.queues.is_empty() {
            e.queues.iter().map(env_queue_json).collect()
        } else {
            s.queues.clone()
        },
        extra: spread(global),
    }
}

/// Puerto de `getEffectiveConfigForGuild(guildId)` (`config/index.js` ~178), sin su caché.
///
/// * `guild_id = None` (o vacío): `getEffectiveConfig()`, cada campo `settings.x || env X`.
/// * Con servidor: `{ ...globalConfig, ...guildSettings }` y cada campo `guild.x || (servidor por
///   defecto ? globalConfig.x : '')`: solo el servidor por defecto ([`is_default_guild`]) hereda
///   canales, roles y colas de la capa global/entorno (aislamiento estricto entre servidores).
///   `guild_settings = None` es un documento ausente (`{}` en el legacy).
///
/// La firma añade `guild_id` a la del plan: el legacy lo necesita (`discordGuildId: guildId` y la
/// comprobación de servidor por defecto) y no está en `GuildSettings`.
pub fn effective_config_for_guild(
    guild_id: Option<&str>,
    global: &GlobalSettings,
    guild_settings: Option<&GuildSettings>,
    env: &Config,
) -> EffectiveConfig {
    let base = global_effective(global, env);
    let Some(guild_id) = nz(guild_id) else {
        return base;
    };
    let empty = GuildSettings::default();
    let g = guild_settings.unwrap_or(&empty);
    let is_default = is_default_guild(guild_id, global, env);

    // `guild.x || (isDefaultGuild ? globalConfig.x : '')`
    let opt = |own: &Option<String>, inherited: &Option<String>| match nz(own.as_deref()) {
        Some(v) => Some(v.to_owned()),
        None if is_default => inherited.clone(),
        None => Some(String::new()),
    };
    let text = |own: &Option<String>, inherited: &String| match nz(own.as_deref()) {
        Some(v) => v.to_owned(),
        None if is_default => inherited.clone(),
        None => String::new(),
    };
    let games = if g.games_per_match.is_some() || g.rounds_per_game.is_some() {
        games_or_one(g.games_per_match.or(g.rounds_per_game).map(f64::from))
    } else {
        base.games_per_match
    };
    let mut extra = base.extra.clone();
    extra.extend(spread(g));

    EffectiveConfig {
        discord_guild_id: Some(guild_id.to_owned()),
        discord_link_channel_id: opt(&g.discord_link_channel_id, &base.discord_link_channel_id),
        discord_matches_channel_id: opt(
            &g.discord_matches_channel_id,
            &base.discord_matches_channel_id,
        ),
        discord_mixton_channel_id: text(
            &g.discord_mixton_channel_id,
            &base.discord_mixton_channel_id,
        ),
        discord_roles_channel_id: opt(&g.discord_roles_channel_id, &base.discord_roles_channel_id),
        discord_status_channel_id: opt(
            &g.discord_status_channel_id,
            &base.discord_status_channel_id,
        ),
        discord_admin_alerts_channel_id: opt(
            &g.discord_admin_alerts_channel_id,
            &base.discord_admin_alerts_channel_id,
        ),
        discord_waiting_room_channel_id: opt(
            &g.discord_waiting_room_channel_id,
            &base.discord_waiting_room_channel_id,
        ),
        discord_sanctions_channel_id: opt(
            &g.discord_sanctions_channel_id,
            &base.discord_sanctions_channel_id,
        ),
        discord_support_channel_id: opt(
            &g.discord_support_channel_id,
            &base.discord_support_channel_id,
        ),
        discord_music_channel_id: opt(&g.discord_music_channel_id, &base.discord_music_channel_id),
        discord_recharge_channel_id: text(
            &g.discord_recharge_channel_id,
            &base.discord_recharge_channel_id,
        ),
        // guild.chat || (isDefault ? global.chat : '') || guild.mixton || ''
        discord_mixton_chat_channel_id: first(&[
            g.discord_mixton_chat_channel_id.as_deref(),
            is_default.then_some(base.discord_mixton_chat_channel_id.as_str()),
            g.discord_mixton_channel_id.as_deref(),
        ])
        .unwrap_or_default(),
        discord_match_access_role_id: text(
            &g.discord_match_access_role_id,
            &base.discord_match_access_role_id,
        ),
        hub_role_id: text(&g.hub_role_id, &base.hub_role_id),
        discord_hub_role_id: text(&g.discord_hub_role_id, &base.discord_hub_role_id),
        faceit_club_id: text(&g.faceit_club_id, &base.faceit_club_id),
        faceit_hub_id: opt(&g.faceit_hub_id, &base.faceit_hub_id),
        faceit_leaderboard_id: opt(&g.faceit_leaderboard_id, &base.faceit_leaderboard_id),
        // guild.adminRoleId || guild.adminRoleName || (isDefault ? global.adminRoleId : '')
        admin_role_id: first(&[
            g.admin_role_id.as_deref(),
            g.admin_role_name.as_deref(),
            is_default.then_some(base.admin_role_id.as_str()),
        ])
        .unwrap_or_default(),
        match_timeout_minutes: match g.match_timeout_minutes {
            Some(n) => Some(i64::from(n)),
            None => base.match_timeout_minutes,
        },
        games_per_match: games,
        rounds_per_game: games,
        queues: if !g.queues.is_empty() {
            g.queues.clone()
        } else if is_default {
            base.queues.clone()
        } else {
            Vec::new()
        },
        extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(extra: &[(&str, &str)]) -> Config {
        let mut vars: HashMap<String, String> = [
            ("ADMIN_JWT_SECRET", "jwt-secret-value"),
            ("BASE_URL", "http://localhost:15633"),
            ("DISCORD_CLIENT_ID", "client-id"),
            ("DISCORD_CLIENT_SECRET", "client-secret-value"),
            ("DISCORD_TOKEN", "bot-token-value"),
            ("DISCORD_GUILD_ID", "ENV_GUILD"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in extra {
            vars.insert((*k).to_string(), (*v).to_string());
        }
        Config::from_map(&vars).expect("config")
    }

    fn global(doc: Value) -> GlobalSettings {
        serde_json::from_value(doc).expect("decodifica")
    }

    fn guild(doc: Value) -> GuildSettings {
        serde_json::from_value(doc).expect("decodifica")
    }

    #[test]
    fn is_default_guild_requires_non_empty_id() {
        let env = config(&[]);
        // settings.discordGuildId gana sobre DISCORD_GUILD_ID
        let g = global(json!({"discordGuildId": "SET"}));
        assert!(is_default_guild("SET", &g, &env));
        assert!(!is_default_guild("ENV_GUILD", &g, &env));
        // ausente o vacío: DISCORD_GUILD_ID
        for g in [global(json!({})), global(json!({"discordGuildId": ""}))] {
            assert!(is_default_guild("ENV_GUILD", &g, &env));
            assert!(!is_default_guild("otro", &g, &env));
            assert!(!is_default_guild("", &g, &env));
        }
        // los dos vacíos: ningún servidor es el de por defecto, tampoco el id vacío
        let mut env = env;
        env.discord_guild_id = String::new();
        let g = global(json!({"discordGuildId": ""}));
        assert!(!is_default_guild("", &g, &env));
        assert!(!is_default_guild("ENV_GUILD", &g, &env));
    }

    #[test]
    fn effective_config_prefers_guild_over_global_over_env() {
        let env = config(&[
            ("DISCORD_LINK_CHANNEL_ID", "env-link"),
            ("DISCORD_STATUS_CHANNEL_ID", "env-status"),
            ("DISCORD_MIXTON_CHANNEL_ID", "env-mixton"),
            ("FACEIT_HUB_ROLE_ID", "env-hubrole"),
            ("ADMIN_ROLE_ID", "env-admin"),
            ("MATCH_TIMEOUT_MINUTES", "90"),
            ("GAMES_PER_MATCH", "3"),
        ]);
        let g = global(json!({
            "discordGuildId": "MAIN",
            "discordLinkChannelId": "set-link",
            "discordStatusChannelId": "",
            "adminRoleName": "Staff",
            "autoWelcomeDm": false,
            "queues": [{"id": "gq"}],
        }));

        // --- Sin servidor: settings || entorno
        let c = effective_config_for_guild(None, &g, None, &env);
        assert_eq!(c.discord_guild_id.as_deref(), Some("MAIN"));
        assert_eq!(c.discord_link_channel_id.as_deref(), Some("set-link"));
        assert_eq!(c.discord_status_channel_id.as_deref(), Some("env-status"));
        assert_eq!(c.discord_roles_channel_id, None, "undefined en el legacy");
        assert_eq!(c.discord_mixton_channel_id, "env-mixton");
        assert_eq!(c.discord_mixton_chat_channel_id, "env-mixton");
        assert_eq!(c.hub_role_id, "env-hubrole");
        assert_eq!(c.faceit_club_id, "");
        assert_eq!(c.admin_role_id, "Staff");
        assert_eq!(c.match_timeout_minutes, Some(90));
        assert_eq!((c.games_per_match, c.rounds_per_game), (3, 3));
        assert_eq!(c.queues, vec![json!({"id": "gq"})]);
        assert_eq!(c.extra["autoWelcomeDm"], json!(false));
        assert_eq!(c.extra["adminRoleName"], json!("Staff"));

        // --- Servidor por defecto: servidor || global (que ya es settings || entorno)
        let own = guild(json!({
            "discordStatusChannelId": "guild-status",
            "discordMixtonChannelId": "guild-mixton",
            "matchTimeoutMinutes": "15",
            "roundsPerGame": 7,
            "autoWelcomeDm": true,
        }));
        let c = effective_config_for_guild(Some("MAIN"), &g, Some(&own), &env);
        assert_eq!(c.discord_guild_id.as_deref(), Some("MAIN"));
        assert_eq!(c.discord_status_channel_id.as_deref(), Some("guild-status"));
        assert_eq!(c.discord_link_channel_id.as_deref(), Some("set-link"));
        assert_eq!(c.discord_roles_channel_id, None);
        assert_eq!(c.discord_mixton_channel_id, "guild-mixton");
        // chat: guild.chat || global.chat (env-mixton) || guild.mixton
        assert_eq!(c.discord_mixton_chat_channel_id, "env-mixton");
        assert_eq!(c.hub_role_id, "env-hubrole");
        assert_eq!(c.admin_role_id, "Staff");
        assert_eq!(c.match_timeout_minutes, Some(15));
        assert_eq!(c.games_per_match, 1, "7 no es 1, 3 ni 5");
        assert_eq!(c.queues, vec![json!({"id": "gq"})]);
        assert_eq!(c.extra["autoWelcomeDm"], json!(true), "el servidor gana");

        // --- Otro servidor: nada se hereda salvo números y extras
        let c = effective_config_for_guild(Some("OTHER"), &g, None, &env);
        assert_eq!(c.discord_guild_id.as_deref(), Some("OTHER"));
        assert_eq!(c.discord_link_channel_id.as_deref(), Some(""));
        assert_eq!(c.discord_roles_channel_id.as_deref(), Some(""));
        assert_eq!(c.discord_mixton_channel_id, "");
        assert_eq!(c.discord_mixton_chat_channel_id, "");
        assert_eq!(c.hub_role_id, "");
        assert_eq!(c.admin_role_id, "");
        assert_eq!(c.match_timeout_minutes, Some(90));
        assert_eq!(c.games_per_match, 3);
        assert!(c.queues.is_empty());
        assert_eq!(c.extra["autoWelcomeDm"], json!(false));

        // otro servidor con su propio chat vacío cae a su canal de Mixton
        let own = guild(json!({"discordMixtonChannelId": "gm", "adminRoleName": "Mods"}));
        let c = effective_config_for_guild(Some("OTHER"), &g, Some(&own), &env);
        assert_eq!(c.discord_mixton_chat_channel_id, "gm");
        assert_eq!(c.admin_role_id, "Mods");

        // el JSON no duplica claves calculadas y lleva `queues` siempre
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["discordGuildId"], json!("OTHER"));
        assert_eq!(json["queues"], json!([]));
        assert_eq!(json["adminRoleName"], json!("Mods"));
    }

    #[test]
    fn effective_config_numbers_follow_legacy_coercions() {
        let env = config(&[]);
        let c = effective_config_for_guild(None, &global(json!({})), None, &env);
        assert_eq!(c.match_timeout_minutes, Some(120));
        assert_eq!(c.games_per_match, 1);
        // matchTimeoutMinutes 0 es falso: cae al entorno / 120
        let g = global(json!({"matchTimeoutMinutes": 0, "gamesPerMatch": 5}));
        let c = effective_config_for_guild(None, &g, None, &env);
        assert_eq!(c.match_timeout_minutes, Some(120));
        assert_eq!(c.games_per_match, 5);
        // entorno no numérico: NaN (null en el JSON)
        let env = config(&[("MATCH_TIMEOUT_MINUTES", "abc"), ("ROUNDS_PER_GAME", " 3 ")]);
        let c = effective_config_for_guild(None, &global(json!({})), None, &env);
        assert_eq!(c.match_timeout_minutes, None);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["matchTimeoutMinutes"],
            Value::Null
        );
        assert_eq!(c.games_per_match, 3);
        // las colas de entorno se usan si el documento no trae ninguna
        let env = config(&[("FACEIT_QUEUE1_ID", "q1")]);
        let c = effective_config_for_guild(None, &global(json!({"queues": []})), None, &env);
        assert_eq!(c.queues[0]["queueId"], json!("q1"));
        assert_eq!(c.queues[0]["hubRoleId"], json!(""));
    }

    #[test]
    fn js_parse_int_matches_node() {
        assert_eq!(js_parse_int("120"), Some(120));
        assert_eq!(js_parse_int("  42abc"), Some(42));
        assert_eq!(js_parse_int("-7"), Some(-7));
        assert_eq!(js_parse_int("+8"), Some(8));
        assert_eq!(js_parse_int("abc"), None);
        assert_eq!(js_parse_int(""), None);
        assert_eq!(js_parse_int("3.9"), Some(3));
    }

    /// La capa global que `web::discord::is_admin` recibe ya resuelta
    /// (`AdminRoleSources::global_admin_role`): `settings.adminRoleId || settings.adminRoleName
    /// || ADMIN_ROLE_ID || ADMIN_ROLE_NAME`, vacíos como ausentes, igual que `adminRoleId` de la
    /// configuración efectiva global.
    #[test]
    fn global_admin_role_layer_matches_legacy_chain() {
        type Case = (
            Value,
            &'static [(&'static str, &'static str)],
            Option<&'static str>,
        );
        let cases: [Case; 8] = [
            (
                json!({"adminRoleId": "1", "adminRoleName": "N"}),
                &[("ADMIN_ROLE_ID", "E")],
                Some("1"),
            ),
            (
                json!({"adminRoleId": "", "adminRoleName": "N"}),
                &[("ADMIN_ROLE_ID", "E")],
                Some("N"),
            ),
            (json!({"adminRoleName": "N"}), &[], Some("N")),
            (
                json!({"adminRoleId": "", "adminRoleName": ""}),
                &[("ADMIN_ROLE_ID", "E")],
                Some("E"),
            ),
            (json!({}), &[("ADMIN_ROLE_NAME", "EN")], Some("EN")),
            (
                json!({}),
                &[("ADMIN_ROLE_ID", "E"), ("ADMIN_ROLE_NAME", "EN")],
                Some("E"),
            ),
            (json!({"adminRoleId": 123}), &[], Some("123")),
            (json!({"adminRoleId": ""}), &[], None),
        ];
        for (doc, vars, want) in cases {
            let env = config(vars);
            let g = global(doc.clone());
            assert_eq!(
                global_admin_role(&g, &env).as_deref(),
                want,
                "{doc} {vars:?}"
            );
            assert_eq!(
                effective_config_for_guild(None, &g, None, &env).admin_role_id,
                want.unwrap_or(""),
                "{doc}"
            );
        }
        // Valores por defecto del legacy (`adminRoleId: ''`): solo cuenta el entorno.
        let env = config(&[("ADMIN_ROLE_ID", "E")]);
        assert_eq!(
            global_admin_role(&GlobalSettings::legacy_defaults(), &env).as_deref(),
            Some("E")
        );
    }

    #[test]
    fn global_settings_decode_keeps_unknown_fields_and_drops_synthetic_ones() {
        let doc = json!({
            "discordGuildId": "MAIN",
            "adminRoleId": "9",
            "autoWelcomeDm": false,
            "updatedAt": "2026-11-02T10:00:00Z",
            "_firestore_id": "settings",
            "_firestore_full_id": "projects/secret/databases/(default)/documents/faceit_config/settings",
        });
        let g = decode_global(doc).unwrap();
        assert_eq!(g.discord_guild_id.as_deref(), Some("MAIN"));
        assert_eq!(g.fields.admin_role_id.as_deref(), Some("9"));
        assert_eq!(
            serde_json::to_value(&g).unwrap(),
            json!({
                "discordGuildId": "MAIN",
                "adminRoleId": "9",
                "autoWelcomeDm": false,
                "updatedAt": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 0},
            })
        );
    }

    #[test]
    fn legacy_defaults_are_the_database_js_ones() {
        let d = GlobalSettings::legacy_defaults();
        assert_eq!(d.discord_guild_id, None);
        assert_eq!(d.fields.admin_role_id.as_deref(), Some(""));
        assert_eq!(d.fields.match_timeout_minutes, Some(120));
        assert_eq!(d.fields.extra["command_llamar_admin"], json!(true));
        assert_eq!(d.fields.extra["webhookValidation"], json!(false));
        assert_eq!(serde_json::to_value(&d).unwrap(), legacy_defaults_json());
    }

    // --- Repo + watcher -------------------------------------------------------------------------

    use tokio::sync::mpsc;

    type Item = Result<ChangeEvent, DataError>;

    #[derive(Default)]
    struct Fake {
        doc: Mutex<Option<GlobalSettings>>,
        failure: Mutex<Option<DataError>>,
        fetches: AtomicU64,
        events: Mutex<Option<mpsc::UnboundedReceiver<Item>>>,
    }

    struct Shared(Arc<Fake>);

    #[async_trait]
    impl GlobalSettingsSource for Shared {
        async fn fetch(&self) -> Result<Option<GlobalSettings>, DataError> {
            self.0.fetches.fetch_add(1, Ordering::SeqCst);
            if let Some(err) = self.0.failure.lock().unwrap().clone() {
                return Err(err);
            }
            Ok(self.0.doc.lock().unwrap().clone())
        }

        fn watch(&self) -> BoxStream<'static, Item> {
            match self.0.events.lock().unwrap().take() {
                Some(rx) => futures::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|item| (item, rx))
                })
                .boxed(),
                None => futures::stream::pending().boxed(),
            }
        }
    }

    impl Fake {
        fn put(&self, admin: &str) {
            *self.doc.lock().unwrap() = Some(global(json!({"adminRoleId": admin})));
        }
        fn fail(&self, err: Option<DataError>) {
            *self.failure.lock().unwrap() = err;
        }
        fn fetches(&self) -> u64 {
            self.fetches.load(Ordering::SeqCst)
        }
    }

    async fn settle() {
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
    }

    fn admin(g: &GlobalSettings) -> Option<&str> {
        g.fields.admin_role_id.as_deref()
    }

    #[tokio::test]
    async fn missing_document_and_errors_fall_back_like_get_settings() {
        let fake = Arc::new(Fake::default());
        let repo = FirestoreGlobalSettingsRepo::new(Shared(fake.clone()), GLOBAL_SETTINGS_TTL);
        assert_eq!(repo.get().await, GlobalSettings::legacy_defaults());
        // Sin copia vieja, un error da los valores por defecto (y no se cachea).
        repo.invalidate().await;
        fake.fail(Some(DataError::Firestore("caído".into())));
        assert_eq!(repo.get().await, GlobalSettings::legacy_defaults());
        // Con copia vieja, cualquier error la sirve.
        fake.fail(None);
        fake.put("r1");
        repo.invalidate().await;
        assert_eq!(admin(&repo.get().await), Some("r1"));
        repo.invalidate().await;
        fake.fail(Some(DataError::Firestore("caído".into())));
        assert_eq!(admin(&repo.get().await), Some("r1"));
    }

    /// `Added`/`Modified`/`Removed`/`Reset`/`Synced` sobre la única clave, idempotentes y sin
    /// esperas reales (solo `yield_now`).
    #[tokio::test]
    async fn global_settings_watcher_follows_every_change_event() {
        let fake = Arc::new(Fake::default());
        let (tx, rx) = mpsc::unbounded_channel();
        *fake.events.lock().unwrap() = Some(rx);
        fake.put("r1");
        let repo = Arc::new(FirestoreGlobalSettingsRepo::new(
            Shared(fake.clone()),
            GLOBAL_SETTINGS_TTL,
        ));
        let _watcher = repo.clone().spawn_watcher();
        settle().await;

        assert_eq!(admin(&repo.get().await), Some("r1"));
        assert_eq!(admin(&repo.get().await), Some("r1"));
        assert_eq!(fake.fetches(), 1, "cacheado");

        // El listener real solo trae `settings` (ver
        // `production_watcher_targets_only_the_settings_document`); aun así, un evento de otro
        // id no tocaría la entrada.
        tx.send(Ok(ChangeEvent::Modified("messages".into())))
            .unwrap();
        settle().await;
        repo.get().await;
        assert_eq!(fake.fetches(), 1);

        for event in [
            ChangeEvent::Added("settings".into()),
            ChangeEvent::Modified("settings".into()),
            ChangeEvent::Reset,
            ChangeEvent::Synced(vec!["messages".into(), "settings".into()]),
        ] {
            fake.put(&format!("{event:?}"));
            // dos veces: idempotente
            tx.send(Ok(event.clone())).unwrap();
            tx.send(Ok(event.clone())).unwrap();
            settle().await;
            assert_eq!(
                admin(&repo.get().await),
                Some(format!("{event:?}").as_str())
            );
        }
        let before = fake.fetches();
        repo.get().await;
        assert_eq!(fake.fetches(), before, "cacheado de nuevo");

        // Synced con `settings`: la copia vieja sigue sirviendo ante error.
        fake.fail(Some(DataError::QuotaExceeded));
        tx.send(Ok(ChangeEvent::Synced(vec!["settings".into()])))
            .unwrap();
        settle().await;
        assert_eq!(
            admin(&repo.get().await),
            Some("Synced([\"messages\", \"settings\"])")
        );

        // Synced sin `settings`: borrado sin evento; la copia vieja no se sirve.
        tx.send(Ok(ChangeEvent::Synced(vec!["messages".into()])))
            .unwrap();
        settle().await;
        assert_eq!(repo.get().await, GlobalSettings::legacy_defaults());

        // Removed: igual (entrada y copia vieja fuera).
        fake.fail(None);
        fake.put("r2");
        repo.get().await;
        tx.send(Ok(ChangeEvent::Removed("settings".into())))
            .unwrap();
        tx.send(Ok(ChangeEvent::Removed("settings".into())))
            .unwrap();
        settle().await;
        fake.fail(Some(DataError::QuotaExceeded));
        assert_eq!(repo.get().await, GlobalSettings::legacy_defaults());
        assert_eq!(repo.watcher_restarts(), 0);
    }

    /// El watcher de producción escucha solo el documento `faceit_config/settings` (como el
    /// `doc('settings').onSnapshot` del legacy): el target que se registra en Firestore lleva ese
    /// único id, así que los cambios de `mixton_chat_<guild>`, `mixton_lobby_<guild>`,
    /// `panel_*`, `messages`... ni se leen (no cuestan lecturas) ni llegan a la caché.
    #[test]
    fn production_watcher_targets_only_the_settings_document() {
        use crate::firestore::{listen_target_type, WatchScope};
        use firestore::FirestoreTargetType;

        let scope = settings_watch_scope();
        assert_eq!(
            scope,
            WatchScope::Document {
                collection: "faceit_config".into(),
                id: "settings".into(),
            }
        );
        match listen_target_type(&scope) {
            FirestoreTargetType::Documents(docs) => {
                assert_eq!(docs.collection, "faceit_config");
                assert_eq!(docs.parent, None);
                assert_eq!(docs.documents, vec!["settings".to_string()]);
                for other in ["mixton_chat_x", "mixton_lobby_x", "panel_x", "messages"] {
                    assert!(!docs.documents.iter().any(|d| d == other), "{other}");
                }
            }
            FirestoreTargetType::Query(q) => {
                panic!("se esperaba un target de documento, no una consulta: {q:?}")
            }
        }
    }

    #[tokio::test]
    async fn in_memory_repo_defaults_when_unset() {
        let repo = InMemoryGlobalSettingsRepo::new();
        assert_eq!(repo.get().await, GlobalSettings::legacy_defaults());
        repo.set(Some(global(json!({"adminRoleId": "x"}))));
        assert_eq!(admin(&repo.get().await), Some("x"));
    }
}
