//! Acceso a la API REST de Discord (v10) y comprobación de administrador.
//!
//! Piezas para la fase siguiente (el callback OAuth sigue en el proxy, ver `routes::auth`):
//!
//! * [`DiscordApi`]: costura para pruebas; [`RestDiscordApi`] es la implementación real y
//!   [`testing::FakeDiscordApi`] el doble.
//! * [`compute_permissions`] / [`has_administrator`] / [`is_admin`]: reproducen
//!   `services/adminCheck.js` (`checkIsAdmin`).
//!
//! # Equivalencia con `member.permissions.has(Administrator)` de discord.js
//!
//! El objeto miembro de la API REST no trae `permissions`; discord.js lo calcula así y aquí se
//! repite: se parte de los permisos del rol `@everyone` (su id es el id del servidor), se hace OR
//! con los de cada rol del miembro (campos `permissions`, cadenas decimales en la API), y el dueño
//! del servidor lo tiene todo. `has(Administrator)` es cierto si el bit `1 << 3` está activo (o
//! si es el dueño). No se modelan los *overwrites* de canal: `checkIsAdmin` solo consulta los
//! permisos de servidor.
//!
//! # Seguridad
//!
//! * El `access_token` OAuth del usuario solo vive dentro de `exchange_code`: no se registra, no
//!   se guarda y no aparece en ningún error.
//! * Los errores ([`DiscordError`]) tienen textos fijos: nunca incluyen cuerpos de respuesta,
//!   URLs con credenciales ni tokens. `Debug` de [`RestDiscordApi`] oculta secretos.
//! * Los ids que se interpolan en rutas se validan (solo dígitos) para impedir inyección de ruta.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use data::GuildSettings;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use reqwest::redirect::Policy;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use thiserror::Error;

/// Base de la API REST de Discord.
pub const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
/// Permiso `ADMINISTRATOR` (`1 << 3`).
pub const PERM_ADMINISTRATOR: u64 = 1 << 3;

/// Máximo de bytes que se leen de una respuesta de Discord.
const MAX_BODY: usize = 256 * 1024;

/// Error al hablar con Discord. Los textos son fijos: no llevan datos de la respuesta.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiscordError {
    /// No se pudo conectar, agotó el tiempo o se cortó la respuesta.
    #[error("discord transport error")]
    Transport,
    /// La respuesta no tiene la forma esperada.
    #[error("discord returned a malformed response")]
    Malformed,
    /// El `code` OAuth ya se usó o expiró (`invalid_grant`).
    #[error("discord rejected the authorization code (invalid_grant)")]
    InvalidGrant,
    /// Cloudflare bloqueó la petición (error 1015).
    #[error("discord request blocked by cloudflare")]
    CloudflareBlocked,
    /// HTTP 429 con `Retry-After`.
    #[error("discord rate limited, retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    /// Cualquier otro estado HTTP inesperado.
    #[error("discord returned HTTP {0}")]
    Status(u16),
    /// Argumento rechazado antes de salir a la red (id con caracteres no válidos).
    #[error("invalid argument")]
    InvalidArgument,
}

/// Usuario autenticado por OAuth (`/users/@me`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
}

/// Miembro de un servidor. La API REST no trae `permissions`: se calculan con [`compute_permissions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildMember {
    pub user_id: String,
    pub roles: Vec<String>,
}

/// Rol de un servidor. `permissions` es la cadena decimal de la API.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Role {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "permissions_string")]
    pub permissions: String,
}

/// La API manda cadenas; por tolerancia también se aceptan números.
fn permissions_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

/// Operaciones de Discord que necesita la web.
#[async_trait]
pub trait DiscordApi: Send + Sync {
    /// Cambia el `code` OAuth por el usuario (`/oauth2/token` + `/users/@me`). El token de acceso
    /// no se conserva.
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<DiscordUser, DiscordError>;

    /// `GET /guilds/{id}/members/{uid}` con el token del bot; 404 -> `Ok(None)`.
    async fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<GuildMember>, DiscordError>;

    /// `GET /guilds/{id}/roles`.
    async fn guild_roles(&self, guild_id: &str) -> Result<Vec<Role>, DiscordError>;

    /// `owner_id` de `GET /guilds/{id}` (el dueño cuenta como administrador).
    async fn guild_owner_id(&self, guild_id: &str) -> Result<String, DiscordError>;
}

