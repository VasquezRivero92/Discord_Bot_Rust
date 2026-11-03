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
        let base_url = required(vars, "BASE_URL")?;
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

        let trust_cf_connecting_ip = match get(vars, "TRUST_CF_CONNECTING_IP") {
            None => false,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => return Err(ConfigError::Invalid("TRUST_CF_CONNECTING_IP", v.to_owned())),
            },
        };

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
            admin_role_id: optional(vars, "ADMIN_ROLE_ID"),
        })
    }

    /// Carga `.env` (si existe) y construye la configuración desde el entorno.
    pub fn from_env() -> Result<Config, ConfigError> {
        let _ = dotenvy::dotenv();
        let vars: HashMap<String, String> = std::env::vars().collect();
        Config::from_map(&vars)
    }
}
