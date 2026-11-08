//! Rutas de administración de partidas, usuarios, catálogo de mapas, roles de Discord y leaderboard (Tanda 2):
//!
//! | ruta | legacy (`routes/api.js`) | respuesta |
//! |---|---|---|
//! | `GET /api/admin/leaderboard` | ~1107 | `{ leaderboard: [...] }` |
//! | `GET /api/admin/discord/roles` | ~869 | `{ roles: [...] }` |
//! | `GET /api/admin/map-catalog` | ~2131 | `{ success: true, catalog: [...], activeCount: N }` |
//! | `POST /api/admin/map-catalog` | ~2145 | `{ success: true, catalog: [...], activeCount: N }` |
//! | `GET /api/admin/users` | ~417 | `{ users: [...] }` |
//! | `GET /api/admin/matches` | ~942 | `{ matches: [...], stats: { serverLoad, avgDurationStr, failedCleanups } }` |
//! | `GET /api/admin/matches/:id/stats` | ~1559 | `{ success: bool, stats: ... }` |
//! | `GET /api/admin/matches/:id/demo` | ~1528 | `{ demo_url: ..., faceit_url: ... }` |

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{FromRef, Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use data::{effective_config_for_guild, timestamp_now};
use serde_json::{json, Map, Value};

use super::{resolve_target_guild_id, AdminContext, AdminError, AdminRepos};
use crate::routes::json_response;
use crate::session::SessionState;

#[derive(Clone)]
pub(crate) struct MatchesUsersState {
    pub(crate) session: SessionState,
    pub(crate) repos: AdminRepos,
}

impl FromRef<MatchesUsersState> for SessionState {
    fn from_ref(s: &MatchesUsersState) -> SessionState {
        s.session.clone()
    }
}

pub(crate) fn router(session: SessionState, repos: AdminRepos) -> Router {
    Router::new()
        .route("/api/admin/leaderboard", get(get_leaderboard))
        .route("/api/admin/discord/roles", get(get_discord_roles))
        .route("/api/admin/discord/channels", get(get_discord_channels))
        .route(
            "/api/admin/map-catalog",
            get(get_map_catalog).post(post_map_catalog),
        )
        .route("/api/admin/users", get(get_users))
        .route("/api/admin/matches", get(get_matches))
        .route("/api/admin/matches/{id}/stats", get(get_match_stats))
        .route("/api/admin/matches/{id}/demo", get(get_match_demo))
        .route("/api/admin/stats/{nickname}", get(get_stats_by_nickname))
        .with_state(MatchesUsersState { session, repos })
}

// ------------------------------------------------------------------ handlers

/// `GET /api/admin/leaderboard`: tabla de clasificación de FACEIT para la guild o global.
async fn get_leaderboard(
    State(st): State<MatchesUsersState>,
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

    let leaderboard_id = eff
        .faceit_leaderboard_id
        .as_deref()
        .unwrap_or("")
        .trim();
    if leaderboard_id.is_empty() {
        return Ok(json_response(StatusCode::OK, &json!({ "leaderboard": [] })));
    }

    let api_key = st
        .repos
        .config
        .faceit_api_key
        .as_deref()
        .unwrap_or("")
        .trim();
    if api_key.is_empty() {
        return Ok(json_response(StatusCode::OK, &json!({ "leaderboard": [] })));
    }

    // Mapa apodo -> discordId para cruzar con los resultados de FACEIT
    let users = st.repos.users.all().await.unwrap_or_default();
    let mut nick_to_discord = HashMap::new();
    for u in users {
        if let Some(nick) = u.nickname() {
            let discord_id = u.discord_id().unwrap_or_else(|| "N/A".to_string());
            nick_to_discord.insert(nick.to_lowercase(), discord_id);
        }
    }

    match fetch_faceit_leaderboard(leaderboard_id, api_key).await {
        Ok(items) => {
            let entries: Vec<Value> = items
                .into_iter()
                .map(|item| {
                    let player = item.get("player");
                    let user_id = player
                        .and_then(|p| p.get("user_id"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let nickname = player
                        .and_then(|p| p.get("nickname"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let skill_level = player
                        .and_then(|p| p.get("skill_level"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let points = item.get("points").and_then(Value::as_i64).unwrap_or(0);
                    let discord_id = nick_to_discord
                        .get(&nickname.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| "N/A".to_string());

                    json!({
                        "id": user_id,
                        "discord_id": discord_id,
                        "faceit_nickname": nickname,
                        "faceit_level": skill_level,
                        "faceit_elo": points,
                    })
                })
                .collect();
            Ok(json_response(StatusCode::OK, &json!({ "leaderboard": entries })))
        }
        Err(e) => {
            tracing::error!("Error al obtener leaderboard {leaderboard_id}: {e}");
            Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al obtener leaderboard" }),
            ))
        }
    }
}

/// `GET /api/admin/discord/roles`: lista de roles de la guild en Discord, ordenados por nombre.
async fn get_discord_roles(
    State(st): State<MatchesUsersState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let guild_id = target
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let gid = st.repos.config.discord_guild_id.trim();
            if !gid.is_empty() {
                Some(gid)
            } else {
                None
            }
        });

    let Some(guild_id) = guild_id else {
        return Ok(json_response(StatusCode::OK, &json!({ "roles": [] })));
    };

    let bot_token = st.repos.config.discord_token.trim();
    if bot_token.is_empty() {
        return Ok(json_response(StatusCode::OK, &json!({ "roles": [] })));
    }

    match fetch_discord_roles(guild_id, bot_token).await {
        Ok(roles) => Ok(json_response(StatusCode::OK, &json!({ "roles": roles }))),
        Err(e) => {
            tracing::error!("Error al listar roles de Discord para {guild_id}: {e}");
            Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al obtener roles de Discord" }),
            ))
        }
    }
}

/// `GET /api/admin/discord/channels`: canales de la guild (texto, anuncios, voz y escenario),
/// agrupados por categoría, para los desplegables de la configuración.
async fn get_discord_channels(
    State(st): State<MatchesUsersState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let guild_id = target
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let gid = st.repos.config.discord_guild_id.trim();
            (!gid.is_empty()).then_some(gid)
        });

    let bot_token = st.repos.config.discord_token.trim();
    let (Some(guild_id), false) = (guild_id, bot_token.is_empty()) else {
        return Ok(json_response(StatusCode::OK, &json!({ "channels": [] })));
    };

    match fetch_discord_channels(guild_id, bot_token).await {
        Ok(channels) => Ok(json_response(StatusCode::OK, &json!({ "channels": channels }))),
        Err(e) => {
            tracing::error!("Error al listar canales de Discord para {guild_id}: {e}");
            Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al obtener canales de Discord" }),
            ))
        }
    }
}

