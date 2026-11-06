//! Rutas de escritura del panel de administración (Tanda 3):
//!
//! | ruta | legacy (`routes/api.js`) | respuesta |
//! |---|---|---|
//! | `POST /api/admin/features/:key` | ~1383 | `{ success: true, key, enabled: bool }` |
//! | `POST /api/admin/features-batch` | ~1438 | `{ success: true, settings }` |
//! | `POST /api/admin/users/restore` | ~694 | `{ success: true, message }` |
//! | `DELETE /api/admin/matches/history/:id` | ~1512 | `{ success: true, message }` |
//!
//! `POST /api/admin/users/verify` permanece en el proxy hacia Node porque requiere
//! sincronizar roles y apodos en Discord a través del cliente del bot (`client.guilds.fetch`,
//! `syncMemberLevelRole`, `syncMemberNickname`).

use axum::{
    extract::{FromRef, Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{delete, post},
    Json, Router,
};
use data::timestamp_now;
use serde_json::{json, Map, Value};

use super::{resolve_target_guild_id, AdminContext, AdminError, AdminRepos};
use crate::routes::json_response;
use crate::session::SessionState;

const MSG_GLOBAL_ADMIN_ONLY: &str =
    "Solo el administrador global puede modificar la configuración global.";
const MSG_FEATURE_ERROR: &str = "Error al actualizar feature flag";
const MSG_FEATURE_BATCH_ERROR: &str = "Error al actualizar servicios por lote";
const MSG_FEATURE_BATCH_INVALID: &str =
    "El cuerpo de la petición debe contener un objeto \"updates\" con los pares servicio/estado.";
const MSG_RESTORE_INVALID: &str = "El formato de datos de backup no es válido";
const MSG_RESTORE_ERROR: &str = "Error al restaurar backup";
const MSG_DELETE_MATCH_NOT_FOUND: &str =
    "No se encontró la partida en el historial o no pudo eliminarse";
const MSG_DELETE_MATCH_ERROR: &str = "Error al eliminar la partida del historial";

#[derive(Clone)]
pub(crate) struct WritesState {
    pub(crate) session: SessionState,
    pub(crate) repos: AdminRepos,
}

impl FromRef<WritesState> for SessionState {
    fn from_ref(s: &WritesState) -> SessionState {
        s.session.clone()
    }
}

pub(crate) fn router(session: SessionState, repos: AdminRepos) -> Router {
    Router::new()
        .route("/api/admin/features/{key}", post(post_feature_flag))
        .route("/api/admin/features-batch", post(post_features_batch))
        .route("/api/admin/users/restore", post(post_users_restore))
        .route(
            "/api/admin/matches/history/{id}",
            delete(delete_match_history),
        )
        .with_state(WritesState { session, repos })
}

/// `POST /api/admin/features/:key`: activa o desactiva un feature flag (global o por servidor).
async fn post_feature_flag(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(key): Path<String>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), Some(&body), &headers)?;
    let enabled = body.get("enabled") == Some(&Value::Bool(true));
    let status_str = if enabled { "activado" } else { "desactivado" };
    let log_level = if enabled { "success" } else { "warn" };

    if let Some(target_guild) = target_guild_id {
        let mut update = Map::new();
        update.insert(key.clone(), json!(enabled));
        update.insert("updatedAt".to_string(), timestamp_now());
        let update_val = Value::Object(update);

        if let Err(e) = st
            .repos
            .doc_sink
            .merge_doc("guild_settings", &target_guild, &update_val)
            .await
        {
            tracing::error!("Error al actualizar feature flag en guild {target_guild}: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": MSG_FEATURE_ERROR }),
            ));
        }

        let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
        let log_entry = json!({
            "level": log_level,
            "message": format!(
                "El servicio \"{key}\" fue {status_str} para el servidor {target_guild} desde el panel web."
            ),
            "source": "system",
            "guild_id": target_guild,
            "createdAt": timestamp_now(),
        });
        let _ = st
            .repos
            .doc_sink
            .set_doc("faceit_bot_logs", &log_id, &log_entry)
            .await;

        Ok(json_response(
            StatusCode::OK,
            &json!({ "success": true, "key": key, "enabled": enabled }),
        ))
    } else {
        if ctx.user.role != "global_admin" {
            return Err(AdminError::Forbidden(MSG_GLOBAL_ADMIN_ONLY));
        }

        let mut update = Map::new();
        update.insert(key.clone(), json!(enabled));
        update.insert("updatedAt".to_string(), timestamp_now());
        let update_val = Value::Object(update);

        if let Err(e) = st
            .repos
            .doc_sink
            .merge_doc("faceit_config", "settings", &update_val)
            .await
        {
            tracing::error!("Error al actualizar feature flag global: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": MSG_FEATURE_ERROR }),
            ));
        }

        let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
        let log_entry = json!({
            "level": log_level,
            "message": format!("El servicio \"{key}\" fue {status_str} desde el panel web."),
            "source": "system",
            "guild_id": Value::Null,
            "createdAt": timestamp_now(),
        });
        let _ = st
            .repos
            .doc_sink
            .set_doc("faceit_bot_logs", &log_id, &log_entry)
            .await;

        Ok(json_response(
            StatusCode::OK,
            &json!({ "success": true, "key": key, "enabled": enabled }),
        ))
    }
}

