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

#[tokio::test]
async fn read_failure_returns_an_empty_history_like_the_legacy() {
    let repo = FakeMatchHistoryRepo::new();
    repo.fail_with(Some(data::DataError::QuotaExceeded));
    let app = public_api::matches_history_router(Arc::new(repo));
    let (status, body) = get(&app, "/api/lobby/matches-history").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["history"], json!([]));
}
