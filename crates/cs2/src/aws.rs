//! Cliente AWS EC2 nativo en Rust con firma SigV4 (sin dependencias pesadas de SDK).
//!
//! Implementa las operaciones requeridas por MachuPeek:
//! - `DescribeInstances`: obtener el estado actual (`running`, `stopped`, `pending`, `stopping`).
//! - `StartInstances`: encender la máquina virtual de AWS EC2 en Lima.
//! - `StopInstances`: apagar la máquina para ahorro de costos.
//! - `wait_until_running` / `wait_until_stopped`: polling con timeout configurable.

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum AwsError {
    #[error("credenciales de AWS no configuradas")]
    MissingCredentials,
    #[error("error de red o petición HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("error devuelto por AWS EC2 (código {status}): {body}")]
    Api { status: u16, body: String },
    #[error("tiempo de espera agotado ({0:?}) esperando estado '{1}'")]
    Timeout(Duration, String),
}

/// Estado de una instancia EC2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceState {
    Pending,
    Running,
    ShuttingDown,
    Terminated,
    Stopping,
    Stopped,
    Unknown(String),
}

impl InstanceState {
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "pending" => InstanceState::Pending,
            "running" => InstanceState::Running,
            "shutting-down" => InstanceState::ShuttingDown,
            "terminated" => InstanceState::Terminated,
            "stopping" => InstanceState::Stopping,
            "stopped" => InstanceState::Stopped,
            other => InstanceState::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            InstanceState::Pending => "pending",
            InstanceState::Running => "running",
            InstanceState::ShuttingDown => "shutting-down",
            InstanceState::Terminated => "terminated",
            InstanceState::Stopping => "stopping",
            InstanceState::Stopped => "stopped",
            InstanceState::Unknown(s) => s.as_str(),
        }
    }
}

/// Cliente de AWS EC2 con firma SigV4.
#[derive(Clone)]
pub struct Ec2Client {
    region: String,
    access_key_id: String,
    secret_access_key: String,
    http: reqwest::Client,
}

