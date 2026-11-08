//! Recarga de saldo con Yape desde Discord: botón `iniciar_recarga_btn` -> modal `recarga_yape_modal`.
//!
//! El envío del modal acredita el saldo con `FirestoreClient::credit_yape_operation`, la misma
//! lógica que usa la web: solo si el monitor registró una entrada del monto indicado.

use data::{FirestoreClient, YapeCredit, YapeError};
use serenity::all::*;
use tracing::warn;

pub const MODAL_ID: &str = "recarga_yape_modal";

fn short_input(label: &str, custom_id: &str, placeholder: &str, max: u16) -> CreateActionRow {
    CreateActionRow::InputText(
        CreateInputText::new(InputTextStyle::Short, label, custom_id)
            .placeholder(placeholder)
            .min_length(1)
            .max_length(max)
            .required(true),
    )
}

/// Abre el formulario de recarga al pulsar el botón del panel.
pub async fn open_modal(
    ctx: &Context,
    component: &ComponentInteraction,
) -> Result<(), serenity::Error> {
    let modal = CreateModal::new(MODAL_ID, "Recargar saldo con Yape").components(vec![
        short_input("Número de Yape desde el que pagaste", "yape_phone", "Ej: 987654321", 9),
        short_input("Monto enviado (S/)", "amount", "Ej: 10.00", 10),
        short_input("Número de operación", "operation_number", "Ej: 25381004", 64),
    ]);
    component
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await
}

fn field(modal: &ModalInteraction, id: &str) -> String {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|c| match c {
            ActionRowComponent::InputText(it) if it.custom_id == id => it.value.clone(),
            _ => None,
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Valida los tres campos; devuelve (celular, monto, operación) o el mensaje de error para el usuario.
pub fn validate(
    phone: &str,
    amount: &str,
    operation: &str,
) -> Result<(String, f64, String), &'static str> {
    let phone: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    if phone.len() != 9 {
        return Err("❌ Ingresa un número de Yape de 9 dígitos.");
    }
    let amount: f64 = match amount.replace(',', ".").parse() {
        Ok(a) if f64::is_finite(a) && a >= 1.0 => a,
        _ => return Err("❌ Ingresa un monto válido, mínimo S/ 1.00."),
    };
    let operation = operation.trim().to_string();
    if operation.len() < 4 || !operation.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("❌ Ingresa un número de operación válido (solo letras y números).");
    }
    Ok((phone, (amount * 100.0).round() / 100.0, operation))
}

pub async fn handle_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    firestore: Option<&FirestoreClient>,
) -> Result<(), serenity::Error> {
    modal.defer_ephemeral(&ctx.http).await?;

    let reply = |text: String| EditInteractionResponse::new().content(text);

    let (phone, amount, operation) = match validate(
        &field(modal, "yape_phone"),
        &field(modal, "amount"),
        &field(modal, "operation_number"),
    ) {
        Ok(v) => v,
        Err(msg) => {
            modal.edit_response(&ctx.http, reply(msg.to_string())).await?;
            return Ok(());
        }
    };

    let Some(fs) = firestore else {
        modal
            .edit_response(
                &ctx.http,
                reply("❌ Error al procesar tu recarga. Intenta de nuevo.".into()),
            )
            .await?;
        return Ok(());
    };

    let discord_id = modal.user.id.to_string();
    if !data::allow_yape_attempt(&discord_id) {
        modal
            .edit_response(
                &ctx.http,
                reply("⏳ Alcanzaste el límite de intentos. Espera 10 minutos antes de intentar otro número de operación.".into()),
            )
            .await?;
        return Ok(());
    }

    // Saldo global del jugador (`faceit_users/{discordId}`), igual que las recargas de la web.
    let text = match fs
        .credit_yape_operation(&operation, &discord_id, None, amount, Some(&phone), "discord")
        .await
    {
        Ok(YapeCredit::Credited {
            amount,
            new_balance,
            ..
        }) => format!(
            "✅ **Recarga acreditada**\n💰 Monto: S/ {amount:.2}\n💳 Saldo actual: S/ {new_balance:.2}"
        ),
        Ok(YapeCredit::NotYetSeen) => {
            "⏳ Aún no vemos el depósito. Espera unos segundos y vuelve a pulsar el botón para intentarlo otra vez.".into()
        }
        Ok(YapeCredit::AlreadyCredited { own: true }) => {
            "⚠️ Esta operación ya fue acreditada en tu billetera.".into()
        }
        Ok(YapeCredit::AlreadyCredited { own: false }) => {
            "⚠️ Esta operación ya fue utilizada para acreditar otra cuenta.".into()
        }
        Ok(YapeCredit::ReservedByOther) => {
            "⚠️ Este número de operación ya está siendo usado por otra cuenta.".into()
        }
        Err(YapeError::Invalid(msg)) => format!("❌ {msg}"),
        Err(YapeError::Data(err)) => {
            warn!("Error al acreditar recarga Yape de {discord_id}: {err}");
            "❌ Error al verificar el depósito Yape. Intenta de nuevo.".into()
        }
    };
    modal.edit_response(&ctx.http, reply(text)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn acepta_datos_validos() {
        assert_eq!(
            validate("987 654 321", "10,5", " 25381004 ").unwrap(),
            ("987654321".to_string(), 10.5, "25381004".to_string())
        );
    }

    #[test]
    fn rechaza_datos_invalidos() {
        assert!(validate("12345", "10", "25381004").is_err());
        assert!(validate("987654321", "0.5", "25381004").is_err());
        assert!(validate("987654321", "abc", "25381004").is_err());
        assert!(validate("987654321", "10", "12").is_err());
        assert!(validate("987654321", "10", "12 34; DROP").is_err());
    }
}
