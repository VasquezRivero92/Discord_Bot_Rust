//! `POST /api/lobby/wallet/recharge-yape`: recarga de saldo verificada contra el monitor Yape.
//!
//! Antes la atendía el legado Node (`routes/lobby.js` ~3761) a través del proxy. La lógica de
//! comprobación y acreditación está en `data::FirestoreClient::credit_yape_operation`, que
//! comparte el bot de Discord. Diferencias con el legado: no se reserva una operación que aún no
//! llegó (se responde 202 sin escribir nada) y se ignora `targetDiscordId`: la cuenta
//! acreditada es siempre la de la sesión.

use axum::extract::{FromRef, Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use data::{
    BalanceAdjustment, FirestoreClient, WithdrawError, WithdrawalRequested, YapeCredit, YapeError,
};
use lobby::auth::{resolve_guild_for_user, RequireUser, SessionVerifier};
use lobby::LobbyManager;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::json_response;

#[derive(Clone)]
pub struct WalletState {
    pub firestore: Arc<FirestoreClient>,
    pub auth: Arc<dyn SessionVerifier>,
    pub lobby: Option<Arc<LobbyManager>>,
    pub default_guild_id: String,
}

impl FromRef<WalletState> for Arc<dyn SessionVerifier> {
    fn from_ref(state: &WalletState) -> Self {
        state.auth.clone()
    }
}

pub fn router(state: WalletState) -> Router {
    Router::new()
        .route("/api/lobby/wallet/recharge-yape", post(recharge_yape))
        .route("/api/lobby/wallet/withdraw", post(withdraw))
        .route(
            "/api/lobby/player-balance/{discord_id}",
            get(player_balance),
        )
        .route("/api/lobby/quick-add-balance", post(quick_add_balance))
        .route(
            "/api/lobby/mixton-check-bet-status",
            post(mixton_check_bet_status),
        )
        .with_state(state)
}

#[derive(Deserialize, Default)]
struct WithdrawPayload {
    #[serde(default)]
    amount: Value,
    #[serde(rename = "phoneNumber", default)]
    phone_number: Value,
}

/// `POST /api/lobby/wallet/withdraw`: descuenta el saldo y deja una solicitud `pending` en
/// `yape_withdrawals`. La cuenta es siempre la de la sesión; el celular puede ser distinto al
/// del perfil. Las garantías contra el doble retiro están en `request_withdrawal`.
async fn withdraw(
    user: RequireUser,
    State(state): State<WalletState>,
    Json(payload): Json<WithdrawPayload>,
) -> Response {
    let Some(amount) = as_amount(&payload.amount) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Ingresa un monto válido." }),
        );
    };
    let phone = as_text(&payload.phone_number);
    if !state
        .firestore
        .allow_wallet_attempt("withdraw", &user.user_id, 5, 600, data::allow_withdraw_attempt)
        .await
    {
        return json_response(
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "error": "Demasiadas solicitudes de retiro. Espera unos minutos." }),
        );
    }
    let result = state
        .firestore
        .request_withdrawal(&user.user_id, amount, &phone, "web")
        .await;
    if let Err(WithdrawError::Data(e)) = &result {
        tracing::error!("retiro de {}: {e}", user.user_id);
    }
    let (status, body) = withdraw_response(result);
    json_response(status, &body)
}

fn withdraw_response(result: Result<WithdrawalRequested, WithdrawError>) -> (StatusCode, Value) {
    match result {
        Ok(w) => {
            let message = if w.status == data::WITHDRAW_STATUS_REVIEW {
                format!(
                    "Solicitud de retiro de S/ {:.2} registrada. Un administrador la revisará antes de enviarla por Yape al {} (celular nuevo o monto alto).",
                    w.amount, w.phone
                )
            } else {
                format!(
                    "Solicitud de retiro de S/ {:.2} registrada. Se enviará por Yape al {}.",
                    w.amount, w.phone
                )
            };
            (
                StatusCode::OK,
                json!({
                    "success": true,
                    "message": message,
                    "amount": w.amount,
                    "newBalance": w.new_balance,
                    "withdrawalId": w.id,
                    "status": w.status,
                }),
            )
        }
        Err(WithdrawError::Invalid(msg)) => (StatusCode::BAD_REQUEST, json!({ "error": msg })),
        Err(WithdrawError::Cooldown { wait_secs }) => (
            StatusCode::TOO_MANY_REQUESTS,
            json!({
                "error": format!("Ya registraste un retiro hace poco. Espera {wait_secs} s para solicitar otro."),
                "retryAfter": wait_secs,
            }),
        ),
        Err(WithdrawError::Busy) => (
            StatusCode::CONFLICT,
            json!({ "error": "Tu saldo se está actualizando. Inténtalo de nuevo en unos segundos." }),
        ),
        Err(WithdrawError::Data(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "No se pudo registrar el retiro. Intenta nuevamente." }),
        ),
    }
}

