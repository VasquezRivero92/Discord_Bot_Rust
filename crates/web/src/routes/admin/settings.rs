//! Rutas de configuración y servidores del panel de administración (Tanda 1):
//!
//! | ruta | legacy (`routes/api.js`) | respuesta |
//! |---|---|---|
//! | `GET /api/admin/settings` | ~1225 | `{ settings: effectiveSettings }` |
//! | `POST /api/admin/settings` | ~1270 | `{ success: true, settings }` |
//! | `GET /api/admin/guilds` | ~1858 | `{ guilds: [...] }` |
//! | `GET /api/admin/guilds/:guildId/settings` | ~2070 | `{ settings: effectiveSettings }` |
//! | `POST /api/admin/guilds/:guildId/settings` | ~2086 | `{ success: true, settings }` |
//! | `GET /api/admin/system` | ~206 | `{ uptimeSeconds, hubRegion }` |

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::{
    extract::{FromRef, Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use common::config::Config;
use data::{effective_config_for_guild, timestamp_now, GlobalSettings, GuildSettings};
use serde_json::{json, Map, Value};

use super::{ensure_manages_guild, resolve_target_guild_id, AdminContext, AdminError, AdminRepos};
use crate::routes::json_response;
use crate::session::SessionState;

const MSG_ADMIN_ONLY: &str = "Solo los administradores tienen acceso a la configuración.";
const MSG_SAVE_SETTINGS_ERROR: &str = "Error al guardar settings en .env";
const MSG_SAVE_GUILD_SETTINGS_ERROR: &str = "No se pudo guardar la configuración del servidor";

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Segundos de ejecución del proceso (equivalente de `process.uptime()`).
pub fn process_uptime() -> f64 {
    PROCESS_START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[derive(Clone)]
pub(crate) struct SettingsState {
    pub(crate) session: SessionState,
    pub(crate) repos: AdminRepos,
}

impl FromRef<SettingsState> for SessionState {
    fn from_ref(s: &SettingsState) -> SessionState {
        s.session.clone()
    }
}

pub(crate) fn router(session: SessionState, repos: AdminRepos) -> Router {
    Router::new()
        .route("/api/admin/settings", get(get_settings).post(post_settings))
        .route("/api/admin/guilds", get(get_guilds))
        .route(
            "/api/admin/guilds/{guild_id}/settings",
            get(get_guild_settings).post(post_guild_settings),
        )
        .route("/api/admin/system", get(get_system))
        .with_state(SettingsState { session, repos })
}

// ------------------------------------------------------------------ handlers

/// `GET /api/admin/settings`: configuración efectiva global.
async fn get_settings(
    State(st): State<SettingsState>,
    ctx: AdminContext,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" && ctx.user.role != "admin" {
        return Err(AdminError::Forbidden(MSG_ADMIN_ONLY));
    }
    let global = st.repos.global_settings.get().await;
    let default_guild_id = global
        .discord_guild_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&st.repos.config.discord_guild_id);

    let guild_settings = if !default_guild_id.is_empty() {
        st.repos
            .guild_settings
            .get(default_guild_id)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let effective = build_global_effective_settings(
        &global,
        guild_settings.as_ref(),
        &st.repos.config,
        default_guild_id,
    );
    Ok(json_response(StatusCode::OK, &json!({ "settings": effective })))
}

/// `POST /api/admin/settings`: guarda actualizaciones en `faceit_config/settings`.
async fn post_settings(
    State(st): State<SettingsState>,
    ctx: AdminContext,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" && ctx.user.role != "admin" {
        return Err(AdminError::Forbidden(MSG_ADMIN_ONLY));
    }
    let mut map = match body {
        Value::Object(m) => m,
        _ => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "El cuerpo debe ser un objeto JSON" }),
            ));
        }
    };

    let has_games = map.contains_key("gamesPerMatch") || map.contains_key("roundsPerGame");
    if has_games {
        let raw = map.get("gamesPerMatch").or_else(|| map.get("roundsPerGame"));
        let parsed = raw.map(parse_games_count).unwrap_or(1);
        map.insert("gamesPerMatch".into(), json!(parsed));
        map.insert("roundsPerGame".into(), json!(parsed));
    }
    map.insert("updatedAt".into(), timestamp_now());

    let updated_value = Value::Object(map);
    if let Err(e) = st
        .repos
        .doc_sink
        .merge_doc("faceit_config", "settings", &updated_value)
        .await
    {
        tracing::error!("Error al guardar settings en Firestore: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": MSG_SAVE_SETTINGS_ERROR }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": "La configuración del bot (.env y queues) fue actualizada desde el panel web.",
        "source": "system",
        "createdAt": timestamp_now(),
    });
    let _ = st
        .repos
        .doc_sink
        .set_doc("faceit_bot_logs", &log_id, &log_entry)
        .await;

    Ok(json_response(
        StatusCode::OK,
        &json!({ "success": true, "settings": updated_value }),
    ))
}

