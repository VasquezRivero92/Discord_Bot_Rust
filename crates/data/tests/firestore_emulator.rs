//! Pruebas del cliente Firestore.
//!
//! Hay dos grupos:
//!
//! - Pruebas unitarias del mapeo de errores (sin red): se ejecutan siempre con `cargo test`.
//! - Pruebas contra el **emulador** de Firestore, marcadas `#[ignore]`. Para ejecutarlas:
//!
//!   ```text
//!   firebase emulators:start --only firestore --project demo-machupeek   # escucha en localhost:8080
//!   FIRESTORE_EMULATOR_HOST=localhost:8080 cargo test -p data -- --ignored
//!   ```
//!
//!   El emulador no necesita datos precargados: cada prueba crea sus propios documentos
//!   (con la API REST del emulador) en colecciones con nombres únicos. Si
//!   `FIRESTORE_EMULATOR_HOST` no está definida, las pruebas de emulador fallan de inmediato:
//!   nunca se conectan a Firestore de producción. El proyecto usado (`demo-machupeek`) lleva el
//!   prefijo `demo-`, que el emulador reserva para proyectos sin credenciales reales.

use data::{ChangeEvent, DataError, Dir, FirestoreClient, Op, Query, Value};
use firestore::gcloud_sdk::tonic::Status;
use futures::StreamExt;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROJECT_ID: &str = "demo-machupeek";

#[test]
fn quota_error_maps_to_quota_exceeded() {
    let err = DataError::from(Status::resource_exhausted("Quota exceeded."));
    assert_eq!(err, DataError::QuotaExceeded);
}

#[test]
fn not_found_status_maps_to_not_found() {
    let err = DataError::from(Status::not_found("no such document"));
    assert_eq!(err, DataError::NotFound);
}

#[test]
fn other_status_maps_to_firestore_with_message() {
    let err = DataError::from(Status::permission_denied("missing permission"));
    match err {
        DataError::Firestore(msg) => assert!(msg.contains("missing permission"), "{msg}"),
        other => panic!("se esperaba DataError::Firestore, llegó {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Pruebas contra el emulador
// ---------------------------------------------------------------------------------------------

/// Host del emulador. Falla si no está definido, para no tocar nunca producción.
fn emulator_host() -> String {
    std::env::var("FIRESTORE_EMULATOR_HOST").expect(
        "FIRESTORE_EMULATOR_HOST debe apuntar al emulador (p. ej. localhost:8080); \
         estas pruebas nunca se ejecutan contra producción",
    )
}

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("reloj")
        .as_nanos();
    format!("{prefix}_{nanos}")
}

async fn emulator_client() -> FirestoreClient {
    emulator_host();
    FirestoreClient::connect(PROJECT_ID, None)
        .await
        .expect("conexión al emulador")
}

/// Crea o reemplaza un documento con la API REST del emulador (`PATCH`), como haría un
/// escritor externo (el bot Node). `fields_json` es el objeto `fields` de la API REST.
async fn rest_upsert(collection: &str, id: &str, fields_json: &str) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let host = emulator_host();
    let body = format!("{{\"fields\":{fields_json}}}");
    let path = format!("/v1/projects/{PROJECT_ID}/databases/(default)/documents/{collection}/{id}");
    let request = format!(
        "PATCH {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer owner\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut socket = tokio::net::TcpStream::connect(&host)
        .await
        .expect("conexión TCP al emulador");
    socket
        .write_all(request.as_bytes())
        .await
        .expect("envío REST");
    let mut response = String::new();
    socket
        .read_to_string(&mut response)
        .await
        .expect("respuesta REST");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "el emulador rechazó la escritura REST: {response}"
    );
}

#[derive(Debug, Deserialize, PartialEq)]
struct Sample {
    name: String,
    level: i64,
    active: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    nickname: Option<String>,
}

/// Requiere el emulador (ver la cabecera del archivo).
#[tokio::test]
#[ignore = "requiere FIRESTORE_EMULATOR_HOST con el emulador de Firestore en marcha"]
async fn get_doc_returns_none_for_missing() {
    let client = emulator_client().await;
    let got: Option<Sample> = client
        .get_doc(&unique("missing_coll"), "does-not-exist")
        .await
        .expect("get_doc");
    assert_eq!(got, None);
}

/// Requiere el emulador (ver la cabecera del archivo).
#[tokio::test]
#[ignore = "requiere FIRESTORE_EMULATOR_HOST con el emulador de Firestore en marcha"]
async fn get_doc_roundtrip() {
    let client = emulator_client().await;
    let collection = unique("roundtrip");
    rest_upsert(
        &collection,
        "doc1",
        r#"{"name":{"stringValue":"neo"},"level":{"integerValue":"7"},
            "active":{"booleanValue":true},
            "updatedAt":{"timestampValue":"2026-11-02T10:00:00Z"}}"#,
    )
    .await;

    let got: Option<Sample> = client.get_doc(&collection, "doc1").await.expect("get_doc");
    let got = got.expect("el documento existe");
    assert_eq!(got.name, "neo");
    assert_eq!(got.level, 7);
    assert!(got.active);
    assert!(got.updated_at.starts_with("2026-11-02T10:00:00"));
    assert_eq!(got.nickname, None);

    let listed: Vec<(String, Sample)> = client.list_docs(&collection).await.expect("list_docs");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "doc1");
    assert_eq!(listed[0].1, got);
}

