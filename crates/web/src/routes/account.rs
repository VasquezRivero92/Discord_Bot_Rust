//! Vinculación y sincronización de cuentas de FACEIT y Steam desde el panel web:
//!
//! * `POST /api/lobby/sync-faceit-from-steam`
//! * `POST /api/lobby/verify-faceit`
//! * `POST /api/lobby/link-steam` y `POST /api/lobby/sync-steam`
//! * `GET /auth/steam` y `GET /auth/steam/callback`
//! * Lectura real de `faceit_users` para `GET /api/lobby/me`, `GET /auth/me` y `GET /api/me`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRef, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use data::{FirestoreClient, Op, Query, Value as QValue};
use lobby::auth::{RequireUser, SessionVerifier};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::json_response;
use crate::session::{OptionalUser, SessionClaims, SessionState};

const USERS_COLLECTION: &str = "faceit_users";
const LOGS_COLLECTION: &str = "faceit_bot_logs";
const PENDING_COLLECTION: &str = "faceit_pending_verifications";
const DEFAULT_FACEIT_BASE: &str = "https://open.faceit.com";
const DEFAULT_STEAM_BASE: &str = "https://steamcommunity.com";

#[derive(Clone)]
pub struct AccountState {
    pub firestore: Arc<FirestoreClient>,
    pub auth: Arc<dyn SessionVerifier>,
    pub session: SessionState,
    pub faceit_api_key: Option<String>,
    pub faceit_base_url: String,
    pub steam_base_url: String,
    pub base_url: String,
    pub default_guild_id: String,
    pub http: reqwest::Client,
}

impl AccountState {
    pub fn new(
        firestore: Arc<FirestoreClient>,
        session: SessionState,
        faceit_api_key: Option<String>,
        base_url: String,
        default_guild_id: String,
    ) -> Self {
        let auth: Arc<dyn SessionVerifier> = Arc::new(session.clone());
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .build()
            .unwrap_or_default();
        Self {
            firestore,
            auth,
            session,
            faceit_api_key: faceit_api_key
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty() && !k.starts_with("TU_")),
            faceit_base_url: DEFAULT_FACEIT_BASE.to_owned(),
            steam_base_url: DEFAULT_STEAM_BASE.to_owned(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            default_guild_id,
            http,
        }
    }
}

impl FromRef<AccountState> for Arc<dyn SessionVerifier> {
    fn from_ref(state: &AccountState) -> Self {
        state.auth.clone()
    }
}

impl FromRef<AccountState> for SessionState {
    fn from_ref(state: &AccountState) -> Self {
        state.session.clone()
    }
}

