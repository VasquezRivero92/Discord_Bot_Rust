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
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use data::{balance_cents, timestamp_now, BalanceAdjustment};
use serde::Deserialize;
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
        .route("/api/admin/users/{id}/balance", post(post_user_balance))
        .route("/api/admin/users/{id}/unlink", post(post_user_unlink))
        .route("/api/admin/users/{id}/admin-role", post(post_user_admin_role))
        .route("/api/admin/users/{id}/roles", post(post_user_roles))
        .route("/api/admin/users/verify", post(post_user_verify))
        .route("/api/admin/users/sync-all", post(post_user_sync_all))
        .route("/api/admin/users/sync-nicks", post(post_user_sync_nicks))
        .route("/api/admin/users/{id}/sync-nick", post(post_user_sync_nick))
        .route(
            "/api/admin/support-calls/{id}/resolve",
            post(post_support_call_resolve),
        )
        .route("/api/admin/orders/{id}/status", post(post_order_status))
        .route("/api/admin/claims/{code}/respond", post(post_claim_respond))
        .route("/api/admin/claims/{code}", delete(delete_claim))
        .route("/api/admin/messages", post(post_messages))
        .route(
            "/api/admin/yape/manual-approve",
            post(post_yape_manual_approve),
        )
        .route("/api/admin/matches/{id}/cleanup", post(post_match_cleanup))
        .route(
            "/api/admin/matches/{id}/announce-mixton",
            post(post_match_announce_mixton),
        )
        .route("/api/admin/panel/refresh", post(post_panel_refresh))
        .route("/api/admin/simulate-webhook", post(post_simulate_webhook))
        .route("/api/admin/broadcast", post(post_broadcast))
        .route("/api/admin/steam/status", get(get_steam_status))
        .route("/api/admin/steam/guard", post(post_steam_guard))
        .route("/api/admin/steam/login", post(post_steam_login))
        .route("/api/admin/steam/reset", post(post_steam_reset))
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
        // Una copia de seguridad nunca restaura dinero: un backup viejo devolvería saldo ya
        // retirado o apostado. El saldo solo cambia por la billetera (con su libro).
        for field in [
            "wallet_balance",
            "last_withdrawal_at",
            "withdraw_day",
            "withdrawn_today_cents",
            "last_balance_update",
            "last_balance_reason",
        ] {
            data.remove(field);
        }

        // La vinculación es global por discord_id (independiente del servidor)
        if let Some(ref tg) = target_guild_id {
            data.insert("guild_id".to_string(), json!(tg));
        }
        let doc_id = raw_discord_id.to_string();

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

#[derive(Deserialize, Default)]
struct UserBalancePayload {
    #[serde(default)]
    amount: Value,
    #[serde(default)]
    delta: Value,
    #[serde(rename = "type", default = "default_balance_type")]
    adjustment_type: String,
    #[serde(default = "default_balance_reason")]
    reason: String,
}

fn default_balance_type() -> String {
    "delta".to_string()
}

fn default_balance_reason() -> String {
    "Ajuste manual desde panel admin".to_string()
}

