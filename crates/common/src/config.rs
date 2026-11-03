//! Configuración tipada leída del entorno (y de `.env` si existe).

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

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
    pub firestore_project_id: Option<String>,
    pub firebase_service_account_path: Option<PathBuf>,
    pub firestore_emulator_host: Option<String>,
    /// Opt-in explícito a Application Default Credentials.
    pub firestore_use_adc: bool,
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
            .field("firestore_project_id", &self.firestore_project_id)
            .field(
                "firebase_service_account_path",
                &self.firebase_service_account_path,
            )
            .field("firestore_emulator_host", &self.firestore_emulator_host)
            .field("firestore_use_adc", &self.firestore_use_adc)
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
        let admin_jwt_secret = required(vars, "ADMIN_JWT_SECRET")?;
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
            firestore_project_id: optional(vars, "FIRESTORE_PROJECT_ID"),
            firebase_service_account_path: optional(vars, "FIREBASE_SERVICE_ACCOUNT_PATH")
                .map(PathBuf::from),
            firestore_emulator_host: optional(vars, "FIRESTORE_EMULATOR_HOST"),
            firestore_use_adc,
        })
    }

    /// Carga `.env` (si existe) y construye la configuración desde el entorno.
    pub fn from_env() -> Result<Config, ConfigError> {
        let _ = dotenvy::dotenv();
        let vars: HashMap<String, String> = std::env::vars().collect();
        Config::from_map(&vars)
    }
}
