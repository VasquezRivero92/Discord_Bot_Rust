//! Pago de premios de una partida a las billeteras de los ganadores (`match_payouts`).
//!
//! Cada ganador recibe `pozo total ÷ ganadores` (su apuesta más la parte proporcional de la de
//! los perdedores, sin comisión). Garantías contra el pago doble:
//!
//! * `match_payouts/{matchId}` se crea con la precondición `no existe` **en la misma transacción**
//!   que los incrementos de saldo: si MatchZy reenvía `series_end`, o dos procesos intentan
//!   pagar a la vez, solo uno confirma; el otro recibe [`PayoutOutcome::AlreadyPaid`] y no toca
//!   ningún saldo.
//! * El reparto es en soles enteros e igual para todos los ganadores; lo que no alcanza para
//!   un sol más por ganador queda para la casa (`houseCents`).
//! * Los saldos suben con incremento atómico, así que no pisan escrituras concurrentes.

use crate::error::DataError;
use crate::firestore::FirestoreClient;
use chrono::Utc;
use firestore::errors::FirestoreError;
use firestore::FirestoreTimestamp;
use firestore::FirestoreWritePrecondition;
use serde::{Deserialize, Serialize};

const PAYOUTS: &str = "match_payouts";
const AUDIT_LOGS: &str = "yape_audit_logs";
const USERS: &str = "faceit_users";

/// Resultado de intentar pagar una partida.
#[derive(Debug, Clone, PartialEq)]
pub enum PayoutOutcome {
    /// Pagada ahora: `(discordId, céntimos)` por ganador.
    Paid(Vec<(String, i64)>),
    /// Ya se había pagado antes; no se escribió nada.
    AlreadyPaid,
    /// Sin apuesta o sin ganadores: no hay nada que pagar (no se registra pago).
    NothingToPay,
}

/// Error al pagar.
#[derive(Debug, Clone, PartialEq)]
pub enum PayoutError {
    Invalid(String),
    Data(DataError),
}

