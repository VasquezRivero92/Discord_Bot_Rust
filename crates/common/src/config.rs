//! Configuración tipada leída del entorno (y de `.env` si existe).

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// Error al construir la configuración.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Falta una variable obligatoria (o está vacía).
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    /// El valor de una variable no se puede interpretar.
    #[error("invalid value for environment variable {0}: {1:?}")]
    Invalid(&'static str, String),
    /// `FIRESTORE_PROJECT_ID` sin ninguna fuente de credenciales explícita.
    #[error(
        "FIRESTORE_PROJECT_ID is set but no Firestore credential source is: set FIRESTORE_EMULATOR_HOST, FIREBASE_SERVICE_ACCOUNT_PATH or FIRESTORE_USE_ADC=1 (Application Default Credentials are never used implicitly)"
    )]
    NoFirestoreCredentials,
}

/// Cómo se conecta (o no) a Firestore. Se decide con [`firestore_mode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirestoreMode {
    /// Sin `FIRESTORE_PROJECT_ID`: nada que necesite Firestore se monta.
    Disabled,
    /// `FIRESTORE_EMULATOR_HOST`: emulador local (nunca datos de producción).
    Emulator { host: String },
    /// `FIREBASE_SERVICE_ACCOUNT_PATH`: clave de cuenta de servicio.
    KeyFile { path: PathBuf },
    /// `FIRESTORE_USE_ADC=1`: Application Default Credentials (opt-in explícito).
    Adc,
}

impl FirestoreMode {
    /// Descripción para logs; no contiene secretos (la ruta de la clave no lo es,
    /// su contenido sí y nunca se lee aquí).
    pub fn describe(&self) -> String {
        match self {
            FirestoreMode::Disabled => "disabled".to_owned(),
            FirestoreMode::Emulator { host } => format!("emulator ({host})"),
            FirestoreMode::KeyFile { path } => format!("key-file ({})", path.display()),
            FirestoreMode::Adc => "adc".to_owned(),
        }
    }
}

/// Modo de credenciales de Firestore, explícito y sin sorpresas.
///
/// | proyecto | emulador | clave | ADC | resultado |
/// |---|---|---|---|---|
/// | no | * | * | * | `Disabled` |
/// | sí | sí | * | * | `Emulator` |
/// | sí | no | sí | * | `KeyFile` |
/// | sí | no | no | sí | `Adc` |
/// | sí | no | no | no | error `NoFirestoreCredentials` |
///
/// El emulador gana porque el crate `firestore` se conecta al emulador siempre que
/// `FIRESTORE_EMULATOR_HOST` está definida; así el modo informado es el real.
pub fn firestore_mode(cfg: &Config) -> Result<FirestoreMode, ConfigError> {
    if cfg.firestore_project_id.is_none() {
        return Ok(FirestoreMode::Disabled);
    }
    if let Some(host) = &cfg.firestore_emulator_host {
        return Ok(FirestoreMode::Emulator { host: host.clone() });
    }
    if let Some(path) = &cfg.firebase_service_account_path {
        return Ok(FirestoreMode::KeyFile { path: path.clone() });
    }
    if cfg.firestore_use_adc {
        return Ok(FirestoreMode::Adc);
    }
    Err(ConfigError::NoFirestoreCredentials)
}

/// Cola de partidas definida por entorno (`getQueuesFromEnv` de `config/index.js`).
///
/// Los nombres legados (camelCase) son los del objeto que construye el legacy: `id`, `queueId`,
/// `name`, `discordMatchesChannelId`, `discordWaitingRoomChannelId`, `discordMixtonChannelId`,
/// `discordMatchAccessRoleId`, `hubRoleId` (solo en las colas numeradas) y `discordHubRoleId`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvQueue {
    pub id: String,
    pub queue_id: String,
    pub name: String,
    pub discord_matches_channel_id: String,
    pub discord_waiting_room_channel_id: String,
    pub discord_mixton_channel_id: String,
    pub discord_match_access_role_id: String,
    /// `None` en las colas por listas separadas por coma (el legacy no pone la clave).
    pub hub_role_id: Option<String>,
    pub discord_hub_role_id: String,
}