/// Permisos efectivos de servidor de `member`: OR de `@everyone` y de sus roles. Las cadenas no
/// numéricas, negativas o que no caben en `u64` cuentan como 0; los roles desconocidos se ignoran.
pub fn compute_permissions(guild_id: &str, member: &GuildMember, roles: &[Role]) -> u64 {
    roles
        .iter()
        .filter(|r| r.id == guild_id || member.roles.contains(&r.id))
        .fold(0u64, |acc, r| {
            acc | r.permissions.parse::<u64>().unwrap_or(0)
        })
}

/// `true` si los roles del miembro (o `@everyone`) tienen `ADMINISTRATOR`.
pub fn has_administrator(guild_id: &str, member: &GuildMember, roles: &[Role]) -> bool {
    compute_permissions(guild_id, member, roles) & PERM_ADMINISTRATOR != 0
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

/// Fuentes del rol de administración. El llamador resuelve la capa global (ver abajo).
///
/// Cadena del legacy (`config/index.js` `getEffectiveConfigForGuild` + `adminCheck.js`):
///
/// * `guild = guildSettings.adminRoleId || guildSettings.adminRoleName`
/// * servidor por defecto (`guildId === globalConfig.discordGuildId`):
///   `guild || globalConfig.adminRoleId`, con `globalConfig.adminRoleId =
///   settings.adminRoleId || settings.adminRoleName || process.env.ADMIN_ROLE_ID ||
///   env.ADMIN_ROLE_ID` (`settings` = documento global `faceit_config/settings`)
/// * cualquier otro servidor: solo `guild` (aislamiento estricto, sin capa global)
/// * `checkIsAdmin` añade al final `|| env.ADMIN_ROLE_ID` (`ADMIN_ROLE_ID || ADMIN_ROLE_NAME` del
///   entorno) para TODOS los servidores.
///
/// Las cadenas vacías cuentan como ausentes (`||` de JS).
///
/// **El llamador debe aportar `global_admin_role`**, ya resuelto como
/// `settings.adminRoleId || settings.adminRoleName || env`. Todavía no hay repo de
/// `faceit_config/settings` en `data`; el plan de la fase 2 lo añade, y hasta entonces el
/// llamador puede pasar solo la parte de entorno (o `None`).
#[derive(Debug, Clone, Copy, Default)]
pub struct AdminRoleSources<'a> {
    /// Ajustes del servidor (`GuildSettingsRepo`), si existen.
    pub guild_settings: Option<&'a GuildSettings>,
    /// Capa global ya resuelta (`globalConfig.adminRoleId`); solo cuenta si `is_default_guild`.
    pub global_admin_role: Option<&'a str>,
    /// `guildId === globalConfig.discordGuildId`.
    pub is_default_guild: bool,
    /// `env.ADMIN_ROLE_ID` (`Config::admin_role_id`).
    pub env_admin_role_id: Option<&'a str>,
}

/// Rol de administración efectivo según la cadena documentada en [`AdminRoleSources`].
pub fn effective_admin_role(src: &AdminRoleSources<'_>) -> Option<String> {
    let guild =
        non_empty(src.guild_settings.and_then(|s| s.admin_role_id.as_deref())).or_else(|| {
            non_empty(
                src.guild_settings
                    .and_then(|s| s.admin_role_name.as_deref()),
            )
        });
    let global = if src.is_default_guild {
        non_empty(src.global_admin_role)
    } else {
        None
    };
    guild
        .or(global)
        .or_else(|| non_empty(src.env_admin_role_id))
        .map(str::to_owned)
}

/// Reproduce `checkIsAdmin`: administrador de Discord (o dueño) -> `true`; si no, el miembro debe
/// tener el rol de administración por id o, como respaldo, por nombre sin distinguir mayúsculas.
///
/// Como `member.roles.cache` de discord.js, el conjunto de roles del miembro incluye `@everyone`
/// (id == `guild_id`).
pub fn is_admin(
    member: &GuildMember,
    roles: &[Role],
    guild_id: &str,
    is_guild_owner: bool,
    sources: &AdminRoleSources<'_>,
) -> bool {
    if is_guild_owner || has_administrator(guild_id, member, roles) {
        return true;
    }
    let Some(wanted) = effective_admin_role(sources) else {
        return false;
    };
    let has_id = |id: &str| id == guild_id || member.roles.iter().any(|m| m == id);
    if has_id(&wanted) {
        return true;
    }
    let wanted_upper = wanted.to_uppercase();
    roles
        .iter()
        .filter(|r| has_id(&r.id))
        .any(|r| !r.name.is_empty() && r.name.to_uppercase() == wanted_upper)
}