/// `GET /api/admin/map-catalog`: catálogo efectivo de mapas para el sistema de veto de Mixton.
async fn get_map_catalog(
    State(st): State<MatchesUsersState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let state = match st.repos.docs.mixton_lobby_state(target.as_deref()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Error al leer estado del lobby para map-catalog: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al obtener catálogo de mapas" }),
            ));
        }
    };

    let effective = compute_effective_map_catalog(state.as_ref());
    let active_count = effective
        .iter()
        .filter(|m| m.get("active") == Some(&Value::Bool(true)))
        .count();

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "catalog": effective,
            "activeCount": active_count,
        }),
    ))
}

/// `POST /api/admin/map-catalog`: actualiza el catálogo de mapas del lobby (mínimo 2 activos).
async fn post_map_catalog(
    State(st): State<MatchesUsersState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;

    let raw_catalog = if let Some(arr) = body.get("catalog").and_then(Value::as_array) {
        Some(arr.clone())
    } else if let Some(arr) = body.as_array() {
        Some(arr.clone())
    } else {
        None
    };

    let Some(catalog) = raw_catalog else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El catálogo debe ser un array de mapas." }),
        ));
    };

    let active_count = catalog
        .iter()
        .filter(|m| m.get("active") == Some(&Value::Bool(true)))
        .count();

    if active_count < 2 {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes mantener al menos 2 mapas activos para el sistema de veto." }),
        ));
    }

    let doc_id = match target.as_deref() {
        Some(g) if !g.is_empty() => format!("mixton_lobby_{g}"),
        _ => "mixton_lobby".to_string(),
    };

    let update = json!({
        "mapCatalog": catalog,
        "updatedAt": timestamp_now(),
    });

    if let Err(e) = st
        .repos
        .doc_sink
        .merge_doc("faceit_config", &doc_id, &update)
        .await
    {
        tracing::error!("Error al guardar catálogo de mapas en {doc_id}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Error al guardar catálogo de mapas" }),
        ));
    }

    let fake_state = json!({ "mapCatalog": catalog });
    let effective = compute_effective_map_catalog(Some(&fake_state));
    let effective_active = effective
        .iter()
        .filter(|m| m.get("active") == Some(&Value::Bool(true)))
        .count();

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "catalog": effective,
            "activeCount": effective_active,
        }),
    ))
}