/// Capa de entorno de `getEffectiveConfig` (`config/index.js` ~128-160): la última de la cadena
/// `settings.x || process.env.X || env.X`. Como el resto de [`Config`], los valores se recortan y
/// un valor vacío cuenta como ausente (en JS `''` también es falso).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvDefaults {
    /// `DISCORD_LINK_CHANNEL_ID`
    pub discord_link_channel_id: Option<String>,
    /// `DISCORD_MATCHES_CHANNEL_ID`
    pub discord_matches_channel_id: Option<String>,
    /// `DISCORD_MIXTON_CHANNEL_ID`
    pub discord_mixton_channel_id: Option<String>,
    /// `DISCORD_ROLES_CHANNEL_ID`
    pub discord_roles_channel_id: Option<String>,
    /// `DISCORD_STATUS_CHANNEL_ID`
    pub discord_status_channel_id: Option<String>,
    /// `DISCORD_ADMIN_ALERTS_CHANNEL_ID`
    pub discord_admin_alerts_channel_id: Option<String>,
    /// `DISCORD_WAITING_ROOM_CHANNEL_ID`
    pub discord_waiting_room_channel_id: Option<String>,
    /// `DISCORD_SANCTIONS_CHANNEL_ID`
    pub discord_sanctions_channel_id: Option<String>,
    /// `DISCORD_SUPPORT_CHANNEL_ID`
    pub discord_support_channel_id: Option<String>,
    /// `DISCORD_MUSIC_CHANNEL_ID`
    pub discord_music_channel_id: Option<String>,
    /// `DISCORD_RECHARGE_CHANNEL_ID`
    pub discord_recharge_channel_id: Option<String>,
    /// `DISCORD_MIXTON_CHAT_CHANNEL_ID`
    pub discord_mixton_chat_channel_id: Option<String>,
    /// `DISCORD_MATCH_ACCESS_ROLE_ID`
    pub discord_match_access_role_id: Option<String>,
    /// `FACEIT_HUB_ROLE_ID`
    pub faceit_hub_role_id: Option<String>,
    /// `DISCORD_HUB_ROLE_ID`
    pub discord_hub_role_id: Option<String>,
    /// `FACEIT_CLUB_ID`
    pub faceit_club_id: Option<String>,
    /// `FACEIT_HUB_ID`
    pub faceit_hub_id: Option<String>,
    /// `FACEIT_LEADERBOARD_ID`
    pub faceit_leaderboard_id: Option<String>,
    /// `MATCH_TIMEOUT_MINUTES`, en crudo (el legacy le aplica `parseInt`).
    pub match_timeout_minutes: Option<String>,
    /// `GAMES_PER_MATCH`, en crudo (el legacy le aplica `Number`).
    pub games_per_match: Option<String>,
    /// `ROUNDS_PER_GAME`, en crudo (alias legado de `GAMES_PER_MATCH`).
    pub rounds_per_game: Option<String>,
    /// Colas de `getQueuesFromEnv`.
    pub queues: Vec<EnvQueue>,
}

