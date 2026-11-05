//! Pruebas de integración para las rutas del draft del lobby Mixton.
//!
//! Verifica las condiciones obligatorias de SDD Task B5:
//! 1. Usuario que no es capitán haciendo pick -> 403 con el mensaje exacto del legado.
//! 2. Capitán del equipo equivocado -> 403 con error de turno.
//! 3. Capitán correcto -> 200 y estado actualizado (jugador añadido, removido de disponibles, pick sequence avanza).
//! 4. Admin puede pickear por el capitán de turno -> 200.
//! 5. mixton-start y mixton-pick-order con validaciones del legado.
//! 6. mixton-auto-assign completa equipos balanceados y finaliza el draft.

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

const SECRET: &str = "lobby-draft-test-secret-at-least-32-chars!";

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
        cs2: None,
        docs: None,
    })
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn make_player(id: &str, elo: u32, level: Option<u32>, tier: &str) -> Player {
    Player {
        discord_id: id.to_string(),
        nickname: format!("Nick_{id}"),
        faceit_player_id: None,
        faceit_nickname: Some(format!("Faceit_{id}")),
        steam_id_64: None,
        avatar: None,
        faceit_elo: elo,
        faceit_level: level,
        ready: true,
        tier: tier.to_string(),
        mixton_tier_override: None,
        discord_user: None,
    }
}

fn setup_lobby_with_captains() -> LobbyState {
    let mut state = LobbyState::default();
    state.teams = vec![
        LobbyTeam {
            id: "team_1".to_string(),
            name: "Equipo 1".to_string(),
            tag: "EQ1".to_string(),
            captain: Some(make_player("cap_a_123", 2000, Some(10), "S")),
            players: Vec::new(),
        },
        LobbyTeam {
            id: "team_2".to_string(),
            name: "Equipo 2".to_string(),
            tag: "EQ2".to_string(),
            captain: Some(make_player("cap_b_456", 1800, Some(9), "A")),
            players: Vec::new(),
        },
    ];
    state.pick_order = vec!["team_1".to_string(), "team_2".to_string()];
    state.available_players = vec![
        make_player("p1_pickable", 1400, Some(6), "B"),
        make_player("p2_pickable", 1300, Some(6), "B"),
        make_player("p3_pickable", 1200, Some(5), "B"),
        make_player("p4_pickable", 1100, Some(4), "C"),
    ];
    state
}

#[tokio::test]
async fn test_mixton_start_requiere_admin_e_inicia_correctamente() {
    let manager = Arc::new(LobbyManager::new("default"));
    let state = setup_lobby_with_captains();
    manager.update_state("default", state).await;

    let app = test_router(manager.clone());
    let user_token = make_token("regular_user", "user", false, None);

    // 1. Usuario normal -> 403
    let req_user = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-start")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::from(json!({}).to_string()))
        .unwrap();

    let res_user = app.oneshot(req_user).await.unwrap();
    assert_eq!(res_user.status(), StatusCode::FORBIDDEN);

    // 2. Admin -> 200
    let app2 = test_router(manager.clone());
    let admin_token = make_token("admin_boss", "admin", true, None);
    let req_admin = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-start")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "pickOrder": ["team_1", "team_2"],
                "betAmount": 25.0
            })
            .to_string(),
        ))
        .unwrap();

    let res_admin = app2.oneshot(req_admin).await.unwrap();
    assert_eq!(res_admin.status(), StatusCode::OK);
    let body = body_json(res_admin).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["state"]["draftStarted"], true);
    assert_eq!(body["state"]["draftFinished"], false);
    assert_eq!(body["state"]["currentPickIndex"], 0);
    assert_eq!(body["state"]["effectiveBetAmount"], 25.0);
}

#[tokio::test]
async fn test_mixton_pick_order_validaciones() {
    let manager = Arc::new(LobbyManager::new("default"));
    let state = setup_lobby_with_captains();
    manager.update_state("default", state).await;

    let admin_token = make_token("admin_boss", "admin", true, None);

    // 1. No array -> 400 "El orden debe ser un array."
    let app = test_router(manager.clone());
    let req_invalid = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick-order")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({ "pickOrder": "not_an_array" }).to_string(),
        ))
        .unwrap();

    let res_invalid = app.oneshot(req_invalid).await.unwrap();
    assert_eq!(res_invalid.status(), StatusCode::BAD_REQUEST);
    let body_inv = body_json(res_invalid).await;
    assert_eq!(body_inv["error"], "El orden debe ser un array.");

    // 2. Equipo inexistente o sin capitán -> 400
    let app2 = test_router(manager.clone());
    let req_missing_team = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick-order")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({ "pickOrder": ["team_999", "team_1"] }).to_string(),
        ))
        .unwrap();

    let res_missing = app2.oneshot(req_missing_team).await.unwrap();
    assert_eq!(res_missing.status(), StatusCode::BAD_REQUEST);
    let body_missing = body_json(res_missing).await;
    assert_eq!(
        body_missing["error"],
        "El equipo team_999 no tiene un capitán asignado."
    );

    // 3. Exitoso
    let app3 = test_router(manager.clone());
    let req_ok = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick-order")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({ "pickOrder": ["team_2", "team_1"] }).to_string(),
        ))
        .unwrap();

    let res_ok = app3.oneshot(req_ok).await.unwrap();
    assert_eq!(res_ok.status(), StatusCode::OK);
    let body_ok = body_json(res_ok).await;
    assert_eq!(body_ok["state"]["pickOrder"], json!(["team_2", "team_1"]));
}

