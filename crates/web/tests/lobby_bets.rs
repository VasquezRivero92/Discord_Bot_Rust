//! Pruebas de integración para cobro de apuestas al lanzar partida (`crates/web/tests/lobby_bets.rs`).
//!
//! Cubre los 9 escenarios de TAREA B16b:
//! (1) Camino feliz admin con apuesta 10: 10 jugadores con saldo 100 -> 200 OK, saldos 90, doc activo con apuesta y pozo.
//! (2) Ciclo completo: tras lanzar, match-finish con x-cs2-secret y winner team1 -> 5 ganadores 110, 5 perdedores 90, historial finished.
//! (3) Un jugador con saldo insuficiente (5) -> 400 con unpaidPlayers y nadie cobrado (todos 100).
//! (4) Admin marca a un jugador como pagado manual -> no se le cobra (queda 100), los otros 9 pagan; en match-cancel solo se reembolsa a los 9.
//! (5) Rollback por fallo de débito en el 4.º jugador -> 500 y los 3 cobrados vuelven a su saldo original (100).
//! (6) Fallo de Firestore al escribir partida activa con apuesta -> 500 con reembolso y sin partida publicada en CS2.
//! (7) Sin bets_enabled (false) y apuesta > 0 -> 400 y saldos intactos.
//! (8) Capitán (no admin) con apuesta acordada 10 en estado -> se cobra 10 e ignora betAmount del body.
//! (9) Apuesta 0 -> no se cobra nada y funciona como antes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use common::config::Config;
use data::{DocSink, FakeDocSink, FakeWalletStore, WalletStore};
use lobby::state::{DiscordUserDetails, LobbyManager, LobbyState, Player};
use serde_json::{json, Value};
use tower::ServiceExt;
use web::session::{new_claims, sign};
use web::{build_app, AppDeps};

const SECRET: &str = "test_cs2_api_shared_secret_xyz123";
const JWT_SECRET: &str = "lobby-launch-test-secret-at-least-32-chars!";

const ALL_PLAYERS: [&str; 10] = [
    "cap_a", "a1", "a2", "a3", "a4", "cap_b", "b1", "b2", "b3", "b4",
];

fn now_secs() -> u64 {
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
        now_secs(),
    );
    sign(&c, JWT_SECRET.as_bytes()).unwrap()
}

fn test_config(secret: Option<&str>) -> Config {
    let mut vars = HashMap::new();
    vars.insert("ADMIN_JWT_SECRET".into(), JWT_SECRET.into());
    vars.insert("BASE_URL".into(), "http://localhost:15633".into());
    vars.insert("DISCORD_CLIENT_ID".into(), "id".into());
    vars.insert("DISCORD_CLIENT_SECRET".into(), "secret".into());
    vars.insert("DISCORD_TOKEN".into(), "token".into());
    vars.insert("DISCORD_GUILD_ID".into(), "default".into());
    if let Some(sec) = secret {
        vars.insert("CS2_API_SECRET".into(), sec.into());
    }
    Config::from_map(&vars).unwrap()
}

fn make_player(id: &str, steam: Option<&str>) -> Player {
    Player {
        discord_id: id.to_string(),
        nickname: format!("Nick_{id}"),
        faceit_player_id: Some(format!("faceit_{id}")),
        faceit_nickname: Some(format!("FC_{id}")),
        steam_id_64: steam.map(str::to_string),
        avatar: None,
        faceit_elo: 1500,
        faceit_level: Some(7),
        ready: true,
        tier: "A".to_string(),
        mixton_tier_override: None,
        discord_user: Some(DiscordUserDetails {
            username: format!("User_{id}"),
            display_name: format!("Display_{id}"),
            avatar_url: "".to_string(),
        }),
    }
}