pub fn router(state: AccountState) -> Router {
    Router::new()
        .route(
            "/api/lobby/sync-faceit-from-steam",
            post(sync_faceit_from_steam),
        )
        .route("/api/lobby/verify-faceit", post(verify_faceit))
        .route("/api/lobby/link-steam", post(link_steam))
        .route("/api/lobby/sync-steam", post(link_steam))
        .route("/auth/steam", get(steam_login))
        .route("/auth/steam/callback", get(steam_callback))
        .with_state(state)
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FaceitGameInfo {
    #[serde(default)]
    pub skill_level: Option<u8>,
    #[serde(default)]
    pub faceit_elo: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FaceitGames {
    #[serde(default)]
    pub cs2: Option<FaceitGameInfo>,
    #[serde(default)]
    pub csgo: Option<FaceitGameInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FaceitPlatforms {
    #[serde(default)]
    pub steam: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FaceitProfile {
    #[serde(default)]
    pub player_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub steam_id_64: Option<String>,
    #[serde(default)]
    pub steam_nickname: Option<String>,
    #[serde(default)]
    pub platforms: Option<FaceitPlatforms>,
    #[serde(default)]
    pub games: Option<FaceitGames>,
}

impl FaceitProfile {
    pub fn skill_level(&self) -> Option<u8> {
        self.games
            .as_ref()
            .and_then(|g| {
                g.cs2
                    .as_ref()
                    .and_then(|c| c.skill_level)
                    .or_else(|| g.csgo.as_ref().and_then(|c| c.skill_level))
            })
            .filter(|&lvl| lvl > 0)
    }

    pub fn elo(&self) -> u32 {
        self.games
            .as_ref()
            .and_then(|g| {
                g.cs2
                    .as_ref()
                    .and_then(|c| c.faceit_elo)
                    .or_else(|| g.csgo.as_ref().and_then(|c| c.faceit_elo))
            })
            .unwrap_or(0)
    }

    pub fn resolved_steam_id(&self) -> Option<String> {
        self.steam_id_64
            .as_deref()
            .filter(|s| is_valid_steam_id64(s))
            .or_else(|| {
                self.platforms
                    .as_ref()
                    .and_then(|p| p.steam.as_deref())
                    .filter(|s| is_valid_steam_id64(s))
            })
            .map(str::to_owned)
    }

    pub fn clean_avatar(&self) -> Option<String> {
        self.avatar
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    pub fn clean_steam_nickname(&self) -> Option<String> {
        self.steam_nickname
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }
}

pub fn is_valid_steam_id64(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.len() == 17
        && trimmed.starts_with("7656119")
        && trimmed != "76561198000000000"
        && trimmed.bytes().all(|b| b.is_ascii_digit())
}

pub fn extract_steam_id64(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    if bytes.len() < 17 {
        return None;
    }
    for i in 0..=(bytes.len() - 17) {
        let slice = &bytes[i..i + 17];
        if slice.starts_with(b"7656119") && slice.iter().all(|b| b.is_ascii_digit()) {
            let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_digit = i + 17 < bytes.len() && bytes[i + 17].is_ascii_digit();
            if !prev_digit && !next_digit {
                if let Ok(s) = std::str::from_utf8(slice) {
                    if is_valid_steam_id64(s) {
                        return Some(s.to_owned());
                    }
                }
            }
        }
    }
    None
}

fn non_empty_str(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "faceit_mock" && *s != "76561198000000000")
        .map(str::to_owned)
}

fn number_as_f64(map: &Map<String, Value>, key: &str) -> Option<f64> {
    match map.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().replace(',', ".").parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

fn number_as_u32(map: &Map<String, Value>, key: &str) -> Option<u32> {
    match map.get(key)? {
        Value::Number(n) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    }
}

fn extract_guild_query(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        if (k == "guildId" || k == "guild_id" || k == "guild") && !v.trim().is_empty() {
            return Some(v.trim().to_owned());
        }
    }
    None
}

fn resolve_target_guild(
    query_guild: Option<&str>,
    user_servidor: Option<&str>,
    default_guild_id: &str,
) -> Option<String> {
    query_guild
        .or(user_servidor)
        .or((!default_guild_id.trim().is_empty()).then_some(default_guild_id.trim()))
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "default")
        .map(str::to_owned)
}

/// Lee y combina el documento del usuario en `faceit_users` (documento del servidor, documento
/// global y documentos adicionales con `discordId == id`), igual que `db.getFullUser` del legado.
pub async fn get_full_user_map(
    firestore: &FirestoreClient,
    discord_id: &str,
    guild_id: Option<&str>,
) -> Option<Map<String, Value>> {
    if discord_id.trim().is_empty() {
        return None;
    }

    let mut guild_map = None;
    if let Some(gid) = guild_id.filter(|g| !g.is_empty() && *g != "default") {
        let doc_id = format!("{gid}_{discord_id}");
        if let Ok(Some(Value::Object(m))) = firestore.get_doc::<Value>(USERS_COLLECTION, &doc_id).await {
            guild_map = Some(m);
        }
    }

    let global_map = match firestore
        .get_doc::<Value>(USERS_COLLECTION, discord_id)
        .await
    {
        Ok(Some(Value::Object(m))) => Some(m),
        _ => None,
    };

    let mut combined = global_map.unwrap_or_default();
    if let Some(gm) = guild_map {
        for (k, v) in gm {
            if !v.is_null() {
                combined.insert(k, v);
            }
        }
    }

    let missing_key_fields = non_empty_str(&combined, "faceit_nickname").is_none()
        || non_empty_str(&combined, "faceit_player_id").is_none()
        || non_empty_str(&combined, "steam_id_64").is_none();

    if missing_key_fields {
        let q = Query {
            filters: vec![(
                "discordId".into(),
                Op::Eq,
                QValue::String(discord_id.to_owned()),
            )],
            ..Query::default()
        };
        if let Ok(docs) = firestore.query::<Value>(USERS_COLLECTION, q).await {
            for (_, doc) in docs {
                let Value::Object(data) = doc else { continue };
                if non_empty_str(&data, "faceit_nickname").is_some() {
                    let mut merged = data.clone();
                    for (k, v) in std::mem::take(&mut combined) {
                        if !v.is_null() {
                            merged.insert(k, v);
                        }
                    }
                    combined = merged;
                }
                if let Some(rol) = non_empty_str(&data, "rol") {
                    if rol == "global_admin" || rol == "admin" {
                        combined.insert("rol".into(), Value::String(rol));
                    }
                }
                if let Some(bal) = number_as_f64(&data, "wallet_balance") {
                    let curr = number_as_f64(&combined, "wallet_balance").unwrap_or(0.0);
                    if curr == 0.0 && bal != 0.0 {
                        combined.insert("wallet_balance".into(), json!(bal));
                    }
                }
                for key in [
                    "phoneNumber",
                    "phone_number",
                    "steam_id_64",
                    "steam_nickname",
                    "faceit_avatar",
                    "faceit_player_id",
                    "faceit_nickname",
                    "faceit_level",
                    "skill_level",
                    "faceit_elo",
                ] {
                    if non_empty_str(&combined, key).is_none() && combined.get(key).is_none() {
                        if let Some(val) = data.get(key).filter(|v| !v.is_null()) {
                            combined.insert(key.into(), val.clone());
                        }
                    }
                }
            }
        }
    }

    if combined.is_empty() {
        return None;
    }
    if non_empty_str(&combined, "discordId").is_none() {
        combined.insert("discordId".into(), Value::String(discord_id.to_owned()));
    }
    Some(combined)
}

/// Busca si `faceit_player_id` ya está vinculado a algún `discordId` en `faceit_users`.
pub async fn find_discord_id_by_faceit_id(
    firestore: &FirestoreClient,
    faceit_player_id: &str,
    guild_id: Option<&str>,
) -> Option<String> {
    if faceit_player_id.trim().is_empty() {
        return None;
    }
    if let Some(gid) = guild_id.filter(|g| !g.is_empty() && *g != "default") {
        let q = Query {
            filters: vec![
                (
                    "faceit_player_id".into(),
                    Op::Eq,
                    QValue::String(faceit_player_id.to_owned()),
                ),
                ("guild_id".into(), Op::Eq, QValue::String(gid.to_owned())),
            ],
            limit: Some(1),
            ..Query::default()
        };
        if let Ok(docs) = firestore.query::<Value>(USERS_COLLECTION, q).await {
            if let Some((doc_id, Value::Object(map))) = docs.into_iter().next() {
                if let Some(did) = resolve_discord_id_from_doc(&doc_id, &map) {
                    return Some(did);
                }
            }
        }
    }

    let q = Query {
        filters: vec![(
            "faceit_player_id".into(),
            Op::Eq,
            QValue::String(faceit_player_id.to_owned()),
        )],
        limit: Some(1),
        ..Query::default()
    };
    if let Ok(docs) = firestore.query::<Value>(USERS_COLLECTION, q).await {
        if let Some((doc_id, Value::Object(map))) = docs.into_iter().next() {
            return resolve_discord_id_from_doc(&doc_id, &map);
        }
    }
    None
}

fn resolve_discord_id_from_doc(doc_id: &str, map: &Map<String, Value>) -> Option<String> {
    non_empty_str(map, "discordId")
        .or_else(|| non_empty_str(map, "discord_id"))
        .or_else(|| {
            if let Some((_, suffix)) = doc_id.split_once('_') {
                (!suffix.is_empty()).then(|| suffix.to_owned())
            } else {
                (!doc_id.is_empty()).then(|| doc_id.to_owned())
            }
        })
}

/// Guarda la vinculación de FACEIT (y opcionalmente Steam) en `faceit_users`.
pub async fn save_linked_accounts(
    firestore: &FirestoreClient,
    discord_id: &str,
    guild_id: Option<&str>,
    profile: &FaceitProfile,
    steam_id_override: Option<&str>,
    steam_nick_override: Option<&str>,
) -> Result<(), data::DataError> {
    let skill_level = profile.skill_level();
    let elo = profile.elo();
    let now_ts = data::timestamp_now();

    let mut user_data = Map::new();
    user_data.insert("discordId".into(), Value::String(discord_id.to_owned()));
    user_data.insert("discord_id".into(), Value::String(discord_id.to_owned()));
    if let Some(gid) = guild_id.filter(|g| !g.is_empty()) {
        user_data.insert("guild_id".into(), Value::String(gid.to_owned()));
    }
    user_data.insert(
        "faceit_player_id".into(),
        Value::String(profile.player_id.clone()),
    );
    user_data.insert(
        "faceit_nickname".into(),
        Value::String(profile.nickname.clone()),
    );
    user_data.insert(
        "faceit_nickname_lower".into(),
        Value::String(profile.nickname.to_lowercase()),
    );
    if let Some(lvl) = skill_level {
        user_data.insert("faceit_level".into(), json!(lvl));
        user_data.insert("skill_level".into(), json!(lvl));
    }
    user_data.insert("faceit_elo".into(), json!(elo));

    let steam_id = steam_id_override
        .filter(|s| is_valid_steam_id64(s))
        .map(str::to_owned)
        .or_else(|| profile.resolved_steam_id());
    if let Some(sid) = steam_id {
        user_data.insert("steam_id_64".into(), Value::String(sid));
    }

    let steam_nick = steam_nick_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| profile.clean_steam_nickname());
    if let Some(snick) = steam_nick {
        user_data.insert("steam_nickname".into(), Value::String(snick));
    }

    if let Some(avatar) = profile.clean_avatar() {
        user_data.insert("faceit_avatar".into(), Value::String(avatar));
    }

    user_data.insert("linkedAt".into(), now_ts.clone());
    user_data.insert("updatedAt".into(), now_ts);

    let payload = Value::Object(user_data);
    firestore
        .merge_doc(USERS_COLLECTION, discord_id, &payload)
        .await?;

    if let Some(gid) = guild_id.filter(|g| !g.is_empty() && *g != "default") {
        let guild_doc_id = format!("{gid}_{discord_id}");
        let _ = firestore
            .merge_doc(USERS_COLLECTION, &guild_doc_id, &payload)
            .await;
    }

    let _ = firestore.delete_doc(PENDING_COLLECTION, discord_id).await;
    Ok(())
}

/// Guarda únicamente `steam_id_64` (y `steam_nickname` si existe) en `faceit_users`.
pub async fn save_steam_id(
    firestore: &FirestoreClient,
    discord_id: &str,
    steam_id_64: &str,
    steam_nickname: Option<&str>,
    guild_id: Option<&str>,
) -> Result<(), data::DataError> {
    let now_ts = data::timestamp_now();
    let mut update = Map::new();
    update.insert("discordId".into(), Value::String(discord_id.to_owned()));
    update.insert("discord_id".into(), Value::String(discord_id.to_owned()));
    update.insert(
        "steam_id_64".into(),
        Value::String(steam_id_64.trim().to_owned()),
    );
    if let Some(nick) = steam_nickname.map(str::trim).filter(|s| !s.is_empty()) {
        update.insert("steam_nickname".into(), Value::String(nick.to_owned()));
    }
    if let Some(gid) = guild_id.filter(|g| !g.is_empty()) {
        update.insert("guild_id".into(), Value::String(gid.to_owned()));
    }
    update.insert("updatedAt".into(), now_ts);

    let payload = Value::Object(update);
    firestore
        .merge_doc(USERS_COLLECTION, discord_id, &payload)
        .await?;

    if let Some(gid) = guild_id.filter(|g| !g.is_empty() && *g != "default") {
        let guild_doc_id = format!("{gid}_{discord_id}");
        let _ = firestore
            .merge_doc(USERS_COLLECTION, &guild_doc_id, &payload)
            .await;
    }
    Ok(())
}

async fn write_bot_log(
    firestore: &FirestoreClient,
    log_type: &str,
    message: &str,
    guild_id: Option<&str>,
) {
    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let entry = json!({
        "type": log_type,
        "message": message,
        "source": "web_panel",
        "guild_id": guild_id,
        "timestamp": data::timestamp_now(),
    });
    let _ = firestore.set_doc(LOGS_COLLECTION, &log_id, &entry).await;
}

/// Consulta un jugador de FACEIT por su `SteamID64` probando `cs2` y luego `csgo`.
pub async fn fetch_faceit_player_by_steam_id(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    steam_id_64: &str,
) -> Result<Option<FaceitProfile>, String> {
    let clean_id = steam_id_64.trim();
    if !is_valid_steam_id64(clean_id) {
        return Ok(None);
    }
    let base = base_url.trim_end_matches('/');
    for game in ["cs2", "csgo"] {
        let url = format!(
            "{base}/data/v4/players?game={game}&game_player_id={}",
            url::form_urlencoded::byte_serialize(clean_id.as_bytes()).collect::<String>()
        );
        let res = http
            .get(&url)
            .bearer_auth(api_key)
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| format!("Error de red al consultar la API de FACEIT: {e}"))?;

        if res.status().is_success() {
            if let Ok(profile) = res.json::<FaceitProfile>().await {
                if !profile.player_id.trim().is_empty() {
                    return Ok(Some(profile));
                }
            }
        } else if res.status() != StatusCode::NOT_FOUND {
            tracing::warn!(
                "[FACEIT] status {} consultando game={} steam_id={}",
                res.status(),
                game,
                clean_id
            );
        }
    }
    Ok(None)
}

/// Consulta un jugador de FACEIT por nickname exacto.
pub async fn fetch_faceit_player_by_nickname(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    nickname: &str,
) -> Result<Option<FaceitProfile>, String> {
    let encoded: String =
        url::form_urlencoded::byte_serialize(nickname.trim().as_bytes()).collect();
    let url = format!(
        "{}/data/v4/players?nickname={encoded}",
        base_url.trim_end_matches('/')
    );
    let res = http
        .get(&url)
        .bearer_auth(api_key)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Error de red al consultar la API de FACEIT: {e}"))?;

    if res.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(format!("API Faceit status {}", res.status().as_u16()));
    }
    let profile = res
        .json::<FaceitProfile>()
        .await
        .map_err(|e| format!("Respuesta inválida de FACEIT: {e}"))?;
    if profile.player_id.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(profile))
}

