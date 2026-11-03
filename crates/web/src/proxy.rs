//! Reverse proxy to the Node legacy, used as the router fallback for every
//! route that has not been migrated to Rust yet. Handles plain HTTP
//! (streaming both directions) and WebSocket upgrades (bidirectional tunnel).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{
        ws::{CloseFrame, Message, WebSocketUpgrade},
        ConnectInfo, FromRequestParts, Request, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::{
    self, client::IntoClientRequest, protocol::frame::coding::CloseCode,
};

/// Max time to establish a connection (TCP, and the WebSocket handshake) to the legacy.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// After one side of a WebSocket tunnel ends, how long the other side gets to finish
/// (e.g. deliver the close handshake) before it is dropped.
const WS_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

/// Close code sent to both ends of a WebSocket tunnel closed for inactivity
/// (RFC 6455 1001, "going away").
const WS_IDLE_CLOSE_CODE: u16 = 1001;
const WS_IDLE_CLOSE_REASON: &str = "idle timeout";

/// Paths exempt from [`ProxyOptions::first_byte_timeout`]: long-lived
/// responses (SSE / long polls) of the legacy. Audited in the legacy
/// (`index.js`, `routes/*.js`, `services/*.js`): the only `text/event-stream`
/// is `GET /api/halloween/stream` (`index.js:560`, `addHalloweenSseClient`);
/// there are no long polls. Matched like an Express app route (case-insensitive,
/// one optional trailing slash), on the raw path and for every method.
/// WebSocket upgrades never use the first-byte timeout (see `ws_idle_timeout`).
const FIRST_BYTE_TIMEOUT_EXEMPT: &[&str] = &["/api/halloween/stream"];

/// Behavior switches for the proxy.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProxyOptions {
    /// Append the TCP peer address to `X-Forwarded-For`. Off by default.
    ///
    /// The legacy runs with `trust proxy 1` and reads the rightmost
    /// `X-Forwarded-For` entry as the client IP. The chain is
    /// cloudflared -> Rust -> Node, so cloudflared's entry (the real visitor)
    /// must stay rightmost: appending the peer (cloudflared's loopback address)
    /// would make `req.ip` in Node wrong. By default the header is forwarded
    /// untouched (byte-for-byte) and never injected when absent.
    pub append_peer_to_xff: bool,
    /// Max time from sending a proxied HTTP request until the response HEAD
    /// (status and headers) of the legacy arrives; on expiry the client gets
    /// `504 {"error":"Servicio no disponible"}`. It includes uploading the
    /// request body when Node reads it before answering. Once the head has
    /// arrived the body streams with no limit. Never applies to
    /// [`FIRST_BYTE_TIMEOUT_EXEMPT`] nor to WebSocket upgrades. `None` = no limit.
    pub first_byte_timeout: Option<Duration>,
    /// Close a proxied WebSocket (both ends, close frame 1001) after this long
    /// without any frame in either direction (ping/pong and close included).
    /// `None` = never.
    pub ws_idle_timeout: Option<Duration>,
}

/// Whether `path` is one of [`FIRST_BYTE_TIMEOUT_EXEMPT`].
fn first_byte_exempt(path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    FIRST_BYTE_TIMEOUT_EXEMPT
        .iter()
        .any(|p| p.eq_ignore_ascii_case(path))
}

#[derive(Clone)]
struct ProxyState {
    options: ProxyOptions,
    /// Path of the legacy base URL without trailing slash ("" for a bare origin).
    base_path: Arc<str>,
    /// Legacy base URL without trailing slash, e.g. `http://127.0.0.1:3001`.
    legacy: Arc<str>,
    client: reqwest::Client,
}