/// `GET /api/admin/guilds`: lista de servidores gestionados por el bot.
async fn get_guilds(
    State(st): State<SettingsState>,
    _ctx: AdminContext,
) -> Result<Response, AdminError> {
    // Servidores donde está el bot (como `client.guilds.cache` del legacy).
    let mut guilds = fetch_bot_guilds(&st.repos.config.discord_token).await;

    // Respaldo del legacy: si no hay ninguno, el servidor por defecto sin datos.
    if guilds.is_empty() {
        let default_guild = if !st.repos.config.discord_guild_id.is_empty() {
            Some(st.repos.config.discord_guild_id.clone())
        } else {
            let global = st.repos.global_settings.get().await;
            global.discord_guild_id.filter(|s| !s.is_empty())
        };
        if let Some(id) = default_guild {
            guilds.push(json!({
                "id": id,
                "name": "Servidor Principal",
                "icon": Value::Null,
                "memberCount": 0
            }));
        }
    }

    Ok(json_response(StatusCode::OK, &json!({ "guilds": guilds })))
}

/// `GET /api/admin/guilds/:guildId/settings`: configuración efectiva de un servidor específico.
async fn get_guild_settings(
    State(st): State<SettingsState>,
    ctx: AdminContext,
    Path(guild_id): Path<String>,
) -> Result<Response, AdminError> {
    ensure_manages_guild(&ctx, &guild_id)?;
    let global = st.repos.global_settings.get().await;
    let guild_settings = st.repos.guild_settings.get(&guild_id).await.ok().flatten();
    let effective = effective_config_for_guild(
        Some(&guild_id),
        &global,
        guild_settings.as_ref(),
        &st.repos.config,
    );
    Ok(json_response(StatusCode::OK, &json!({ "settings": effective })))
}

/// `POST /api/admin/guilds/:guildId/settings`: guarda configuración de un servidor en `guild_settings`.
async fn post_guild_settings(
    State(st): State<SettingsState>,
    ctx: AdminContext,
    Path(guild_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    ensure_manages_guild(&ctx, &guild_id)?;
    let mut map = match body {
        Value::Object(m) => m,
        _ => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "El cuerpo debe ser un objeto JSON" }),
            ));
        }
    };

    let has_games = map.contains_key("gamesPerMatch") || map.contains_key("roundsPerGame");
    if has_games {
        let raw = map.get("gamesPerMatch").or_else(|| map.get("roundsPerGame"));
        let parsed = raw.map(parse_games_count).unwrap_or(1);
        map.insert("gamesPerMatch".into(), json!(parsed));
        map.insert("roundsPerGame".into(), json!(parsed));
    }
    map.insert("updatedAt".into(), timestamp_now());

    let updated_value = Value::Object(map);
    if let Err(e) = st
        .repos
        .doc_sink
        .merge_doc("guild_settings", &guild_id, &updated_value)
        .await
    {
        tracing::error!("Error al guardar settings de la guild {guild_id}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": MSG_SAVE_GUILD_SETTINGS_ERROR }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!("Configuración del servidor {guild_id} actualizada desde el panel web."),
        "source": "system",
        "guild_id": guild_id,
        "createdAt": timestamp_now(),
    });
    let _ = st
        .repos
        .doc_sink
        .set_doc("faceit_bot_logs", &log_id, &log_entry)
        .await;

    Ok(json_response(
        StatusCode::OK,
        &json!({ "success": true, "settings": updated_value }),
    ))
}

/// `GET /api/admin/system`: estado general y región del hub Faceit.
async fn get_system(
    State(st): State<SettingsState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let global = st.repos.global_settings.get().await;
    let guild_settings = if let Some(ref g) = target {
        st.repos.guild_settings.get(g).await.ok().flatten()
    } else {
        None
    };
    let eff = effective_config_for_guild(
        target.as_deref(),
        &global,
        guild_settings.as_ref(),
        &st.repos.config,
    );

    let mut region = "Desconocida".to_string();
    if let Some(hub_id) = eff.faceit_hub_id.as_ref().filter(|s| !s.is_empty()) {
        if let Some(api_key) = st
            .repos
            .config
            .faceit_api_key
            .as_ref()
            .filter(|k| !k.is_empty())
        {
            if let Ok(reg) = fetch_hub_region(hub_id, api_key).await {
                region = reg;
            }
        }
    }

    let uptime = process_uptime();
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "uptimeSeconds": uptime,
            "hubRegion": region
        }),
    ))
}

// ------------------------------------------------------------------- helpers

fn parse_games_count(v: &Value) -> u32 {
    let n = match v {
        Value::Number(num) => num.as_u64().unwrap_or(1),
        Value::String(s) => s.trim().parse::<u64>().unwrap_or(1),
        _ => 1,
    };
    if n == 1 || n == 3 || n == 5 {
        n as u32
    } else {
        1
    }
}

