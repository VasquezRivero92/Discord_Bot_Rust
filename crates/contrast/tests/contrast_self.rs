//! Autopruebas del arnes contra dos servidores axum de juguete en puertos
//! efimeros, y del binario `contrast` (codigos de salida).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::Body,
    http::{header, Request, Response, StatusCode},
    Router,
};
use contrast::{run_case, Case, CaseClaims, Ctx};

type Log = Arc<Mutex<Vec<String>>>;

async fn serve(router: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l, router).await.unwrap();
    });
    format!("http://{addr}")
}

fn fixed(status: u16, headers: Vec<(&'static str, &'static str)>, body: &'static str) -> Router {
    Router::new().fallback(move || {
        let headers = headers.clone();
        async move {
            let mut r = Response::builder().status(status);
            for (k, v) in headers {
                r = r.header(k, v);
            }
            r.body(Body::from(body)).unwrap()
        }
    })
}

fn json_router(body: &'static str) -> Router {
    fixed(200, vec![("content-type", "application/json")], body)
}

fn ctx() -> Ctx {
    Ctx { jwt_secret: None }
}

async fn closed_port_url() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

#[tokio::test]
async fn identical_responses_pass() {
    let node = serve(json_router(r#"{"ok":true,"n":[1,2]}"#)).await;
    let rust = serve(json_router(r#"{"n":[1,2],"ok":true}"#)).await;
    let case = Case::get("same", "/api/x?y=1");
    run_case(&node, &rust, &case, &ctx()).await.unwrap();
    // las URLs base toleran la barra final
    run_case(&format!("{node}/"), &format!("{rust}/"), &case, &ctx())
        .await
        .unwrap();
}

#[tokio::test]
async fn status_mismatch_reports_diff() {
    let node = serve(json_router(r#"{"ok":true}"#)).await;
    let rust = serve(fixed(
        404,
        vec![("content-type", "application/json")],
        r#"{"ok":true}"#,
    ))
    .await;
    let d = run_case(&node, &rust, &Case::get("st", "/x"), &ctx())
        .await
        .unwrap_err();
    let s = d.to_string();
    assert!(
        s.contains("st") && s.contains("status") && s.contains("200") && s.contains("404"),
        "{s}"
    );
}

#[tokio::test]
async fn ignored_field_is_not_compared() {
    let node = serve(json_router(r#"{"a":1,"timestamp":111}"#)).await;
    let rust = serve(json_router(r#"{"a":1,"timestamp":222}"#)).await;
    let mut case = Case::get("ts", "/x");
    assert!(run_case(&node, &rust, &case, &ctx()).await.is_err());
    case.ignore_fields = vec!["timestamp".into()];
    run_case(&node, &rust, &case, &ctx()).await.unwrap();
}

#[tokio::test]
async fn extra_json_key_reports_diff() {
    let node = serve(json_router(r#"{"a":1}"#)).await;
    let rust = serve(json_router(r#"{"a":1,"extra":2}"#)).await;
    let d = run_case(&node, &rust, &Case::get("ex", "/x"), &ctx())
        .await
        .unwrap_err();
    assert!(d.to_string().contains("$.extra"), "{d}");
}

#[tokio::test]
async fn redirect_is_not_followed_and_location_compared() {
    let a = fixed(302, vec![("location", "/login")], "");
    let b = fixed(302, vec![("location", "/login")], "");
    let c = fixed(302, vec![("location", "/other")], "");
    let (a, b, c) = (serve(a).await, serve(b).await, serve(c).await);
    let case = Case::get("redir", "/x");
    run_case(&a, &b, &case, &ctx()).await.unwrap();
    let d = run_case(&a, &c, &case, &ctx()).await.unwrap_err();
    assert!(d.to_string().contains("location"), "{d}");
}

#[tokio::test]
async fn set_cookie_attribute_mismatch_over_the_wire() {
    let a = fixed(200, vec![("set-cookie", "t=1; Path=/; HttpOnly")], "");
    let b = fixed(200, vec![("set-cookie", "t=2; Path=/")], "");
    let (a, b) = (serve(a).await, serve(b).await);
    let d = run_case(&a, &b, &Case::get("ck", "/x"), &ctx())
        .await
        .unwrap_err();
    assert!(d.to_string().contains("set-cookie"), "{d}");
}

#[tokio::test]
async fn transport_error_names_the_side() {
    let up = serve(json_router("{}")).await;
    let down = closed_port_url().await;
    let case = Case::get("tr", "/x");
    let d = run_case(&down, &up, &case, &ctx()).await.unwrap_err();
    let s = d.to_string();
    assert!(s.contains("transport") && s.contains("node"), "{s}");
    let d = run_case(&up, &down, &case, &ctx()).await.unwrap_err();
    let s = d.to_string();
    assert!(s.contains("transport") && s.contains("rust"), "{s}");
}

#[tokio::test]
async fn expect_status_enforced() {
    let node = serve(fixed(500, vec![], "x")).await;
    let rust = serve(fixed(500, vec![], "x")).await;
    let mut case = Case::get("exp", "/x");
    run_case(&node, &rust, &case, &ctx()).await.unwrap();
    case.expect_status = Some(200);
    assert!(run_case(&node, &rust, &case, &ctx()).await.is_err());
}

fn claims() -> CaseClaims {
    CaseClaims {
        user_id: "u1".into(),
        username: "tester".into(),
        avatar: Some("av".into()),
        is_admin: true,
        role: "admin".into(),
        servidor: Some("s1".into()),
    }
}

#[tokio::test]
async fn cookie_jwt_without_secret_is_a_clear_error() {
    let node = serve(json_router("{}")).await;
    let mut case = Case::get("jwt", "/x");
    case.cookie_jwt = Some(claims());
    let d = run_case(&node, &node, &case, &ctx()).await.unwrap_err();
    let s = d.to_string();
    assert!(s.contains("secret") && s.contains("jwt"), "{s}");
}

#[tokio::test]
async fn cookie_jwt_is_signed_and_sent_in_both_cookies() {
    let seen: Log = Arc::default();
    let mk = |seen: Log| {
        Router::new().fallback(move |req: Request<Body>| {
            let seen = seen.clone();
            async move {
                let c = req
                    .headers()
                    .get(header::COOKIE)
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                seen.lock().unwrap().push(c);
                Response::new(Body::from("{}"))
            }
        })
    };
    let node = serve(mk(seen.clone())).await;
    let rust = serve(mk(seen.clone())).await;
    let mut case = Case::get("jwt", "/x");
    case.cookie_jwt = Some(claims());
    let ctx = Ctx {
        jwt_secret: Some(b"s3cret".to_vec()),
    };
    run_case(&node, &rust, &case, &ctx).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    for c in &seen {
        let auth = c
            .split("; ")
            .find_map(|p| p.strip_prefix("auth_token="))
            .expect("auth_token");
        assert!(c.contains("; admin_token="), "{c}");
        let cl = web::session::verify(auth, b"s3cret").unwrap();
        assert_eq!(cl.user_id, "u1");
        assert_eq!(cl.role, "admin");
        assert!(cl.is_admin);
        assert_eq!(cl.servidor.as_deref(), Some("s1"));
        assert!(web::session::verify(auth, b"other").is_err());
    }
}

fn logging(name: &'static str, log: Log, delay_ms: u64) -> Router {
    Router::new().fallback(move |req: Request<Body>| {
        let log = log.clone();
        async move {
            let method = req.method().to_string();
            let body = axum::body::to_bytes(req.into_body(), 1 << 20)
                .await
                .unwrap();
            log.lock().unwrap().push(format!("{name}:start:{method}"));
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            log.lock().unwrap().push(format!("{name}:end"));
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body))
                .unwrap()
        }
    })
}

#[tokio::test]
async fn non_get_runs_sequentially_node_first_and_sends_body() {
    let log: Log = Arc::default();
    let node = serve(logging("node", log.clone(), 300)).await;
    let rust = serve(logging("rust", log.clone(), 0)).await;
    let mut case = Case::get("post", "/w");
    case.method = "POST".into();
    case.body = Some("payload".into());
    run_case(&node, &rust, &case, &ctx()).await.unwrap();
    let got = log.lock().unwrap().clone();
    assert_eq!(
        got,
        ["node:start:POST", "node:end", "rust:start:POST", "rust:end"]
    );
}

#[tokio::test]
async fn get_runs_concurrently() {
    let log: Log = Arc::default();
    let node = serve(logging("node", log.clone(), 400)).await;
    let rust = serve(logging("rust", log.clone(), 0)).await;
    run_case(&node, &rust, &Case::get("g", "/r"), &ctx())
        .await
        .unwrap();
    let got = log.lock().unwrap().clone();
    let pos = |s: &str| got.iter().position(|x| x == s).unwrap();
    assert!(pos("rust:end") < pos("node:end"), "{got:?}");
}

// ---------------------------------------------------------------- CLI

struct Cli {
    code: i32,
    out: String,
}

async fn cli(args: Vec<String>, env: Vec<(&'static str, &'static str)>) -> Cli {
    tokio::task::spawn_blocking(move || {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_contrast"));
        c.args(&args).env_remove("CONTRAST_JWT_SECRET");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        Cli {
            code: o.status.code().unwrap(),
            out: format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        }
    })
    .await
    .unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn cases_dir(toml: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("c.toml"), toml).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exit_0_when_all_pass() {
    let node = serve(json_router(r#"{"a":1}"#)).await;
    let rust = serve(json_router(r#"{"a":1}"#)).await;
    let d = cases_dir("[[case]]\nname='uno'\npath='/a'\n[[case]]\nname='dos'\npath='/b'\n");
    let dir = d.path().to_str().unwrap();
    let r = cli(s(&["--node", &node, "--rust", &rust, dir]), vec![]).await;
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        r.out.contains("PASS uno") && r.out.contains("PASS dos"),
        "{}",
        r.out
    );
    assert!(r.out.contains("2 passed, 0 failed"), "{}", r.out);
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exit_1_on_mismatch_and_never_prints_secret() {
    let node = serve(json_router(r#"{"a":1}"#)).await;
    let rust = serve(json_router(r#"{"a":2}"#)).await;
    let d = cases_dir(
        "[[case]]\nname='malo'\npath='/a'\n[case.cookie_jwt]\nuser_id='1'\nusername='u'\n\n[[case]]\nname='otro'\npath='/b'\nignore_fields=['a']\n",
    );
    let dir = d.path().to_str().unwrap();
    let r = cli(
        s(&[
            "--node",
            &node,
            "--rust",
            &rust,
            "--jwt-secret",
            "TOPSECRETVALUE",
            dir,
        ]),
        vec![],
    )
    .await;
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(
        r.out.contains("FAIL malo") && r.out.contains("PASS otro"),
        "{}",
        r.out
    );
    assert!(r.out.contains("1 passed, 1 failed"), "{}", r.out);
    assert!(!r.out.contains("TOPSECRETVALUE"), "{}", r.out);

    // --only filtra por subcadena
    let r = cli(
        s(&["--node", &node, "--rust", &rust, "--only", "otro", dir]),
        vec![],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains("1 passed, 0 failed"), "{}", r.out);
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_secret_from_env() {
    let seen: Log = Arc::default();
    let mk = |seen: Log| {
        Router::new().fallback(move |req: Request<Body>| {
            let seen = seen.clone();
            async move {
                let c = req.headers().get(header::COOKIE).is_some();
                seen.lock().unwrap().push(c.to_string());
                Response::new(Body::from("{}"))
            }
        })
    };
    let node = serve(mk(seen.clone())).await;
    let rust = serve(mk(seen.clone())).await;
    let d =
        cases_dir("[[case]]\nname='j'\npath='/a'\n[case.cookie_jwt]\nuser_id='1'\nusername='u'\n");
    let dir = d.path().to_str().unwrap();
    let r = cli(s(&["--node", &node, "--rust", &rust, dir]), vec![]).await;
    assert_eq!(r.code, 1, "sin secreto debe fallar: {}", r.out);
    assert!(r.out.to_lowercase().contains("secret"), "{}", r.out);
    let r = cli(
        s(&["--node", &node, "--rust", &rust, dir]),
        vec![("CONTRAST_JWT_SECRET", "envsecret")],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(!r.out.contains("envsecret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exit_2_on_bad_usage() {
    let d = cases_dir("[[case]]\nname='a'\npath='/a'\n");
    let dir = d.path().to_str().unwrap();
    let bad = cases_dir("[[case]]\nname='a'\npath='/a'\nbogus=1\n");
    let bad_dir = bad.path().to_str().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let empty_dir = empty.path().to_str().unwrap();
    for args in [
        s(&[]),
        s(&["--node", "http://127.0.0.1:1", dir]),
        s(&["--node", "ftp://x", "--rust", "http://127.0.0.1:1", dir]),
        s(&[
            "--node",
            "http://127.0.0.1:1",
            "--rust",
            "http://127.0.0.1:1",
        ]),
        s(&[
            "--node",
            "http://127.0.0.1:1",
            "--rust",
            "http://127.0.0.1:1",
            "--nope",
            dir,
        ]),
        s(&[
            "--node",
            "http://127.0.0.1:1",
            "--rust",
            "http://127.0.0.1:1",
            "/no/such/dir",
        ]),
        s(&[
            "--node",
            "http://127.0.0.1:1",
            "--rust",
            "http://127.0.0.1:1",
            bad_dir,
        ]),
        s(&[
            "--node",
            "http://127.0.0.1:1",
            "--rust",
            "http://127.0.0.1:1",
            empty_dir,
        ]),
        s(&[
            "--node",
            "http://127.0.0.1:1",
            "--rust",
            "http://127.0.0.1:1",
            "--only",
            "zzz",
            dir,
        ]),
    ] {
        let r = cli(args.clone(), vec![]).await;
        assert_eq!(r.code, 2, "{args:?} -> {}", r.out);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_never_echoes_secrets() {
    let node = serve(json_router(r#"{"a":1}"#)).await;
    let rust = serve(json_router(r#"{"a":1}"#)).await;
    let d = cases_dir("[[case]]\nname='a'\npath='/a'\n");
    let dir = d.path().to_str().unwrap();
    // forma `=`: flag desconocido, sin eco del valor
    let r = cli(
        s(&[
            "--node",
            &node,
            "--rust",
            &rust,
            "--jwt-secret=hunter2",
            dir,
        ]),
        vec![],
    )
    .await;
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(!r.out.contains("hunter2"), "{}", r.out);
    assert!(r.out.contains("--jwt-secret"), "{}", r.out);
    // camino feliz
    let r = cli(
        s(&[
            "--node",
            &node,
            "--rust",
            &rust,
            "--jwt-secret",
            "hunter2",
            dir,
        ]),
        vec![],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(!r.out.contains("hunter2"), "{}", r.out);
    // URL invalida con secreto presente
    let r = cli(
        s(&[
            "--node",
            "ftp://x",
            "--rust",
            &rust,
            "--jwt-secret",
            "hunter2",
            dir,
        ]),
        vec![],
    )
    .await;
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(!r.out.contains("hunter2"), "{}", r.out);
    // flag desconocido larguisimo: nombre truncado, sin valor
    let long = format!("--{}=hunter2", "x".repeat(100));
    let r = cli(s(&["--node", &node, "--rust", &rust, &long, dir]), vec![]).await;
    assert_eq!(r.code, 2);
    assert!(
        !r.out.contains("hunter2") && !r.out.contains(&"x".repeat(50)),
        "{}",
        r.out
    );
}
