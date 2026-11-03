//! CLI: `contrast --node <url> --rust <url> [--jwt-secret <s>] [--only <txt>] <cases_dir>`.
//!
//! Salida: una linea por caso (`PASS nombre` / `FAIL nombre` + diff con
//! sangria) y el resumen `N passed, M failed`. Codigo de salida: 0 todo bien,
//! 1 algun caso difiere, 2 error de uso o de carga.

use std::path::PathBuf;
use std::process::ExitCode;

use contrast::{load_cases, run_case, Ctx};

const USAGE: &str = "usage: contrast --node <url> --rust <url> [--jwt-secret <secret>] [--only <substring>] <cases_dir>\n  the secret may also come from the CONTRAST_JWT_SECRET environment variable";

struct Opts {
    node: String,
    rust: String,
    secret: Option<String>,
    only: Option<String>,
    dir: PathBuf,
}

fn parse_args(args: Vec<String>, env_secret: Option<String>) -> Result<Opts, String> {
    let (mut node, mut rust, mut secret, mut only, mut dir) = (None, None, env_secret, None, None);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().ok_or_else(|| format!("{flag} requires a value"));
        match a.as_str() {
            "--node" => node = Some(value("--node")?),
            "--rust" => rust = Some(value("--rust")?),
            "--jwt-secret" => secret = Some(value("--jwt-secret")?),
            "--only" => only = Some(value("--only")?),
            f if f.starts_with("--") => return Err(format!("unknown flag {f}")),
            _ => {
                if dir.replace(PathBuf::from(&a)).is_some() {
                    return Err("only one cases directory may be given".into());
                }
            }
        }
    }
    let url = |v: Option<String>, flag: &str| {
        let v = v.ok_or_else(|| format!("missing {flag}"))?;
        if v.starts_with("http://") || v.starts_with("https://") {
            Ok(v)
        } else {
            Err(format!("{flag} must be an http:// or https:// URL"))
        }
    };
    Ok(Opts {
        node: url(node, "--node")?,
        rust: url(rust, "--rust")?,
        secret: secret.filter(|s| !s.is_empty()),
        only,
        dir: dir.ok_or("missing cases directory")?,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let opts = match parse_args(
        std::env::args().skip(1).collect(),
        std::env::var("CONTRAST_JWT_SECRET").ok(),
    ) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut cases = match load_cases(&opts.dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(only) = &opts.only {
        cases.retain(|c| c.name.contains(only.as_str()));
    }
    if cases.is_empty() {
        eprintln!("error: no cases to run in {}", opts.dir.display());
        return ExitCode::from(2);
    }

    let ctx = Ctx {
        jwt_secret: opts.secret.map(String::into_bytes),
    };
    let (mut passed, mut failed) = (0usize, 0usize);
    for case in &cases {
        match run_case(&opts.node, &opts.rust, case, &ctx).await {
            Ok(()) => {
                passed += 1;
                println!("PASS {}", case.name);
            }
            Err(d) => {
                failed += 1;
                println!("FAIL {}", case.name);
                for line in d.to_string().lines() {
                    println!("    {line}");
                }
            }
        }
    }
    println!("{passed} passed, {failed} failed");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
