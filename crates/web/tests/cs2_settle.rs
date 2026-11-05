//! Pruebas de integración de la liquidación de partidas CS2 (`crates/web/tests/cs2_settle.rs`).
//!
//! Cubre:
//! (1) Partida con apuesta 10 y 10 jugadores: match-finish con cabecera correcta y winner team1.
//! (2) Segundo match-finish igual: resolved:false y sin doble pago.
//! (3) Dos match-finish concurrentes: solo un conjunto de pagos.
//! (4) match-cancel con apuesta: reembolsa 10 a los que no estaban en manual_paid_players, status cancelled, winner CANCELLED.
//! (5) Cabecera incorrecta o ausente con secreto configurado: 401 y nada cambia.
//! (6) Sin secreto configurado y partida con apuesta: 401 y el doc sigue activo.
//! (7) Partida sin apuesta sin cabecera y sin secreto: se cierra y pasa al historial sin tocar saldos.
//! (8) Marcador sin winner decide por marcador.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use common::config::Config;
use data::{FakeDocSink, FakeWalletStore, WalletStore};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::{build_app, AppDeps};

const SECRET: &str = "test_cs2_api_shared_secret_xyz123";

fn test_config(secret: Option<&str>) -> Config {
    let mut vars = HashMap::new();
    vars.insert("ADMIN_JWT_SECRET".into(), "test_secret_1234567890".into());
    vars.insert("BASE_URL".into(), "http://localhost:15633".into());
    vars.insert("DISCORD_CLIENT_ID".into(), "id".into());
    vars.insert("DISCORD_CLIENT_SECRET".into(), "secret".into());
    vars.insert("DISCORD_TOKEN".into(), "token".into());
    vars.insert("DISCORD_GUILD_ID".into(), "guild".into());
    if let Some(s) = secret {
        vars.insert("CS2_API_SECRET".into(), s.into());
    }
    Config::from_map(&vars).unwrap()
}

struct TestContext {
    app: Router,
    docs: Arc<FakeDocSink>,
    wallet: Arc<FakeWalletStore>,
}

fn setup(secret: Option<&str>) -> TestContext {
    let cfg = test_config(secret);
    let docs = Arc::new(FakeDocSink::new());
    let wallet = Arc::new(FakeWalletStore::new());
    let cs2 = Arc::new(cs2::Cs2ServerManager::new(&cfg));

    let app = build_app(AppDeps {
        config: cfg,
        claims: None,
        admin: None,
        cs2: Some(cs2),
        lobby: None,
        docs: Some(docs.clone()),
        source: Some(docs.clone()),
        notifier: Default::default(),
        wallet: Some(wallet.clone()),
    });

    TestContext { app, docs, wallet }
}

async fn post_json(
    app: &Router,
    uri: &str,
    header_secret: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req_builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");

    if let Some(sec) = header_secret {
        req_builder = req_builder.header("x-cs2-secret", sec);
    }

    let req = req_builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn make_roster(prefix: &str, start: usize, end: usize) -> Vec<Value> {
    (start..=end)
        .map(|i| {
            json!({
                "id": format!("user_{i}"),
                "nickname": format!("{prefix}{i}"),
                "discord_id": format!("discord_{i}"),
            })
        })
        .collect()
}

// (1) Partida con apuesta 10 y 10 jugadores, rosters en el doc, saldos iniciales 90:
// match-finish con cabecera correcta y winner team1 -> los 5 de faction1 reciben 20 (total_pot 100/5) y los otros 5 no cambian;
// el doc activo desaparece; existe el historial con status finished y winner faction1
#[tokio::test]
async fn test_1_match_finish_with_bet_winner_team1() {
    let ctx = setup(Some(SECRET));

    // Inicializar saldos en 90
    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "total_pot": 100.0,
        "faction1_name": "Team A",
        "faction2_name": "Team B",
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });

    ctx.docs
        .insert("faceit_active_matches", "match_finish_1", active_match);

    let (status, resp) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        Some(SECRET),
        json!({
            "matchId": "match_finish_1",
            "winner": "team1",
            "team1_score": 13,
            "team2_score": 7,
            "stats": {"map": "de_dust2"}
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["resolved"], true);
    assert_eq!(resp["action"], "win_a");
    assert_eq!(resp["statsSaved"], true);
    assert_eq!(resp["payoutsApplied"], 5);

    // Los 5 de faction1 reciben 20 (90 + 20 = 110)
    for i in 1..=5 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0, "discord_{i} debía recibir 20 de premio");
    }

    // Los otros 5 no cambian (siguen en 90)
    for i in 6..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0, "discord_{i} debía mantener su saldo");
    }

    // Documento activo desaparece
    assert_eq!(
        ctx.docs.get("faceit_active_matches", "match_finish_1"),
        None
    );

    // Historial existe con status finished y winner faction1
    let hist = ctx
        .docs
        .get("faceit_matches_history", "match_finish_1")
        .expect("historial debe existir");
    assert_eq!(hist["status"], "finished");
    assert_eq!(hist["winner"], "faction1");
    assert_eq!(hist["faction1_score"], 13);
    assert_eq!(hist["faction2_score"], 7);
    assert_eq!(hist["stats"]["map"], "de_dust2");
    assert!(hist.get("createdAt").is_some());
    assert!(hist.get("updatedAt").is_some());
}