impl std::fmt::Display for PayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayoutError::Invalid(m) => f.write_str(m),
            PayoutError::Data(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PayoutError {}

impl From<DataError> for PayoutError {
    fn from(e: DataError) -> Self {
        PayoutError::Data(e)
    }
}

impl From<FirestoreError> for PayoutError {
    fn from(e: FirestoreError) -> Self {
        PayoutError::Data(DataError::from(e))
    }
}

/// Reparte `total_cents` entre `n` ganadores en **soles enteros**: cada uno recibe lo mismo,
/// un múltiplo de 100 céntimos, y lo que no alcanza para otro sol entero por ganador queda
/// para la casa ([`house_cents`]). Nunca se paga más que el pozo.
pub fn split_pot(total_cents: i64, n: usize) -> Vec<i64> {
    if n == 0 || total_cents <= 0 {
        return Vec::new();
    }
    let soles_each = total_cents / 100 / n as i64;
    vec![soles_each * 100; n]
}

/// Lo que queda para la casa tras [`split_pot`].
pub fn house_cents(total_cents: i64, shares: &[i64]) -> i64 {
    total_cents - shares.iter().sum::<i64>()
}

/// Ids de ganadores únicos, sin vacíos, conservando el orden.
fn clean_winners(winners: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    winners
        .iter()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

/// Claves donde el legado puede guardar la apuesta por jugador (sin confirmar contra datos
/// reales: si no aparece ninguna, o discrepan, no se paga).
const BET_KEYS: [&str; 6] = [
    "betAmount",
    "bet_amount",
    "bet",
    "effectiveBetAmount",
    "agreedBetAmount",
    "bet_amount_per_player",
];

/// Lo que hace falta para pagar una partida, sacado del documento de la partida.
#[derive(Debug, Clone, PartialEq)]
pub struct PayoutPlan {
    pub bet_cents: i64,
    pub total_players: usize,
    pub winners: Vec<String>,
}

fn discord_id_of(player: &serde_json::Value) -> Option<String> {
    let v = player.get("discord_id").or_else(|| player.get("discordId"))?;
    let id = match v {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!id.is_empty()).then_some(id)
}

fn cents_of(v: &serde_json::Value) -> Option<i64> {
    let n = match v {
        serde_json::Value::Number(n) => n.as_f64()?,
        serde_json::Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        _ => return None,
    };
    (0.0..=1_000_000.0)
        .contains(&n)
        .then(|| (n * 100.0).round() as i64)
}

/// Deriva el plan de pago de la partida y el equipo ganador (`team1`/`team2`). Falla cerrado:
/// ante cualquier duda devuelve el motivo y no se paga.
pub fn plan_payout(doc: &serde_json::Value, winner_team: &str) -> Result<PayoutPlan, String> {
    let winning_key = match winner_team.trim().to_lowercase().as_str() {
        "team1" | "1" | "faction1" => "faction1_roster",
        "team2" | "2" | "faction2" => "faction2_roster",
        other => return Err(format!("equipo ganador desconocido: {other:?}")),
    };
    let roster = |key: &str| doc.get(key).and_then(|r| r.as_array()).cloned().unwrap_or_default();
    let (r1, r2) = (roster("faction1_roster"), roster("faction2_roster"));
    if r1.is_empty() || r2.is_empty() {
        return Err("la partida no tiene los dos rosters".into());
    }

    let mut bets: Vec<i64> = BET_KEYS
        .iter()
        .filter_map(|k| doc.get(*k).and_then(cents_of))
        .collect();
    bets.sort_unstable();
    bets.dedup();
    let bet_cents = match bets.as_slice() {
        [] => return Err("la partida no tiene un monto de apuesta reconocible".into()),
        [one] => *one,
        _ => return Err("la partida tiene montos de apuesta que no coinciden".into()),
    };

    let winners_roster = if winning_key == "faction1_roster" { &r1 } else { &r2 };
    let winners = winners_roster
        .iter()
        .map(discord_id_of)
        .collect::<Option<Vec<_>>>()
        .ok_or("algún ganador no tiene discord_id")?;
    Ok(PayoutPlan {
        bet_cents,
        total_players: r1.len() + r2.len(),
        winners,
    })
}

#[derive(Serialize, Deserialize)]
struct WinnerShare {
    #[serde(rename = "discordId")]
    discord_id: String,
    #[serde(rename = "amountCents")]
    amount_cents: i64,
}

#[derive(Serialize, Deserialize)]
struct PayoutDoc {
    #[serde(rename = "matchId")]
    match_id: String,
    status: String,
    #[serde(rename = "betCents")]
    bet_cents: i64,
    #[serde(rename = "totalPotCents")]
    total_pot_cents: i64,
    /// Sobrante del reparto en soles enteros: queda para la casa.
    #[serde(rename = "houseCents")]
    house_cents: i64,
    winners: Vec<WinnerShare>,
    source: String,
    #[serde(rename = "createdAt")]
    created_at: FirestoreTimestamp,
}

#[derive(Serialize, Deserialize)]
struct UserCredit {
    last_balance_update: i64,
    last_balance_reason: String,
    #[serde(rename = "discordId")]
    discord_id: String,
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

impl FirestoreClient {
    /// Paga el pozo de `match_id` a `winners`. `bet_cents` es la apuesta por jugador y
    /// `total_players` el número de jugadores que apostaron (ganadores y perdedores).
    pub async fn pay_match_winners(
        &self,
        match_id: &str,
        bet_cents: i64,
        total_players: usize,
        winners: &[String],
        source: &str,
    ) -> Result<PayoutOutcome, PayoutError> {
        if let Some(m) = &self.mysql {
            return m
                .pay_match_winners(match_id, bet_cents, total_players, winners, source)
                .await;
        }
        // El cobro previo de apuestas (`match_stakes`) solo existe con MySQL; sin apuestas
        // cobradas, pagar el pozo crearía dinero. Falla cerrado.
        if self.get_raw_doc(crate::mysql::STAKES, match_id.trim()).await?.is_none() {
            return Err(PayoutError::Invalid(
                "la partida no tiene apuestas cobradas; no se paga".into(),
            ));
        }
        let match_id = match_id.trim();
        if match_id.is_empty()
            || match_id.len() > 128
            || !match_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(PayoutError::Invalid("id de partida no válido".into()));
        }
        let winners = clean_winners(winners);
        if bet_cents <= 0 || total_players == 0 || winners.is_empty() {
            return Ok(PayoutOutcome::NothingToPay);
        }
        if winners.len() > total_players {
            return Err(PayoutError::Invalid(
                "hay más ganadores que jugadores en la partida".into(),
            ));
        }
        let total = bet_cents
            .checked_mul(total_players as i64)
            .ok_or_else(|| PayoutError::Invalid("pozo fuera de rango".into()))?;
        let shares = split_pot(total, winners.len());

        // Atajo: si ya está pagada no se arma la transacción (la precondición sigue siendo la
        // garantía real ante carreras).
        if self.get_raw_doc(PAYOUTS, match_id).await?.is_some() {
            return Ok(PayoutOutcome::AlreadyPaid);
        }

        let now = FirestoreTimestamp::now();
        let now_ms = Utc::now().timestamp_millis();
        let payout = PayoutDoc {
            match_id: match_id.to_string(),
            status: "paid".into(),
            bet_cents,
            total_pot_cents: total,
            house_cents: house_cents(total, &shares),
            winners: winners
                .iter()
                .zip(&shares)
                .map(|(w, c)| WinnerShare {
                    discord_id: w.clone(),
                    amount_cents: *c,
                })
                .collect(),
            source: source.to_string(),
            created_at: now,
        };

        let db = self.db();
        let mut tx = db.begin_transaction().await?;
        db.fluent()
            .update()
            .in_col(PAYOUTS)
            .precondition(FirestoreWritePrecondition::Exists(false))
            .document_id(match_id)
            .object(&payout)
            .add_to_transaction(&mut tx)?;
        for (winner, cents) in winners.iter().zip(&shares) {
            let amount = *cents as f64 / 100.0;
            let credit = UserCredit {
                last_balance_update: now_ms,
                last_balance_reason: format!("Premio de la partida {match_id}"),
                discord_id: winner.clone(),
            };
            db.fluent()
                .update()
                .fields(["last_balance_update", "last_balance_reason", "discordId"])
                .in_col(USERS)
                .transforms(|t| t.fields([t.field("wallet_balance").increment(amount)]))
                .document_id(winner)
                .object(&credit)
                .add_to_transaction(&mut tx)?;
            let audit = AuditLog {
                event: "match_payout".into(),
                source: source.to_string(),
                discord_id: winner.clone(),
                amount,
                operation_number: match_id.to_string(),
                created_at: now,
            };
            db.fluent()
                .update()
                .in_col(AUDIT_LOGS)
                .document_id(format!("payout_{match_id}_{winner}"))
                .object(&audit)
                .add_to_transaction(&mut tx)?;
        }

        match tx.commit().await {
            Ok(_) => Ok(PayoutOutcome::Paid(
                winners.into_iter().zip(shares).collect(),
            )),
            // Otro proceso pagó entre la lectura y el commit: no se acreditó nada aquí.
            Err(FirestoreError::DataConflictError(_)) => Ok(PayoutOutcome::AlreadyPaid),
            Err(FirestoreError::DatabaseError(e)) if e.public.code == "FailedPrecondition" => {
                Ok(PayoutOutcome::AlreadyPaid)
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_reparto_es_en_soles_enteros_y_el_sobrante_va_a_la_casa() {
        assert_eq!(split_pot(10_000, 5), vec![2000; 5]);
        // S/ 10 entre 3: S/ 3 a cada uno, S/ 1 para la casa.
        let s = split_pot(1000, 3);
        assert_eq!(s, vec![300; 3]);
        assert_eq!(house_cents(1000, &s), 100);
        // Pozo menor a S/ 1 por ganador: nadie cobra, todo queda para la casa.
        assert_eq!(split_pot(250, 3), vec![0; 3]);
        assert!(split_pot(9_999, 7).iter().all(|c| c % 100 == 0));
        assert!(split_pot(9_999, 7).iter().sum::<i64>() <= 9_999);
    }

    #[test]
    fn sin_ganadores_o_sin_pozo_no_hay_reparto() {
        assert!(split_pot(1000, 0).is_empty());
        assert!(split_pot(0, 5).is_empty());
        assert!(split_pot(-5, 5).is_empty());
    }

    #[test]
    fn cinco_contra_cinco_a_diez_soles_paga_veinte() {
        // 10 jugadores x S/ 10 = S/ 100 entre 5 ganadores.
        assert_eq!(split_pot(10 * 1000, 5), vec![2000; 5]);
    }

    fn doc(bet: serde_json::Value) -> serde_json::Value {
        let team = |p: &str| -> Vec<serde_json::Value> {
            (0..5)
                .map(|i| serde_json::json!({"id": format!("{p}{i}"), "discord_id": format!("{p}{i}")}))
                .collect()
        };
        serde_json::json!({"betAmount": bet, "faction1_roster": team("a"), "faction2_roster": team("b")})
    }

    #[test]
    fn plan_toma_ganadores_del_equipo_y_cuenta_todos_los_jugadores() {
        let p = plan_payout(&doc(serde_json::json!(10)), "team2").unwrap();
        assert_eq!(p.bet_cents, 1000);
        assert_eq!(p.total_players, 10);
        assert_eq!(p.winners, vec!["b0", "b1", "b2", "b3", "b4"]);
    }

    #[test]
    fn plan_falla_cerrado_ante_datos_dudosos() {
        assert!(plan_payout(&doc(serde_json::json!(10)), "draw").is_err());
        let mut sin_apuesta = doc(serde_json::json!(10));
        sin_apuesta.as_object_mut().unwrap().remove("betAmount");
        assert!(plan_payout(&sin_apuesta, "team1").is_err());
        let mut discrepa = doc(serde_json::json!(10));
        discrepa["bet_amount"] = serde_json::json!(5);
        assert!(plan_payout(&discrepa, "team1").is_err());
        let mut sin_id = doc(serde_json::json!(10));
        sin_id["faction1_roster"][2] = serde_json::json!({"id": "x"});
        assert!(plan_payout(&sin_id, "team1").is_err());
        assert!(plan_payout(&sin_id, "team2").is_ok());
    }

    #[test]
    fn plan_con_apuesta_cero_es_partida_gratis() {
        assert_eq!(plan_payout(&doc(serde_json::json!(0)), "team1").unwrap().bet_cents, 0);
    }

    #[test]
    fn limpia_ganadores_duplicados_vacios_o_con_caracteres_raros() {
        let w = ["111", " 222 ", "111", "", "a/b", "333"].map(String::from);
        assert_eq!(clean_winners(&w), vec!["111", "222", "333"]);
    }
}
