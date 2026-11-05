//! Pruebas de integración para el webhook de MatchZy con liquidación de partidas (`crates/web/tests/matchzy_settle.rs`).
//!
//! Cubre:
//! (1) series_end con cabecera correcta, winner {"team":"team1"} sobre una partida con apuesta 10 y 10 jugadores.
//! (2) segundo series_end igual -> resolved:false y ningún saldo cambia.
//! (3) map_end NO cierra la partida (el doc activo sigue y no se paga).
//! (4) cabecera ausente o incorrecta con secreto configurado -> 401 y nada cambia (incluso para map_end u otros eventos).
//! (5) sin secreto configurado y partida con apuesta -> 401 y el doc sigue activo; sin apuesta y sin secreto -> se cierra.
//! (6) winner como texto "team2" y marcador (1, 2) -> gana faction2.
//! (7) series_end sin winner decide por marcador.

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

const SECRET: &str = "test_matchzy_shared_secret_abc987";

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

// (1) series_end con cabecera correcta, winner {"team":"team1"} sobre una partida con apuesta 10 y 10 jugadores
// -> ganadores cobran el pozo/5, el doc activo desaparece, historial finished.
#[tokio::test]
async fn test_1_series_end_with_bet_winner_team1_object() {
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
        .insert("faceit_active_matches", "match_matchzy_1", active_match);

    let (status, resp) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "series_end",
            "matchid": "match_matchzy_1",
            "winner": { "team": "team1" },
            "team1_series_score": 2,
            "team2_series_score": 0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["event"], "series_end");
    assert_eq!(resp["resolved"], true);
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

    // El documento activo desaparece
    assert_eq!(
        ctx.docs.get("faceit_active_matches", "match_matchzy_1"),
        None
    );

    // El historial existe con status finished y winner faction1
    let hist = ctx
        .docs
        .get("faceit_matches_history", "match_matchzy_1")
        .expect("debe existir en historial");
    assert_eq!(hist["status"], "finished");
    assert_eq!(hist["winner"], "faction1");
    assert_eq!(hist["faction1_score"], 2);
    assert_eq!(hist["faction2_score"], 0);
}

// (2) Segundo series_end igual -> resolved:false y ningún saldo cambia.
#[tokio::test]
async fn test_2_duplicate_series_end_does_not_double_pay() {
    let ctx = setup(Some(SECRET));

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
        .insert("faceit_active_matches", "match_matchzy_2", active_match);

    let (status1, resp1) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "series_end",
            "matchid": "match_matchzy_2",
            "winner": { "team": "team1" },
            "team1_series_score": 2,
            "team2_series_score": 1
        }),
    )
    .await;

    assert_eq!(status1, StatusCode::OK);
    assert_eq!(resp1["resolved"], true);
    assert_eq!(resp1["payoutsApplied"], 5);

    // Segundo series_end para la misma partida
    let (status2, resp2) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "series_end",
            "matchid": "match_matchzy_2",
            "winner": { "team": "team1" },
            "team1_series_score": 2,
            "team2_series_score": 1
        }),
    )
    .await;

    assert_eq!(status2, StatusCode::OK);
    assert_eq!(resp2["success"], true);
    assert_eq!(resp2["event"], "series_end");
    assert_eq!(resp2["resolved"], false);

    // Saldos intactos: no hubo doble pago
    for i in 1..=5 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0, "discord_{i} no debe recibir doble premio");
    }
    for i in 6..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0);
    }
}

// (3) map_end NO cierra la partida (el doc activo sigue y no se paga).
#[tokio::test]
async fn test_3_map_end_does_not_close_match_or_payout() {
    let ctx = setup(Some(SECRET));

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
        .insert("faceit_active_matches", "match_map_end", active_match);

    let (status, resp) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "map_end",
            "matchid": "match_map_end",
            "winner": { "team": "team1" },
            "team1_series_score": 1,
            "team2_series_score": 0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["event"], "map_end");

    // El documento activo sigue presente
    assert!(
        ctx.docs
            .get("faceit_active_matches", "match_map_end")
            .is_some(),
        "el documento activo no debe ser borrado en map_end"
    );

    // Historial no se ha creado
    assert_eq!(
        ctx.docs.get("faceit_matches_history", "match_map_end"),
        None,
        "no debe existir historial en map_end"
    );

    // Los saldos no se alteraron
    for i in 1..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0, "los saldos no deben cambiar en map_end");
    }
}