fn setup_ten_players_state(steams_valid: bool) -> LobbyState {
    let mut state = LobbyState::default();
    let prefix_steam = |i: usize| {
        if steams_valid {
            Some(format!("76561198{:09}", i))
        } else {
            None
        }
    };

    let team_a = &mut state.teams[0];
    team_a.captain = Some(make_player("cap_a", prefix_steam(1).as_deref()));
    team_a.players = vec![
        make_player("a1", prefix_steam(2).as_deref()),
        make_player("a2", prefix_steam(3).as_deref()),
        make_player("a3", prefix_steam(4).as_deref()),
        make_player("a4", prefix_steam(5).as_deref()),
    ];

    let team_b = &mut state.teams[1];
    team_b.captain = Some(make_player("cap_b", prefix_steam(6).as_deref()));
    team_b.players = vec![
        make_player("b1", prefix_steam(7).as_deref()),
        make_player("b2", prefix_steam(8).as_deref()),
        make_player("b3", prefix_steam(9).as_deref()),
        make_player("b4", prefix_steam(10).as_deref()),
    ];

    state
}

struct TestContext {
    app: Router,
    cs2: Arc<cs2::Cs2ServerManager>,
    manager: Arc<LobbyManager>,
}

fn setup_app(
    secret: Option<&str>,
    wallet_opt: Option<Arc<dyn WalletStore>>,
    docs_opt: Option<Arc<dyn DocSink>>,
) -> (TestContext, Arc<FakeDocSink>, Arc<FakeWalletStore>) {
    let cfg = test_config(secret);
    let fake_docs = Arc::new(FakeDocSink::new());
    let fake_wallet = Arc::new(FakeWalletStore::new());
    let cs2 = Arc::new(cs2::Cs2ServerManager::new(&cfg));
    let manager = Arc::new(LobbyManager::new("default"));

    let docs_sink: Arc<dyn DocSink> = docs_opt.unwrap_or_else(|| fake_docs.clone());
    let wallet_store: Arc<dyn WalletStore> = wallet_opt.unwrap_or_else(|| fake_wallet.clone());

    let app = build_app(AppDeps {
        config: cfg,
        claims: None,
        admin: None,
        cs2: Some(cs2.clone()),
        lobby: Some(manager.clone()),
        docs: Some(docs_sink),
        source: Some(fake_docs.clone()),
        notifier: Default::default(),
        wallet: Some(wallet_store),
    });

    (TestContext { app, cs2, manager }, fake_docs, fake_wallet)
}