/// Builds a router that reverse-proxies every request to `legacy_url`.
///
/// Mount it with `Router::fallback_service(fallback_proxy(url))`. For the
/// `X-Forwarded-For` header to carry the peer address the outer app must be
/// served with `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn fallback_proxy(legacy_url: String, options: ProxyOptions) -> Router {
    let client = reqwest::Client::builder()
        // 3xx from Node must reach the browser untouched.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        // Deliberately NO overall/read timeout: long polls and SSE streams
        // (e.g. /api/halloween/stream) must be able to stay open indefinitely.
        // Bodies are passed through as-is (no gzip/brotli features enabled, so
        // Content-Encoding is never decoded or re-encoded).
        .no_proxy()
        .build()
        .expect("reqwest client");
    let base_path: Arc<str> = reqwest::Url::parse(&legacy_url)
        .map(|u| u.path().trim_end_matches('/').to_owned())
        .unwrap_or_default()
        .into();
    let state = ProxyState {
        options,
        base_path,
        legacy: legacy_url.trim_end_matches('/').into(),
        client,
    };
    Router::new().fallback(handle).with_state(state)
}

async fn handle(State(state): State<ProxyState>, req: Request) -> Response {
    if is_websocket_upgrade(req.headers()) {
        proxy_websocket(state, req).await
    } else {
        proxy_http(state, req).await
    }
}

fn bad_gateway() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"Servicio no disponible"}"#,
    )
        .into_response()
}

fn gateway_timeout() -> Response {
    (
        StatusCode::GATEWAY_TIMEOUT,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"Servicio no disponible"}"#,
    )
        .into_response()
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "proxy-connection"
    ) || name.as_str().starts_with("proxy-")
}

/// Removes hop-by-hop headers (RFC 9110 7.6.1), including any header named in `Connection`.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .collect();
    let doomed: Vec<HeaderName> = headers
        .keys()
        .filter(|k| is_hop_by_hop(k) || listed.contains(k))
        .cloned()
        .collect();
    for name in doomed {
        headers.remove(name);
    }
}

fn client_ip(req: &Request) -> Option<std::net::IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

/// Adds `X-Forwarded-Proto` / `X-Forwarded-Host` only if absent and, when
/// `append_peer` is set, appends the peer to `X-Forwarded-For` (raw bytes, so
/// non-UTF-8 values survive). Otherwise `X-Forwarded-For` is left untouched.
fn add_forwarded_headers(
    headers: &mut HeaderMap,
    peer: Option<std::net::IpAddr>,
    host: Option<HeaderValue>,
    append_peer: bool,
) {
    if let (true, Some(ip)) = (append_peer, peer) {
        let mut joined: Vec<u8> = Vec::new();
        for v in headers.get_all(X_FORWARDED_FOR) {
            joined.extend_from_slice(v.as_bytes());
            joined.extend_from_slice(b", ");
        }
        joined.extend_from_slice(ip.to_string().as_bytes());
        if let Ok(v) = HeaderValue::from_bytes(&joined) {
            headers.insert(X_FORWARDED_FOR, v);
        }
    }
    if !headers.contains_key(X_FORWARDED_PROTO) {
        headers.insert(X_FORWARDED_PROTO, HeaderValue::from_static("http"));
    }
    if !headers.contains_key(X_FORWARDED_HOST) {
        if let Some(host) = host {
            headers.insert(X_FORWARDED_HOST, host);
        }
    }
}

/// Log line for a rejected target: method and PATH only. The query string is
/// never logged (it can carry OAuth codes or tokens); `{:?}` escapes the path.
fn invalid_path_log(method: &axum::http::Method, pq: &axum::http::uri::PathAndQuery) -> String {
    format!(
        "proxy rejected a target the url crate would rewrite: {method} {:?}",
        pq.path()
    )
}

fn invalid_path(method: &axum::http::Method, pq: &axum::http::uri::PathAndQuery) -> Response {
    tracing::warn!("{}", invalid_path_log(method, pq));
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"Ruta no válida"}"#,
    )
        .into_response()
}

/// Builds the upstream URL and verifies the `url` crate did not alter the
/// request target (dot-segment resolution, re-encoding). A mismatch would let
/// Rust and Node disagree on the path, so such requests are rejected.
/// An empty query (`/x?`) is preserved as `?` on both sides.
fn upstream_url(state: &ProxyState, pq: &axum::http::uri::PathAndQuery) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(&format!("{}{}", state.legacy, pq.as_str())).ok()?;
    let path_ok = url.path() == format!("{}{}", state.base_path, pq.path());
    (path_ok && url.query() == pq.query()).then_some(url)
}

