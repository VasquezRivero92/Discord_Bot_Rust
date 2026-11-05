//! Pruebas de integración para las rutas del veto de mapas del lobby Mixton.
//!
//! Verifica las condiciones obligatorias de SDD Task B6:
//! 1. Capitán ajeno intentando iniciar veto -> 403 con mensaje exacto del legado.
//! 2. Capitán de la partida o admin iniciando veto -> 200 y MapVetoState inicializado.
//! 3. Capitán ajeno intentando banear -> 403 con error del legado ("No es tu turno para banear...").
//! 4. Capitán correcto baneando -> 200, avanza currentStepIndex y actualiza turno.
//! 5. Admin pudiendo actuar por el equipo en turno -> 200.
//! 6. Mapa repetido -> 400 ("Este mapa ya fue baneado.").
//! 7. Mapa fuera del pool -> 404 ("Mapa no válido en el pool.").
//! 8. Veto completo hasta el final -> completed: true y chosenMap fijado.
//! 9. Cancelar veto requiere admin y limpia el estado a None.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use lobby::{default_catalog, LobbyApiState, LobbyManager, LobbyState, LobbyTeam, Player};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-veto-test-secret-at-least-32-chars!";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_token(user_id: &str, role: &str, is_admin: bool, servidor: Option<&str>) -> String {
    let c = new_claims(
        user_id.into(),
        format!("User_{user_id}"),
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
        notifier: Default::default(),
    })
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn make_player(id: &str, nickname: &str) -> Player {
    Player {
        discord_id: id.to_string(),
        nickname: nickname.to_string(),
        faceit_player_id: None,
        faceit_nickname: Some(format!("FC_{nickname}")),
        steam_id_64: Some("76561198000000000".to_string()),
        avatar: None,
        faceit_elo: 1500,
        faceit_level: Some(7),
        ready: true,
        tier: "A".to_string(),
        mixton_tier_override: None,
        discord_user: None,
    }
}

fn make_ready_lobby_state() -> LobbyState {
    let mut state = LobbyState::default();
    state.map_catalog = default_catalog();

    let team_a = LobbyTeam {
        id: "team_1".to_string(),
        name: "Equipo Alfa".to_string(),
        tag: "ALFA".to_string(),
        captain: Some(make_player("cap_1", "CapitanAlfa")),
        players: vec![
            make_player("p1_1", "P1_1"),
            make_player("p1_2", "P1_2"),
            make_player("p1_3", "P1_3"),
            make_player("p1_4", "P1_4"),
        ],
    };

    let team_b = LobbyTeam {
        id: "team_2".to_string(),
        name: "Equipo Beta".to_string(),
        tag: "BETA".to_string(),
        captain: Some(make_player("cap_2", "CapitanBeta")),
        players: vec![
            make_player("p2_1", "P2_1"),
            make_player("p2_2", "P2_2"),
            make_player("p2_3", "P2_3"),
            make_player("p2_4", "P2_4"),
        ],
    };

    state.teams = vec![team_a, team_b];
    state
}

#[tokio::test]
async fn test_mixton_start_veto_usuario_no_capitan_ni_admin_devuelve_403() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    manager
        .update_state("guild_test", make_ready_lobby_state())
        .await;
    let app = test_router(manager);

    let token = make_token("random_user", "user", false, Some("guild_test"));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-start-veto")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "teamIdA": "team_1",
                        "teamIdB": "team_2",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "Solo los capitanes de la partida o un administrador pueden iniciar la fase de veto."
    );
}

#[tokio::test]
async fn test_mixton_start_veto_capitan_o_admin_inicia_correctamente() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    manager
        .update_state("guild_test", make_ready_lobby_state())
        .await;
    let app = test_router(manager);

    // Capitán del Equipo Alfa iniciando veto
    let token = make_token("cap_1", "user", false, Some("guild_test"));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-start-veto")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "teamIdA": "team_1",
                        "teamIdB": "team_2",
                        "gamesPerMatch": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    let veto = &body["mapVeto"];
    assert_eq!(veto["active"], true);
    assert_eq!(veto["completed"], false);
    assert_eq!(veto["vetoMode"], "bo1");
    assert_eq!(veto["actionType"], "ban");
    assert_eq!(veto["currentStepIndex"], 0);
    assert_eq!(veto["currentBanIndex"], 0);
    assert_eq!(veto["turn"], "team_1");
    assert!(veto["turnEndsAt"].as_i64().is_some());
    assert_eq!(veto["steps"].as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn test_mixton_ban_map_capitan_ajeno_devuelve_403() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    let mut state = make_ready_lobby_state();
    let maps = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    state.map_veto = Some(
        lobby::init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000)
            .unwrap(),
    );
    manager.update_state("guild_test", state).await;
    let app = test_router(manager);

    // Es turno de team_1 (capitán cap_1). Capitán cap_2 intenta banear -> 403
    let token = make_token("cap_2", "user", false, Some("guild_test"));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-ban-map")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "mapId": "de_mirage" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(
        body["error"],
        "No es tu turno para banear. Es turno de Equipo Alfa."
    );
}