/// Consulta un jugador de FACEIT por `player_id` (UUID).
pub async fn fetch_faceit_player_by_id(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    player_id: &str,
) -> Result<Option<FaceitProfile>, String> {
    let url = format!(
        "{}/data/v4/players/{}",
        base_url.trim_end_matches('/'),
        player_id.trim()
    );
    let res = http
        .get(&url)
        .bearer_auth(api_key)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Error de red al consultar la API de FACEIT: {e}"))?;

    if res.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(format!("API Faceit status {}", res.status().as_u16()));
    }
    let profile = res
        .json::<FaceitProfile>()
        .await
        .map_err(|e| format!("Respuesta inválida de FACEIT: {e}"))?;
    if profile.player_id.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(profile))
}

/// Construye el objeto `user` para `GET /api/lobby/me`, `GET /auth/me` y `GET /api/me` leyendo
/// `faceit_users` (y enriqueciendo desde FACEIT si falta `steam_id_64` o `faceit_player_id`).
pub async fn build_authenticated_user_json(
    claims: &SessionClaims,
    firestore: &FirestoreClient,
    http: &reqwest::Client,
    faceit_base_url: &str,
    faceit_api_key: Option<&str>,
    query_guild: Option<&str>,
    default_guild_id: &str,
) -> Value {
    let avatar_url = claims
        .avatar
        .as_deref()
        .map(|av| {
            if av.starts_with("http") {
                av.to_string()
            } else {
                format!(
                    "https://cdn.discordapp.com/avatars/{}/{}.png",
                    claims.user_id, av
                )
            }
        })
        .unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".to_string());

    let target_guild = resolve_target_guild(
        query_guild,
        claims.servidor.as_deref(),
        default_guild_id,
    );

    let mut full_user = get_full_user_map(firestore, &claims.user_id, target_guild.as_deref())
        .await
        .unwrap_or_default();

    // Si tiene Faceit vinculado pero le falta steam_id_64 o faceit_avatar, enriquecer desde FACEIT
    if let (Some(api_key), Some(player_id)) = (
        faceit_api_key.filter(|k| !k.is_empty()),
        non_empty_str(&full_user, "faceit_player_id"),
    ) {
        let missing_steam = non_empty_str(&full_user, "steam_id_64")
            .filter(|s| is_valid_steam_id64(s))
            .is_none();
        let missing_avatar = non_empty_str(&full_user, "faceit_avatar").is_none();
        if missing_steam || missing_avatar {
            if let Ok(Some(profile)) =
                fetch_faceit_player_by_id(http, faceit_base_url, api_key, &player_id).await
            {
                let mut patch = Map::new();
                if missing_steam {
                    if let Some(sid) = profile.resolved_steam_id() {
                        full_user.insert("steam_id_64".into(), Value::String(sid.clone()));
                        patch.insert("steam_id_64".into(), Value::String(sid));
                    }
                }
                if non_empty_str(&full_user, "steam_nickname").is_none() {
                    if let Some(snick) = profile.clean_steam_nickname() {
                        full_user.insert("steam_nickname".into(), Value::String(snick.clone()));
                        patch.insert("steam_nickname".into(), Value::String(snick));
                    }
                }
                if missing_avatar {
                    if let Some(av) = profile.clean_avatar() {
                        full_user.insert("faceit_avatar".into(), Value::String(av.clone()));
                        patch.insert("faceit_avatar".into(), Value::String(av));
                    }
                }
                if !patch.is_empty() {
                    patch.insert("updatedAt".into(), data::timestamp_now());
                    let _ = firestore
                        .merge_doc(USERS_COLLECTION, &claims.user_id, &Value::Object(patch))
                        .await;
                }
            }
        }
    }

    // Si tiene Steam vinculado pero aún no tiene Faceit en la BD, intentar detectarlo por SteamID64
    if let (Some(api_key), Some(steam_id)) = (
        faceit_api_key.filter(|k| !k.is_empty()),
        non_empty_str(&full_user, "steam_id_64").filter(|s| is_valid_steam_id64(s)),
    ) {
        let missing_faceit = non_empty_str(&full_user, "faceit_player_id").is_none()
            || non_empty_str(&full_user, "faceit_nickname").is_none();
        if missing_faceit {
            if let Ok(Some(profile)) =
                fetch_faceit_player_by_steam_id(http, faceit_base_url, api_key, &steam_id).await
            {
                let existing_owner = find_discord_id_by_faceit_id(
                    firestore,
                    &profile.player_id,
                    target_guild.as_deref(),
                )
                .await;
                if existing_owner
                    .as_deref()
                    .is_none_or(|owner| owner == claims.user_id)
                {
                    let lvl = profile.skill_level();
                    let elo = profile.elo();
                    full_user.insert(
                        "faceit_player_id".into(),
                        Value::String(profile.player_id.clone()),
                    );
                    full_user.insert(
                        "faceit_nickname".into(),
                        Value::String(profile.nickname.clone()),
                    );
                    if let Some(l) = lvl {
                        full_user.insert("faceit_level".into(), json!(l));
                    }
                    full_user.insert("faceit_elo".into(), json!(elo));
                    if let Some(av) = profile.clean_avatar() {
                        full_user.insert("faceit_avatar".into(), Value::String(av));
                    }
                    let existing_snick = non_empty_str(&full_user, "steam_nickname");
                    let _ = save_linked_accounts(
                        firestore,
                        &claims.user_id,
                        target_guild.as_deref(),
                        &profile,
                        Some(&steam_id),
                        existing_snick.as_deref(),
                    )
                    .await;
                }
            }
        }
    }

    let phone = non_empty_str(&full_user, "phoneNumber")
        .or_else(|| non_empty_str(&full_user, "phone_number"));
    let has_saved_phone = phone.is_some();
    let balance = number_as_f64(&full_user, "wallet_balance").unwrap_or(0.0);
    let faceit_nickname = non_empty_str(&full_user, "faceit_nickname");
    let faceit_player_id = non_empty_str(&full_user, "faceit_player_id");
    let faceit_avatar = non_empty_str(&full_user, "faceit_avatar");
    let skill_level = number_as_u32(&full_user, "faceit_level")
        .or_else(|| number_as_u32(&full_user, "skill_level"))
        .or_else(|| number_as_u32(&full_user, "skillLevel"));
    let elo = number_as_u32(&full_user, "faceit_elo").or_else(|| number_as_u32(&full_user, "elo"));
    let tier = non_empty_str(&full_user, "mixton_tier_override")
        .or_else(|| non_empty_str(&full_user, "mixton_tier"));
    let steam_id_64 = non_empty_str(&full_user, "steam_id_64").filter(|s| is_valid_steam_id64(s));
    let steam_nickname = non_empty_str(&full_user, "steam_nickname");

    json!({
        "user": {
            "userId": claims.user_id,
            "username": claims.username,
            "avatar": claims.avatar,
            "avatarURL": avatar_url,
            "isAdmin": claims.is_admin,
            "role": claims.role,
            "servidor": claims.servidor,
            "hasSavedPhone": has_saved_phone,
            "phoneNumber": phone,
            "balance": balance,
            "faceit_nickname": faceit_nickname,
            "faceit_player_id": faceit_player_id,
            "faceit_avatar": faceit_avatar,
            "skillLevel": skill_level,
            "faceit_level": skill_level,
            "elo": elo,
            "faceit_elo": elo,
            "tier": tier,
            "steam_id_64": steam_id_64,
            "steam_nickname": steam_nickname,
        }
    })
}

