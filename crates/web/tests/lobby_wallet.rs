//! Pruebas de integración para las rutas de monedero del lobby (Tarea B15):
//! - GET /api/lobby/player-balance/{discord_id}
//! - POST /api/lobby/quick-add-balance
//! - POST /api/lobby/mixton-check-bet-status
//! - 503 sin monedero configurado (wallet: None)

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use data::{FakeWalletStore, WalletStore};
use lobby::state::DiscordUserDetails;
use lobby::{LobbyApiState, LobbyManager, LobbyState, LobbyTeam, Player};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-wallet-integration-test-secret-32-chars!";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_token(user_id: &str, role: &str, is_admin: bool, servidor: Option<&str>) -> String {
    let c = new_claims(
        user_id.into(),
        "Tester".into(),
        Some("avatar.png".into()),
        role.into(),
        is_admin,
        servidor.map(str::to_owned),
        now(),
    );
    sign(&c, SECRET.as_bytes()).unwrap()
}

fn test_router(manager: Arc<LobbyManager>, wallet: Option<Arc<dyn WalletStore>>) -> Router {
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let tmp_uploads = tempfile::tempdir().expect("tempdir");
    let uploads_dir = tmp_uploads.path().to_path_buf();
    std::mem::forget(tmp_uploads);
    lobby::router(LobbyApiState {
        manager,
        auth: Arc::new(session),
        uploads_dir,
        cs2: None,
        docs: None,
        notifier: Default::default(),
        wallet,
    })
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn make_player(discord_id: &str, nick: &str, steam_id_64: Option<&str>) -> Player {
    Player {
        discord_id: discord_id.to_string(),
        nickname: nick.to_string(),
        faceit_nickname: Some(nick.to_string()),
        faceit_elo: 1200,
        ready: true,
        tier: "C".to_string(),
        steam_id_64: steam_id_64.map(str::to_string),
        discord_user: Some(DiscordUserDetails {
            username: nick.to_string(),
            display_name: nick.to_string(),
            avatar_url: format!("https://cdn.discordapp.com/avatars/{discord_id}/avatar.png"),
        }),
        ..Default::default()
    }
}

// -----------------------------------------------------------------------------
// Pruebas de player-balance
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_player_balance_con_saldo_y_sin_saldo() {
    let manager = Arc::new(LobbyManager::new("default"));
    let fake_wallet = Arc::new(FakeWalletStore::new());

    // Cargamos saldo previo para el jugador 111111111111111111 en servidor default
    fake_wallet
        .adjust(
            Some("default"),
            "111111111111111111",
            75.50,
            "Carga inicial",
        )
        .await
        .unwrap();

    let app = test_router(manager, Some(fake_wallet));
    let token = make_token("111111111111111111", "player", false, Some("default"));

    // 1. Jugador con saldo
    let req = Request::builder()
        .method("GET")
        .uri("/api/lobby/player-balance/111111111111111111?guild_id=default")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["success"], true);
    assert_eq!(json["discordId"], "111111111111111111");
    assert_eq!(json["balance"], 75.5);

    // 2. Jugador sin saldo -> 0
    let req2 = Request::builder()
        .method("GET")
        .uri("/api/lobby/player-balance/999999999999999999?guild_id=default")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    let res2 = app.oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let json2 = body_json(res2).await;
    assert_eq!(json2["success"], true);
    assert_eq!(json2["discordId"], "999999999999999999");
    assert_eq!(json2["balance"], 0.0);
}

