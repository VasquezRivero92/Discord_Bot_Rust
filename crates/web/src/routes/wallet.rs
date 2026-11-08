//! `POST /api/lobby/wallet/recharge-yape`: recarga de saldo verificada contra el monitor Yape.
//!
//! Antes la atendía el legado Node (`routes/lobby.js` ~3761) a través del proxy. La lógica de
//! comprobación y acreditación está en `data::FirestoreClient::credit_yape_operation`, que
//! comparte el bot de Discord. Diferencias con el legado: no se reserva una operación que aún no
//! llegó (se responde 202 sin escribir nada) y se ignora `targetDiscordId`: la cuenta
//! acreditada es siempre la de la sesión.

use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use data::{FirestoreClient, WithdrawError, WithdrawalRequested, YapeCredit, YapeError};
use lobby::auth::{RequireUser, SessionVerifier};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::json_response;

#[derive(Clone)]
pub struct WalletState {
    pub firestore: Arc<FirestoreClient>,
    pub auth: Arc<dyn SessionVerifier>,
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
    if !data::allow_withdraw_attempt(&user.user_id) {
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
        Ok(w) => (
            StatusCode::OK,
            json!({
                "success": true,
                "message": format!(
                    "Solicitud de retiro de S/ {:.2} registrada. Se enviará por Yape al {}.",
                    w.amount, w.phone
                ),
                "amount": w.amount,
                "newBalance": w.new_balance,
                "withdrawalId": w.id,
                "status": "pending",
            }),
        ),
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
    if !data::allow_yape_attempt(&user.user_id) {
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
        }));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "pending");
        assert_eq!(body["newBalance"], 2.5);
        assert_eq!(body["withdrawalId"], "wd_1");
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