// ------------------------------------------------------------------ POST /api/lobby/sync-faceit-from-steam

async fn sync_faceit_from_steam(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(state): State<AccountState>,
) -> Response {
    let target_guild = resolve_target_guild(
        extract_guild_query(raw.as_deref()).as_deref(),
        user.servidor.as_deref(),
        &state.default_guild_id,
    );

    let full_user = get_full_user_map(&state.firestore, &user.user_id, target_guild.as_deref())
        .await
        .unwrap_or_default();

    let Some(steam_id_64) =
        non_empty_str(&full_user, "steam_id_64").filter(|s| is_valid_steam_id64(s))
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({
                "error": "No tienes una cuenta de Steam conectada. Primero conecta tu cuenta de Steam con el botón de Steam."
            }),
        );
    };

    let Some(api_key) = state.faceit_api_key.as_deref() else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "La API Key de FACEIT no está configurada en el servidor." }),
        );
    };

    let profile = match fetch_faceit_player_by_steam_id(
        &state.http,
        &state.faceit_base_url,
        api_key,
        &steam_id_64,
    )
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return json_response(
                StatusCode::NOT_FOUND,
                &json!({
                    "error": format!(
                        "No se encontró ninguna cuenta de FACEIT vinculada al SteamID64 {steam_id_64} en CS2 ni CSGO. Asegúrate de haber jugado en FACEIT con esta cuenta de Steam."
                    )
                }),
            );
        }
        Err(err) => {
            tracing::error!("Error en /sync-faceit-from-steam para {}: {err}", user.user_id);
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al comunicarse con la API de FACEIT." }),
            );
        }
    };

    if let Some(existing_discord_id) = find_discord_id_by_faceit_id(
        &state.firestore,
        &profile.player_id,
        target_guild.as_deref(),
    )
    .await
    {
        if existing_discord_id != user.user_id {
            return json_response(
                StatusCode::CONFLICT,
                &json!({
                    "error": format!(
                        "El perfil de FACEIT \"{}\" ya se encuentra vinculado a otra cuenta de Discord.",
                        profile.nickname
                    )
                }),
            );
        }
    }

    let existing_steam_nick = non_empty_str(&full_user, "steam_nickname");
    if let Err(err) = save_linked_accounts(
        &state.firestore,
        &user.user_id,
        target_guild.as_deref(),
        &profile,
        Some(&steam_id_64),
        existing_steam_nick.as_deref(),
    )
    .await
    {
        tracing::error!(
            "Error guardando vinculación FACEIT desde Steam para {}: {err}",
            user.user_id
        );
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "No se pudo guardar la vinculación en la base de datos." }),
        );
    }

    write_bot_log(
        &state.firestore,
        "success",
        &format!(
            "Usuario {} ({}) detectó y vinculó FACEIT {} desde SteamID64 {}",
            user.username, user.user_id, profile.nickname, steam_id_64
        ),
        target_guild.as_deref(),
    )
    .await;

    let skill_level = profile.skill_level();
    let elo = profile.elo();
    let avatar = profile.clean_avatar();
    let steam_nick = existing_steam_nick.or_else(|| profile.clean_steam_nickname());

    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": format!(
                "¡Perfil de FACEIT \"{}\" detectado y vinculado exitosamente desde tu cuenta de Steam!",
                profile.nickname
            ),
            "faceit": {
                "nickname": profile.nickname,
                "playerId": profile.player_id,
                "level": skill_level,
                "elo": elo,
                "avatar": avatar,
            },
            "steam": {
                "steam_id_64": steam_id_64,
                "steam_nickname": steam_nick,
            }
        }),
    )
}

