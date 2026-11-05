//! Binario `web`: el frente de entrada de machupeek.com.
//!
//! * `web` arranca el servidor (`Config::from_env()`, ensamblado con
//!   [`web::build_app`], apagado ordenado con Ctrl-C / SIGTERM).
//! * `web --healthcheck` hace `GET http://127.0.0.1:$PORT/api/ping` y sale con
//!   0 si responde 200 y 1 en otro caso (lo usa el `HEALTHCHECK` del Dockerfile;
//!   la imagen no trae curl).
//!
//! Códigos de salida: 0 correcto, 1 fallo en tiempo de ejecución (bind,
//! conexión a Firestore, healthcheck), 2 configuración inválida.

use std::future::IntoFuture;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use common::config::{firestore_mode, Config, FirestoreMode};
use data::{ClaimsRepo, FirestoreClient};
use tracing::{error, info, warn};
use web::routes::admin::AdminRepos;
use web::{build_app, AppDeps};

const DEFAULT_PORT: u16 = 15633;
/// Tiempo para cerrar conexiones abiertas tras la señal (docker manda SIGKILL a los 10 s).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(8);
/// Tope para conectar con Firestore al arrancar.
const FIRESTORE_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    if std::env::args().skip(1).any(|a| a == "--healthcheck") {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|p| p.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        return if healthcheck(addr) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    run().await
}

async fn run() -> ExitCode {
    init_tracing();
    for var in [
        "FIRESTORE_EMULATOR_HOST",
        "FIRESTORE_USE_ADC",
        "FIREBASE_SERVICE_ACCOUNT_PATH",
    ] {
        if let Ok(val) = std::env::var(var) {
            if val.trim().is_empty() {
                std::env::remove_var(var);
            }
        }
    }

    // El error nunca contiene secretos: `Missing` nombra la variable e `Invalid`
    // solo se usa con variables no secretas (PORT, BASE_URL, banderas).
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };
    let mode = match firestore_mode(&cfg) {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };

    let client = match connect_firestore(&cfg, &mode).await {
        Ok(client) => client,
        Err(msg) => {
            error!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    log_banner(&cfg, &mode);

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("cannot bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    info!("listening on http://{addr}");

    let admin = client.as_ref().map(AdminRepos::firestore);
    let claims = admin
        .as_ref()
        .map(|a| a.claims.clone() as Arc<dyn ClaimsRepo>);
    let cs2 = Some(Arc::new(cs2::Cs2ServerManager::new(&cfg)));
    let lobby = Some(Arc::new(lobby::LobbyManager::new(&cfg.discord_guild_id)));

    // Arrancar el bot nativo de Discord si hay token configurado
    if let Some(lobby_mgr) = lobby.clone() {
        let bot_cfg = cfg.clone();
        let bot_fs = client.clone();
        tokio::spawn(async move {
            info!("Iniciando bot nativo de Discord integrado...");
            if let Err(e) = bot::start_bot(bot_cfg, bot_fs, lobby_mgr).await {
                error!("El bot de Discord finalizó con error: {:?}", e);
            }
        });
    }

    let docs = client
        .clone()
        .map(|c| Arc::new(c) as Arc<dyn data::DocSink>);
    let app = build_app(AppDeps {
        config: cfg,
        claims,
        admin,
        cs2,
        lobby,
        docs,
    });
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = stop_rx.wait_for(|stop| *stop).await;
    });
    let mut server = tokio::spawn(server.into_future());

    tokio::select! {
        res = &mut server => {
            return match res {
                Ok(Ok(())) => ExitCode::SUCCESS,
                Ok(Err(e)) => { error!("server error: {e}"); ExitCode::FAILURE }
                Err(e) => { error!("server task failed: {e}"); ExitCode::FAILURE }
            };
        }
        () = shutdown_signal() => {}
    }

    info!(
        "shutdown requested; draining connections for up to {}s",
        SHUTDOWN_GRACE.as_secs()
    );
    let _ = stop_tx.send(true);
    match tokio::time::timeout(SHUTDOWN_GRACE, &mut server).await {
        Ok(_) => info!("stopped"),
        // WebSockets y streams SSE proxificados pueden no cerrar nunca solos.
        Err(_) => warn!("grace period expired; closing remaining connections"),
    }
    ExitCode::SUCCESS
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // Sin colores ANSI cuando la salida no es una terminal (docker logs, archivos).
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stdout());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .init();
}

