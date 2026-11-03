use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocketUpgrade},
        Request,
    },
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{any, get, head, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

/// A server running on an ephemeral port; aborted on drop.
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

/// Address on which nothing listens.
async fn dead_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}

async fn proxy_to(legacy: SocketAddr) -> TestServer {
    serve(web::app_with_proxy(format!("http://{legacy}"))).await
}

fn dump(method: &str, uri: &str, headers: &HeaderMap) -> String {
    let mut out = format!("{method} {uri}\n");
    for (k, v) in headers {
        out.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
    }
    out
}

async fn echo(req: Request) -> String {
    dump(
        req.method().as_str(),
        &req.uri().path_and_query().unwrap().to_string(),
        req.headers(),
    )
}

async fn ws_echo(ws: WebSocketUpgrade) -> Response {
    ws.protocols(["chat"]).on_upgrade(|mut socket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            match msg {
                Message::Text(_) | Message::Binary(_) => {
                    if socket.send(msg).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    })
}

async fn count_body(req: Request) -> String {
    let mut stream = req.into_body().into_data_stream();
    let mut total = 0usize;
    while let Some(chunk) = stream.next().await {
        total += chunk.unwrap().len();
    }
    total.to_string()
}

fn legacy_app() -> Router {
    Router::new()
        .route("/echo", any(echo))
        .route(
            "/create",
            post(|body: String| async move { (StatusCode::CREATED, format!("got:{body}")) }),
        )
        .route(
            "/cookies",
            get(|| async {
                let mut h = HeaderMap::new();
                h.append(header::SET_COOKIE, "a=1; Path=/".parse().unwrap());
                h.append(header::SET_COOKIE, "b=2; Path=/; HttpOnly".parse().unwrap());
                (h, "ok")
            }),
        )
        .route(
            "/redirect",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, "/elsewhere")],
                    "moved",
                )
            }),
        )
        .route("/nocontent", get(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/headonly",
            head(|| async { ([(header::CONTENT_LENGTH, "1234")], Body::empty()) }),
        )
        .route("/count", post(count_body))
        .route("/ws", get(ws_echo))
}

#[tokio::test]
async fn proxies_get_with_query_and_headers() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let res = client()
        .get(format!("http://{}/echo?a=1&b=dos%20x", proxy.addr))
        .header("x-custom", "hola")
        .header("cookie", "sid=abc")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body = res.text().await.unwrap();
    assert!(body.starts_with("GET /echo?a=1&b=dos%20x\n"), "{body}");
    assert!(body.contains("x-custom: hola\n"), "{body}");
    assert!(body.contains("cookie: sid=abc\n"), "{body}");
    assert!(body.contains("x-forwarded-for: 127.0.0.1\n"), "{body}");
}

#[tokio::test]
async fn proxies_post_body_and_status() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let res = client()
        .post(format!("http://{}/create", proxy.addr))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.text().await.unwrap(), "got:hello");
}

#[tokio::test]
async fn preserves_set_cookie_headers() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let res = client()
        .get(format!("http://{}/cookies", proxy.addr))
        .send()
        .await
        .unwrap();
    let cookies: Vec<_> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(cookies, vec!["a=1; Path=/", "b=2; Path=/; HttpOnly"]);
}

#[tokio::test]
async fn returns_502_json_when_legacy_down() {
    let proxy = proxy_to(dead_addr().await).await;

    let started = Instant::now();
    let res = client()
        .get(format!("http://{}/anything", proxy.addr))
        .send()
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(res.status(), 502);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    assert_eq!(
        res.text().await.unwrap(),
        r#"{"error":"Servicio no disponible"}"#
    );
    assert!(elapsed < Duration::from_millis(2500), "took {elapsed:?}");
}