fn build_global_effective_settings(
    global: &GlobalSettings,
    guild_settings: Option<&GuildSettings>,
    config: &Config,
    default_guild_id: &str,
) -> Value {
    let mut eff_val = match serde_json::to_value(effective_config_for_guild(
        if default_guild_id.is_empty() {
            None
        } else {
            Some(default_guild_id)
        },
        global,
        guild_settings,
        config,
    )) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };

    let s = &global.fields;
    let opt_set = |m: &mut Map<String, Value>, key: &str, val: &Option<String>| {
        if let Some(v) = val.as_ref().filter(|s| !s.is_empty()) {
            m.insert(key.to_string(), json!(v));
        }
    };

    if let Some(ref gid) = global.discord_guild_id.as_ref().filter(|s| !s.is_empty()) {
        eff_val.insert("discordGuildId".into(), json!(gid));
    }
    opt_set(&mut eff_val, "faceitClubId", &s.faceit_club_id);
    opt_set(&mut eff_val, "discordLinkChannelId", &s.discord_link_channel_id);
    opt_set(
        &mut eff_val,
        "discordMatchesChannelId",
        &s.discord_matches_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordMixtonChannelId",
        &s.discord_mixton_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordMixtonChatChannelId",
        &s.discord_mixton_chat_channel_id,
    );
    opt_set(&mut eff_val, "discordRolesChannelId", &s.discord_roles_channel_id);
    opt_set(
        &mut eff_val,
        "discordStatusChannelId",
        &s.discord_status_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordWaitingRoomChannelId",
        &s.discord_waiting_room_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordAdminAlertsChannelId",
        &s.discord_admin_alerts_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordSanctionsChannelId",
        &s.discord_sanctions_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordSupportChannelId",
        &s.discord_support_channel_id,
    );
    opt_set(&mut eff_val, "discordMusicChannelId", &s.discord_music_channel_id);
    opt_set(
        &mut eff_val,
        "discordRechargeChannelId",
        &s.discord_recharge_channel_id,
    );
    opt_set(
        &mut eff_val,
        "discordMatchAccessRoleId",
        &s.discord_match_access_role_id,
    );
    opt_set(&mut eff_val, "hubRoleId", &s.hub_role_id);
    opt_set(&mut eff_val, "discordHubRoleId", &s.discord_hub_role_id);
    opt_set(&mut eff_val, "faceitHubId", &s.faceit_hub_id);
    opt_set(
        &mut eff_val,
        "faceitLeaderboardId",
        &s.faceit_leaderboard_id,
    );

    if let Some(ref r) = s
        .admin_role_id
        .as_ref()
        .filter(|s| !s.is_empty())
        .or(s.admin_role_name.as_ref().filter(|s| !s.is_empty()))
    {
        eff_val.insert("adminRoleId".into(), json!(r));
    }
    if let Some(m) = s.match_timeout_minutes {
        eff_val.insert("matchTimeoutMinutes".into(), json!(m));
    }
    if s.games_per_match.is_some() || s.rounds_per_game.is_some() {
        let g = s.games_per_match.or(s.rounds_per_game).unwrap_or(1);
        let g = if g == 1 || g == 3 || g == 5 { g } else { 1 };
        eff_val.insert("gamesPerMatch".into(), json!(g));
        eff_val.insert("roundsPerGame".into(), json!(g));
    }
    if !s.queues.is_empty() {
        eff_val.insert("queues".into(), json!(s.queues));
    }
    for (k, v) in &s.extra {
        eff_val.insert(k.clone(), v.clone());
    }

    Value::Object(eff_val)
}

async fn fetch_hub_region(hub_id: &str, api_key: &str) -> Result<String, ()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ())?;
    let url = format!("https://open.faceit.com/data/v4/hubs/{hub_id}");
    let res = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| ())?;
    if !res.status().is_success() {
        return Err(());
    }
    let body: Value = res.json().await.map_err(|_| ())?;
    if let Some(r) = body.get("region").and_then(Value::as_str) {
        Ok(r.to_uppercase())
    } else {
        Err(())
    }
}

/// `GET /users/@me/guilds?with_counts=true` con el token del bot: id, nombre, icono y miembros.
/// Cualquier fallo devuelve una lista vacía (el llamador aplica el respaldo del legacy).
async fn fetch_bot_guilds(bot_token: &str) -> Vec<Value> {
    if bot_token.is_empty() {
        return Vec::new();
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return Vec::new();
    };
    let res = client
        .get("https://discord.com/api/v10/users/@me/guilds?with_counts=true")
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .header(
            reqwest::header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await;
    let Ok(res) = res else { return Vec::new() };
    if !res.status().is_success() {
        tracing::warn!("[ADMIN] Discord /users/@me/guilds respondió {}", res.status());
        return Vec::new();
    }
    let Ok(Value::Array(items)) = res.json::<Value>().await else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|g| {
            let id = g.get("id")?.as_str()?;
            let icon = g
                .get("icon")
                .and_then(Value::as_str)
                .map(|h| format!("https://cdn.discordapp.com/icons/{id}/{h}.webp"));
            Some(json!({
                "id": id,
                "name": g.get("name").and_then(Value::as_str).unwrap_or(""),
                "icon": icon,
                "memberCount": g.get("approximate_member_count").and_then(Value::as_u64).unwrap_or(0)
            }))
        })
        .collect()
}