/// `POST /api/admin/features-batch`: actualización masiva de feature flags.
async fn post_features_batch(
    State(st): State<WritesState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    let updates_map = match body.get("updates").and_then(Value::as_object) {
        Some(m) => m,
        None => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": MSG_FEATURE_BATCH_INVALID }),
            ));
        }
    };

    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), Some(&body), &headers)?;

    if let Some(target_guild) = target_guild_id {
        let existing = st
            .repos
            .guild_settings
            .get(&target_guild)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut settings_map =
            match serde_json::to_value(&existing).unwrap_or_else(|_| Value::Object(Map::new())) {
                Value::Object(m) => m,
                _ => Map::new(),
            };

        for (k, v) in updates_map {
            let enabled = v == &Value::Bool(true);
            settings_map.insert(k.clone(), json!(enabled));
        }
        settings_map.insert("updatedAt".to_string(), timestamp_now());

        let update_val = Value::Object(settings_map.clone());
        if let Err(e) = st
            .repos
            .doc_sink
            .merge_doc("guild_settings", &target_guild, &update_val)
            .await
        {
            tracing::error!("Error al actualizar servicios por lote en guild {target_guild}: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": MSG_FEATURE_BATCH_ERROR }),
            ));
        }

        let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
        let log_entry = json!({
            "level": "info",
            "message": format!(
                "Actualización de lote de servicios ({} flags modificados) para el servidor {target_guild}.",
                updates_map.len()
            ),
            "source": "system",
            "guild_id": target_guild,
            "createdAt": timestamp_now(),
        });
        let _ = st
            .repos
            .doc_sink
            .set_doc("faceit_bot_logs", &log_id, &log_entry)
            .await;

        Ok(json_response(
            StatusCode::OK,
            &json!({ "success": true, "settings": settings_map }),
        ))
    } else {
        if ctx.user.role != "global_admin" {
            return Err(AdminError::Forbidden(MSG_GLOBAL_ADMIN_ONLY));
        }

        let existing = st.repos.global_settings.get().await;
        let mut settings_map =
            match serde_json::to_value(&existing).unwrap_or_else(|_| Value::Object(Map::new())) {
                Value::Object(m) => m,
                _ => Map::new(),
            };

        for (k, v) in updates_map {
            let enabled = v == &Value::Bool(true);
            settings_map.insert(k.clone(), json!(enabled));
        }
        settings_map.insert("updatedAt".to_string(), timestamp_now());

        let update_val = Value::Object(settings_map.clone());
        if let Err(e) = st
            .repos
            .doc_sink
            .merge_doc("faceit_config", "settings", &update_val)
            .await
        {
            tracing::error!("Error al actualizar servicios por lote globales: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": MSG_FEATURE_BATCH_ERROR }),
            ));
        }

        let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
        let log_entry = json!({
            "level": "info",
            "message": format!(
                "Actualización de lote de servicios ({} flags modificados) globales.",
                updates_map.len()
            ),
            "source": "system",
            "guild_id": Value::Null,
            "createdAt": timestamp_now(),
        });
        let _ = st
            .repos
            .doc_sink
            .set_doc("faceit_bot_logs", &log_id, &log_entry)
            .await;

        Ok(json_response(
            StatusCode::OK,
            &json!({ "success": true, "settings": settings_map }),
        ))
    }
}