/// Lista separada por comas, recortada y sin vacíos.
fn csv(vars: &HashMap<String, String>, key: &str) -> Vec<String> {
    get(vars, key)
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Primera de `keys` con valor.
fn first_of(vars: &HashMap<String, String>, keys: &[String]) -> Option<String> {
    keys.iter().find_map(|k| optional(vars, k))
}

/// Puerto de `getQueuesFromEnv` (`config/index.js` ~68-122).
fn env_queues(vars: &HashMap<String, String>) -> Vec<EnvQueue> {
    let mut queues = Vec::new();
    // 1. Colas numeradas QUEUE1..QUEUE10.
    for i in 1..=10 {
        let pair = |a: String, b: String| first_of(vars, &[a, b]);
        let queue_id = pair(
            format!("FACEIT_QUEUE{i}_ID"),
            format!("FACEIT_QUEUE_{i}_ID"),
        );
        let name = pair(
            format!("DISCORD_QUEUE{i}_NAME"),
            format!("FACEIT_QUEUE{i}_NAME"),
        )
        .unwrap_or_else(|| format!("Cola {i}"));
        let matches = pair(
            format!("DISCORD_QUEUE{i}_MATCHES_CHANNEL_ID"),
            format!("DISCORD_QUEUE_{i}_MATCHES_CHANNEL_ID"),
        );
        let waiting = pair(
            format!("DISCORD_QUEUE{i}_WAITING_ROOM_CHANNEL_ID"),
            format!("DISCORD_QUEUE_{i}_WAITING_ROOM_CHANNEL_ID"),
        );
        let mixton = pair(
            format!("DISCORD_QUEUE{i}_MIXTON_CHANNEL_ID"),
            format!("DISCORD_QUEUE_{i}_MIXTON_CHANNEL_ID"),
        );
        let access = pair(
            format!("DISCORD_QUEUE{i}_MATCH_ACCESS_ROLE_ID"),
            format!("DISCORD_QUEUE_{i}_MATCH_ACCESS_ROLE_ID"),
        );
        let hub_role = first_of(
            vars,
            &[
                format!("FACEIT_QUEUE{i}_HUB_ROLE_ID"),
                format!("FACEIT_QUEUE_{i}_HUB_ROLE_ID"),
                format!("QUEUE{i}_HUB_ROLE_ID"),
                format!("QUEUE_{i}_HUB_ROLE_ID"),
            ],
        );
        let discord_hub_role = pair(
            format!("DISCORD_QUEUE{i}_HUB_ROLE_ID"),
            format!("DISCORD_QUEUE_{i}_HUB_ROLE_ID"),
        );
        if queue_id.is_some() || matches.is_some() || waiting.is_some() {
            let id = queue_id.unwrap_or_else(|| format!("queue_{i}"));
            queues.push(EnvQueue {
                queue_id: id.clone(),
                id,
                name,
                discord_matches_channel_id: matches.unwrap_or_default(),
                discord_waiting_room_channel_id: waiting.unwrap_or_default(),
                discord_mixton_channel_id: mixton.unwrap_or_default(),
                discord_match_access_role_id: access.unwrap_or_default(),
                hub_role_id: Some(hub_role.unwrap_or_default()),
                discord_hub_role_id: discord_hub_role.unwrap_or_default(),
            });
        }
    }
    // 2. Sin colas numeradas: listas separadas por coma en las variables base (más de una).
    if queues.is_empty() {
        let matches = csv(vars, "DISCORD_MATCHES_CHANNEL_ID");
        let waiting = csv(vars, "DISCORD_WAITING_ROOM_CHANNEL_ID");
        let mixton = csv(vars, "DISCORD_MIXTON_CHANNEL_ID");
        let ids = csv(vars, "FACEIT_QUEUE_IDS");
        // Como el legacy: la lista de Mixton no cuenta para la longitud.
        let max_len = matches.len().max(waiting.len()).max(ids.len());
        if max_len > 1 {
            let at = |list: &[String], i: usize| {
                list.get(i)
                    .or_else(|| list.first())
                    .cloned()
                    .unwrap_or_default()
            };
            for i in 0..max_len {
                let id = ids
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("queue_{}", i + 1));
                queues.push(EnvQueue {
                    queue_id: id.clone(),
                    id,
                    name: format!("Cola {}", i + 1),
                    discord_matches_channel_id: at(&matches, i),
                    discord_waiting_room_channel_id: at(&waiting, i),
                    discord_mixton_channel_id: at(&mixton, i),
                    discord_match_access_role_id: optional(vars, "DISCORD_MATCH_ACCESS_ROLE_ID")
                        .unwrap_or_default(),
                    hub_role_id: None,
                    discord_hub_role_id: optional(vars, "DISCORD_HUB_ROLE_ID").unwrap_or_default(),
                });
            }
        }
    }
    queues
}