/// `GET /api/admin/users`: lista de usuarios vinculados con su rol de club en Discord.
async fn get_users(
    State(st): State<MatchesUsersState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let all_users = match st.repos.users.all().await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Error al obtener usuarios: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al obtener usuarios" }),
            ));
        }
    };

    let guild_id_to_use = target
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let gid = st.repos.config.discord_guild_id.trim();
            if !gid.is_empty() {
                Some(gid)
            } else {
                None
            }
        });

    let bot_token = st.repos.config.discord_token.trim();
    let member_club_roles = if let Some(gid) = guild_id_to_use {
        if !bot_token.is_empty() {
            fetch_discord_member_club_roles(gid, bot_token)
                .await
                .unwrap_or_default()
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    let mut users_list = Vec::new();
    for u in all_users {
        // Los usuarios vinculados son globales; si tienen servidor asignado y difiere del filtro,
        // solo se filtra cuando el documento explícitamente pertenece a otro servidor distinto.
        if let (Some(ref t), Some(ug)) = (&target, u.guild()) {
            if ug != t.as_str() {
                continue;
            }
        }

        let actual_discord_id = u
            .discord_id()
            .or_else(|| u.doc_id.clone())
            .unwrap_or_default();

        let club_role = member_club_roles.get(&actual_discord_id).cloned();

        let mut user_val = match serde_json::to_value(&u) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };

        user_val.insert("discordId".into(), Value::String(actual_discord_id));
        user_val.insert(
            "clubRole".into(),
            match club_role {
                Some(r) => Value::String(r),
                None => Value::Null,
            },
        );

        users_list.push(Value::Object(user_val));
    }

    Ok(json_response(StatusCode::OK, &json!({ "users": users_list })))
}