// (4) Cabecera ausente o incorrecta con secreto configurado -> 401 y nada cambia (incluso para map_end u otros eventos).
#[tokio::test]
async fn test_4_auth_failure_with_configured_secret() {
    let ctx = setup(Some(SECRET));

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
        .insert("faceit_active_matches", "match_auth_test", active_match);

    // 4a: series_end sin cabecera
    let (status1, resp1) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        None,
        json!({
            "event": "series_end",
            "matchid": "match_auth_test",
            "winner": "team1",
        }),
    )
    .await;
    assert_eq!(status1, StatusCode::UNAUTHORIZED);
    assert_eq!(resp1["error"], "No autorizado.");

    // 4b: series_end con cabecera incorrecta
    let (status2, resp2) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some("secreto_invalido"),
        json!({
            "event": "series_end",
            "matchid": "match_auth_test",
            "winner": "team1",
        }),
    )
    .await;
    assert_eq!(status2, StatusCode::UNAUTHORIZED);
    assert_eq!(resp2["error"], "No autorizado.");

    // 4c: map_end sin cabecera
    let (status3, resp3) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        None,
        json!({
            "event": "map_end",
            "matchid": "match_auth_test",
        }),
    )
    .await;
    assert_eq!(status3, StatusCode::UNAUTHORIZED);
    assert_eq!(resp3["error"], "No autorizado.");

    // 4d: otro evento con cabecera incorrecta
    let (status4, resp4) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some("wrong_key"),
        json!({
            "event": "round_end",
            "matchid": "match_auth_test",
        }),
    )
    .await;
    assert_eq!(status4, StatusCode::UNAUTHORIZED);
    assert_eq!(resp4["error"], "No autorizado.");

    // Documento activo intacto
    assert!(ctx
        .docs
        .get("faceit_active_matches", "match_auth_test")
        .is_some());

    // Historial no existe
    assert_eq!(
        ctx.docs.get("faceit_matches_history", "match_auth_test"),
        None
    );

    // Saldos intactos
    for i in 1..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0);
    }
}

// (5) Sin secreto configurado y partida con apuesta -> 401 y el doc sigue activo;
// sin apuesta y sin secreto -> se cierra.
#[tokio::test]
async fn test_5_unconfigured_secret_behavior() {
    let ctx = setup(None);

    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    // 5a: Partida con apuesta sin secreto -> 401
    let active_with_bet = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "total_pot": 100.0,
        "faction1_name": "Team A",
        "faction2_name": "Team B",
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });
    ctx.docs
        .insert("faceit_active_matches", "match_bet_no_sec", active_with_bet);

    let (status_bet, resp_bet) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        None,
        json!({
            "event": "series_end",
            "matchid": "match_bet_no_sec",
            "winner": { "team": "team1" },
            "team1_series_score": 2,
            "team2_series_score": 0
        }),
    )
    .await;

    assert_eq!(status_bet, StatusCode::UNAUTHORIZED);
    assert_eq!(resp_bet["error"], "No autorizado.");
    assert!(ctx
        .docs
        .get("faceit_active_matches", "match_bet_no_sec")
        .is_some());

    for i in 1..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0);
    }

    // 5b: Partida sin apuesta sin secreto -> 200 y se cierra
    let active_no_bet = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 0.0,
        "total_pot": 0.0,
        "faction1_name": "Team A",
        "faction2_name": "Team B",
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });
    ctx.docs
        .insert("faceit_active_matches", "match_free_no_sec", active_no_bet);

    let (status_free, resp_free) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        None,
        json!({
            "event": "series_end",
            "matchid": "match_free_no_sec",
            "winner": "team1",
            "team1_series_score": 2,
            "team2_series_score": 0
        }),
    )
    .await;

    assert_eq!(status_free, StatusCode::OK);
    assert_eq!(resp_free["success"], true);
    assert_eq!(resp_free["event"], "series_end");
    assert_eq!(resp_free["resolved"], true);
    assert_eq!(resp_free["payoutsApplied"], 0);

    // El documento activo desaparece
    assert_eq!(
        ctx.docs.get("faceit_active_matches", "match_free_no_sec"),
        None
    );

    // Historial creado
    let hist = ctx
        .docs
        .get("faceit_matches_history", "match_free_no_sec")
        .expect("debe existir en historial");
    assert_eq!(hist["status"], "finished");
    assert_eq!(hist["winner"], "faction1");

    // Saldos intactos
    for i in 1..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0);
    }
}

// (6) winner como texto "team2" y marcador (1, 2) -> gana faction2.
#[tokio::test]
async fn test_6_winner_as_text_team2_and_scores() {
    let ctx = setup(Some(SECRET));

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
        .insert("faceit_active_matches", "match_team2", active_match);

    let (status, resp) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "series_end",
            "matchid": "match_team2",
            "winner": "team2",
            "team1_series_score": 1,
            "team2_series_score": 2
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["resolved"], true);
    assert_eq!(resp["payoutsApplied"], 5);

    // faction2 (discord_6..=10) ganan 20 (90 + 20 = 110)
    for i in 6..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0, "discord_{i} de faction2 debía recibir premio");
    }

    // faction1 (discord_1..=5) se quedan en 90
    for i in 1..=5 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 90.0, "discord_{i} de faction1 debía mantener su saldo");
    }

    assert_eq!(ctx.docs.get("faceit_active_matches", "match_team2"), None);

    let hist = ctx
        .docs
        .get("faceit_matches_history", "match_team2")
        .expect("historial debe existir");
    assert_eq!(hist["winner"], "faction2");
    assert_eq!(hist["faction1_score"], 1);
    assert_eq!(hist["faction2_score"], 2);
}

