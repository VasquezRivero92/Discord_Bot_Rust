//! Solicitudes de retiro de saldo (`yape_withdrawals`).
//!
//! El retiro se registra como solicitud `pending`; otro proyecto lee las pendientes y hace el
//! Yape. Lo que este módulo garantiza es que **nunca se registren retiros que sumen más que el
//! saldo**:
//!
//! * El documento del usuario se lee con su `update_time` y se actualiza en una transacción con
//!   la precondición `UpdateTime(leído)`. Cualquier escritura entre la lectura y el commit (otro
//!   retiro, una recarga, el pago de una partida) hace fallar el commit: se relee y se
//!   revalida el saldo desde cero (máximo [`MAX_RETRIES`] veces).
//! * El saldo nuevo y la solicitud se escriben en la misma transacción: o están las dos cosas o
//!   ninguna. La solicitud usa la precondición `no existe`.
//! * `last_withdrawal_at` (en el mismo documento, así que la comprobación también es atómica)
//!   impone [`COOLDOWN_MS`] entre retiros: un doble clic no genera dos solicitudes.
//! * Los montos se manejan en céntimos enteros, nunca con `f64` para comparar o restar.

use crate::error::DataError;
use crate::firestore::{decode_document, FirestoreClient};
use chrono::Utc;
use firestore::errors::FirestoreError;
use firestore::{FirestoreInstant, FirestoreTimestamp, FirestoreWritePrecondition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const WITHDRAWALS: &str = "yape_withdrawals";
const AUDIT_LOGS: &str = "yape_audit_logs";
const USERS: &str = "faceit_users";

/// Retiro mínimo en céntimos (S/ 1.00, igual que la recarga mínima).
pub const MIN_WITHDRAWAL_CENTS: i64 = 100;
/// Tope de lo retirado por cuenta en un día de Lima (S/ 500): acota lo que se pierde si
/// roban una sesión.
pub const DAILY_MAX_CENTS: i64 = 50_000;
/// Hasta este monto (S/ 200) un retiro al celular registrado se paga solo; por encima queda
/// en revisión manual.
pub const AUTO_PAY_MAX_CENTS: i64 = 20_000;
/// Estado que el worker de Yape paga automáticamente.
pub const STATUS_PENDING: &str = "pending";
/// Estado que el worker ignora: un administrador lo aprueba (pasándolo a `pending`) o lo
/// rechaza y devuelve el saldo.
pub const STATUS_REVIEW: &str = "review";
/// Perú no tiene horario de verano: el día de Lima es UTC-5 todo el año.
const LIMA_OFFSET_SECS: i32 = -5 * 3600;
/// Espera mínima entre dos solicitudes de la misma cuenta.
const COOLDOWN_MS: i64 = 60_000;
/// Reintentos si el documento del usuario cambió entre la lectura y el commit.
const MAX_RETRIES: usize = 4;

/// Solicitud registrada.
#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawalRequested {
    pub id: String,
    pub amount: f64,
    pub phone: String,
    pub new_balance: f64,
    /// [`STATUS_PENDING`] (se paga solo) o [`STATUS_REVIEW`] (espera a un administrador).
    pub status: String,
}

/// Error al solicitar un retiro: `Invalid` lleva un mensaje apto para mostrar al usuario.
#[derive(Debug, Clone, PartialEq)]
pub enum WithdrawError {
    /// Datos o saldo no válidos para el retiro.
    Invalid(String),
    /// Hay que esperar antes de otra solicitud.
    Cooldown { wait_secs: u64 },
    /// El saldo cambió varias veces seguidas durante el intento; reintentar.
    Busy,
    Data(DataError),
}