/// Resuelve todo por red y aplica [`is_admin`] (equivale a `checkDiscordAdmin`): `Ok(false)` si el
/// usuario no es miembro.
pub async fn check_discord_admin(
    api: &dyn DiscordApi,
    guild_id: &str,
    user_id: &str,
    sources: &AdminRoleSources<'_>,
) -> Result<bool, DiscordError> {
    let Some(member) = api.guild_member(guild_id, user_id).await? else {
        return Ok(false);
    };
    let roles = api.guild_roles(guild_id).await?;
    let owner = api.guild_owner_id(guild_id).await?;
    Ok(is_admin(
        &member,
        &roles,
        guild_id,
        owner == user_id,
        sources,
    ))
}

/// Implementación real sobre HTTP.
#[derive(Clone)]
pub struct RestDiscordApi {
    http: reqwest::Client,
    api_base: String,
    client_id: String,
    client_secret: String,
    bot_token: String,
    user_agent: String,
}

impl fmt::Debug for RestDiscordApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RestDiscordApi")
            .field("api_base", &self.api_base)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("bot_token", &"<redacted>")
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

impl RestDiscordApi {
    /// `site_url` es `BASE_URL`; el User-Agent es el del legacy:
    /// `DiscordBot (${BASE_URL || 'https://discord.com'}, 1.0.0)` (evita el bloqueo 1015).
    pub fn new(client_id: &str, client_secret: &str, bot_token: &str, site_url: &str) -> Self {
        let site = if site_url.is_empty() {
            "https://discord.com"
        } else {
            site_url
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .build()
            .expect("cliente HTTP de Discord");
        Self {
            http,
            api_base: DISCORD_API_BASE.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            bot_token: bot_token.to_owned(),
            user_agent: format!("DiscordBot ({site}, 1.0.0)"),
        }
    }

    /// Construye desde la configuración.
    pub fn from_config(cfg: &common::config::Config) -> Self {
        Self::new(
            &cfg.discord_client_id,
            &cfg.discord_client_secret,
            &cfg.discord_token,
            &cfg.base_url,
        )
    }

    /// Cambia la base de la API (para apuntar a un doble local en pruebas).
    pub fn with_base_url(mut self, api_base: &str) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_owned();
        self
    }

    fn bot_get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{}", self.api_base, path))
            .header(USER_AGENT, &self.user_agent)
            .header(AUTHORIZATION, format!("Bot {}", self.bot_token))
    }
}

fn digits_only(id: &str) -> Result<&str, DiscordError> {
    if !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(DiscordError::InvalidArgument)
    }
}

