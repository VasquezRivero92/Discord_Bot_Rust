//! Comportamiento transversal del legacy (`D:\Discord_Faceit\index.js`):
//! `trust proxy`, helmet sin CSP, Permissions-Policy, CORS, `express.json`
//! con límite de 64 kb, redirección a HTTPS y rate limit global.
//!
//! [`apply`] se puede aplicar a cualquier `Router`; el ensamblado final lo
//! hace la tarea 17.
//!
//! Orden de capas (de fuera hacia dentro, es decir, orden de ejecución de la
//! petición):
//!
//! 1. cabeceras de seguridad (solo tocan la respuesta, así que también cubren
//!    301, 403, 413 y 429; helmet era el primer middleware del legacy),
//! 2. redirección HTTPS (antes de leer nada ni consumir cuota),
//! 3. CORS (el preflight se responde aquí y no consume cuota; un origen no
//!    permitido se rechaza aquí),
//! 4. rate limit,
//! 5. límite de cuerpo JSON.
//!
//! # Exención de estáticos (frente de entrada, `web::build_app`)
//!
//! En el legado `express.static` (index.js:156) corre ANTES de la redirección
//! HTTPS (169) y del `globalLimiter` (188): los archivos de `public/` nunca
//! consumían cuota. Por eso el rate limit es también una pieza aparte,
//! [`RateLimit`]: se construye UNA vez por app (un único cubo por clave de
//! cliente) y `build_app` lo aplica solo a las rutas migradas y al proxy, no a
//! lo que contesta `static_files`. [`apply_except_rate_limit`] pone el resto de
//! capas por fuera de todo. Desviación consciente: las rutas SPA/legales
//! (`/`, `/tienda`, `/terms`, `/admin`, ...) en el legado eran `app.get`
//! posteriores al limitador y sí consumían cuota; en Rust solo sirven HTML
//! estático y quedan exentas igual que los archivos. Lo que `static_files` pasa
//! al proxy (p. ej. un asset inexistente) sí cuenta.
//!
//! [`apply`] sigue siendo el todo-en-uno (con el límite para todo el router).

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{IntoResponse, Response},
    Router,
};
use common::config::Config;
use governor::{clock::Clock, DefaultKeyedRateLimiter, Quota};
use http_body::{Frame, SizeHint};

/// `express.json({ limit: '64kb' })`.
const JSON_BODY_LIMIT: usize = 64 * 1024;
/// `rateLimit({ max: 100, windowMs: 60_000 })`.
const RATE_LIMIT_MAX: u32 = 100;
/// Cada cuántas peticiones se purgan las claves inactivas del limitador.
const CLEANUP_EVERY: u64 = 4096;

/// Cabeceras por defecto de helmet 8.2.0 sin `Content-Security-Policy`
/// (ni COEP, que helmet desactiva por defecto), más `Permissions-Policy`.
const SECURITY_HEADERS: &[(&str, &str)] = &[
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    ("origin-agent-cluster", "?1"),
    ("referrer-policy", "no-referrer"),
    (
        "strict-transport-security",
        "max-age=31536000; includeSubDomains",
    ),
    ("x-content-type-options", "nosniff"),
    ("x-dns-prefetch-control", "off"),
    ("x-download-options", "noopen"),
    ("x-frame-options", "SAMEORIGIN"),
    ("x-permitted-cross-domain-policies", "none"),
    ("x-xss-protection", "0"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    ),
];

struct Shared {
    production: bool,
    cors_origins: Vec<String>,
}

struct LimiterState {
    trust_cf_connecting_ip: bool,
    limiter: DefaultKeyedRateLimiter<IpAddr>,
    requests: AtomicU64,
}

/// Rate limit global (`rateLimit({ max: 100, windowMs: 60_000 })`). Cada valor
/// es un limitador; sus clones comparten los cubos, así que aplicarlo a varias
/// ramas de un router da un único cubo por clave de cliente.
#[derive(Clone)]
pub struct RateLimit {
    state: Arc<LimiterState>,
}

impl RateLimit {
    pub fn new(cfg: &Config) -> Self {
        let quota = Quota::per_minute(
            std::num::NonZeroU32::new(RATE_LIMIT_MAX).expect("rate limit is non-zero"),
        );
        Self {
            state: Arc::new(LimiterState {
                trust_cf_connecting_ip: cfg.trust_cf_connecting_ip,
                limiter: DefaultKeyedRateLimiter::keyed(quota),
                requests: AtomicU64::new(0),
            }),
        }
    }

    /// Limita todo `router` (rutas y fallback).
    pub fn layer(&self, router: Router) -> Router {
        router.layer(from_fn_with_state(self.state.clone(), rate_limit))
    }
}