// (7) series_end sin winner decide por marcador.
#[tokio::test]
async fn test_7_series_end_without_winner_decides_by_score() {
    let ctx = setup(Some(SECRET));

    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    // 7a: Marcador 2-1 a favor de faction1
    let active_a = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "total_pot": 100.0,
        "faction1_name": "Team A",
        "faction2_name": "Team B",
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });
    ctx.docs
        .insert("faceit_active_matches", "match_score_a", active_a);

    let (status_a, resp_a) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "series_end",
            "matchid": "match_score_a",
            "winner": null,
            "team1_series_score": 2,
            "team2_series_score": 1
        }),
    )
    .await;

    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(resp_a["resolved"], true);
    assert_eq!(resp_a["payoutsApplied"], 5);

    // faction1 cobra
    for i in 1..=5 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0);
    }

    let hist_a = ctx
        .docs
        .get("faceit_matches_history", "match_score_a")
        .unwrap();
    assert_eq!(hist_a["winner"], "faction1");

    // 7b: Marcador 0-2 a favor de faction2
    // Reiniciar saldos para 7b
    for i in 1..=10 {
        ctx.wallet
            .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
    }

    let active_b = json!({
        "guild_id": "g100",
        "bet_amount_per_player": 10.0,
        "total_pot": 100.0,
        "faction1_name": "Team A",
        "faction2_name": "Team B",
        "faction1_roster": make_roster("A", 1, 5),
        "faction2_roster": make_roster("B", 6, 10),
    });
    ctx.docs
        .insert("faceit_active_matches", "match_score_b", active_b);

    let (status_b, resp_b) = post_json(
        &ctx.app,
        "/api/matchzy/event",
        Some(SECRET),
        json!({
            "event": "series_end",
            "matchid": "match_score_b",
            "team1_series_score": 0,
            "team2_series_score": 2
        }),
    )
    .await;

    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(resp_b["resolved"], true);
    assert_eq!(resp_b["payoutsApplied"], 5);

    // faction2 cobra
    for i in 6..=10 {
        let bal = ctx
            .wallet
            .balance(Some("g100"), &format!("discord_{i}"))
            .await
            .unwrap();
        assert_eq!(bal, 110.0);
    }

    let hist_b = ctx
        .docs
        .get("faceit_matches_history", "match_score_b")
        .unwrap();
    assert_eq!(hist_b["winner"], "faction2");
}

// (8) series_end sin winner y con empate o marcador ausente: NO se adivina un ganador (el legado
// pagaba al equipo A); se cancela y se reembolsa la cuota a todos.
#[tokio::test]
async fn test_8_series_end_without_winner_and_tie_refunds_everyone() {
    let ctx = setup(Some(SECRET));

    for (id, body) in [
        (
            "match_tie",
            json!({
                "event": "series_end",
                "matchid": "match_tie",
                "winner": null,
                "team1_series_score": 1,
                "team2_series_score": 1
            }),
        ),
        (
            "match_noscore",
            json!({
                "event": "series_end",
                "matchid": "match_noscore"
            }),
        ),
    ] {
        // Cada jugador ya pagó 10 al lanzar: saldo 90.
        for i in 1..=10 {
            ctx.wallet
                .set_balance(Some("g100"), &format!("discord_{i}"), 90.0);
        }
        ctx.docs.insert(
            "faceit_active_matches",
            id,
            json!({
                "guild_id": "g100",
                "bet_amount_per_player": 10.0,
                "total_pot": 100.0,
                "faction1_name": "Team A",
                "faction2_name": "Team B",
                "faction1_roster": make_roster("A", 1, 5),
                "faction2_roster": make_roster("B", 6, 10),
            }),
        );

        let (status, resp) = post_json(&ctx.app, "/api/matchzy/event", Some(SECRET), body).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(resp["resolved"], true, "{id}");
        assert_eq!(resp["payoutsApplied"], 10, "{id}: se reembolsa a los 10");

        // Todos recuperan su cuota (90 + 10), nadie cobra el pozo.
        for i in 1..=10 {
            let bal = ctx
                .wallet
                .balance(Some("g100"), &format!("discord_{i}"))
                .await
                .unwrap();
            assert_eq!(bal, 100.0, "{id}: jugador {i} debe quedar con 100");
        }

        let hist = ctx.docs.get("faceit_matches_history", id).unwrap();
        assert_eq!(hist["status"], "cancelled", "{id}");
        assert_eq!(hist["winner"], "CANCELLED", "{id}");
        assert!(ctx.docs.get("faceit_active_matches", id).is_none(), "{id}");
    }
}
