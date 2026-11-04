use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaceitPlayerGamesCs2 {
    #[serde(default)]
    pub faceit_elo: Option<u32>,
    #[serde(default)]
    pub skill_level: Option<u8>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaceitPlayerGames {
    #[serde(default)]
    pub cs2: Option<FaceitPlayerGamesCs2>,
    #[serde(default)]
    pub csgo: Option<FaceitPlayerGamesCs2>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaceitPlayer {
    pub player_id: String,
    pub nickname: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub games: Option<FaceitPlayerGames>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaceitLifetimeStats {
    #[serde(rename = "Average K/D Ratio", default)]
    pub kd_ratio: Option<String>,
    #[serde(rename = "Win Rate %", default)]
    pub win_rate: Option<String>,
    #[serde(rename = "Matches", default)]
    pub matches: Option<String>,
    #[serde(rename = "Current Win Streak", default)]
    pub win_streak: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaceitPlayerStats {
    #[serde(default)]
    pub lifetime: Option<FaceitLifetimeStats>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LeaderboardPlayer {
    pub nickname: String,
    pub player_id: String,
    #[serde(default)]
    pub skill_level: Option<u8>,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LeaderboardItem {
    pub position: u32,
    pub points: Option<i64>,
    pub player: LeaderboardPlayer,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LeaderboardResponse {
    pub items: Vec<LeaderboardItem>,
}

#[derive(Clone)]
pub struct FaceitClient {
    client: reqwest::Client,
    api_key: String,
}

impl FaceitClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
        }
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", self.api_key.trim())) {
            headers.insert(AUTHORIZATION, val);
        }
        headers
    }

    pub async fn get_player_by_nickname(&self, nickname: &str) -> Result<Option<FaceitPlayer>, reqwest::Error> {
        let encoded_nick: String = url::form_urlencoded::byte_serialize(nickname.as_bytes()).collect();
        let url = format!(
            "https://open.faceit.com/data/v4/players?nickname={}",
            encoded_nick
        );
        let resp = self.client.get(&url).headers(self.headers()).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        let player = resp.json::<FaceitPlayer>().await?;
        Ok(Some(player))
    }

    pub async fn get_player_by_id(&self, player_id: &str) -> Result<Option<FaceitPlayer>, reqwest::Error> {
        let url = format!("https://open.faceit.com/data/v4/players/{}", player_id);
        let resp = self.client.get(&url).headers(self.headers()).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        let player = resp.json::<FaceitPlayer>().await?;
        Ok(Some(player))
    }

    pub async fn get_player_stats(&self, player_id: &str) -> Result<Option<FaceitPlayerStats>, reqwest::Error> {
        let url = format!("https://open.faceit.com/data/v4/players/{}/stats/cs2", player_id);
        let resp = self.client.get(&url).headers(self.headers()).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        let stats = resp.json::<FaceitPlayerStats>().await?;
        Ok(Some(stats))
    }

    pub async fn get_leaderboard(&self, leaderboard_id: &str) -> Result<Option<LeaderboardResponse>, reqwest::Error> {
        let url = format!(
            "https://open.faceit.com/data/v4/leaderboards/{}?limit=20",
            leaderboard_id
        );
        let resp = self.client.get(&url).headers(self.headers()).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        let lb = resp.json::<LeaderboardResponse>().await?;
        Ok(Some(lb))
    }
}