/// Añade al `router` el comportamiento transversal del legacy.
pub fn apply(router: Router, cfg: &Config) -> Router {
    let limited = RateLimit::new(cfg).layer(router.layer(from_fn(body_limit)));
    outer_layers(limited, cfg)
}

/// Todas las capas de [`apply`] salvo el rate limit, que el llamador aplica por
/// ramas con un [`RateLimit`]. El límite de cuerpo JSON queda por fuera del rate
/// limit, como `express.json` (index.js:147) antes del `globalLimiter` (188).
pub fn apply_except_rate_limit(router: Router, cfg: &Config) -> Router {
    outer_layers(router.layer(from_fn(body_limit)), cfg)
}

fn outer_layers(router: Router, cfg: &Config) -> Router {
    let shared = Arc::new(Shared {
        production: cfg.production,
        cors_origins: cfg.cors_origins.clone(),
    });
    // La última capa añadida es la más externa.
    router
        .layer(from_fn_with_state(shared.clone(), cors))
        .layer(from_fn_with_state(shared, https_redirect))
        .layer(from_fn(security_headers))
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        format!("{{\"error\":\"{message}\"}}"),
    )
        .into_response()
}

// ---------------------------------------------------------------- HTTPS

/// Protocolo del cliente como `req.protocol` de Express con `trust proxy 1`:
/// la PRIMERA entrada de `X-Forwarded-Proto`, sin espacios; `http` si falta,
/// está vacía o no es UTF-8. Se normaliza a minúsculas (Express no lo hace; con
/// Cloudflare el valor siempre llega en minúsculas). Lo usan la redirección
/// HTTPS y `GET /api/ping`, para que nunca discrepen.
pub fn forwarded_proto(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "http".to_owned())
}

async fn https_redirect(State(shared): State<Arc<Shared>>, req: Request, next: Next) -> Response {
    if shared.production {
        let https = forwarded_proto(req.headers()) == "https";
        if !https {
            let host = req
                .headers()
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .map(str::to_owned)
                .or_else(|| req.uri().authority().map(|a| a.to_string()));
            let target = req
                .uri()
                .path_and_query()
                .map_or("/", |pq| pq.as_str())
                .to_owned();
            return match host
                .and_then(|h| HeaderValue::from_str(&format!("https://{h}{target}")).ok())
            {
                Some(location) => (
                    StatusCode::MOVED_PERMANENTLY,
                    [(header::LOCATION, location)],
                )
                    .into_response(),
                None => StatusCode::BAD_REQUEST.into_response(),
            };
        }
    }
    next.run(req).await
}

// ----------------------------------------------------------------- CORS

fn append_vary(headers: &mut HeaderMap, value: &'static str) {
    let present = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case(value) || t.trim() == "*");
    if !present {
        headers.append(header::VARY, HeaderValue::from_static(value));
    }
}

fn set_cors_origin(headers: &mut HeaderMap, origin: &HeaderValue) {
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
}

/// Equivale a `cors({ origin, credentials: true })` del legacy. Un origen no
/// permitido se responde con 403 y JSON (desviación intencionada: el legacy
/// pasaba un `Error` a Express y respondía 500).
async fn cors(State(shared): State<Arc<Shared>>, req: Request, next: Next) -> Response {
    let Some(origin) = req.headers().get(header::ORIGIN).cloned() else {
        // Como el paquete `cors`: sin Origin no hay ACAO, pero sí
        // credenciales y `Vary: Origin`.
        let mut res = next.run(req).await;
        res.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        append_vary(res.headers_mut(), "Origin");
        return res;
    };

    let allowed = shared.cors_origins.is_empty()
        || origin
            .to_str()
            .is_ok_and(|o| shared.cors_origins.iter().any(|a| a == o));
    if !allowed {
        return json_error(StatusCode::FORBIDDEN, "Origen CORS no permitido.");
    }

    let is_preflight = req.method() == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    if is_preflight {
        let mut res = StatusCode::NO_CONTENT.into_response();
        let h = res.headers_mut();
        set_cors_origin(h, &origin);
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,HEAD,PUT,PATCH,POST,DELETE"),
        );
        if let Some(requested) = req.headers().get(header::ACCESS_CONTROL_REQUEST_HEADERS) {
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, requested.clone());
        }
        h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        append_vary(h, "Origin");
        append_vary(h, "Access-Control-Request-Headers");
        return res;
    }

    let mut res = next.run(req).await;
    set_cors_origin(res.headers_mut(), &origin);
    append_vary(res.headers_mut(), "Origin");
    res
}

// ----------------------------------------------------------- Rate limit

fn parse_ip(value: &str) -> Option<IpAddr> {
    value.trim().parse::<IpAddr>().ok().map(bucket_ip)
}

