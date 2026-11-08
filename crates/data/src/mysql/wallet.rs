//! Movimientos de billetera en MySQL: cobro y devolución de apuestas, libro de movimientos
//! (`wallet_ledger`), avisos por mensaje privado de Discord (`user_notifications`),
//! conciliación de saldos y límites de intentos que sobreviven a reinicios (`wallet_attempts`).
//!
//! Reglas de dinero:
//! * El pozo de una partida solo se paga desde apuestas **cobradas** (`match_stakes`): sin cobro
//!   previo no hay premio, así que un premio nunca crea dinero.
//! * Cobro todo-o-nada: o pagan todos los jugadores o no paga ninguno. Los saldos se bloquean
//!   en orden fijo (ids ordenados) para que dos cobros simultáneos no se bloqueen entre sí.
//! * Cada cambio de saldo deja una fila en `wallet_ledger` en la misma transacción, con el
//!   saldo resultante. Si el saldo de un documento no coincide con su último movimiento, alguien
//!   lo cambió por fuera (SQL a mano, restauración de backup...) y la conciliación lo avisa.

use super::*;

/// Colección de apuestas cobradas por partida.
pub const STAKES: &str = "match_stakes";

/// Resultado de cobrar las apuestas de una partida.
#[derive(Debug, Clone, PartialEq)]
pub enum StakeOutcome {
    /// Se cobró a todos: `total_cents` es el pozo.
    Collected { total_cents: i64 },
    /// Ya se había cobrado esta partida: no se volvió a cobrar.
    AlreadyCollected,
    /// Jugadores sin saldo suficiente, con su saldo en céntimos. No se cobró a nadie.
    Insufficient(Vec<(String, i64)>),
}

/// Aviso pendiente de enviar. `discord_id = None` va al canal de alertas de administración.
#[derive(Debug, Clone, PartialEq)]
pub struct WalletNotification {
    pub id: u64,
    pub discord_id: Option<String>,
    pub message: String,
}

/// Cuenta cuyo saldo no coincide con su último movimiento del libro.
#[derive(Debug, Clone, PartialEq)]
pub struct WalletMismatch {
    pub account_id: String,
    pub ledger_cents: i64,
    pub actual_cents: i64,
}

/// Apuestas listas para pagarse (ver [`take_stakes_for_payout`]).
pub(super) enum StakesForPayout {
    Ready { players: Vec<String> },
    AlreadyPaid,
}

fn soles(cents: i64) -> String {
    format!("S/ {:.2}", cents as f64 / 100.0)
}

fn valid_id(id: &str, max: usize) -> bool {
    !id.is_empty()
        && id.len() <= max
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Ids únicos y ordenados: el orden fijo de bloqueo evita interbloqueos entre cobros.
fn sorted_players(players: &[String]) -> Vec<String> {
    let mut v: Vec<String> = players
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| valid_id(p, 64))
        .collect();
    v.sort();
    v.dedup();
    v
}

pub(super) async fn bootstrap_wallet_tables(pool: &MySqlPool) -> Result<(), DataError> {
    for ddl in [
        r#"CREATE TABLE IF NOT EXISTS wallet_ledger (
            id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
            account_id VARCHAR(96) NOT NULL COMMENT 'id del documento faceit_users',
            delta_cents BIGINT NOT NULL,
            balance_after_cents BIGINT NOT NULL,
            kind VARCHAR(32) NOT NULL COMMENT 'recharge, withdrawal, withdrawal_refund, stake, stake_refund, prize',
            reference VARCHAR(128) NULL,
            created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
            PRIMARY KEY (id),
            KEY idx_ledger_account (account_id, id)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"#,
        r#"CREATE TABLE IF NOT EXISTS user_notifications (
            id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
            discord_id VARCHAR(32) NULL COMMENT 'NULL = canal de alertas de administración',
            message VARCHAR(1000) NOT NULL,
            attempts INT NOT NULL DEFAULT 0,
            created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
            sent_at DATETIME(3) NULL,
            PRIMARY KEY (id),
            KEY idx_notifications_pending (sent_at, id)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"#,
        r#"CREATE TABLE IF NOT EXISTS wallet_attempts (
            id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
            scope VARCHAR(32) NOT NULL,
            subject VARCHAR(64) NOT NULL,
            created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
            PRIMARY KEY (id),
            KEY idx_attempts_lookup (scope, subject, created_at)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"#,
    ] {
        sqlx::query(ddl)
            .execute(pool)
            .await
            .map_err(sqlx_to_data_err)?;
    }
    Ok(())
}

