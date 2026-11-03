//! End-to-end del frente de entrada: `web::build_app` servido por TCP real con
//! `into_make_service_with_connect_info`, contra un Node simulado en un puerto
//! efímero y un directorio de estáticos temporal. Nada aquí toca Firestore,
//! Discord ni el legado real.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use common::config::Config;
use data::InMemoryClaimsRepo;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite;
use web::{build_app, AppDeps};

const NO_STORE: &str = "no-cache, no-store, must-revalidate";

// ------------------------------------------------------------------ harness

async fn serve(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// Node simulado: registra `MÉTODO ruta?query` y lo devuelve en el cuerpo;
/// `/api/lobby/mixton-ws` es un eco WebSocket (la ruta real de `index.js`).
#[derive(Clone, Default)]
struct Legacy {
    seen: Arc<Mutex<Vec<String>>>,
}

impl Legacy {
    async fn start() -> (Self, SocketAddr) {
        let legacy = Legacy::default();
        let seen = legacy.seen.clone();
        let app = Router::new()
            .route("/api/lobby/mixton-ws", get(ws_echo))
            .fallback(any(move |req: Request| {
                let seen = seen.clone();
                async move {
                    let line = format!(
                        "{} {}",
                        req.method(),
                        req.uri().path_and_query().map_or("/", |pq| pq.as_str())
                    );
                    seen.lock().unwrap().push(line.clone());
                    format!("LEGACY {line}")
                }
            }));
        let addr = serve(app).await;
        (legacy, addr)
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

async fn ws_echo(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            match msg {
                Message::Text(t) => {
                    let reply = format!("LEGACY-WS {}", t.as_str());
                    if socket.send(Message::Text(reply.into())).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    })
    .into_response()
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn static_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "index.html", "<html>SPA-INDEX</html>");
    write(tmp.path(), "terms.html", "<html>TERMS</html>");
    write(tmp.path(), "about.html", "<html>ABOUT</html>");
    tmp
}

fn config(legacy: &str, static_dir: &Path, extra: &[(&str, &str)]) -> Config {
    let mut vars: HashMap<String, String> = [
        ("ADMIN_JWT_SECRET", "e2e-jwt-secret"),
        ("BASE_URL", "http://localhost:15633"),
        ("DISCORD_CLIENT_ID", "123"),
        ("DISCORD_CLIENT_SECRET", "e2e-client-secret"),
        ("DISCORD_TOKEN", "e2e-bot-token"),
        ("DISCORD_GUILD_ID", "456"),
        ("LEGACY_URL", legacy),
        ("STATIC_DIR", static_dir.to_str().unwrap()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    for (k, v) in extra {
        vars.insert((*k).to_owned(), (*v).to_owned());
    }
    Config::from_map(&vars).unwrap()
}

fn claims_repo() -> Arc<InMemoryClaimsRepo> {
    let repo = Arc::new(InMemoryClaimsRepo::new());
    repo.insert(
        "LR-1",
        serde_json::from_value(json!({"claimCode": "LR-1"})).unwrap(),
    );
    repo
}

struct Front {
    addr: SocketAddr,
    legacy: Legacy,
    client: reqwest::Client,
    _static: tempfile::TempDir,
}

impl Front {
    async fn start(with_claims: bool, extra: &[(&str, &str)]) -> Front {
        let (legacy, legacy_addr) = Legacy::start().await;
        let dir = static_dir();
        let cfg = config(&format!("http://{legacy_addr}"), dir.path(), extra);
        let claims = with_claims.then(|| claims_repo() as Arc<dyn data::ClaimsRepo>);
        let addr = serve(build_app(AppDeps {
            config: cfg,
            claims,
        }))
        .await;
        Front {
            addr,
            legacy,
            client: client(),
            _static: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(self.url(path)).send().await.unwrap()
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn header(res: &reqwest::Response, name: &str) -> String {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

async fn json_body(res: reqwest::Response) -> Value {
    serde_json::from_str(&res.text().await.unwrap()).unwrap()
}

// -------------------------------------------------------------- Rust routes

#[tokio::test]
async fn migrated_routes_are_answered_by_rust_and_never_reach_node() {
    let f = Front::start(true, &[]).await;

    let ping = f.get("/api/ping").await;
    assert_eq!(ping.status(), 200);
    assert!(header(&ping, "content-type").starts_with("application/json"));
    assert_eq!(json_body(ping).await["ok"], true);

    let products = f.get("/api/products").await;
    assert_eq!(products.status(), 200);
    let products = json_body(products).await;
    assert_eq!(products["success"], true);
    assert_eq!(products["currency"], "PEN");

    let claim = f.get("/api/reclamaciones/LR-1").await;
    assert_eq!(claim.status(), 200);
    assert_eq!(json_body(claim).await["claim"]["claimCode"], "LR-1");

    let missing = f.get("/api/reclamaciones/NOPE").await;
    assert_eq!(missing.status(), 404);

    let login = f.get("/auth/discord").await;
    assert_eq!(login.status(), 302);
    assert!(header(&login, "location").starts_with("https://discord.com/oauth2/authorize?"));
    assert!(header(&login, "location")
        .contains("redirect_uri=http%3A%2F%2Flocalhost%3A15633%2Fauth%2Fdiscord%2Fcallback"));

    let logout = f.get("/auth/logout").await;
    assert_eq!(logout.status(), 302);
    assert_eq!(header(&logout, "location"), "/");
    assert_eq!(logout.headers().get_all("set-cookie").iter().count(), 2);

    let head = f.client.head(f.url("/api/ping")).send().await.unwrap();
    assert_eq!(head.status(), 200);

    assert!(f.legacy.seen().is_empty(), "{:?}", f.legacy.seen());
}

#[tokio::test]
async fn claim_lookup_stays_on_node_when_firestore_is_disabled() {
    let f = Front::start(false, &[]).await;
    let res = f.get("/api/reclamaciones/LR-1").await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.text().await.unwrap(),
        "LEGACY GET /api/reclamaciones/LR-1"
    );
    // Lo demás de la API pública sigue en Rust.
    assert_eq!(f.get("/api/products").await.status(), 200);
    assert_eq!(f.legacy.seen(), ["GET /api/reclamaciones/LR-1"]);
}

// ------------------------------------------------------------------ proxied

#[tokio::test]
async fn everything_not_migrated_reaches_node() {
    let f = Front::start(true, &[]).await;
    let cases: &[(&str, &str)] = &[
        ("GET", "/auth/discord/callback?code=x&state=/"),
        ("GET", "/auth/steam"),
        ("GET", "/auth/steam/return?openid.mode=x"),
        ("POST", "/auth/demo-login"),
        ("GET", "/health"),
        ("GET", "/api/admin/me"),
        ("GET", "/api/cs2/server-info"),
        ("GET", "/api/lobby/state"),
        ("POST", "/api/reclamaciones"),
        ("POST", "/api/checkout/purchase"),
        ("POST", "/webhook/faceit"),
        ("POST", "/webhooks/yape"),
        // Método equivocado en una ruta migrada: Node, no el 405 de axum.
        ("POST", "/api/ping"),
        ("POST", "/auth/discord"),
        ("DELETE", "/api/reclamaciones/LR-1"),
        ("PUT", "/api/products"),
        ("GET", "/api/no-migrado"),
    ];
    for (method, path) in cases {
        let res = f
            .client
            .request(method.parse().unwrap(), f.url(path))
            .body("")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{method} {path}");
        // Las respuestas proxificadas también llevan las cabeceras de seguridad.
        assert_eq!(header(&res, "x-content-type-options"), "nosniff");
        assert!(res.headers().get("set-cookie").is_none(), "{method} {path}");
        assert_eq!(
            res.text().await.unwrap(),
            format!("LEGACY {method} {path}"),
            "{method} {path}"
        );
    }
    let expected: Vec<String> = cases.iter().map(|(m, p)| format!("{m} {p}")).collect();
    assert_eq!(f.legacy.seen(), expected);
}

#[tokio::test]
async fn lobby_websocket_is_tunneled_to_node() {
    let f = Front::start(true, &[]).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/lobby/mixton-ws", f.addr))
            .await
            .unwrap();
    ws.send(tungstenite::Message::text("hola")).await.unwrap();
    let got = ws.next().await.unwrap().unwrap();
    assert_eq!(got.into_text().unwrap().as_str(), "LEGACY-WS hola");
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn legacy_down_gives_502_json_while_rust_routes_keep_working() {
    // Puerto cerrado: se reserva y se suelta.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let dir = static_dir();
    let cfg = config(&format!("http://{dead}"), dir.path(), &[]);
    let addr = serve(build_app(AppDeps {
        config: cfg,
        claims: None,
    }))
    .await;
    let c = client();

    let res = c
        .get(format!("http://{addr}/api/admin/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);
    assert_eq!(header(&res, "x-content-type-options"), "nosniff");
    assert_eq!(
        json_body(res).await,
        json!({"error": "Servicio no disponible"})
    );

    let ping = c
        .get(format!("http://{addr}/api/ping"))
        .send()
        .await
        .unwrap();
    assert_eq!(ping.status(), 200);
    let spa = c.get(format!("http://{addr}/tienda")).send().await.unwrap();
    assert_eq!(spa.status(), 200);
}

// ------------------------------------------------------------------- static

#[tokio::test]
async fn static_files_and_spa_routes_are_served_by_rust() {
    let f = Front::start(true, &[]).await;

    let file = f.get("/about.html").await;
    assert_eq!(file.status(), 200);
    assert_eq!(header(&file, "cache-control"), NO_STORE);
    assert_eq!(header(&file, "x-frame-options"), "SAMEORIGIN");
    assert_eq!(file.text().await.unwrap(), "<html>ABOUT</html>");

    let spa = f.get("/tienda").await;
    assert_eq!(spa.status(), 200);
    assert_eq!(header(&spa, "cache-control"), NO_STORE);
    assert_eq!(spa.text().await.unwrap(), "<html>SPA-INDEX</html>");

    // `/terms` es una ruta SPA propia con la política del legado (1 h), no no-store.
    let terms = f.get("/terms").await;
    assert_eq!(terms.status(), 200);
    assert_eq!(header(&terms, "cache-control"), "public, max-age=3600");
    assert_eq!(terms.text().await.unwrap(), "<html>TERMS</html>");

    // Un archivo que no existe no es nuestro: Node.
    let missing = f.get("/no-existe.js").await;
    assert_eq!(missing.text().await.unwrap(), "LEGACY GET /no-existe.js");

    assert_eq!(f.legacy.seen(), ["GET /no-existe.js"]);
}

// --------------------------------------------------------------- middleware

#[tokio::test]
async fn security_headers_on_rust_responses() {
    let f = Front::start(true, &[]).await;
    let res = f.get("/api/ping").await;
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "SAMEORIGIN"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        assert_eq!(header(&res, name), value, "{name}");
    }
    assert!(res.headers().get("x-powered-by").is_none());
}

async fn hammer(f: &Front, headers: &[(&str, &str)], n: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut req = f.client.get(f.url("/api/ping"));
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        out.push(req.send().await.unwrap().status().as_u16());
    }
    out
}

/// 100 peticiones pasan; poco después llega un 429 (el cubo GCRA repone una
/// cada 0,6 s, por eso se admite un margen de unas pocas).
async fn assert_limited(f: &Front, headers: &[(&str, &str)]) -> reqwest::Response {
    let first = hammer(f, headers, 100).await;
    assert!(first.iter().all(|s| *s == 200), "{first:?}");
    for _ in 0..10 {
        let mut req = f.client.get(f.url("/api/ping"));
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = req.send().await.unwrap();
        if res.status() == 429 {
            return res;
        }
    }
    panic!("no 429 after 110 requests");
}

#[tokio::test]
async fn rate_limit_keys_on_cf_connecting_ip_when_trusted() {
    let f = Front::start(true, &[("TRUST_CF_CONNECTING_IP", "1")]).await;
    // XFF distinto en cada lote: la clave es CF-Connecting-IP.
    let limited = assert_limited(
        &f,
        &[
            ("cf-connecting-ip", "203.0.113.7"),
            ("x-forwarded-for", "198.51.100.1"),
        ],
    )
    .await;
    assert_eq!(header(&limited, "x-content-type-options"), "nosniff");
    assert!(!header(&limited, "retry-after").is_empty());
    let other = hammer(
        &f,
        &[
            ("cf-connecting-ip", "203.0.113.8"),
            ("x-forwarded-for", "198.51.100.1"),
        ],
        1,
    )
    .await;
    assert_eq!(other, [200]);
    // El 429 también cubre lo proxificado y no llega a Node.
    let res = f
        .client
        .get(f.url("/health"))
        .header("cf-connecting-ip", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 429);
    assert_eq!(header(&res, "x-content-type-options"), "nosniff");
    assert!(f.legacy.seen().is_empty());
}

#[tokio::test]
async fn rate_limit_keys_on_rightmost_xff_when_cf_is_not_trusted() {
    let f = Front::start(true, &[]).await;
    // CF-Connecting-IP se ignora (falsificable si el origen fuese alcanzable).
    assert_limited(
        &f,
        &[
            ("cf-connecting-ip", "203.0.113.7"),
            ("x-forwarded-for", "10.0.0.1, 192.0.2.50"),
        ],
    )
    .await;
    // Misma entrada derecha con otra izquierda y otra CF: mismo cubo.
    let same = hammer(
        &f,
        &[
            ("cf-connecting-ip", "203.0.113.99"),
            ("x-forwarded-for", "10.9.9.9, 192.0.2.50"),
        ],
        1,
    )
    .await;
    assert_eq!(same, [429]);
    // Otra entrada derecha: cubo nuevo.
    let other = hammer(
        &f,
        &[
            ("cf-connecting-ip", "203.0.113.7"),
            ("x-forwarded-for", "192.0.2.50, 192.0.2.51"),
        ],
        1,
    )
    .await;
    assert_eq!(other, [200]);
}

#[tokio::test]
async fn https_redirect_in_production() {
    let f = Front::start(true, &[("BASE_URL", "https://machupeek.example")]).await;
    let res = f
        .client
        .get(f.url("/api/ping?x=1"))
        .header("x-forwarded-proto", "http")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 301);
    assert_eq!(
        header(&res, "location"),
        format!("https://{}/api/ping?x=1", f.addr)
    );
    assert_eq!(header(&res, "x-content-type-options"), "nosniff");

    let ok = f
        .client
        .get(f.url("/api/ping"))
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    // Proxificado tampoco sale sin redirigir.
    let proxied = f
        .client
        .get(f.url("/health"))
        .header("x-forwarded-proto", "http")
        .send()
        .await
        .unwrap();
    assert_eq!(proxied.status(), 301);
    assert!(f.legacy.seen().is_empty());
}