// ------------------------------------------------------------------ POST /api/lobby/verify-faceit

#[derive(Deserialize, Default)]
struct VerifyFaceitPayload {
    #[serde(default)]
    nickname: Option<String>,
}

async fn verify_faceit(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(state): State<AccountState>,
    Json(payload): Json<VerifyFaceitPayload>,
) -> Response {
    let clean_nick = payload.nickname.as_deref().unwrap_or("").trim();
    if clean_nick.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Ingresa un nickname de FACEIT válido." }),
        );
    }

    let Some(api_key) = state.faceit_api_key.as_deref() else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "La API Key de FACEIT no está configurada en el servidor." }),
        );
    };

    let profile = match fetch_faceit_player_by_nickname(
        &state.http,
        &state.faceit_base_url,
        api_key,
        clean_nick,
    )
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return json_response(
                StatusCode::NOT_FOUND,
                &json!({
                    "error": format!("No se encontró ningún jugador en FACEIT con el nickname \"{clean_nick}\".")
                }),
            );
        }
        Err(err) => {
            tracing::error!("Error en /verify-faceit para {}: {err}", user.user_id);
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al comunicarse con la API de FACEIT." }),
            );
        }
    };

    let target_guild = resolve_target_guild(
        extract_guild_query(raw.as_deref()).as_deref(),
        user.servidor.as_deref(),
        &state.default_guild_id,
    );

    if let Some(existing_discord_id) = find_discord_id_by_faceit_id(
        &state.firestore,
        &profile.player_id,
        target_guild.as_deref(),
    )
    .await
    {
        if existing_discord_id != user.user_id {
            return json_response(
                StatusCode::CONFLICT,
                &json!({
                    "error": format!(
                        "El perfil de FACEIT \"{}\" ya se encuentra vinculado a otra cuenta de Discord.",
                        profile.nickname
                    )
                }),
            );
        }
    }

    if let Err(err) = save_linked_accounts(
        &state.firestore,
        &user.user_id,
        target_guild.as_deref(),
        &profile,
        None,
        None,
    )
    .await
    {
        tracing::error!("Error guardando /verify-faceit para {}: {err}", user.user_id);
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "No se pudo guardar la vinculación en la base de datos." }),
        );
    }

    write_bot_log(
        &state.firestore,
        "success",
        &format!(
            "Usuario {} ({}) vinculó FACEIT {} desde la web",
            user.username, user.user_id, profile.nickname
        ),
        target_guild.as_deref(),
    )
    .await;

    let skill_level = profile.skill_level();
    let elo = profile.elo();
    let steam_id_64 = profile.resolved_steam_id();
    let steam_nickname = profile.clean_steam_nickname();
    let avatar = profile.clean_avatar();

    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": format!("¡Cuenta de FACEIT {} verificada y vinculada exitosamente!", profile.nickname),
            "faceit": {
                "nickname": profile.nickname,
                "playerId": profile.player_id,
                "level": skill_level,
                "elo": elo,
                "avatar": avatar,
            },
            "steam": steam_id_64.map(|sid| json!({
                "steam_id_64": sid,
                "steam_nickname": steam_nickname,
            })),
        }),
    )
}

