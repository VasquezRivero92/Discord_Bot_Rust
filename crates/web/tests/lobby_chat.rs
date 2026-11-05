//! Pruebas completas del chat interactivo de Mixtón (SDD Task B3).
//!
//! Cubre las 8 pruebas obligatorias:
//! (1) texto simple por JSON con usuario autenticado -> 200, author correcto, authorId del body ignorado;
//! (2) sin sesión -> 401;
//! (3) PNG de ~3 MB (supera el límite default de 2 MB de Axum) -> 200 y archivo guardado con chat_*.png;
//! (4) extensiones no permitidas (.html, .svg) o content-type inválido -> 400;
//! (5) archivo de 9 MB -> 400 "El archivo supera el máximo de 8 MB." y el directorio queda sin archivo;
//! (6) texto > 400 caracteres -> 400; mensaje vacío sin imagen -> 400;
//! (7) dos mensajes seguidos del mismo usuario en < 1 s -> segundo da 400 de frecuencia;
//! (8) "../../evil.png" no escapa del directorio y se guarda como chat_*;
//! Más pruebas unitarias de `resolve_discord_avatar_url` (los 4 casos).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use lobby::{resolve_discord_avatar_url, LobbyApiState, LobbyManager};
use web::session::{new_claims, sign, SessionState};

const SECRET: &str = "lobby-chat-test-secret-at-least-32-chars!";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_token(
    user_id: &str,
    username: &str,
    avatar: Option<&str>,
    role: &str,
    is_admin: bool,
    servidor: Option<&str>,
) -> String {
    let c = new_claims(
        user_id.into(),
        username.into(),
        avatar.map(str::to_owned),
        role.into(),
        is_admin,
        servidor.map(str::to_owned),
        now(),
    );
    sign(&c, SECRET.as_bytes()).unwrap()
}

fn setup_test_app() -> (Router, PathBuf, Arc<LobbyManager>) {
    let session = SessionState::new(SECRET.as_bytes(), false).unwrap();
    let tmp = tempfile::tempdir().expect("tempdir");
    let uploads_dir = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    let manager = Arc::new(LobbyManager::new("default"));
    let router = lobby::router(LobbyApiState {
        manager: manager.clone(),
        auth: Arc::new(session),
        uploads_dir: uploads_dir.clone(),
    });
    (router, uploads_dir, manager)
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024 * 12)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Construye un cuerpo multipart/form-data a mano con delimitador (boundary).
fn make_multipart_body(
    boundary: &str,
    text_fields: &[(&str, &str)],
    files: &[(&str, &str, &str, &[u8])], // (nombre_campo, nombre_archivo, content_type, datos)
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, val) in text_fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{val}\r\n").as_bytes(),
        );
    }
    for (name, filename, ct, data) in files {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {ct}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

// ---------------------------------------------------------------------------
// (1) Texto simple por JSON con usuario autenticado -> 200, author y discordId
// protegidos; authorId falso en el body es ignorado.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_1_texto_simple_por_json_con_usuario_autenticado() {
    let (app, _uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_real_123", "JugadorPro", None, "user", false, None);

    let payload = json!({
        "message": "Hola a toda la sala de Mixtón",
        "authorId": "attacker_666",
        "discordId": "attacker_666",
        "authorName": "HackerFake",
        "author": "Impostor"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(payload.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    let msg = &body["message"];
    assert_eq!(msg["author"], "JugadorPro");
    assert_eq!(msg["discordId"], "u_real_123");
    assert_eq!(msg["content"], "Hola a toda la sala de Mixtón");
    assert!(msg["imageUrl"].is_null());
    assert_eq!(msg["source"], "web");
    assert_eq!(msg["isAdmin"], false);
    assert!(msg["id"].as_str().unwrap().starts_with("web_"));
}

// ---------------------------------------------------------------------------
// (2) Petición sin sesión -> 401 Unauthorized
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_2_sin_sesion_devuelve_401() {
    let (app, _uploads_dir, _mgr) = setup_test_app();

    // Ruta mixton-chat-send
    let req1 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"message": "hola"}).to_string()))
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::UNAUTHORIZED);

    // Ruta mixton-chat-message
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-message")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"message": "hola"}).to_string()))
        .unwrap();
    let res2 = app.oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// (3) PNG de ~3 MB (supera límite de 2 MB de Axum) -> 200, guardado con chat_*
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_3_png_de_3mb_supera_limite_axum_default_y_guarda_archivo() {
    let (app, uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_artist", "Artista", None, "user", false, None);

    let boundary = "---------------------------boundary3mbtest";
    // Generar ~3 MB de datos de imagen válidos
    let mut img_data = vec![0x89, 0x50, 0x4E, 0x47]; // cabecera PNG
    img_data.resize(3 * 1024 * 1024, 0x55);

    let multipart_body = make_multipart_body(
        boundary,
        &[("message", "Captura de 3 MB")],
        &[("image", "screenshot.PNG", "image/png", &img_data)],
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(multipart_body))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = body_json(res).await;
    assert_eq!(body["success"], true);

    let msg = &body["message"];
    let img_url = msg["imageUrl"].as_str().expect("imageUrl presente");
    assert!(
        img_url.starts_with("/uploads/chat/chat_"),
        "URL debe empezar por /uploads/chat/chat_, obtenida: {img_url}"
    );
    assert!(img_url.ends_with(".png"));

    let filename = img_url.strip_prefix("/uploads/chat/").unwrap();
    let saved_path = uploads_dir.join(filename);
    assert!(
        saved_path.exists(),
        "El archivo guardado debe existir en el directorio temporal"
    );
    let meta = std::fs::metadata(&saved_path).unwrap();
    assert_eq!(meta.len(), img_data.len() as u64);
}

// ---------------------------------------------------------------------------
// (4) Extensiones y content-types no permitidos -> 400
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_4_archivos_no_imagen_o_extension_invalida_devuelven_400() {
    let (app, uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_tester", "Tester", None, "user", false, None);

    let boundary = "---------------------------boundary4test";

    // 4a. Archivo .html con Content-Type: text/html
    let body_html = make_multipart_body(
        boundary,
        &[("message", "Intento de exploit HTML")],
        &[(
            "image",
            "malicious.html",
            "text/html",
            b"<script>alert(1)</script>",
        )],
    );
    let req1 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(body_html))
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::BAD_REQUEST);
    let json1 = body_json(res1).await;
    assert_eq!(
        json1["error"],
        "Solo se permiten archivos de imagen (PNG, JPG, WEBP, GIF)."
    );

    // 4b. Archivo con Content-Type: image/png pero extensión .svg
    let body_svg = make_multipart_body(
        boundary,
        &[("message", "Intento con SVG")],
        &[("image", "vector.svg", "image/png", b"<svg><circle/></svg>")],
    );
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(body_svg))
        .unwrap();
    let res2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::BAD_REQUEST);
    let json2 = body_json(res2).await;
    assert_eq!(
        json2["error"],
        "Solo se permiten archivos de imagen (PNG, JPG, WEBP, GIF)."
    );

    // 4c. Archivo con Content-Type: image/png pero extensión .html
    let body_png_html = make_multipart_body(
        boundary,
        &[("message", "Intento con html disfrazado")],
        &[(
            "image",
            "test.html",
            "image/png",
            b"<html><body>test</body></html>",
        )],
    );
    let req3 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(body_png_html))
        .unwrap();
    let res3 = app.oneshot(req3).await.unwrap();
    assert_eq!(res3.status(), StatusCode::BAD_REQUEST);
    let json3 = body_json(res3).await;
    assert_eq!(
        json3["error"],
        "Solo se permiten archivos de imagen (PNG, JPG, WEBP, GIF)."
    );

    // El directorio temporal debe quedar completamente limpio
    let files_count = std::fs::read_dir(&uploads_dir).unwrap().count();
    assert_eq!(files_count, 0);
}

