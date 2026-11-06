//! Fotos de perfil de Steam para el detalle del historial de partidas.
//!
//! Port de `services/steamAvatarService.js` del legacy: `fetchSteamAvatar` (perfil XML público,
//! caché de 2 h, 4 s de espera) y `enrichMatchWithSteamAvatars` (inyecta `steam_id_64` y `avatar`
//! en los rosters del historial y en `stats.teams[].players`).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use data::UsersRepo;
use futures_util::future::join_all;
use serde_json::{Map, Value};

const TTL: Duration = Duration::from_secs(2 * 60 * 60);
const TIMEOUT: Duration = Duration::from_secs(4);
const STEAM_BASE: &str = "https://steamcommunity.com";

/// Origen de avatares por SteamID64. Costura para probar sin red.
#[async_trait]
pub trait SteamAvatars: Send + Sync {
    /// URL del avatar de cada SteamID64 que se haya podido resolver (las que fallan no salen).
    async fn fetch_many(&self, ids: &[String]) -> HashMap<String, String>;
}

/// Cliente del perfil XML público de Steam, con caché en memoria.
pub struct HttpSteamAvatars {
    http: reqwest::Client,
    base: String,
    cache: Mutex<HashMap<String, (Instant, String)>>,
}

impl HttpSteamAvatars {
    pub fn new() -> Self {
        Self::with_base(STEAM_BASE)
    }

    /// Con otra raíz (pruebas con un servidor local).
    pub fn with_base(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .build()
            .unwrap_or_default();
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    async fn fetch_one(&self, id: &str) -> Option<String> {
        if let Some((at, url)) = self.cache.lock().ok()?.get(id) {
            if at.elapsed() < TTL {
                return Some(url.clone());
            }
        }
        let res = self
            .http
            .get(format!("{}/profiles/{id}/?xml=1", self.base))
            .send()
            .await
            .ok()?;
        if !res.status().is_success() {
            return None;
        }
        let xml = res.text().await.ok()?;
        let url = ["avatarMedium", "avatarFull", "avatarIcon"]
            .iter()
            .find_map(|tag| cdata(&xml, tag))?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(id.to_owned(), (Instant::now(), url.clone()));
        }
        Some(url)
    }
}