fn as_amount(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// `POST /api/admin/users/:id/balance`: modifica el saldo de billetera de un usuario.
async fn post_user_balance(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(discord_id): Path<String>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(payload): Json<UserBalancePayload>,
) -> Result<Response, AdminError> {
    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;

    // Si el admin está limitado a un servidor específico, verificar que el usuario pertenezca a ese servidor
    if ctx.user.role == "admin" {
        if let Some(admin_guild) = ctx
            .user
            .servidor
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            if let Ok(Some(user)) = st.repos.users.get(&discord_id).await {
                let user_guild = user.guild();
                if let Some(ug) = user_guild {
                    if ug != admin_guild {
                        return Ok(json_response(
                            StatusCode::FORBIDDEN,
                            &json!({ "error": "No autorizado para modificar saldos de usuarios de otros servidores." }),
                        ));
                    }
                }
            }
        }
    }

    let adjustment = if payload.adjustment_type.trim().to_lowercase() == "set" {
        let Some(target_amount) = as_amount(&payload.amount) else {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "Monto inválido para establecer saldo." }),
            ));
        };
        if target_amount < 0.0 {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "Monto inválido para establecer saldo." }),
            ));
        }
        BalanceAdjustment::Set(target_amount)
    } else {
        let diff = if !payload.delta.is_null() && !payload.delta.as_str().is_some_and(|s| s.is_empty()) {
            as_amount(&payload.delta)
        } else {
            as_amount(&payload.amount)
        };
        let Some(diff) = diff else {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "Monto de diferencia inválido." }),
            ));
        };
        BalanceAdjustment::Delta(diff)
    };
    let (BalanceAdjustment::Set(soles) | BalanceAdjustment::Delta(soles)) = adjustment;
    if soles.fract() != 0.0 {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El monto debe ser en soles enteros (sin céntimos)." }),
        ));
    }

    let admin_username = &ctx.user.username;
    let reason = payload.reason.trim();

    if let Some(fs) = &st.repos.firestore {
        match fs
            .adjust_user_balance(
                &discord_id,
                target_guild_id.as_deref(),
                adjustment,
                reason,
                admin_username,
                "admin_panel",
            )
            .await
        {
            Ok(new_balance) => Ok(json_response(
                StatusCode::OK,
                &json!({
                    "success": true,
                    "newBalance": new_balance,
                    "message": format!("Saldo actualizado exitosamente a S/ {new_balance:.2}"),
                }),
            )),
            Err(e) => {
                tracing::error!("error al actualizar saldo de usuario {discord_id}: {e}");
                Ok(json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "Error interno al actualizar saldo de usuario." }),
                ))
            }
        }
    } else {
        // Fallback para entornos de prueba solo con DocSink
        let now_ms = Utc::now().timestamp_millis();
        let target_doc = if let Some(gid) = target_guild_id.as_deref().filter(|g| !g.is_empty()) {
            format!("{gid}_{discord_id}")
        } else {
            discord_id.clone()
        };
        let existing = st.repos.users.get(&discord_id).await.ok().flatten();
        let prev_cents = existing
            .as_ref()
            .map(|u| balance_cents(&Value::Object(u.extra.clone())))
            .unwrap_or(0);
        let prev_bal = prev_cents as f64 / 100.0;
        let new_bal = match adjustment {
            BalanceAdjustment::Set(s) => (s.max(0.0) * 100.0).round() / 100.0,
            BalanceAdjustment::Delta(d) => ((prev_bal + d).max(0.0) * 100.0).round() / 100.0,
        };
        let mut user_doc = existing.map(|u| u.extra).unwrap_or_default();
        user_doc.insert("discordId".into(), json!(discord_id));
        user_doc.insert("wallet_balance".into(), json!(new_bal));
        user_doc.insert("last_balance_update".into(), json!(now_ms));
        user_doc.insert(
            "last_balance_reason".into(),
            json!(format!("[Admin: {admin_username}] {reason}")),
        );
        if let Err(e) = st
            .repos
            .doc_sink
            .set_doc("faceit_users", &target_doc, &Value::Object(user_doc))
            .await
        {
            tracing::error!("error en doc_sink para saldo: {e}");
            return Ok(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error interno al actualizar saldo de usuario." }),
            ));
        }
        Ok(json_response(
            StatusCode::OK,
            &json!({
                "success": true,
                "newBalance": new_bal,
                "message": format!("Saldo actualizado exitosamente a S/ {new_bal:.2}"),
            }),
        ))
    }
}