impl Ec2Client {
    pub fn new(
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        Self {
            region: region.into(),
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Construye el cliente desde la configuración común si las credenciales están presentes.
    pub fn from_config(cfg: &common::config::Config) -> Option<Self> {
        let key = cfg.aws_access_key_id.as_ref()?;
        let secret = cfg.aws_secret_access_key.as_ref()?;
        if key.is_empty() || secret.is_empty() {
            return None;
        }
        Some(Self::new(&cfg.aws_region, key, secret))
    }

    /// Consulta el estado actual de la instancia (equivalente a `getEc2InstanceState`).
    pub async fn get_instance_state(&self, instance_id: &str) -> Result<InstanceState, AwsError> {
        let body = format!("Action=DescribeInstances&Version=2016-11-15&InstanceId.1={instance_id}");
        let response_xml = self.send_action(&body).await?;
        Ok(parse_instance_state_from_xml(&response_xml))
    }

    /// Enciende la instancia EC2.
    pub async fn start_instance(&self, instance_id: &str) -> Result<(), AwsError> {
        let body = format!("Action=StartInstances&Version=2016-11-15&InstanceId.1={instance_id}");
        let _ = self.send_action(&body).await?;
        info!("[AWS EC2] Orden StartInstances enviada para {instance_id}");
        Ok(())
    }

    /// Apaga la instancia EC2 (ahorro de costos).
    pub async fn stop_instance(&self, instance_id: &str) -> Result<(), AwsError> {
        let current = self.get_instance_state(instance_id).await?;
        if current == InstanceState::Stopped || current == InstanceState::Stopping {
            info!(
                "[AWS EC2] La instancia {instance_id} ya está en estado '{}'. No requiere orden de parada.",
                current.as_str()
            );
            return Ok(());
        }
        let body = format!("Action=StopInstances&Version=2016-11-15&InstanceId.1={instance_id}");
        let _ = self.send_action(&body).await?;
        info!("[AWS EC2] Orden StopInstances enviada correctamente a AWS para {instance_id}");
        Ok(())
    }

    /// Asegura que la instancia esté en estado `Running`.
    /// Si está `Stopping`, espera a que termine de apagarse y luego la enciende.
    pub async fn ensure_instance_running(
        &self,
        instance_id: &str,
        timeout: Duration,
    ) -> Result<(), AwsError> {
        let current = self.get_instance_state(instance_id).await?;
        info!(
            "[AWS EC2] Estado actual de la instancia {instance_id}: '{}'",
            current.as_str()
        );

        if current == InstanceState::Running {
            return Ok(());
        }

        if current == InstanceState::Stopping {
            info!("[AWS EC2] La instancia está terminando de apagarse. Esperando antes de encender...");
            self.wait_for_state(instance_id, InstanceState::Stopped, Duration::from_secs(90))
                .await?;
        }

        info!("[AWS EC2] Encendiendo máquina virtual CS2 en Lima ({instance_id})...");
        self.start_instance(instance_id).await?;

        info!("[AWS EC2] Esperando que la máquina virtual alcance estado 'running'...");
        self.wait_for_state(instance_id, InstanceState::Running, timeout)
            .await?;
        info!("[AWS EC2] Máquina virtual en estado RUNNING exitosamente.");
        Ok(())
    }

    /// Espera en sondeo periódico hasta que la instancia alcance el estado deseado.
    pub async fn wait_for_state(
        &self,
        instance_id: &str,
        target: InstanceState,
        max_duration: Duration,
    ) -> Result<(), AwsError> {
        let start = tokio::time::Instant::now();
        let poll_interval = Duration::from_secs(3);

        while start.elapsed() < max_duration {
            match self.get_instance_state(instance_id).await {
                Ok(state) if state == target => return Ok(()),
                Ok(state) => {
                    info!(
                        "[AWS EC2] Esperando estado '{}' para {instance_id} (estado actual: '{}')...",
                        target.as_str(),
                        state.as_str()
                    );
                }
                Err(e) => {
                    warn!("[AWS EC2] Error transitorio consultando estado: {e}");
                }
            }
            tokio::time::sleep(poll_interval).await;
        }

        Err(AwsError::Timeout(max_duration, target.as_str().to_owned()))
    }

    /// Envía una acción a la API Query de AWS EC2 firmada con SigV4.
    async fn send_action(&self, body: &str) -> Result<String, AwsError> {
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        let host = format!("ec2.{}.amazonaws.com", self.region);
        let endpoint = format!("https://{host}/");

        let content_type = "application/x-www-form-urlencoded; charset=utf-8";
        let payload_hash = hex::encode(Sha256::digest(body.as_bytes()));

        let canonical_headers = format!(
            "content-type:{content_type}\nhost:{host}\nx-amz-date:{amz_date}\n"
        );
        let signed_headers = "content-type;host;x-amz-date";

        let canonical_request = format!(
            "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let canonical_request_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));

        let credential_scope = format!("{date_stamp}/{}/ec2/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}"
        );

        let signing_key = get_signature_key(
            &self.secret_access_key,
            &date_stamp,
            &self.region,
            "ec2",
        );
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key_id
        );

        let res = self
            .http
            .post(&endpoint)
            .header("Content-Type", content_type)
            .header("Host", &host)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &auth_header)
            .body(body.to_owned())
            .send()
            .await?;

        let status = res.status();
        let resp_body = res.text().await?;

        if !status.is_success() {
            return Err(AwsError::Api {
                status: status.as_u16(),
                body: resp_body,
            });
        }

        Ok(resp_body)
    }
}

/// Extrae el nombre del estado (`<name>running</name>`) del XML devuelto por EC2.
pub fn parse_instance_state_from_xml(xml: &str) -> InstanceState {
    if let Some(pos) = xml.find("<name>") {
        let rest = &xml[pos + 6..];
        if let Some(end) = rest.find("</name>") {
            return InstanceState::from_str(&rest[..end]);
        }
    }
    InstanceState::Unknown("unknown".to_owned())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn get_signature_key(key: &str, date_stamp: &str, region_name: &str, service_name: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{key}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region_name.as_bytes());
    let k_service = hmac_sha256(&k_region, service_name.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_xml_instance_state_running() {
        let sample = r#"
        <DescribeInstancesResponse xmlns="http://ec2.amazonaws.com/doc/2016-11-15/">
            <reservationSet>
                <item>
                    <instancesSet>
                        <item>
                            <instanceState>
                                <code>16</code>
                                <name>running</name>
                            </instanceState>
                        </item>
                    </instancesSet>
                </item>
            </reservationSet>
        </DescribeInstancesResponse>
        "#;
        assert_eq!(parse_instance_state_from_xml(sample), InstanceState::Running);
    }

    #[test]
    fn parse_xml_instance_state_stopped() {
        let sample = "<instanceState><code>80</code><name>stopped</name></instanceState>";
        assert_eq!(parse_instance_state_from_xml(sample), InstanceState::Stopped);
    }

    #[test]
    fn sigv4_derivation_deterministic() {
        let key = get_signature_key("secret", "20261104", "us-east-1", "ec2");
        assert!(!key.is_empty());
        assert_eq!(key.len(), 32);
    }
}