async fn post_lobby_launch(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header("content-type", "application/json")
        .header("cookie", format!("auth_token={token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn post_cs2_json(
    app: &Router,
    uri: &str,
    secret: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req_builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");

    if let Some(sec) = secret {
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

// Wrapper para simular fallo en el N-ésimo debit_exact
#[derive(Debug)]
struct FailingDebitWallet {
    inner: Arc<FakeWalletStore>,
    debit_count: std::sync::atomic::AtomicUsize,
    fail_on: usize,
}

#[async_trait::async_trait]
impl WalletStore for FailingDebitWallet {
    async fn balance(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
    ) -> Result<f64, data::WalletError> {
        self.inner.balance(guild_id, discord_id).await
    }

    async fn users_map(
        &self,
        guild_id: Option<&str>,
        discord_ids: &[String],
    ) -> Result<HashMap<String, Value>, data::WalletError> {
        self.inner.users_map(guild_id, discord_ids).await
    }

    async fn adjust(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        delta: f64,
        reason: &str,
    ) -> Result<f64, data::WalletError> {
        self.inner.adjust(guild_id, discord_id, delta, reason).await
    }

    async fn debit_exact(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        amount: f64,
        reason: &str,
    ) -> Result<f64, data::WalletError> {
        let count = self
            .debit_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if count == self.fail_on {
            return Err(data::WalletError::Store(
                "Simulated debit failure".to_string(),
            ));
        }
        self.inner
            .debit_exact(guild_id, discord_id, amount, reason)
            .await
    }
}

// Wrapper para simular fallo en set_doc de DocSink
struct FailingSetDocSink {
    inner: Arc<FakeDocSink>,
    fail_collection: String,
}

#[async_trait::async_trait]
impl DocSink for FailingSetDocSink {
    async fn set_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), data::DataError> {
        if collection == self.fail_collection {
            return Err(data::DataError::Firestore(
                "Simulated doc write failure".into(),
            ));
        }
        self.inner.set_doc(collection, id, val).await
    }

    async fn merge_doc(
        &self,
        collection: &str,
        id: &str,
        val: &serde_json::Value,
    ) -> Result<(), data::DataError> {
        self.inner.merge_doc(collection, id, val).await
    }

    async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), data::DataError> {
        self.inner.delete_doc(collection, id).await
    }

    async fn delete_fields(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<(), data::DataError> {
        self.inner.delete_fields(collection, id, fields).await
    }

    async fn take_doc(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, data::DataError> {
        self.inner.take_doc(collection, id).await
    }
}

// (1) Camino feliz admin, apuesta 10, 10 jugadores con saldo 100 -> 200; saldos 90; doc activo con bet_amount_per_player 10, total_pot 100, manual_paid_players [];
#[tokio::test]
async fn test_1_camino_feliz_admin_apuesta_10() {
    let (ctx, docs, wallet) = setup_app(Some(SECRET), None, None);
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let match_id = body["matchId"].as_str().unwrap();

    // Todos los 10 jugadores con saldo 90
    for id in ALL_PLAYERS {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 90.0, "El jugador {id} debe tener saldo 90");
    }

    // Documento activo en faceit_active_matches
    let doc = docs
        .get("faceit_active_matches", match_id)
        .expect("El documento debe existir en Firestore");
    assert_eq!(doc["bet_amount_per_player"], 10.0);
    assert_eq!(doc["total_pot"], 100.0);
    assert_eq!(doc["manual_paid_players"], json!([]));
}

// (2) Ciclo completo: tras lanzar, match-finish con la cabecera x-cs2-secret y winner team1 -> los 5 ganadores 90+20=110, los otros 90, historial finished.
#[tokio::test]
async fn test_2_ciclo_completo_con_match_finish() {
    let (ctx, docs, wallet) = setup_app(Some(SECRET), None, None);
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let match_id = body["matchId"].as_str().unwrap();

    let (finish_status, finish_body) = post_cs2_json(
        &ctx.app,
        "/api/cs2/match-finish",
        Some(SECRET),
        json!({
            "matchId": match_id,
            "winner": "team1",
            "team1_score": 13,
            "team2_score": 5,
            "stats": {"map": "Dust II"}
        }),
    )
    .await;

    assert_eq!(finish_status, StatusCode::OK);
    assert_eq!(finish_body["success"], true);

    const WINNERS: [&str; 5] = ["cap_a", "a1", "a2", "a3", "a4"];
    for id in WINNERS {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 110.0, "Ganador {id} debe tener saldo 110");
    }

    const LOSERS: [&str; 5] = ["cap_b", "b1", "b2", "b3", "b4"];
    for id in LOSERS {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 90.0, "Perdedor {id} debe tener saldo 90");
    }

    assert!(docs.get("faceit_active_matches", match_id).is_none());
    let hist = docs
        .get("faceit_matches_history", match_id)
        .expect("Debe existir en historial");
    assert_eq!(hist["status"], "finished");
    assert_eq!(hist["winner"], "faction1");
}

// (3) Un jugador con saldo 5 -> 400 con unpaidPlayers y NADIE cobrado (todos 100).
#[tokio::test]
async fn test_3_un_jugador_con_saldo_insuficiente_400_nadie_cobrado() {
    let (ctx, _docs, wallet) = setup_app(Some(SECRET), None, None);
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }
    wallet.set_balance(Some("default"), "a2", 5.0);

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains(
        "Faltan 1 jugador(es) por completar su cuota: FC_a2 (Saldo: S/ 5.00, Falta: S/ 5.00)"
    ));
    let unpaid = body["unpaidPlayers"].as_array().unwrap();
    assert_eq!(unpaid.len(), 1);
    assert_eq!(unpaid[0]["discordId"], "a2");
    assert_eq!(unpaid[0]["nickname"], "FC_a2");
    assert_eq!(unpaid[0]["balance"], 5.0);
    assert_eq!(unpaid[0]["deficit"], 5.0);

    // Nadie cobrado: a2 tiene 5, los demás 100
    for id in ALL_PLAYERS {
        let expected = if id == "a2" { 5.0 } else { 100.0 };
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, expected, "Jugador {id} debe mantener saldo intacto");
    }

    let active = ctx.cs2.get_all_active_matches().await;
    assert_eq!(active.len(), 0);
}