impl fmt::Display for WithdrawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WithdrawError::Invalid(m) => f.write_str(m),
            WithdrawError::Cooldown { wait_secs } => {
                write!(f, "espera {wait_secs} s antes de otra solicitud")
            }
            WithdrawError::Busy => f.write_str("saldo en uso, reintenta"),
            WithdrawError::Data(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WithdrawError {}

impl From<DataError> for WithdrawError {
    fn from(e: DataError) -> Self {
        WithdrawError::Data(e)
    }
}

impl From<FirestoreError> for WithdrawError {
    fn from(e: FirestoreError) -> Self {
        WithdrawError::Data(DataError::from(e))
    }
}

/// Solicitudes permitidas por usuario en [`ATTEMPT_WINDOW`], además del cooldown de la base.
const MAX_ATTEMPTS: usize = 5;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(600);

static ATTEMPTS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();

/// Registra un intento del usuario; `false` si ya agotó los de la ventana.
pub fn allow_attempt(user: &str) -> bool {
    let mut map = ATTEMPTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    map.retain(|_, v| {
        v.retain(|t| now.duration_since(*t) < ATTEMPT_WINDOW);
        !v.is_empty()
    });
    let hits = map.entry(user.to_string()).or_default();
    if hits.len() >= MAX_ATTEMPTS {
        return false;
    }
    hits.push(now);
    true
}

/// Convierte un monto en soles a céntimos exactos. Rechaza no finitos, ≤ 0 y más de 2 decimales.
pub fn to_cents(amount: f64) -> Option<i64> {
    if !amount.is_finite() || amount <= 0.0 || amount > 1_000_000.0 {
        return None;
    }
    let scaled = amount * 100.0;
    let cents = scaled.round();
    ((scaled - cents).abs() < 1e-6).then_some(cents as i64)
}

/// Celular peruano de 9 dígitos que empieza con 9.
pub fn normalize_phone(raw: &str) -> Option<String> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    (digits.len() == 9 && digits.starts_with('9')).then_some(digits)
}

fn cents_to_soles(cents: i64) -> f64 {
    cents as f64 / 100.0
}

/// Saldo del documento en céntimos (0 si falta o no es un número válido y positivo).
pub fn balance_cents(user: &serde_json::Value) -> i64 {
    let n = match user.get("wallet_balance") {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.trim().replace(',', ".").parse().ok(),
        _ => None,
    };
    match n {
        Some(b) if b.is_finite() && b > 0.0 => (b * 100.0).round() as i64,
        _ => 0,
    }
}

/// Decisión pura sobre una solicitud: devuelve el saldo restante en céntimos o el motivo.
fn check_request(
    balance_cents: i64,
    amount_cents: i64,
    last_withdrawal_ms: Option<i64>,
    now_ms: i64,
) -> Result<i64, WithdrawError> {
    if amount_cents < MIN_WITHDRAWAL_CENTS {
        return Err(WithdrawError::Invalid(format!(
            "El retiro mínimo es de S/ {:.2}.",
            cents_to_soles(MIN_WITHDRAWAL_CENTS)
        )));
    }
    if amount_cents % 100 != 0 {
        return Err(WithdrawError::Invalid(
            "Los retiros son en soles enteros (sin céntimos).".into(),
        ));
    }
    if amount_cents > balance_cents {
        return Err(WithdrawError::Invalid(format!(
            "Saldo insuficiente: tienes S/ {:.2} en tu billetera.",
            cents_to_soles(balance_cents)
        )));
    }
    if let Some(last) = last_withdrawal_ms {
        let elapsed = now_ms - last;
        if (0..COOLDOWN_MS).contains(&elapsed) {
            return Err(WithdrawError::Cooldown {
                wait_secs: ((COOLDOWN_MS - elapsed) as u64).div_ceil(1000),
            });
        }
    }
    Ok(balance_cents - amount_cents)
}

/// Día de Lima (`AAAA-MM-DD`) de un instante en milisegundos.
fn lima_day(now_ms: i64) -> String {
    let offset = chrono::FixedOffset::east_opt(LIMA_OFFSET_SECS).expect("offset válido");
    chrono::DateTime::from_timestamp_millis(now_ms)
        .unwrap_or_default()
        .with_timezone(&offset)
        .format("%Y-%m-%d")
        .to_string()
}

/// Lo que se escribe si el retiro procede.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WithdrawalPlan {
    pub remaining_cents: i64,
    pub balance_cents: i64,
    /// Día de Lima del retiro y total retirado ese día, este incluido.
    pub day: String,
    pub withdrawn_today_cents: i64,
    pub status: &'static str,
    pub review_reason: Option<&'static str>,
}

