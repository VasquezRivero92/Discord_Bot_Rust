//! Guardas de tiempo del proxy (`ProxyOptions::first_byte_timeout` y
//! `ws_idle_timeout`) contra un Node simulado.
//!
//! Con sockets reales no se usa `tokio::time::pause`: el avance automático del
//! reloj podría disparar el `CONNECT_TIMEOUT` del proxy antes de que el kernel
//! complete una conexión de loopback. En su lugar el Node simulado se bloquea
//! en un `Notify` que el test libera, de modo que el resultado no depende del
//! planificador: las esperas solo pueden alargar el test, nunca invertirlo. El
//! temporizador de inactividad se prueba además con reloj pausado y sin
//! sockets en `proxy.rs`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::{Body, Bytes},
    extract::ws::{Message, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, protocol::frame::coding::CloseCode};
use web::proxy::ProxyOptions;

const FIRST_BYTE: Duration = Duration::from_millis(300);
/// Bastante más que `FIRST_BYTE`: si la guarda aplicara, ya habría respondido.
const PAST_FIRST_BYTE: Duration = Duration::from_millis(900);
const GENEROUS: Duration = Duration::from_secs(10);
const SERVICE_UNAVAILABLE: &str = r#"{"error":"Servicio no disponible"}"#;

struct TestServer {
    addr: SocketAddr,
    handle: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn serve(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    TestServer { addr, handle }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}

async fn proxy_with(legacy: SocketAddr, options: ProxyOptions) -> TestServer {
    serve(web::app_with_proxy_options(
        format!("http://{legacy}"),
        options,
    ))
    .await
}

fn first_byte(limit: Duration) -> ProxyOptions {
    ProxyOptions {
        first_byte_timeout: Some(limit),
        ..Default::default()
    }
}

/// Node simulado cuyas respuestas esperan a `release`. `arrived` avisa cuando
/// llega cada petición.
struct GatedLegacy {
    server: TestServer,
    release: Arc<Notify>,
    arrived: mpsc::UnboundedReceiver<()>,
}

async fn gated_legacy() -> GatedLegacy {
    let release = Arc::new(Notify::new());
    let (arrived_tx, arrived) = mpsc::unbounded_channel();

    let head_after_release = {
        let release = release.clone();
        let arrived_tx = arrived_tx.clone();
        move || {
            let release = release.clone();
            let arrived_tx = arrived_tx.clone();
            async move {
                let _ = arrived_tx.send(());
                release.notified().await;
                "late"
            }
        }
    };
    let sse = {
        let release = release.clone();
        let arrived_tx = arrived_tx.clone();
        move || {
            let release = release.clone();
            let arrived_tx = arrived_tx.clone();
            async move {
                let _ = arrived_tx.send(());
                release.notified().await;
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    "data: {}\n\n",
                )
            }
        }
    };
    // Cabecera y primer trozo al instante; el resto, tras `release`.
    let drip = {
        let release = release.clone();
        move || {
            let release = release.clone();
            async move {
                let body = futures_util::stream::unfold(0u8, move |step| {
                    let release = release.clone();
                    async move {
                        match step {
                            0 => Some((Ok::<_, std::io::Error>(Bytes::from("uno")), 1)),
                            1 => {
                                release.notified().await;
                                Some((Ok(Bytes::from("dos")), 2))
                            }
                            _ => None,
                        }
                    }
                });
                Response::new(Body::from_stream(body))
            }
        }
    };
    let app = Router::new()
        .route("/slow", get(head_after_release))
        .route("/api/halloween/stream", get(sse))
        .route("/drip", get(drip));
    GatedLegacy {
        server: serve(app).await,
        release,
        arrived,
    }
}

#[tokio::test]
async fn slow_upstream_without_first_byte_gets_504() {
    let legacy = gated_legacy().await;
    let proxy = proxy_with(legacy.server.addr, first_byte(FIRST_BYTE)).await;

    let started = Instant::now();
    let res = tokio::time::timeout(
        GENEROUS,
        client()
            .get(format!("http://{}/slow?token=x", proxy.addr))
            .send(),
    )
    .await
    .expect("no 504: the request waited for Node")
    .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    assert_eq!(res.text().await.unwrap(), SERVICE_UNAVAILABLE);
    assert!(elapsed >= FIRST_BYTE, "{elapsed:?}");
    assert!(elapsed < GENEROUS, "{elapsed:?}");
}

#[tokio::test]
async fn sse_route_is_exempt_from_first_byte_timeout() {
    let mut legacy = gated_legacy().await;
    let proxy = proxy_with(legacy.server.addr, first_byte(FIRST_BYTE)).await;

    let pending = tokio::spawn(
        client()
            .get(format!("http://{}/api/halloween/stream", proxy.addr))
            .send(),
    );
    legacy.arrived.recv().await.unwrap();
    tokio::time::sleep(PAST_FIRST_BYTE).await;
    legacy.release.notify_one();

    let res = pending.await.unwrap().unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    assert_eq!(res.text().await.unwrap(), "data: {}\n\n");
}

#[tokio::test]
async fn first_byte_timeout_is_disabled_by_default() {
    let mut legacy = gated_legacy().await;
    let proxy = proxy_with(legacy.server.addr, ProxyOptions::default()).await;

    let pending = tokio::spawn(client().get(format!("http://{}/slow", proxy.addr)).send());
    legacy.arrived.recv().await.unwrap();
    tokio::time::sleep(PAST_FIRST_BYTE).await;
    legacy.release.notify_one();

    let res = pending.await.unwrap().unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await.unwrap(), "late");
}