#[derive(Deserialize, Default)]
struct RechargePayload {
    #[serde(default)]
    amount: Value,
    #[serde(rename = "phoneNumber", default)]
    phone_number: Value,
    #[serde(rename = "operationNumber", default)]
    operation_number: Value,
}

/// Texto de un campo JSON que puede llegar como texto o como número.
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn as_amount(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

async fn recharge_yape(
    user: RequireUser,
    State(state): State<WalletState>,
    Json(payload): Json<RechargePayload>,
) -> Response {
    let phone: String = as_text(&payload.phone_number)
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let operation = as_text(&payload.operation_number);
    let amount = as_amount(&payload.amount);
    let (Some(amount), true, 9) = (amount, !operation.is_empty(), phone.len()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes proporcionar monto, número de operación y celular Yape válido para recibir premios." }),
        );
    };
    if !state
        .firestore
        .allow_wallet_attempt("yape", &user.user_id, 10, 600, data::allow_yape_attempt)
        .await
    {
        return json_response(
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "error": "Alcanzaste el límite de intentos. Espera 10 minutos antes de intentar otro número de operación." }),
        );
    }

    let result = state
        .firestore
        .credit_yape_operation(&operation, &user.user_id, None, amount, Some(&phone), "web")
        .await;
    if let Err(YapeError::Data(e)) = &result {
        tracing::error!("recarga Yape de {}: {e}", user.user_id);
    }
    let (status, body) = outcome_response(result);
    json_response(status, &body)
}

/// Traduce el resultado a la respuesta HTTP que espera el frontend (mismos cuerpos que el legado).
fn outcome_response(result: Result<YapeCredit, YapeError>) -> (StatusCode, Value) {
    match result {
        Ok(YapeCredit::Credited {
            operation,
            amount,
            new_balance,
        }) => (
            StatusCode::OK,
            json!({
                "success": true,
                "message": format!("¡Recarga de S/ {amount:.2} acreditada exitosamente con Yape!"),
                "amount": amount,
                "newBalance": new_balance,
                "operationNumber": operation,
            }),
        ),
        Ok(YapeCredit::NotYetSeen) => (
            StatusCode::ACCEPTED,
            json!({
                "pending": true,
                "message": "Aún no vemos el depósito. Inténtalo de nuevo en unos segundos.",
            }),
        ),
        Ok(YapeCredit::AlreadyCredited { own }) => (
            StatusCode::CONFLICT,
            json!({ "error": if own {
                "Esta operación ya fue acreditada en tu billetera."
            } else {
                "Esta operación ya fue utilizada para acreditar otra cuenta."
            } }),
        ),
        Ok(YapeCredit::ReservedByOther) => (
            StatusCode::CONFLICT,
            json!({ "error": "Este número de operación ya está siendo usado por otra cuenta." }),
        ),
        Err(YapeError::Invalid(msg)) => (StatusCode::BAD_REQUEST, json!({ "error": msg })),
        Err(YapeError::Data(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Error al verificar el depósito Yape. Intenta nuevamente." }),
        ),
    }
}

fn extract_guild_id(raw: &str) -> Option<String> {
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        if (k == "guildId" || k == "guild_id") && !v.is_empty() {
            return Some(v.into_owned());
        }
    }
    None
}

fn is_admin_user(user: &RequireUser) -> bool {
    user.is_admin_like()
}