// ------------------------------------------------------------------ POST /api/lobby/link-steam

#[derive(Deserialize, Default)]
struct LinkSteamPayload {
    #[serde(rename = "steamInput", default)]
    steam_input: Option<String>,
}

async fn link_steam(
    user: RequireUser,
    RawQuery(raw): RawQuery,
    State(state): State<AccountState>,
    Json(payload): Json<LinkSteamPayload>,
) -> Response {
    let input = payload
        .steam_input
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("detect_faceit");

    let target_guild = resolve_target_guild(
        extract_guild_query(raw.as_deref()).as_deref(),
        user.servidor.as_deref(),
        &state.default_guild_id,
    );

    let (steam_id_64, steam_nickname) = if input == "detect_faceit" {
        let full_user =
            get_full_user_map(&state.firestore, &user.user_id, target_guild.as_deref())
                .await
                .unwrap_or_default();
        let Some(faceit_player_id) = non_empty_str(&full_user, "faceit_player_id") else {
            return json_response(
                StatusCode::BAD_REQUEST,
                &json!({
                    "error": "Primero debes vincular tu cuenta de FACEIT para consultar tu Steam en la API."
                }),
            );
        };
        let Some(api_key) = state.faceit_api_key.as_deref() else {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "La API Key de FACEIT no está configurada en el servidor." }),
            );
        };
        match fetch_faceit_player_by_id(
            &state.http,
            &state.faceit_base_url,
            api_key,
            &faceit_player_id,
        )
        .await
        {
            Ok(Some(profile)) => {
                let Some(sid) = profile.resolved_steam_id() else {
                    return json_response(
                        StatusCode::NOT_FOUND,
                        &json!({
                            "error": "Tu perfil de FACEIT no tiene una cuenta de Steam vinculada públicamente."
                        }),
                    );
                };
                (sid, profile.clean_steam_nickname())
            }
            Ok(None) => {
                return json_response(
                    StatusCode::NOT_FOUND,
                    &json!({
                        "error": "Tu perfil de FACEIT no tiene una cuenta de Steam vinculada públicamente."
                    }),
                );
            }
            Err(err) => {
                tracing::error!("Error consultando Steam en FACEIT para {}: {err}", user.user_id);
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "Error al consultar datos de Steam en FACEIT." }),
                );
            }
        }
    } else {
        let Some(sid) = extract_steam_id64(input) else {
            return json_response(
                StatusCode::BAD_REQUEST,
                &json!({
                    "error": "Formato inválido. Ingresa un SteamID64 válido de 17 dígitos (ej: 76561198012345678) o el link a tu perfil (steamcommunity.com/profiles/...)."
                }),
            );
        };
        (sid, None)
    };

    if let Err(err) = save_steam_id(
        &state.firestore,
        &user.user_id,
        &steam_id_64,
        steam_nickname.as_deref(),
        target_guild.as_deref(),
    )
    .await
    {
        tracing::error!("Error guardando SteamID64 para {}: {err}", user.user_id);
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "No se pudo guardar el SteamID64 en la base de datos." }),
        );
    }

    write_bot_log(
        &state.firestore,
        "info",
        &format!(
            "Usuario {} ({}) conectó SteamID64 {}",
            user.username, user.user_id, steam_id_64
        ),
        target_guild.as_deref(),
    )
    .await;

    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": format!("¡Cuenta de Steam conectada exitosamente! SteamID64: {steam_id_64}"),
            "steam_id_64": steam_id_64,
            "steam_nickname": steam_nickname,
        }),
    )
}