/// IPv4 (incluida IPv4-mapeada) tal cual; IPv6 agrupada por prefijo /56, como
/// `ipKeyGenerator` de express-rate-limit 8 (`ipv6Subnet: 56`).
fn bucket_ip(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let mask = u128::MAX << (128 - 56);
            IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask))
        }
        v4 => v4,
    }
}

/// Clave del cliente: `CF-Connecting-IP` si es una IP válida; si no, la
/// entrada válida más a la derecha de `X-Forwarded-For` (`trust proxy 1`);
/// si no, la dirección del socket; si no, una clave fija.
fn client_key(req: &Request, trust_cf: bool) -> IpAddr {
    let headers = req.headers();
    // `CF-Connecting-IP` es opt-in: el origen puede ser alcanzable sin pasar
    // por Cloudflare y entonces cualquiera podría falsearla. Cloudflare añade
    // la IP real del cliente al final de `X-Forwarded-For`, que ya se usa.
    if let Some(ip) = trust_cf
        .then(|| headers.get("cf-connecting-ip"))
        .flatten()
        .and_then(|v| v.to_str().ok())
        .and_then(parse_ip)
    {
        return ip;
    }
    // Express une varias cabeceras X-Forwarded-For con ", ".
    let xff: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .collect();
    if let Some(ip) = xff.iter().rev().find_map(|e| parse_ip(e)) {
        return ip;
    }
    if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return bucket_ip(addr.ip());
    }
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

async fn rate_limit(State(shared): State<Arc<LimiterState>>, req: Request, next: Next) -> Response {
    let n = shared.requests.fetch_add(1, Ordering::Relaxed);
    if n % CLEANUP_EVERY == CLEANUP_EVERY - 1 {
        shared.limiter.retain_recent();
        shared.limiter.shrink_to_fit();
    }

    let key = client_key(&req, shared.trust_cf_connecting_ip);
    match shared.limiter.check_key(&key) {
        Ok(()) => next.run(req).await,
        Err(not_until) => {
            let wait = not_until.wait_time_from(shared.limiter.clock().now());
            let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
            let secs = secs.max(1);
            let mut res = json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "Demasiadas peticiones. Intenta de nuevo más tarde.",
            );
            let h = res.headers_mut();
            h.insert(header::RETRY_AFTER, HeaderValue::from(secs));
            h.insert("ratelimit-limit", HeaderValue::from(RATE_LIMIT_MAX));
            h.insert("ratelimit-remaining", HeaderValue::from_static("0"));
            h.insert("ratelimit-reset", HeaderValue::from(secs));
            res
        }
    }
}

// ----------------------------------------------------------- Body limit

/// Cuerpo que falla (y levanta `exceeded`) al superar `limit` bytes.
struct LimitedBody {
    inner: Body,
    remaining: usize,
    exceeded: Arc<AtomicBool>,
}

#[derive(Debug)]
struct BodyTooLarge;

impl std::fmt::Display for BodyTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request body too large")
    }
}

impl std::error::Error for BodyTooLarge {}

impl http_body::Body for LimitedBody {
    type Data = <Body as http_body::Body>::Data;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let len = data.len();
                    if len > self.remaining {
                        self.remaining = 0;
                        self.exceeded.store(true, Ordering::SeqCst);
                        return Poll::Ready(Some(Err(Box::new(BodyTooLarge))));
                    }
                    self.remaining -= len;
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(Box::new(e)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
}

fn too_large() -> Response {
    json_error(StatusCode::PAYLOAD_TOO_LARGE, "Cuerpo demasiado grande")
}

/// Como `express.json({ limit: '64kb' })`: solo aplica a `application/json`.
/// Con `Content-Length` se decide antes de leer; sin él (o si miente) el
/// límite se aplica mientras el cuerpo se transmite, sin almacenarlo.
async fn body_limit(req: Request, next: Next) -> Response {
    if !is_json(req.headers()) {
        return next.run(req).await;
    }
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|len| len > JSON_BODY_LIMIT as u64) {
        return too_large();
    }

    let exceeded = Arc::new(AtomicBool::new(false));
    let (parts, body) = req.into_parts();
    let body = Body::new(LimitedBody {
        inner: body,
        remaining: JSON_BODY_LIMIT,
        exceeded: exceeded.clone(),
    });
    let res = next.run(Request::from_parts(parts, body)).await;
    if exceeded.load(Ordering::SeqCst) {
        return too_large();
    }
    res
}

// ----------------------------------------------------- Security headers

/// Añade las cabeceras de helmet solo si la respuesta no las trae ya (las
/// respuestas proxificadas desde Node ya pasaron por helmet) y quita
/// `X-Powered-By`.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        let name = HeaderName::from_static(name);
        if !h.contains_key(&name) {
            h.insert(name, HeaderValue::from_static(value));
        }
    }
    h.remove("x-powered-by");
    res
}