// -----------------------------------------------------------------------------
// Pruebas de quick-add-balance
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_quick_add_balance_permisos_y_validaciones() {
    let manager = Arc::new(LobbyManager::new("default"));
    let fake_wallet = Arc::new(FakeWalletStore::new());
    let app = test_router(manager, Some(fake_wallet.clone()));

    let user_token = make_token("user_normal", "player", false, Some("default"));
    let admin_token = make_token("admin_user", "admin", true, Some("default"));

    // 1. Usuario no admin -> 403
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "discordId": "target1", "amount": 25.0 }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 2. Faltan discordId o monto -> 400 "Faltan discordId o monto."
    let req_missing_id = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "amount": 25.0 }).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req_missing_id).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let json = body_json(res).await;
    assert_eq!(json["error"], "Faltan discordId o monto.");

    let req_missing_amount = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "discordId": "target1" }).to_string()))
        .unwrap();
    let res = app.clone().oneshot(req_missing_amount).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let json = body_json(res).await;
    assert_eq!(json["error"], "Faltan discordId o monto.");

    // 3. Monto no numérico, <= 0 -> 400 "El monto debe ser un número positivo."
    for invalid_val in [json!(-10.0), json!(0.0), json!("abc"), json!("-5")] {
        let req_invalid = Request::builder()
            .method("POST")
            .uri("/api/lobby/quick-add-balance?guild_id=default")
            .header(header::COOKIE, format!("auth_token={admin_token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "discordId": "target1", "amount": invalid_val }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req_invalid).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let json = body_json(res).await;
        assert_eq!(json["error"], "El monto debe ser un número positivo.");
    }

    // 4. Admin OK -> 200 y el saldo en FakeWalletStore sube
    let req_ok = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "discordId": "target1",
                "amount": "50.25",
                "reason": "Bono de bienvenida"
            })
            .to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req_ok).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["success"], true);
    assert_eq!(json["discordId"], "target1");
    assert_eq!(json["newBalance"], 50.25);

    // Verificamos que el saldo en el store subió efectivamente
    let bal = fake_wallet
        .balance(Some("default"), "target1")
        .await
        .unwrap();
    assert_eq!(bal, 50.25);

    // Segunda recarga sumando 10 -> nuevo saldo 60.25
    let req_ok2 = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "discordId": "target1", "amount": 10.0 }).to_string(),
        ))
        .unwrap();
    let res = app.oneshot(req_ok2).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["newBalance"], 60.25);
    let bal2 = fake_wallet
        .balance(Some("default"), "target1")
        .await
        .unwrap();
    assert_eq!(bal2, 60.25);
}