// ------------------------------------------------------------------ GET /auth/steam y callback

fn redirect_found(location: &str) -> Response {
    let mut res = (
        StatusCode::FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("Found. Redirecting to {location}"),
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}

fn effective_base_url(headers: &HeaderMap, fallback_base_url: &str) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| *s == "https" || *s == "http")
        .unwrap_or_else(|| {
            if fallback_base_url.starts_with("https") {
                "https"
            } else {
                "http"
            }
        });
    match host {
        Some(h) if !h.contains('/') && !h.contains('\\') => format!("{proto}://{h}"),
        _ => fallback_base_url.trim_end_matches('/').to_owned(),
    }
}

async fn steam_login(
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    State(state): State<AccountState>,
) -> Response {
    if user.is_none() {
        return redirect_found("/auth/discord?redirect=/auth/steam");
    }

    let base = effective_base_url(&headers, &state.base_url);
    let return_to = format!("{base}/auth/steam/callback");
    let realm = format!("{base}/");

    let mut url = url::Url::parse(&format!(
        "{}/openid/login",
        state.steam_base_url.trim_end_matches('/')
    ))
    .expect("URL de Steam válida");
    url.query_pairs_mut()
        .append_pair("openid.ns", "http://specs.openid.net/auth/2.0")
        .append_pair("openid.mode", "checkid_setup")
        .append_pair("openid.return_to", &return_to)
        .append_pair("openid.realm", &realm)
        .append_pair(
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select",
        )
        .append_pair(
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select",
        );

    redirect_found(url.as_str())
}