async fn proxy_http(state: ProxyState, req: Request) -> Response {
    let peer = client_ip(&req);
    let (parts, body) = req.into_parts();
    let pq = parts
        .uri
        .path_and_query()
        .cloned()
        .unwrap_or_else(|| axum::http::uri::PathAndQuery::from_static("/"));
    let Some(url) = upstream_url(&state, &pq) else {
        return invalid_path(&parts.method, &pq);
    };

    // Decide before stripping (Transfer-Encoding is hop-by-hop): send a body
    // only when the client announced one, otherwise reqwest would add
    // `Transfer-Encoding: chunked` to bodiless requests.
    let has_body = parts.headers.contains_key(header::TRANSFER_ENCODING)
        || parts
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim() != "0");

    // The original Host is kept so the legacy sees what the browser asked for.
    let host = parts.headers.get(header::HOST).cloned();
    let mut headers = parts.headers;
    strip_hop_by_hop(&mut headers);
    add_forwarded_headers(&mut headers, peer, host, state.options.append_peer_to_xff);

    let method = parts.method.clone();
    let mut builder = state.client.request(parts.method, url).headers(headers);
    if has_body {
        // Streamed, never buffered in memory.
        builder = builder.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }
    // `send` resolves as soon as the response head arrives: the limit covers
    // the time to first byte, never the body.
    let sent = match state.options.first_byte_timeout {
        Some(limit) if !first_byte_exempt(pq.path()) => {
            match tokio::time::timeout(limit, builder.send()).await {
                Ok(sent) => sent,
                Err(_) => {
                    // Method and path only: the query may carry tokens.
                    tracing::warn!(
                        "proxy first-byte timeout ({}s): {method} {:?}",
                        limit.as_secs(),
                        pq.path()
                    );
                    return gateway_timeout();
                }
            }
        }
        _ => builder.send().await,
    };
    let upstream = match sent {
        Ok(res) => res,
        Err(_) => return bad_gateway(),
    };

    let mut response = Response::builder().status(upstream.status());
    {
        let out = response.headers_mut().expect("fresh builder");
        // `append` keeps repeated headers such as multiple Set-Cookie.
        for (name, value) in upstream.headers() {
            out.append(name.clone(), value.clone());
        }
        strip_hop_by_hop(out);
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| bad_gateway())
}

fn ws_url(legacy: &str, path_and_query: &str) -> String {
    let base = if let Some(rest) = legacy.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = legacy.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        legacy.to_owned()
    };
    format!("{base}{path_and_query}")
}

async fn proxy_websocket(state: ProxyState, req: Request) -> Response {
    let peer = client_ip(&req);
    let (mut parts, _body) = req.into_parts();

    let pq = parts
        .uri
        .path_and_query()
        .cloned()
        .unwrap_or_else(|| axum::http::uri::PathAndQuery::from_static("/"));
    if upstream_url(&state, &pq).is_none() {
        return invalid_path(&parts.method, &pq);
    }
    let url = ws_url(&state.legacy, pq.as_str());

    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };

    let Ok(mut upstream_req) = url.into_client_request() else {
        return bad_gateway();
    };
    {
        let out = upstream_req.headers_mut();
        for name in [
            header::COOKIE,
            header::AUTHORIZATION,
            header::ORIGIN,
            header::SEC_WEBSOCKET_PROTOCOL,
            header::USER_AGENT,
            X_FORWARDED_PROTO,
            X_FORWARDED_HOST,
        ] {
            for value in parts.headers.get_all(&name) {
                out.append(name.clone(), value.clone());
            }
        }
        for value in parts.headers.get_all(X_FORWARDED_FOR) {
            out.append(X_FORWARDED_FOR, value.clone());
        }
        let host = parts.headers.get(header::HOST).cloned();
        add_forwarded_headers(out, peer, host, state.options.append_peer_to_xff);
    }

    // Connect upstream BEFORE accepting the client upgrade, so a dead legacy
    // yields a clean 502 instead of an accepted-then-dropped socket.
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(upstream_req),
    )
    .await;
    let (upstream, upstream_res) = match connected {
        Ok(Ok(ok)) => ok,
        _ => return bad_gateway(),
    };

    // Echo back whichever subprotocol the legacy selected.
    let ws = match upstream_res
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
    {
        Some(proto) => ws.protocols([proto.to_owned()]),
        None => ws,
    };
    let idle = state.options.ws_idle_timeout;
    ws.on_upgrade(move |client| async move {
        let (client_tx, client_rx) = client.split();
        let (up_tx, up_rx) = upstream.split();
        tunnel(client_tx, client_rx, up_tx, up_rx, idle).await;
    })
}

