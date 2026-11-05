//! Pruebas de integración para el lanzamiento de partidas en modo STEAM (POST /api/lobby/mixton-create-match).
//!
//! Tarea B12:
//! - 403 usuario ajeno (no admin ni capitán de ninguno de los 2 equipos)
//! - 400 equipos incompletos (menos de 5 jugadores por equipo)
//! - 400 falta SteamID (mensaje exacto con lista de nombres)
//! - 400 apuesta sin pago manual
//! - 400 modo faceit
//! - 503 sin cs2 configurado
//! - Camino feliz con 10 jugadores con SteamID válido: 200, lobby reiniciado,
//!   1 partida activa en Cs2ServerManager y documento guardado en Firestore `faceit_active_matches`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::config::Config;
use cs2::Cs2ServerManager;
use data::{DocSink, FakeDocSink};
use lobby::{LobbyApiState, LobbyManager, LobbyState, Player};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-launch-integration-test-secret-32-chars!";

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

fn make_cs2_manager() -> Arc<Cs2ServerManager> {
    let mut vars = std::collections::HashMap::new();
    vars.insert(
        "ADMIN_JWT_SECRET".into(),
        "lobby-launch-test-secret-at-least-32-chars!".into(),
    );
    vars.insert("BASE_URL".into(), "https://machupeek.com".into());
    vars.insert("DISCORD_CLIENT_ID".into(), "id".into());
    vars.insert("DISCORD_CLIENT_SECRET".into(), "secret".into());
    vars.insert("DISCORD_TOKEN".into(), "token".into());
    vars.insert("DISCORD_GUILD_ID".into(), "default".into());

    let cfg = Config::from_map(&vars).unwrap();
    Arc::new(Cs2ServerManager::new(&cfg))
}

fn test_router(
    manager: Arc<LobbyManager>,
    cs2: Option<Arc<Cs2ServerManager>>,
    docs: Option<Arc<dyn DocSink>>,
) -> Router {
    test_router_with_notifier(manager, cs2, docs, Default::default())
}