/// `GET /api/admin/matches`: partidas activas con estadísticas de carga, duración y limpiezas.
async fn get_matches(
    State(st): State<MatchesUsersState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let matches = match st.repos.docs.active_matches(target.as_deref()).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Error al obtener partidas activas: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al obtener partidas" }),
            ));
        }
    };

    // 1. Carga de servidores (basada en partidas activas / capacidad max 10)
    let max_capacity = 10.0;
    let server_load =
        ((matches.len() as f64 / max_capacity * 100.0 * 10.0).round() / 10.0).min(100.0);

    // 2. Duración promedio
    let avg_duration_str = if matches.is_empty() {
        "0m 0s".to_string()
    } else {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let total_duration_ms: u64 = matches
            .iter()
            .map(|m| {
                let start = m
                    .get("startedAt")
                    .and_then(Value::as_u64)
                    .unwrap_or(now_ms);
                now_ms.saturating_sub(start)
            })
            .sum();
        let avg_ms = total_duration_ms / matches.len() as u64;
        let avg_sec = avg_ms / 1000;
        let mins = avg_sec / 60;
        let secs = avg_sec % 60;
        format!("{mins}m {secs}s")
    };

    // 3. Limpiezas fallidas (canales de voz huérfanos con prefijo 🔊 que no corresponden a partidas activas)
    let mut failed_cleanups = 0;
    let guild_id_to_use = target
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let gid = st.repos.config.discord_guild_id.trim();
            if !gid.is_empty() {
                Some(gid)
            } else {
                None
            }
        });

    let bot_token = st.repos.config.discord_token.trim();
    if let Some(gid) = guild_id_to_use {
        if !bot_token.is_empty() {
            let mut active_vc_ids = HashSet::new();
            for m in &matches {
                if let Some(vc1) = m.get("faction1_vc_id").and_then(Value::as_str) {
                    if !vc1.is_empty() {
                        active_vc_ids.insert(vc1.to_string());
                    }
                }
                if let Some(vc2) = m.get("faction2_vc_id").and_then(Value::as_str) {
                    if !vc2.is_empty() {
                        active_vc_ids.insert(vc2.to_string());
                    }
                }
            }
            if let Ok(count) = fetch_discord_orphan_vcs(gid, bot_token, &active_vc_ids).await {
                failed_cleanups = count;
            }
        }
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "matches": matches,
            "stats": {
                "serverLoad": server_load,
                "avgDurationStr": avg_duration_str,
                "failedCleanups": failed_cleanups,
            }
        }),
    ))
}

/// `GET /api/admin/matches/:id/stats`: estadísticas detalladas de una partida FACEIT.
async fn get_match_stats(
    State(st): State<MatchesUsersState>,
    _ctx: AdminContext,
    Path(match_id): Path<String>,
) -> Result<Response, AdminError> {
    if match_id.starts_with("mixton_") || match_id.starts_with("manual_") {
        return Ok(json_response(
            StatusCode::OK,
            &json!({ "success": false, "stats": Value::Null }),
        ));
    }

    let api_key = st
        .repos
        .config
        .faceit_api_key
        .as_deref()
        .unwrap_or("")
        .trim();
    if !api_key.is_empty() {
        if let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(10)).build() {
            let base = faceit_api_base_url();
            let url = format!("{base}/matches/{match_id}/stats");
            let res = client
                .get(&url)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await;
            if let Ok(res) = res {
                if res.status().is_success() {
                    if let Ok(stats_data) = res.json::<Value>().await {
                        return Ok(json_response(
                            StatusCode::OK,
                            &json!({ "success": true, "stats": stats_data }),
                        ));
                    }
                }
            }
        }
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({ "success": false, "stats": Value::Null }),
    ))
}

/// `GET /api/admin/matches/:id/demo`: URL de la demo de una partida (Mixton o FACEIT).
async fn get_match_demo(
    State(st): State<MatchesUsersState>,
    _ctx: AdminContext,
    Path(match_id): Path<String>,
) -> Result<Response, AdminError> {
    if match_id.starts_with("mixton_") || match_id.starts_with("manual_") {
        let match_doc = match st.repos.docs.active_match(&match_id).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Error al obtener partida activa {match_id}: {e}");
                return Ok(json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "Error al obtener la demo de la partida" }),
                ));
            }
        };
        let demo_url = match_doc
            .as_ref()
            .and_then(|m| m.get("demo_url"))
            .cloned()
            .unwrap_or(Value::Null);
        return Ok(json_response(
            StatusCode::OK,
            &json!({ "demo_url": demo_url }),
        ));
    }

    let api_key = st
        .repos
        .config
        .faceit_api_key
        .as_deref()
        .unwrap_or("")
        .trim();
    if !api_key.is_empty() {
        if let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(10)).build() {
            let base = faceit_api_base_url();
            let url = format!("{base}/matches/{match_id}");
            let res = client
                .get(&url)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await;
            if let Ok(res) = res {
                if res.status().is_success() {
                    if let Ok(data) = res.json::<Value>().await {
                        let demo_url = if let Some(arr) = data.get("demo_url").and_then(Value::as_array) {
                            arr.first().cloned().unwrap_or(Value::Null)
                        } else {
                            data.get("demo_url").cloned().unwrap_or(Value::Null)
                        };
                        let faceit_url = data
                            .get("faceit_url")
                            .cloned()
                            .unwrap_or_else(|| Value::String(format!("https://www.faceit.com/en/cs2/room/{match_id}")));

                        return Ok(json_response(
                            StatusCode::OK,
                            &json!({
                                "demo_url": demo_url,
                                "faceit_url": faceit_url,
                            }),
                        ));
                    }
                }
            }
        }
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({ "demo_url": Value::Null }),
    ))
}