impl EnvDefaults {
    /// Lee la capa de entorno de un mapa de variables.
    pub fn from_map(vars: &HashMap<String, String>) -> EnvDefaults {
        let o = |key: &str| optional(vars, key);
        EnvDefaults {
            discord_link_channel_id: o("DISCORD_LINK_CHANNEL_ID"),
            discord_matches_channel_id: o("DISCORD_MATCHES_CHANNEL_ID"),
            discord_mixton_channel_id: o("DISCORD_MIXTON_CHANNEL_ID"),
            discord_roles_channel_id: o("DISCORD_ROLES_CHANNEL_ID"),
            discord_status_channel_id: o("DISCORD_STATUS_CHANNEL_ID"),
            discord_admin_alerts_channel_id: o("DISCORD_ADMIN_ALERTS_CHANNEL_ID"),
            discord_waiting_room_channel_id: o("DISCORD_WAITING_ROOM_CHANNEL_ID"),
            discord_sanctions_channel_id: o("DISCORD_SANCTIONS_CHANNEL_ID"),
            discord_support_channel_id: o("DISCORD_SUPPORT_CHANNEL_ID"),
            discord_music_channel_id: o("DISCORD_MUSIC_CHANNEL_ID"),
            discord_recharge_channel_id: o("DISCORD_RECHARGE_CHANNEL_ID"),
            discord_mixton_chat_channel_id: o("DISCORD_MIXTON_CHAT_CHANNEL_ID"),
            discord_match_access_role_id: o("DISCORD_MATCH_ACCESS_ROLE_ID"),
            faceit_hub_role_id: o("FACEIT_HUB_ROLE_ID"),
            discord_hub_role_id: o("DISCORD_HUB_ROLE_ID"),
            faceit_club_id: o("FACEIT_CLUB_ID"),
            faceit_hub_id: o("FACEIT_HUB_ID"),
            faceit_leaderboard_id: o("FACEIT_LEADERBOARD_ID"),
            match_timeout_minutes: o("MATCH_TIMEOUT_MINUTES"),
            games_per_match: o("GAMES_PER_MATCH"),
            rounds_per_game: o("ROUNDS_PER_GAME"),
            queues: env_queues(vars),
        }
    }
}

