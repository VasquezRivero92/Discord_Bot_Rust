//! Abstracción para notificaciones de inicio de partida (Discord, etc.).
//!
//! Permite que el crate `lobby` invoque notificaciones de partidas creadas sin
//! depender directamente de crates externos como `bot` o `serenity`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

/// Datos de un equipo para el aviso de lanzamiento.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TeamNotice {
    /// Nombre visible del equipo.
    pub name: String,
    /// Discord ID del capitán.
    pub captain_discord_id: String,
    /// Lista de jugadores (Discord ID, Nickname visible). Incluye al capitán.
    pub players: Vec<(String, String)>,
}

/// Información completa del lanzamiento de partida CS2.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MatchLaunchInfo {
    /// Identificador del servidor / guild de Discord.
    pub guild_id: String,
    /// Identificador único de la partida creada.
    pub match_id: String,
    /// Dirección IP y puerto del servidor CS2 (ej. "127.0.0.1:27015").
    pub server_ip: String,
    /// Nombre visible del mapa oficial elegido (ej. "Mirage").
    pub map_name: String,
    /// Cantidad de juegos pactados (1 para BO1, 3 para BO3, etc.).
    pub games_per_match: u32,
    /// Monto de la apuesta por jugador (0.0 si es sin apuesta).
    pub bet_per_player: f64,
    /// Nombre del usuario que inició la creación de la partida.
    pub initiator_username: String,
    /// Notificación y alineación del Equipo A.
    pub team_a: TeamNotice,
    /// Notificación y alineación del Equipo B.
    pub team_b: TeamNotice,
}

/// Trait para notificar cuando una partida ha sido lanzada con éxito.
///
/// Definido con `Pin<Box<dyn Future>>` para mantener compatibilidad con trait objects (`dyn MatchNotifier`)
/// sin requerir dependencias adicionales de macros en `crates/lobby`.
pub trait MatchNotifier: Send + Sync {
    /// Notifica que una partida ha sido lanzada en el servidor CS2.
    fn match_launched<'a>(
        &'a self,
        info: &'a MatchLaunchInfo,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Contenedor compartido para registrar el notificador de forma diferida.
pub type NotifierSlot = Arc<OnceLock<Arc<dyn MatchNotifier>>>;

/// Notificador simulado para pruebas unitarias y de integración.
#[derive(Debug, Default)]
pub struct FakeNotifier {
    /// Historial de notificaciones recibidas en memoria.
    pub recorded: Mutex<Vec<MatchLaunchInfo>>,
}

impl FakeNotifier {
    /// Crea una nueva instancia vacía de `FakeNotifier`.
    pub fn new() -> Self {
        Self {
            recorded: Mutex::new(Vec::new()),
        }
    }

    /// Retorna una copia de todas las notificaciones registradas.
    pub fn matches(&self) -> Vec<MatchLaunchInfo> {
        self.recorded
            .lock()
            .map(|list| list.clone())
            .unwrap_or_default()
    }
}

impl MatchNotifier for FakeNotifier {
    fn match_launched<'a>(
        &'a self,
        info: &'a MatchLaunchInfo,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let info_clone = info.clone();
        Box::pin(async move {
            if let Ok(mut lock) = self.recorded.lock() {
                lock.push(info_clone);
            }
        })
    }
}