/// `GET /api/lobby/player-balance/:discordId`: consulta el saldo de un jugador individual.
async fn player_balance(
    State(state): State<WalletState>,
    _user: RequireUser,
    Path(discord_id): Path<String>,
    RawQuery(q): RawQuery,
) -> Response {
    let guild_id = q.as_deref().and_then(extract_guild_id);
    match state
        .firestore
        .get_user_balance(&discord_id, guild_id.as_deref())
        .await
    {
        Ok(balance) => json_response(
            StatusCode::OK,
            &json!({
                "success": true,
                "discordId": discord_id,
                "balance": balance,
            }),
        ),
        Err(e) => {
            tracing::error!("error al consultar saldo de jugador {discord_id}: {e}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al consultar saldo." }),
            )
        }
    }
}

#[derive(Deserialize, Default)]
struct QuickAddPayload {
    #[serde(rename = "discordId", default)]
    discord_id: Value,
    #[serde(default)]
    amount: Value,
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /api/lobby/quick-add-balance`: suma saldo rápido desde el lobby con permisos de admin.
async fn quick_add_balance(
    State(state): State<WalletState>,
    user: RequireUser,
    RawQuery(q): RawQuery,
    Json(payload): Json<QuickAddPayload>,
) -> Response {
    if !is_admin_user(&user) {
        return json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "Acceso restringido a administradores." }),
        );
    }
    let discord_id = as_text(&payload.discord_id);
    let amount = as_amount(&payload.amount);
    if discord_id.is_empty() || amount.is_none() {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Faltan discordId o monto." }),
        );
    }
    let amount = amount.unwrap();
    if amount <= 0.0 {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El monto debe ser un número positivo." }),
        );
    }
    if amount.fract() != 0.0 {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "El monto debe ser en soles enteros (sin céntimos)." }),
        );
    }
    // Un admin de servidor solo recarga en su servidor; el `guildId` de la URL no lo amplía.
    let query_guild = q.as_deref().and_then(extract_guild_id);
    let guild_id = match resolve_guild_for_user(&user, query_guild.as_deref(), false) {
        Ok(g) => g.or_else(|| user.servidor.clone()),
        Err(err) => return json_response(StatusCode::FORBIDDEN, &json!({ "error": err })),
    };

    let reason = payload
        .reason
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| format!("Recarga manual por admin {}", user.username));

    match state
        .firestore
        .adjust_user_balance(
            &discord_id,
            guild_id.as_deref(),
            BalanceAdjustment::Delta(amount),
            &reason,
            &user.username,
            "lobby_quick_add",
        )
        .await
    {
        Ok(new_balance) => json_response(
            StatusCode::OK,
            &json!({
                "success": true,
                "discordId": discord_id,
                "newBalance": new_balance,
            }),
        ),
        Err(e) => {
            tracing::error!("error al recargar saldo manual para {discord_id}: {e}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "Error al recargar saldo." }),
            )
        }
    }
}

#[derive(Deserialize, Default)]
struct CheckBetPayload {
    #[serde(rename = "teamIdA")]
    team_id_a: Option<String>,
    #[serde(rename = "teamIdB")]
    team_id_b: Option<String>,
    #[serde(rename = "betAmount", default)]
    bet_amount: Value,
}

/// `POST /api/lobby/mixton-check-bet-status`: revisa si los jugadores de ambos equipos pueden pagar la apuesta.
async fn mixton_check_bet_status(
    State(state): State<WalletState>,
    user: RequireUser,
    RawQuery(q): RawQuery,
    Json(payload): Json<CheckBetPayload>,
) -> Response {
    let (Some(team_id_a), Some(team_id_b)) = (payload.team_id_a, payload.team_id_b) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes seleccionar dos equipos." }),
        );
    };
    if team_id_a.trim().is_empty() || team_id_b.trim().is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Debes seleccionar dos equipos." }),
        );
    }
    let Some(lobby) = &state.lobby else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "Lobby no configurado." }),
        );
    };

    let query_guild = q.as_deref().and_then(extract_guild_id);
    let target_guild = match resolve_guild_for_user(&user, query_guild.as_deref(), false) {
        Ok(Some(g)) => g,
        Ok(None) => state.default_guild_id.clone(),
        Err(err) => return json_response(StatusCode::FORBIDDEN, &json!({ "error": err })),
    };

    let lobby_state = lobby.get_state(&target_guild).await;
    let team_a = lobby_state.teams.iter().find(|t| t.id == team_id_a);
    let team_b = lobby_state.teams.iter().find(|t| t.id == team_id_b);

    if team_a.is_none() || team_b.is_none() {
        return json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "Equipos no encontrados." }),
        );
    }
    let team_a = team_a.unwrap();
    let team_b = team_b.unwrap();

    if team_a.captain.is_none()
        || team_a.players.len() != 4
        || team_b.captain.is_none()
        || team_b.players.len() != 4
    {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Ambos equipos deben estar completos (5 jugadores c/u)." }),
        );
    }

    let parsed_bet_amount = as_amount(&payload.bet_amount).unwrap_or(0.0).max(0.0);
    let parsed_bet_amount = (parsed_bet_amount * 100.0).round() / 100.0;

    let mut player_status_list = Vec::new();

    // Roster A: capitan + 4 jugadores
    let roster_a: Vec<_> = team_a
        .captain
        .as_ref()
        .into_iter()
        .chain(team_a.players.iter())
        .collect();
    // Roster B: capitan + 4 jugadores
    let roster_b: Vec<_> = team_b
        .captain
        .as_ref()
        .into_iter()
        .chain(team_b.players.iter())
        .collect();

    for (p, team_letter, team_obj) in roster_a
        .into_iter()
        .map(|p| (p, "A", team_a))
        .chain(roster_b.into_iter().map(|p| (p, "B", team_b)))
    {
        let balance = state
            .firestore
            .get_user_balance(&p.discord_id, Some(&target_guild))
            .await
            .unwrap_or(0.0);

        let user_doc = state
            .firestore
            .get_doc::<Value>("faceit_users", &p.discord_id)
            .await
            .ok()
            .flatten();

        let has_saved_phone = user_doc
            .as_ref()
            .and_then(|u| u.get("phoneNumber"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());

        let mut steam_id = p
            .steam_id_64
            .as_deref()
            .map(str::trim)
            .filter(|s| s.len() == 17 && s.chars().all(|c| c.is_ascii_digit()))
            .map(str::to_string);

        if steam_id.is_none() {
            steam_id = user_doc
                .as_ref()
                .and_then(|u| u.get("steam_id_64"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| s.len() == 17 && s.chars().all(|c| c.is_ascii_digit()))
                .map(str::to_string);
        }

        let has_steam = steam_id.is_some();
        let has_enough = parsed_bet_amount == 0.0 || balance >= parsed_bet_amount;
        let deficit = if has_enough {
            0.0
        } else {
            ((parsed_bet_amount - balance) * 100.0).round() / 100.0
        };

        let is_captain = team_obj
            .captain
            .as_ref()
            .is_some_and(|c| c.discord_id == p.discord_id);

        let display_name = p
            .discord_user
            .as_ref()
            .and_then(|u| {
                if !u.display_name.is_empty() {
                    Some(u.display_name.clone())
                } else if !u.username.is_empty() {
                    Some(u.username.clone())
                } else {
                    p.faceit_nickname.clone()
                }
            })
            .or_else(|| p.faceit_nickname.clone())
            .unwrap_or_default();

        player_status_list.push(json!({
            "discordId": p.discord_id,
            "faceit_nickname": p.faceit_nickname,
            "displayName": display_name,
            "avatarURL": p.discord_user.as_ref().map(|u| u.avatar_url.clone()),
            "hasSavedPhone": has_saved_phone,
            "hasSteam": has_steam,
            "steam_id_64": steam_id,
            "team": team_letter,
            "teamName": team_obj.name,
            "isCaptain": is_captain,
            "balance": balance,
            "hasEnough": has_enough,
            "deficit": deficit,
        }));
    }

    let total_pot = ((parsed_bet_amount * 10.0) * 100.0).round() / 100.0;
    let all_ready = player_status_list
        .iter()
        .all(|p| p.get("hasEnough").and_then(Value::as_bool).unwrap_or(false));

    let missing_steam_players: Vec<_> = player_status_list
        .iter()
        .filter(|p| !p.get("hasSteam").and_then(Value::as_bool).unwrap_or(false))
        .map(|p| {
            json!({
                "discordId": p.get("discordId"),
                "faceit_nickname": p.get("faceit_nickname"),
                "displayName": p.get("displayName"),
            })
        })
        .collect();

    json_response(
        StatusCode::OK,
        &json!({
            "betAmount": parsed_bet_amount,
            "totalPot": total_pot,
            "allReady": all_ready,
            "teamA": { "id": team_a.id, "name": team_a.name },
            "teamB": { "id": team_b.id, "name": team_b.name },
            "players": player_status_list,
            "missingSteamPlayers": missing_steam_players,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use data::DataError;

    #[test]
    fn acreditada_responde_200_con_el_nuevo_saldo() {
        let (status, body) = outcome_response(Ok(YapeCredit::Credited {
            operation: "21740348".into(),
            amount: 5.0,
            new_balance: 7.5,
        }));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["newBalance"], 7.5);
        assert_eq!(body["operationNumber"], "21740348");
        assert_eq!(
            body["message"],
            "¡Recarga de S/ 5.00 acreditada exitosamente con Yape!"
        );
    }

    #[test]
    fn pendiente_responde_202_sin_error() {
        let (status, body) = outcome_response(Ok(YapeCredit::NotYetSeen));
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["pending"], true);
        assert!(body.get("error").is_none());
    }

    #[test]
    fn operacion_repetida_o_ajena_responde_409() {
        let (own, b1) = outcome_response(Ok(YapeCredit::AlreadyCredited { own: true }));
        let (other, b2) = outcome_response(Ok(YapeCredit::AlreadyCredited { own: false }));
        let (reserved, _) = outcome_response(Ok(YapeCredit::ReservedByOther));
        assert_eq!(
            (own, other, reserved),
            (StatusCode::CONFLICT, StatusCode::CONFLICT, StatusCode::CONFLICT)
        );
        assert_ne!(b1["error"], b2["error"]);
    }

    #[test]
    fn errores_de_validacion_son_400_y_los_de_datos_500_sin_detalle() {
        let (status, body) = outcome_response(Err(YapeError::Invalid("monto distinto".into())));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "monto distinto");
        let (status, body) = outcome_response(Err(YapeError::Data(DataError::Firestore(
            "detalle interno".into(),
        ))));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body["error"].as_str().unwrap().contains("interno"));
    }

    #[test]
    fn retiro_registrado_responde_200_pendiente_con_nuevo_saldo() {
        let (status, body) = withdraw_response(Ok(WithdrawalRequested {
            id: "wd_1".into(),
            amount: 10.0,
            phone: "987654321".into(),
            new_balance: 2.5,
            status: "pending".into(),
        }));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "pending");
        assert_eq!(body["newBalance"], 2.5);
        assert_eq!(body["withdrawalId"], "wd_1");
    }

    #[test]
    fn retiro_en_revision_lo_dice_en_estado_y_mensaje() {
        let (status, body) = withdraw_response(Ok(WithdrawalRequested {
            id: "wd_2".into(),
            amount: 300.0,
            phone: "911111111".into(),
            new_balance: 0.0,
            status: data::WITHDRAW_STATUS_REVIEW.into(),
        }));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "review");
        assert!(body["message"].as_str().unwrap().contains("administrador"));
    }

    #[test]
    fn errores_de_retiro_mapean_a_400_429_409_y_500_sin_detalle() {
        let (s, _) = withdraw_response(Err(WithdrawError::Invalid("saldo".into())));
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, b) = withdraw_response(Err(WithdrawError::Cooldown { wait_secs: 30 }));
        assert_eq!((s, b["retryAfter"].as_u64()), (StatusCode::TOO_MANY_REQUESTS, Some(30)));
        let (s, _) = withdraw_response(Err(WithdrawError::Busy));
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, b) = withdraw_response(Err(WithdrawError::Data(DataError::Firestore(
            "detalle interno".into(),
        ))));
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!b["error"].as_str().unwrap().contains("interno"));
    }

    #[test]
    fn monto_acepta_numero_y_texto() {
        assert_eq!(as_amount(&json!(5)), Some(5.0));
        assert_eq!(as_amount(&json!("5,5")), Some(5.5));
        assert_eq!(as_amount(&json!("x")), None);
        assert_eq!(as_amount(&Value::Null), None);
    }
}