/// Configuración de la aplicación.
#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub legacy_url: String,
    pub steam_bot_url: String,
    pub static_dir: PathBuf,
    pub admin_jwt_secret: String,
    pub base_url: String,
    pub discord_client_id: String,
    pub discord_client_secret: String,
    pub discord_token: String,
    pub discord_guild_id: String,
    pub cors_origins: Vec<String>,
    pub production: bool,
    /// Confiar en `CF-Connecting-IP` para identificar al cliente (opt-in).
    pub trust_cf_connecting_ip: bool,
    pub cs2_server_public_ip: Option<String>,
    pub mixton_server_ip: Option<String>,
    pub admin_role_id: Option<String>,
    /// Añadir la IP del par TCP a `X-Forwarded-For` en el proxy (por defecto no).
    pub proxy_append_peer_to_xff: bool,
    /// `PROXY_FIRST_BYTE_TIMEOUT_SECS`: tope hasta la cabecera de respuesta de
    /// Node en el proxy HTTP (504 al vencer). `None` (vacío o `0`) = sin tope.
    pub proxy_first_byte_timeout: Option<Duration>,
    /// `WS_IDLE_TIMEOUT_SECS`: cierre de un WebSocket proxificado sin ningún
    /// frame en ninguna dirección. `None` (vacío o `0`) = nunca.
    pub ws_idle_timeout: Option<Duration>,
    pub firestore_project_id: Option<String>,
    pub firebase_service_account_path: Option<PathBuf>,
    pub firestore_emulator_host: Option<String>,
    /// Opt-in explícito a Application Default Credentials.
    pub firestore_use_adc: bool,
    /// Capa de entorno de la configuración efectiva (`getEffectiveConfig`).
    pub env_defaults: EnvDefaults,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("port", &self.port)
            .field("legacy_url", &self.legacy_url)
            .field("steam_bot_url", &self.steam_bot_url)
            .field("static_dir", &self.static_dir)
            .field("admin_jwt_secret", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("discord_client_id", &self.discord_client_id)
            .field("discord_client_secret", &"<redacted>")
            .field("discord_token", &"<redacted>")
            .field("discord_guild_id", &self.discord_guild_id)
            .field("cors_origins", &self.cors_origins)
            .field("production", &self.production)
            .field("trust_cf_connecting_ip", &self.trust_cf_connecting_ip)
            .field("cs2_server_public_ip", &self.cs2_server_public_ip)
            .field("mixton_server_ip", &self.mixton_server_ip)
            .field("admin_role_id", &self.admin_role_id)
            .field("proxy_append_peer_to_xff", &self.proxy_append_peer_to_xff)
            .field("proxy_first_byte_timeout", &self.proxy_first_byte_timeout)
            .field("ws_idle_timeout", &self.ws_idle_timeout)
            .field("firestore_project_id", &self.firestore_project_id)
            .field(
                "firebase_service_account_path",
                &self.firebase_service_account_path,
            )
            .field("firestore_emulator_host", &self.firestore_emulator_host)
            .field("firestore_use_adc", &self.firestore_use_adc)
            .field("env_defaults", &self.env_defaults)
            .finish()
    }
}

/// Valor no vacío (tras recortar espacios) o `None`.
fn get<'a>(vars: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    vars.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

fn required(vars: &HashMap<String, String>, key: &'static str) -> Result<String, ConfigError> {
    get(vars, key)
        .map(str::to_owned)
        .ok_or(ConfigError::Missing(key))
}

fn optional(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    get(vars, key).map(str::to_owned)
}

fn with_default(vars: &HashMap<String, String>, key: &str, default: &str) -> String {
    optional(vars, key).unwrap_or_else(|| default.to_owned())
}

/// Booleano opcional: ausente o vacío = `false`; valor desconocido = error.
fn flag(vars: &HashMap<String, String>, key: &'static str) -> Result<bool, ConfigError> {
    match get(vars, key) {
        None => Ok(false),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(ConfigError::Invalid(key, v.to_owned())),
        },
    }
}

/// Segundos opcionales: ausente, vacío o `0` = `None` (desactivado); un entero
/// no negativo en base 10 = `Some`; cualquier otra cosa = error.
fn seconds(
    vars: &HashMap<String, String>,
    key: &'static str,
) -> Result<Option<Duration>, ConfigError> {
    let Some(v) = get(vars, key) else {
        return Ok(None);
    };
    // Solo dígitos: `u64::from_str` aceptaría también `+5`.
    let n = v
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| v.parse::<u64>().ok())
        .flatten()
        .ok_or_else(|| ConfigError::Invalid(key, v.to_owned()))?;
    Ok((n > 0).then(|| Duration::from_secs(n)))
}

/// Equivalente a `new URL(base).origin` para URLs http(s).
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?.to_ascii_lowercase();
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    let host_port = host_port.strip_suffix(default_port).unwrap_or(&host_port);
    if host_port.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host_port}"))
}