#[tokio::test]
async fn test_mixton_ban_map_capitan_correcto_devuelve_200_y_avanza_paso() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    let mut state = make_ready_lobby_state();
    let maps = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    state.map_veto = Some(
        lobby::init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000)
            .unwrap(),
    );
    manager.update_state("guild_test", state).await;
    let app = test_router(manager);

    // Capitán cap_1 banea de_mirage
    let token = make_token("cap_1", "user", false, Some("guild_test"));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-ban-map")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "mapId": "de_mirage" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    let veto = &body["mapVeto"];
    assert_eq!(veto["bannedMaps"], json!(["de_mirage"]));
    assert_eq!(veto["currentStepIndex"], 1);
    assert_eq!(veto["currentBanIndex"], 1);
    // Siguiendo la secuencia del legado (1 A, 2 B, 2 A, 1 B), el siguiente turno es team_2
    assert_eq!(veto["turn"], "team_2");
}

#[tokio::test]
async fn test_mixton_ban_map_admin_puede_actuar_por_capitan() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    let mut state = make_ready_lobby_state();
    let maps = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    state.map_veto = Some(
        lobby::init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000)
            .unwrap(),
    );
    manager.update_state("guild_test", state).await;
    let app = test_router(manager);

    // Admin banea de_inferno en lugar del capitán de turno
    let token = make_token("admin_master", "admin", false, Some("guild_test"));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-ban-map")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "mapId": "de_inferno" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["mapVeto"]["bannedMaps"], json!(["de_inferno"]));
}

#[tokio::test]
async fn test_mixton_ban_map_validaciones_repetido_y_pool() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    let mut state = make_ready_lobby_state();
    let maps = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    state.map_veto = Some(
        lobby::init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000)
            .unwrap(),
    );
    // Marcamos de_mirage como ya baneado previamente
    state
        .map_veto
        .as_mut()
        .unwrap()
        .banned_maps
        .push("de_mirage".to_string());
    manager.update_state("guild_test", state).await;

    // 1. Intentar banear mapa ya descartado -> 400
    let app1 = test_router(manager.clone());
    let token = make_token("cap_1", "user", false, Some("guild_test"));
    let res1 = app1
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-ban-map")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "mapId": "de_mirage" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res1.status(), StatusCode::BAD_REQUEST);
    let body1 = body_json(res1).await;
    assert_eq!(body1["error"], "Este mapa ya fue baneado.");

    // 2. Intentar banear mapa inexistente en el pool -> 404
    let app2 = test_router(manager.clone());
    let res2 = app2
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-ban-map")
                .header(header::COOKIE, format!("auth_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "mapId": "de_nonexistent" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::NOT_FOUND);
    let body2 = body_json(res2).await;
    assert_eq!(body2["error"], "Mapa no válido en el pool.");
}

#[tokio::test]
async fn test_mixton_cancel_veto_requiere_admin_y_limpia_estado() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    let mut state = make_ready_lobby_state();
    let maps = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    state.map_veto = Some(
        lobby::init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000)
            .unwrap(),
    );
    manager.update_state("guild_test", state).await;

    // 1. Usuario normal -> 403
    let app1 = test_router(manager.clone());
    let user_token = make_token("user_1", "user", false, Some("guild_test"));
    let res1 = app1
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-cancel-veto")
                .header(header::COOKIE, format!("auth_token={user_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res1.status(), StatusCode::FORBIDDEN);

    // 2. Administrador -> 200 y mapVeto queda None
    let app2 = test_router(manager.clone());
    let admin_token = make_token("admin_1", "admin", false, Some("guild_test"));
    let res2 = app2
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/lobby/mixton-cancel-veto")
                .header(header::COOKIE, format!("auth_token={admin_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let current_state = manager.get_state("guild_test").await;
    assert!(current_state.map_veto.is_none());
}

#[tokio::test]
async fn test_mixton_veto_completo_hasta_el_final() {
    let manager = Arc::new(LobbyManager::new("guild_test"));
    let mut state = make_ready_lobby_state();
    let maps = state
        .map_catalog
        .iter()
        .filter(|m| m.active)
        .cloned()
        .collect();
    state.map_veto = Some(
        lobby::init_map_veto_state("team_1", "team_2", maps, 1, 0.0, "auto", "steam", 1000)
            .unwrap(),
    );
    manager.update_state("guild_test", state).await;

    let cap1_token = make_token("cap_1", "user", false, Some("guild_test"));
    let cap2_token = make_token("cap_2", "user", false, Some("guild_test"));

    // Secuencia para 7 mapas: A(1), B(2), A(2), B(1)
    let turns = vec![
        (&cap1_token, "de_mirage"),
        (&cap2_token, "de_inferno"),
        (&cap2_token, "de_nuke"),
        (&cap1_token, "de_ancient"),
        (&cap1_token, "de_anubis"),
        (&cap2_token, "de_dust2"),
    ];

    let mut last_body = Value::Null;
    for (tok, map_id) in turns {
        let app = test_router(manager.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/lobby/mixton-ban-map")
                    .header(header::COOKIE, format!("auth_token={tok}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "mapId": map_id }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        last_body = body_json(res).await;
    }

    let final_veto = &last_body["mapVeto"];
    assert_eq!(final_veto["completed"], true);
    assert_eq!(final_veto["active"], false);
    assert_eq!(final_veto["chosenMap"]["id"], "de_vertigo");
    assert_eq!(final_veto["final_chosen_map"], "de_vertigo");
    assert!(final_veto["turnEndsAt"].is_null());
    assert!(final_veto["autoLaunchEndsAt"].as_i64().is_some());
}