// (2) Segundo match-finish igual -> resolved:false y NINGÚN saldo cambia (sin doble pago)
#[tokio::test]
async fn test_2_second_match_finish_no_double_payment() {
    let ctx = setup(Some(SECRET));

    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "total_pot": 100.0,
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });

    ctx.docs
        .insert("faceit_active_matches", "match_repeat", active_match);

    // Primer cierre exitoso
    let (status1, resp1) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        Some(SECRET),
        json!({
            "matchId": "match_repeat",
            "winner": "team1",
        }),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);
    assert_eq!(resp1["resolved"], true);

    // Segundo cierre
    let (status2, resp2) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        Some(SECRET),
        json!({
            "matchId": "match_repeat",
            "winner": "team1",
        }),
    )
    .await;

    assert_eq!(status2, StatusCode::OK);
    assert_eq!(resp2["success"], true);
    assert_eq!(resp2["resolved"], false);
    assert_eq!(resp2["message"], "Match no encontrado o ya resuelto.");

    // Los saldos de los ganadores siguen en 110 (no 130)
    for i in 1..=5 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0, "discord_{i} no debe ser pagado dos veces");
    }
}

// (3) Dos match-finish concurrentes -> solo un conjunto de pagos
#[tokio::test]
async fn test_3_concurrent_match_finish_single_payout() {
    let ctx = setup(Some(SECRET));

    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "total_pot": 100.0,
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });

    ctx.docs
        .insert("faceit_active_matches", "match_concurrent", active_match);

    let app1 = ctx.app.clone();
    let app2 = ctx.app.clone();

    let task1 = tokio::spawn(async move {
        post_json(
            &app1,
            "/api/cs2/match-finish",
            Some(SECRET),
            json!({
                "matchId": "match_concurrent",
                "winner": "team1",
            }),
        )
        .await
    });

    let task2 = tokio::spawn(async move {
        post_json(
            &app2,
            "/api/cs2/match-finish",
            Some(SECRET),
            json!({
                "matchId": "match_concurrent",
                "winner": "team1",
            }),
        )
        .await
    });

    let (res1, res2) = tokio::join!(task1, task2);
    let (status1, resp1) = res1.unwrap();
    let (status2, resp2) = res2.unwrap();

    assert_eq!(status1, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);

    let resolved_count =
        (resp1["resolved"] == true) as usize + (resp2["resolved"] == true) as usize;
    assert_eq!(
        resolved_count, 1,
        "exactamente uno de los cierres concurrentes debe ser resuelto"
    );

    // Los saldos deben ser 110 (exactamente 1 pago)
    for i in 1..=5 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0);
    }
}

// (4) match-cancel con apuesta: reembolsa 10 a los que no estaban en manual_paid_players, status cancelled, winner CANCELLED
#[tokio::test]
async fn test_4_match_cancel_refunds_digital_payments() {
    let ctx = setup(Some(SECRET));

    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    // discord_2 y discord_8 pagaron manualmente en efectivo
    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "manual_paid_players": ["discord_2", "discord_8"],
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });

    ctx.docs
        .insert("faceit_active_matches", "match_cancel_test", active_match);

    let (status, resp) = post_json(
        &ctx.app,
        "/api/cs2/match-cancel",
        Some(SECRET),
        json!({
            "matchId": "match_cancel_test",
            "reason": "Servidor crasheado",
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["resolved"], true);
    assert_eq!(resp["action"], "cancel");
    // 10 jugadores en total menos 2 manual_paid = 8 reembolsos
    assert_eq!(resp["payoutsApplied"], 8);

    // Los 8 digitales suben de 90 a 100
    for i in [1, 3, 4, 5, 6, 7, 9, 10] {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 100.0, "discord_{i} debía recibir su reembolso de 10");
    }

    // Los 2 manual_paid siguen en 90
    for i in [2, 8] {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0, "discord_{i} no debía recibir reembolso digital");
    }

    // Historial registra status cancelled y winner CANCELLED
    let hist = ctx
        .docs
        .get("faceit_matches_history", "match_cancel_test")
        .expect("historial de cancelación debe existir");
    assert_eq!(hist["status"], "cancelled");
    assert_eq!(hist["winner"], "CANCELLED");
}