// ---------------------------------------------------------------------------
// (5) Archivo de 9 MB -> 400 "El archivo supera el máximo de 8 MB."
// y el directorio de subida queda sin ese archivo.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_5_archivo_de_9mb_devuelve_400_y_no_queda_en_directorio() {
    let (app, uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_big", "BigUploader", None, "user", false, None);

    let boundary = "---------------------------boundary9mb";
    // 9 * 1024 * 1024 bytes = exactamente 9 MiB
    let big_data = vec![0x42; 9 * 1024 * 1024];

    let body_bytes = make_multipart_body(
        boundary,
        &[("message", "Subiendo archivo gigante")],
        &[("image", "gigante.jpg", "image/jpeg", &big_data)],
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(body_bytes))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let body = body_json(res).await;
    assert_eq!(body["error"], "El archivo supera el máximo de 8 MB.");

    // El directorio de subida debe quedar sin ese archivo
    let count = std::fs::read_dir(&uploads_dir).unwrap().count();
    assert_eq!(count, 0, "No debe quedar ningún archivo guardado en disco");
}

// ---------------------------------------------------------------------------
// (6) Texto de 401 caracteres -> 400; mensaje vacío sin imagen -> 400
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_6_texto_401_chars_y_mensaje_vacio_devuelven_400() {
    let (app, _uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_validator", "Validator", None, "user", false, None);

    // 6a. Texto de 401 caracteres (contando caracteres Unicode)
    let long_text = "a".repeat(401);
    let req1 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(json!({"message": long_text}).to_string()))
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::BAD_REQUEST);
    let json1 = body_json(res1).await;
    assert_eq!(
        json1["error"],
        "El mensaje no puede superar los 400 caracteres."
    );

    // 6b. Mensaje vacío sin imagen
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(json!({"message": "   \n\t  "}).to_string()))
        .unwrap();
    let res2 = app.oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::BAD_REQUEST);
    let json2 = body_json(res2).await;
    assert_eq!(
        json2["error"],
        "El mensaje debe contener texto o una imagen."
    );
}