async fn fetch_steam_xml_nickname(
    http: &reqwest::Client,
    steam_base_url: &str,
    steam_id_64: &str,
) -> Option<String> {
    let url = format!(
        "{}/profiles/{steam_id_64}/?xml=1",
        steam_base_url.trim_end_matches('/')
    );
    let res = http.get(&url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let xml = res.text().await.ok()?;
    let open = "<steamID><![CDATA[";
    let start = xml.find(open)? + open.len();
    let end = xml[start..].find("]]></steamID>")? + start;
    let nick = xml[start..end].trim();
    (!nick.is_empty()).then(|| nick.to_owned())
}

async fn steam_callback(
    OptionalUser(user): OptionalUser,
    RawQuery(raw): RawQuery,
    State(state): State<AccountState>,
) -> Response {
    let Some(user) = user else {
        return redirect_found("/cuenta?error=auth_required");
    };

    let raw_query = raw.unwrap_or_default();
    let mut pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(raw_query.as_bytes())
            .into_owned()
            .collect();

    let claimed_id = pairs
        .iter()
        .find(|(k, _)| k == "openid.claimed_id")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    for (k, v) in &mut pairs {
        if k == "openid.mode" {
            *v = "check_authentication".to_owned();
        }
    }
    if !pairs.iter().any(|(k, _)| k == "openid.mode") {
        pairs.push(("openid.mode".to_owned(), "check_authentication".to_owned()));
    }

    let check_url = format!(
        "{}/openid/login",
        state.steam_base_url.trim_end_matches('/')
    );
    let check_res = match state.http.post(&check_url).form(&pairs).send().await {
        Ok(res) => res,
        Err(err) => {
            tracing::error!("[STEAM AUTH] Error verificando OpenID con Steam: {err}");
            return redirect_found("/cuenta?error=steam_error");
        }
    };

    let check_text = check_res.text().await.unwrap_or_default();
    if !check_text.contains("is_valid:true") {
        tracing::warn!("[STEAM AUTH] Validación de OpenID fallida desde Steam.");
        return redirect_found("/cuenta?error=steam_validation_failed");
    }

    let Some(steam_id_64) = extract_steam_id64(&claimed_id) else {
        return redirect_found("/cuenta?error=no_steamid_found");
    };

    let steam_nickname =
        fetch_steam_xml_nickname(&state.http, &state.steam_base_url, &steam_id_64).await;
    let target_guild = resolve_target_guild(
        None,
        user.servidor.as_deref(),
        &state.default_guild_id,
    );

    let mut faceit_linked = false;
    let mut faceit_conflict = false;
    let mut faceit_not_found = false;

    if let Some(api_key) = state.faceit_api_key.as_deref() {
        match fetch_faceit_player_by_steam_id(
            &state.http,
            &state.faceit_base_url,
            api_key,
            &steam_id_64,
        )
        .await
        {
            Ok(Some(profile)) => {
                let existing_discord_id = find_discord_id_by_faceit_id(
                    &state.firestore,
                    &profile.player_id,
                    target_guild.as_deref(),
                )
                .await;
                if existing_discord_id
                    .as_deref()
                    .is_some_and(|id| id != user.user_id)
                {
                    faceit_conflict = true;
                    let _ = save_steam_id(
                        &state.firestore,
                        &user.user_id,
                        &steam_id_64,
                        steam_nickname.as_deref(),
                        target_guild.as_deref(),
                    )
                    .await;
                } else if save_linked_accounts(
                    &state.firestore,
                    &user.user_id,
                    target_guild.as_deref(),
                    &profile,
                    Some(&steam_id_64),
                    steam_nickname.as_deref(),
                )
                .await
                .is_ok()
                {
                    faceit_linked = true;
                    write_bot_log(
                        &state.firestore,
                        "success",
                        &format!(
                            "Usuario {} ({}) vinculó Steam {} y detectó FACEIT: {}",
                            user.username, user.user_id, steam_id_64, profile.nickname
                        ),
                        target_guild.as_deref(),
                    )
                    .await;
                }
            }
            Ok(None) => {
                faceit_not_found = true;
                let _ = save_steam_id(
                    &state.firestore,
                    &user.user_id,
                    &steam_id_64,
                    steam_nickname.as_deref(),
                    target_guild.as_deref(),
                )
                .await;
            }
            Err(err) => {
                tracing::error!("[STEAM AUTH] Error consultando FACEIT con SteamID: {err}");
                let _ = save_steam_id(
                    &state.firestore,
                    &user.user_id,
                    &steam_id_64,
                    steam_nickname.as_deref(),
                    target_guild.as_deref(),
                )
                .await;
            }
        }
    } else {
        faceit_not_found = true;
        let _ = save_steam_id(
            &state.firestore,
            &user.user_id,
            &steam_id_64,
            steam_nickname.as_deref(),
            target_guild.as_deref(),
        )
        .await;
    }

    if faceit_linked {
        return redirect_found("/cuenta?steam_connected=true&faceit_linked=true");
    }
    if faceit_conflict {
        return redirect_found("/cuenta?steam_connected=true&faceit_conflict=true");
    }
    if faceit_not_found {
        return redirect_found("/cuenta?steam_connected=true&faceit_not_found=true");
    }
    redirect_found("/cuenta?steam_connected=true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrae_steam_id64_de_id_directo_y_de_url_de_perfil() {
        assert_eq!(
            extract_steam_id64("76561198012345678").as_deref(),
            Some("76561198012345678")
        );
        assert_eq!(
            extract_steam_id64("https://steamcommunity.com/profiles/76561198012345678/").as_deref(),
            Some("76561198012345678")
        );
        assert_eq!(extract_steam_id64("76561198000000000"), None);
        assert_eq!(extract_steam_id64("12345"), None);
    }
}
