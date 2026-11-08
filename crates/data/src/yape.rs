//! Acreditación de recargas Yape verificadas contra el monitor (`yape_transacciones`).
//!
//! Replica `findIncomingTransaction` + `creditYapeOperation` del legado Node:
//!
//! * El monitor guarda cada pago en `yape_transacciones/{numero_operacion}` con `tipo`
//!   (`ENTRADA`/`SALIDA`) y `monto`. Solo se acredita una `ENTRADA` cuyo monto coincide con el
//!   pedido (a 2 decimales).
//! * `yape_operations/{operación}` marca cada operación como acreditada una sola vez. El
//!   documento se escribe con una precondición (`no existe` o `update_time` leído), de modo que
//!   dos peticiones simultáneas con la misma operación no pueden acreditar dos veces.
//! * El saldo (`faceit_users/{discordId}` o `{guild}_{discordId}`) sube con un incremento
//!   atómico en la misma transacción que marca la operación.
//!
//! A diferencia del legado no se reserva una operación que aún no llegó: se responde
//! [`YapeCredit::NotYetSeen`] y no se escribe nada.

use crate::error::DataError;
use crate::firestore::FirestoreClient;
use chrono::Utc;
use firestore::{FirestoreInstant, FirestoreTimestamp, FirestoreWritePrecondition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TRANSACTIONS: &str = "yape_transacciones";
const OPERATIONS: &str = "yape_operations";
const AUDIT_LOGS: &str = "yape_audit_logs";
const USERS: &str = "faceit_users";

/// Resultado de intentar acreditar una operación.
#[derive(Debug, Clone, PartialEq)]
pub enum YapeCredit {
    /// Saldo acreditado.
    Credited {
        operation: String,
        amount: f64,
        new_balance: f64,
    },
    /// El monitor todavía no registró ese pago; no se escribió nada.
    NotYetSeen,
    /// La operación ya se acreditó (`own`: a esta misma cuenta).
    AlreadyCredited { own: bool },
    /// La operación está reservada por otra cuenta.
    ReservedByOther,
}

/// Error al acreditar: `Invalid` lleva un mensaje apto para mostrar al usuario.
#[derive(Debug, Clone, PartialEq)]
pub enum YapeError {
    Invalid(String),
    Data(DataError),
}

impl fmt::Display for YapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            YapeError::Invalid(m) => f.write_str(m),
            YapeError::Data(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for YapeError {}

impl From<DataError> for YapeError {
    fn from(e: DataError) -> Self {
        YapeError::Data(e)
    }
}

impl From<firestore::errors::FirestoreError> for YapeError {
    fn from(e: firestore::errors::FirestoreError) -> Self {
        YapeError::Data(DataError::from(e))
    }
}

/// Intentos de verificación permitidos por usuario en [`ATTEMPT_WINDOW`] (como los limitadores
/// por cuenta del legado): sin tope, alguien podría probar números de operación al azar.
const MAX_ATTEMPTS: usize = 10;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(600);

static ATTEMPTS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();

/// Registra un intento del usuario; `false` si ya agotó los de la ventana. Es global al proceso,
/// así que la web y el bot de Discord comparten el mismo cupo.
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

/// Mensaje único cuando el pago existe pero no cuadra (tipo, monto ilegible o monto distinto).
/// No dice el monto real: si lo dijera, bastaría probar números de operación ajenos con
/// cualquier monto para aprender el correcto y reclamar el depósito de otra persona.
pub(crate) const PAYMENT_MISMATCH: &str =
    "No pudimos validar el depósito con ese número de operación y monto. Revisa ambos datos.";

/// Valida el monto que el jugador dice haber yapeado: soles enteros, mayor que cero.
pub(crate) fn validate_recharge_amount(amount: f64) -> Result<(), YapeError> {
    if !amount.is_finite() || amount <= 0.0 {
        return Err(YapeError::Invalid("Ingresa un monto válido.".into()));
    }
    if amount.fract() != 0.0 {
        return Err(YapeError::Invalid(
            "Las recargas son en soles enteros (sin céntimos).".into(),
        ));
    }
    Ok(())
}

/// Un número de operación válido como id de documento: letras y números, 1 a 64 caracteres.
pub fn normalize_operation(raw: &str) -> Result<String, YapeError> {
    let op = raw.trim();
    if op.is_empty() || op.len() > 64 || !op.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(YapeError::Invalid(
            "Ingresa un número de operación Yape válido.".into(),
        ));
    }
    Ok(op.to_string())
}

/// Monto de un campo de Firestore que puede venir como número o como texto.
fn amount_of(v: Option<&serde_json::Value>) -> Option<f64> {
    let n = match v? {
        serde_json::Value::Number(n) => n.as_f64()?,
        serde_json::Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        _ => return None,
    };
    (n.is_finite() && n > 0.0).then_some(n)
}

/// Compara dos montos a 2 decimales (como `toFixed(2)` del legado).
fn same_amount(a: f64, b: f64) -> bool {
    (a * 100.0).round() == (b * 100.0).round()
}

fn text(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

#[derive(Serialize, Deserialize)]
struct OpDetails {
    #[serde(rename = "receivedAt")]
    received_at: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct OpDoc {
    #[serde(rename = "operationNumber")]
    operation_number: String,
    #[serde(rename = "discordId")]
    discord_id: String,
    #[serde(rename = "guildId")]
    guild_id: Option<String>,
    amount: f64,
    credited: bool,
    status: String,
    details: OpDetails,
    #[serde(rename = "creditedAt")]
    credited_at: FirestoreTimestamp,
    #[serde(rename = "createdAt")]
    created_at: FirestoreTimestamp,
}

#[derive(Serialize, Deserialize)]
struct UserCredit {
    last_balance_update: i64,
    last_balance_reason: String,
    #[serde(rename = "phoneNumber")]
    phone_number: Option<String>,
    #[serde(rename = "discordId")]
    discord_id: String,
    guild_id: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: FirestoreTimestamp,
}

#[derive(Serialize, Deserialize)]
struct AuditLog {
    event: String,
    source: String,
    #[serde(rename = "discordId")]
    discord_id: String,
    #[serde(rename = "guildId")]
    guild_id: Option<String>,
    amount: f64,
    #[serde(rename = "operationNumber")]
    operation_number: String,
    #[serde(rename = "createdAt")]
    created_at: FirestoreTimestamp,
}

impl FirestoreClient {
    /// Acredita `operation` al jugador si el monitor registró una `ENTRADA` del monto pedido.
    pub async fn credit_yape_operation(
        &self,
        operation: &str,
        discord_id: &str,
        guild_id: Option<&str>,
        expected_amount: f64,
        phone: Option<&str>,
        source: &str,
    ) -> Result<YapeCredit, YapeError> {
        if let Some(m) = &self.mysql {
            return m
                .credit_yape_operation(
                    operation,
                    discord_id,
                    guild_id,
                    expected_amount,
                    phone,
                    source,
                )
                .await;
        }
        let operation = normalize_operation(operation)?;
        validate_recharge_amount(expected_amount)?;
        let guild_id = guild_id.map(str::trim).filter(|g| !g.is_empty());

        // 1. El pago tiene que existir en el monitor, ser una ENTRADA y coincidir en monto.
        let Some(payment) = self
            .get_doc::<serde_json::Value>(TRANSACTIONS, &operation)
            .await?
        else {
            return Ok(YapeCredit::NotYetSeen);
        };
        if text(&payment, "tipo").unwrap_or_default().to_uppercase() != "ENTRADA" {
            return Err(YapeError::Invalid(PAYMENT_MISMATCH.into()));
        }
        let Some(amount) = amount_of(payment.get("monto")) else {
            return Err(YapeError::Invalid(PAYMENT_MISMATCH.into()));
        };
        if !same_amount(amount, expected_amount) {
            return Err(YapeError::Invalid(PAYMENT_MISMATCH.into()));
        }
        let received_at = text(&payment, "fecha_hora").or_else(|| text(&payment, "fecha_registro"));

        // 2. La operación no puede haberse usado ya.
        let existing = self.get_doc::<serde_json::Value>(OPERATIONS, &operation).await?;
        let mut precondition = FirestoreWritePrecondition::Exists(false);
        let mut is_new_op = true;
        if let Some(op) = &existing {
            let belongs = text(op, "discordId").as_deref() == Some(discord_id)
                && text(op, "guildId").as_deref() == guild_id;
            let credited = op.get("credited").and_then(|c| c.as_bool()) == Some(true)
                || text(op, "status").as_deref() == Some("credited");
            if credited {
                return Ok(YapeCredit::AlreadyCredited { own: belongs });
            }
            if !belongs {
                return Ok(YapeCredit::ReservedByOther);
            }
            // Reserva propia pendiente (creada por el legado): se completa sin pisar una
            // acreditación concurrente.
            let raw = self.get_raw_doc(OPERATIONS, &operation).await?;
            let Some(update_time) = raw.and_then(|d| d.update_time) else {
                return Err(YapeError::Data(DataError::Firestore(
                    "la operación pendiente no tiene update_time".into(),
                )));
            };
            precondition = FirestoreWritePrecondition::UpdateTime(FirestoreInstant::from(
                FirestoreTimestamp::try_from(update_time)?,
            ));
            is_new_op = false;
        }

        // 3. Una transacción: marcar la operación, subir el saldo y dejar la auditoría.
        let user_id = match guild_id {
            Some(g) => format!("{g}_{discord_id}"),
            None => discord_id.to_string(),
        };
        let existing_user = self.get_doc::<serde_json::Value>(USERS, &user_id).await?;
        let user_exists = existing_user.is_some();
        // El celular registrado es el destino del pago automático de retiros: una recarga solo
        // lo guarda si la cuenta aún no tiene uno.
        let has_phone = existing_user
            .as_ref()
            .and_then(|u| u.get("phoneNumber"))
            .and_then(|p| p.as_str())
            .is_some_and(|p| !p.trim().is_empty());
        let now = FirestoreTimestamp::now();
        let now_ms = Utc::now().timestamp_millis();
        let reason = format!("Recarga Yape verificada (operación: {operation})");

        let op_doc = OpDoc {
            operation_number: operation.clone(),
            discord_id: discord_id.to_string(),
            guild_id: guild_id.map(str::to_string),
            amount,
            credited: true,
            status: "credited".into(),
            details: OpDetails { received_at },
            credited_at: now,
            created_at: now,
        };
        let mut op_fields = vec![
            "operationNumber",
            "discordId",
            "guildId",
            "amount",
            "credited",
            "status",
            "details",
            "creditedAt",
        ];
        if is_new_op {
            op_fields.push("createdAt");
        }

        let phone = phone.map(str::trim).filter(|p| !p.is_empty());
        let user_doc = UserCredit {
            last_balance_update: now_ms,
            last_balance_reason: reason,
            phone_number: phone.map(str::to_string),
            discord_id: discord_id.to_string(),
            guild_id: guild_id.map(str::to_string),
            created_at: now,
        };
        let mut user_fields = vec!["last_balance_update", "last_balance_reason"];
        if phone.is_some() && !has_phone {
            user_fields.push("phoneNumber");
        }
        if !user_exists {
            user_fields.extend(["discordId", "guild_id", "createdAt"]);
        }

        let audit = AuditLog {
            event: "credited".into(),
            source: source.to_string(),
            discord_id: discord_id.to_string(),
            guild_id: guild_id.map(str::to_string),
            amount,
            operation_number: operation.clone(),
            created_at: now,
        };
        let audit_id = format!("credited_{now_ms}_{operation}");

        let db = self.db();
        let mut tx = db.begin_transaction().await?;
        db.fluent()
            .update()
            .fields(op_fields)
            .in_col(OPERATIONS)
            .precondition(precondition)
            .document_id(&operation)
            .object(&op_doc)
            .add_to_transaction(&mut tx)?;
        db.fluent()
            .update()
            .fields(user_fields)
            .in_col(USERS)
            .transforms(|t| t.fields([t.field("wallet_balance").increment(amount)]))
            .document_id(&user_id)
            .object(&user_doc)
            .add_to_transaction(&mut tx)?;
        db.fluent()
            .update()
            .in_col(AUDIT_LOGS)
            .document_id(&audit_id)
            .object(&audit)
            .add_to_transaction(&mut tx)?;
        tx.commit().await?;

        let new_balance = self
            .get_doc::<serde_json::Value>(USERS, &user_id)
            .await?
            .and_then(|u| u.get("wallet_balance").and_then(|b| b.as_f64()))
            .unwrap_or(amount);
        Ok(YapeCredit::Credited {
            operation,
            amount,
            new_balance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normaliza_la_operacion() {
        assert_eq!(normalize_operation(" 21740348 ").unwrap(), "21740348");
        assert!(normalize_operation("").is_err());
        assert!(normalize_operation("12/34").is_err());
        assert!(normalize_operation(&"9".repeat(65)).is_err());
    }

    #[test]
    fn monto_acepta_numero_y_texto() {
        assert_eq!(amount_of(Some(&json!(5))), Some(5.0));
        assert_eq!(amount_of(Some(&json!("5.50"))), Some(5.5));
        assert_eq!(amount_of(Some(&json!("5,5"))), Some(5.5));
        assert_eq!(amount_of(Some(&json!(0))), None);
        assert_eq!(amount_of(Some(&json!("abc"))), None);
        assert_eq!(amount_of(None), None);
    }

    #[test]
    fn limita_los_intentos_por_usuario() {
        let user = "usuario-de-prueba-limite";
        assert!((0..MAX_ATTEMPTS).all(|_| allow_attempt(user)));
        assert!(!allow_attempt(user));
        assert!(allow_attempt("otro-usuario-de-prueba"));
    }

    #[test]
    fn recarga_solo_en_soles_enteros() {
        assert!(validate_recharge_amount(10.0).is_ok());
        assert!(validate_recharge_amount(10.5).is_err());
        assert!(validate_recharge_amount(0.0).is_err());
        assert!(validate_recharge_amount(f64::INFINITY).is_err());
    }

    #[test]
    fn compara_montos_a_dos_decimales() {
        assert!(same_amount(5.0, 5.001));
        assert!(!same_amount(5.0, 5.01));
    }
}