impl Default for HttpSteamAvatars {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SteamAvatars for HttpSteamAvatars {
    async fn fetch_many(&self, ids: &[String]) -> HashMap<String, String> {
        let found = join_all(ids.iter().map(|id| async move {
            let avatar = self.fetch_one(id).await;
            (id.clone(), avatar)
        }))
        .await;
        found
            .into_iter()
            .filter_map(|(id, avatar)| avatar.map(|a| (id, a)))
            .collect()
    }
}

/// Contenido de `<tag><![CDATA[...]]></tag>`, sin espacios en los extremos.
fn cdata(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}><![CDATA[");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find("]]>")? + start;
    let url = xml[start..end].trim();
    (!url.is_empty()).then(|| url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdata_reads_the_profile_xml_like_the_legacy_regexes() {
        let xml = "<profile><avatarIcon><![CDATA[https://x/i.jpg]]></avatarIcon>\
                   <avatarMedium><![CDATA[ https://x/m.jpg ]]></avatarMedium></profile>";
        assert_eq!(cdata(xml, "avatarMedium").as_deref(), Some("https://x/m.jpg"));
        assert_eq!(cdata(xml, "avatarIcon").as_deref(), Some("https://x/i.jpg"));
        assert_eq!(cdata(xml, "avatarFull"), None);
        assert_eq!(cdata("<avatarMedium><![CDATA[]]></avatarMedium>", "avatarMedium"), None);
    }

    #[test]
    fn steam_ids_are_exactly_17_digits() {
        assert!(is_steam_id("76561198000000001"));
        assert!(!is_steam_id("7656119800000000"));
        assert!(!is_steam_id("7656119800000000a"));
    }
}

/// `/^\d{17}$/`
fn is_steam_id(s: &str) -> bool {
    s.len() == 17 && s.bytes().all(|b| b.is_ascii_digit())
}

/// `String(v)` de un texto o un número; el resto (y el texto vacío) no cuenta.
fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Lo que se sabe de un jugador del roster: su SteamID64 y la foto de su cuenta vinculada.
#[derive(Default, Clone)]
struct Info {
    s64: Option<String>,
    avatar: Option<String>,
}

fn key(v: Option<&Value>) -> Option<String> {
    text(v).map(|s| s.to_lowercase().trim().to_owned())
}

fn roster_s64(p: &Map<String, Value>) -> Option<String> {
    text(p.get("steam_id_64")).or_else(|| text(p.get("id")).filter(|id| is_steam_id(id)))
}

/// `enrichMatchWithSteamAvatars`: no falla nunca; sin datos de Steam deja las partidas como están.
pub async fn enrich_match(
    history: &mut Value,
    stats: &mut Option<Value>,
    users: &dyn UsersRepo,
    avatars: &dyn SteamAvatars,
) {
    let roster: Vec<Map<String, Value>> = ["faction1_roster", "faction2_roster"]
        .iter()
        .filter_map(|k| history.get(*k).and_then(Value::as_array))
        .flatten()
        .filter_map(|p| p.as_object().cloned())
        .collect();

    // 1. SteamID64 desde el roster o, si falta, desde el usuario vinculado por Discord.
    let infos = join_all(roster.iter().map(|p| async move {
        let mut info = Info {
            s64: roster_s64(p),
            avatar: None,
        };
        if let Some(discord_id) = text(p.get("discord_id")) {
            if let Ok(Some(user)) = users.get(&discord_id).await {
                if info.s64.is_none() {
                    info.s64 = text(user.extra.get("steam_id_64")).filter(|s| is_steam_id(s));
                }
                info.avatar = text(user.extra.get("discord_avatar"))
                    .or_else(|| text(user.extra.get("faceit_avatar")));
            }
        }
        info
    }))
    .await;

    let mut lookup: HashMap<String, Info> = HashMap::new();
    for (p, info) in roster.iter().zip(&infos) {
        if let Some(k) = key(p.get("nickname")) {
            lookup.insert(k, info.clone());
        }
        if let Some(k) = key(p.get("id")) {
            lookup.insert(k, info.clone());
        }
        if let Some(k) = text(p.get("discord_id")) {
            lookup.insert(k.trim().to_owned(), info.clone());
        }
    }

    let stats_player = |pl: &Map<String, Value>| -> (Option<String>, Option<Info>) {
        let direct = text(pl.get("playerId")).filter(|id| is_steam_id(id));
        let matched = key(pl.get("playerId"))
            .and_then(|k| lookup.get(&k))
            .or_else(|| key(pl.get("nickname")).and_then(|k| lookup.get(&k)))
            .cloned();
        let s64 = direct.or_else(|| matched.as_ref().and_then(|m| m.s64.clone()));
        (s64, matched)
    };
    let history_player = |p: &Map<String, Value>| -> (Option<String>, Option<Info>) {
        let matched = key(p.get("nickname"))
            .and_then(|k| lookup.get(&k))
            .or_else(|| key(p.get("id")).and_then(|k| lookup.get(&k)))
            .cloned();
        let s64 = text(p.get("steam_id_64")).or_else(|| matched.as_ref().and_then(|m| m.s64.clone()));
        (s64, matched)
    };

    // 2. Todos los SteamID64 presentes en las estadísticas y el roster.
    let mut wanted: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut want = |s64: Option<String>| {
        if let Some(s) = s64.filter(|s| is_steam_id(s)) {
            if seen.insert(s.clone()) {
                wanted.push(s);
            }
        }
    };
    if let Some(teams) = stats.as_ref().and_then(|s| s.get("teams")).and_then(Value::as_array) {
        for pl in teams
            .iter()
            .filter_map(|t| t.get("players").and_then(Value::as_array))
            .flatten()
            .filter_map(Value::as_object)
        {
            want(stats_player(pl).0);
        }
    }
    for p in &roster {
        want(history_player(p).0);
    }

    // 3. Foto real de Steam.
    let steam = avatars.fetch_many(&wanted).await;

    // 4. Inyectar en `stats.teams[].players[]`.
    if let Some(teams) = stats
        .as_mut()
        .and_then(|s| s.get_mut("teams"))
        .and_then(Value::as_array_mut)
    {
        for pl in teams
            .iter_mut()
            .filter_map(|t| t.get_mut("players").and_then(Value::as_array_mut))
            .flatten()
            .filter_map(Value::as_object_mut)
        {
            let (s64, matched) = stats_player(pl);
            let avatar = s64
                .as_ref()
                .and_then(|s| steam.get(s).cloned())
                .or_else(|| text(pl.get("avatar")))
                .or_else(|| matched.and_then(|m| m.avatar));
            pl.insert("steam_id_64".into(), s64.map_or(Value::Null, Value::String));
            pl.insert("avatar".into(), avatar.map_or(Value::Null, Value::String));
        }
    }

    // 5. Inyectar en los rosters del historial.
    for k in ["faction1_roster", "faction2_roster"] {
        let Some(players) = history.get_mut(k).and_then(Value::as_array_mut) else {
            continue;
        };
        for p in players.iter_mut().filter_map(Value::as_object_mut) {
            let (s64, matched) = history_player(p);
            let avatar = s64
                .as_ref()
                .and_then(|s| steam.get(s).cloned())
                .or_else(|| text(p.get("avatar")))
                .or_else(|| matched.and_then(|m| m.avatar));
            p.insert("steam_id_64".into(), s64.map_or(Value::Null, Value::String));
            p.insert("avatar".into(), avatar.map_or(Value::Null, Value::String));
        }
    }
}
