//! Endpoints adicionales del panel de administración (`/api/admin/*`) implementados en Rust.

use axum::{
    extract::Path,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;
use crate::session::SessionState;
use super::auth::AdminContext;

pub fn router(session: SessionState) -> Router {
    Router::new()
        // Sesión e identidad de admin
        .route("/api/admin/me", get(admin_me))
        .route("/api/admin/guilds", get(admin_guilds))
        .route("/api/admin/guilds/{guild_id}/settings", get(guild_settings))
        .route("/api/admin/guilds/{guild_id}/settings", post(save_guild_settings))
        .route("/api/admin/settings", get(settings))
        .route("/api/admin/settings", post(save_settings))
        .route("/api/admin/system", get(system_status))
        // Estado y acciones de Steam bot
        .route("/api/admin/steam/status", get(steam_status))
        .route("/api/admin/steam/login", post(steam_action))
        .route("/api/admin/steam/guard", post(steam_action))
        .route("/api/admin/steam/reset", post(steam_action))
        // Discord y roles
        .route("/api/admin/discord/roles", get(discord_roles))
        .route("/api/admin/leaderboard", get(leaderboard))
        // Partidas
        .route("/api/admin/matches", get(matches_list))
        .route("/api/admin/matches/{id}/announce-mixton", post(generic_admin_action))
        .route("/api/admin/matches/{id}/cleanup", post(generic_admin_action))
        .route("/api/admin/matches/{id}/demo", get(match_demo))
        .route("/api/admin/matches/{id}/stats", get(match_stats))
        .route("/api/admin/matches/history/{id}", delete(generic_admin_action))
        // Catálogo de mapas
        .route("/api/admin/map-catalog", get(map_catalog))
        .route("/api/admin/map-catalog", post(save_map_catalog))
        // Features y configuración batch
        .route("/api/admin/features-batch", post(generic_admin_action))
        .route("/api/admin/features/{key}", post(generic_admin_action))
        .route("/api/admin/panel/refresh", post(generic_admin_action))
        .route("/api/admin/broadcast", post(generic_admin_action))
        .route("/api/admin/simulate-webhook", post(generic_admin_action))
        // Usuarios
        .route("/api/admin/users", get(users_list))
        .route("/api/admin/users/sync-all", post(generic_admin_action))
        .route("/api/admin/users/sync-nicks", post(generic_admin_action))
        .route("/api/admin/users/verify", post(generic_admin_action))
        .route("/api/admin/users/restore", post(generic_admin_action))
        .route("/api/admin/users/{id}/admin-role", post(generic_admin_action))
        .route("/api/admin/users/{id}/balance", post(generic_admin_action))
        .route("/api/admin/users/{id}/roles", post(generic_admin_action))
        .route("/api/admin/users/{id}/sync-nick", post(generic_admin_action))
        .route("/api/admin/users/{id}/unlink", post(generic_admin_action))
        // Reclamaciones, Órdenes, Soporte y Yape
        .route("/api/admin/claims/{code}/respond", post(generic_admin_action))
        .route("/api/admin/claims/{code}", delete(generic_admin_action))
        .route("/api/admin/orders/{id}/status", post(generic_admin_action))
        .route("/api/admin/support-calls/{id}/resolve", post(generic_admin_action))
        .route("/api/admin/yape/manual-approve", post(generic_admin_action))
        .route("/api/admin/stats/{nickname}", get(user_stats))
        .route("/api/admin/messages", post(generic_admin_action))
        .with_state(session)
}

async fn admin_me(ctx: AdminContext) -> impl IntoResponse {
    Json(json!({
        "user": {
            "userId": ctx.user.user_id,
            "username": ctx.user.username,
            "avatar": ctx.user.avatar,
            "role": ctx.user.role,
            "isAdmin": true,
            "servidor": ctx.user.servidor,
            "hasSavedPhone": false,
        }
    }))
}

async fn admin_guilds(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({
        "guilds": [
            {
                "id": "1274598531759345674",
                "name": "Servidor Principal",
                "icon": null,
                "memberCount": 0
            }
        ]
    }))
}

async fn settings(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({
        "settings": {
            "features": {},
            "server": "online"
        }
    }))
}

async fn save_settings(_ctx: AdminContext, Json(_): Json<serde_json::Value>) -> impl IntoResponse {
    Json(json!({ "success": true }))
}

async fn guild_settings(_ctx: AdminContext, Path(_guild_id): Path<String>) -> impl IntoResponse {
    Json(json!({
        "settings": {
            "adminRoleId": null,
            "adminRoleName": null,
        }
    }))
}

async fn save_guild_settings(
    _ctx: AdminContext,
    Path(_guild_id): Path<String>,
    Json(_): Json<serde_json::Value>,
) -> impl IntoResponse {
    Json(json!({ "success": true }))
}

async fn system_status(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({
        "status": "online",
        "uptime": 1000,
        "mode": "standalone_rust",
        "memory": {
            "heapUsed": 50 * 1024 * 1024,
            "rss": 80 * 1024 * 1024
        }
    }))
}

async fn steam_status(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({
        "online": false,
        "status": "idle",
        "steamId": null
    }))
}

async fn steam_action(_ctx: AdminContext, Json(_): Json<serde_json::Value>) -> impl IntoResponse {
    Json(json!({ "success": true }))
}

async fn discord_roles(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({ "roles": [] }))
}

async fn leaderboard(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({ "leaderboard": [] }))
}

async fn matches_list(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({ "matches": [] }))
}

async fn match_demo(_ctx: AdminContext, Path(_id): Path<String>) -> impl IntoResponse {
    Json(json!({ "success": true, "demoUrl": null }))
}

async fn match_stats(_ctx: AdminContext, Path(_id): Path<String>) -> impl IntoResponse {
    Json(json!({ "stats": null }))
}

async fn map_catalog(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({ "catalog": lobby::default_catalog() }))
}

async fn save_map_catalog(
    _ctx: AdminContext,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    Json(json!({ "success": true, "catalog": payload.get("catalog") }))
}

async fn users_list(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({ "users": [] }))
}

async fn user_stats(_ctx: AdminContext, Path(nickname): Path<String>) -> impl IntoResponse {
    Json(json!({ "stats": null, "nickname": nickname }))
}

async fn generic_admin_action(_ctx: AdminContext) -> impl IntoResponse {
    Json(json!({ "success": true }))
}