// -----------------------------------------------------------------------------
// Pruebas de mixton-check-bet-status
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_mixton_check_bet_status_10_jugadores_y_saldos_mezclados() {
    let manager = Arc::new(LobbyManager::new("default"));
    let fake_wallet = Arc::new(FakeWalletStore::new());

    // Configuramos 10 jugadores en el lobby: Team A (cap A + 4), Team B (cap B + 4)
    let p_a1 = make_player("p_a1", "CapitanA", Some("76561198000000001"));
    let p_a2 = make_player("p_a2", "PlayerA2", Some("76561198000000002"));
    let p_a3 = make_player("p_a3", "PlayerA3", Some("76561198000000003"));
    let p_a4 = make_player("p_a4", "PlayerA4", Some("76561198000000004"));
    let p_a5 = make_player("p_a5", "PlayerA5", Some("76561198000000005"));

    let p_b1 = make_player("p_b1", "CapitanB", Some("76561198000000006"));
    let p_b2 = make_player("p_b2", "PlayerB2", Some("76561198000000007"));
    let p_b3 = make_player("p_b3", "PlayerB3", Some("76561198000000008"));
    let p_b4 = make_player("p_b4", "PlayerB4", None); // Sin SteamID!
    let p_b5 = make_player("p_b5", "PlayerB5", Some("76561198000000010"));

    let team_a = LobbyTeam {
        id: "team_alpha".to_string(),
        name: "Alpha".to_string(),
        tag: "ALPHA".to_string(),
        captain: Some(p_a1.clone()),
        players: vec![p_a2.clone(), p_a3.clone(), p_a4.clone(), p_a5.clone()],
    };
    let team_b = LobbyTeam {
        id: "team_beta".to_string(),
        name: "Beta".to_string(),
        tag: "BETA".to_string(),
        captain: Some(p_b1.clone()),
        players: vec![p_b2.clone(), p_b3.clone(), p_b4.clone(), p_b5.clone()],
    };

    let mut state = LobbyState::default();
    state.teams = vec![team_a, team_b];
    state.effective_bet_amount = 10.0;
    manager.update_state("default", state).await;

    // Asignamos saldos en fake_wallet para apuesta de 10.0:
    // Mayoría tiene 15.0 (hasEnough = true, deficit = 0.0)
    // p_a3 tiene 5.0 (hasEnough = false, deficit = 5.0)
    // p_b5 tiene 0.0 (hasEnough = false, deficit = 10.0)
    for id in [
        "p_a1", "p_a2", "p_a4", "p_a5", "p_b1", "p_b2", "p_b3", "p_b4",
    ] {
        fake_wallet
            .adjust(Some("default"), id, 15.0, "Saldo inicial")
            .await
            .unwrap();
    }
    fake_wallet
        .adjust(Some("default"), "p_a3", 5.0, "Saldo parcial")
        .await
        .unwrap();

    let app = test_router(manager, Some(fake_wallet));
    let token = make_token("p_a1", "player", false, Some("default"));

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-check-bet-status?guild_id=default")
        .header(header::COOKIE, format!("auth_token={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "teamIdA": "team_alpha",
                "teamIdB": "team_beta",
                "betAmount": 10.0
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;

    assert_eq!(json["betAmount"], 10.0);
    assert_eq!(json["totalPot"], 100.0);
    // allReady debe ser false porque p_a3 y p_b5 no tienen suficiente saldo, y p_b4 no tiene SteamID
    assert_eq!(json["allReady"], false);

    assert_eq!(json["teamA"]["id"], "team_alpha");
    assert_eq!(json["teamB"]["id"], "team_beta");

    let players = json["players"].as_array().unwrap();
    assert_eq!(players.len(), 10);

    // Verificamos p_a1 (capitán A, saldo 15, apuesta 10)
    let pl_a1 = players.iter().find(|p| p["discordId"] == "p_a1").unwrap();
    assert_eq!(pl_a1["isCaptain"], true);
    assert_eq!(pl_a1["team"], "A");
    assert_eq!(pl_a1["balance"], 15.0);
    assert_eq!(pl_a1["hasEnough"], true);
    assert_eq!(pl_a1["deficit"], 0.0);
    assert_eq!(pl_a1["hasSteam"], true);

    // Verificamos p_a3 (saldo 5, apuesta 10 -> deficit 5.0)
    let pl_a3 = players.iter().find(|p| p["discordId"] == "p_a3").unwrap();
    assert_eq!(pl_a3["balance"], 5.0);
    assert_eq!(pl_a3["hasEnough"], false);
    assert_eq!(pl_a3["deficit"], 5.0);

    // Verificamos p_b5 (saldo 0, apuesta 10 -> deficit 10.0)
    let pl_b5 = players.iter().find(|p| p["discordId"] == "p_b5").unwrap();
    assert_eq!(pl_b5["balance"], 0.0);
    assert_eq!(pl_b5["hasEnough"], false);
    assert_eq!(pl_b5["deficit"], 10.0);

    // Verificamos missingSteamPlayers -> debe contener a p_b4
    let missing_steam = json["missingSteamPlayers"].as_array().unwrap();
    assert_eq!(missing_steam.len(), 1);
    assert_eq!(missing_steam[0]["discordId"], "p_b4");
    assert_eq!(missing_steam[0]["displayName"], "PlayerB4");
}

// -----------------------------------------------------------------------------
// Pruebas de 503 sin wallet (wallet: None)
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_rutas_con_wallet_none_devuelven_503() {
    let manager = Arc::new(LobbyManager::new("default"));
    let app = test_router(manager, None); // wallet: None

    let token = make_token("admin_user", "admin", true, Some("default"));

    // 1. GET /api/lobby/player-balance/{id}
    let req1 = Request::builder()
        .method("GET")
        .uri("/api/lobby/player-balance/12345?guild_id=default")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json1 = body_json(res1).await;
    assert_eq!(json1["error"], "El monedero no está configurado.");

    // 2. POST /api/lobby/quick-add-balance
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/lobby/quick-add-balance?guild_id=default")
        .header(header::COOKIE, format!("auth_token={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "discordId": "12345", "amount": 10.0 }).to_string(),
        ))
        .unwrap();
    let res2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json2 = body_json(res2).await;
    assert_eq!(json2["error"], "El monedero no está configurado.");

    // 3. POST /api/lobby/mixton-check-bet-status
    let req3 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-check-bet-status?guild_id=default")
        .header(header::COOKIE, format!("auth_token={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "teamIdA": "a", "teamIdB": "b", "betAmount": 10.0 }).to_string(),
        ))
        .unwrap();
    let res3 = app.oneshot(req3).await.unwrap();
    assert_eq!(res3.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json3 = body_json(res3).await;
    assert_eq!(json3["error"], "El monedero no está configurado.");
}