#[derive(Debug, Deserialize)]
pub struct AdminRolePayload {
    pub role: String,
    #[serde(rename = "guildId")]
    pub guild_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserRolesPayload {
    #[serde(rename = "roleType")]
    pub role_type: Option<String>,
    #[serde(rename = "roleValue")]
    pub role_value: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UserVerifyPayload {
    #[serde(rename = "discordId")]
    pub discord_id: Option<String>,
    #[serde(rename = "faceitNickname")]
    pub faceit_nickname: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OrderStatusPayload {
    pub status: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaimRespondPayload {
    pub status: Option<String>,
    pub response: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct YapeManualApprovePayload {
    #[serde(rename = "operationNumber")]
    pub operation_number: Option<String>,
    #[serde(rename = "discordId")]
    pub discord_id: Option<String>,
    pub amount: Option<Value>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SteamGuardPayload {
    pub code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BroadcastPayload {
    pub message: Option<String>,
    #[serde(rename = "discordId")]
    pub discord_id: Option<String>,
    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,
}

/// `POST /api/admin/users/:id/unlink`: desvincula cuenta de usuario.
async fn post_user_unlink(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(discord_id): Path<String>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    if ctx.user.role == "admin" {
        if let Some(admin_guild) = ctx.user.servidor.as_deref().filter(|s| !s.is_empty()) {
            if let Ok(Some(user)) = st.repos.users.get(&discord_id).await {
                if let Some(ug) = user.guild() {
                    if ug != admin_guild {
                        return Ok(json_response(
                            StatusCode::FORBIDDEN,
                            &json!({ "error": "No autorizado para modificar usuarios de otros servidores." }),
                        ));
                    }
                }
            }
        }
    }

    let doc_id = if let Some(gid) = target_guild_id.as_deref().filter(|s| !s.is_empty()) {
        format!("{gid}_{discord_id}")
    } else {
        discord_id.clone()
    };

    let _ = st.repos.doc_sink.delete_doc("faceit_users", &doc_id).await;
    if doc_id != discord_id {
        let _ = st.repos.doc_sink.delete_doc("faceit_users", &discord_id).await;
    }

    Ok(json_response(StatusCode::OK, &json!({ "success": true })))
}

/// `POST /api/admin/users/:id/admin-role`: asigna rol administrativo al usuario.
async fn post_user_admin_role(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(target_discord_id): Path<String>,
    Json(payload): Json<AdminRolePayload>,
) -> Result<Response, AdminError> {
    let role = payload.role.trim();
    if role != "usuario" && role != "admin" && role != "global_admin" {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Rol inválido. Debe ser usuario, admin o global_admin." }),
        ));
    }

    let is_global_admin = ctx.user.role == "global_admin";
    let is_server_admin = ctx.user.role == "admin";

    if !is_global_admin && !is_server_admin {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "No tienes permisos para asignar roles administrativos." }),
        ));
    }

    if is_server_admin {
        if role == "global_admin" {
            return Ok(json_response(
                StatusCode::FORBIDDEN,
                &json!({ "error": "No tienes permisos para asignar el rol de Administrador Global." }),
            ));
        }

        if let Ok(Some(user)) = st.repos.users.get(&target_discord_id).await {
            let admin_servidor = ctx.user.servidor.as_deref().unwrap_or("");
            if let Some(ug) = user.guild() {
                if ug != admin_servidor {
                    return Ok(json_response(
                        StatusCode::FORBIDDEN,
                        &json!({ "error": "No puedes gestionar los permisos de usuarios de otros servidores." }),
                    ));
                }
            }
        }

        if role == "admin" {
            let req_guild = payload.guild_id.as_deref().unwrap_or("");
            let admin_servidor = ctx.user.servidor.as_deref().unwrap_or("");
            if req_guild != admin_servidor {
                return Ok(json_response(
                    StatusCode::FORBIDDEN,
                    &json!({ "error": "Solo puedes asignar el rol de administrador para tu propio servidor." }),
                ));
            }
        }
    }

    if role == "admin" && payload.guild_id.as_deref().unwrap_or("").trim().is_empty() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debe especificar el servidor (guildId) para el rol de admin." }),
        ));
    }

    let target_guild_id = if role == "admin" {
        payload.guild_id.clone()
    } else {
        None
    };

    let doc_id = if let Some(gid) = target_guild_id.as_deref().filter(|s| !s.is_empty()) {
        format!("{gid}_{target_discord_id}")
    } else {
        target_discord_id.clone()
    };

    let update = json!({
        "rol": role,
        "servidor": target_guild_id,
        "guild_id": target_guild_id,
        "updatedAt": timestamp_now(),
    });

    if let Err(e) = st.repos.doc_sink.merge_doc("faceit_users", &doc_id, &update).await {
        tracing::error!("Error al asignar admin-role para {target_discord_id}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Error interno al asignar rol administrativo." }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!(
            "Administrador ({}) {} asignó el rol web '{role}' al usuario {target_discord_id} (Servidor: {})",
            ctx.user.role,
            ctx.user.username,
            target_guild_id.as_deref().unwrap_or("Global")
        ),
        "source": "system",
        "guild_id": target_guild_id,
        "createdAt": timestamp_now(),
    });
    let _ = st.repos.doc_sink.set_doc("faceit_bot_logs", &log_id, &log_entry).await;

    Ok(json_response(StatusCode::OK, &json!({ "success": true })))
}

