//! Tests de `GET /api/lobby/matches-history` (`public_api::matches_history_router`).
//!
//! Fuente (legacy, solo lectura): `routes/lobby.js:5030` y `database.js:1669` (`getMatchHistory`).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use data::{FakeMatchHistoryRepo, MatchHistoryEntry};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::routes::public_api;

const MACHU: &str = "1274598531759345674";
const HUB: &str = "1472731390675255410";

fn entry(id: &str, guild: &str, secs: i64) -> MatchHistoryEntry {
    serde_json::from_value(json!({
        "id": id,
        "guild_id": guild,
        "map": "de_mirage",
        "createdAt": { "_seconds": secs, "_nanoseconds": 0 },
    }))
    .unwrap()
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn app_with(entries: Vec<MatchHistoryEntry>) -> Router {
    let repo = FakeMatchHistoryRepo::new();
    for e in entries {
        repo.push(e);
    }
    public_api::matches_history_router(Arc::new(repo))
}

fn ids(body: &Value) -> Vec<&str> {
    body["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn returns_real_matches_newest_first_without_a_guild() {
    let app = app_with(vec![
        entry("old", MACHU, 100),
        entry("new", "otra", 300),
        entry("mid", MACHU, 200),
    ]);
    let (status, body) = get(&app, "/api/lobby/matches-history?limit=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), ["new", "mid", "old"]);
    assert_eq!(body["history"][0]["map"], "de_mirage");
}

#[tokio::test]
async fn machupeek_and_hub_guilds_share_one_history() {
    let app = app_with(vec![
        entry("machu", MACHU, 200),
        entry("hub", HUB, 300),
        entry("other", "otra", 400),
    ]);
    let (_, body) = get(&app, &format!("/api/lobby/matches-history?guildId={MACHU}")).await;
    assert_eq!(ids(&body), ["hub", "machu"]);
    let (_, body) = get(&app, "/api/lobby/matches-history?guildId=otra").await;
    assert_eq!(ids(&body), ["other"]);
}

#[tokio::test]
async fn limit_is_clamped_and_alias_path_works() {
    let many: Vec<_> = (0..60).map(|i| entry(&format!("m{i}"), MACHU, i)).collect();
    let app = app_with(many);
    let (_, body) = get(&app, "/api/lobby/matches-history?limit=500").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 50);
    let (_, body) = get(&app, "/api/lobby/match-history?limit=3").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 3);
    let (_, body) = get(&app, "/api/lobby/matches-history").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 20);
}

struct FakeAvatars(std::collections::HashMap<String, String>);

#[async_trait::async_trait]
impl web::steam_avatars::SteamAvatars for FakeAvatars {
    async fn fetch_many(&self, ids: &[String]) -> std::collections::HashMap<String, String> {
        ids.iter()
            .filter_map(|id| self.0.get(id).map(|a| (id.clone(), a.clone())))
            .collect()
    }
}

const STEAM_A: &str = "76561198000000001";
const STEAM_B: &str = "76561198000000002";

fn detail_app(docs: data::InMemoryNodeDocs, users: data::FakeUsersRepo) -> Router {
    public_api::match_detail_router(public_api::MatchDetailState {
        docs: Arc::new(data::FirestoreAdminDocsRepo::new(docs)),
        users: Arc::new(users),
        avatars: Arc::new(FakeAvatars(
            [
                (STEAM_A.to_string(), "https://steam/a.jpg".to_string()),
                (STEAM_B.to_string(), "https://steam/b.jpg".to_string()),
            ]
            .into(),
        )),
    })
}

#[tokio::test]
async fn detail_adds_steam_avatars_to_rosters_and_stats() {
    let docs = data::InMemoryNodeDocs::new();
    docs.put(
        "faceit_matches_history",
        "m1",
        json!({
            "id": "m1",
            "faction1_roster": [{ "id": STEAM_A, "nickname": "Ana" }],
            "faction2_roster": [{ "id": "uuid-b", "nickname": "Beto", "discord_id": "222" }],
        }),
    );
    docs.put(
        "faceit_match_stats",
        "m1",
        json!({ "teams": [{ "players": [
            { "playerId": STEAM_A, "nickname": "Ana" },
            { "playerId": "uuid-b", "nickname": "Beto" },
        ]}]}),
    );
    let users = data::FakeUsersRepo::new();
    users.insert("222", json!({ "steam_id_64": STEAM_B, "discord_avatar": "https://dc/b.png" }));

    let (status, body) = get(&detail_app(docs, users), "/api/lobby/matches-history/m1").await;
    assert_eq!(status, StatusCode::OK);
    let h = &body["history"];
    assert_eq!(h["faction1_roster"][0]["avatar"], "https://steam/a.jpg");
    assert_eq!(h["faction1_roster"][0]["steam_id_64"], STEAM_A);
    assert_eq!(h["faction2_roster"][0]["avatar"], "https://steam/b.jpg");
    assert_eq!(h["faction2_roster"][0]["steam_id_64"], STEAM_B);
    let players = &body["stats"]["teams"][0]["players"];
    assert_eq!(players[0]["avatar"], "https://steam/a.jpg");
    assert_eq!(players[1]["avatar"], "https://steam/b.jpg");
    assert_eq!(players[1]["steam_id_64"], STEAM_B);
}

#[tokio::test]
async fn detail_falls_back_to_the_linked_account_photo_and_embedded_stats() {
    let docs = data::InMemoryNodeDocs::new();
    docs.put(
        "faceit_matches_history",
        "m2",
        json!({
            "id": "m2",
            "faction1_roster": [{ "id": "x", "nickname": "Sin Steam", "discord_id": "333" }],
            "stats": { "teams": [] },
        }),
    );
    let users = data::FakeUsersRepo::new();
    users.insert("333", json!({ "discord_avatar": "https://dc/c.png" }));

    let (status, body) = get(&detail_app(docs, users), "/api/lobby/match-history/m2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["history"]["faction1_roster"][0]["avatar"], "https://dc/c.png");
    assert_eq!(body["history"]["faction1_roster"][0]["steam_id_64"], Value::Null);
    assert_eq!(body["stats"], json!({ "teams": [] }));
}

#[tokio::test]
async fn detail_of_an_unknown_match_is_404() {
    let app = detail_app(data::InMemoryNodeDocs::new(), data::FakeUsersRepo::new());
    let (status, body) = get(&app, "/api/lobby/matches-history/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Partida no encontrada.");
}

#[tokio::test]
async fn read_failure_returns_an_empty_history_like_the_legacy() {
    let repo = FakeMatchHistoryRepo::new();
    repo.fail_with(Some(data::DataError::QuotaExceeded));
    let app = public_api::matches_history_router(Arc::new(repo));
    let (status, body) = get(&app, "/api/lobby/matches-history").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["history"], json!([]));
}
