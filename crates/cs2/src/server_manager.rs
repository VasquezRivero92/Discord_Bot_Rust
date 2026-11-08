//! Gestor central de servidores dedicados de CS2 y orquestación de AWS EC2.

use crate::aws::Ec2Client;
use crate::matchzy::{GeneratedMatchZy, MatchParams};
use crate::ssh::{execute_remote_command, resolve_ssh_key, wait_for_ssh_ready};
use common::config::Config;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub const PORT_POOL: [u16; 5] = [27015, 27016, 27017, 27018, 27019];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveMatch {
    pub match_id: String,
    pub assigned_port: u16,
    pub server_ip: String,
    pub published_at: i64,
    pub map: String,
    pub bet_amount_per_player: f64,
    pub total_pot: f64,
    pub tournament_match_id: Option<String>,
}

#[derive(Clone)]
pub struct Cs2ServerManager {
    server_base_host: String,
    server_ip: String,
    ssh_user: String,
    ssh_key_path: Option<PathBuf>,
    ec2_instance_id: Option<String>,
    ec2_client: Option<Ec2Client>,
    active_matches: Arc<RwLock<HashMap<String, ActiveMatch>>>,
}

impl Cs2ServerManager {
    pub fn new(cfg: &Config) -> Self {
        let base_host = cfg
            .cs2_server_public_ip
            .as_deref()
            .or(cfg.mixton_server_ip.as_deref())
            .unwrap_or(&cfg.cs2_server_ip);

        let clean_host = base_host
            .split(':')
            .next()
            .unwrap_or("96.0.23.220")
            .trim()
            .to_string();

        let primary_server_ip = format!("{clean_host}:27015");

        let key_path = resolve_ssh_key(cfg.cs2_ssh_key_path.as_deref());
        let ec2_client = Ec2Client::from_config(cfg);

        Self {
            server_base_host: clean_host,
            server_ip: primary_server_ip,
            ssh_user: cfg.cs2_ssh_user.clone(),
            ssh_key_path: key_path,
            ec2_instance_id: cfg.aws_ec2_instance_id.clone(),
            ec2_client,
            active_matches: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Sin acceso remoto: ni SSH ni EC2. Para pruebas: `new` encuentra la clave real
    /// (`clave_cs2.pem`) y apunta al servidor de producción, así que una prueba que lance o
    /// configure una partida ejecutaría comandos en él.
    pub fn offline(mut self) -> Self {
        self.ssh_key_path = None;
        self.ec2_client = None;
        self.ec2_instance_id = None;
        self
    }

    pub fn server_ip(&self) -> &str {
        &self.server_ip
    }

    /// Asigna el primer puerto libre disponible en el pool (27015..27019).
    pub async fn allocate_port(&self, match_id: &str) -> Option<u16> {
        let matches = self.active_matches.read().await;
        if let Some(existing) = matches.get(match_id) {
            return Some(existing.assigned_port);
        }
        let busy: std::collections::HashSet<u16> =
            matches.values().map(|m| m.assigned_port).collect();

        for port in PORT_POOL {
            if !busy.contains(&port) {
                return Some(port);
            }
        }
        None
    }

    pub fn get_server_ip_for_port(&self, port: u16) -> String {
        format!("{}:{port}", self.server_base_host)
    }

    /// Publica o registra una partida activa en memoria.
    pub async fn publish_match(
        &self,
        match_id: String,
        port: Option<u16>,
        map: String,
        bet_amount: f64,
        total_players: usize,
        tournament_match_id: Option<String>,
    ) -> ActiveMatch {
        let assigned_port = match port {
            Some(p) if PORT_POOL.contains(&p) => p,
            _ => self.allocate_port(&match_id).await.unwrap_or(27015),
        };

        let active = ActiveMatch {
            server_ip: self.get_server_ip_for_port(assigned_port),
            assigned_port,
            match_id: match_id.clone(),
            published_at: chrono::Utc::now().timestamp_millis(),
            map,
            bet_amount_per_player: bet_amount,
            total_pot: (bet_amount * total_players as f64 * 100.0).round() / 100.0,
            tournament_match_id,
        };

        let mut matches = self.active_matches.write().await;
        matches.insert(match_id.clone(), active.clone());
        info!(
            "[CS2ServerManager] Partida {match_id} registrada en puerto {assigned_port} ({}). Activas: {}",
            active.server_ip,
            matches.len()
        );

        active
    }

    /// Retorna todas las partidas activas en curso.
    pub async fn get_all_active_matches(&self) -> Vec<ActiveMatch> {
        self.active_matches.read().await.values().cloned().collect()
    }

    /// Remueve una partida activa de la memoria.
    pub async fn clear_match(&self, match_id: &str) -> Option<ActiveMatch> {
        let mut matches = self.active_matches.write().await;
        let removed = matches.remove(match_id);
        if let Some(ref m) = removed {
            info!(
                "[CS2ServerManager] Partida {match_id} liberada del puerto {}. Activas restantes: {}",
                m.assigned_port,
                matches.len()
            );
        }
        removed
    }

    /// Inicia el servidor CS2 en el puerto especificado.
    /// Si la máquina de AWS EC2 está apagada, la enciende primero y espera conexión SSH.
    pub async fn start_server(&self, port: u16) -> Result<(), String> {
        info!("[CS2ServerManager] Iniciando proceso de arranque en puerto {port}...");

        // 1. Asegurar máquina virtual en AWS EC2
        let mut ec2_error = None;
        if let (Some(ref ec2), Some(ref inst_id)) = (&self.ec2_client, &self.ec2_instance_id) {
            info!("[CS2ServerManager] Verificando estado de AWS EC2 ({inst_id})...");
            if let Err(e) = ec2
                .ensure_instance_running(inst_id, Duration::from_secs(120))
                .await
            {
                error!("[CS2ServerManager] Error encendiendo instancia EC2: {e}");
                ec2_error = Some(e.to_string());
            }
        }

        // 2. Verificar y esperar SSH
        if let Some(ref key) = self.ssh_key_path {
            if let Err(e) = wait_for_ssh_ready(
                &self.server_base_host,
                &self.ssh_user,
                key,
                15,
                Duration::from_secs(3),
            )
            .await
            {
                // Sin SSH no se puede lanzar tmux: el llamador debe cancelar y devolver la apuesta
                // en vez de dar por iniciada una partida en un servidor apagado.
                warn!("[CS2ServerManager] Espera de SSH reportó: {e}");
                return Err(match ec2_error {
                    Some(ec2) => format!("la instancia EC2 no encendió ({ec2}) y SSH no responde ({e})"),
                    None => format!("SSH no responde: {e}"),
                });
            }

            // 3. Lanzar sesión tmux en Linux
            let tmux_cmd = format!(
                "tmux new-session -d -s cs2_{port} '/home/ubuntu/run_server.sh {port} de_mirage' || true"
            );
            match execute_remote_command(
                &self.server_base_host,
                &self.ssh_user,
                key,
                &tmux_cmd,
                Duration::from_secs(10),
            )
            .await
            {
                Ok(_) => {
                    info!("[CS2ServerManager] Servidor CS2 iniciado en AWS Lima vía tmux (puerto {port}).");
                    return Ok(());
                }
                Err(e) => {
                    warn!("[CS2ServerManager] Fallback SSH tmux falló: {e}");
                    return Err(format!("no se pudo iniciar CS2 por SSH: {e}"));
                }
            }
        } else {
            warn!("[CS2ServerManager] Clave SSH no disponible para comando remoto de inicio.");
        }

        // Sin SSH (agente local que lee `/api/cs2/current-match`), lo único comprobable es EC2.
        if let Some(e) = ec2_error {
            return Err(format!("la instancia EC2 no encendió: {e}"));
        }
        Ok(())
    }

    /// Configura y sincroniza MatchZy en el servidor remoto.
    pub async fn configure_match(&self, params: MatchParams) -> Result<GeneratedMatchZy, String> {
        let match_id = params.match_id.clone();
        let generated = GeneratedMatchZy::build(params);

        if let Some(ref key) = self.ssh_key_path {
            let sync_cmd = generated.build_remote_sync_command();
            info!("[CS2ServerManager] Sincronizando match.json y whitelist.cfg en AWS EC2 para match {match_id}...");
            match execute_remote_command(
                &self.server_base_host,
                &self.ssh_user,
                key,
                &sync_cmd,
                Duration::from_secs(15),
            )
            .await
            {
                Ok(_) => {
                    info!("[CS2ServerManager] Configuración de MatchZy sincronizada en AWS EC2 exitosamente.");
                }
                Err(e) => {
                    warn!("[CS2ServerManager] Error sincronizando MatchZy por SSH: {e}");
                }
            }
        } else {
            warn!("[CS2ServerManager] Sin clave SSH: no se pudo sincronizar MatchZy remotamente.");
        }

        Ok(generated)
    }

    /// Auto-apagado de máquina virtual en AWS EC2 si no queda ninguna partida activa (ahorro de costos).
    pub async fn auto_shutdown_if_idle(&self) -> bool {
        let matches = self.active_matches.read().await;
        if matches.is_empty() {
            if let (Some(ref ec2), Some(ref inst_id)) = (&self.ec2_client, &self.ec2_instance_id) {
                info!("[CS2ServerManager] No quedan más partidas activas. Apagando máquina virtual en AWS Lima para ahorro de costos...");
                if let Err(e) = ec2.stop_instance(inst_id).await {
                    warn!("[CS2ServerManager] Error al apagar instancia EC2: {e}");
                    return false;
                }
                return true;
            }
        } else {
            info!(
                "[CS2ServerManager] Quedan {} partida(s) en curso. Manteniendo servidor encendido.",
                matches.len()
            );
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allocates_ports_sequentially_and_frees_them() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("ADMIN_JWT_SECRET".into(), "test_secret_1234567890".into());
        vars.insert("BASE_URL".into(), "https://machupeek.com".into());
        vars.insert("DISCORD_CLIENT_ID".into(), "id".into());
        vars.insert("DISCORD_CLIENT_SECRET".into(), "secret".into());
        vars.insert("DISCORD_TOKEN".into(), "token".into());
        vars.insert("DISCORD_GUILD_ID".into(), "guild".into());

        let cfg = Config::from_map(&vars).unwrap();
        let mgr = Cs2ServerManager::new(&cfg);

        let p1 = mgr.allocate_port("m1").await.unwrap();
        assert_eq!(p1, 27015);
        mgr.publish_match("m1".into(), Some(p1), "de_mirage".into(), 5.0, 10, None)
            .await;

        let p2 = mgr.allocate_port("m2").await.unwrap();
        assert_eq!(p2, 27016);
        mgr.publish_match("m2".into(), Some(p2), "de_inferno".into(), 5.0, 10, None)
            .await;

        assert_eq!(mgr.get_all_active_matches().await.len(), 2);

        mgr.clear_match("m1").await;
        assert_eq!(mgr.get_all_active_matches().await.len(), 1);

        // El puerto 27015 vuelve a estar libre
        let p3 = mgr.allocate_port("m3").await.unwrap();
        assert_eq!(p3, 27015);
    }
}