fn to_upstream(msg: Message) -> Option<tungstenite::Message> {
    Some(match msg {
        Message::Text(t) => tungstenite::Message::text(t.as_str()),
        Message::Binary(b) => tungstenite::Message::binary(b),
        Message::Ping(b) => tungstenite::Message::Ping(b),
        Message::Pong(b) => tungstenite::Message::Pong(b),
        Message::Close(frame) => {
            tungstenite::Message::Close(frame.map(|f| tungstenite::protocol::CloseFrame {
                code: CloseCode::from(f.code),
                reason: f.reason.as_str().into(),
            }))
        }
    })
}

fn to_client(msg: tungstenite::Message) -> Option<Message> {
    Some(match msg {
        tungstenite::Message::Text(t) => Message::text(t.as_str()),
        tungstenite::Message::Binary(b) => Message::Binary(b),
        tungstenite::Message::Ping(b) => Message::Ping(b),
        tungstenite::Message::Pong(b) => Message::Pong(b),
        tungstenite::Message::Close(frame) => Message::Close(frame.map(|f| CloseFrame {
            code: u16::from(f.code),
            reason: f.reason.as_str().into(),
        })),
        tungstenite::Message::Frame(_) => return None,
    })
}

/// Completes once `idle` passes without a `notify_one` on `activity`.
async fn idle_expired(activity: &Notify, idle: Duration) {
    // `notify_one` stores a permit when nobody waits, so activity between two
    // iterations is never lost.
    while tokio::time::timeout(idle, activity.notified())
        .await
        .is_ok()
    {}
}