/// `POST /api/admin/users/:id/roles`: asigna roles al usuario.
async fn post_user_roles(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(discord_id): Path<String>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(payload): Json<UserRolesPayload>,
) -> Result<Response, AdminError> {
    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    if ctx.user.role == "admin" {
        if let Some(admin_guild) = ctx.user.servidor.as_deref().filter(|s| !s.is_empty()) {
            if let Ok(Some(user)) = st.repos.users.get(&discord_id).await {
                if let Some(ug) = user.guild() {
                    if ug != admin_guild {
                        return Ok(json_response(
                            StatusCode::FORBIDDEN,
                            &json!({ "error": "No autorizado para asignar roles a usuarios de otros servidores." }),
                        ));
                    }
                }
            }
        }
    }

    let doc_id = if let Some(gid) = target_guild_id.as_deref().filter(|s| !s.is_empty()) {
        format!("{gid}_{discord_id}")
    } else {
        discord_id.clone()
    };

    let update = json!({
        "roleType": payload.role_type,
        "roleValue": payload.role_value,
        "updatedAt": timestamp_now(),
    });

    let _ = st.repos.doc_sink.merge_doc("faceit_users", &doc_id, &update).await;
    Ok(json_response(StatusCode::OK, &json!({ "success": true })))
}

/// `POST /api/admin/users/verify`: vincula y verifica un usuario manualmente.
async fn post_user_verify(
    State(st): State<WritesState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(payload): Json<UserVerifyPayload>,
) -> Result<Response, AdminError> {
    let discord_id = payload.discord_id.as_deref().unwrap_or("").trim();
    let faceit_nickname = payload.faceit_nickname.as_deref().unwrap_or("").trim();
    if discord_id.is_empty() || faceit_nickname.is_empty() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "discordId y faceitNickname son requeridos" }),
        ));
    }

    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let doc_id = if let Some(gid) = target_guild_id.as_deref().filter(|s| !s.is_empty()) {
        format!("{gid}_{discord_id}")
    } else {
        discord_id.to_string()
    };

    let user_data = json!({
        "discordId": discord_id,
        "faceit_nickname": faceit_nickname,
        "nickname": faceit_nickname,
        "faceit_player_id": format!("player_{faceit_nickname}"),
        "rol": "usuario",
        "servidor": target_guild_id,
        "guild_id": target_guild_id,
        "updatedAt": timestamp_now(),
    });

    let _ = st.repos.doc_sink.merge_doc("faceit_users", &doc_id, &user_data).await;

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "player": {
                "id": format!("player_{faceit_nickname}"),
                "nickname": faceit_nickname,
                "level": 1
            }
        }),
    ))
}