/// `POST /api/admin/users/restore`: importa copia de seguridad de usuarios en Firestore.
async fn post_users_restore(
    State(st): State<WritesState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    let users_arr = match body.get("users").and_then(Value::as_array) {
        Some(arr) => arr,
        None => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": MSG_RESTORE_INVALID }),
            ));
        }
    };

    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), Some(&body), &headers)?;
    let mut restored_count = 0usize;

    for u in users_arr {
        let u_obj = match u.as_object() {
            Some(obj) => obj,
            None => continue,
        };
        let discord_id = match u_obj.get("discordId").and_then(Value::as_str) {
            Some(id) if !id.trim().is_empty() => id.trim(),
            _ => continue,
        };

        let raw_discord_id = discord_id.rsplit('_').next().unwrap_or(discord_id);
        let mut data = u_obj.clone();
        data.remove("discordId");

        let doc_id = if let Some(ref tg) = target_guild_id {
            data.insert("guild_id".to_string(), json!(tg));
            if data.contains_key("servidor") {
                data.insert("servidor".to_string(), json!(tg));
            }
            if data.contains_key("mixton_guild_id") {
                data.insert("mixton_guild_id".to_string(), json!(tg));
            }
            format!("{}_{}", tg, raw_discord_id)
        } else {
            discord_id.to_string()
        };

        if let Some(linked_at) = data.get("linkedAt") {
            if let Some(sec) = linked_at.get("_seconds").and_then(Value::as_i64) {
                let nanos = linked_at
                    .get("_nanoseconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                if let Some(dt) = chrono::DateTime::from_timestamp(sec, nanos) {
                    data.insert(
                        "linkedAt".to_string(),
                        json!({ "$timestamp": dt.to_rfc3339() }),
                    );
                }
            } else if let Some(s) = linked_at.as_str() {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                    data.insert(
                        "linkedAt".to_string(),
                        json!({ "$timestamp": dt.to_rfc3339() }),
                    );
                }
            }
        }

        if let Err(e) = st
            .repos
            .doc_sink
            .merge_doc("faceit_users", &doc_id, &Value::Object(data))
            .await
        {
            tracing::error!("Error al restaurar usuario {doc_id}: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": MSG_RESTORE_ERROR }),
            ));
        }
        restored_count += 1;
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!("Backup de usuarios importado: {restored_count} vinculaciones actualizadas."),
        "source": "system",
        "guild_id": target_guild_id.as_deref().map(|g| json!(g)).unwrap_or(Value::Null),
        "createdAt": timestamp_now(),
    });
    let _ = st
        .repos
        .doc_sink
        .set_doc("faceit_bot_logs", &log_id, &log_entry)
        .await;

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": format!("Backup importado con éxito: {restored_count} vinculaciones restauradas.")
        }),
    ))
}

/// `DELETE /api/admin/matches/history/:id`: elimina una partida del historial.
async fn delete_match_history(
    State(st): State<WritesState>,
    _ctx: AdminContext,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    if id.trim().is_empty() {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": MSG_DELETE_MATCH_NOT_FOUND }),
        ));
    }

    if let Err(e) = st
        .repos
        .doc_sink
        .delete_doc("faceit_matches_history", &id)
        .await
    {
        tracing::error!("Error al eliminar partida {id} del historial: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": MSG_DELETE_MATCH_ERROR }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!("La partida {id} fue eliminada del historial desde el panel."),
        "source": "system",
        "guild_id": Value::Null,
        "createdAt": timestamp_now(),
    });
    let _ = st
        .repos
        .doc_sink
        .set_doc("faceit_bot_logs", &log_id, &log_entry)
        .await;

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Partida eliminada del historial con éxito"
        }),
    ))
}