/// Decide sobre un retiro a partir del documento del usuario tal como se leyó (bajo bloqueo o
/// precondición). Valida saldo, mínimo, cooldown y tope diario, y elige si se paga solo:
/// únicamente al celular ya registrado en la cuenta y hasta [`AUTO_PAY_MAX_CENTS`]. Así, quien
/// robe una sesión no puede mandarse el dinero a su número sin que lo vea un administrador.
pub(crate) fn plan_withdrawal(
    user: &serde_json::Value,
    amount_cents: i64,
    phone: &str,
    now_ms: i64,
) -> Result<WithdrawalPlan, WithdrawError> {
    let balance = balance_cents(user);
    let last = user.get("last_withdrawal_at").and_then(|v| v.as_i64());
    let remaining = check_request(balance, amount_cents, last, now_ms)?;

    let day = lima_day(now_ms);
    let already_today = match user.get("withdraw_day").and_then(|v| v.as_str()) {
        Some(d) if d == day => user
            .get("withdrawn_today_cents")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0),
        _ => 0,
    };
    let withdrawn_today = already_today.saturating_add(amount_cents);
    if withdrawn_today > DAILY_MAX_CENTS {
        return Err(WithdrawError::Invalid(format!(
            "Superas el máximo diario de retiro (S/ {:.2}). Hoy puedes retirar hasta S/ {:.2}.",
            cents_to_soles(DAILY_MAX_CENTS),
            cents_to_soles((DAILY_MAX_CENTS - already_today).max(0))
        )));
    }

    let registered = user
        .get("phoneNumber")
        .and_then(|v| v.as_str())
        .and_then(normalize_phone);
    let review_reason = if registered.as_deref() != Some(phone) {
        Some("celular distinto al registrado en la cuenta")
    } else if amount_cents > AUTO_PAY_MAX_CENTS {
        Some("monto mayor al pago automático")
    } else {
        None
    };
    Ok(WithdrawalPlan {
        remaining_cents: remaining,
        balance_cents: balance,
        day,
        withdrawn_today_cents: withdrawn_today,
        status: if review_reason.is_some() {
            STATUS_REVIEW
        } else {
            STATUS_PENDING
        },
        review_reason,
    })
}

#[derive(Serialize, Deserialize)]
struct UserDebit {
    wallet_balance: f64,
    last_withdrawal_at: i64,
    last_balance_update: i64,
    last_balance_reason: String,
    withdraw_day: String,
    withdrawn_today_cents: i64,
}

#[derive(Serialize, Deserialize)]
struct WithdrawalDoc {
    #[serde(rename = "type")]
    kind: String,
    status: String,
    #[serde(rename = "reviewReason", skip_serializing_if = "Option::is_none")]
    review_reason: Option<String>,
    /// Día de Lima del retiro: el reembolso lo descuenta del tope diario de ese día.
    #[serde(rename = "withdrawDay")]
    withdraw_day: String,
    #[serde(rename = "discordId")]
    discord_id: String,
    amount: f64,
    #[serde(rename = "amountCents")]
    amount_cents: i64,
    #[serde(rename = "phoneNumber")]
    phone_number: String,
    #[serde(rename = "balanceBefore")]
    balance_before: f64,
    #[serde(rename = "balanceAfter")]
    balance_after: f64,
    source: String,
    #[serde(rename = "createdAt")]
    created_at: FirestoreTimestamp,
}

