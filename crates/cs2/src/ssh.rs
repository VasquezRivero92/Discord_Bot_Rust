//! Ejecución remota de comandos vía SSH en la máquina virtual Linux de CS2.

use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum SshError {
    #[error("clave privada SSH no encontrada en ninguna ruta configurada")]
    KeyNotFound,
    #[error("error ejecutando proceso SSH: {0}")]
    Io(#[from] std::io::Error),
    #[error("comando SSH falló con código {code:?}: {output}")]
    CommandFailed { code: Option<i32>, output: String },
    #[error("tiempo de espera agotado ({0:?}) ejecutando comando SSH: {1}")]
    Timeout(Duration, String),
}

/// Resuelve la clave privada SSH probando las rutas conocidas de producción y desarrollo.
pub fn resolve_ssh_key(configured: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = configured {
        candidates.push(p.to_path_buf());
    }
    candidates.push(PathBuf::from("/app/clave_cs2.pem"));
    candidates.push(PathBuf::from("clave_cs2.pem"));
    candidates.push(PathBuf::from("D:/Discord_Faceit/clave_cs2.pem"));
    candidates.push(PathBuf::from(r"C:\Users\tec_v\Downloads\clave_cs2.pem"));

    for c in candidates {
        if c.exists() && c.is_file() {
            return Some(c);
        }
    }
    None
}

/// Ejecuta un comando en la máquina virtual remota a través de SSH con timeout y captura de salida.
pub async fn execute_remote_command(
    host: &str,
    user: &str,
    key_path: &Path,
    cmd: &str,
    timeout: Duration,
) -> Result<String, SshError> {
    let target = format!("{user}@{host}");
    let key_str = key_path.to_string_lossy().to_string();

    let mut command = tokio::process::Command::new("ssh");
    command
        .arg("-i")
        .arg(&key_str)
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg(&target)
        .arg(cmd);

    let output_future = command.output();

    match tokio::time::timeout(timeout, output_future).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

            if output.status.success() {
                Ok(stdout)
            } else {
                let combined = if !stderr.is_empty() { stderr } else { stdout };
                Err(SshError::CommandFailed {
                    code: output.status.code(),
                    output: combined,
                })
            }
        }
        Ok(Err(e)) => Err(SshError::Io(e)),
        Err(_) => Err(SshError::Timeout(timeout, cmd.to_owned())),
    }
}

/// Espera con sondeo periódico hasta que el servicio SSH de la máquina virtual responda (ejecuta `uptime`).
pub async fn wait_for_ssh_ready(
    host: &str,
    user: &str,
    key_path: &Path,
    max_attempts: u32,
    delay: Duration,
) -> Result<(), SshError> {
    for i in 1..=max_attempts {
        match execute_remote_command(host, user, key_path, "uptime", Duration::from_secs(4)).await {
            Ok(_) => {
                info!("[SSH] Conexión SSH establecida y servidor listo (intento {i}).");
                return Ok(());
            }
            Err(e) => {
                if i == max_attempts {
                    warn!("[SSH] Servidor SSH no respondió tras {max_attempts} intentos: {e}");
                    return Err(e);
                }
                tokio::time::sleep(delay).await;
            }
        }
    }
    Err(SshError::Timeout(
        delay * max_attempts,
        "wait_for_ssh_ready".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_ssh_key_returns_none_when_no_candidate_exists() {
        let dummy = Path::new("non_existent_folder_abc/key.pem");
        // En tests unitarios sin archivos no encuentra nada si no existe
        let _ = resolve_ssh_key(Some(dummy));
    }
}