#[tokio::test]
async fn proxies_websocket_echo() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let mut req = format!("ws://{}/ws?room=1", proxy.addr)
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("sec-websocket-protocol", "chat".parse().unwrap());
    let (mut ws, response) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(
        response.headers().get("sec-websocket-protocol").unwrap(),
        "chat"
    );

    ws.send(tungstenite::Message::text("ping-texto"))
        .await
        .unwrap();
    let got = ws.next().await.unwrap().unwrap();
    assert_eq!(got, tungstenite::Message::text("ping-texto"));

    ws.send(tungstenite::Message::binary(vec![1u8, 2, 3]))
        .await
        .unwrap();
    let got = ws.next().await.unwrap().unwrap();
    assert_eq!(got, tungstenite::Message::binary(vec![1u8, 2, 3]));

    ws.close(None).await.unwrap();
    // The stream must terminate (legacy closes, proxy tears the tunnel down).
    let end = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(m) = ws.next().await {
            if m.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(end.is_ok(), "websocket did not terminate after close");
}

#[tokio::test]
async fn websocket_upgrade_returns_502_when_legacy_down() {
    let proxy = proxy_to(dead_addr().await).await;

    let started = Instant::now();
    let err = tokio_tungstenite::connect_async(format!("ws://{}/ws", proxy.addr))
        .await
        .unwrap_err();
    match err {
        tungstenite::Error::Http(res) => assert_eq!(res.status(), 502),
        other => panic!("expected an HTTP 502 handshake rejection, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_millis(2500));
}

#[tokio::test]
async fn passes_redirects_through_unfollowed() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let res = client()
        .get(format!("http://{}/redirect", proxy.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 302);
    assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/elsewhere");
    assert_eq!(res.text().await.unwrap(), "moved");
}

#[tokio::test]
async fn sets_forwarded_proto_and_host_when_absent() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let body = client()
        .get(format!("http://{}/echo", proxy.addr))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("x-forwarded-proto: http\n"), "{body}");
    assert!(
        body.contains(&format!("x-forwarded-host: {}\n", proxy.addr)),
        "{body}"
    );
}

#[tokio::test]
async fn preserves_existing_forwarded_headers() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let body = client()
        .get(format!("http://{}/echo", proxy.addr))
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-host", "machupeek.com")
        .header("x-forwarded-for", "203.0.113.9")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("x-forwarded-proto: https\n"), "{body}");
    assert!(body.contains("x-forwarded-host: machupeek.com\n"), "{body}");
    assert!(
        body.contains("x-forwarded-for: 203.0.113.9, 127.0.0.1\n"),
        "{body}"
    );
    assert_eq!(body.matches("x-forwarded-proto").count(), 1);
}

#[tokio::test]
async fn streams_large_request_bodies() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;
    let size = 8 * 1024 * 1024;

    // With Content-Length.
    let res = client()
        .post(format!("http://{}/count", proxy.addr))
        .body(vec![7u8; size])
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), size.to_string());

    // Chunked (no Content-Length).
    let chunks = (0..8).map(|_| Ok::<_, std::io::Error>(vec![7u8; 1024 * 1024]));
    let res = client()
        .post(format!("http://{}/count", proxy.addr))
        .body(reqwest::Body::wrap_stream(futures_util::stream::iter(
            chunks,
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), size.to_string());
}

#[tokio::test]
async fn handles_204_and_head_responses() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let res = client()
        .get(format!("http://{}/nocontent", proxy.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(res.bytes().await.unwrap().is_empty());

    let res = client()
        .head(format!("http://{}/headonly", proxy.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers().get(header::CONTENT_LENGTH).unwrap(), "1234");
    assert!(res.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn does_not_forward_hop_by_hop_headers() {
    let legacy = serve(legacy_app()).await;
    let proxy = proxy_to(legacy.addr).await;

    let body = client()
        .get(format!("http://{}/echo", proxy.addr))
        .header("connection", "keep-alive, x-secret")
        .header("keep-alive", "timeout=5")
        .header("x-secret", "1")
        .header("proxy-authorization", "Basic abc")
        .header("te", "trailers")
        .header("x-keep", "yes")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for gone in ["keep-alive:", "x-secret:", "proxy-authorization:", "te:"] {
        assert!(!body.contains(gone), "{gone} leaked: {body}");
    }
    assert!(body.contains("x-keep: yes\n"), "{body}");
}