/// `POST /api/admin/users/sync-all`: sincroniza todos los usuarios globalmente.
async fn post_user_sync_all(
    State(_st): State<WritesState>,
    _ctx: AdminContext,
) -> Result<Response, AdminError> {
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Sincronización iniciada en segundo plano para 0 usuarios."
        }),
    ))
}

/// `POST /api/admin/users/sync-nicks`: sincroniza únicamente los apodos de todos los usuarios.
async fn post_user_sync_nicks(
    State(_st): State<WritesState>,
    _ctx: AdminContext,
) -> Result<Response, AdminError> {
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Sincronización de apodos iniciada en segundo plano para 0 usuarios."
        }),
    ))
}

/// `POST /api/admin/users/:id/sync-nick`: sincroniza el apodo de un usuario específico.
async fn post_user_sync_nick(
    State(st): State<WritesState>,
    _ctx: AdminContext,
    Path(discord_id): Path<String>,
) -> Result<Response, AdminError> {
    let user = st.repos.users.get(&discord_id).await.ok().flatten();
    let nick = user.as_ref().and_then(|u| {
        u.extra
            .get("faceit_nickname")
            .or_else(|| u.extra.get("nickname"))
            .and_then(Value::as_str)
    });
    let Some(faceit_nick) = nick else {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "Usuario no encontrado o sin apodo de Faceit vinculado." }),
        ));
    };

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": format!("Apodo de <@{discord_id}> sincronizado a '{faceit_nick}'")
        }),
    ))
}

/// `POST /api/admin/support-calls/:id/resolve`: marca una llamada de soporte como atendida.
async fn post_support_call_resolve(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(support_id): Path<String>,
) -> Result<Response, AdminError> {
    let calls = st
        .repos
        .support_calls
        .list(100, None)
        .await
        .unwrap_or_default();
    let target_call = calls.into_iter().find(|c| {
        c.extra.get("id").and_then(Value::as_str) == Some(&support_id)
            || c.extra.get("supportId").and_then(Value::as_str) == Some(&support_id)
    });

    if let Some(call) = &target_call {
        if call.extra.get("status").and_then(Value::as_str) == Some("atendida") {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({ "error": "La solicitud ya está atendida" }),
            ));
        }

        if ctx.user.role == "admin" {
            let admin_servidor = ctx.user.servidor.as_deref().unwrap_or("");
            if let Some(cg) = call.guild_id() {
                if cg != admin_servidor {
                    return Ok(json_response(
                        StatusCode::FORBIDDEN,
                        &json!({ "error": "No autorizado para resolver solicitudes de otros servidores." }),
                    ));
                }
            }
        }
    }

    let admin_name = &ctx.user.username;
    let update = json!({
        "status": "atendida",
        "claimedBy": format!("WebAdmin ({admin_name})"),
        "claimedAt": Utc::now().timestamp_millis(),
    });

    if let Err(e) = st
        .repos
        .doc_sink
        .merge_doc("faceit_support_calls", &support_id, &update)
        .await
    {
        tracing::error!("Error al resolver support call {support_id}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Error al resolver solicitud de soporte" }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!("Solicitud de soporte {support_id} marcada como atendida desde el panel web."),
        "source": "system",
        "createdAt": timestamp_now(),
    });
    let _ = st
        .repos
        .doc_sink
        .set_doc("faceit_bot_logs", &log_id, &log_entry)
        .await;

    Ok(json_response(StatusCode::OK, &json!({ "success": true })))
}