#[derive(Serialize, Deserialize)]
struct AuditLog {
    event: String,
    source: String,
    #[serde(rename = "discordId")]
    discord_id: String,
    amount: f64,
    #[serde(rename = "operationNumber")]
    operation_number: String,
    #[serde(rename = "createdAt")]
    created_at: FirestoreTimestamp,
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// El commit falló porque cambió el documento leído (o el id ya existía): se puede reintentar.
fn is_contention(e: &FirestoreError) -> bool {
    match e {
        FirestoreError::DataConflictError(_) => true,
        FirestoreError::DatabaseError(db) => {
            matches!(db.public.code.as_str(), "FailedPrecondition" | "Aborted")
        }
        _ => false,
    }
}

impl FirestoreClient {
    /// Descuenta `amount` del saldo de `discord_id` y registra la solicitud `pending`.
    pub async fn request_withdrawal(
        &self,
        discord_id: &str,
        amount: f64,
        phone: &str,
        source: &str,
    ) -> Result<WithdrawalRequested, WithdrawError> {
        if let Some(m) = &self.mysql {
            return m.request_withdrawal(discord_id, amount, phone, source).await;
        }
        let amount_cents = to_cents(amount).ok_or_else(|| {
            WithdrawError::Invalid("Ingresa un monto válido (máximo 2 decimales).".into())
        })?;
        let phone = normalize_phone(phone).ok_or_else(|| {
            WithdrawError::Invalid("Ingresa un celular Yape válido de 9 dígitos.".into())
        })?;
        let amount = cents_to_soles(amount_cents);

        for _ in 0..MAX_RETRIES {
            // 1. Estado actual del usuario con su update_time: lo que se valida es lo que se
            //    protege con la precondición.
            let Some(raw) = self.get_raw_doc(USERS, discord_id).await? else {
                return Err(WithdrawError::Invalid(
                    "No tienes saldo en tu billetera.".into(),
                ));
            };
            let Some(update_time) = raw.update_time else {
                return Err(WithdrawError::Data(DataError::Firestore(
                    "el usuario no tiene update_time".into(),
                )));
            };
            let (_, user): (String, serde_json::Value) = decode_document(&raw)?;
            let now_ms = Utc::now().timestamp_millis();
            let plan = plan_withdrawal(&user, amount_cents, &phone, now_ms)?;
            let (balance, remaining) = (plan.balance_cents, plan.remaining_cents);

            // 2. Una transacción: saldo nuevo + solicitud + auditoría.
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let id = format!("wd_{now_ms}_{seq}_{discord_id}");
            let now = FirestoreTimestamp::now();
            let new_balance = cents_to_soles(remaining);
            let debit = UserDebit {
                wallet_balance: new_balance,
                last_withdrawal_at: now_ms,
                last_balance_update: now_ms,
                last_balance_reason: format!("Solicitud de retiro Yape ({id})"),
                withdraw_day: plan.day.clone(),
                withdrawn_today_cents: plan.withdrawn_today_cents,
            };
            let withdrawal = WithdrawalDoc {
                kind: "withdrawal".into(),
                status: plan.status.into(),
                review_reason: plan.review_reason.map(str::to_string),
                withdraw_day: plan.day.clone(),
                discord_id: discord_id.to_string(),
                amount,
                amount_cents,
                phone_number: phone.clone(),
                balance_before: cents_to_soles(balance),
                balance_after: new_balance,
                source: source.to_string(),
                created_at: now,
            };
            let audit = AuditLog {
                event: "withdrawal_requested".into(),
                source: source.to_string(),
                discord_id: discord_id.to_string(),
                amount,
                operation_number: id.clone(),
                created_at: now,
            };

            let db = self.db();
            let mut tx = db.begin_transaction().await?;
            db.fluent()
                .update()
                .fields([
                    "wallet_balance",
                    "last_withdrawal_at",
                    "last_balance_update",
                    "last_balance_reason",
                    "withdraw_day",
                    "withdrawn_today_cents",
                ])
                .in_col(USERS)
                .precondition(FirestoreWritePrecondition::UpdateTime(
                    FirestoreInstant::from(FirestoreTimestamp::try_from(update_time)?),
                ))
                .document_id(discord_id)
                .object(&debit)
                .add_to_transaction(&mut tx)?;
            db.fluent()
                .update()
                .in_col(WITHDRAWALS)
                .precondition(FirestoreWritePrecondition::Exists(false))
                .document_id(&id)
                .object(&withdrawal)
                .add_to_transaction(&mut tx)?;
            db.fluent()
                .update()
                .in_col(AUDIT_LOGS)
                .document_id(format!("withdrawal_{id}"))
                .object(&audit)
                .add_to_transaction(&mut tx)?;

            match tx.commit().await {
                Ok(_) => {
                    return Ok(WithdrawalRequested {
                        id,
                        amount,
                        phone,
                        new_balance,
                        status: plan.status.into(),
                    })
                }
                // El saldo cambió desde la lectura: se vuelve a validar con el saldo real.
                Err(e) if is_contention(&e) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(WithdrawError::Busy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn convierte_a_centimos_exactos() {
        assert_eq!(to_cents(10.0), Some(1000));
        assert_eq!(to_cents(0.1 + 0.2), Some(30));
        assert_eq!(to_cents(19.99), Some(1999));
        assert_eq!(to_cents(1.005), None);
        assert_eq!(to_cents(0.0), None);
        assert_eq!(to_cents(-5.0), None);
        assert_eq!(to_cents(f64::NAN), None);
        assert_eq!(to_cents(f64::INFINITY), None);
    }

    #[test]
    fn valida_el_celular() {
        assert_eq!(normalize_phone("987 654 321").as_deref(), Some("987654321"));
        assert_eq!(normalize_phone("887654321"), None);
        assert_eq!(normalize_phone("98765432"), None);
        assert_eq!(normalize_phone(""), None);
    }

    #[test]
    fn lee_el_saldo_de_numero_texto_o_ausente() {
        assert_eq!(balance_cents(&json!({"wallet_balance": 12.5})), 1250);
        assert_eq!(balance_cents(&json!({"wallet_balance": 7})), 700);
        assert_eq!(balance_cents(&json!({"wallet_balance": "3,30"})), 330);
        assert_eq!(balance_cents(&json!({"wallet_balance": -4.0})), 0);
        assert_eq!(balance_cents(&json!({})), 0);
    }

    #[test]
    fn no_permite_superar_el_saldo_ni_por_un_centimo() {
        assert_eq!(check_request(1000, 1000, None, 0), Ok(0));
        assert!(matches!(
            check_request(1000, 1001, None, 0),
            Err(WithdrawError::Invalid(_))
        ));
        assert!(matches!(
            check_request(0, 100, None, 0),
            Err(WithdrawError::Invalid(_))
        ));
    }

    #[test]
    fn solo_soles_enteros() {
        assert!(matches!(
            check_request(5000, 1050, None, 0),
            Err(WithdrawError::Invalid(_))
        ));
        assert_eq!(check_request(5000, 1000, None, 0), Ok(4000));
        // Un saldo con céntimos heredado se puede retirar en soles enteros; los céntimos quedan.
        assert_eq!(check_request(1050, 1000, None, 0), Ok(50));
    }

    #[test]
    fn exige_el_minimo() {
        assert!(matches!(
            check_request(1000, 99, None, 0),
            Err(WithdrawError::Invalid(_))
        ));
        assert_eq!(check_request(1000, 100, None, 0), Ok(900));
    }

    #[test]
    fn dos_retiros_seguidos_no_pasan_el_cooldown() {
        let now = 1_000_000;
        assert_eq!(
            check_request(5000, 1000, Some(now - 5_000), now),
            Err(WithdrawError::Cooldown { wait_secs: 55 })
        );
        assert_eq!(
            check_request(5000, 1000, Some(now - COOLDOWN_MS), now),
            Ok(4000)
        );
    }

    // 2026-11-07 12:00 en Lima (17:00 UTC).
    const NOW: i64 = 1_794_070_800_000;

    #[test]
    fn al_celular_registrado_y_monto_bajo_se_paga_solo() {
        let user = json!({"wallet_balance": 100.0, "phoneNumber": "987654321"});
        let plan = plan_withdrawal(&user, 5000, "987654321", NOW).unwrap();
        assert_eq!(plan.status, STATUS_PENDING);
        assert_eq!(plan.remaining_cents, 5000);
        assert_eq!(plan.withdrawn_today_cents, 5000);
        assert_eq!(plan.day, "2026-11-07");
    }

    #[test]
    fn celular_nuevo_o_sin_registrar_va_a_revision() {
        let user = json!({"wallet_balance": 100.0, "phoneNumber": "987654321"});
        let plan = plan_withdrawal(&user, 1000, "911111111", NOW).unwrap();
        assert_eq!(plan.status, STATUS_REVIEW);
        let sin_celular = json!({"wallet_balance": 100.0});
        assert_eq!(
            plan_withdrawal(&sin_celular, 1000, "987654321", NOW).unwrap().status,
            STATUS_REVIEW
        );
    }

    #[test]
    fn monto_alto_va_a_revision() {
        let user = json!({"wallet_balance": 400.0, "phoneNumber": "987654321"});
        let plan = plan_withdrawal(&user, AUTO_PAY_MAX_CENTS + 100, "987654321", NOW).unwrap();
        assert_eq!(plan.status, STATUS_REVIEW);
        let plan = plan_withdrawal(&user, AUTO_PAY_MAX_CENTS, "987654321", NOW).unwrap();
        assert_eq!(plan.status, STATUS_PENDING);
    }

    #[test]
    fn respeta_el_tope_diario_y_se_reinicia_al_dia_siguiente() {
        let hoy = json!({
            "wallet_balance": 2000.0, "phoneNumber": "987654321",
            "withdraw_day": "2026-11-07", "withdrawn_today_cents": DAILY_MAX_CENTS - 1000,
        });
        assert!(plan_withdrawal(&hoy, 1000, "987654321", NOW).is_ok());
        assert!(matches!(
            plan_withdrawal(&hoy, 1100, "987654321", NOW),
            Err(WithdrawError::Invalid(_))
        ));
        let ayer = json!({
            "wallet_balance": 2000.0, "phoneNumber": "987654321",
            "withdraw_day": "2026-11-06", "withdrawn_today_cents": DAILY_MAX_CENTS,
        });
        assert_eq!(
            plan_withdrawal(&ayer, 1000, "987654321", NOW).unwrap().withdrawn_today_cents,
            1000
        );
    }

    #[test]
    fn el_dia_es_el_de_lima() {
        // 2026-11-08 03:00 UTC sigue siendo 7 de noviembre en Lima.
        assert_eq!(lima_day(1_794_106_800_000), "2026-11-07");
    }

    #[test]
    fn limita_los_intentos_por_usuario() {
        let user = "retiro-usuario-limite";
        assert!((0..MAX_ATTEMPTS).all(|_| allow_attempt(user)));
        assert!(!allow_attempt(user));
        assert!(allow_attempt("retiro-otro-usuario"));
    }
}