/// Lee el cuerpo con tope de tamaño.
async fn read_capped(mut res: reqwest::Response) -> Result<Vec<u8>, DiscordError> {
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|_| DiscordError::Transport)? {
        if out.len() + chunk.len() > MAX_BODY {
            return Err(DiscordError::Malformed);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn retry_after(res: &reqwest::Response) -> Duration {
    res.headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| Duration::from_secs_f64(s.min(86_400.0)))
        .unwrap_or(Duration::from_secs(1))
}

/// Traduce un estado no exitoso.
fn status_error(res: &reqwest::Response) -> DiscordError {
    if res.status().as_u16() == 429 {
        DiscordError::RateLimited {
            retry_after: retry_after(res),
        }
    } else {
        DiscordError::Status(res.status().as_u16())
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, DiscordError> {
    serde_json::from_slice(body).map_err(|_| DiscordError::Malformed)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct RawMember {
    #[serde(default)]
    user: Option<RawMemberUser>,
    #[serde(default)]
    roles: Vec<String>,
}

#[derive(Deserialize)]
struct RawMemberUser {
    id: String,
}

#[derive(Deserialize)]
struct RawGuild {
    owner_id: String,
}

#[async_trait]
impl DiscordApi for RestDiscordApi {
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<DiscordUser, DiscordError> {
        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", &self.client_id)
            .append_pair("client_secret", &self.client_secret)
            .append_pair("grant_type", "authorization_code")
            .append_pair("code", code)
            .append_pair("redirect_uri", redirect_uri)
            .finish();
        let res = self
            .http
            .post(format!("{}/oauth2/token", self.api_base))
            .header(USER_AGENT, &self.user_agent)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .map_err(|_| DiscordError::Transport)?;

        if !res.status().is_success() {
            let err = status_error(&res);
            let body = read_capped(res).await.unwrap_or_default();
            let text = String::from_utf8_lossy(&body);
            // Mismo orden que el legacy: Cloudflare, luego invalid_grant.
            if text.contains("1015") || text.contains("cf-wrapper") || text.contains("Cloudflare") {
                return Err(DiscordError::CloudflareBlocked);
            }
            if text.contains("invalid_grant") {
                return Err(DiscordError::InvalidGrant);
            }
            return Err(err);
        }
        let token: TokenResponse = parse(&read_capped(res).await?)?;

        let res = self
            .http
            .get(format!("{}/users/@me", self.api_base))
            .header(USER_AGENT, &self.user_agent)
            .header(AUTHORIZATION, format!("Bearer {}", token.access_token))
            .send()
            .await
            .map_err(|_| DiscordError::Transport)?;
        if !res.status().is_success() {
            return Err(status_error(&res));
        }
        parse(&read_capped(res).await?)
    }

    async fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<GuildMember>, DiscordError> {
        let (g, u) = (digits_only(guild_id)?, digits_only(user_id)?);
        let res = self
            .bot_get(&format!("/guilds/{g}/members/{u}"))
            .send()
            .await
            .map_err(|_| DiscordError::Transport)?;
        if res.status().as_u16() == 404 {
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(status_error(&res));
        }
        let raw: RawMember = parse(&read_capped(res).await?)?;
        Ok(Some(GuildMember {
            user_id: raw.user.map(|x| x.id).unwrap_or_else(|| u.to_owned()),
            roles: raw.roles,
        }))
    }

    async fn guild_roles(&self, guild_id: &str) -> Result<Vec<Role>, DiscordError> {
        let g = digits_only(guild_id)?;
        let res = self
            .bot_get(&format!("/guilds/{g}/roles"))
            .send()
            .await
            .map_err(|_| DiscordError::Transport)?;
        if !res.status().is_success() {
            return Err(status_error(&res));
        }
        parse(&read_capped(res).await?)
    }

    async fn guild_owner_id(&self, guild_id: &str) -> Result<String, DiscordError> {
        let g = digits_only(guild_id)?;
        let res = self
            .bot_get(&format!("/guilds/{g}"))
            .send()
            .await
            .map_err(|_| DiscordError::Transport)?;
        if !res.status().is_success() {
            return Err(status_error(&res));
        }
        let raw: RawGuild = parse(&read_capped(res).await?)?;
        Ok(raw.owner_id)
    }
}

/// Doble de pruebas con respuestas guionizadas y registro de llamadas.
pub mod testing {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{DiscordApi, DiscordError, DiscordUser, GuildMember, Role};

    #[derive(Default)]
    struct Script {
        user: Option<Result<DiscordUser, DiscordError>>,
        members: HashMap<(String, String), Result<Option<GuildMember>, DiscordError>>,
        roles: HashMap<String, Result<Vec<Role>, DiscordError>>,
        owners: HashMap<String, Result<String, DiscordError>>,
        calls: Vec<String>,
    }

    /// Doble de [`DiscordApi`]. Lo no guionizado responde `Err(Transport)` (o `Ok(None)` para
    /// miembros), nunca entra en pánico.
    #[derive(Default)]
    pub struct FakeDiscordApi {
        inner: Mutex<Script>,
    }

    impl FakeDiscordApi {
        pub fn new() -> Self {
            Self::default()
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Script> {
            self.inner.lock().unwrap_or_else(|e| e.into_inner())
        }

        pub fn with_user(self, r: Result<DiscordUser, DiscordError>) -> Self {
            self.lock().user = Some(r);
            self
        }

        pub fn with_member(
            self,
            guild: &str,
            user: &str,
            r: Result<Option<GuildMember>, DiscordError>,
        ) -> Self {
            self.lock().members.insert((guild.into(), user.into()), r);
            self
        }

        pub fn with_roles(self, guild: &str, r: Result<Vec<Role>, DiscordError>) -> Self {
            self.lock().roles.insert(guild.into(), r);
            self
        }

        pub fn with_owner(self, guild: &str, r: Result<String, DiscordError>) -> Self {
            self.lock().owners.insert(guild.into(), r);
            self
        }

        /// Llamadas hechas, en orden (`exchange_code`, `guild_member:g:u`, ...). Nunca incluyen el
        /// `code` ni tokens.
        pub fn calls(&self) -> Vec<String> {
            self.lock().calls.clone()
        }
    }

    #[async_trait]
    impl DiscordApi for FakeDiscordApi {
        async fn exchange_code(
            &self,
            _code: &str,
            redirect_uri: &str,
        ) -> Result<DiscordUser, DiscordError> {
            let mut s = self.lock();
            s.calls.push(format!("exchange_code:{redirect_uri}"));
            s.user.clone().unwrap_or(Err(DiscordError::Transport))
        }

        async fn guild_member(
            &self,
            guild_id: &str,
            user_id: &str,
        ) -> Result<Option<GuildMember>, DiscordError> {
            let mut s = self.lock();
            s.calls.push(format!("guild_member:{guild_id}:{user_id}"));
            s.members
                .get(&(guild_id.to_owned(), user_id.to_owned()))
                .cloned()
                .unwrap_or(Ok(None))
        }

        async fn guild_roles(&self, guild_id: &str) -> Result<Vec<Role>, DiscordError> {
            let mut s = self.lock();
            s.calls.push(format!("guild_roles:{guild_id}"));
            s.roles
                .get(guild_id)
                .cloned()
                .unwrap_or(Err(DiscordError::Transport))
        }

        async fn guild_owner_id(&self, guild_id: &str) -> Result<String, DiscordError> {
            let mut s = self.lock();
            s.calls.push(format!("guild_owner_id:{guild_id}"));
            s.owners
                .get(guild_id)
                .cloned()
                .unwrap_or(Err(DiscordError::Transport))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use data::GuildSettings;

    const GUILD: &str = "100";

    fn role(id: &str, name: &str, perms: &str) -> Role {
        Role {
            id: id.into(),
            name: name.into(),
            permissions: perms.into(),
        }
    }

    fn member(roles: &[&str]) -> GuildMember {
        GuildMember {
            user_id: "42".into(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    fn settings(id: Option<&str>, name: Option<&str>) -> GuildSettings {
        GuildSettings {
            admin_role_id: id.map(str::to_owned),
            admin_role_name: name.map(str::to_owned),
            ..Default::default()
        }
    }

    /// Atajo: servidor que no es el de por defecto, sin capa global.
    fn is_admin(
        m: &GuildMember,
        r: &[Role],
        g: &str,
        owner: bool,
        s: Option<&GuildSettings>,
        env: Option<&str>,
    ) -> bool {
        super::is_admin(m, r, g, owner, &src(s, None, false, env))
    }

    fn src<'a>(
        s: Option<&'a GuildSettings>,
        global: Option<&'a str>,
        default_guild: bool,
        env: Option<&'a str>,
    ) -> AdminRoleSources<'a> {
        AdminRoleSources {
            guild_settings: s,
            global_admin_role: global,
            is_default_guild: default_guild,
            env_admin_role_id: env,
        }
    }

    fn eff(
        s: Option<&GuildSettings>,
        global: Option<&str>,
        default_guild: bool,
        env: Option<&str>,
    ) -> Option<String> {
        effective_admin_role(&src(s, global, default_guild, env))
    }

    #[test]
    fn guild_admin_role_id_wins_over_global_and_env() {
        let s = settings(Some("1"), Some("n"));
        assert_eq!(
            eff(Some(&s), Some("g"), true, Some("e")).as_deref(),
            Some("1")
        );
        assert_eq!(
            eff(Some(&s), Some("g"), false, Some("e")).as_deref(),
            Some("1")
        );
    }

    #[test]
    fn guild_admin_role_name_used_when_id_absent_or_empty() {
        for s in [settings(None, Some("n")), settings(Some(""), Some("n"))] {
            assert_eq!(
                eff(Some(&s), Some("g"), true, Some("e")).as_deref(),
                Some("n")
            );
        }
    }

    #[test]
    fn global_layer_applies_only_to_the_default_guild() {
        let empty = settings(None, None);
        assert_eq!(
            eff(Some(&empty), Some("g"), true, Some("e")).as_deref(),
            Some("g")
        );
        assert_eq!(eff(None, Some("g"), true, Some("e")).as_deref(), Some("g"));
        // otro servidor: aislamiento estricto, cae directo al entorno (checkIsAdmin)
        assert_eq!(
            eff(Some(&empty), Some("g"), false, Some("e")).as_deref(),
            Some("e")
        );
        assert_eq!(eff(None, Some("g"), false, None), None);
    }

    #[test]
    fn env_is_the_last_resort_for_every_guild() {
        assert_eq!(eff(None, None, true, Some("e")).as_deref(), Some("e"));
        assert_eq!(eff(None, Some(""), true, Some("e")).as_deref(), Some("e"));
        assert_eq!(eff(None, None, false, Some("e")).as_deref(), Some("e"));
    }

    #[test]
    fn empty_strings_count_as_absent() {
        let s = settings(Some(""), Some(""));
        assert_eq!(eff(Some(&s), Some(""), true, Some("")), None);
        assert_eq!(eff(None, None, false, Some("")), None);
        assert_eq!(eff(None, None, false, None), None);
    }

    #[test]
    fn is_admin_uses_the_global_layer_for_the_default_guild_only() {
        let roles = vec![role("9", "Staff", "0")];
        let m = member(&["9"]);
        assert!(super::is_admin(
            &m,
            &roles,
            GUILD,
            false,
            &src(None, Some("9"), true, None)
        ));
        assert!(!super::is_admin(
            &m,
            &roles,
            GUILD,
            false,
            &src(None, Some("9"), false, None)
        ));
        // nombre global, sin distinguir mayusculas
        assert!(super::is_admin(
            &m,
            &roles,
            GUILD,
            false,
            &src(None, Some("STAFF"), true, None)
        ));
    }

    #[test]
    fn administrator_permission_short_circuits_without_any_role_configured() {
        let roles = vec![role("9", "Boss", "8")];
        assert!(super::is_admin(
            &member(&["9"]),
            &roles,
            GUILD,
            false,
            &AdminRoleSources::default()
        ));
    }

    #[test]
    fn permissions_or_everyone_and_member_roles() {
        let roles = vec![
            role(GUILD, "@everyone", "1024"),
            role("1", "a", "2"),
            role("2", "b", "4"),
            role("3", "c", "65536"),
        ];
        let p = compute_permissions(GUILD, &member(&["1", "2"]), &roles);
        assert_eq!(p, 1024 | 2 | 4);
    }

    #[test]
    fn administrator_bit_is_1_shl_3() {
        assert_eq!(PERM_ADMINISTRATOR, 8);
        let roles = vec![role(GUILD, "@everyone", "0"), role("1", "mod", "8")];
        assert!(has_administrator(GUILD, &member(&["1"]), &roles));
        assert!(!has_administrator(GUILD, &member(&[]), &roles));
    }

    #[test]
    fn everyone_role_with_administrator_applies_to_everybody() {
        let roles = vec![role(GUILD, "@everyone", "8")];
        assert!(has_administrator(GUILD, &member(&[]), &roles));
    }

    #[test]
    fn permissions_edge_cases_do_not_panic() {
        assert_eq!(compute_permissions(GUILD, &member(&[]), &[]), 0);
        let roles = vec![role("1", "a", "8")];
        // rol desconocido: se ignora
        assert_eq!(compute_permissions(GUILD, &member(&["999"]), &roles), 0);
        // cadenas invalidas, vacias, negativas o desbordadas cuentan como 0
        for bad in [
            "",
            "abc",
            "-1",
            "18446744073709551616",
            "99999999999999999999999",
            " 8",
            "8 ",
            "0x8",
            "8.0",
        ] {
            let roles = vec![role("1", "a", bad)];
            assert_eq!(
                compute_permissions(GUILD, &member(&["1"]), &roles),
                0,
                "{bad:?}"
            );
        }
        // u64::MAX es valido (contiene el bit Administrator)
        let roles = vec![role("1", "a", "18446744073709551615")];
        assert_eq!(
            compute_permissions(GUILD, &member(&["1"]), &roles),
            u64::MAX
        );
        assert!(has_administrator(GUILD, &member(&["1"]), &roles));
    }

    #[test]
    fn is_admin_true_with_administrator_permission() {
        let roles = vec![role(GUILD, "@everyone", "0"), role("1", "Boss", "8")];
        assert!(is_admin(&member(&["1"]), &roles, GUILD, false, None, None));
    }

    #[test]
    fn is_admin_true_for_guild_owner_without_roles() {
        assert!(is_admin(&member(&[]), &[], GUILD, true, None, None));
    }

    #[test]
    fn is_admin_true_with_role_id_match() {
        let roles = vec![role("555", "Staff", "0")];
        let s = settings(Some("555"), None);
        assert!(is_admin(
            &member(&["555"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            None
        ));
        // aunque el rol no este en la lista de roles del servidor, el id basta (como roles.cache.has)
        assert!(is_admin(
            &member(&["555"]),
            &[],
            GUILD,
            false,
            Some(&s),
            None
        ));
    }

    #[test]
    fn is_admin_true_with_role_name_case_insensitive() {
        let roles = vec![role("7", "Mixton Admin", "0")];
        let s = settings(Some("MIXTON admin"), None);
        assert!(is_admin(
            &member(&["7"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            None
        ));
    }

    #[test]
    fn is_admin_name_only_counts_roles_the_member_has() {
        let roles = vec![role("7", "Staff", "0"), role("8", "Other", "0")];
        let s = settings(Some("staff"), None);
        assert!(!is_admin(
            &member(&["8"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            None
        ));
    }

    #[test]
    fn is_admin_false_without_role() {
        let roles = vec![role(GUILD, "@everyone", "1024"), role("7", "Staff", "0")];
        let s = settings(Some("555"), None);
        assert!(!is_admin(
            &member(&["7"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            None
        ));
    }

    #[test]
    fn is_admin_false_when_no_admin_role_configured_anywhere() {
        let roles = vec![role("7", "Staff", "0")];
        assert!(!is_admin(&member(&["7"]), &roles, GUILD, false, None, None));
        let s = settings(Some(""), Some(""));
        assert!(!is_admin(
            &member(&["7"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            Some("")
        ));
    }

    #[test]
    fn env_fallback_is_used_when_settings_have_no_role() {
        let roles = vec![role("9", "Staff", "0")];
        let s = settings(None, None);
        assert!(is_admin(
            &member(&["9"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            Some("9")
        ));
        assert!(is_admin(
            &member(&["9"]),
            &roles,
            GUILD,
            false,
            None,
            Some("staff")
        ));
    }

    #[test]
    fn settings_role_wins_over_env() {
        let roles = vec![role("9", "Staff", "0")];
        let s = settings(Some("1"), None);
        assert!(!is_admin(
            &member(&["9"]),
            &roles,
            GUILD,
            false,
            Some(&s),
            Some("9")
        ));
    }

    #[test]
    fn everyone_role_counts_like_discordjs_cache() {
        // roles.cache de discord.js incluye @everyone (id == guild id).
        let s = settings(Some(GUILD), None);
        assert!(is_admin(&member(&[]), &[], GUILD, false, Some(&s), None));
        let roles = vec![role(GUILD, "@everyone", "0")];
        let s = settings(Some("@EVERYONE"), None);
        assert!(is_admin(&member(&[]), &roles, GUILD, false, Some(&s), None));
    }

    #[test]
    fn debug_never_prints_secrets() {
        let api = RestDiscordApi::new(
            "cid-1",
            "SECRET-CLIENT",
            "SECRET-BOT-TOKEN",
            "https://machupeek.example",
        );
        let d = format!("{api:?}");
        assert!(
            !d.contains("SECRET-CLIENT") && !d.contains("SECRET-BOT-TOKEN"),
            "{d}"
        );
        assert!(d.contains("cid-1"));
    }

    #[test]
    fn errors_do_not_carry_payloads() {
        for e in [
            DiscordError::Transport,
            DiscordError::Malformed,
            DiscordError::InvalidGrant,
            DiscordError::CloudflareBlocked,
            DiscordError::Status(500),
            DiscordError::RateLimited {
                retry_after: std::time::Duration::from_secs(3),
            },
        ] {
            assert!(!format!("{e} {e:?}").to_lowercase().contains("token"));
        }
    }
}