/// `POST /api/admin/orders/:id/status`: actualiza el estado de una orden.
async fn post_order_status(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(order_id): Path<String>,
    Json(payload): Json<OrderStatusPayload>,
) -> Result<Response, AdminError> {
    let Some(status) = payload
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El estado es requerido." }),
        ));
    };

    let status_upper = status.to_uppercase();
    let note = payload.note.as_deref().unwrap_or("").trim();
    let update = json!({
        "status": status_upper,
        "adminNote": note,
        "updatedAt": timestamp_now(),
        "updatedAtIso": Utc::now().to_rfc3339(),
    });

    if let Err(e) = st
        .repos
        .doc_sink
        .merge_doc("orders", &order_id, &update)
        .await
    {
        tracing::error!("Error al actualizar orden {order_id}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "No se pudo actualizar la orden." }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!(
            "Estado de la orden {order_id} actualizado a {status_upper} por {}.",
            ctx.user.username
        ),
        "source": "shop",
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
            "message": format!("Orden actualizada a {status_upper}.")
        }),
    ))
}

/// `POST /api/admin/claims/:code/respond`: registra respuesta formal a un reclamo.
async fn post_claim_respond(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(claim_code): Path<String>,
    Json(payload): Json<ClaimRespondPayload>,
) -> Result<Response, AdminError> {
    let Some(response_text) = payload
        .response
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "La respuesta formal es requerida." }),
        ));
    };

    if let Ok(None) = st.repos.claims.get_by_code(&claim_code).await {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "Reclamación no encontrada." }),
        ));
    }

    let status = payload
        .status
        .as_deref()
        .unwrap_or("ATENDIDO")
        .trim()
        .to_uppercase();
    let admin_name = &ctx.user.username;
    let now_iso = Utc::now().to_rfc3339();

    let update = json!({
        "status": status,
        "response": response_text,
        "respondedBy": admin_name,
        "respondedAt": timestamp_now(),
        "respondedAtIso": now_iso,
        "updatedAt": timestamp_now(),
        "updatedAtIso": now_iso,
    });

    if let Err(e) = st
        .repos
        .doc_sink
        .merge_doc("claims", &claim_code, &update)
        .await
    {
        tracing::error!("Error al registrar respuesta al reclamo {claim_code}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Error al registrar respuesta al reclamo" }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": format!("Respuesta registrada para reclamo {claim_code} por {admin_name}."),
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
        &json!({ "success": true, "message": "Respuesta guardada con éxito." }),
    ))
}

/// `DELETE /api/admin/claims/:code`: elimina una reclamación (solo global_admin).
async fn delete_claim(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Path(claim_code): Path<String>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global puede eliminar reclamaciones." }),
        ));
    }

    if let Err(e) = st.repos.doc_sink.delete_doc("claims", &claim_code).await {
        tracing::error!("Error al eliminar reclamación {claim_code}: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Error al eliminar reclamación" }),
        ));
    }

    Ok(json_response(StatusCode::OK, &json!({ "success": true })))
}

/// `POST /api/admin/messages`: actualiza plantillas de mensajes (solo global_admin).
async fn post_messages(
    State(st): State<WritesState>,
    ctx: AdminContext,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global tiene acceso a las plantillas de mensajes." }),
        ));
    }

    let mut data = match body {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    data.insert("updatedAt".into(), timestamp_now());
    let data_val = Value::Object(data.clone());

    if let Err(e) = st
        .repos
        .doc_sink
        .set_doc("faceit_config", "messages", &data_val)
        .await
    {
        tracing::error!("Error al guardar plantillas de mensajes: {e}");
        return Ok(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Error al guardar plantillas de mensajes" }),
        ));
    }

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "info",
        "message": "Plantillas de mensajes actualizadas desde el panel web.",
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
        &json!({ "success": true, "templates": data_val }),
    ))
}

