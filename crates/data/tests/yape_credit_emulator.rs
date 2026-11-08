//! Pruebas de `FirestoreClient::credit_yape_operation` contra el **emulador** de Firestore.
//!
//! Marcadas `#[ignore]`: necesitan un emulador (ver `firestore_emulator.rs`):
//!
//! ```text
//! docker run --rm -p 8080:8080 mtlynch/firestore-emulator
//! FIRESTORE_EMULATOR_HOST=localhost:8080 cargo test -p data --test yape_credit_emulator -- --ignored
//! ```
//!
//! Si `FIRESTORE_EMULATOR_HOST` no está definida fallan de inmediato: nunca tocan producción.
//! Cada prueba usa operaciones y usuarios con ids únicos.

use data::{FirestoreClient, YapeCredit, YapeError};
use std::time::{SystemTime, UNIX_EPOCH};

const PROJECT_ID: &str = "demo-machupeek";

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
    format!("{prefix}{nanos}")
}

async fn client() -> FirestoreClient {
    emulator_host();
    FirestoreClient::connect(PROJECT_ID, None)
        .await
        .expect("conexión al emulador")
}

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

/// Pago del monitor: `tipo`, `monto` (entero, como en `yape_transacciones`) y la operación.
async fn seed_payment(op: &str, tipo: &str, monto: &str) {
    rest_upsert(
        "yape_transacciones",
        op,
        &format!(
            "{{\"tipo\":{{\"stringValue\":\"{tipo}\"}},\"monto\":{{\"integerValue\":\"{monto}\"}},\
             \"numero_operacion\":{{\"stringValue\":\"{op}\"}},\
             \"fecha_hora\":{{\"stringValue\":\"07 nov. 2026 - 06:55 p. m.\"}}}}"
        ),
    )
    .await;
}

#[tokio::test]
#[ignore = "necesita el emulador de Firestore"]
async fn acredita_una_sola_vez_y_suma_al_saldo_existente() {
    let fs = client().await;
    let user = unique("u");
    let other = unique("o");
    let op = unique("1");
    rest_upsert(
        "faceit_users",
        &user,
        "{\"wallet_balance\":{\"integerValue\":\"3\"},\"discordId\":{\"stringValue\":\"x\"}}",
    )
    .await;
    seed_payment(&op, "ENTRADA", "5").await;

    let got = fs
        .credit_yape_operation(&op, &user, None, 5.0, Some("987654321"), "test")
        .await
        .expect("acredita");
    assert_eq!(
        got,
        YapeCredit::Credited {
            operation: op.clone(),
            amount: 5.0,
            new_balance: 8.0
        }
    );

    // Mismo usuario y otro usuario: la operación ya está usada, y el saldo no vuelve a subir.
    assert_eq!(
        fs.credit_yape_operation(&op, &user, None, 5.0, None, "test")
            .await
            .unwrap(),
        YapeCredit::AlreadyCredited { own: true }
    );
    assert_eq!(
        fs.credit_yape_operation(&op, &other, None, 5.0, None, "test")
            .await
            .unwrap(),
        YapeCredit::AlreadyCredited { own: false }
    );
    let balance = fs
        .get_doc::<serde_json::Value>("faceit_users", &user)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(balance["wallet_balance"].as_f64(), Some(8.0));
    assert_eq!(balance["phoneNumber"], "987654321");
}

#[tokio::test]
#[ignore = "necesita el emulador de Firestore"]
async fn crea_el_usuario_si_no_existe() {
    let fs = client().await;
    let user = unique("n");
    let op = unique("2");
    seed_payment(&op, "ENTRADA", "10").await;
    let got = fs
        .credit_yape_operation(&op, &user, None, 10.0, None, "test")
        .await
        .expect("acredita");
    assert!(matches!(got, YapeCredit::Credited { new_balance, .. } if new_balance == 10.0));
    let doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &user)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(doc["discordId"], user.as_str());
    assert_eq!(doc["wallet_balance"].as_f64(), Some(10.0));
}

#[tokio::test]
#[ignore = "necesita el emulador de Firestore"]
async fn rechaza_pagos_que_no_corresponden() {
    let fs = client().await;
    let user = unique("r");

    // Aún no llegó: no se escribe nada.
    let missing = unique("3");
    assert_eq!(
        fs.credit_yape_operation(&missing, &user, None, 5.0, None, "test")
            .await
            .unwrap(),
        YapeCredit::NotYetSeen
    );
    assert!(fs
        .get_doc::<serde_json::Value>("yape_operations", &missing)
        .await
        .unwrap()
        .is_none());

    // Salida en vez de entrada.
    let out = unique("4");
    seed_payment(&out, "SALIDA", "5").await;
    assert!(matches!(
        fs.credit_yape_operation(&out, &user, None, 5.0, None, "test").await,
        Err(YapeError::Invalid(_))
    ));

    // Monto distinto al del depósito.
    let wrong = unique("5");
    seed_payment(&wrong, "ENTRADA", "5").await;
    assert!(matches!(
        fs.credit_yape_operation(&wrong, &user, None, 50.0, None, "test").await,
        Err(YapeError::Invalid(_))
    ));
    assert!(fs
        .get_doc::<serde_json::Value>("faceit_users", &user)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "necesita el emulador de Firestore"]
async fn respeta_reservas_pendientes_del_legado() {
    let fs = client().await;
    let owner = unique("p");
    let thief = unique("t");
    let op = unique("6");
    seed_payment(&op, "ENTRADA", "5").await;
    rest_upsert(
        "yape_operations",
        &op,
        &format!(
            "{{\"operationNumber\":{{\"stringValue\":\"{op}\"}},\"discordId\":{{\"stringValue\":\"{owner}\"}},\
             \"amount\":{{\"integerValue\":\"5\"}},\"status\":{{\"stringValue\":\"pending\"}},\
             \"credited\":{{\"booleanValue\":false}}}}"
        ),
    )
    .await;

    assert_eq!(
        fs.credit_yape_operation(&op, &thief, None, 5.0, None, "test")
            .await
            .unwrap(),
        YapeCredit::ReservedByOther
    );
    assert!(matches!(
        fs.credit_yape_operation(&op, &owner, None, 5.0, None, "test")
            .await
            .unwrap(),
        YapeCredit::Credited { .. }
    ));
}

#[tokio::test]
#[ignore = "necesita el emulador de Firestore"]
async fn dos_peticiones_simultaneas_acreditan_una_sola_vez() {
    let fs = client().await;
    let user = unique("c");
    let op = unique("7");
    seed_payment(&op, "ENTRADA", "5").await;

    let (a, b) = tokio::join!(
        fs.credit_yape_operation(&op, &user, None, 5.0, None, "test"),
        fs.credit_yape_operation(&op, &user, None, 5.0, None, "test"),
    );
    let credited = [&a, &b]
        .iter()
        .filter(|r| matches!(r, Ok(YapeCredit::Credited { .. })))
        .count();
    assert_eq!(credited, 1, "a={a:?} b={b:?}");
    let doc = fs
        .get_doc::<serde_json::Value>("faceit_users", &user)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(doc["wallet_balance"].as_f64(), Some(5.0));
}