/// Pumps frames both ways until either side closes. Runs inside a single task:
/// when one direction ends the other gets a short grace period (to deliver the
/// close handshake) and is then dropped, so nothing outlives the tunnel.
///
/// With `idle`, every frame received from either side (data, ping, pong,
/// close) restarts the timer; if it runs out both ends get a close frame
/// ([`WS_IDLE_CLOSE_CODE`]) and the tunnel ends.
///
/// Generic over the halves so the timing can be tested without sockets.
async fn tunnel<CTx, CRx, CErr, UTx, URx, UErr>(
    mut client_tx: CTx,
    mut client_rx: CRx,
    mut up_tx: UTx,
    mut up_rx: URx,
    idle: Option<Duration>,
) where
    CTx: futures_util::Sink<Message> + Unpin,
    CRx: futures_util::Stream<Item = Result<Message, CErr>> + Unpin,
    UTx: futures_util::Sink<tungstenite::Message> + Unpin,
    URx: futures_util::Stream<Item = Result<tungstenite::Message, UErr>> + Unpin,
{
    let activity = Notify::new();
    let idle_fired = {
        let client_to_up = async {
            while let Some(Ok(msg)) = client_rx.next().await {
                activity.notify_one();
                let is_close = matches!(msg, Message::Close(_));
                if let Some(m) = to_upstream(msg) {
                    if up_tx.send(m).await.is_err() {
                        break;
                    }
                }
                if is_close {
                    break;
                }
            }
            let _ = up_tx.close().await;
        };
        let up_to_client = async {
            while let Some(Ok(msg)) = up_rx.next().await {
                activity.notify_one();
                let is_close = matches!(msg, tungstenite::Message::Close(_));
                if let Some(m) = to_client(msg) {
                    if client_tx.send(m).await.is_err() {
                        break;
                    }
                }
                if is_close {
                    break;
                }
            }
            let _ = client_tx.close().await;
        };
        let idle_timer = async {
            match idle {
                Some(idle) => idle_expired(&activity, idle).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(client_to_up, up_to_client, idle_timer);

        tokio::select! {
            _ = &mut client_to_up => {
                let _ = tokio::time::timeout(WS_DRAIN_TIMEOUT, &mut up_to_client).await;
                false
            }
            _ = &mut up_to_client => {
                let _ = tokio::time::timeout(WS_DRAIN_TIMEOUT, &mut client_to_up).await;
                false
            }
            _ = &mut idle_timer => true,
        }
    };
    if idle_fired {
        let close_client = async {
            let frame = CloseFrame {
                code: WS_IDLE_CLOSE_CODE,
                reason: WS_IDLE_CLOSE_REASON.into(),
            };
            let _ = client_tx.send(Message::Close(Some(frame))).await;
            let _ = client_tx.close().await;
        };
        let close_upstream = async {
            let frame = tungstenite::protocol::CloseFrame {
                code: CloseCode::from(WS_IDLE_CLOSE_CODE),
                reason: WS_IDLE_CLOSE_REASON.into(),
            };
            let _ = up_tx.send(tungstenite::Message::Close(Some(frame))).await;
            let _ = up_tx.close().await;
        };
        let _ = tokio::time::timeout(
            WS_DRAIN_TIMEOUT,
            futures_util::future::join(close_client, close_upstream),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        first_byte_exempt, invalid_path_log, tunnel, FIRST_BYTE_TIMEOUT_EXEMPT, WS_IDLE_CLOSE_CODE,
    };
    use axum::extract::ws::Message;
    use axum::http::{uri::PathAndQuery, Method};
    use futures_util::{Sink, Stream};
    use std::pin::Pin;
    use std::time::Duration;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite;

    #[test]
    fn every_exempt_path_is_exempt_like_an_express_route() {
        for path in FIRST_BYTE_TIMEOUT_EXEMPT {
            assert!(first_byte_exempt(path), "{path}");
            assert!(first_byte_exempt(&format!("{path}/")), "{path}/");
            assert!(first_byte_exempt(&path.to_uppercase()), "{path} upper");
            assert!(!first_byte_exempt(&format!("{path}//")), "{path}//");
            assert!(!first_byte_exempt(&format!("{path}x")), "{path}x");
            assert!(!first_byte_exempt(&format!("{path}/x")), "{path}/x");
            assert!(!first_byte_exempt(&format!("/{path}")), "/{path}");
        }
    }

    #[test]
    fn halloween_stream_is_the_exempt_route() {
        assert_eq!(FIRST_BYTE_TIMEOUT_EXEMPT, &["/api/halloween/stream"]);
        assert!(first_byte_exempt("/API/Halloween/Stream"));
        assert!(!first_byte_exempt("/api/halloween/status"));
        assert!(!first_byte_exempt("/api/halloween/%73tream"));
        assert!(!first_byte_exempt("/"));
    }

    type BoxSink<T> = Pin<Box<dyn Sink<T, Error = ()> + Send>>;
    type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, ()>> + Send>>;

    fn sink<T: Send + 'static>(tx: UnboundedSender<T>) -> BoxSink<T> {
        Box::pin(futures_util::sink::unfold(tx, |tx, item| async move {
            tx.send(item).map_err(|_| ())?;
            Ok(tx)
        }))
    }

    fn stream<T: Send + 'static>(rx: UnboundedReceiver<T>) -> BoxStream<T> {
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|m| (Ok(m), rx))
        }))
    }

    /// A tunnel over channels. `client_in`/`up_in` feed the tunnel (what the
    /// browser / Node send); `client_out`/`up_out` collect what it forwards.
    struct FakeTunnel {
        client_in: UnboundedSender<Message>,
        up_in: UnboundedSender<tungstenite::Message>,
        client_out: UnboundedReceiver<Message>,
        up_out: UnboundedReceiver<tungstenite::Message>,
        task: JoinHandle<()>,
    }

    fn fake_tunnel(idle: Option<Duration>) -> FakeTunnel {
        let (client_in, client_rx) = unbounded_channel();
        let (up_in, up_rx) = unbounded_channel();
        let (client_tx, client_out) = unbounded_channel();
        let (up_tx, up_out) = unbounded_channel();
        let task = tokio::spawn(tunnel(
            sink(client_tx),
            stream(client_rx),
            sink(up_tx),
            stream(up_rx),
            idle,
        ));
        FakeTunnel {
            client_in,
            up_in,
            client_out,
            up_out,
            task,
        }
    }

    fn drain<T>(rx: &mut UnboundedReceiver<T>) -> Vec<T> {
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push(m);
        }
        out
    }

    fn assert_idle_close(client: &[Message], upstream: &[tungstenite::Message]) {
        match client.last() {
            Some(Message::Close(Some(f))) => assert_eq!(f.code, WS_IDLE_CLOSE_CODE),
            other => panic!("client did not get the idle close frame: {other:?}"),
        }
        match upstream.last() {
            Some(tungstenite::Message::Close(Some(f))) => {
                assert_eq!(u16::from(f.code), WS_IDLE_CLOSE_CODE)
            }
            other => panic!("upstream did not get the idle close frame: {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn idle_tunnel_sends_close_to_both_ends_after_exactly_the_timeout() {
        let idle = Duration::from_secs(30);
        let mut t = fake_tunnel(Some(idle));
        let started = tokio::time::Instant::now();
        (&mut t.task).await.unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed >= idle && elapsed < idle + Duration::from_secs(1),
            "{elapsed:?}"
        );
        assert_idle_close(&drain(&mut t.client_out), &drain(&mut t.up_out));
    }

    #[tokio::test(start_paused = true)]
    async fn every_kind_of_frame_in_either_direction_resets_the_idle_timer() {
        let idle = Duration::from_secs(30);
        let mut t = fake_tunnel(Some(idle));
        let step = Duration::from_secs(20);
        // `Ok`: frame from the browser; `Err`: frame from Node.
        let frames: Vec<Result<Message, tungstenite::Message>> = vec![
            Ok(Message::text("hola")),
            Err(tungstenite::Message::text("hola")),
            Ok(Message::Ping(vec![1].into())),
            Ok(Message::Pong(vec![2].into())),
            Err(tungstenite::Message::Ping(vec![3].into())),
            Err(tungstenite::Message::Pong(vec![4].into())),
            Ok(Message::Binary(vec![5].into())),
        ];
        for frame in frames {
            tokio::time::sleep(step).await;
            match frame {
                Ok(m) => t.client_in.send(m).unwrap(),
                Err(m) => t.up_in.send(m).unwrap(),
            }
        }
        // 7 x 20 s = 140 s, far beyond 30 s: still open.
        tokio::time::sleep(step).await;
        assert!(!t.task.is_finished(), "active tunnel was closed");
        assert!(drain(&mut t.client_out)
            .iter()
            .all(|m| !matches!(m, Message::Close(_))));
        assert!(drain(&mut t.up_out)
            .iter()
            .all(|m| !matches!(m, tungstenite::Message::Close(_))));

        // Silence from now on: closes 30 s after the last frame (20 s already passed).
        let quiet = tokio::time::Instant::now();
        (&mut t.task).await.unwrap();
        let elapsed = quiet.elapsed();
        assert!(
            elapsed >= Duration::from_secs(10) && elapsed < Duration::from_secs(11),
            "{elapsed:?}"
        );
        assert_idle_close(&drain(&mut t.client_out), &drain(&mut t.up_out));
    }

    #[tokio::test(start_paused = true)]
    async fn without_idle_timeout_a_silent_tunnel_stays_open() {
        let t = fake_tunnel(None);
        tokio::time::sleep(Duration::from_secs(365 * 24 * 3600)).await;
        assert!(!t.task.is_finished());
        // A close from the browser still ends it normally.
        t.client_in.send(Message::Close(None)).unwrap();
        let mut t = t;
        (&mut t.task).await.unwrap();
        assert!(matches!(
            drain(&mut t.up_out).last(),
            Some(tungstenite::Message::Close(None))
        ));
    }

    #[test]
    fn invalid_path_log_has_method_and_path_but_never_the_query() {
        let pq = PathAndQuery::from_static("/a/%2e%2e/admin?code=SECRET-OAUTH&state=x");
        let line = invalid_path_log(&Method::GET, &pq);
        assert!(line.contains("GET"), "{line}");
        assert!(line.contains("/a/%2e%2e/admin"), "{line}");
        assert!(!line.contains("SECRET"), "{line}");
        assert!(!line.contains("code="), "{line}");
        assert!(!line.contains('?'), "{line}");
    }
}