// ------------------------------------------------------------------ helpers

fn base_map_catalog() -> Vec<Value> {
    vec![
        json!({
            "id": "de_mirage",
            "name": "Mirage",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mirage.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mirage.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_inferno",
            "name": "Inferno",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_inferno.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_inferno.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_dust2",
            "name": "Dust II",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_dust2.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_dust2.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_nuke",
            "name": "Nuke",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_nuke.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_nuke.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_ancient",
            "name": "Ancient",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_ancient.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_ancient.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_anubis",
            "name": "Anubis",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_anubis.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_anubis.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_vertigo",
            "name": "Vertigo",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_vertigo.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_vertigo.png",
            "active": true,
            "isOfficialPremier": true
        }),
        json!({
            "id": "de_train",
            "name": "Train",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_train.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_train.png",
            "active": false,
            "isOfficialPremier": false
        }),
        json!({
            "id": "de_overpass",
            "name": "Overpass",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_overpass.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_overpass.png",
            "active": false,
            "isOfficialPremier": false
        }),
        json!({
            "id": "de_cache",
            "name": "Cache",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_cache.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_cache.png",
            "active": false,
            "isOfficialPremier": false
        }),
        json!({
            "id": "cs_office",
            "name": "Office",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/cs_office.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/cs_office.png",
            "active": false,
            "isOfficialPremier": false
        }),
        json!({
            "id": "cs_italy",
            "name": "Italy",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/cs_italy.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/cs_italy.png",
            "active": false,
            "isOfficialPremier": false
        }),
        json!({
            "id": "de_thera",
            "name": "Thera",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_thera.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_thera.png",
            "active": false,
            "isOfficialPremier": false
        }),
        json!({
            "id": "de_mills",
            "name": "Mills",
            "image": "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mills.png",
            "icon": "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mills.png",
            "active": false,
            "isOfficialPremier": false
        }),
    ]
}

fn compute_effective_map_catalog(state: Option<&Value>) -> Vec<Value> {
    let base = base_map_catalog();
    let stored_catalog = state
        .and_then(|s| s.get("mapCatalog"))
        .and_then(Value::as_array);

    let Some(stored_arr) = stored_catalog else {
        return base;
    };
    if stored_arr.is_empty() {
        return base;
    }

    let mut stored_map: HashMap<String, &Value> = HashMap::new();
    for m in stored_arr {
        if let Some(id) = m.get("id").and_then(Value::as_str) {
            stored_map.insert(id.to_string(), m);
        }
    }

    let mut merged = Vec::new();
    for base_map in &base {
        let id = base_map.get("id").and_then(Value::as_str).unwrap_or("");
        if let Some(stored) = stored_map.get(id) {
            let mut obj = base_map.as_object().cloned().unwrap_or_default();
            if let Some(stored_obj) = stored.as_object() {
                for (k, v) in stored_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }
            if let Some(active) = stored.get("active").and_then(Value::as_bool) {
                obj.insert("active".into(), Value::Bool(active));
            }
            merged.push(Value::Object(obj));
        } else {
            merged.push(base_map.clone());
        }
    }

    // Agregar mapas personalizados que haya añadido el admin
    for m in stored_arr {
        if let Some(id) = m.get("id").and_then(Value::as_str) {
            if !base.iter().any(|b| b.get("id").and_then(Value::as_str) == Some(id)) {
                let mut obj = m.as_object().cloned().unwrap_or_default();
                obj.entry("name".to_string())
                    .or_insert_with(|| Value::String(id.to_string()));
                obj.entry("image".to_string()).or_insert_with(|| {
                    Value::String(
                        "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mirage.png"
                            .to_string(),
                    )
                });
                obj.entry("icon".to_string()).or_insert_with(|| {
                    Value::String(
                        "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mirage.png"
                            .to_string(),
                    )
                });
                obj.entry("active".to_string())
                    .or_insert_with(|| Value::Bool(true));
                obj.insert("isCustom".into(), Value::Bool(true));
                merged.push(Value::Object(obj));
            }
        }
    }

    merged
}

