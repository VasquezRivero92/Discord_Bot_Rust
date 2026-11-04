//! Crate `lobby`: Gestión en tiempo real del lobby Mixton, salas y WebSockets.

pub mod maps;
pub mod routes;
pub mod state;
pub mod ws;

pub use maps::{default_catalog, MapInfo};
pub use routes::{router, LobbyApiState};
pub use state::{ChatMessage, LobbyManager, LobbyState, LobbyTeam, MapVetoState, Player};
pub use ws::mixton_ws_handler;