/// Requiere el emulador (ver la cabecera del archivo).
#[tokio::test]
#[ignore = "requiere FIRESTORE_EMULATOR_HOST con el emulador de Firestore en marcha"]
async fn watch_emits_modified_after_update() {
    let client = emulator_client().await;
    let collection = unique("watch");
    rest_upsert(&collection, "w1", r#"{"name":{"stringValue":"a"}}"#).await;

    let mut events = Box::pin(client.watch_collection(&collection));

    // Instantánea inicial: el documento existente llega como Added.
    let first = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .expect("Added inicial en < 5 s")
        .expect("el stream sigue abierto")
        .expect("evento sin error");
    assert_eq!(first, ChangeEvent::Added("w1".into()));

    rest_upsert(&collection, "w1", r#"{"name":{"stringValue":"b"}}"#).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let next = tokio::time::timeout_at(deadline, events.next())
            .await
            .expect("Modified en < 5 s")
            .expect("el stream sigue abierto")
            .expect("evento sin error");
        if next == ChangeEvent::Modified("w1".into()) {
            break;
        }
    }
}

/// Requiere el emulador (ver la cabecera del archivo). `watch_document` solo entrega eventos del
/// documento observado, nunca de sus vecinos de colección.
#[tokio::test]
#[ignore = "requiere FIRESTORE_EMULATOR_HOST con el emulador de Firestore en marcha"]
async fn watch_document_ignores_other_documents() {
    let client = emulator_client().await;
    let collection = unique("watchdoc");
    rest_upsert(&collection, "settings", r#"{"v":{"stringValue":"a"}}"#).await;
    rest_upsert(&collection, "mixton_chat_x", r#"{"v":{"stringValue":"a"}}"#).await;

    let mut events = Box::pin(client.watch_document(&collection, "settings"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    macro_rules! next {
        () => {
            tokio::time::timeout_at(deadline, events.next())
                .await
                .expect("evento a tiempo")
                .expect("el stream sigue abierto")
                .expect("evento sin error")
        };
    }
    assert_eq!(next!(), ChangeEvent::Added("settings".into()));
    assert_eq!(next!(), ChangeEvent::Synced(vec!["settings".into()]));

    rest_upsert(&collection, "mixton_chat_x", r#"{"v":{"stringValue":"b"}}"#).await;
    rest_upsert(&collection, "settings", r#"{"v":{"stringValue":"b"}}"#).await;
    loop {
        let event = next!();
        assert!(
            !format!("{event:?}").contains("mixton_chat_x"),
            "evento de otro documento: {event:?}"
        );
        if event == ChangeEvent::Modified("settings".into()) {
            break;
        }
    }
}

/// Requiere el emulador (ver la cabecera del archivo).
#[tokio::test]
#[ignore = "requiere FIRESTORE_EMULATOR_HOST con el emulador de Firestore en marcha"]
async fn query_orders_and_limits() {
    let client = emulator_client().await;
    let collection = unique("query");
    for (id, name, level) in [
        ("a", "uno", 1),
        ("b", "dos", 5),
        ("c", "tres", 3),
        ("d", "x", 9),
    ] {
        rest_upsert(
            &collection,
            id,
            &format!(
                r#"{{"name":{{"stringValue":"{name}"}},"level":{{"integerValue":"{level}"}},
                    "active":{{"booleanValue":true}},
                    "updatedAt":{{"timestampValue":"2026-11-02T10:00:00Z"}}}}"#
            ),
        )
        .await;
    }

    // Filtro de rango + orden descendente + límite (un solo campo: sin índice compuesto).
    let got: Vec<(String, Sample)> = client
        .query(
            &collection,
            Query {
                filters: vec![("level".into(), Op::Ge, Value::Integer(3))],
                order_by: Some(("level".into(), Dir::Desc)),
                limit: Some(2),
            },
        )
        .await
        .expect("query");
    let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["d", "b"]);
    assert_eq!(got[0].1.level, 9);

    // Filtro de igualdad sin orden ni límite.
    let named: Vec<(String, Sample)> = client
        .query(
            &collection,
            Query {
                filters: vec![("name".into(), Op::Eq, Value::String("tres".into()))],
                ..Query::default()
            },
        )
        .await
        .expect("query");
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].0, "c");
}