fn discord_api_base_url() -> String {
    std::env::var("DISCORD_API_BASE_URL")
        .unwrap_or_else(|_| "https://discord.com/api/v10".to_string())
}

fn faceit_api_base_url() -> String {
    std::env::var("FACEIT_API_BASE_URL")
        .unwrap_or_else(|_| "https://open.faceit.com/data/v4".to_string())
}

async fn fetch_faceit_leaderboard(id: &str, api_key: &str) -> Result<Vec<Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let base = faceit_api_base_url();
    let url = format!("{base}/leaderboards/{id}?offset=0&limit=50");
    let res = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if res.status() == StatusCode::NOT_FOUND {
        // Intento 2: Podría ser un Hub ID en lugar de Leaderboard ID
        let hub_url = format!("{base}/leaderboards/hubs/{id}");
        let hub_res = client
            .get(&hub_url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if hub_res.status().is_success() {
            let hub_data: Value = hub_res.json().await.map_err(|e| e.to_string())?;
            if let Some(items) = hub_data.get("items").and_then(Value::as_array) {
                let mut seasons: Vec<&Value> = items
                    .iter()
                    .filter(|i| {
                        i.get("leaderboard_type").and_then(Value::as_str) == Some("hub_season")
                    })
                    .collect();
                seasons.sort_by_key(|i| i.get("season").and_then(Value::as_i64).unwrap_or(0));
                let latest_id = seasons
                    .last()
                    .and_then(|i| i.get("leaderboard_id"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        items
                            .first()
                            .and_then(|i| i.get("leaderboard_id"))
                            .and_then(Value::as_str)
                    });
                if let Some(lid) = latest_id {
                    let s_url = format!(
                        "{base}/leaderboards/{lid}?offset=0&limit=50"
                    );
                    let s_res = client
                        .get(&s_url)
                        .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
                        .header(reqwest::header::ACCEPT, "application/json")
                        .send()
                        .await
                        .map_err(|e| e.to_string())?;
                    if s_res.status().is_success() {
                        let l_data: Value = s_res.json().await.map_err(|e| e.to_string())?;
                        return Ok(l_data
                            .get("items")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default());
                    }
                }
            }
        }
        return Ok(Vec::new());
    }

    if !res.status().is_success() {
        return Err(format!("FACEIT API returned status {}", res.status()));
    }

    let data: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(data
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

async fn fetch_discord_roles(guild_id: &str, bot_token: &str) -> Result<Vec<Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let base = discord_api_base_url();
    let url = format!("{base}/guilds/{guild_id}/roles");
    let res = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .header(
            reqwest::header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Discord roles API returned status {}", res.status()));
    }

    let roles_val: Value = res.json().await.map_err(|e| e.to_string())?;
    let Some(roles_arr) = roles_val.as_array() else {
        return Ok(Vec::new());
    };

    let mut roles = Vec::new();
    for r in roles_arr {
        let name = r.get("name").and_then(Value::as_str).unwrap_or("");
        if name == "@everyone" {
            continue;
        }
        let id = r.get("id").and_then(Value::as_str).unwrap_or("");
        let color_int = r.get("color").and_then(Value::as_u64).unwrap_or(0);
        let color_hex = format!("#{:06X}", color_int);

        roles.push(json!({
            "id": id,
            "name": name,
            "color": color_hex,
        }));
    }

    roles.sort_by(|a, b| {
        let name_a = a.get("name").and_then(Value::as_str).unwrap_or("");
        let name_b = b.get("name").and_then(Value::as_str).unwrap_or("");
        name_a.cmp(name_b)
    });

    Ok(roles)
}

/// Canales de la guild como `{ id, name, kind, parentId, parentName }`, con `kind` en
/// `text` | `voice`; las categorías y los tipos sin uso (foros, hilos) se descartan.
async fn fetch_discord_channels(guild_id: &str, bot_token: &str) -> Result<Vec<Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let base = discord_api_base_url();
    let res = client
        .get(format!("{base}/guilds/{guild_id}/channels"))
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .header(
            reqwest::header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Discord channels API returned status {}", res.status()));
    }

    let body: Value = res.json().await.map_err(|e| e.to_string())?;
    let Some(all) = body.as_array() else {
        return Ok(Vec::new());
    };

    let category_name = |id: &str| -> Option<&str> {
        all.iter()
            .find(|c| c.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|c| c.get("name").and_then(Value::as_str))
    };

    let mut channels: Vec<(u64, Value)> = Vec::new();
    for c in all {
        let kind = match c.get("type").and_then(Value::as_u64) {
            Some(0) | Some(5) => "text",
            Some(2) | Some(13) => "voice",
            _ => continue,
        };
        let id = c.get("id").and_then(Value::as_str).unwrap_or("");
        let parent_id = c.get("parent_id").and_then(Value::as_str);
        let position = c.get("position").and_then(Value::as_u64).unwrap_or(0);
        channels.push((
            position,
            json!({
                "id": id,
                "name": c.get("name").and_then(Value::as_str).unwrap_or(""),
                "kind": kind,
                "parentId": parent_id,
                "parentName": parent_id.and_then(category_name),
            }),
        ));
    }
    channels.sort_by_key(|(position, _)| *position);

    Ok(channels.into_iter().map(|(_, v)| v).collect())
}