/// Anota un cambio de saldo en el libro (misma transacción que el cambio).
pub(super) async fn append_ledger(
    tx: &mut Transaction<'_, MySql>,
    account_id: &str,
    delta_cents: i64,
    balance_after_cents: i64,
    kind: &str,
    reference: &str,
) -> Result<(), DataError> {
    sqlx::query(
        "INSERT INTO wallet_ledger (account_id, delta_cents, balance_after_cents, kind, reference) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(account_id)
    .bind(delta_cents)
    .bind(balance_after_cents)
    .bind(kind)
    .bind(reference)
    .execute(&mut **tx)
    .await
    .map_err(sqlx_to_data_err)?;
    Ok(())
}

/// Encola un aviso (misma transacción que el movimiento: si el movimiento no se confirma, no
/// se avisa). `discord_id = None` = canal de alertas de administración.
pub(super) async fn enqueue_notification(
    tx: &mut Transaction<'_, MySql>,
    discord_id: Option<&str>,
    message: &str,
) -> Result<(), DataError> {
    let message: String = message.chars().take(1000).collect();
    sqlx::query("INSERT INTO user_notifications (discord_id, message) VALUES (?, ?)")
        .bind(discord_id)
        .bind(message)
        .execute(&mut **tx)
        .await
        .map_err(sqlx_to_data_err)?;
    Ok(())
}

/// Cambia el saldo de `account_id` en `delta_cents` (bajo bloqueo), con libro, auditoría y aviso.
/// Crea el documento si no existe (una devolución nunca se pierde porque se borró la cuenta).
#[allow(clippy::too_many_arguments)]
async fn apply_balance_change(
    tx: &mut Transaction<'_, MySql>,
    account_id: &str,
    delta_cents: i64,
    kind: &str,
    reference: &str,
    reason: &str,
    audit_event: &str,
    dm: &str,
) -> Result<i64, DataError> {
    let mut user = match fetch_doc_for_update(tx, "faceit_users", account_id).await? {
        Some(Value::Object(m)) => m,
        _ => {
            let mut m = Map::new();
            m.insert("discordId".into(), json!(account_id));
            m.insert("createdAt".into(), timestamp_now());
            m
        }
    };
    let before = crate::withdraw::balance_cents(&Value::Object(user.clone()));
    let after = before + delta_cents;
    user.insert("wallet_balance".into(), json!(after as f64 / 100.0));
    user.insert("last_balance_update".into(), json!(Utc::now().timestamp_millis()));
    user.insert("last_balance_reason".into(), json!(reason));
    let user_val = Value::Object(user);
    upsert_realtime_doc(tx, "faceit_users", account_id, &user_val.to_string()).await?;
    sync_relational_upsert(tx, "faceit_users", account_id, &user_val).await?;
    append_ledger(tx, account_id, delta_cents, after, kind, reference).await?;

    let audit_id = format!("{kind}_{reference}_{account_id}");
    let audit_val = json!({
        "event": audit_event,
        "source": "system",
        "discordId": account_id,
        "amount": delta_cents.abs() as f64 / 100.0,
        "operationNumber": reference,
        "createdAt": timestamp_now(),
    });
    upsert_realtime_doc(tx, "yape_audit_logs", &audit_id, &audit_val.to_string()).await?;
    sync_relational_upsert(tx, "yape_audit_logs", &audit_id, &audit_val).await?;
    enqueue_notification(tx, Some(account_id), &format!("{dm} Saldo actual: {}.", soles(after)))
        .await?;
    Ok(after)
}

/// Bloquea las apuestas de `match_id` para pagar el premio y las marca pagadas en la misma
/// transacción. Falla cerrado: sin apuestas cobradas, con otra apuesta o con un ganador que no
/// apostó, no se paga nada.
pub(super) async fn take_stakes_for_payout(
    tx: &mut Transaction<'_, MySql>,
    match_id: &str,
    bet_cents: i64,
    winners: &[String],
) -> Result<StakesForPayout, PayoutError> {
    let Some(Value::Object(mut st)) = fetch_doc_for_update(tx, STAKES, match_id).await? else {
        return Err(PayoutError::Invalid(
            "la partida no tiene apuestas cobradas; no se paga".into(),
        ));
    };
    match st.get("status").and_then(Value::as_str) {
        Some("collected") => {}
        Some("paid") => return Ok(StakesForPayout::AlreadyPaid),
        other => {
            return Err(PayoutError::Invalid(format!(
                "las apuestas de la partida están en estado {other:?}; no se paga"
            )))
        }
    }
    check_stakes_match(&Value::Object(st.clone()), bet_cents, winners)?;
    let players = stake_players(&Value::Object(st.clone()));
    st.insert("status".into(), json!("paid"));
    st.insert("paidAt".into(), timestamp_now());
    upsert_realtime_doc(tx, STAKES, match_id, &Value::Object(st).to_string()).await?;
    Ok(StakesForPayout::Ready { players })
}

fn stake_players(st: &Value) -> Vec<String> {
    st.get("players")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn manual_paid_of(st: &Value) -> Vec<String> {
    st.get("manualPaid")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn check_stakes_match(st: &Value, bet_cents: i64, winners: &[String]) -> Result<(), PayoutError> {
    if st.get("betCents").and_then(Value::as_i64) != Some(bet_cents) {
        return Err(PayoutError::Invalid(
            "la apuesta de la partida no coincide con la cobrada; no se paga".into(),
        ));
    }
    let players = stake_players(st);
    if let Some(w) = winners.iter().find(|w| !players.contains(w)) {
        return Err(PayoutError::Invalid(format!(
            "el ganador {w} no pagó apuesta en esta partida; no se paga"
        )));
    }
    Ok(())
}

/// Lo mismo que [`take_stakes_for_payout`] sobre el almacén en memoria (pruebas).
pub(super) fn take_stakes_for_payout_mem(
    store: &mut InMemoryStore,
    match_id: &str,
    bet_cents: i64,
    winners: &[String],
) -> Result<StakesForPayout, PayoutError> {
    let Some(st) = store
        .collections
        .get_mut(STAKES)
        .and_then(|c| c.get_mut(match_id))
    else {
        return Err(PayoutError::Invalid(
            "la partida no tiene apuestas cobradas; no se paga".into(),
        ));
    };
    match st.get("status").and_then(Value::as_str) {
        Some("collected") => {}
        Some("paid") => return Ok(StakesForPayout::AlreadyPaid),
        _ => {
            return Err(PayoutError::Invalid(
                "las apuestas no están cobradas; no se paga".into(),
            ))
        }
    }
    check_stakes_match(st, bet_cents, winners)?;
    let players = stake_players(st);
    st["status"] = json!("paid");
    Ok(StakesForPayout::Ready { players })
}

impl MysqlClient {
    /// Cobra `bet_cents` a cada jugador de `match_id` **antes** de crear la partida en el
    /// servidor. Todo-o-nada e idempotente (una sola vez por partida).
    ///
    /// `manual_paid` (solo lo decide un administrador) son jugadores que pagaron fuera de la
    /// billetera (efectivo, Yape directo): cuentan para el pozo y pueden ganar, pero no se les
    /// descuenta nada. Si la partida se cancela no se les devuelve a la billetera (no pagaron
    /// con ella): se alerta a los administradores.
    pub async fn collect_match_stakes(
        &self,
        match_id: &str,
        bet_cents: i64,
        players: &[String],
        manual_paid: &[String],
    ) -> Result<StakeOutcome, DataError> {
        let match_id = match_id.trim();
        if !valid_id(match_id, 128) {
            return Err(DataError::Firestore("id de partida no válido".into()));
        }
        if bet_cents <= 0 || bet_cents % 100 != 0 {
            return Err(DataError::Firestore(
                "la apuesta debe ser de soles enteros y mayor que cero".into(),
            ));
        }
        let all_players = sorted_players(players);
        if all_players.is_empty() {
            return Err(DataError::Firestore("la partida no tiene jugadores".into()));
        }
        let manual: Vec<String> = sorted_players(manual_paid)
            .into_iter()
            .filter(|p| all_players.contains(p))
            .collect();
        // Solo se descuenta a quienes no pagaron a mano.
        let players: Vec<String> = all_players
            .iter()
            .filter(|p| !manual.contains(p))
            .cloned()
            .collect();
        let total = bet_cents
            .checked_mul(all_players.len() as i64)
            .ok_or_else(|| DataError::Firestore("pozo fuera de rango".into()))?;
        let stakes_doc = json!({
            "matchId": match_id,
            "status": "collected",
            "betCents": bet_cents,
            "totalCents": total,
            "players": all_players,
            "manualPaid": manual,
            "createdAt": timestamp_now(),
        });

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                // La fila es el candado de idempotencia: si ya existe, ya se cobró.
                if !insert_realtime_doc_once(&mut tx, STAKES, match_id, &stakes_doc.to_string())
                    .await?
                {
                    return Ok(StakeOutcome::AlreadyCollected);
                }
                let mut short = Vec::new();
                for p in &players {
                    let bal = fetch_doc_for_update(&mut tx, "faceit_users", p)
                        .await?
                        .map(|u| crate::withdraw::balance_cents(&u))
                        .unwrap_or(0);
                    if bal < bet_cents {
                        short.push((p.clone(), bal));
                    }
                }
                if !short.is_empty() {
                    // Se descarta la transacción entera (también la fila de apuestas).
                    return Ok(StakeOutcome::Insufficient(short));
                }
                for p in &players {
                    apply_balance_change(
                        &mut tx,
                        p,
                        -bet_cents,
                        "stake",
                        match_id,
                        &format!("Apuesta de la partida {match_id}"),
                        "stake_collected",
                        &format!(
                            "Se cobró tu apuesta de {} para la partida {match_id}.",
                            soles(bet_cents)
                        ),
                    )
                    .await?;
                }
                tx.commit().await.map_err(sqlx_to_data_err)?;
                self.notify_write(STAKES, match_id, RealtimeOp::Upsert);
                for p in &players {
                    self.notify_write("faceit_users", p, RealtimeOp::Upsert);
                }
                Ok(StakeOutcome::Collected { total_cents: total })
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                if guard
                    .collections
                    .get(STAKES)
                    .is_some_and(|c| c.contains_key(match_id))
                {
                    return Ok(StakeOutcome::AlreadyCollected);
                }
                let users = guard.collections.entry("faceit_users".into()).or_default();
                let short: Vec<(String, i64)> = players
                    .iter()
                    .map(|p| {
                        let bal = users
                            .get(p)
                            .map(crate::withdraw::balance_cents)
                            .unwrap_or(0);
                        (p.clone(), bal)
                    })
                    .filter(|(_, bal)| *bal < bet_cents)
                    .collect();
                if !short.is_empty() {
                    return Ok(StakeOutcome::Insufficient(short));
                }
                for p in &players {
                    let u = users.entry(p.clone()).or_insert_with(|| json!({}));
                    let bal = crate::withdraw::balance_cents(u);
                    u["wallet_balance"] = json!((bal - bet_cents) as f64 / 100.0);
                }
                guard
                    .collections
                    .entry(STAKES.into())
                    .or_default()
                    .insert(match_id.to_string(), stakes_doc);
                Ok(StakeOutcome::Collected { total_cents: total })
            }
        }
    }

    /// Devuelve las apuestas de una partida cancelada. Idempotente: solo actúa si siguen
    /// `collected` (no pagadas ni devueltas). Devuelve el total devuelto ahora (0 si nada).
    pub async fn refund_match_stakes(&self, match_id: &str, reason: &str) -> Result<i64, DataError> {
        let match_id = match_id.trim();
        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                let Some(Value::Object(mut st)) =
                    fetch_doc_for_update(&mut tx, STAKES, match_id).await?
                else {
                    return Ok(0);
                };
                if st.get("status").and_then(Value::as_str) != Some("collected") {
                    return Ok(0);
                }
                let bet = st.get("betCents").and_then(Value::as_i64).unwrap_or(0);
                let manual = manual_paid_of(&Value::Object(st.clone()));
                let mut players: Vec<String> = stake_players(&Value::Object(st.clone()))
                    .into_iter()
                    .filter(|p| !manual.contains(p))
                    .collect();
                players.sort();
                if !manual.is_empty() {
                    let ids: Vec<String> = manual.iter().map(|m| format!("<@{m}>")).collect();
                    enqueue_notification(
                        &mut tx,
                        None,
                        &format!(
                            "Partida {match_id} cancelada: {} pagaron a mano ({} c/u) y hay que devolvérselo fuera de la billetera: {}",
                            manual.len(),
                            soles(bet),
                            ids.join(", ")
                        ),
                    )
                    .await?;
                }
                for p in &players {
                    apply_balance_change(
                        &mut tx,
                        p,
                        bet,
                        "stake_refund",
                        match_id,
                        &format!("Devolución de apuesta: partida {match_id} cancelada"),
                        "stake_refunded",
                        &format!(
                            "La partida {match_id} se canceló: te devolvimos tu apuesta de {}.",
                            soles(bet)
                        ),
                    )
                    .await?;
                }
                st.insert("status".into(), json!("refunded"));
                st.insert("refundedAt".into(), timestamp_now());
                st.insert("refundReason".into(), json!(reason));
                upsert_realtime_doc(&mut tx, STAKES, match_id, &Value::Object(st).to_string())
                    .await?;
                tx.commit().await.map_err(sqlx_to_data_err)?;
                self.notify_write(STAKES, match_id, RealtimeOp::Upsert);
                for p in &players {
                    self.notify_write("faceit_users", p, RealtimeOp::Upsert);
                }
                Ok(bet * players.len() as i64)
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let Some(st) = guard
                    .collections
                    .get_mut(STAKES)
                    .and_then(|c| c.get_mut(match_id))
                else {
                    return Ok(0);
                };
                if st.get("status").and_then(Value::as_str) != Some("collected") {
                    return Ok(0);
                }
                st["status"] = json!("refunded");
                let bet = st.get("betCents").and_then(Value::as_i64).unwrap_or(0);
                let manual = manual_paid_of(st);
                let players: Vec<String> = stake_players(st)
                    .into_iter()
                    .filter(|p| !manual.contains(p))
                    .collect();
                let users = guard.collections.entry("faceit_users".into()).or_default();
                for p in &players {
                    let u = users.entry(p.clone()).or_insert_with(|| json!({}));
                    let bal = crate::withdraw::balance_cents(u);
                    u["wallet_balance"] = json!((bal + bet) as f64 / 100.0);
                }
                Ok(bet * players.len() as i64)
            }
        }
    }

    /// Avisos pendientes de envío (más antiguos primero).
    pub async fn pending_notifications(
        &self,
        limit: u32,
    ) -> Result<Vec<WalletNotification>, DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query(
            "SELECT id, discord_id, message FROM user_notifications \
             WHERE sent_at IS NULL ORDER BY id ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(sqlx_to_data_err)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some(WalletNotification {
                    id: r.try_get::<u64, _>("id").ok()?,
                    discord_id: r.try_get::<Option<String>, _>("discord_id").ok()?,
                    message: r.try_get("message").ok()?,
                })
            })
            .collect())
    }

    /// Marca un aviso como enviado, o suma un intento fallido. Tras 5 fallos (p. ej. el usuario
    /// tiene los mensajes privados cerrados) se da por terminado para no reintentar siempre.
    pub async fn mark_notification(&self, id: u64, delivered: bool) -> Result<(), DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            return Ok(());
        };
        let sql = if delivered {
            "UPDATE user_notifications SET sent_at = NOW(3) WHERE id = ?"
        } else {
            "UPDATE user_notifications SET attempts = attempts + 1, \
             sent_at = IF(attempts + 1 >= 5, NOW(3), NULL) WHERE id = ?"
        };
        sqlx::query(sql)
            .bind(id)
            .execute(pool)
            .await
            .map_err(sqlx_to_data_err)?;
        Ok(())
    }

    /// Encola un mensaje privado para un jugador (fuera de una transacción).
    pub async fn notify_user(&self, discord_id: &str, message: &str) -> Result<(), DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            return Ok(());
        };
        let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
        enqueue_notification(&mut tx, Some(discord_id), message).await?;
        tx.commit().await.map_err(sqlx_to_data_err)
    }

    /// Encola una alerta para el canal de administración (fuera de una transacción).
    pub async fn alert_admins(&self, message: &str) -> Result<(), DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            return Ok(());
        };
        let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
        enqueue_notification(&mut tx, None, message).await?;
        tx.commit().await.map_err(sqlx_to_data_err)
    }

    /// Cuentas cuyo saldo actual no coincide con el saldo de su último movimiento del libro.
    /// Las cuentas sin movimientos (anteriores al libro) no se comparan.
    pub async fn reconcile_wallets(&self) -> Result<Vec<WalletMismatch>, DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query(
            r#"
            SELECT l.account_id,
                   l.balance_after_cents,
                   CAST(ROUND(COALESCE(CAST(JSON_UNQUOTE(JSON_EXTRACT(d.doc_json, '$.wallet_balance')) AS DECIMAL(14, 2)), 0) * 100) AS SIGNED) AS actual_cents
            FROM (
                SELECT account_id, balance_after_cents,
                       ROW_NUMBER() OVER (PARTITION BY account_id ORDER BY id DESC) AS rn
                FROM wallet_ledger
            ) l
            LEFT JOIN realtime_documents d
              ON d.collection = 'faceit_users' AND d.doc_id = l.account_id
            WHERE l.rn = 1
            "#,
        )
        .fetch_all(pool)
        .await
        .map_err(sqlx_to_data_err)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let m = WalletMismatch {
                    account_id: r.try_get("account_id").ok()?,
                    ledger_cents: r.try_get("balance_after_cents").ok()?,
                    actual_cents: r.try_get("actual_cents").ok()?,
                };
                (m.ledger_cents != m.actual_cents).then_some(m)
            })
            .collect())
    }

    /// Registra un intento de `subject` en `scope` y devuelve `false` si ya hubo `max` en los
    /// últimos `window_secs` segundos. Persistente y compartido entre procesos (la web y el bot).
    pub async fn allow_attempt_db(
        &self,
        scope: &str,
        subject: &str,
        max: i64,
        window_secs: i64,
    ) -> Result<bool, DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            return Ok(true);
        };
        let recent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wallet_attempts \
             WHERE scope = ? AND subject = ? AND created_at > NOW(3) - INTERVAL ? SECOND",
        )
        .bind(scope)
        .bind(subject)
        .bind(window_secs)
        .fetch_one(pool)
        .await
        .map_err(sqlx_to_data_err)?;
        if recent >= max {
            return Ok(false);
        }
        sqlx::query("INSERT INTO wallet_attempts (scope, subject) VALUES (?, ?)")
            .bind(scope)
            .bind(subject)
            .execute(pool)
            .await
            .map_err(sqlx_to_data_err)?;
        // Limpieza ocasional de intentos viejos.
        if Utc::now().timestamp_subsec_nanos() % 64 == 0 {
            let _ = sqlx::query(
                "DELETE FROM wallet_attempts WHERE created_at < NOW(3) - INTERVAL 1 DAY",
            )
            .execute(pool)
            .await;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich(client: &MysqlClient, id: &str, soles: f64) {
        if let Storage::Memory(mem) = &client.storage {
            mem.lock()
                .unwrap()
                .collections
                .entry("faceit_users".into())
                .or_default()
                .insert(id.into(), json!({ "wallet_balance": soles }));
        }
    }

    fn balance(client: &MysqlClient, id: &str) -> i64 {
        match &client.storage {
            Storage::Memory(mem) => mem
                .lock()
                .unwrap()
                .collections
                .get("faceit_users")
                .and_then(|c| c.get(id))
                .map(crate::withdraw::balance_cents)
                .unwrap_or(0),
            _ => unreachable!(),
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn cobra_a_todos_o_a_nadie() {
        let c = MysqlClient::in_memory();
        rich(&c, "a", 10.0);
        rich(&c, "b", 3.0);
        let out = c.collect_match_stakes("m1", 500, &ids(&["a", "b"]), &[]).await.unwrap();
        assert_eq!(out, StakeOutcome::Insufficient(vec![("b".into(), 300)]));
        assert_eq!((balance(&c, "a"), balance(&c, "b")), (1000, 300));

        rich(&c, "b", 5.0);
        let out = c.collect_match_stakes("m1", 500, &ids(&["b", "a", "a"]), &[]).await.unwrap();
        assert_eq!(out, StakeOutcome::Collected { total_cents: 1000 });
        assert_eq!((balance(&c, "a"), balance(&c, "b")), (500, 0));
        // Una segunda señal de inicio no vuelve a cobrar.
        assert_eq!(
            c.collect_match_stakes("m1", 500, &ids(&["a", "b"]), &[]).await.unwrap(),
            StakeOutcome::AlreadyCollected
        );
        assert_eq!(balance(&c, "a"), 500);
    }

    #[tokio::test]
    async fn el_pagado_a_mano_entra_al_pozo_sin_descuento_ni_reembolso() {
        let c = MysqlClient::in_memory();
        rich(&c, "a", 10.0);
        // "b" no tiene saldo pero el admin lo marcó como pagado en efectivo.
        let out = c
            .collect_match_stakes("mp", 500, &ids(&["a", "b"]), &ids(&["b"]))
            .await
            .unwrap();
        assert_eq!(out, StakeOutcome::Collected { total_cents: 1000 });
        assert_eq!((balance(&c, "a"), balance(&c, "b")), (500, 0));
        // Puede ganar el pozo completo.
        let paid = c.pay_match_winners("mp", 500, 2, &ids(&["b"]), "t").await.unwrap();
        assert_eq!(paid, PayoutOutcome::Paid(vec![("b".into(), 1000)]));

        // Si se cancela, solo se devuelve a quien pagó con la billetera.
        rich(&c, "a", 10.0);
        c.collect_match_stakes("mp2", 500, &ids(&["a", "b"]), &ids(&["b"]))
            .await
            .unwrap();
        assert_eq!(c.refund_match_stakes("mp2", "x").await.unwrap(), 500);
        assert_eq!((balance(&c, "a"), balance(&c, "b")), (1000, 1000));
    }

    #[tokio::test]
    async fn la_apuesta_es_en_soles_enteros() {
        let c = MysqlClient::in_memory();
        assert!(c.collect_match_stakes("m", 550, &ids(&["a"]), &[]).await.is_err());
        assert!(c.collect_match_stakes("m", 0, &ids(&["a"]), &[]).await.is_err());
    }

    #[tokio::test]
    async fn al_cancelar_se_devuelve_una_sola_vez() {
        let c = MysqlClient::in_memory();
        rich(&c, "a", 10.0);
        rich(&c, "b", 10.0);
        c.collect_match_stakes("m2", 500, &ids(&["a", "b"]), &[]).await.unwrap();
        assert_eq!(c.refund_match_stakes("m2", "cancelada").await.unwrap(), 1000);
        assert_eq!(c.refund_match_stakes("m2", "cancelada").await.unwrap(), 0);
        assert_eq!((balance(&c, "a"), balance(&c, "b")), (1000, 1000));
        assert_eq!(c.refund_match_stakes("sin_cobro", "x").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sin_cobro_no_hay_premio_y_lo_pagado_no_se_devuelve() {
        let c = MysqlClient::in_memory();
        rich(&c, "a", 10.0);
        rich(&c, "b", 10.0);
        // Sin apuestas cobradas: no se paga (no se crea dinero).
        assert!(c.pay_match_winners("m3", 500, 2, &ids(&["a"]), "t").await.is_err());
        c.collect_match_stakes("m3", 500, &ids(&["a", "b"]), &[]).await.unwrap();
        // Ganador que no apostó, o apuesta distinta a la cobrada: no se paga.
        assert!(c.pay_match_winners("m3", 500, 2, &ids(&["x"]), "t").await.is_err());
        assert!(c.pay_match_winners("m3", 1000, 2, &ids(&["a"]), "t").await.is_err());
        let paid = c.pay_match_winners("m3", 500, 2, &ids(&["a"]), "t").await.unwrap();
        assert_eq!(paid, PayoutOutcome::Paid(vec![("a".into(), 1000)]));
        assert_eq!((balance(&c, "a"), balance(&c, "b")), (1500, 500));
        // Pagada: ni se vuelve a pagar ni se devuelve la apuesta.
        assert_eq!(
            c.pay_match_winners("m3", 500, 2, &ids(&["a"]), "t").await.unwrap(),
            PayoutOutcome::AlreadyPaid
        );
        assert_eq!(c.refund_match_stakes("m3", "x").await.unwrap(), 0);
        assert_eq!(balance(&c, "a"), 1500);
    }
}