// (5) Cabecera incorrecta o ausente con secreto configurado -> 401 y nada cambia
#[tokio::test]
async fn test_5_secret_configured_unauthorized_requests() {
    let ctx = setup(Some(SECRET));

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "faction1_roster": [{"discord_id": "d1"}],
    });

    ctx.docs
        .insert("faceit_active_matches", "m_sec_test", active_match.clone());

    // 5a: Cabecera ausente
    let (status1, resp1) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        None,
        json!({"matchId": "m_sec_test"}),
    )
    .await;
    assert_eq!(status1, StatusCode::UNAUTHORIZED);
    assert_eq!(resp1["error"], "No autorizado.");
    assert!(ctx
        .docs
        .get("faceit_active_matches", "m_sec_test")
        .is_some());

    // 5b: Cabecera incorrecta
    let (status2, resp2) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        Some("clave_invalida"),
        json!({"matchId": "m_sec_test"}),
    )
    .await;
    assert_eq!(status2, StatusCode::UNAUTHORIZED);
    assert_eq!(resp2["error"], "No autorizado.");
    assert!(ctx
        .docs
        .get("faceit_active_matches", "m_sec_test")
        .is_some());

    // 5c: En match-cancel también
    let (status3, resp3) = post_json(
        &ctx.app,
        "/api/cs2/match-cancel",
        Some("clave_invalida"),
        json!({"matchId": "m_sec_test"}),
    )
    .await;
    assert_eq!(status3, StatusCode::UNAUTHORIZED);
    assert_eq!(resp3["error"], "No autorizado.");
    assert!(ctx
        .docs
        .get("faceit_active_matches", "m_sec_test")
        .is_some());
}

// (6) Sin secreto configurado y partida con apuesta -> 401 y el doc sigue activo
#[tokio::test]
async fn test_6_no_secret_configured_and_match_with_bet() {
    let ctx = setup(None); // secret = None

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 15.0,
        "faction1_roster": [{"discord_id": "d1"}],
    });

    ctx.docs
        .insert("faceit_active_matches", "m_nobet_sec", active_match.clone());

    let (status, resp) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        None,
        json!({"matchId": "m_nobet_sec"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(resp["error"], "No autorizado.");
    // El documento sigue activo (no fue reclamado)
    assert!(ctx
        .docs
        .get("faceit_active_matches", "m_nobet_sec")
        .is_some());
}

// (7) Partida sin apuesta sin cabecera y sin secreto -> se cierra y pasa al historial sin tocar saldos
#[tokio::test]
async fn test_7_match_without_bet_no_secret_closes_successfully() {
    let ctx = setup(None);

    ctx.wallet.set_balance(Some("g100"), "d1", 50.0);

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 0.0,
        "faction1_roster": [{"id": "p1", "nickname": "P1", "discord_id": "d1"}],
        "faction2_roster": [{"id": "p2", "nickname": "P2", "discord_id": "d2"}],
    });

    ctx.docs
        .insert("faceit_active_matches", "m_free_1", active_match);

    let (status, resp) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        None,
        json!({
            "matchId": "m_free_1",
            "winner": "faction1"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["resolved"], true);
    assert_eq!(resp["payoutsApplied"], 0);

    // Documento activo desaparece
    assert_eq!(ctx.docs.get("faceit_active_matches", "m_free_1"), None);

    // Historial registrado
    let hist = ctx
        .docs
        .get("faceit_matches_history", "m_free_1")
        .expect("historial debe existir");
    assert_eq!(hist["status"], "finished");
    assert_eq!(hist["winner"], "faction1");

    // Saldo no cambió
    let bal = ctx.wallet.balance(Some("g100"), "d1").await.unwrap();
    assert_eq!(bal, 50.0);
}

// (8) Marcador sin winner decide por marcador
#[tokio::test]
async fn test_8_score_without_winner_decides_action() {
    let ctx = setup(Some(SECRET));

    ctx.wallet.set_balance(Some("g100"), "d_b1", 100.0);

    let active_match = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 5.0,
        "total_pot": 10.0,
        "faction1_roster": [{"id": "p1", "nickname": "A1", "discord_id": "d_a1"}],
        "faction2_roster": [{"id": "p2", "nickname": "B1", "discord_id": "d_b1"}],
    });

    ctx.docs
        .insert("faceit_active_matches", "m_score_decision", active_match);

    // Marcador 11 a 13 a favor de team2, sin winner
    let (status, resp) = post_json(
        &ctx.app,
        "/api/cs2/match-finish",
        Some(SECRET),
        json!({
            "matchId": "m_score_decision",
            "team1_score": 11,
            "team2_score": 13
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["resolved"], true);
    assert_eq!(resp["action"], "win_b");
    assert_eq!(resp["payoutsApplied"], 1);

    // faction2 recibió los 10 de premio (100 + 10 = 110)
    let bal_b = ctx.wallet.balance(Some("g100"), "d_b1").await.unwrap();
    assert_eq!(bal_b, 110.0);

    // Historial ganador faction2
    let hist = ctx
        .docs
        .get("faceit_matches_history", "m_score_decision")
        .expect("historial existe");
    assert_eq!(hist["winner"], "faction2");
    assert_eq!(hist["faction1_score"], 11);
    assert_eq!(hist["faction2_score"], 13);
}