async fn fetch_discord_member_club_roles(
    guild_id: &str,
    bot_token: &str,
) -> Result<HashMap<String, String>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    // 1. Obtener roles para identificar cuáles coinciden con CLUB_ROLES_MAP
    let base = discord_api_base_url();
    let roles_url = format!("{base}/guilds/{guild_id}/roles");
    let res = client
        .get(&roles_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .header(
            reqwest::header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Discord roles API returned status {}", res.status()));
    }

    let roles_val: Value = res.json().await.map_err(|e| e.to_string())?;
    let Some(roles_arr) = roles_val.as_array() else {
        return Ok(HashMap::new());
    };

    let club_names = ["EXTREME", "EXTREME BAJOS", "PREMIUM", "PLUS"];
    let mut role_id_to_club_name = HashMap::new();
    for r in roles_arr {
        let name = r
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_uppercase();
        let id = r.get("id").and_then(Value::as_str).unwrap_or("");
        if club_names.contains(&name.as_str()) {
            role_id_to_club_name.insert(id.to_string(), name);
        }
    }

    if role_id_to_club_name.is_empty() {
        return Ok(HashMap::new());
    }

    // 2. Obtener miembros (hasta 1000)
    let members_url = format!("{base}/guilds/{guild_id}/members?limit=1000");
    let m_res = client
        .get(&members_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .header(
            reqwest::header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !m_res.status().is_success() {
        return Err(format!("Discord members API returned status {}", m_res.status()));
    }

    let members_val: Value = m_res.json().await.map_err(|e| e.to_string())?;
    let Some(members_arr) = members_val.as_array() else {
        return Ok(HashMap::new());
    };

    let mut member_club_role = HashMap::new();
    for m in members_arr {
        let user_id = m
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if user_id.is_empty() {
            continue;
        }
        if let Some(roles) = m.get("roles").and_then(Value::as_array) {
            for r_val in roles {
                if let Some(rid) = r_val.as_str() {
                    if let Some(cname) = role_id_to_club_name.get(rid) {
                        member_club_role.insert(user_id.to_string(), cname.clone());
                        break;
                    }
                }
            }
        }
    }

    Ok(member_club_role)
}

async fn fetch_discord_orphan_vcs(
    guild_id: &str,
    bot_token: &str,
    active_vc_ids: &HashSet<String>,
) -> Result<usize, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let base = discord_api_base_url();
    let url = format!("{base}/guilds/{guild_id}/channels");
    let res = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .header(
            reqwest::header::USER_AGENT,
            "DiscordBot (https://machupeek.com, 1.0.0)",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Discord channels API returned status {}", res.status()));
    }

    let channels_val: Value = res.json().await.map_err(|e| e.to_string())?;
    let Some(channels_arr) = channels_val.as_array() else {
        return Ok(0);
    };

    let orphan_count = channels_arr
        .iter()
        .filter(|c| {
            let channel_type = c.get("type").and_then(Value::as_i64).unwrap_or(-1);
            let name = c.get("name").and_then(Value::as_str).unwrap_or("");
            let id = c.get("id").and_then(Value::as_str).unwrap_or("");
            channel_type == 2 && name.starts_with("🔊") && !active_vc_ids.contains(id)
        })
        .count();

    Ok(orphan_count)
}

/// `GET /api/admin/stats/:nickname`: estadísticas de un jugador en FACEIT.
async fn get_stats_by_nickname(
    State(st): State<MatchesUsersState>,
    _ctx: AdminContext,
    Path(nickname): Path<String>,
) -> Response {
    let base = "https://open.faceit.com/data/v4";
    if let Some(api_key) = st.repos.config.faceit_api_key.as_deref().filter(|s| !s.is_empty()) {
        let client = reqwest::Client::new();
        let player_url = format!("{base}/players?nickname={nickname}");
        if let Ok(res) = client
            .get(&player_url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
        {
            if res.status().is_success() {
                if let Ok(player_data) = res.json::<Value>().await {
                    let player_id = player_data.get("player_id").and_then(Value::as_str).unwrap_or("");
                    let stats_url = format!("{base}/players/{player_id}/stats/cs2");
                    let stats = if let Ok(s_res) = client
                        .get(&stats_url)
                        .header("Authorization", format!("Bearer {api_key}"))
                        .send()
                        .await
                    {
                        s_res.json::<Value>().await.unwrap_or(json!({}))
                    } else {
                        json!({})
                    };

                    let cs2 = player_data.get("games").and_then(|g| g.get("cs2"));
                    let level = cs2.and_then(|c| c.get("skill_level")).and_then(Value::as_i64).unwrap_or(1);
                    let elo = cs2.and_then(|c| c.get("faceit_elo")).and_then(Value::as_i64).unwrap_or(1000);

                    return json_response(
                        StatusCode::OK,
                        &json!({
                            "player": {
                                "player_id": player_id,
                                "nickname": nickname,
                                "avatar": player_data.get("avatar"),
                                "country": player_data.get("country"),
                                "level": level,
                                "elo": elo,
                                "role": format!("LVL {level}")
                            },
                            "stats": stats,
                            "history": []
                        }),
                    );
                }
            } else if res.status() == StatusCode::NOT_FOUND {
                return json_response(
                    StatusCode::NOT_FOUND,
                    &json!({ "error": "Jugador no encontrado en Faceit" }),
                );
            }
        }
    }

    json_response(
        StatusCode::OK,
        &json!({
            "player": {
                "player_id": format!("player_{nickname}"),
                "nickname": nickname,
                "avatar": Value::Null,
                "country": "pe",
                "level": 1,
                "elo": 1000,
                "role": "LVL 1"
            },
            "stats": {},
            "history": []
        }),
    )
}
