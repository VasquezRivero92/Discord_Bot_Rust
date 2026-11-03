//! Ejecucion de un caso contra Node y Rust.

use std::error::Error as _;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{redirect::Policy, Client, Method};

use crate::{compare, Case, Diff, Observed};

const TIMEOUT: Duration = Duration::from_secs(10);
const JWT_TTL_SECS: u64 = 3600;

/// Contexto de ejecucion: secreto con el que se firma `cookie_jwt`.
#[derive(Clone, Default)]
pub struct Ctx {
    pub jwt_secret: Option<Vec<u8>>,
}

impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field(
                "jwt_secret",
                &self.jwt_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Ejecuta `case` contra `node` y `rust` (URLs base) y compara las respuestas.
///
/// GET/HEAD/OPTIONS corren a la vez; el resto de metodos corren en secuencia
/// (Node primero) porque no son idempotentes: ver la documentacion del crate.
pub async fn run_case(node: &str, rust: &str, case: &Case, ctx: &Ctx) -> Result<(), Diff> {
    let config_err = |msg: String| Diff {
        case: case.name.clone(),
        part: "configuration".into(),
        lines: vec![msg],
    };
    let method = Method::from_bytes(case.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| config_err(format!("invalid method `{}`", case.method)))?;
    let cookie = match &case.cookie_jwt {
        None => None,
        Some(claims) => {
            let secret = ctx.jwt_secret.as_deref().ok_or_else(|| {
                config_err(
                    "case uses cookie_jwt but no JWT secret was provided (--jwt-secret or CONTRAST_JWT_SECRET)"
                        .into(),
                )
            })?;
            let jwt =
                sign(claims, secret).map_err(|e| config_err(format!("cannot sign jwt: {e}")))?;
            Some(format!("auth_token={jwt}; admin_token={jwt}"))
        }
    };

    let idempotent = matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
    let (n, r) = if idempotent {
        tokio::join!(
            fetch(node, case, &method, cookie.as_deref()),
            fetch(rust, case, &method, cookie.as_deref())
        )
    } else {
        let n = fetch(node, case, &method, cookie.as_deref()).await;
        let r = fetch(rust, case, &method, cookie.as_deref()).await;
        (n, r)
    };

    match (n, r) {
        (Ok(n), Ok(r)) => compare(&n, &r, case),
        (n, r) => {
            let mut sides = Vec::new();
            let mut lines = Vec::new();
            for (side, res) in [("node", n), ("rust", r)] {
                if let Err(e) = res {
                    sides.push(side);
                    lines.push(format!("{side}: {e}"));
                }
            }
            Err(Diff {
                case: case.name.clone(),
                part: format!("transport error ({})", sides.join(" and ")),
                lines,
            })
        }
    }
}

fn sign(c: &crate::CaseClaims, secret: &[u8]) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let mut claims = web::session::new_claims(
        c.user_id.clone(),
        c.username.clone(),
        c.avatar.clone(),
        c.role.clone(),
        c.is_admin,
        c.servidor.clone(),
        now,
    );
    claims.exp = now + JWT_TTL_SECS;
    web::session::sign(&claims, secret).map_err(|e| e.to_string())
}

/// Texto del error con su cadena de causas (reqwest oculta el motivo real).
fn describe(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

async fn fetch(
    base: &str,
    case: &Case,
    method: &Method,
    cookie: Option<&str>,
) -> Result<Observed, String> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(TIMEOUT)
        .no_proxy()
        .build()
        .map_err(|e| describe(&e))?;
    let url = format!("{}{}", base.trim_end_matches('/'), case.path);
    let mut req = client.request(method.clone(), url);
    for (k, v) in &case.headers {
        req = req.header(k, v);
    }
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    if let Some(b) = &case.body {
        req = req.body(b.clone());
    }
    let resp = req.send().await.map_err(|e| describe(&e))?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = resp.bytes().await.map_err(|e| describe(&e))?.to_vec();
    Ok(Observed {
        status,
        headers,
        body,
    })
}
