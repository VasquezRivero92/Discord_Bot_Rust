//! Recarga de saldo con Yape desde Discord: botón `iniciar_recarga_btn` -> modal `recarga_yape_modal`.
//!
//! La acreditación de saldo todavía no existe en Rust, así que el envío del modal registra la
//! solicitud en `yape_recharge_requests` (una por número de operación) y avisa al canal del staff.

use common::config::Config;
use data::FirestoreClient;
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
    config: &Config,
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

    let doc_id = format!("yape_{operation}");
    if matches!(
        fs.get_doc::<serde_json::Value>("yape_recharge_requests", &doc_id)
            .await,
        Ok(Some(_))
    ) {
        modal
            .edit_response(
                &ctx.http,
                reply("⚠️ Ese número de operación ya fue registrado.".into()),
            )
            .await?;
        return Ok(());
    }

    let doc = serde_json::json!({
        "id": doc_id,
        "discord_id": modal.user.id.to_string(),
        "guild_id": modal.guild_id.map(|g| g.to_string()),
        "yape_phone": phone,
        "amount": amount,
        "operation_number": operation,
        "status": "pendiente",
        "createdAt": data::timestamp_now(),
    });
    if let Err(err) = fs.set_doc("yape_recharge_requests", &doc_id, &doc).await {
        warn!("Error al registrar recarga Yape: {}", err);
        modal
            .edit_response(
                &ctx.http,
                reply("❌ Error al procesar tu recarga. Intenta de nuevo.".into()),
            )
            .await?;
        return Ok(());
    }

    let alerts_channel = config
        .env_defaults
        .discord_admin_alerts_channel_id
        .as_deref()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(ChannelId::new);
    match alerts_channel {
        Some(chan) => {
            let embed = CreateEmbed::new()
                .title("💜 Nueva recarga Yape")
                .color(0x742284)
                .field("👤 Jugador", format!("<@{}>", modal.user.id), true)
                .field("💰 Monto", format!("S/ {amount:.2}"), true)
                .field("📱 Yape", &phone, true)
                .field("🔢 N.º de operación", format!("`{operation}`"), false)
                .timestamp(Timestamp::now());
            if let Err(err) = chan
                .send_message(&ctx.http, CreateMessage::new().embed(embed))
                .await
            {
                warn!("No se pudo avisar al staff de la recarga Yape: {}", err);
            }
        }
        None => warn!("Canal de alertas del staff no configurado: recarga Yape sin aviso"),
    }

    modal
        .edit_response(
            &ctx.http,
            reply(format!(
                "✅ **Recarga recibida**\n💰 Monto: S/ {amount:.2}\n📱 Yape: {phone}\n🔢 Operación: `{operation}`"
            )),
        )
        .await?;
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