// (4) Admin marca a un jugador como pagado manual -> a ese no se le cobra (queda 100), los otros 9 pagan; en match-cancel solo se reembolsa a los 9.
#[tokio::test]
async fn test_4_admin_marca_manual_paid_no_cobra_y_cancel_solo_reembolsa_a_los_demas() {
    let (ctx, docs, wallet) = setup_app(Some(SECRET), None, None);
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0,
            "manualPaidDiscordIds": ["cap_a"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let match_id = body["matchId"].as_str().unwrap();

    assert_eq!(
        wallet.balance(Some("default"), "cap_a").await.unwrap(),
        100.0
    );
    for id in ALL_PLAYERS.iter().filter(|&&id| id != "cap_a") {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 90.0, "Jugador {id} debe haber pagado 10");
    }

    let doc = docs
        .get("faceit_active_matches", match_id)
        .expect("Doc activo debe existir");
    assert_eq!(doc["manual_paid_players"], json!(["cap_a"]));

    let (cancel_status, cancel_body) = post_cs2_json(
        &ctx.app,
        "/api/cs2/match-cancel",
        Some(SECRET),
        json!({
            "matchId": match_id,
            "reason": "Cancelled by admin"
        }),
    )
    .await;

    assert_eq!(cancel_status, StatusCode::OK);
    assert_eq!(cancel_body["success"], true);

    // cap_a sigue en 100 (no se le reembolsa)
    assert_eq!(
        wallet.balance(Some("default"), "cap_a").await.unwrap(),
        100.0
    );
    // Los otros 9 vuelven a 100
    for id in ALL_PLAYERS.iter().filter(|&&id| id != "cap_a") {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 100.0, "Jugador {id} debe tener 100 tras reembolso");
    }
}

// (5) Rollback: envuelve el FakeWalletStore en un wrapper de prueba que falla en el 4.º debit_exact -> respuesta 500 y los 3 cobrados vuelven a su saldo original.
#[tokio::test]
async fn test_5_rollback_falla_en_4to_debit_exact_reembolsa_los_3_cobrados_500() {
    let fake_docs = Arc::new(FakeDocSink::new());
    let fake_wallet = Arc::new(FakeWalletStore::new());
    let failing_wallet = Arc::new(FailingDebitWallet {
        inner: fake_wallet.clone(),
        debit_count: std::sync::atomic::AtomicUsize::new(0),
        fail_on: 4,
    });

    let (ctx, _docs, _w) = setup_app(Some(SECRET), Some(failing_wallet), Some(fake_docs));
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        fake_wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "Error al cobrar la cuota de apuesta.");

    // Los 3 cobrados vuelven a su saldo original (100) y los demás intactos
    for id in ALL_PLAYERS {
        let bal = fake_wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 100.0, "Jugador {id} debe haber vuelto a su saldo 100");
    }

    let active = ctx.cs2.get_all_active_matches().await;
    assert_eq!(active.len(), 0);
}