// ---------------------------------------------------------------------------
// (7) Frecuencia: dos mensajes seguidos del mismo usuario en < 1 s -> 400
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_7_frecuencia_dos_mensajes_en_menos_de_1s_devuelve_400() {
    let (app, _uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_spammer", "Spammer", None, "user", false, None);

    // Primer mensaje -> 200 OK
    let req1 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(json!({"message": "Primer mensaje"}).to_string()))
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    // Segundo mensaje inmediato del mismo usuario -> 400 con mensaje exacto
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(
            json!({"message": "Segundo mensaje inmediato"}).to_string(),
        ))
        .unwrap();
    let res2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::BAD_REQUEST);
    let json2 = body_json(res2).await;
    assert_eq!(
        json2["error"],
        "Por favor espera un segundo antes de enviar otro mensaje."
    );

    // Otro usuario distinto sí puede enviar de inmediato -> 200 OK
    let token_other = make_token("u_other", "OtherUser", None, "user", false, None);
    let req3 = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("auth_token={token_other}"))
        .body(Body::from(
            json!({"message": "Mensaje de otro usuario"}).to_string(),
        ))
        .unwrap();
    let res3 = app.oneshot(req3).await.unwrap();
    assert_eq!(res3.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// (8) Nombre malicioso "../../evil.png" no escapa del directorio temporal
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_8_path_traversal_evil_png_no_escapa_del_directorio() {
    let (app, uploads_dir, _mgr) = setup_test_app();
    let token = make_token("u_sec", "SecAuditor", None, "user", false, None);

    let boundary = "---------------------------boundary8traversal";
    let body_bytes = make_multipart_body(
        boundary,
        &[("message", "Probando path traversal")],
        &[("image", "../../evil.png", "image/png", b"fake-png-content")],
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/lobby/mixton-chat-send")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::from(body_bytes))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = body_json(res).await;
    let img_url = body["message"]["imageUrl"].as_str().unwrap();

    assert!(img_url.starts_with("/uploads/chat/chat_"));
    assert!(img_url.ends_with(".png"));
    assert!(!img_url.contains(".."));
    assert!(!img_url.contains("evil"));

    let filename = img_url.strip_prefix("/uploads/chat/").unwrap();
    let saved_file = uploads_dir.join(filename);
    assert!(
        saved_file.exists(),
        "El archivo debe guardarse dentro de uploads_dir"
    );

    // Verificar que nunca se escribió en el directorio superior
    let parent_evil = uploads_dir.join("..").join("evil.png");
    assert!(
        !parent_evil.exists(),
        "No debe existir archivo fuera de uploads_dir"
    );
}

// ---------------------------------------------------------------------------
// Pruebas unitarias de resolve_discord_avatar_url (los 4 casos)
// ---------------------------------------------------------------------------
#[test]
fn test_resolve_discord_avatar_url_casos() {
    // Caso 1: Avatar empieza por http:// o https:// -> se devuelve tal cual
    let url_https = resolve_discord_avatar_url(
        Some("123456789"),
        Some("https://images.example.com/custom_avatar.webp"),
    );
    assert_eq!(url_https, "https://images.example.com/custom_avatar.webp");

    let url_http = resolve_discord_avatar_url(
        Some("123456789"),
        Some("http://images.example.com/custom_avatar.png"),
    );
    assert_eq!(url_http, "http://images.example.com/custom_avatar.png");

    // Caso 2: Avatar es hash hex/underscore (^[0-9a-fA-F_]+$) y hay user_id -> CDN con ?size=128
    let url_hex =
        resolve_discord_avatar_url(Some("803378680482594817"), Some("a_1234567890abcdef_99"));
    assert_eq!(
        url_hex,
        "https://cdn.discordapp.com/avatars/803378680482594817/a_1234567890abcdef_99.png?size=128"
    );

    // Caso 3: user_id numérico sin avatar -> embed/avatars/{(user_id >> 22) % 6}.png
    // 803378680482594817 >> 22 = 191540403481; 191540403481 % 6 = 1
    let url_snowflake = resolve_discord_avatar_url(Some("803378680482594817"), None);
    assert_eq!(
        url_snowflake,
        "https://cdn.discordapp.com/embed/avatars/1.png"
    );

    // Otro snowflake para verificar cálculo modular:
    // (0 >> 22) % 6 = 0
    let url_zero = resolve_discord_avatar_url(Some("0"), None);
    assert_eq!(url_zero, "https://cdn.discordapp.com/embed/avatars/0.png");

    // Caso 4: Sin user_id numérico ni avatar -> embed/avatars/0.png
    let url_fallback_none = resolve_discord_avatar_url(None, None);
    assert_eq!(
        url_fallback_none,
        "https://cdn.discordapp.com/embed/avatars/0.png"
    );

    let url_fallback_str = resolve_discord_avatar_url(Some("not_a_number"), None);
    assert_eq!(
        url_fallback_str,
        "https://cdn.discordapp.com/embed/avatars/0.png"
    );
}
