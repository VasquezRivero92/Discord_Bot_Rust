//! Reverse proxy to the Node legacy, used as the router fallback for every
//! route that has not been migrated to Rust yet. Handles plain HTTP
//! (streaming both directions) and WebSocket upgrades (bidirectional tunnel).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, FromRequestParts, Request, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use futures_util::{SinkExt, StreamExt};
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

fn invalid_path() -> Response {
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
        return invalid_path();
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

    let mut builder = state.client.request(parts.method, url).headers(headers);
    if has_body {
        // Streamed, never buffered in memory.
        builder = builder.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }
    let upstream = match builder.send().await {
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

type UpstreamSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
        return invalid_path();
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
    ws.on_upgrade(move |client| tunnel(client, upstream))
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

/// Pumps frames both ways until either side closes. Runs inside a single task:
/// when one direction ends the other gets a short grace period (to deliver the
/// close handshake) and is then dropped, so nothing outlives the tunnel.
async fn tunnel(client: WebSocket, upstream: UpstreamSocket) {
    let (mut client_tx, mut client_rx) = client.split();
    let (mut up_tx, mut up_rx) = upstream.split();

    let client_to_up = async {
        while let Some(Ok(msg)) = client_rx.next().await {
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
    tokio::pin!(client_to_up, up_to_client);

    tokio::select! {
        _ = &mut client_to_up => {
            let _ = tokio::time::timeout(WS_DRAIN_TIMEOUT, &mut up_to_client).await;
        }
        _ = &mut up_to_client => {
            let _ = tokio::time::timeout(WS_DRAIN_TIMEOUT, &mut client_to_up).await;
        }
    }
}