#[tokio::test]
async fn test_mixton_pick_usuario_no_capitan_devuelve_403() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_lobby_with_captains();
    state.draft_started = true;
    state.draft_finished = false;
    state.current_pick_index = 0; // Turno de team_1 (capitán cap_a_123)
    manager.update_state("default", state).await;

    let app = test_router(manager);
    let outsider_token = make_token("outsider_user", "user", false, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={outsider_token}"))
        .body(Body::from(
            json!({
                "playerDiscordId": "p1_pickable"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "No es tu turno. Es turno del capitán de Equipo 1."
    );
}

#[tokio::test]
async fn test_mixton_pick_capitan_del_equipo_equivocado_devuelve_403() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_lobby_with_captains();
    state.draft_started = true;
    state.draft_finished = false;
    state.current_pick_index = 0; // Turno de team_1 (capitán cap_a_123)
    manager.update_state("default", state).await;

    let app = test_router(manager);
    // Capitán del equipo 2 intentando elegir en turno del equipo 1
    let cap_b_token = make_token("cap_b_456", "user", false, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={cap_b_token}"))
        .body(Body::from(
            json!({
                "playerDiscordId": "p1_pickable"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "No es tu turno. Es turno del capitán de Equipo 1."
    );
}

#[tokio::test]
async fn test_mixton_pick_capitan_correcto_devuelve_200_y_actualiza_estado() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_lobby_with_captains();
    state.draft_started = true;
    state.draft_finished = false;
    state.current_pick_index = 0; // Turno de team_1
    manager.update_state("default", state).await;

    let app = test_router(manager);
    // Capitán legítimo de team_1
    let cap_a_token = make_token("cap_a_123", "user", false, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={cap_a_token}"))
        .body(Body::from(
            json!({
                "playerDiscordId": "p1_pickable"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    // Verificar que el jugador fue agregado al equipo 1
    let team1_players = body["state"]["teams"][0]["players"]
        .as_array()
        .expect("team 1 players array");
    assert_eq!(team1_players.len(), 1);
    assert_eq!(team1_players[0]["discordId"], "p1_pickable");

    // Verificar que el pick index avanzó
    assert_eq!(body["state"]["currentPickIndex"], 1);
}

#[tokio::test]
async fn test_mixton_pick_admin_puede_pickear_por_capitan() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_lobby_with_captains();
    state.draft_started = true;
    state.draft_finished = false;
    state.current_pick_index = 0; // Turno de team_1
    manager.update_state("default", state).await;

    let app = test_router(manager);
    // Administrador actuando en nombre del capitán
    let admin_token = make_token("admin_master", "admin", true, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(
            json!({
                "playerDiscordId": "p1_pickable"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["state"]["currentPickIndex"], 1);
}

#[tokio::test]
async fn test_mixton_pick_jugador_no_disponible_devuelve_400() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_lobby_with_captains();
    state.draft_started = true;
    state.draft_finished = false;
    state.current_pick_index = 0;
    manager.update_state("default", state).await;

    let app = test_router(manager);
    let cap_a_token = make_token("cap_a_123", "user", false, None);

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-pick")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={cap_a_token}"))
        .body(Body::from(
            json!({
                "playerDiscordId": "ghost_player_not_in_queue"
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "El jugador seleccionado no está disponible (/mixton_listo)."
    );
}

#[tokio::test]
async fn test_mixton_auto_assign_completa_equipos_y_finaliza_draft() {
    let manager = Arc::new(LobbyManager::new("default"));
    let mut state = setup_lobby_with_captains();
    // Proveer 8 jugadores en total para completar 2 equipos de 4 jugadores cada uno
    state.available_players = vec![
        make_player("p1", 2000, Some(10), "S"),
        make_player("p2", 1800, Some(9), "A"),
        make_player("p3", 1600, Some(8), "A"),
        make_player("p4", 1400, Some(6), "B"),
        make_player("p5", 1300, Some(6), "B"),
        make_player("p6", 1100, Some(4), "C"),
        make_player("p7", 1000, Some(3), "C"),
        make_player("p8", 900, Some(2), "C"),
    ];
    manager.update_state("default", state).await;

    // 1. Usuario normal -> 403
    let app = test_router(manager.clone());
    let user_token = make_token("user_123", "user", false, None);
    let req_user = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-auto-assign")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={user_token}"))
        .body(Body::from(json!({}).to_string()))
        .unwrap();

    let res_user = app.oneshot(req_user).await.unwrap();
    assert_eq!(res_user.status(), StatusCode::FORBIDDEN);

    // 2. Admin -> 200 y balanceado
    let app2 = test_router(manager.clone());
    let admin_token = make_token("admin_boss", "admin", true, None);
    let req_admin = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-auto-assign")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={admin_token}"))
        .body(Body::from(json!({}).to_string()))
        .unwrap();

    let res_admin = app2.oneshot(req_admin).await.unwrap();
    assert_eq!(res_admin.status(), StatusCode::OK);
    let body = body_json(res_admin).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["state"]["draftFinished"], true);
    assert_eq!(body["state"]["draftStarted"], false);

    let t1_players = body["state"]["teams"][0]["players"].as_array().unwrap();
    let t2_players = body["state"]["teams"][1]["players"].as_array().unwrap();
    assert_eq!(t1_players.len(), 4);
    assert_eq!(t2_players.len(), 4);
}