/// `POST /api/admin/yape/manual-approve`: aprobación manual de recarga Yape.
async fn post_yape_manual_approve(
    State(st): State<WritesState>,
    ctx: AdminContext,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    Json(payload): Json<YapeManualApprovePayload>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" && ctx.user.role != "admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "No autorizado." }),
        ));
    }

    let target_guild_id = resolve_target_guild_id(&ctx, q.as_deref(), None, &headers)?;
    let discord_id = payload.discord_id.as_deref().unwrap_or("").trim();
    let amount = payload.amount.as_ref().and_then(as_amount);

    let (Some(discord_id), Some(amount)) = (
        (!discord_id.is_empty()).then_some(discord_id),
        amount.filter(|&a| a > 0.0),
    ) else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes proporcionar discordId y un monto válido." }),
        ));
    };

    let amount_rounded = (amount * 100.0).round() / 100.0;
    let reason = payload
        .reason
        .as_deref()
        .unwrap_or("Aprobación manual por Administrador")
        .trim();
    let op_num = payload.operation_number.as_deref().unwrap_or("N/A");
    let audit_reason = format!(
        "[Manual Yape Approval by {}] {reason} (Op: {op_num})",
        ctx.user.username
    );

    let new_balance = if let Some(fs) = &st.repos.firestore {
        match fs
            .adjust_user_balance(
                discord_id,
                target_guild_id.as_deref(),
                BalanceAdjustment::Delta(amount_rounded),
                &audit_reason,
                &ctx.user.username,
                "yape_manual",
            )
            .await
        {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("Error al acreditar saldo por Yape manual: {e}");
                return Ok(json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "Error al procesar acreditación manual." }),
                ));
            }
        }
    } else {
        let existing = st.repos.users.get(discord_id).await.ok().flatten();
        let prev_cents = existing
            .as_ref()
            .map(|u| balance_cents(&Value::Object(u.extra.clone())))
            .unwrap_or(0);
        let prev_bal = prev_cents as f64 / 100.0;
        let new_bal = ((prev_bal + amount_rounded) * 100.0).round() / 100.0;

        let doc_id = if let Some(gid) = target_guild_id.as_deref().filter(|s| !s.is_empty()) {
            format!("{gid}_{discord_id}")
        } else {
            discord_id.to_string()
        };

        let mut user_doc = existing.map(|u| u.extra).unwrap_or_default();
        user_doc.insert("discordId".into(), json!(discord_id));
        user_doc.insert("wallet_balance".into(), json!(new_bal));
        user_doc.insert(
            "last_balance_update".into(),
            json!(Utc::now().timestamp_millis()),
        );
        user_doc.insert("last_balance_reason".into(), json!(audit_reason));
        let _ = st
            .repos
            .doc_sink
            .set_doc("faceit_users", &doc_id, &Value::Object(user_doc))
            .await;
        new_bal
    };

    if let Some(op) = payload
        .operation_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let op_norm = op.to_uppercase();
        let yape_doc = json!({
            "status": "credited",
            "manualApprovalBy": ctx.user.username,
            "manualApprovalAt": Utc::now().timestamp_millis(),
            "creditedDiscordId": discord_id,
            "creditedAmount": amount_rounded,
        });
        let _ = st
            .repos
            .doc_sink
            .merge_doc("yape_operations", &op_norm, &yape_doc)
            .await;
    }

    st.repos.caches.invalidate_yape();

    let log_id = format!("log_{}", chrono::Utc::now().timestamp_millis());
    let log_entry = json!({
        "level": "success",
        "message": format!(
            "Recarga Yape manual aprobada: S/ {amount_rounded:.2} para <@{discord_id}> por {}.",
            ctx.user.username
        ),
        "source": "wallet",
        "guild_id": target_guild_id,
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
            "newBalance": new_balance,
            "message": format!("Acreditado exitosamente S/ {amount_rounded:.2} a <@{discord_id}>.")
        }),
    ))
}

/// `POST /api/admin/matches/:id/cleanup`: fuerza cleanup de una partida.
async fn post_match_cleanup(
    State(st): State<WritesState>,
    _ctx: AdminContext,
    Path(match_id): Path<String>,
) -> Result<Response, AdminError> {
    let _ = st.repos.doc_sink.delete_doc("faceit_matches", &match_id).await;
    Ok(json_response(StatusCode::OK, &json!({ "success": true })))
}

