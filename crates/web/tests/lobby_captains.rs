//! Pruebas de integración para las rutas de capitanes y ruleta del lobby Mixton.
//!
//! Verifica las condiciones obligatorias de SDD Task B2:
//! 1. Admin asigna capitán no elegible (nivel 3) -> 400 "Los capitanes solo pueden ser de Tier S o Tier A."
//! 2. captain-proposal de un usuario ajeno a los equipos -> 403
//! 3. captain-ready sin acuerdo de montos -> 400 "Los capitanes deben marcar el mismo monto"

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use lobby::{LobbyApiState, LobbyManager, LobbyState, LobbyTeam, Player};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-captains-test-secret-at-least-32-chars!";

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

fn test_router(manager: Arc<LobbyManager>) -> Router {
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let tmp_uploads = tempfile::tempdir().expect("tempdir");
    let uploads_dir = tmp_uploads.path().to_path_buf();
    std::mem::forget(tmp_uploads);
    lobby::router(LobbyApiState {
        manager,
        auth: Arc::new(session),
        uploads_dir,
    })
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn setup_two_teams_state() -> LobbyState {
    let mut state = LobbyState::default();
    state.teams = vec![
        LobbyTeam {
            id: "team_1".to_string(),
            name: "Equipo 1".to_string(),
            tag: "EQ1".to_string(),
            captain: None,
            players: Vec::new(),
        },
        LobbyTeam {
            id: "team_2".to_string(),
            name: "Equipo 2".to_string(),
            tag: "EQ2".to_string(),
            captain: None,
            players: Vec::new(),
        },
    ];
    state
}

#[tokio::test]
async fn test_admin_asigna_capitan_no_elegible_devuelve_400() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_two_teams_state();

    // Jugador nivel 3 (Tier C, no elegible para capitán)
    state.available_players.push(Player {
        discord_id: "user_ineligible_3".to_string(),
        nickname: "NoobPlayer".to_string(),
        faceit_level: Some(3),
        faceit_elo: 800,
        ready: true,
        tier: "C".to_string(),
        ..Default::default()
    });
    manager.update_state("default", state).await;

    let app = test_router(manager);
    let admin_token = make_token("admin_master", "admin", true, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-assign-captain")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamId": "team_1",
                "discordId": "user_ineligible_3"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Los capitanes solo pueden ser de Tier S o Tier A."
    );
}

#[tokio::test]
async fn test_captain_proposal_de_usuario_ajeno_devuelve_403() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_two_teams_state();

    state.teams[0].captain = Some(Player {
        discord_id: "cap_a_123".to_string(),
        nickname: "CapitanA".to_string(),
        faceit_level: Some(10),
        tier: "S".to_string(),
        ready: true,
        ..Default::default()
    });
    state.teams[1].captain = Some(Player {
        discord_id: "cap_b_456".to_string(),
        nickname: "CapitanB".to_string(),
        faceit_level: Some(9),
        tier: "A".to_string(),
        ready: true,
        ..Default::default()
    });
    manager.update_state("default", state).await;

    let app = test_router(manager);
    // Usuario común no admin y que no es capitán de ninguno de los equipos
    let stranger_token = make_token("stranger_user_999", "user", false, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-captain-proposal")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={stranger_token}"))
        .body(Body::from(
            json!({
                "teamId": "team_1",
                "betAmount": 20.0
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Solo el capitán de Equipo 1 o un administrador pueden marcar este monto."
    );
}

#[tokio::test]
async fn test_captain_ready_sin_acuerdo_devuelve_400() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_two_teams_state();

    state.teams[0].captain = Some(Player {
        discord_id: "cap_a_123".to_string(),
        nickname: "CapitanA".to_string(),
        faceit_level: Some(10),
        tier: "S".to_string(),
        ready: true,
        ..Default::default()
    });
    state.teams[1].captain = Some(Player {
        discord_id: "cap_b_456".to_string(),
        nickname: "CapitanB".to_string(),
        faceit_level: Some(9),
        tier: "A".to_string(),
        ready: true,
        ..Default::default()
    });
    // Propuestas desiguales (10 vs 25)
    state.captain_a_proposal = Some(json!(10.0));
    state.captain_b_proposal = Some(json!(25.0));
    manager.update_state("default", state).await;

    let app = test_router(manager);
    let cap_a_token = make_token("cap_a_123", "user", false, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-captain-ready")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={cap_a_token}"))
        .body(Body::from(
            json!({
                "teamId": "team_1",
                "isReady": true
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let body = body_json(res).await;
    let err_msg = body["error"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("Los capitanes deben marcar el mismo monto"),
        "Mensaje de error inesperado: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_captain_assign_eligible_tier_s_succeeds() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_two_teams_state();

    state.available_players.push(Player {
        discord_id: "pro_player_10".to_string(),
        nickname: "ProPlayer".to_string(),
        faceit_level: Some(10),
        faceit_elo: 2400,
        ready: true,
        tier: "S".to_string(),
        ..Default::default()
    });
    manager.update_state("default", state).await;

    let app = test_router(manager.clone());
    let admin_token = make_token("admin_master", "admin", true, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-assign-captain")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "teamId": "team_1",
                "discordId": "pro_player_10"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    let current = manager.get_state("default").await;
    assert_eq!(
        current.teams[0]
            .captain
            .as_ref()
            .map(|c| c.discord_id.as_str()),
        Some("pro_player_10")
    );
}

#[tokio::test]
async fn test_force_roulette_starts_immediately() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_two_teams_state();

    state.teams[0].captain = Some(Player {
        discord_id: "cap_a".to_string(),
        faceit_level: Some(10),
        ..Default::default()
    });
    state.teams[1].captain = Some(Player {
        discord_id: "cap_b".to_string(),
        faceit_level: Some(8),
        ..Default::default()
    });
    manager.update_state("default", state).await;

    let app = test_router(manager.clone());
    let admin_token = make_token("admin_master", "admin", true, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-force-roulette")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = body_json(res).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["message"], "Ruleta iniciada de inmediato.");
}
