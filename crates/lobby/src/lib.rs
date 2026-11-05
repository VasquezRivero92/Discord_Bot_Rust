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
    assign_captain, auto_assign_captains, auto_balance, generate_draft_pick_sequence,
    is_captain_eligible, pick_player, player_tier, proposal_to_f64, set_captain_proposal,
    set_captain_ready, set_pick_order, start_draft,
};
pub use maps::{default_catalog, MapInfo};
pub use roulette::{
    decide_roulette, random_unit, start_draft_after_roulette, RouletteTimings,
    ROULETTE_DRAFT_DELAY_MS,
};
pub use routes::{resolve_discord_avatar_url, router, LobbyApiState};
pub use state::{ChatMessage, LobbyManager, LobbyState, LobbyTeam, MapVetoState, Player};
pub use ws::mixton_ws_handler;