impl Config {
    /// Construye la configuración a partir de un mapa de variables.
    pub fn from_map(vars: &HashMap<String, String>) -> Result<Config, ConfigError> {
        // El legacy usa el valor crudo de `process.env`: no se recorta (un
        // secreto con espacios en los bordes firmaría distinto). Solo se
        // rechaza si queda vacío tras recortar.
        let admin_jwt_secret = match vars.get("ADMIN_JWT_SECRET") {
            Some(v) if !v.trim().is_empty() => v.clone(),
            _ => return Err(ConfigError::Missing("ADMIN_JWT_SECRET")),
        };
        // config/index.js:28: `.replace(/\/+$/, '')`, una sola vez aquí.
        let raw_base_url = required(vars, "BASE_URL")?;
        let base_url = raw_base_url.trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(ConfigError::Invalid("BASE_URL", raw_base_url));
        }
        let discord_client_id = required(vars, "DISCORD_CLIENT_ID")?;
        let discord_client_secret = required(vars, "DISCORD_CLIENT_SECRET")?;
        let discord_token = required(vars, "DISCORD_TOKEN")?;
        let discord_guild_id = required(vars, "DISCORD_GUILD_ID")?;

        let port = match get(vars, "PORT") {
            None => 15633,
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| ConfigError::Invalid("PORT", p.to_owned()))?,
        };

        let node_production = get(vars, "NODE_ENV") == Some("production");
        let production = node_production || base_url.starts_with("https");

        let trust_cf_connecting_ip = flag(vars, "TRUST_CF_CONNECTING_IP")?;
        let proxy_append_peer_to_xff = flag(vars, "PROXY_APPEND_PEER_TO_XFF")?;
        let firestore_use_adc = flag(vars, "FIRESTORE_USE_ADC")?;
        let proxy_first_byte_timeout = seconds(vars, "PROXY_FIRST_BYTE_TIMEOUT_SECS")?;
        let ws_idle_timeout = seconds(vars, "WS_IDLE_TIMEOUT_SECS")?;

        let raw_origins = get(vars, "CORS_ORIGINS")
            .or_else(|| get(vars, "ADMIN_ALLOWED_ORIGINS"))
            .unwrap_or("");
        let mut cors_origins: Vec<String> = raw_origins
            .split(',')
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(str::to_owned)
            .collect();
        // Como index.js: el respaldo solo aplica con NODE_ENV=production.
        if cors_origins.is_empty() && node_production {
            let origin = origin_of(&base_url)
                .ok_or_else(|| ConfigError::Invalid("BASE_URL", base_url.clone()))?;
            cors_origins.push(origin);
        }

        Ok(Config {
            port,
            legacy_url: with_default(vars, "LEGACY_URL", "http://127.0.0.1:3001"),
            steam_bot_url: with_default(vars, "STEAM_BOT_URL", "http://127.0.0.1:15634"),
            static_dir: PathBuf::from(with_default(vars, "STATIC_DIR", "public")),
            admin_jwt_secret,
            base_url,
            discord_client_id,
            discord_client_secret,
            discord_token,
            discord_guild_id,
            cors_origins,
            production,
            trust_cf_connecting_ip,
            cs2_server_public_ip: optional(vars, "CS2_SERVER_PUBLIC_IP"),
            mixton_server_ip: optional(vars, "MIXTON_SERVER_IP"),
            // config/index.js: ADMIN_ROLE_ID || ADMIN_ROLE_NAME.
            admin_role_id: optional(vars, "ADMIN_ROLE_ID")
                .or_else(|| optional(vars, "ADMIN_ROLE_NAME")),
            proxy_append_peer_to_xff,
            proxy_first_byte_timeout,
            ws_idle_timeout,
            firestore_project_id: optional(vars, "FIRESTORE_PROJECT_ID"),
            firebase_service_account_path: optional(vars, "FIREBASE_SERVICE_ACCOUNT_PATH")
                .map(PathBuf::from),
            firestore_emulator_host: optional(vars, "FIRESTORE_EMULATOR_HOST"),
            firestore_use_adc,
            env_defaults: EnvDefaults::from_map(vars),
        })
    }

    /// Carga `.env` (si existe) y construye la configuración desde el entorno.
    pub fn from_env() -> Result<Config, ConfigError> {
        let _ = dotenvy::dotenv();
        let vars: HashMap<String, String> = std::env::vars().collect();
        Config::from_map(&vars)
    }
}