// (6) Fallo de docs (wrapper DocSink que falla set_doc) con bet>0 -> 500 "No se pudo registrar la partida. Se reembolsó la cuota." y todos los saldos intactos, y NO hay partida publicada en cs2.get_all_active_matches();
#[tokio::test]
async fn test_6_fallo_de_docs_set_doc_con_bet_mayor_a_cero_500_reembolsa_sin_publicar() {
    let fake_docs = Arc::new(FakeDocSink::new());
    let fake_wallet = Arc::new(FakeWalletStore::new());
    let failing_docs = Arc::new(FailingSetDocSink {
        inner: fake_docs.clone(),
        fail_collection: "faceit_active_matches".to_string(),
    });

    let (ctx, _docs, _w) = setup_app(Some(SECRET), Some(fake_wallet.clone()), Some(failing_docs));
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        fake_wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        body["error"],
        "No se pudo registrar la partida. Se reembolsó la cuota."
    );

    // Todos los saldos intactos (100)
    for id in ALL_PLAYERS {
        let bal = fake_wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(
            bal, 100.0,
            "Jugador {id} debe tener 100 tras reembolso por fallo de guardado"
        );
    }

    // NO hay partida publicada en cs2.get_all_active_matches()
    let active = ctx.cs2.get_all_active_matches().await;
    assert_eq!(active.len(), 0);
}

// (7) Sin bets_enabled (false) y bet>0 -> rechazo como antes y saldos intactos.
#[tokio::test]
async fn test_7_sin_bets_enabled_rechazo_y_saldos_intactos() {
    // Sin CS2_API_SECRET -> bets_enabled es false
    let (ctx, _docs, wallet) = setup_app(None, None, None);
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 10.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"],
        "Las apuestas con saldo aún no están disponibles en esta versión. Un administrador debe marcar a todos los jugadores como pagados manualmente o lanzar sin apuesta."
    );

    for id in ALL_PLAYERS {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 100.0, "Jugador {id} debe mantener saldo intacto");
    }

    let active = ctx.cs2.get_all_active_matches().await;
    assert_eq!(active.len(), 0);
}

// (8) Capitán (no admin) con apuesta acordada 10 en el estado -> se cobra esa apuesta (ignora cualquier betAmount del body).
#[tokio::test]
async fn test_8_capitan_con_apuesta_acordada_cobra_acordada_e_ignora_body() {
    let (ctx, docs, wallet) = setup_app(Some(SECRET), None, None);
    let mut state = setup_ten_players_state(true);
    state.bet_agreed = true;
    state.agreed_bet_amount = Some(10.0);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }

    // cap_a no es admin
    let cap_token = make_token("cap_a", "user", false, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &cap_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 50.0 // Se debe ignorar porque el capitán no es admin
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let match_id = body["matchId"].as_str().unwrap();

    // Se cobraron 10 (la acordada), no 50
    for id in ALL_PLAYERS {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(
            bal, 90.0,
            "Jugador {id} debe haber pagado la apuesta acordada de 10"
        );
    }

    let doc = docs
        .get("faceit_active_matches", match_id)
        .expect("Doc debe existir");
    assert_eq!(doc["bet_amount_per_player"], 10.0);
    assert_eq!(doc["total_pot"], 100.0);
}

// (9) Apuesta 0 -> no se cobra nada y funciona como antes.
#[tokio::test]
async fn test_9_apuesta_cero_no_cobra_nada_y_funciona_como_antes() {
    let (ctx, docs, wallet) = setup_app(Some(SECRET), None, None);
    let state = setup_ten_players_state(true);
    ctx.manager.update_state("default", state).await;

    for id in ALL_PLAYERS {
        wallet.set_balance(Some("default"), id, 100.0);
    }

    let admin_token = make_token("admin_1", "admin", true, Some("default"));
    let (status, body) = post_lobby_launch(
        &ctx.app,
        &admin_token,
        json!({
            "teamIdA": "team_1",
            "teamIdB": "team_2",
            "matchMode": "steam",
            "betAmount": 0.0
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let match_id = body["matchId"].as_str().unwrap();

    for id in ALL_PLAYERS {
        let bal = wallet.balance(Some("default"), id).await.unwrap();
        assert_eq!(bal, 100.0, "Jugador {id} no debe haber pagado nada");
    }

    let doc = docs
        .get("faceit_active_matches", match_id)
        .expect("Doc debe existir");
    assert_eq!(doc["bet_amount_per_player"], 0.0);
    assert_eq!(doc["total_pot"], 0.0);
    assert_eq!(doc["entity_custom_name"], "Mixton / Web");

    let active = ctx.cs2.get_all_active_matches().await;
    assert_eq!(active.len(), 1);
}
