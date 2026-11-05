//! Crate `lobby`: Gestión en tiempo real del lobby Mixton, salas y WebSockets.

pub mod auth;
pub mod draft;
pub mod maps;
pub mod roulette;
pub mod routes;
pub mod state;
pub mod ws;

pub use auth::{
    resolve_guild_for_user, LobbyUser, OptionalLobbyUser, RequireAdmin, RequireUser,
    SessionVerifier,
};
pub use draft::{
    assign_captain, auto_assign_captains, generate_draft_pick_sequence, is_captain_eligible,
    player_tier, proposal_to_f64, set_captain_proposal, set_captain_ready,
};
pub use maps::{default_catalog, MapInfo};
pub use roulette::{
    decide_roulette, random_unit, start_draft_after_roulette, RouletteTimings,
    ROULETTE_DRAFT_DELAY_MS,
};
pub use routes::{router, LobbyApiState};
pub use state::{ChatMessage, LobbyManager, LobbyState, LobbyTeam, MapVetoState, Player};
pub use ws::mixton_ws_handler;