#[tokio::test]
async fn body_already_started_is_not_cut_by_first_byte_timeout() {
    let legacy = gated_legacy().await;
    let proxy = proxy_with(legacy.server.addr, first_byte(FIRST_BYTE)).await;

    let mut res = client()
        .get(format!("http://{}/drip", proxy.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.chunk().await.unwrap().unwrap(), "uno");
    tokio::time::sleep(PAST_FIRST_BYTE).await;
    legacy.release.notify_one();
    let mut rest = Vec::new();
    while let Some(chunk) = res.chunk().await.unwrap() {
        rest.extend_from_slice(&chunk);
    }
    assert_eq!(rest, b"dos");
}

// ------------------------------------------------------------ WebSocket

/// Node simulado con eco; al terminar cada socket informa del código de
/// cierre recibido (`None` si acabó sin frame de cierre).
async fn ws_legacy() -> (TestServer, mpsc::UnboundedReceiver<Option<u16>>) {
    let (closed_tx, closed) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let closed_tx = closed_tx.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(msg)) = socket.recv().await {
                        match msg {
                            Message::Text(_) | Message::Binary(_) => {
                                if socket.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Message::Close(frame) => {
                                let _ = closed_tx.send(frame.map(|f| f.code));
                                return;
                            }
                            _ => {}
                        }
                    }
                    let _ = closed_tx.send(None);
                })
                .into_response()
            }
        }),
    );
    (serve(app).await, closed)
}

fn ws_idle(limit: Duration) -> ProxyOptions {
    ProxyOptions {
        ws_idle_timeout: Some(limit),
        ..Default::default()
    }
}

#[tokio::test]
async fn idle_websocket_is_closed_after_timeout() {
    let idle = Duration::from_millis(300);
    let (legacy, mut closed) = ws_legacy().await;
    let proxy = proxy_with(legacy.addr, ws_idle(idle)).await;

    let started = Instant::now();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", proxy.addr))
        .await
        .unwrap();
    let got = tokio::time::timeout(GENEROUS, ws.next())
        .await
        .expect("idle socket was never closed");
    let elapsed = started.elapsed();
    match got {
        Some(Ok(tungstenite::Message::Close(Some(frame)))) => {
            assert_eq!(frame.code, CloseCode::Away);
        }
        other => panic!("expected a close frame, got {other:?}"),
    }
    assert!(elapsed >= idle, "{elapsed:?}");

    // Node also got the close frame.
    let upstream = tokio::time::timeout(GENEROUS, closed.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(upstream, Some(1001));
}

#[tokio::test]
async fn active_websocket_is_not_closed() {
    // 200 ms between frames against a 1 s limit: only a 5x stall could fail it.
    let idle = Duration::from_secs(1);
    let (legacy, mut closed) = ws_legacy().await;
    let proxy = proxy_with(legacy.addr, ws_idle(idle)).await;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", proxy.addr))
        .await
        .unwrap();
    let started = Instant::now();
    let mut round = 0u32;
    while started.elapsed() < idle * 3 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        round += 1;
        if round.is_multiple_of(2) {
            // Only pings: they count as activity too.
            ws.send(tungstenite::Message::Ping(vec![round as u8].into()))
                .await
                .unwrap();
            loop {
                match ws.next().await {
                    Some(Ok(tungstenite::Message::Pong(_))) => break,
                    Some(Ok(tungstenite::Message::Close(f))) => {
                        panic!("closed while active: {f:?}")
                    }
                    Some(Ok(_)) => {}
                    other => panic!("socket ended while active: {other:?}"),
                }
            }
        } else {
            let text = format!("frame-{round}");
            ws.send(tungstenite::Message::text(text.clone()))
                .await
                .unwrap();
            loop {
                match ws.next().await {
                    Some(Ok(tungstenite::Message::Text(t))) => {
                        assert_eq!(t.as_str(), text);
                        break;
                    }
                    Some(Ok(tungstenite::Message::Close(f))) => {
                        panic!("closed while active: {f:?}")
                    }
                    // Pongs left over from the previous ping.
                    Some(Ok(_)) => {}
                    other => panic!("socket ended while active: {other:?}"),
                }
            }
        }
    }
    assert!(closed.try_recv().is_err(), "Node saw the socket end");
}

#[tokio::test]
async fn websocket_without_idle_timeout_stays_open() {
    let (legacy, mut closed) = ws_legacy().await;
    let proxy = proxy_with(legacy.addr, ProxyOptions::default()).await;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", proxy.addr))
        .await
        .unwrap();
    // Silent for longer than the idle limit of the other tests.
    assert!(
        tokio::time::timeout(PAST_FIRST_BYTE, ws.next())
            .await
            .is_err(),
        "silent socket got a frame or ended"
    );
    ws.send(tungstenite::Message::text("sigue")).await.unwrap();
    match ws.next().await {
        Some(Ok(tungstenite::Message::Text(t))) => assert_eq!(t.as_str(), "sigue"),
        other => panic!("{other:?}"),
    }
    assert!(closed.try_recv().is_err());
}