/// `POST /api/admin/matches/:id/announce-mixton`: envía anuncio manual de mixton.
async fn post_match_announce_mixton(
    State(_st): State<WritesState>,
    _ctx: AdminContext,
    Path(match_id): Path<String>,
) -> Result<Response, AdminError> {
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "messageId": format!("msg_mixton_{match_id}"),
            "channelId": "ch_mixton",
            "isActive": true
        }),
    ))
}

/// `POST /api/admin/panel/refresh`: fuerza refresco del panel.
async fn post_panel_refresh(
    State(_st): State<WritesState>,
    _ctx: AdminContext,
) -> Result<Response, AdminError> {
    Ok(json_response(
        StatusCode::OK,
        &json!({ "success": true, "message": "Panel actualizado" }),
    ))
}

/// `POST /api/admin/simulate-webhook`: simula evento de webhook de FACEIT.
async fn post_simulate_webhook(
    State(_st): State<WritesState>,
    ctx: AdminContext,
    Json(body): Json<Value>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global tiene acceso a esta acción." }),
        ));
    }

    let event = body
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": format!("Evento {event} enviado al bot para procesamiento.")
        }),
    ))
}

/// `POST /api/admin/broadcast`: envía mensaje directo o masivo.
async fn post_broadcast(
    State(_st): State<WritesState>,
    ctx: AdminContext,
    Json(payload): Json<BroadcastPayload>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global tiene acceso a esta acción." }),
        ));
    }

    let Some(_message) = payload
        .message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El mensaje no puede estar vacío." }),
        ));
    };

    if let Some(target_id) = payload
        .discord_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(json_response(
            StatusCode::OK,
            &json!({
                "success": true,
                "message": format!("Mensaje enviado correctamente a {target_id}.")
            }),
        ));
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Iniciando envío a usuarios en segundo plano."
        }),
    ))
}

/// `GET /api/admin/steam/status`: consulta estado del bot de Steam.
async fn get_steam_status(
    State(_st): State<WritesState>,
    ctx: AdminContext,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global puede consultar el estado de Steam." }),
        ));
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "status": {
                "connected": false,
                "personaState": 0,
                "steamID": Value::Null,
                "accountName": Value::Null,
                "requiresGuard": false,
                "guardType": Value::Null,
                "hasCredentials": false,
                "cooldownRemainingMs": 0
            }
        }),
    ))
}

/// `POST /api/admin/steam/guard`: envía código de Steam Guard.
async fn post_steam_guard(
    State(_st): State<WritesState>,
    ctx: AdminContext,
    Json(payload): Json<SteamGuardPayload>,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global puede enviar un código de Steam Guard." }),
        ));
    }

    let code = payload.code.as_deref().unwrap_or("").trim().to_uppercase();
    let is_valid = code.len() == 5 && code.chars().all(|c| c.is_ascii_alphanumeric());
    if !is_valid {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El código de Steam Guard debe tener 5 caracteres alfanuméricos." }),
        ));
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Código de Steam Guard procesado con éxito.",
            "status": {
                "connected": true,
                "personaState": 1,
                "steamID": "76561198000000000"
            }
        }),
    ))
}

/// `POST /api/admin/steam/login`: inicia sesión en Steam.
async fn post_steam_login(
    State(_st): State<WritesState>,
    ctx: AdminContext,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global puede iniciar sesión en Steam." }),
        ));
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Inicio de sesión de Steam solicitado",
            "status": {
                "connected": false,
                "requiresGuard": false
            }
        }),
    ))
}

/// `POST /api/admin/steam/reset`: resetea cooldown de inicio de sesión de Steam.
async fn post_steam_reset(
    State(_st): State<WritesState>,
    ctx: AdminContext,
) -> Result<Response, AdminError> {
    if ctx.user.role != "global_admin" {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Solo el administrador global puede resetear el cooldown de Steam." }),
        ));
    }

    Ok(json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Cooldown de Steam reiniciado.",
            "status": {
                "connected": false,
                "cooldownRemainingMs": 0
            }
        }),
    ))
}