/// Conecta solo en los modos explícitos. `Disabled` no conecta y deja
/// `GET /api/reclamaciones/:code` y las lecturas de `/api/admin` en el proxy.
async fn connect_firestore(
    cfg: &Config,
    mode: &FirestoreMode,
) -> Result<Option<FirestoreClient>, String> {
    let project = match (mode, cfg.firestore_project_id.as_deref()) {
        (FirestoreMode::Disabled, _) | (_, None) => {
            if cfg.firebase_service_account_path.is_some()
                || cfg.firestore_emulator_host.is_some()
                || cfg.firestore_use_adc
            {
                warn!(
                    "Firestore credential variables are set but FIRESTORE_PROJECT_ID is not: \
                     Firestore stays disabled"
                );
            }
            return Ok(None);
        }
        (_, Some(project)) => project,
    };
    let key = match mode {
        FirestoreMode::KeyFile { path } => Some(path.as_path()),
        _ => None,
    };
    info!(
        "connecting to Firestore project {project} (credential mode: {})",
        mode.describe()
    );
    match tokio::time::timeout(
        FIRESTORE_CONNECT_TIMEOUT,
        FirestoreClient::connect(project, key),
    )
    .await
    {
        Ok(Ok(client)) => Ok(Some(client)),
        // DataError solo lleva el mensaje del cliente, nunca el contenido de la clave.
        Ok(Err(e)) => Err(format!(
            "cannot connect to Firestore ({}): {e}",
            mode.describe()
        )),
        Err(_) => Err(format!(
            "timed out connecting to Firestore ({}) after {}s",
            mode.describe(),
            FIRESTORE_CONNECT_TIMEOUT.as_secs()
        )),
    }
}

/// Nunca registra secretos: solo datos de despliegue.
fn log_banner(cfg: &Config, mode: &FirestoreMode) {
    info!(
        port = cfg.port,
        legacy_url = %cfg.legacy_url,
        static_dir = %cfg.static_dir.display(),
        base_url = %cfg.base_url,
        production = cfg.production,
        trust_cf_connecting_ip = cfg.trust_cf_connecting_ip,
        proxy_append_peer_to_xff = cfg.proxy_append_peer_to_xff,
        proxy_first_byte_timeout_secs = cfg.proxy_first_byte_timeout.map_or(0, |d| d.as_secs()),
        ws_idle_timeout_secs = cfg.ws_idle_timeout.map_or(0, |d| d.as_secs()),
        firestore = %mode.describe(),
        "machupeek web front door starting"
    );
    if !cfg.static_dir.is_dir() {
        warn!(
            "STATIC_DIR {} is not a directory: SPA routes will fail",
            cfg.static_dir.display()
        );
    }
    if !cfg.trust_cf_connecting_ip {
        info!("rate limit keyed by the rightmost X-Forwarded-For entry (CF-Connecting-IP ignored)");
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// `GET /api/ping` sobre TCP plano. `X-Forwarded-Proto: https` evita la
/// redirección HTTPS de producción (la petición no sale de la máquina).
fn healthcheck(addr: SocketAddr) -> bool {
    let attempt = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&addr, HEALTHCHECK_TIMEOUT)?;
        stream.set_read_timeout(Some(HEALTHCHECK_TIMEOUT))?;
        stream.set_write_timeout(Some(HEALTHCHECK_TIMEOUT))?;
        write!(
            stream,
            "GET /api/ping HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Forwarded-Proto: https\r\n\
             User-Agent: web-healthcheck\r\nConnection: close\r\n\r\n"
        )?;
        let mut head = [0u8; 13];
        stream.read_exact(&mut head)?;
        // "HTTP/1.1 200 OK" (o "HTTP/1.1 200" + CRLF, sin frase de estado)
        Ok(head.starts_with(b"HTTP/1.")
            && &head[8..12] == b" 200"
            && matches!(head[12], b' ' | 0x0d))
    };
    attempt().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Servidor de un solo uso que contesta `reply` y devuelve lo que recibió.
    fn one_shot(reply: &'static str) -> (SocketAddr, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap();
            s.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (addr, handle)
    }

    #[test]
    fn healthcheck_ok_on_200_and_sends_https_proto() {
        let (addr, h) = one_shot("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        assert!(healthcheck(addr));
        let req = h.join().unwrap();
        assert!(req.starts_with("GET /api/ping HTTP/1.1\r\n"), "{req}");
        assert!(req.contains("X-Forwarded-Proto: https\r\n"), "{req}");
    }

    #[test]
    fn healthcheck_fails_on_non_200() {
        for reply in [
            "HTTP/1.1 502 Bad Gateway\r\n\r\n",
            "HTTP/1.1 301 Moved Permanently\r\n\r\n",
            "HTTP/1.1 2000\r\n\r\n",
            "garbage",
        ] {
            let (addr, h) = one_shot(reply);
            assert!(!healthcheck(addr), "{reply}");
            h.join().unwrap();
        }
    }

    #[test]
    fn healthcheck_fails_when_nothing_listens() {
        let addr = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        assert!(!healthcheck(addr));
    }
}