fn test_router_with_notifier(
    manager: Arc<LobbyManager>,
    cs2: Option<Arc<Cs2ServerManager>>,
    docs: Option<Arc<dyn DocSink>>,
    notifier: lobby::notify::NotifierSlot,
) -> Router {
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let tmp_uploads = tempfile::tempdir().expect("tempdir");
    let uploads_dir = tmp_uploads.path().to_path_buf();
    std::mem::forget(tmp_uploads);
    lobby::router(LobbyApiState {
        manager,
        auth: Arc::new(session),
        uploads_dir,
        cs2,
        docs,
        notifier,
        wallet: None,
    })
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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
        discord_user: Some(lobby::state::DiscordUserDetails {
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

#[tokio::test]
async fn test_launch_403_usuario_ajeno() {
    let manager = Arc::new(LobbyManager::new("default"));
    let state = setup_ten_players_state(true);
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let app = test_router(manager, Some(cs2), None);

    let random_token = make_token("stranger_123", "user", false, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={random_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Solo los capitanes del encuentro o un administrador pueden lanzar la partida."
    );
}

#[tokio::test]
async fn test_launch_400_equipos_incompletos() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_ten_players_state(true);
    state.teams[0].players.pop(); // Solo 3 jugadores
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let app = test_router(manager, Some(cs2), None);

    let admin_token = make_token("admin_123", "admin", true, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Ambos equipos deben estar completos (5 jugadores c/u)."
    );
}

#[tokio::test]
async fn test_launch_400_falta_steam_id() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_ten_players_state(true);
    state.teams[0].players[0].steam_id_64 = None; // FC_a1
    state.teams[1].players[1].steam_id_64 = Some("invalid_short".into()); // FC_b2
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let app = test_router(manager, Some(cs2), None);

    let admin_token = make_token("admin_123", "admin", true, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "No se puede lanzar la partida. Todos los jugadores deben tener su SteamID vinculado para ingresar al servidor CS2. Falta vincular el Steam ID de: FC_a1, FC_b2."
    );
}

#[tokio::test]
async fn test_launch_400_apuesta_sin_pago_manual() {
    let manager = Arc::new(LobbyManager::new("default"));
    let state = setup_ten_players_state(true);
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let app = test_router(manager, Some(cs2), None);

    let admin_token = make_token("admin_123", "admin", true, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam",
                "betAmount": 10.0,
                "manualPaidDiscordIds": ["cap_a", "a1"] // Faltan 8 jugadores
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Las apuestas con saldo aún no están disponibles en esta versión. Un administrador debe marcar a todos los jugadores como pagados manualmente o lanzar sin apuesta."
    );
}

#[tokio::test]
async fn test_launch_400_modo_faceit() {
    let manager = Arc::new(LobbyManager::new("default"));
    let state = setup_ten_players_state(true);
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let app = test_router(manager, Some(cs2), None);

    let admin_token = make_token("admin_123", "admin", true, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "faceit"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "El modo FACEIT aún no está disponible en esta versión. Usa el modo STEAM."
    );
}

#[tokio::test]
async fn test_launch_503_sin_cs2() {
    let manager = Arc::new(LobbyManager::new("default"));
    let state = setup_ten_players_state(true);
    manager.update_state("default", state).await;

    // cs2 es None
    let app = test_router(manager, None, None);

    let admin_token = make_token("admin_123", "admin", true, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "El gestor de servidores CS2 no está configurado."
    );
}

#[tokio::test]
async fn test_launch_camino_feliz_completo() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_ten_players_state(true);

    // Añadimos a los 10 jugadores en available_players más 2 adicionales
    for team in &state.teams {
        if let Some(ref cap) = team.captain {
            state.available_players.push(cap.clone());
        }
        state.available_players.extend(team.players.iter().cloned());
    }
    state
        .available_players
        .push(make_player("extra_player_1", None));
    state
        .available_players
        .push(make_player("extra_player_2", None));
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let fake_docs = Arc::new(FakeDocSink::new());
    let app = test_router(manager.clone(), Some(cs2.clone()), Some(fake_docs.clone()));

    // El capitán de Team A lanza la partida
    let cap_token = make_token("cap_a", "user", false, Some("default"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={cap_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam",
                "chosenMap": { "id": "de_dust2", "name": "Dust II" }
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;

    assert_eq!(body["success"], true);
    let match_id = body["matchId"].as_str().expect("matchId debe ser string");
    assert!(match_id.starts_with("mixton_"));
    assert_eq!(body["faceitHubUrl"], Value::Null);
    assert_eq!(body["assignedQueue"]["id"], "steam");
    assert_eq!(body["assignedQueue"]["queueId"], "steam");
    assert_eq!(body["assignedQueue"]["name"], "Servidor CS2 Dedicado");
    assert_eq!(body["gamesPerMatch"], 1);
    assert_eq!(body["roundsPerGame"], 1);
    assert!(body["serverIp"].as_str().unwrap().contains(":27015"));

    // 1. El estado del lobby queda reiniciado
    let final_state = manager.get_state("default").await;
    assert!(final_state.teams[0].captain.is_none());
    assert!(final_state.teams[0].players.is_empty());
    assert!(final_state.teams[1].captain.is_none());
    assert!(final_state.teams[1].players.is_empty());
    assert!(!final_state.draft_started);
    assert!(!final_state.draft_finished);
    assert!(final_state.pick_order.is_empty());
    assert_eq!(final_state.current_pick_index, 0);

    // Los 10 jugadores lanzados salieron de available_players; solo quedan los 2 extras
    assert_eq!(final_state.available_players.len(), 2);
    assert_eq!(
        final_state.available_players[0].discord_id,
        "extra_player_1"
    );
    assert_eq!(
        final_state.available_players[1].discord_id,
        "extra_player_2"
    );

    // 2. Cs2ServerManager tiene 1 partida activa registrada
    let active_matches = cs2.get_all_active_matches().await;
    assert_eq!(active_matches.len(), 1);
    assert_eq!(active_matches[0].match_id, match_id);
    assert_eq!(active_matches[0].map, "Dust II");
    assert_eq!(active_matches[0].assigned_port, 27015);

    // 3. FakeDocSink recibió el documento en faceit_active_matches
    let doc = fake_docs
        .get("faceit_active_matches", match_id)
        .expect("El documento debe existir en Firestore");
    assert_eq!(doc["id"], match_id);
    assert_eq!(doc["entity_custom_name"], "Mixton / Web");
    assert_eq!(doc["game"], "cs2");
    assert_eq!(doc["map"], "Dust II");
    assert_eq!(doc["location"], "Mixton");
    assert_eq!(doc["faction1_name"], "Equipo FC_cap_a");
    assert_eq!(doc["faction2_name"], "Equipo FC_cap_b");
    assert_eq!(doc["queue_id"], "steam");
    assert_eq!(doc["queue_name"], "Servidor CS2 Dedicado");
    assert_eq!(doc["creator_id"], "cap_a");
    assert_eq!(doc["guild_id"], "default");
    assert!(doc["startedAt"].is_i64());
    assert!(doc["createdAt"].get("$timestamp").is_some());

    let f1_roster = doc["faction1_roster"].as_array().unwrap();
    assert_eq!(f1_roster.len(), 5);
    assert_eq!(f1_roster[0]["id"], "faceit_cap_a");
    assert_eq!(f1_roster[0]["discord_id"], "cap_a");
    assert_eq!(f1_roster[0]["steam_id_64"], "76561198000000001");
}

#[tokio::test]
async fn test_launch_409_carrera_doble_lanzamiento() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_ten_players_state(true);
    for team in &state.teams {
        if let Some(ref cap) = team.captain {
            state.available_players.push(cap.clone());
        }
        state.available_players.extend(team.players.iter().cloned());
    }
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let fake_docs = Arc::new(FakeDocSink::new());
    let app = test_router(manager.clone(), Some(cs2.clone()), Some(fake_docs.clone()));

    let cap_token = make_token("cap_a", "user", false, Some("default"));

    // 1. Mantenemos un guard obtenido con manager.try_begin_launch("default")
    let guard = manager.try_begin_launch("default");
    assert!(guard.is_some());

    let req_builder = || {
        Request::builder()
            .method("POST")
            .uri("/api/lobby/mixton-create-match")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, format!("auth_token={cap_token}"))
            .body(Body::from(
                json!({
                    "teamIdA": "team_1",
                    "teamIdB": "team_2",
                    "matchMode": "steam",
                    "chosenMap": { "id": "de_dust2", "name": "Dust II" }
                })
                .to_string(),
            ))
            .unwrap()
    };

    // La petición devuelve 409 con el mensaje exacto
    let res = app.clone().oneshot(req_builder()).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Ya se está lanzando una partida en este servidor."
    );

    // 2. Tras soltar el guard vuelve a funcionar (200 en el camino feliz)
    drop(guard);

    let res2 = app.oneshot(req_builder()).await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let body2 = body_json(res2).await;
    assert_eq!(body2["success"], true);
}

#[tokio::test]
async fn test_launch_notifica_en_slot_con_fake_notifier() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_ten_players_state(true);
    for team in &state.teams {
        if let Some(ref cap) = team.captain {
            state.available_players.push(cap.clone());
        }
        state.available_players.extend(team.players.iter().cloned());
    }
    manager.update_state("default", state).await;

    let cs2 = make_cs2_manager();
    let fake_docs = Arc::new(FakeDocSink::new());

    let slot: lobby::notify::NotifierSlot = Default::default();
    let fake_notifier = Arc::new(lobby::notify::FakeNotifier::new());
    assert!(slot.set(fake_notifier.clone()).is_ok());

    let app = test_router_with_notifier(
        manager.clone(),
        Some(cs2.clone()),
        Some(fake_docs.clone()),
        slot,
    );

    let cap_token = make_token("cap_a", "user", false, Some("default"));

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-create-match")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={cap_token}"))
        .body(Body::from(
            json!({
                "teamIdA": "team_1",
                "teamIdB": "team_2",
                "matchMode": "steam",
                "chosenMap": { "id": "de_dust2", "name": "Dust II" }
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);
    let match_id = body["matchId"].as_str().unwrap().to_string();
    let server_ip = body["serverIp"].as_str().unwrap().to_string();

    // Espera hasta 1 segundo porque la notificación se despacha en tokio::spawn
    let mut received = Vec::new();
    for _ in 0..50 {
        received = fake_notifier.matches();
        if !received.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert_eq!(
        received.len(),
        1,
        "FakeNotifier debe recibir exactamente 1 MatchLaunchInfo"
    );
    let info = &received[0];
    assert_eq!(info.match_id, match_id);
    assert_eq!(info.server_ip, server_ip);
    assert_eq!(info.map_name, "Dust II");
    assert_eq!(info.team_a.name, "Equipo FC_cap_a");
    assert_eq!(info.team_b.name, "Equipo FC_cap_b");
    assert_eq!(info.team_a.captain_discord_id, "cap_a");
    assert_eq!(info.team_b.captain_discord_id, "cap_b");
    assert_eq!(info.team_a.players.len(), 5);
    assert_eq!(info.team_b.players.len(), 5);

    // Los 10 jugadores deben estar presentes con sus discord_id
    let all_players: Vec<String> = info
        .team_a
        .players
        .iter()
        .chain(info.team_b.players.iter())
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(all_players.len(), 10);
    assert!(all_players.contains(&"cap_a".to_string()));
    assert!(all_players.contains(&"cap_b".to_string()));
    for i in 1..=4 {
        assert!(all_players.contains(&format!("a{i}")));
        assert!(all_players.contains(&format!("b{i}")));
    }
}
