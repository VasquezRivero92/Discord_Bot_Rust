//! Cliente MySQL con motor de tiempo real (CDC / Pub-Sub) compatible 100% con la API de Firestore.
//!
//! Combina:
//! 1. Pool asíncrono de conexiones MySQL (`sqlx::MySqlPool`).
//! 2. Almacén documental JSON (`realtime_documents`) sincronizado transaccionalmente con las
//!    tablas relacionales normalizadas (`users`, `guilds`, `yape_transactions`, `yape_operations`,
//!    `yape_withdrawals`, `match_payouts`, `yape_audit_logs`, `orders`, `claims`,
//!    `support_calls`, `bot_logs`, `matches`).
//! 3. Motor de eventos en tiempo real (`realtime_events` + `tokio::sync::broadcast` + CDC poller)
//!    que replica exactamente la semántica de `onSnapshot` (`Added`, `Modified`, `Removed`,
//!    `Reset`, `Synced`) tanto para escrituras del propio servidor como para cambios externos en SQL.

mod wallet;
pub use wallet::{StakeOutcome, WalletMismatch, WalletNotification, STAKES};

use crate::error::DataError;
use crate::firestore::{ChangeEvent, Dir, Op, Query, Value as QValue, WatchScope};
use crate::payout::{house_cents, split_pot, PayoutError, PayoutOutcome};
use crate::withdraw::{
    normalize_phone, plan_withdrawal, to_cents, WithdrawError, WithdrawalRequested,
};
use crate::write::timestamp_now;
use crate::yape::{
    normalize_operation, validate_recharge_amount, YapeCredit, YapeError, PAYMENT_MISMATCH,
};
use chrono::Utc;
use futures::Stream;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use sqlx::{MySql, MySqlPool, Row, Transaction};
use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Notify};

const CDC_POLL_INTERVAL: Duration = Duration::from_millis(250);
const BROADCAST_CAPACITY: usize = 4096;

/// Operación registrada en el bus de tiempo real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeOp {
    Upsert,
    Delete,
}

/// Evento emitido por el motor de tiempo real (interno o desde `realtime_events`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEvent {
    pub seq: u64,
    pub collection: String,
    pub doc_id: String,
    pub op: RealtimeOp,
}

/// Resultado del cierre de partida en MySQL.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MatchFinishOutcome {
    pub match_id: String,
    pub winner: String,
    pub score_a: u32,
    pub score_b: u32,
    pub payouts: Vec<PayoutShare>,
    pub stats_saved: bool,
}

/// Cuota de premio pagada a un jugador.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PayoutShare {
    pub discord_id: String,
    pub amount: f64,
}

/// Modo de ajuste de saldo administrativo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BalanceAdjustment {
    Delta(f64),
    Set(f64),
}

/// Errores durante la compra en la tienda.
#[derive(Debug, thiserror::Error)]
pub enum CheckoutError {
    #[error("{0}")]
    Invalid(String),
    #[error("No encontramos ese depósito. Verifica el número de operación y espera unos segundos si acabas de yapear.")]
    PaymentNotFound,
    #[error("El número de operación indicado no corresponde a un depósito recibido.")]
    PaymentNotEntry,
    #[error("El depósito es de S/ {0:.2} y no coincide con el monto solicitado.")]
    AmountMismatch(f64),
    #[error("Este depósito Yape ya ha sido utilizado en otro pedido.")]
    AlreadyUsed,
    #[error(transparent)]
    Data(#[from] DataError),
}

/// Almacén en memoria para pruebas unitarias sin necesidad de un demonio MySQL externo.
#[derive(Default)]
struct InMemoryStore {
    collections: BTreeMap<String, BTreeMap<String, Value>>,
    seq: u64,
}

#[derive(Clone)]
enum Storage {
    Pool {
        pool: MySqlPool,
        wake_cdc: Arc<Notify>,
    },
    Memory(Arc<Mutex<InMemoryStore>>),
}

/// Cliente MySQL con soporte de consultas estructuradas, transacciones ACID y streams en tiempo real.
#[derive(Clone)]
pub struct MysqlClient {
    storage: Storage,
    tx_events: broadcast::Sender<RealtimeEvent>,
}

static WITHDRAW_SEQ: AtomicU64 = AtomicU64::new(0);

impl MysqlClient {
    /// Conecta al servidor MySQL indicado por `database_url` (`mysql://user:pass@host:port/db`),
    /// garantiza la existencia de las tablas de tiempo real (`realtime_documents` y
    /// `realtime_events`) y arranca el colector CDC en segundo plano.
    pub async fn connect(database_url: &str) -> Result<Self, DataError> {
        let opts = MySqlConnectOptions::from_str(database_url)
            .map_err(|e| DataError::Firestore(format!("URL de MySQL inválida: {e}")))?;
        let pool = MySqlPoolOptions::new()
            .max_connections(20)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(15))
            .connect_with(opts)
            .await
            .map_err(sqlx_to_data_err)?;

        bootstrap_realtime_tables(&pool).await?;

        let last_seq: u64 = sqlx::query_scalar(
            "SELECT CAST(COALESCE(MAX(seq), 0) AS UNSIGNED) FROM realtime_events",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

        let (tx_events, _) = broadcast::channel(BROADCAST_CAPACITY);
        let wake_cdc = Arc::new(Notify::new());

        spawn_cdc_poller(
            pool.clone(),
            tx_events.clone(),
            wake_cdc.clone(),
            last_seq,
        );

        Ok(Self {
            storage: Storage::Pool { pool, wake_cdc },
            tx_events,
        })
    }

    /// Crea una instancia de [`MysqlClient`] respaldada en memoria (ideal para pruebas unitarias
    /// deterministas sin requerir un servidor MySQL corriendo en la máquina de CI).
    pub fn in_memory() -> Self {
        let (tx_events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            storage: Storage::Memory(Arc::new(Mutex::new(InMemoryStore::default()))),
            tx_events,
        }
    }

    /// Devuelve una referencia al pool MySQL subyacente (si no es en memoria).
    pub fn pool(&self) -> Option<&MySqlPool> {
        match &self.storage {
            Storage::Pool { pool, .. } => Some(pool),
            Storage::Memory(_) => None,
        }
    }

    fn notify_write(&self, collection: &str, doc_id: &str, op: RealtimeOp) {
        match &self.storage {
            Storage::Pool { wake_cdc, .. } => {
                wake_cdc.notify_one();
            }
            Storage::Memory(mem) => {
                let seq = {
                    let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                    guard.seq += 1;
                    guard.seq
                };
                let _ = self.tx_events.send(RealtimeEvent {
                    seq,
                    collection: collection.to_string(),
                    doc_id: doc_id.to_string(),
                    op,
                });
            }
        }
    }

    /// Lee el JSON crudo almacenado en `realtime_documents` (conservando centinelas `$timestamp`).
    pub async fn get_raw_json(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<Value>, DataError> {
        match &self.storage {
            Storage::Pool { pool, .. } => {
                let row = sqlx::query(
                    "SELECT CAST(doc_json AS CHAR) AS doc_str FROM realtime_documents WHERE collection = ? AND doc_id = ?",
                )
                .bind(collection)
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(sqlx_to_data_err)?;

                match row {
                    Some(r) => {
                        let s: String = r.try_get("doc_str").map_err(sqlx_to_data_err)?;
                        let v: Value = serde_json::from_str(&s)
                            .map_err(|e| DataError::Decode(format!("{collection}/{id}: {e}")))?;
                        Ok(Some(v))
                    }
                    None => Ok(None),
                }
            }
            Storage::Memory(mem) => {
                let guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                Ok(guard
                    .collections
                    .get(collection)
                    .and_then(|c| c.get(id))
                    .cloned())
            }
        }
    }

    /// Lista todos los documentos crudos `(doc_id, doc_json)` de `collection`, ordenados por `doc_id`.
    pub async fn list_raw_json(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, Value)>, DataError> {
        match &self.storage {
            Storage::Pool { pool, .. } => {
                let rows = sqlx::query(
                    "SELECT doc_id, CAST(doc_json AS CHAR) AS doc_str FROM realtime_documents WHERE collection = ? ORDER BY doc_id ASC",
                )
                .bind(collection)
                .fetch_all(pool)
                .await
                .map_err(sqlx_to_data_err)?;

                let mut out = Vec::with_capacity(rows.len());
                for r in rows {
                    let id: String = r.try_get("doc_id").map_err(sqlx_to_data_err)?;
                    let s: String = r.try_get("doc_str").map_err(sqlx_to_data_err)?;
                    let v: Value = serde_json::from_str(&s)
                        .map_err(|e| DataError::Decode(format!("{collection}/{id}: {e}")))?;
                    out.push((id, v));
                }
                Ok(out)
            }
            Storage::Memory(mem) => {
                let guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                Ok(guard
                    .collections
                    .get(collection)
                    .map(|c| c.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default())
            }
        }
    }

    /// Lee `collection/id` y lo decodifica a `T` (los `$timestamp` se entregan como RFC 3339).
    pub async fn get_doc<T: DeserializeOwned>(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<T>, DataError> {
        let Some(raw) = self.get_raw_json(collection, id).await? else {
            return Ok(None);
        };
        let normalized = json_for_serde(raw);
        let decoded = serde_json::from_value(normalized)
            .map_err(|e| DataError::Decode(format!("{collection}/{id}: {e}")))?;
        Ok(Some(decoded))
    }

    /// Lista todos los documentos de `collection` decodificados como `(id, T)`.
    pub async fn list_docs<T: DeserializeOwned>(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, T)>, DataError> {
        let docs = self.list_raw_json(collection).await?;
        docs.into_iter()
            .map(|(id, raw)| {
                let normalized = json_for_serde(raw);
                let val = serde_json::from_value(normalized)
                    .map_err(|e| DataError::Decode(format!("{collection}/{id}: {e}")))?;
                Ok((id, val))
            })
            .collect()
    }

    /// Ejecuta la consulta estructurada `q` sobre `collection`.
    pub async fn query<T: DeserializeOwned>(
        &self,
        collection: &str,
        q: Query,
    ) -> Result<Vec<(String, T)>, DataError> {
        let docs = self.list_raw_json(collection).await?;
        let filtered = apply_query_in_memory(docs, &q);
        filtered
            .into_iter()
            .map(|(id, raw)| {
                let normalized = json_for_serde(raw);
                let val = serde_json::from_value(normalized)
                    .map_err(|e| DataError::Decode(format!("{collection}/{id}: {e}")))?;
                Ok((id, val))
            })
            .collect()
    }

    /// Cuenta cuántos documentos de `collection` cumplen los filtros de `q`.
    pub async fn count(&self, collection: &str, q: Query) -> Result<u64, DataError> {
        let docs = self.list_raw_json(collection).await?;
        let q_no_limit = Query {
            filters: q.filters,
            order_by: None,
            limit: None,
        };
        Ok(apply_query_in_memory(docs, &q_no_limit).len() as u64)
    }

    /// Crea o reemplaza el documento en `collection/id`.
    pub async fn set_doc(
        &self,
        collection: &str,
        id: &str,
        val: &Value,
    ) -> Result<(), DataError> {
        if !val.is_object() {
            return Err(DataError::Firestore(
                "El documento debe ser un objeto JSON".into(),
            ));
        }
        match &self.storage {
            Storage::Pool { pool, .. } => {
                let json_str = serde_json::to_string(val)
                    .map_err(|e| DataError::Decode(e.to_string()))?;
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                upsert_realtime_doc(&mut tx, collection, id, &json_str).await?;
                sync_relational_upsert(&mut tx, collection, id, val).await?;
                tx.commit().await.map_err(sqlx_to_data_err)?;
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .collections
                    .entry(collection.to_string())
                    .or_default()
                    .insert(id.to_string(), val.clone());
            }
        }
        self.notify_write(collection, id, RealtimeOp::Upsert);
        Ok(())
    }

    /// Crea o fusiona únicamente los campos de primer nivel de `val` en `collection/id`.
    pub async fn merge_doc(
        &self,
        collection: &str,
        id: &str,
        val: &Value,
    ) -> Result<(), DataError> {
        let patch = match val {
            Value::Object(m) => m,
            _ => {
                return Err(DataError::Firestore(
                    "El documento debe ser un objeto JSON".into(),
                ))
            }
        };
        if patch.is_empty() {
            return Ok(());
        }

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                let existing = fetch_doc_for_update(&mut tx, collection, id).await?;
                let mut merged = match existing {
                    Some(Value::Object(m)) => m,
                    _ => Map::new(),
                };
                for (k, v) in patch {
                    merged.insert(k.clone(), v.clone());
                }
                let merged_val = Value::Object(merged);
                let json_str = serde_json::to_string(&merged_val)
                    .map_err(|e| DataError::Decode(e.to_string()))?;
                upsert_realtime_doc(&mut tx, collection, id, &json_str).await?;
                sync_relational_upsert(&mut tx, collection, id, &merged_val).await?;
                tx.commit().await.map_err(sqlx_to_data_err)?;
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let entry = guard
                    .collections
                    .entry(collection.to_string())
                    .or_default()
                    .entry(id.to_string())
                    .or_insert_with(|| json!({}));
                if let Value::Object(curr) = entry {
                    for (k, v) in patch {
                        curr.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        self.notify_write(collection, id, RealtimeOp::Upsert);
        Ok(())
    }

    /// Elimina `collection/id`. Si no existe, devuelve `Ok(())`.
    pub async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), DataError> {
        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                let res = sqlx::query(
                    "DELETE FROM realtime_documents WHERE collection = ? AND doc_id = ?",
                )
                .bind(collection)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(sqlx_to_data_err)?;
                if res.rows_affected() > 0 {
                    record_realtime_event(&mut tx, collection, id, "DELETE").await?;
                }
                sync_relational_delete(&mut tx, collection, id).await?;
                tx.commit().await.map_err(sqlx_to_data_err)?;
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(col) = guard.collections.get_mut(collection) {
                    col.remove(id);
                }
            }
        }
        self.notify_write(collection, id, RealtimeOp::Delete);
        Ok(())
    }

    /// Elimina únicamente los campos indicados de `collection/id`.
    pub async fn delete_fields(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<(), DataError> {
        if fields.is_empty() {
            return Ok(());
        }
        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                if let Some(Value::Object(mut map)) =
                    fetch_doc_for_update(&mut tx, collection, id).await?
                {
                    for f in fields {
                        map.remove(*f);
                    }
                    let updated = Value::Object(map);
                    let json_str = serde_json::to_string(&updated)
                        .map_err(|e| DataError::Decode(e.to_string()))?;
                    upsert_realtime_doc(&mut tx, collection, id, &json_str).await?;
                    sync_relational_upsert(&mut tx, collection, id, &updated).await?;
                    tx.commit().await.map_err(sqlx_to_data_err)?;
                    self.notify_write(collection, id, RealtimeOp::Upsert);
                }
            }
            Storage::Memory(mem) => {
                let mut changed = false;
                {
                    let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(Value::Object(map)) = guard
                        .collections
                        .get_mut(collection)
                        .and_then(|c| c.get_mut(id))
                    {
                        for f in fields {
                            map.remove(*f);
                        }
                        changed = true;
                    }
                }
                if changed {
                    self.notify_write(collection, id, RealtimeOp::Upsert);
                }
            }
        }
        Ok(())
    }

    /// Observa en tiempo real todos los cambios de `collection`.
    pub fn watch_collection(
        &self,
        collection: &str,
    ) -> impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static {
        self.watch(WatchScope::Collection(collection.to_string()))
    }

    /// Observa en tiempo real los cambios de un único documento `collection/id`.
    pub fn watch_document(
        &self,
        collection: &str,
        id: &str,
    ) -> impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static {
        self.watch(WatchScope::Document {
            collection: collection.to_string(),
            id: id.to_string(),
        })
    }

    /// Abre un stream reactivo de cambios para `scope`:
    /// 1. Suscribe el receptor broadcast **antes** de leer la instantánea inicial para no perder
    ///    eventos que ocurran durante la carga inicial.
    /// 2. Emite `ChangeEvent::Added(id)` por cada documento existente en la instantánea, seguido de
    ///    `ChangeEvent::Synced(ids_ordenados)`.
    /// 3. Por cada evento posterior en tiempo real emite `Added`, `Modified` o `Removed` (o `Reset`
    ///    si el buffer del receptor sufrió `Lagged`).
    pub fn watch(
        &self,
        scope: WatchScope,
    ) -> impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static {
        let client = self.clone();
        let rx = self.tx_events.subscribe();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Result<ChangeEvent, DataError>>();

        tokio::spawn(async move {
            let initial_ids: Result<Vec<String>, DataError> = match &scope {
                WatchScope::Collection(col) => client
                    .list_raw_json(col)
                    .await
                    .map(|docs| docs.into_iter().map(|(id, _)| id).collect()),
                WatchScope::Document { collection, id } => client
                    .get_raw_json(collection, id)
                    .await
                    .map(|opt| opt.map(|_| vec![id.clone()]).unwrap_or_default()),
            };

            let mut known = HashSet::new();
            match initial_ids {
                Ok(mut ids) => {
                    ids.sort();
                    for id in &ids {
                        known.insert(id.clone());
                        if out_tx.send(Ok(ChangeEvent::Added(id.clone()))).is_err() {
                            return;
                        }
                    }
                    if out_tx.send(Ok(ChangeEvent::Synced(ids))).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = out_tx.send(Err(err));
                    return;
                }
            }

            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let matches_scope = match &scope {
                            WatchScope::Collection(col) => &ev.collection == col,
                            WatchScope::Document { collection, id } => {
                                &ev.collection == collection && &ev.doc_id == id
                            }
                        };
                        if !matches_scope {
                            continue;
                        }
                        let change = match ev.op {
                            RealtimeOp::Upsert => {
                                if known.insert(ev.doc_id.clone()) {
                                    Some(ChangeEvent::Added(ev.doc_id))
                                } else {
                                    Some(ChangeEvent::Modified(ev.doc_id))
                                }
                            }
                            RealtimeOp::Delete => {
                                if known.remove(&ev.doc_id) {
                                    Some(ChangeEvent::Removed(ev.doc_id))
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(c) = change {
                            if out_tx.send(Ok(c)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        known.clear();
                        if out_tx.send(Ok(ChangeEvent::Reset)).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        futures::stream::unfold(out_rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })
    }

    // ========================================================================
    // TRANSACCIONES FINANCIERAS ACID (Yape Recargas, Retiros y Premios)
    // ========================================================================

    /// Acredita `operation` al jugador con bloqueo pesimista `FOR UPDATE` en MySQL.
    pub async fn credit_yape_operation(
        &self,
        operation: &str,
        discord_id: &str,
        guild_id: Option<&str>,
        expected_amount: f64,
        phone: Option<&str>,
        source: &str,
    ) -> Result<YapeCredit, YapeError> {
        let operation = normalize_operation(operation)?;
        validate_recharge_amount(expected_amount)?;
        let guild_id = guild_id.map(str::trim).filter(|g| !g.is_empty());
        let phone = phone.map(str::trim).filter(|p| !p.is_empty());

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;

                // 1. Buscar la transacción del monitor en realtime_documents (o yape_transactions)
                let mut payment =
                    fetch_doc_for_update(&mut tx, "yape_transacciones", &operation).await?;
                if payment.is_none() {
                    let row = sqlx::query(
                        r#"
                        SELECT tipo, CAST(monto AS DOUBLE) AS monto_val, fecha_hora, fecha_registro, sender, telefono, mensaje, comprobante_url
                        FROM yape_transactions
                        WHERE operation_number = ?
                        FOR UPDATE
                        "#,
                    )
                    .bind(&operation)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(sqlx_to_data_err)?;

                    if let Some(r) = row {
                        // Sin `tipo` legible no se asume ENTRADA: el pago se rechaza más abajo.
                        let tipo: String = r
                            .try_get::<Option<String>, _>("tipo")
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        let monto_val: f64 = r.try_get("monto_val").unwrap_or(0.0);
                        let fecha_hora: Option<String> = r.try_get("fecha_hora").ok();
                        let fecha_reg: Option<String> = r.try_get("fecha_registro").ok();
                        let sender: Option<String> = r.try_get("sender").ok();
                        let telefono: Option<String> = r.try_get("telefono").ok();
                        let mensaje: Option<String> = r.try_get("mensaje").ok();
                        let comprobante_url: Option<String> = r.try_get("comprobante_url").ok();

                        let doc = json!({
                            "numero_operacion": operation,
                            "tipo": tipo,
                            "monto": monto_val,
                            "fecha_hora": fecha_hora,
                            "fecha_registro": fecha_reg,
                            "sender": sender,
                            "telefono": telefono,
                            "mensaje": mensaje,
                            "comprobante_url": comprobante_url,
                        });
                        let _ = upsert_realtime_doc(&mut tx, "yape_transacciones", &operation, &doc.to_string()).await;
                        payment = Some(doc);
                    }
                }
                let Some(payment) = payment else {
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
                let received_at =
                    text(&payment, "fecha_hora").or_else(|| text(&payment, "fecha_registro"));

                // 2. Bloquear la operación para impedir doble acreditación
                let existing_op =
                    fetch_doc_for_update(&mut tx, "yape_operations", &operation).await?;
                if let Some(op) = &existing_op {
                    let belongs = text(op, "discordId").as_deref() == Some(discord_id)
                        && text(op, "guildId").as_deref() == guild_id;
                    let credited = op.get("credited").and_then(Value::as_bool) == Some(true)
                        || text(op, "status").as_deref() == Some("credited");
                    if credited {
                        return Ok(YapeCredit::AlreadyCredited { own: belongs });
                    }
                    if !belongs {
                        return Ok(YapeCredit::ReservedByOther);
                    }
                }

                // 3. Bloquear y actualizar saldo del usuario + operación + auditoría
                let user_id = match guild_id {
                    Some(g) => format!("{g}_{discord_id}"),
                    None => discord_id.to_string(),
                };
                let existing_user =
                    fetch_doc_for_update(&mut tx, "faceit_users", &user_id).await?;
                let now_ts = timestamp_now();
                let now_ms = Utc::now().timestamp_millis();
                let reason = format!("Recarga Yape verificada (operación: {operation})");

                let prev_balance = existing_user
                    .as_ref()
                    .and_then(|u| amount_of_zero(u.get("wallet_balance")))
                    .unwrap_or(0.0);
                let new_balance = ((prev_balance + amount) * 100.0).round() / 100.0;

                let mut user_map = match existing_user {
                    Some(Value::Object(m)) => m,
                    _ => {
                        let mut m = Map::new();
                        m.insert("discordId".into(), json!(discord_id));
                        if let Some(g) = guild_id {
                            m.insert("guild_id".into(), json!(g));
                        }
                        m.insert("createdAt".into(), now_ts.clone());
                        m
                    }
                };
                user_map.insert("wallet_balance".into(), json!(new_balance));
                user_map.insert("last_balance_update".into(), json!(now_ms));
                user_map.insert("last_balance_reason".into(), json!(reason));
                // Solo se registra el celular si la cuenta no tiene uno: es el destino del pago
                // automático de retiros y no se cambia con una recarga.
                if let Some(p) = phone.filter(|_| !has_phone(&user_map)) {
                    user_map.insert("phoneNumber".into(), json!(p));
                }
                let user_val = Value::Object(user_map);

                let created_at = existing_op
                    .as_ref()
                    .and_then(|o| o.get("createdAt").cloned())
                    .unwrap_or_else(|| now_ts.clone());
                let op_val = json!({
                    "operationNumber": operation,
                    "discordId": discord_id,
                    "guildId": guild_id,
                    "amount": amount,
                    "credited": true,
                    "status": "credited",
                    "details": { "receivedAt": received_at },
                    "creditedAt": now_ts,
                    "createdAt": created_at,
                });

                let audit_id = format!("credited_{now_ms}_{operation}");
                let audit_val = json!({
                    "event": "credited",
                    "source": source,
                    "discordId": discord_id,
                    "guildId": guild_id,
                    "amount": amount,
                    "operationNumber": operation,
                    "createdAt": now_ts,
                });

                upsert_realtime_doc(
                    &mut tx,
                    "yape_operations",
                    &operation,
                    &op_val.to_string(),
                )
                .await?;
                upsert_realtime_doc(&mut tx, "faceit_users", &user_id, &user_val.to_string())
                    .await?;
                upsert_realtime_doc(
                    &mut tx,
                    "yape_audit_logs",
                    &audit_id,
                    &audit_val.to_string(),
                )
                .await?;

                sync_relational_upsert(&mut tx, "faceit_users", &user_id, &user_val).await?;
                sync_relational_upsert(&mut tx, "yape_operations", &operation, &op_val).await?;
                sync_relational_upsert(&mut tx, "yape_audit_logs", &audit_id, &audit_val).await?;
                let amount_cents = (amount * 100.0).round() as i64;
                let new_cents = (new_balance * 100.0).round() as i64;
                wallet::append_ledger(&mut tx, &user_id, amount_cents, new_cents, "recharge", &operation)
                    .await?;
                wallet::enqueue_notification(
                    &mut tx,
                    Some(discord_id),
                    &format!(
                        "Recarga Yape acreditada: S/ {amount:.2} (operación {operation}). Saldo actual: S/ {new_balance:.2}."
                    ),
                )
                .await?;

                tx.commit().await.map_err(sqlx_to_data_err)?;
                self.notify_write("yape_operations", &operation, RealtimeOp::Upsert);
                self.notify_write("faceit_users", &user_id, RealtimeOp::Upsert);
                self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);

                Ok(YapeCredit::Credited {
                    operation,
                    amount,
                    new_balance,
                })
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let payment = guard
                    .collections
                    .get("yape_transacciones")
                    .and_then(|c| c.get(&operation))
                    .cloned();
                let Some(payment) = payment else {
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
                let received_at =
                    text(&payment, "fecha_hora").or_else(|| text(&payment, "fecha_registro"));

                let existing_op = guard
                    .collections
                    .get("yape_operations")
                    .and_then(|c| c.get(&operation))
                    .cloned();
                if let Some(op) = &existing_op {
                    let belongs = text(op, "discordId").as_deref() == Some(discord_id)
                        && text(op, "guildId").as_deref() == guild_id;
                    let credited = op.get("credited").and_then(Value::as_bool) == Some(true)
                        || text(op, "status").as_deref() == Some("credited");
                    if credited {
                        return Ok(YapeCredit::AlreadyCredited { own: belongs });
                    }
                    if !belongs {
                        return Ok(YapeCredit::ReservedByOther);
                    }
                }

                let user_id = match guild_id {
                    Some(g) => format!("{g}_{discord_id}"),
                    None => discord_id.to_string(),
                };
                let existing_user = guard
                    .collections
                    .get("faceit_users")
                    .and_then(|c| c.get(&user_id))
                    .cloned();
                let now_ts = timestamp_now();
                let now_ms = Utc::now().timestamp_millis();
                let prev_balance = existing_user
                    .as_ref()
                    .and_then(|u| amount_of_zero(u.get("wallet_balance")))
                    .unwrap_or(0.0);
                let new_balance = ((prev_balance + amount) * 100.0).round() / 100.0;

                let mut user_map = match existing_user {
                    Some(Value::Object(m)) => m,
                    _ => {
                        let mut m = Map::new();
                        m.insert("discordId".into(), json!(discord_id));
                        if let Some(g) = guild_id {
                            m.insert("guild_id".into(), json!(g));
                        }
                        m.insert("createdAt".into(), now_ts.clone());
                        m
                    }
                };
                user_map.insert("wallet_balance".into(), json!(new_balance));
                user_map.insert("last_balance_update".into(), json!(now_ms));
                user_map.insert(
                    "last_balance_reason".into(),
                    json!(format!("Recarga Yape verificada (operación: {operation})")),
                );
                // Solo se registra el celular si la cuenta no tiene uno: es el destino del pago
                // automático de retiros y no se cambia con una recarga.
                if let Some(p) = phone.filter(|_| !has_phone(&user_map)) {
                    user_map.insert("phoneNumber".into(), json!(p));
                }

                let op_val = json!({
                    "operationNumber": operation,
                    "discordId": discord_id,
                    "guildId": guild_id,
                    "amount": amount,
                    "credited": true,
                    "status": "credited",
                    "details": { "receivedAt": received_at },
                    "creditedAt": now_ts,
                    "createdAt": now_ts,
                });
                let audit_id = format!("credited_{now_ms}_{operation}");
                let audit_val = json!({
                    "event": "credited",
                    "source": source,
                    "discordId": discord_id,
                    "guildId": guild_id,
                    "amount": amount,
                    "operationNumber": operation,
                    "createdAt": now_ts,
                });

                guard
                    .collections
                    .entry("yape_operations".into())
                    .or_default()
                    .insert(operation.clone(), op_val);
                guard
                    .collections
                    .entry("faceit_users".into())
                    .or_default()
                    .insert(user_id.clone(), Value::Object(user_map));
                guard
                    .collections
                    .entry("yape_audit_logs".into())
                    .or_default()
                    .insert(audit_id.clone(), audit_val);
                drop(guard);

                self.notify_write("yape_operations", &operation, RealtimeOp::Upsert);
                self.notify_write("faceit_users", &user_id, RealtimeOp::Upsert);
                self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);

                Ok(YapeCredit::Credited {
                    operation,
                    amount,
                    new_balance,
                })
            }
        }
    }

    /// Solicita un retiro descontando atómicamente del saldo del jugador en MySQL.
    pub async fn request_withdrawal(
        &self,
        discord_id: &str,
        amount: f64,
        phone: &str,
        source: &str,
    ) -> Result<WithdrawalRequested, WithdrawError> {
        let amount_cents = to_cents(amount).ok_or_else(|| {
            WithdrawError::Invalid("Ingresa un monto válido (máximo 2 decimales).".into())
        })?;
        let phone = normalize_phone(phone).ok_or_else(|| {
            WithdrawError::Invalid("Ingresa un celular Yape válido de 9 dígitos.".into())
        })?;
        let amount = amount_cents as f64 / 100.0;

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                let Some(user_val) =
                    fetch_doc_for_update(&mut tx, "faceit_users", discord_id).await?
                else {
                    return Err(WithdrawError::Invalid(
                        "No tienes saldo en tu billetera.".into(),
                    ));
                };
                let now_ms = Utc::now().timestamp_millis();
                let plan = plan_withdrawal(&user_val, amount_cents, &phone, now_ms)?;
                let (bal_cents, remaining_cents) = (plan.balance_cents, plan.remaining_cents);

                let seq = WITHDRAW_SEQ.fetch_add(1, Ordering::Relaxed);
                let id = format!("wd_{now_ms}_{seq}_{discord_id}");
                let now_ts = timestamp_now();
                let new_balance = remaining_cents as f64 / 100.0;

                let mut user_map = user_val.as_object().cloned().unwrap_or_default();
                user_map.insert("wallet_balance".into(), json!(new_balance));
                user_map.insert("last_withdrawal_at".into(), json!(now_ms));
                user_map.insert("last_balance_update".into(), json!(now_ms));
                user_map.insert(
                    "last_balance_reason".into(),
                    json!(format!("Solicitud de retiro Yape ({id})")),
                );
                user_map.insert("withdraw_day".into(), json!(plan.day));
                user_map.insert(
                    "withdrawn_today_cents".into(),
                    json!(plan.withdrawn_today_cents),
                );
                let updated_user = Value::Object(user_map);

                let withdrawal_doc = json!({
                    "type": "withdrawal",
                    "status": plan.status,
                    "reviewReason": plan.review_reason,
                    "withdrawDay": plan.day,
                    "discordId": discord_id,
                    "amount": amount,
                    "amountCents": amount_cents,
                    "phoneNumber": phone,
                    "balanceBefore": bal_cents as f64 / 100.0,
                    "balanceAfter": new_balance,
                    "source": source,
                    "createdAt": now_ts,
                });
                let audit_id = format!("withdrawal_{id}");
                let audit_doc = json!({
                    "event": "withdrawal_requested",
                    "source": source,
                    "discordId": discord_id,
                    "amount": amount,
                    "operationNumber": id,
                    "createdAt": now_ts,
                });

                upsert_realtime_doc(
                    &mut tx,
                    "faceit_users",
                    discord_id,
                    &updated_user.to_string(),
                )
                .await?;
                upsert_realtime_doc(
                    &mut tx,
                    "yape_withdrawals",
                    &id,
                    &withdrawal_doc.to_string(),
                )
                .await?;
                upsert_realtime_doc(
                    &mut tx,
                    "yape_audit_logs",
                    &audit_id,
                    &audit_doc.to_string(),
                )
                .await?;

                sync_relational_upsert(&mut tx, "faceit_users", discord_id, &updated_user).await?;
                sync_relational_upsert(&mut tx, "yape_withdrawals", &id, &withdrawal_doc).await?;
                sync_relational_upsert(&mut tx, "yape_audit_logs", &audit_id, &audit_doc).await?;
                wallet::append_ledger(
                    &mut tx,
                    discord_id,
                    -amount_cents,
                    remaining_cents,
                    "withdrawal",
                    &id,
                )
                .await?;
                let dm = if plan.status == crate::withdraw::STATUS_REVIEW {
                    format!(
                        "Registramos tu retiro de S/ {amount:.2} al {phone}. Queda en revisión antes de enviarse ({}).",
                        plan.review_reason.unwrap_or("revisión")
                    )
                } else {
                    format!("Registramos tu retiro de S/ {amount:.2}; se enviará por Yape al {phone} en unos minutos.")
                };
                wallet::enqueue_notification(
                    &mut tx,
                    Some(discord_id),
                    &format!("{dm} Saldo actual: S/ {new_balance:.2}."),
                )
                .await?;
                if plan.status == crate::withdraw::STATUS_REVIEW {
                    wallet::enqueue_notification(
                        &mut tx,
                        None,
                        &format!(
                            "Retiro {id} en revisión: <@{discord_id}> S/ {amount:.2} al {phone} ({}).",
                            plan.review_reason.unwrap_or("revisión")
                        ),
                    )
                    .await?;
                }

                tx.commit().await.map_err(sqlx_to_data_err)?;
                self.notify_write("faceit_users", discord_id, RealtimeOp::Upsert);
                self.notify_write("yape_withdrawals", &id, RealtimeOp::Upsert);
                self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);

                Ok(WithdrawalRequested {
                    id,
                    amount,
                    phone,
                    new_balance,
                    status: plan.status.into(),
                })
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let Some(user_val) = guard
                    .collections
                    .get("faceit_users")
                    .and_then(|c| c.get(discord_id))
                    .cloned()
                else {
                    return Err(WithdrawError::Invalid(
                        "No tienes saldo en tu billetera.".into(),
                    ));
                };
                let now_ms = Utc::now().timestamp_millis();
                let plan = plan_withdrawal(&user_val, amount_cents, &phone, now_ms)?;
                let (bal_cents, remaining_cents) = (plan.balance_cents, plan.remaining_cents);

                let seq = WITHDRAW_SEQ.fetch_add(1, Ordering::Relaxed);
                let id = format!("wd_{now_ms}_{seq}_{discord_id}");
                let now_ts = timestamp_now();
                let new_balance = remaining_cents as f64 / 100.0;

                let mut user_map = user_val.as_object().cloned().unwrap_or_default();
                user_map.insert("wallet_balance".into(), json!(new_balance));
                user_map.insert("last_withdrawal_at".into(), json!(now_ms));
                user_map.insert("last_balance_update".into(), json!(now_ms));
                user_map.insert(
                    "last_balance_reason".into(),
                    json!(format!("Solicitud de retiro Yape ({id})")),
                );
                user_map.insert("withdraw_day".into(), json!(plan.day));
                user_map.insert(
                    "withdrawn_today_cents".into(),
                    json!(plan.withdrawn_today_cents),
                );

                let withdrawal_doc = json!({
                    "type": "withdrawal",
                    "status": plan.status,
                    "reviewReason": plan.review_reason,
                    "withdrawDay": plan.day,
                    "discordId": discord_id,
                    "amount": amount,
                    "amountCents": amount_cents,
                    "phoneNumber": phone,
                    "balanceBefore": bal_cents as f64 / 100.0,
                    "balanceAfter": new_balance,
                    "source": source,
                    "createdAt": now_ts,
                });
                let audit_id = format!("withdrawal_{id}");
                let audit_doc = json!({
                    "event": "withdrawal_requested",
                    "source": source,
                    "discordId": discord_id,
                    "amount": amount,
                    "operationNumber": id,
                    "createdAt": now_ts,
                });

                guard
                    .collections
                    .entry("faceit_users".into())
                    .or_default()
                    .insert(discord_id.to_string(), Value::Object(user_map));
                guard
                    .collections
                    .entry("yape_withdrawals".into())
                    .or_default()
                    .insert(id.clone(), withdrawal_doc);
                guard
                    .collections
                    .entry("yape_audit_logs".into())
                    .or_default()
                    .insert(audit_id.clone(), audit_doc);
                drop(guard);

                self.notify_write("faceit_users", discord_id, RealtimeOp::Upsert);
                self.notify_write("yape_withdrawals", &id, RealtimeOp::Upsert);
                self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);

                Ok(WithdrawalRequested {
                    id,
                    amount,
                    phone,
                    new_balance,
                    status: plan.status.into(),
                })
            }
        }
    }

    /// Devuelve el saldo de los retiros que el worker de Yape rechazó porque el envío falló
    /// antes de confirmar (el dinero no salió). Devuelve los ids reembolsados ahora.
    ///
    /// Idempotente y sin carreras: cada reembolso bloquea (`FOR UPDATE`) la fila relacional,
    /// el documento del retiro (que es la prueba del débito) y el del usuario, y marca
    /// `refundedAt` en la misma transacción que sube el saldo. Sin documento de débito no se
    /// devuelve nada: una fila `rejected` inventada no crea saldo.
    pub async fn refund_rejected_withdrawals(&self, limit: u32) -> Result<Vec<String>, DataError> {
        let Storage::Pool { pool, .. } = &self.storage else {
            // En memoria no existe la tabla relacional que escribe el worker.
            return Ok(Vec::new());
        };
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM yape_withdrawals \
             WHERE status = 'rejected' AND (admin_note IS NULL OR admin_note NOT LIKE 'Reembolsado%') \
             ORDER BY updated_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(sqlx_to_data_err)?;

        let mut refunded = Vec::new();
        for id in ids {
            let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM yape_withdrawals WHERE id = ? FOR UPDATE")
                    .bind(&id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(sqlx_to_data_err)?;
            if status.as_deref() != Some("rejected") {
                continue;
            }
            let Some(Value::Object(mut wd)) =
                fetch_doc_for_update(&mut tx, "yape_withdrawals", &id).await?
            else {
                tracing::error!("[Reembolso] {id}: rechazado sin documento de débito; no se devuelve nada");
                sqlx::query("UPDATE yape_withdrawals SET admin_note = 'Reembolsado: no (sin débito registrado)' WHERE id = ?")
                    .bind(&id)
                    .execute(&mut *tx)
                    .await
                    .map_err(sqlx_to_data_err)?;
                tx.commit().await.map_err(sqlx_to_data_err)?;
                continue;
            };
            if wd.contains_key("refundedAt") {
                continue;
            }
            let amount_cents = wd.get("amountCents").and_then(Value::as_i64).unwrap_or(0);
            let Some(discord_id) = text(&Value::Object(wd.clone()), "discordId") else {
                continue;
            };
            if amount_cents <= 0 {
                continue;
            }
            let Some(Value::Object(mut user)) =
                fetch_doc_for_update(&mut tx, "faceit_users", &discord_id).await?
            else {
                tracing::error!("[Reembolso] {id}: el usuario {discord_id} ya no existe; queda pendiente");
                continue;
            };

            let now_ms = Utc::now().timestamp_millis();
            let now_ts = timestamp_now();
            let new_cents =
                crate::withdraw::balance_cents(&Value::Object(user.clone())) + amount_cents;
            user.insert("wallet_balance".into(), json!(new_cents as f64 / 100.0));
            user.insert("last_balance_update".into(), json!(now_ms));
            user.insert(
                "last_balance_reason".into(),
                json!(format!("Reembolso de retiro no enviado ({id})")),
            );
            // El retiro no salió: no cuenta para el tope diario de su día.
            if user.get("withdraw_day") == wd.get("withdrawDay") {
                let today = user
                    .get("withdrawn_today_cents")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                user.insert(
                    "withdrawn_today_cents".into(),
                    json!((today - amount_cents).max(0)),
                );
            }
            let user_val = Value::Object(user);
            wd.insert("refundedAt".into(), now_ts.clone());
            wd.insert("refundCents".into(), json!(amount_cents));
            let wd_val = Value::Object(wd);
            let audit_id = format!("refund_{id}");
            let audit_val = json!({
                "event": "withdrawal_refunded",
                "source": "yape_auto",
                "discordId": discord_id,
                "amount": amount_cents as f64 / 100.0,
                "operationNumber": id,
                "createdAt": now_ts,
            });

            upsert_realtime_doc(&mut tx, "faceit_users", &discord_id, &user_val.to_string())
                .await?;
            upsert_realtime_doc(&mut tx, "yape_withdrawals", &id, &wd_val.to_string()).await?;
            upsert_realtime_doc(&mut tx, "yape_audit_logs", &audit_id, &audit_val.to_string())
                .await?;
            sync_relational_upsert(&mut tx, "faceit_users", &discord_id, &user_val).await?;
            sync_relational_upsert(&mut tx, "yape_audit_logs", &audit_id, &audit_val).await?;
            sqlx::query(
                "UPDATE yape_withdrawals SET admin_note = LEFT(CONCAT('Reembolsado | ', COALESCE(admin_note, '')), 255) WHERE id = ?",
            )
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_to_data_err)?;
            wallet::append_ledger(
                &mut tx,
                &discord_id,
                amount_cents,
                new_cents,
                "withdrawal_refund",
                &id,
            )
            .await?;
            wallet::enqueue_notification(
                &mut tx,
                Some(&discord_id),
                &format!(
                    "Tu retiro de S/ {:.2} no se pudo enviar por Yape: te devolvimos el saldo. Saldo actual: S/ {:.2}.",
                    amount_cents as f64 / 100.0,
                    new_cents as f64 / 100.0
                ),
            )
            .await?;
            tx.commit().await.map_err(sqlx_to_data_err)?;

            self.notify_write("faceit_users", &discord_id, RealtimeOp::Upsert);
            self.notify_write("yape_withdrawals", &id, RealtimeOp::Upsert);
            self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);
            tracing::info!("[Reembolso] {id}: devueltos S/ {} a {discord_id}", amount_cents / 100);
            refunded.push(id);
        }
        Ok(refunded)
    }

    /// Paga el pozo de `match_id` a `winners` en una única transacción ACID en MySQL.
    pub async fn pay_match_winners(
        &self,
        match_id: &str,
        bet_cents: i64,
        total_players: usize,
        winners: &[String],
        source: &str,
    ) -> Result<PayoutOutcome, PayoutError> {
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
        let now_ts = timestamp_now();
        let now_ms = Utc::now().timestamp_millis();
        // El pozo sale de los jugadores que de verdad pagaron apuesta (`match_stakes`), no del
        // documento de la partida: un premio nunca crea dinero.
        let build = |players: usize| -> Result<(i64, Vec<i64>, Value), PayoutError> {
            let total = bet_cents
                .checked_mul(players as i64)
                .ok_or_else(|| PayoutError::Invalid("pozo fuera de rango".into()))?;
            let shares = split_pot(total, winners.len());
            let winners_json: Vec<Value> = winners
                .iter()
                .zip(&shares)
                .map(|(w, c)| json!({ "discordId": w, "amountCents": c }))
                .collect();
            let doc = json!({
                "matchId": match_id,
                "status": "paid",
                "betCents": bet_cents,
                "totalPotCents": total,
                "houseCents": house_cents(total, &shares),
                "winners": winners_json,
                "source": source,
                "createdAt": now_ts,
            });
            Ok((total, shares, doc))
        };

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;
                let players =
                    match wallet::take_stakes_for_payout(&mut tx, match_id, bet_cents, &winners)
                        .await?
                    {
                        wallet::StakesForPayout::Ready { players } => players,
                        wallet::StakesForPayout::AlreadyPaid => {
                            return Ok(PayoutOutcome::AlreadyPaid)
                        }
                    };
                let (_total, shares, payout_doc) = build(players.len())?;
                // INSERT simple (no upsert): la clave primaria es la que impide el pago doble,
                // con cualquier nivel de aislamiento. Un `FOR UPDATE` sobre una fila que aún no
                // existe no bloquea nada en READ COMMITTED.
                if !insert_realtime_doc_once(
                    &mut tx,
                    "match_payouts",
                    match_id,
                    &payout_doc.to_string(),
                )
                .await?
                {
                    return Ok(PayoutOutcome::AlreadyPaid);
                }
                sync_relational_upsert(&mut tx, "match_payouts", match_id, &payout_doc).await?;

                for (winner, cents) in winners.iter().zip(&shares) {
                    let amount = *cents as f64 / 100.0;
                    let existing =
                        fetch_doc_for_update(&mut tx, "faceit_users", winner).await?;
                    let prev = existing
                        .as_ref()
                        .and_then(|u| amount_of_zero(u.get("wallet_balance")))
                        .unwrap_or(0.0);
                    let new_bal = ((prev + amount) * 100.0).round() / 100.0;
                    let mut u_map = existing
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    u_map.insert("discordId".into(), json!(winner));
                    u_map.insert("wallet_balance".into(), json!(new_bal));
                    u_map.insert("last_balance_update".into(), json!(now_ms));
                    u_map.insert(
                        "last_balance_reason".into(),
                        json!(format!("Premio de la partida {match_id}")),
                    );
                    let u_val = Value::Object(u_map);
                    upsert_realtime_doc(&mut tx, "faceit_users", winner, &u_val.to_string())
                        .await?;
                    sync_relational_upsert(&mut tx, "faceit_users", winner, &u_val).await?;
                    let new_cents = (new_bal * 100.0).round() as i64;
                    wallet::append_ledger(&mut tx, winner, *cents, new_cents, "prize", match_id)
                        .await?;
                    wallet::enqueue_notification(
                        &mut tx,
                        Some(winner),
                        &format!(
                            "¡Ganaste la partida {match_id}! Premio: S/ {amount:.2}. Saldo actual: S/ {new_bal:.2}."
                        ),
                    )
                    .await?;

                    let audit_id = format!("payout_{match_id}_{winner}");
                    let audit_val = json!({
                        "event": "match_payout",
                        "source": source,
                        "discordId": winner,
                        "amount": amount,
                        "operationNumber": match_id,
                        "createdAt": now_ts,
                    });
                    upsert_realtime_doc(
                        &mut tx,
                        "yape_audit_logs",
                        &audit_id,
                        &audit_val.to_string(),
                    )
                    .await?;
                    sync_relational_upsert(&mut tx, "yape_audit_logs", &audit_id, &audit_val)
                        .await?;
                }

                tx.commit().await.map_err(sqlx_to_data_err)?;
                self.notify_write("match_payouts", match_id, RealtimeOp::Upsert);
                for winner in &winners {
                    self.notify_write("faceit_users", winner, RealtimeOp::Upsert);
                    self.notify_write(
                        "yape_audit_logs",
                        &format!("payout_{match_id}_{winner}"),
                        RealtimeOp::Upsert,
                    );
                }

                Ok(PayoutOutcome::Paid(
                    winners.into_iter().zip(shares).collect(),
                ))
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                if guard
                    .collections
                    .get("match_payouts")
                    .and_then(|c| c.get(match_id))
                    .is_some()
                {
                    return Ok(PayoutOutcome::AlreadyPaid);
                }
                let players = match wallet::take_stakes_for_payout_mem(
                    &mut guard,
                    match_id,
                    bet_cents,
                    &winners,
                )? {
                    wallet::StakesForPayout::Ready { players } => players,
                    wallet::StakesForPayout::AlreadyPaid => return Ok(PayoutOutcome::AlreadyPaid),
                };
                let (_total, shares, payout_doc) = build(players.len())?;
                guard
                    .collections
                    .entry("match_payouts".into())
                    .or_default()
                    .insert(match_id.to_string(), payout_doc);

                for (winner, cents) in winners.iter().zip(&shares) {
                    let amount = *cents as f64 / 100.0;
                    let existing = guard
                        .collections
                        .get("faceit_users")
                        .and_then(|c| c.get(winner))
                        .cloned();
                    let prev = existing
                        .as_ref()
                        .and_then(|u| amount_of_zero(u.get("wallet_balance")))
                        .unwrap_or(0.0);
                    let new_bal = ((prev + amount) * 100.0).round() / 100.0;
                    let mut u_map = existing
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    u_map.insert("discordId".into(), json!(winner));
                    u_map.insert("wallet_balance".into(), json!(new_bal));
                    u_map.insert("last_balance_update".into(), json!(now_ms));
                    u_map.insert(
                        "last_balance_reason".into(),
                        json!(format!("Premio de la partida {match_id}")),
                    );
                    guard
                        .collections
                        .entry("faceit_users".into())
                        .or_default()
                        .insert(winner.clone(), Value::Object(u_map));

                    let audit_id = format!("payout_{match_id}_{winner}");
                    let audit_val = json!({
                        "event": "match_payout",
                        "source": source,
                        "discordId": winner,
                        "amount": amount,
                        "operationNumber": match_id,
                        "createdAt": now_ts,
                    });
                    guard
                        .collections
                        .entry("yape_audit_logs".into())
                        .or_default()
                        .insert(audit_id, audit_val);
                }
                drop(guard);

                self.notify_write("match_payouts", match_id, RealtimeOp::Upsert);
                for winner in &winners {
                    self.notify_write("faceit_users", winner, RealtimeOp::Upsert);
                    self.notify_write(
                        "yape_audit_logs",
                        &format!("payout_{match_id}_{winner}"),
                        RealtimeOp::Upsert,
                    );
                }

                Ok(PayoutOutcome::Paid(
                    winners.into_iter().zip(shares).collect(),
                ))
            }
        }
    }

    /// Obtiene el saldo en soles de un usuario (0.0 si no existe o no tiene saldo).
    pub async fn get_user_balance(
        &self,
        discord_id: &str,
        guild_id: Option<&str>,
    ) -> Result<f64, DataError> {
        let discord_id = discord_id.trim();
        if discord_id.is_empty() {
            return Ok(0.0);
        }
        match &self.storage {
            Storage::Pool { .. } => {
                let mut doc = None;
                if let Some(gid) = guild_id.map(str::trim).filter(|g| !g.is_empty()) {
                    let target_id = format!("{gid}_{discord_id}");
                    doc = self.get_doc::<Value>("faceit_users", &target_id).await?;
                }
                if doc.as_ref().and_then(|d| d.get("wallet_balance")).is_none() {
                    doc = self.get_doc::<Value>("faceit_users", discord_id).await?;
                }
                if doc.as_ref().and_then(|d| d.get("wallet_balance")).is_none() {
                    let q = Query {
                        filters: vec![(
                            "discordId".into(),
                            Op::Eq,
                            crate::firestore::Value::String(discord_id.to_string()),
                        )],
                        limit: Some(1),
                        ..Default::default()
                    };
                    let docs = self.query::<Value>("faceit_users", q).await?;
                    doc = docs.into_iter().next().map(|(_, v)| v);
                }
                let cents = doc.as_ref().map(crate::withdraw::balance_cents).unwrap_or(0);
                Ok(cents as f64 / 100.0)
            }
            Storage::Memory(mem) => {
                let guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let col = match guard.collections.get("faceit_users") {
                    Some(c) => c,
                    None => return Ok(0.0),
                };
                if let Some(gid) = guild_id.map(str::trim).filter(|g| !g.is_empty()) {
                    let target_id = format!("{gid}_{discord_id}");
                    if let Some(doc) = col.get(&target_id) {
                        if doc.get("wallet_balance").is_some() {
                            let cents = crate::withdraw::balance_cents(doc);
                            return Ok(cents as f64 / 100.0);
                        }
                    }
                }
                if let Some(doc) = col.get(discord_id) {
                    let cents = crate::withdraw::balance_cents(doc);
                    return Ok(cents as f64 / 100.0);
                }
                for doc in col.values() {
                    if text(doc, "discordId").as_deref() == Some(discord_id) {
                        let cents = crate::withdraw::balance_cents(doc);
                        return Ok(cents as f64 / 100.0);
                    }
                }
                Ok(0.0)
            }
        }
    }

    /// Ajusta el saldo de un usuario de forma atómica pasando por el libro contable (`wallet_ledger`).
    pub async fn adjust_user_balance(
        &self,
        discord_id: &str,
        guild_id: Option<&str>,
        adjustment: BalanceAdjustment,
        reason: &str,
        admin_name: &str,
        source: &str,
    ) -> Result<f64, DataError> {
        let discord_id = discord_id.trim();
        if discord_id.is_empty() {
            return Err(DataError::Firestore("discord_id vacío".into()));
        }
        let now_ts = timestamp_now();
        let now_ms = Utc::now().timestamp_millis();
        let audit_reason = if reason.trim().is_empty() {
            format!("[Admin: {admin_name}] Ajuste manual")
        } else {
            format!("[Admin: {admin_name}] {reason}")
        };

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;

                let target_doc_id = if let Some(gid) = guild_id.map(str::trim).filter(|g| !g.is_empty()) {
                    let guild_doc_id = format!("{gid}_{discord_id}");
                    if fetch_doc_for_update(&mut tx, "faceit_users", &guild_doc_id).await?.is_some() {
                        guild_doc_id
                    } else if fetch_doc_for_update(&mut tx, "faceit_users", discord_id).await?.is_some() {
                        discord_id.to_string()
                    } else {
                        guild_doc_id
                    }
                } else {
                    discord_id.to_string()
                };

                let existing_user = fetch_doc_for_update(&mut tx, "faceit_users", &target_doc_id).await?;
                let prev_cents = existing_user.as_ref().map(crate::withdraw::balance_cents).unwrap_or(0);
                let prev_balance = prev_cents as f64 / 100.0;

                let new_balance = match adjustment {
                    BalanceAdjustment::Set(s) => (s.max(0.0) * 100.0).round() / 100.0,
                    BalanceAdjustment::Delta(d) => ((prev_balance + d).max(0.0) * 100.0).round() / 100.0,
                };
                let new_cents = (new_balance * 100.0).round() as i64;
                let delta_cents = new_cents - prev_cents;

                let mut user_map = match existing_user {
                    Some(Value::Object(m)) => m,
                    _ => {
                        let mut m = Map::new();
                        m.insert("discordId".into(), json!(discord_id));
                        if let Some(gid) = guild_id.map(str::trim).filter(|g| !g.is_empty()) {
                            m.insert("guild_id".into(), json!(gid));
                        }
                        m.insert("createdAt".into(), now_ts.clone());
                        m
                    }
                };
                user_map.insert("wallet_balance".into(), json!(new_balance));
                user_map.insert("last_balance_update".into(), json!(now_ms));
                user_map.insert("last_balance_reason".into(), json!(audit_reason));
                let user_val = Value::Object(user_map);

                upsert_realtime_doc(&mut tx, "faceit_users", &target_doc_id, &user_val.to_string()).await?;
                sync_relational_upsert(&mut tx, "faceit_users", &target_doc_id, &user_val).await?;

                let ref_str = format!("{admin_name}: {source}");
                wallet::append_ledger(
                    &mut tx,
                    &target_doc_id,
                    delta_cents,
                    new_cents,
                    "admin_adjustment",
                    &ref_str,
                )
                .await?;

                let audit_id = format!("admin_{now_ms}_{discord_id}");
                let audit_val = json!({
                    "event": "admin_adjustment",
                    "source": source,
                    "admin": admin_name,
                    "discordId": discord_id,
                    "guildId": guild_id,
                    "previousBalance": prev_balance,
                    "newBalance": new_balance,
                    "delta": delta_cents as f64 / 100.0,
                    "reason": reason,
                    "createdAt": now_ts,
                });
                upsert_realtime_doc(&mut tx, "yape_audit_logs", &audit_id, &audit_val.to_string()).await?;
                sync_relational_upsert(&mut tx, "yape_audit_logs", &audit_id, &audit_val).await?;

                let log_id = format!("log_{now_ms}_{discord_id}");
                let log_val = json!({
                    "type": "info",
                    "message": format!(
                        "Saldo de <@{discord_id}> modificado a S/ {new_balance:.2} por {admin_name}. Motivo: {reason}"
                    ),
                    "category": "wallet",
                    "guildId": guild_id,
                    "timestamp": now_ts,
                });
                upsert_realtime_doc(&mut tx, "faceit_bot_logs", &log_id, &log_val.to_string()).await?;
                sync_relational_upsert(&mut tx, "faceit_bot_logs", &log_id, &log_val).await?;

                wallet::enqueue_notification(
                    &mut tx,
                    Some(discord_id),
                    &format!("Tu saldo ha sido actualizado a S/ {new_balance:.2}. Motivo: {reason}"),
                )
                .await?;

                tx.commit().await.map_err(sqlx_to_data_err)?;

                self.notify_write("faceit_users", &target_doc_id, RealtimeOp::Upsert);
                self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);
                self.notify_write("faceit_bot_logs", &log_id, RealtimeOp::Upsert);

                Ok(new_balance)
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let target_doc_id = if let Some(gid) = guild_id.map(str::trim).filter(|g| !g.is_empty()) {
                    let guild_doc_id = format!("{gid}_{discord_id}");
                    if guard.collections.get("faceit_users").and_then(|c| c.get(&guild_doc_id)).is_some() {
                        guild_doc_id
                    } else if guard.collections.get("faceit_users").and_then(|c| c.get(discord_id)).is_some() {
                        discord_id.to_string()
                    } else {
                        guild_doc_id
                    }
                } else {
                    discord_id.to_string()
                };

                let existing = guard
                    .collections
                    .get("faceit_users")
                    .and_then(|c| c.get(&target_doc_id))
                    .cloned();
                let prev_cents = existing.as_ref().map(crate::withdraw::balance_cents).unwrap_or(0);
                let prev_balance = prev_cents as f64 / 100.0;

                let new_balance = match adjustment {
                    BalanceAdjustment::Set(s) => (s.max(0.0) * 100.0).round() / 100.0,
                    BalanceAdjustment::Delta(d) => ((prev_balance + d).max(0.0) * 100.0).round() / 100.0,
                };
                let delta_cents = ((new_balance - prev_balance) * 100.0).round() as i64;

                let mut user_map = match existing {
                    Some(Value::Object(m)) => m,
                    _ => {
                        let mut m = Map::new();
                        m.insert("discordId".into(), json!(discord_id));
                        if let Some(gid) = guild_id.map(str::trim).filter(|g| !g.is_empty()) {
                            m.insert("guild_id".into(), json!(gid));
                        }
                        m.insert("createdAt".into(), now_ts.clone());
                        m
                    }
                };
                user_map.insert("wallet_balance".into(), json!(new_balance));
                user_map.insert("last_balance_update".into(), json!(now_ms));
                user_map.insert("last_balance_reason".into(), json!(audit_reason));

                guard
                    .collections
                    .entry("faceit_users".into())
                    .or_default()
                    .insert(target_doc_id.clone(), Value::Object(user_map));

                let audit_id = format!("admin_{now_ms}_{discord_id}");
                let audit_val = json!({
                    "event": "admin_adjustment",
                    "source": source,
                    "admin": admin_name,
                    "discordId": discord_id,
                    "guildId": guild_id,
                    "previousBalance": prev_balance,
                    "newBalance": new_balance,
                    "delta": delta_cents as f64 / 100.0,
                    "reason": reason,
                    "createdAt": now_ts,
                });
                guard
                    .collections
                    .entry("yape_audit_logs".into())
                    .or_default()
                    .insert(audit_id.clone(), audit_val);

                let log_id = format!("log_{now_ms}_{discord_id}");
                let log_val = json!({
                    "type": "info",
                    "message": format!(
                        "Saldo de <@{discord_id}> modificado a S/ {new_balance:.2} por {admin_name}. Motivo: {reason}"
                    ),
                    "category": "wallet",
                    "guildId": guild_id,
                    "timestamp": now_ts,
                });
                guard
                    .collections
                    .entry("faceit_bot_logs".into())
                    .or_default()
                    .insert(log_id.clone(), log_val);

                drop(guard);

                self.notify_write("faceit_users", &target_doc_id, RealtimeOp::Upsert);
                self.notify_write("yape_audit_logs", &audit_id, RealtimeOp::Upsert);
                self.notify_write("faceit_bot_logs", &log_id, RealtimeOp::Upsert);

                Ok(new_balance)
            }
        }
    }

    /// Procesa la compra de productos en la tienda mediante un depósito verificado de Yape.
    pub async fn checkout_purchase(
        &self,
        operation_number: &str,
        expected_amount: f64,
        customer: &Value,
        items: &Value,
    ) -> Result<String, CheckoutError> {
        let operation = normalize_operation(operation_number)
            .map_err(|e| CheckoutError::Invalid(e.to_string()))?;
        if expected_amount <= 0.0 {
            return Err(CheckoutError::Invalid("Monto esperado inválido.".into()));
        }

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;

                // 1. Verificar si la operación ya se usó en un pedido
                let existing_order = sqlx::query(
                    r#"
                    SELECT id FROM orders WHERE charge_id = ? OR reference_code = ?
                    UNION
                    SELECT doc_id FROM realtime_documents WHERE collection = 'orders' AND (JSON_EXTRACT(doc_json, '$.chargeId') = ? OR JSON_EXTRACT(doc_json, '$.referenceCode') = ?)
                    LIMIT 1
                    "#,
                )
                .bind(&operation)
                .bind(&operation)
                .bind(&operation)
                .bind(&operation)
                .fetch_optional(&mut *tx)
                .await
                .map_err(sqlx_to_data_err)?;

                if existing_order.is_some() {
                    return Err(CheckoutError::AlreadyUsed);
                }

                // 2. Verificar el pago en yape_transacciones o yape_transactions
                let payment = fetch_doc_for_update(&mut tx, "yape_transacciones", &operation).await?;
                let (tipo, monto) = if let Some(p) = payment {
                    let tipo = text(&p, "tipo").unwrap_or_default();
                    let monto = amount_of(p.get("monto")).unwrap_or(0.0);
                    (tipo, monto)
                } else {
                    let row = sqlx::query(
                        "SELECT tipo, CAST(monto AS DOUBLE) AS monto_val FROM yape_transactions WHERE operation_number = ? FOR UPDATE",
                    )
                    .bind(&operation)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(sqlx_to_data_err)?;

                    let Some(r) = row else {
                        return Err(CheckoutError::PaymentNotFound);
                    };
                    let tipo: String = r.try_get("tipo").unwrap_or_default();
                    let monto: f64 = r.try_get("monto_val").unwrap_or(0.0);
                    (tipo, monto)
                };

                if tipo.trim().to_uppercase() != "ENTRADA" {
                    return Err(CheckoutError::PaymentNotEntry);
                }
                if (monto * 100.0).round() != (expected_amount * 100.0).round() {
                    return Err(CheckoutError::AmountMismatch(monto));
                }

                // 3. Crear orden
                let now_ms = Utc::now().timestamp_millis();
                let now_ts = timestamp_now();
                let now_iso = Utc::now().to_rfc3339();
                let seq = WITHDRAW_SEQ.fetch_add(1, Ordering::Relaxed) % 900 + 100;
                let order_id = format!("ORD-{now_ms}-{seq}");

                // La operación se reclama en `yape_operations`, la misma marca que usa la recarga
                // de saldo: un pago sirve para una compra o para una recarga, nunca para las dos.
                // El INSERT único también frena dos compras simultáneas con la misma operación.
                let claim = json!({
                    "operationNumber": operation,
                    "discordId": "",
                    "amount": monto,
                    "credited": true,
                    "status": "credited",
                    "usedBy": "order",
                    "orderId": order_id,
                    "createdAt": now_ts,
                });
                if !insert_realtime_doc_once(&mut tx, "yape_operations", &operation, &claim.to_string()).await? {
                    return Err(CheckoutError::AlreadyUsed);
                }
                sync_relational_upsert(&mut tx, "yape_operations", &operation, &claim).await?;

                let order_val = json!({
                    "orderId": order_id,
                    "customer": customer,
                    "items": items,
                    "totalAmount": expected_amount,
                    "currency": "PEN",
                    "paymentMethod": "yape_transferencia",
                    "chargeId": operation,
                    "referenceCode": operation,
                    "status": "COMPLETADA",
                    "createdAt": now_ts,
                    "createdAtIso": now_iso,
                });

                upsert_realtime_doc(&mut tx, "orders", &order_id, &order_val.to_string()).await?;
                sync_relational_upsert(&mut tx, "orders", &order_id, &order_val).await?;

                // 4. Si el cliente indicó discordId y compró packs de créditos, acreditar a su billetera
                let discord_id = customer.get("discordId").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
                if let Some(did) = discord_id {
                    let mut credits_to_add: f64 = 0.0;
                    if let Some(items_arr) = items.as_array() {
                        for item in items_arr {
                            let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                            let qty = item.get("quantity").and_then(Value::as_u64).unwrap_or(1) as f64;
                            if item_id == "prod_pack_10" {
                                credits_to_add += 10.0 * qty;
                            } else if item_id == "prod_pack_25" {
                                credits_to_add += 25.0 * qty;
                            }
                        }
                    }
                    if credits_to_add > 0.0 {
                        let existing_user = fetch_doc_for_update(&mut tx, "faceit_users", did).await?;
                        let prev_cents = existing_user.as_ref().map(crate::withdraw::balance_cents).unwrap_or(0);
                        let prev_balance = prev_cents as f64 / 100.0;
                        let new_balance = ((prev_balance + credits_to_add) * 100.0).round() / 100.0;
                        let new_cents = (new_balance * 100.0).round() as i64;
                        let delta_cents = new_cents - prev_cents;

                        let mut user_map = match existing_user {
                            Some(Value::Object(m)) => m,
                            _ => {
                                let mut m = Map::new();
                                m.insert("discordId".into(), json!(did));
                                m.insert("createdAt".into(), now_ts.clone());
                                m
                            }
                        };
                        user_map.insert("wallet_balance".into(), json!(new_balance));
                        user_map.insert("last_balance_update".into(), json!(now_ms));
                        user_map.insert("last_balance_reason".into(), json!(format!("Compra de tienda acreditada ({order_id})")));
                        let updated_user = Value::Object(user_map);

                        upsert_realtime_doc(&mut tx, "faceit_users", did, &updated_user.to_string()).await?;
                        sync_relational_upsert(&mut tx, "faceit_users", did, &updated_user).await?;

                        wallet::append_ledger(&mut tx, did, delta_cents, new_cents, "recharge", &order_id).await?;
                    }
                }

                tx.commit().await.map_err(sqlx_to_data_err)?;

                self.notify_write("orders", &order_id, RealtimeOp::Upsert);
                if let Some(did) = discord_id {
                    self.notify_write("faceit_users", did, RealtimeOp::Upsert);
                }

                Ok(order_id)
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());

                // Check already used
                if let Some(orders) = guard.collections.get("orders") {
                    for ord in orders.values() {
                        let charge = text(ord, "chargeId");
                        let reference = text(ord, "referenceCode");
                        if charge.as_deref() == Some(&operation) || reference.as_deref() == Some(&operation) {
                            return Err(CheckoutError::AlreadyUsed);
                        }
                    }
                }

                // Check payment
                let payment = guard.collections.get("yape_transacciones").and_then(|c| c.get(&operation)).cloned();
                let Some(payment) = payment else {
                    return Err(CheckoutError::PaymentNotFound);
                };
                if text(&payment, "tipo").unwrap_or_default().to_uppercase() != "ENTRADA" {
                    return Err(CheckoutError::PaymentNotEntry);
                }
                let Some(amount) = amount_of(payment.get("monto")) else {
                    return Err(CheckoutError::PaymentNotFound);
                };
                if (amount * 100.0).round() != (expected_amount * 100.0).round() {
                    return Err(CheckoutError::AmountMismatch(amount));
                }

                let now_ms = Utc::now().timestamp_millis();
                let now_ts = timestamp_now();
                let now_iso = Utc::now().to_rfc3339();
                let seq = WITHDRAW_SEQ.fetch_add(1, Ordering::Relaxed) % 900 + 100;
                let order_id = format!("ORD-{now_ms}-{seq}");

                let order_val = json!({
                    "orderId": order_id,
                    "customer": customer,
                    "items": items,
                    "totalAmount": expected_amount,
                    "currency": "PEN",
                    "paymentMethod": "yape_transferencia",
                    "chargeId": operation,
                    "referenceCode": operation,
                    "status": "COMPLETADA",
                    "createdAt": now_ts,
                    "createdAtIso": now_iso,
                });

                if guard.collections.get("yape_operations").is_some_and(|c| c.contains_key(&operation)) {
                    return Err(CheckoutError::AlreadyUsed);
                }
                guard.collections.entry("yape_operations".into()).or_default().insert(
                    operation.clone(),
                    json!({ "operationNumber": operation, "discordId": "", "amount": amount, "credited": true, "status": "credited", "usedBy": "order", "orderId": order_id }),
                );
                guard.collections.entry("orders".into()).or_default().insert(order_id.clone(), order_val);

                let discord_id = customer.get("discordId").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                if let Some(ref did) = discord_id {
                    let mut credits_to_add: f64 = 0.0;
                    if let Some(items_arr) = items.as_array() {
                        for item in items_arr {
                            let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                            let qty = item.get("quantity").and_then(Value::as_u64).unwrap_or(1) as f64;
                            if item_id == "prod_pack_10" {
                                credits_to_add += 10.0 * qty;
                            } else if item_id == "prod_pack_25" {
                                credits_to_add += 25.0 * qty;
                            }
                        }
                    }
                    if credits_to_add > 0.0 {
                        let existing = guard.collections.get("faceit_users").and_then(|c| c.get(did)).cloned();
                        let prev_cents = existing.as_ref().map(crate::withdraw::balance_cents).unwrap_or(0);
                        let prev_balance = prev_cents as f64 / 100.0;
                        let new_balance = ((prev_balance + credits_to_add) * 100.0).round() / 100.0;

                        let mut user_map = match existing {
                            Some(Value::Object(m)) => m,
                            _ => {
                                let mut m = Map::new();
                                m.insert("discordId".into(), json!(did));
                                m.insert("createdAt".into(), now_ts.clone());
                                m
                            }
                        };
                        user_map.insert("wallet_balance".into(), json!(new_balance));
                        user_map.insert("last_balance_update".into(), json!(now_ms));
                        user_map.insert("last_balance_reason".into(), json!(format!("Compra de tienda acreditada ({order_id})")));
                        guard.collections.entry("faceit_users".into()).or_default().insert(did.clone(), Value::Object(user_map));
                    }
                }

                drop(guard);

                self.notify_write("orders", &order_id, RealtimeOp::Upsert);
                if let Some(ref did) = discord_id {
                    self.notify_write("faceit_users", did, RealtimeOp::Upsert);
                }

                Ok(order_id)
            }
        }
    }

    /// Finaliza una partida registrando resultado, marcador y estadísticas en MySQL,
    /// archivando la partida de `faceit_active_matches` a `faceit_matches_history`
    /// y, solo con `pay_winners`, liquidando el pozo a los ganadores. La decisión de pagar es del
    /// llamador (`MATCH_PAYOUT_ENABLED`): esta capa nunca paga por su cuenta.
    pub async fn finish_match(
        &self,
        match_id: &str,
        winner: Option<&str>,
        team1_score: Option<u32>,
        team2_score: Option<u32>,
        stats: Option<&Value>,
        pay_winners: bool,
    ) -> Result<MatchFinishOutcome, DataError> {
        let match_id = match_id.trim();
        if match_id.is_empty() || match_id.len() > 128 {
            return Err(DataError::Firestore("id de partida no válido".into()));
        }

        // 1. Obtener documento previo si existe en activas o historial
        let active_doc = self.get_doc::<Value>("faceit_active_matches", match_id).await?;
        let match_doc = match active_doc {
            Some(d) => Some(d),
            None => self.get_doc::<Value>("faceit_matches_history", match_id).await?,
        };

        // 2. Determinar marcador y ganador
        let score_a = team1_score
            .or_else(|| {
                stats.and_then(|s| {
                    s.get("teams")?
                        .get(0)?
                        .get("score")?
                        .as_u64()
                        .map(|n| n as u32)
                })
            })
            .unwrap_or(0);

        let score_b = team2_score
            .or_else(|| {
                stats.and_then(|s| {
                    s.get("teams")?
                        .get(1)?
                        .get("score")?
                        .as_u64()
                        .map(|n| n as u32)
                })
            })
            .unwrap_or(0);

        let winner_str = winner
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| {
                if score_a > score_b {
                    "team1".to_string()
                } else if score_b > score_a {
                    "team2".to_string()
                } else {
                    "draw".to_string()
                }
            });

        let now_ts = timestamp_now();

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;

                // a) Eliminar de partidas activas en tiempo real
                let _ = sqlx::query(
                    "DELETE FROM realtime_documents WHERE collection = 'faceit_active_matches' AND doc_id = ?",
                )
                .bind(match_id)
                .execute(&mut *tx)
                .await;
                let _ = record_realtime_event(&mut tx, "faceit_active_matches", match_id, "DELETE").await;

                // b) Construir y guardar documento de historial
                let mut history_obj = match_doc
                    .as_ref()
                    .and_then(|d| d.as_object().cloned())
                    .unwrap_or_default();
                history_obj.insert("matchId".into(), json!(match_id));
                history_obj.insert("status".into(), json!("FINISHED"));
                history_obj.insert("lifecycle_state".into(), json!("finished"));
                history_obj.insert("winner".into(), json!(&winner_str));
                history_obj.insert("faction1_score".into(), json!(score_a));
                history_obj.insert("faction2_score".into(), json!(score_b));
                history_obj.insert("finished_at".into(), json!(now_ts));
                let history_val = Value::Object(history_obj);

                upsert_realtime_doc(
                    &mut tx,
                    "faceit_matches_history",
                    match_id,
                    &history_val.to_string(),
                )
                .await?;

                // c) Actualizar tabla relacional `matches`
                let updated = sqlx::query(
                    r#"
                    UPDATE matches
                    SET lifecycle_state = 'finished',
                        status = 'FINISHED',
                        faction1_score = ?,
                        faction2_score = ?,
                        winner = ?,
                        finished_at = CURRENT_TIMESTAMP(3),
                        updated_at = CURRENT_TIMESTAMP(3)
                    WHERE match_id = ?
                    "#,
                )
                .bind(score_a)
                .bind(score_b)
                .bind(&winner_str)
                .bind(match_id)
                .execute(&mut *tx)
                .await
                .map_err(sqlx_to_data_err)?;

                if updated.rows_affected() == 0 {
                    let guild_id = match_doc
                        .as_ref()
                        .and_then(|d| text(d, "guild_id").or_else(|| text(d, "guildId")));
                    if let Some(ref gid) = guild_id {
                        let _ = sqlx::query("INSERT IGNORE INTO guilds (guild_id) VALUES (?)")
                            .bind(gid)
                            .execute(&mut *tx)
                            .await;
                    }
                    let f1_name = match_doc
                        .as_ref()
                        .and_then(|d| text(d, "faction1_name"))
                        .unwrap_or_else(|| "Equipo 1".into());
                    let f2_name = match_doc
                        .as_ref()
                        .and_then(|d| text(d, "faction2_name"))
                        .unwrap_or_else(|| "Equipo 2".into());
                    let f1_roster = match_doc.as_ref().and_then(|d| d.get("faction1_roster"));
                    let f2_roster = match_doc.as_ref().and_then(|d| d.get("faction2_roster"));
                    let bet_amount_cents = match_doc
                        .as_ref()
                        .and_then(|d| {
                            d.get("bet_amount_cents")
                                .or_else(|| d.get("betAmountCents"))
                        })
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let total_pot_cents = match_doc
                        .as_ref()
                        .and_then(|d| {
                            d.get("total_pot_cents")
                                .or_else(|| d.get("totalPotCents"))
                        })
                        .and_then(Value::as_i64)
                        .unwrap_or(0);

                    let _ = sqlx::query(
                        r#"
                        INSERT INTO matches (
                            match_id, guild_id, launch_mode, lifecycle_state, status,
                            faction1_name, faction2_name, faction1_score, faction2_score,
                            winner, bet_amount_cents, total_pot_cents,
                            faction1_roster, faction2_roster, finished_at
                        ) VALUES (?, ?, 'steam', 'finished', 'FINISHED', ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP(3))
                        ON DUPLICATE KEY UPDATE
                            lifecycle_state = 'finished',
                            status = 'FINISHED',
                            faction1_score = VALUES(faction1_score),
                            faction2_score = VALUES(faction2_score),
                            winner = VALUES(winner),
                            finished_at = CURRENT_TIMESTAMP(3)
                        "#,
                    )
                    .bind(match_id)
                    .bind(guild_id)
                    .bind(f1_name)
                    .bind(f2_name)
                    .bind(score_a)
                    .bind(score_b)
                    .bind(&winner_str)
                    .bind(bet_amount_cents)
                    .bind(total_pot_cents)
                    .bind(f1_roster.map(Value::to_string))
                    .bind(f2_roster.map(Value::to_string))
                    .execute(&mut *tx)
                    .await;
                }

                // d) Guardar estadísticas detalladas si fueron recibidas
                if let Some(stats_val) = stats {
                    upsert_realtime_doc(&mut tx, "matches_stats", match_id, &stats_val.to_string()).await?;
                    upsert_realtime_doc(&mut tx, "faceit_match_stats", match_id, &stats_val.to_string()).await?;

                    let rounds = (score_a + score_b).max(1) as u8;
                    let _ = sqlx::query(
                        r#"
                        INSERT INTO match_stats (match_id, rounds_count, raw_stats_json)
                        VALUES (?, ?, CAST(? AS JSON))
                        ON DUPLICATE KEY UPDATE
                            rounds_count = VALUES(rounds_count),
                            raw_stats_json = VALUES(raw_stats_json),
                            updated_at = CURRENT_TIMESTAMP(3)
                        "#,
                    )
                    .bind(match_id)
                    .bind(rounds)
                    .bind(&stats_val.to_string())
                    .execute(&mut *tx)
                    .await;

                    let map_name = stats_val
                        .get("map")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| match_doc.as_ref().and_then(|d| text(d, "map_name")));

                    if let Some(teams) = stats_val.get("teams").and_then(Value::as_array) {
                        for (idx, team) in teams.iter().enumerate() {
                            let faction = if idx == 0 { "faction1" } else { "faction2" };
                            if let Some(players) = team.get("players").and_then(Value::as_array) {
                                for p in players {
                                    let nick = p
                                        .get("nickname")
                                        .or_else(|| p.get("name"))
                                        .and_then(Value::as_str)
                                        .unwrap_or("Jugador");
                                    let s64 = p
                                        .get("steamid64")
                                        .or_else(|| p.get("steam_id_64"))
                                        .or_else(|| p.get("steamId"))
                                        .and_then(Value::as_str);
                                    let mut did = p
                                        .get("discordId")
                                        .or_else(|| p.get("discord_id"))
                                        .and_then(Value::as_str)
                                        .map(str::to_string);

                                    if did.is_none() {
                                        if let (Some(s), Some(doc)) = (s64, match_doc.as_ref()) {
                                            for r_key in ["faction1_roster", "faction2_roster"] {
                                                if let Some(roster) = doc.get(r_key).and_then(Value::as_array) {
                                                    for rp in roster {
                                                        let rp_s64 = rp.get("steam_id_64").or_else(|| rp.get("steamId64")).and_then(Value::as_str);
                                                        if rp_s64 == Some(s) {
                                                            did = rp.get("discord_id").or_else(|| rp.get("discordId")).and_then(Value::as_str).map(str::to_string);
                                                            break;
                                                        }
                                                    }
                                                }
                                                if did.is_some() { break; }
                                            }
                                        }
                                    }

                                    if let Some(ref d) = did {
                                        let _ = sqlx::query("INSERT IGNORE INTO users (discord_id, steam_id_64, steam_nickname) VALUES (?, ?, ?)")
                                            .bind(d)
                                            .bind(s64)
                                            .bind(nick)
                                            .execute(&mut *tx)
                                            .await;
                                    }

                                    let kills = p.get("kills").and_then(Value::as_u64).unwrap_or(0) as u16;
                                    let deaths = p.get("deaths").and_then(Value::as_u64).unwrap_or(0) as u16;
                                    let assists = p.get("assists").and_then(Value::as_u64).unwrap_or(0) as u16;
                                    let kd = format!("{:.2}", p.get("kd").and_then(Value::as_f64).unwrap_or(0.0));
                                    let adr = format!("{:.2}", p.get("damage").or_else(|| p.get("adr")).and_then(Value::as_f64).unwrap_or(0.0));
                                    let mvps = p.get("mvps").and_then(Value::as_u64).unwrap_or(0) as u16;
                                    let hs = p.get("headshots").and_then(Value::as_u64).unwrap_or(0) as u16;

                                    let _ = sqlx::query(
                                        r#"
                                        INSERT INTO match_player_stats (
                                            match_id, map_number, map_name, discord_id, steam_id_64,
                                            nickname, faction, kills, deaths, assists, kd_ratio,
                                            adr, headshots, mvps
                                        ) VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                                        "#,
                                    )
                                    .bind(match_id)
                                    .bind(&map_name)
                                    .bind(did)
                                    .bind(s64)
                                    .bind(nick)
                                    .bind(faction)
                                    .bind(kills)
                                    .bind(deaths)
                                    .bind(assists)
                                    .bind(&kd)
                                    .bind(&adr)
                                    .bind(hs)
                                    .bind(mvps)
                                    .execute(&mut *tx)
                                    .await;
                                }
                            }
                        }
                    }
                }

                tx.commit().await.map_err(sqlx_to_data_err)?;

                // Notificar cambios por bus en tiempo real
                self.notify_write("faceit_active_matches", match_id, RealtimeOp::Delete);
                self.notify_write("faceit_matches_history", match_id, RealtimeOp::Upsert);
                self.notify_write("matches", match_id, RealtimeOp::Upsert);
                if stats.is_some() {
                    self.notify_write("matches_stats", match_id, RealtimeOp::Upsert);
                    self.notify_write("faceit_match_stats", match_id, RealtimeOp::Upsert);
                }

                // e) Liquidar pozo si corresponde
                let mut payout_shares = Vec::new();
                if let Some(doc) = match_doc.as_ref().filter(|_| pay_winners) {
                    if let Ok(plan) = crate::payout::plan_payout(doc, &winner_str) {
                        if plan.bet_cents > 0 && !plan.winners.is_empty() {
                            match self
                                .pay_match_winners(
                                    match_id,
                                    plan.bet_cents,
                                    plan.total_players,
                                    &plan.winners,
                                    "cs2_api",
                                )
                                .await
                            {
                                Ok(PayoutOutcome::Paid(shares)) => {
                                    for (did, cents) in shares {
                                        payout_shares.push(PayoutShare {
                                            discord_id: did,
                                            amount: cents as f64 / 100.0,
                                        });
                                    }
                                }
                                Ok(PayoutOutcome::AlreadyPaid) => {
                                    tracing::info!("[CS2 API] Partida {match_id} ya estaba pagada.");
                                }
                                Ok(PayoutOutcome::NothingToPay) => {}
                                Err(e) => {
                                    tracing::error!("[CS2 API] Error liquidando premios de {match_id}: {e}");
                                }
                            }
                        }
                    }
                }

                Ok(MatchFinishOutcome {
                    match_id: match_id.to_string(),
                    winner: winner_str,
                    score_a,
                    score_b,
                    payouts: payout_shares,
                    stats_saved: stats.is_some(),
                })
            }
            Storage::Memory(mem) => {
                let history_doc = {
                    let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                    let active_doc = guard
                        .collections
                        .get_mut("faceit_active_matches")
                        .and_then(|c| c.remove(match_id));
                    let mut history_doc = active_doc.unwrap_or_else(|| json!({ "matchId": match_id }));
                    if let Some(obj) = history_doc.as_object_mut() {
                        obj.insert("status".into(), json!("FINISHED"));
                        obj.insert("lifecycle_state".into(), json!("finished"));
                        obj.insert("winner".into(), json!(&winner_str));
                        obj.insert("faction1_score".into(), json!(score_a));
                        obj.insert("faction2_score".into(), json!(score_b));
                        obj.insert("finished_at".into(), json!(now_ts));
                    }
                    guard
                        .collections
                        .entry("faceit_matches_history".into())
                        .or_default()
                        .insert(match_id.to_string(), history_doc.clone());
                    if let Some(s) = stats {
                        guard
                            .collections
                            .entry("matches_stats".into())
                            .or_default()
                            .insert(match_id.to_string(), s.clone());
                        guard
                            .collections
                            .entry("faceit_match_stats".into())
                            .or_default()
                            .insert(match_id.to_string(), s.clone());
                    }
                    history_doc
                };

                let mut payout_shares = Vec::new();
                if let Some(plan) = pay_winners
                    .then(|| crate::payout::plan_payout(&history_doc, &winner_str).ok())
                    .flatten()
                {
                    if plan.bet_cents > 0 && !plan.winners.is_empty() {
                        if let Ok(PayoutOutcome::Paid(shares)) = self
                            .pay_match_winners(
                                match_id,
                                plan.bet_cents,
                                plan.total_players,
                                &plan.winners,
                                "cs2_api",
                            )
                            .await
                        {
                            for (did, cents) in shares {
                                payout_shares.push(PayoutShare {
                                    discord_id: did,
                                    amount: cents as f64 / 100.0,
                                });
                            }
                        }
                    }
                }

                Ok(MatchFinishOutcome {
                    match_id: match_id.to_string(),
                    winner: winner_str,
                    score_a,
                    score_b,
                    payouts: payout_shares,
                    stats_saved: stats.is_some(),
                })
            }
        }
    }

    /// Cancela una partida en MySQL marcándola como CANCELLED / cancelled,
    /// eliminándola de `faceit_active_matches` y moviéndola a `faceit_matches_history`.
    pub async fn cancel_match(
        &self,
        match_id: &str,
        reason: Option<&str>,
    ) -> Result<(), DataError> {
        let match_id = match_id.trim();
        if match_id.is_empty() || match_id.len() > 128 {
            return Err(DataError::Firestore("id de partida no válido".into()));
        }

        // Una partida cancelada devuelve las apuestas cobradas (idempotente: si ya se pagó el
        // premio o ya se devolvió, no hace nada).
        let refunded = self
            .refund_match_stakes(match_id, reason.unwrap_or("partida cancelada"))
            .await?;
        if refunded > 0 {
            tracing::info!("[Cancelación] {match_id}: devueltos S/ {} en apuestas", refunded / 100);
        }

        let now_ts = timestamp_now();

        match &self.storage {
            Storage::Pool { pool, .. } => {
                let mut tx = pool.begin().await.map_err(sqlx_to_data_err)?;

                let _ = sqlx::query(
                    "DELETE FROM realtime_documents WHERE collection = 'faceit_active_matches' AND doc_id = ?",
                )
                .bind(match_id)
                .execute(&mut *tx)
                .await;
                let _ = record_realtime_event(&mut tx, "faceit_active_matches", match_id, "DELETE").await;

                let cancel_doc = json!({
                    "matchId": match_id,
                    "status": "CANCELLED",
                    "lifecycle_state": "cancelled",
                    "cancelReason": reason.unwrap_or("Cancelada"),
                    "updatedAt": now_ts,
                });

                upsert_realtime_doc(
                    &mut tx,
                    "faceit_matches_history",
                    match_id,
                    &cancel_doc.to_string(),
                )
                .await?;

                let _ = sqlx::query(
                    r#"
                    UPDATE matches
                    SET lifecycle_state = 'cancelled',
                        status = 'CANCELLED',
                        updated_at = CURRENT_TIMESTAMP(3)
                    WHERE match_id = ?
                    "#,
                )
                .bind(match_id)
                .execute(&mut *tx)
                .await;

                tx.commit().await.map_err(sqlx_to_data_err)?;

                self.notify_write("faceit_active_matches", match_id, RealtimeOp::Delete);
                self.notify_write("faceit_matches_history", match_id, RealtimeOp::Upsert);
                self.notify_write("matches", match_id, RealtimeOp::Upsert);
            }
            Storage::Memory(mem) => {
                let mut guard = mem.lock().unwrap_or_else(|e| e.into_inner());
                let active_doc = guard
                    .collections
                    .get_mut("faceit_active_matches")
                    .and_then(|c| c.remove(match_id));
                let mut history_doc = active_doc.unwrap_or_else(|| json!({ "matchId": match_id }));
                if let Some(obj) = history_doc.as_object_mut() {
                    obj.insert("status".into(), json!("CANCELLED"));
                    obj.insert("lifecycle_state".into(), json!("cancelled"));
                    obj.insert("cancelReason".into(), json!(reason.unwrap_or("Cancelada")));
                    obj.insert("updatedAt".into(), json!(now_ts));
                }
                guard
                    .collections
                    .entry("faceit_matches_history".into())
                    .or_default()
                    .insert(match_id.to_string(), history_doc);
            }
        }
        Ok(())
    }
}

// ============================================================================
// CONVERSIÓN DE TIMESTAMPS Y FORMATO NODE / SERDE
// ============================================================================

/// Convierte recursivamente los centinelas `{"$timestamp": "<RFC3339>"}` a texto RFC 3339,
/// idéntico a lo que entrega el deserializador serde de `firestore-rs` en [`crate::DocSource`].
pub(crate) fn json_for_serde(val: Value) -> Value {
    match val {
        Value::Array(arr) => Value::Array(arr.into_iter().map(json_for_serde).collect()),
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(Value::String(ts)) = map.get("$timestamp") {
                    return Value::String(ts.clone());
                }
            }
            Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, json_for_serde(v)))
                    .collect(),
            )
        }
        other => other,
    }
}

/// Convierte recursivamente un documento almacenado en `realtime_documents` al formato que
/// devuelve `res.json(...)` en Node ([`crate::NodeDocSource`]): los centinelas
/// `{"$timestamp": "<RFC3339>"}` se transforman en `{"_seconds": s, "_nanoseconds": n}`, mientras
/// que los textos planos con forma de fecha se quedan como textos.
pub(crate) fn json_for_node(val: Value) -> Value {
    match val {
        Value::Array(arr) => Value::Array(arr.into_iter().map(json_for_node).collect()),
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(Value::String(ts)) = map.get("$timestamp") {
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                        return json!({
                            "_seconds": dt.timestamp(),
                            "_nanoseconds": dt.timestamp_subsec_nanos(),
                        });
                    }
                }
            }
            Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, json_for_node(v)))
                    .collect(),
            )
        }
        other => other,
    }
}

// ============================================================================
// MOTOR DE CONSULTAS (FILTROS, ORDEN Y LÍMITE)
// ============================================================================

fn compare_json_field(field_val: Option<&Value>, op: Op, target: &QValue) -> bool {
    let Some(v) = field_val else {
        return false;
    };
    let unwrapped = match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("$timestamp") => {
            m.get("$timestamp").unwrap_or(v)
        }
        other => other,
    };

    match (unwrapped, target) {
        (Value::String(a), QValue::String(b)) => cmp_ord(a.cmp(b), op),
        (Value::Bool(a), QValue::Bool(b)) => match op {
            Op::Eq => a == b,
            Op::Ne => a != b,
            _ => false,
        },
        (Value::Number(n), QValue::Integer(b)) => {
            if let Some(a) = n.as_i64() {
                cmp_ord(a.cmp(b), op)
            } else if let Some(a) = n.as_f64() {
                cmp_float(a, *b as f64, op)
            } else {
                false
            }
        }
        (Value::Number(n), QValue::Double(b)) => {
            n.as_f64().is_some_and(|a| cmp_float(a, *b, op))
        }
        _ => false,
    }
}

fn cmp_ord(ord: std::cmp::Ordering, op: Op) -> bool {
    match op {
        Op::Eq => ord.is_eq(),
        Op::Ne => ord.is_ne(),
        Op::Lt => ord.is_lt(),
        Op::Le => ord.is_le(),
        Op::Gt => ord.is_gt(),
        Op::Ge => ord.is_ge(),
    }
}

fn cmp_float(a: f64, b: f64, op: Op) -> bool {
    match a.partial_cmp(&b) {
        Some(ord) => cmp_ord(ord, op),
        None => false,
    }
}

fn sort_key_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    if let (Some(ms_a), Some(ms_b)) = (extract_sort_millis(a), extract_sort_millis(b)) {
        return ms_a.cmp(&ms_b);
    }
    if let (Some(na), Some(nb)) = (a.as_f64(), b.as_f64()) {
        return na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal);
    }
    if let (Some(sa), Some(sb)) = (a.as_str(), b.as_str()) {
        return sa.cmp(sb);
    }
    std::cmp::Ordering::Equal
}

fn extract_sort_millis(v: &Value) -> Option<i64> {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(ts)) = m.get("$timestamp") {
                return chrono::DateTime::parse_from_rfc3339(ts)
                    .ok()
                    .map(|t| t.timestamp_millis());
            }
            let secs = m.get("_seconds")?.as_i64()?;
            let nanos = m.get("_nanoseconds").and_then(Value::as_i64).unwrap_or(0);
            Some(secs * 1000 + nanos / 1_000_000)
        }
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.timestamp_millis()),
        _ => None,
    }
}

fn apply_query_in_memory(
    docs: Vec<(String, Value)>,
    q: &Query,
) -> Vec<(String, Value)> {
    let mut filtered: Vec<(String, Value)> = docs
        .into_iter()
        .filter(|(_, doc)| {
            q.filters
                .iter()
                .all(|(field, op, val)| compare_json_field(doc.get(field), *op, val))
        })
        .collect();

    if let Some((ref field, dir)) = q.order_by {
        // Como Firestore: un documento que no contiene el campo por el que se ordena no entra
        filtered.retain(|(_, doc)| doc.get(field).is_some_and(|v| !v.is_null()));
        filtered.sort_by(|(id_a, a), (id_b, b)| {
            let va = a.get(field).unwrap_or(&Value::Null);
            let vb = b.get(field).unwrap_or(&Value::Null);
            let cmp = sort_key_cmp(va, vb).then_with(|| id_a.cmp(id_b));
            match dir {
                Dir::Asc => cmp,
                Dir::Desc => cmp.reverse(),
            }
        });
    }

    if let Some(limit) = q.limit {
        filtered.truncate(limit as usize);
    }
    filtered
}

// ============================================================================
// INICIALIZACIÓN, CDC POLLER Y SINCRONIZACIÓN RELACIONAL EN MYSQL
// ============================================================================

fn sqlx_to_data_err(err: sqlx::Error) -> DataError {
    match err {
        sqlx::Error::RowNotFound => DataError::NotFound,
        other => DataError::Firestore(format!("MySQL error: {other}")),
    }
}

async fn bootstrap_realtime_tables(pool: &MySqlPool) -> Result<(), DataError> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS realtime_documents (
            collection VARCHAR(64) NOT NULL,
            doc_id VARCHAR(128) NOT NULL,
            doc_json JSON NOT NULL,
            created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
            updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
            PRIMARY KEY (collection, doc_id),
            KEY idx_rt_docs_updated (collection, updated_at)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
        "#,
    )
    .execute(pool)
    .await
    .map_err(sqlx_to_data_err)?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS realtime_events (
            seq BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
            collection VARCHAR(64) NOT NULL,
            doc_id VARCHAR(128) NOT NULL,
            op ENUM('UPSERT', 'DELETE') NOT NULL,
            changed_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
            PRIMARY KEY (seq),
            KEY idx_rt_events_col_seq (collection, seq),
            KEY idx_rt_events_changed_at (changed_at)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
        "#,
    )
    .execute(pool)
    .await
    .map_err(sqlx_to_data_err)?;

    ensure_withdrawal_review_status(pool).await?;
    wallet::bootstrap_wallet_tables(pool).await
}

/// Las bases creadas antes del estado `review` no lo tienen en el ENUM y rechazarían esos
/// retiros. Añadir un valor al final de un ENUM es una operación instantánea en MySQL 8. Si
/// la tabla aún no existe (base sin el esquema relacional) no hay nada que migrar.
async fn ensure_withdrawal_review_status(pool: &MySqlPool) -> Result<(), DataError> {
    let column_type: Option<String> = sqlx::query_scalar(
        "SELECT CAST(COLUMN_TYPE AS CHAR) FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'yape_withdrawals' AND COLUMN_NAME = 'status'",
    )
    .fetch_optional(pool)
    .await
    .map_err(sqlx_to_data_err)?;
    match column_type {
        Some(t) if !t.contains("'review'") => {
            sqlx::query(
                "ALTER TABLE yape_withdrawals MODIFY status \
                 ENUM('pending', 'processing', 'completed', 'rejected', 'cancelled', 'review') \
                 NOT NULL DEFAULT 'pending'",
            )
            .execute(pool)
            .await
            .map_err(sqlx_to_data_err)?;
            tracing::info!("yape_withdrawals.status: añadido el estado 'review'");
        }
        _ => {}
    }
    Ok(())
}

fn spawn_cdc_poller(
    pool: MySqlPool,
    tx_events: broadcast::Sender<RealtimeEvent>,
    wake_cdc: Arc<Notify>,
    mut last_seq: u64,
) {
    tokio::spawn(async move {
        loop {
            let _ = tokio::time::timeout(CDC_POLL_INTERVAL, wake_cdc.notified()).await;

            let rows = sqlx::query(
                "SELECT CAST(seq AS UNSIGNED) AS seq_u, collection, doc_id, op FROM realtime_events WHERE seq > ? ORDER BY seq ASC LIMIT 500",
            )
            .bind(last_seq)
            .fetch_all(&pool)
            .await;

            if let Ok(rows) = rows {
                for row in rows {
                    let seq: u64 = row.try_get("seq_u").unwrap_or(last_seq);
                    let collection: String = row.try_get("collection").unwrap_or_default();
                    let doc_id: String = row.try_get("doc_id").unwrap_or_default();
                    let op_str: String = row.try_get("op").unwrap_or_default();
                    let op = if op_str.eq_ignore_ascii_case("DELETE") {
                        RealtimeOp::Delete
                    } else {
                        RealtimeOp::Upsert
                    };
                    if seq > last_seq {
                        last_seq = seq;
                    }
                    let _ = tx_events.send(RealtimeEvent {
                        seq,
                        collection,
                        doc_id,
                        op,
                    });
                }
            }
        }
    });
}

async fn fetch_doc_for_update(
    tx: &mut Transaction<'_, MySql>,
    collection: &str,
    id: &str,
) -> Result<Option<Value>, DataError> {
    let row = sqlx::query(
        "SELECT CAST(doc_json AS CHAR) AS doc_str FROM realtime_documents WHERE collection = ? AND doc_id = ? FOR UPDATE",
    )
    .bind(collection)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(sqlx_to_data_err)?;

    match row {
        Some(r) => {
            let s: String = r.try_get("doc_str").map_err(sqlx_to_data_err)?;
            let v: Value = serde_json::from_str(&s)
                .map_err(|e| DataError::Decode(format!("{collection}/{id}: {e}")))?;
            Ok(Some(v))
        }
        None => Ok(None),
    }
}

async fn upsert_realtime_doc(
    tx: &mut Transaction<'_, MySql>,
    collection: &str,
    id: &str,
    json_str: &str,
) -> Result<(), DataError> {
    sqlx::query(
        r#"
        INSERT INTO realtime_documents (collection, doc_id, doc_json)
        VALUES (?, ?, CAST(? AS JSON))
        ON DUPLICATE KEY UPDATE
            doc_json = VALUES(doc_json),
            updated_at = CURRENT_TIMESTAMP(3)
        "#,
    )
    .bind(collection)
    .bind(id)
    .bind(json_str)
    .execute(&mut **tx)
    .await
    .map_err(sqlx_to_data_err)?;

    record_realtime_event(tx, collection, id, "UPSERT").await
}

/// Crea `collection/id` solo si no existe. `false` si ya existía (clave duplicada, error 1062):
/// sirve de candado de idempotencia que no depende del nivel de aislamiento.
async fn insert_realtime_doc_once(
    tx: &mut Transaction<'_, MySql>,
    collection: &str,
    id: &str,
    json_str: &str,
) -> Result<bool, DataError> {
    let res = sqlx::query(
        "INSERT INTO realtime_documents (collection, doc_id, doc_json) VALUES (?, ?, CAST(? AS JSON))",
    )
    .bind(collection)
    .bind(id)
    .bind(json_str)
    .execute(&mut **tx)
    .await;
    match res {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => return Ok(false),
        Err(e) => return Err(sqlx_to_data_err(e)),
    }
    record_realtime_event(tx, collection, id, "UPSERT").await?;
    Ok(true)
}

async fn record_realtime_event(
    tx: &mut Transaction<'_, MySql>,
    collection: &str,
    id: &str,
    op: &str,
) -> Result<(), DataError> {
    sqlx::query("INSERT INTO realtime_events (collection, doc_id, op) VALUES (?, ?, ?)")
        .bind(collection)
        .bind(id)
        .bind(op)
        .execute(&mut **tx)
        .await
        .map_err(sqlx_to_data_err)?;
    Ok(())
}

/// Sincroniza en modo best-effort el documento escrito hacia las tablas relacionales normalizadas
/// (`users`, `guilds`, `yape_operations`, `yape_withdrawals`, `match_payouts`, `yape_audit_logs`,
/// `orders`, `claims`, `support_calls`, `bot_logs`) para que las vistas SQL y reportes BI siempre
/// reflejen los datos en vivo.
async fn sync_relational_upsert(
    tx: &mut Transaction<'_, MySql>,
    collection: &str,
    id: &str,
    val: &Value,
) -> Result<(), DataError> {
    match collection {
        "faceit_users" | "users" => {
            // Normalizar siempre el identificador al discord_id global (sin prefijo de servidor)
            let discord_id = text(val, "discordId")
                .or_else(|| text(val, "discord_id"))
                .unwrap_or_else(|| {
                    id.rsplit_once('_')
                        .map(|(_, d)| d.to_string())
                        .unwrap_or_else(|| id.to_string())
                });
            let faceit_nick = text(val, "faceit_nickname");
            let faceit_player_id = text(val, "faceit_player_id");
            let elo = val
                .get("faceit_elo")
                .or_else(|| val.get("elo"))
                .and_then(Value::as_i64)
                .map(|n| n as i32)
                .unwrap_or(1000);
            let level = val
                .get("skill_level")
                .or_else(|| val.get("faceit_level"))
                .and_then(Value::as_u64)
                .map(|n| n as u8)
                .unwrap_or(1);
            let phone = text(val, "phoneNumber").or_else(|| text(val, "phone_number"));
            let steam_id = text(val, "steam_id_64").or_else(|| text(val, "steamId"));
            let steam_nick = text(val, "steam_nickname");
            let rol = text(val, "rol").unwrap_or_else(|| "user".into());
            let status = text(val, "status").unwrap_or_else(|| "Verificado".into());
            let tier_override = text(val, "mixton_tier_override");
            let club_role = text(val, "club_role").or_else(|| text(val, "clubRole"));
            let wallet_cents = balance_cents(val);
            let last_wd = val.get("last_withdrawal_at").and_then(Value::as_i64);
            let last_upd = val.get("last_balance_update").and_then(Value::as_i64);
            let last_reason = text(val, "last_balance_reason");

            let _ = sqlx::query(
                r#"
                INSERT INTO users (
                    discord_id, faceit_player_id, faceit_nickname,
                    faceit_elo, faceit_level, steam_id_64, steam_nickname, phone_number,
                    rol, status, club_role, mixton_tier_override,
                    wallet_balance_cents, last_withdrawal_at, last_balance_update,
                    last_balance_reason
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE
                    faceit_player_id = COALESCE(VALUES(faceit_player_id), faceit_player_id),
                    faceit_nickname = COALESCE(VALUES(faceit_nickname), faceit_nickname),
                    faceit_elo = COALESCE(VALUES(faceit_elo), faceit_elo),
                    faceit_level = COALESCE(VALUES(faceit_level), faceit_level),
                    steam_id_64 = COALESCE(VALUES(steam_id_64), steam_id_64),
                    steam_nickname = COALESCE(VALUES(steam_nickname), steam_nickname),
                    phone_number = COALESCE(VALUES(phone_number), phone_number),
                    rol = COALESCE(VALUES(rol), rol),
                    status = COALESCE(VALUES(status), status),
                    club_role = COALESCE(VALUES(club_role), club_role),
                    mixton_tier_override = COALESCE(VALUES(mixton_tier_override), mixton_tier_override),
                    wallet_balance_cents = IF(?, VALUES(wallet_balance_cents), wallet_balance_cents),
                    last_withdrawal_at = COALESCE(VALUES(last_withdrawal_at), last_withdrawal_at),
                    last_balance_update = COALESCE(VALUES(last_balance_update), last_balance_update),
                    last_balance_reason = COALESCE(VALUES(last_balance_reason), last_balance_reason)
                "#,
            )
            .bind(&discord_id)
            .bind(faceit_player_id)
            .bind(faceit_nick)
            .bind(elo)
            .bind(level)
            .bind(steam_id)
            .bind(steam_nick)
            .bind(phone)
            .bind(rol)
            .bind(status)
            .bind(club_role)
            .bind(tier_override)
            .bind(wallet_cents)
            .bind(last_wd)
            .bind(last_upd)
            .bind(last_reason)
            // Varios documentos (`{discordId}` y `{guild}_{discordId}`) caen en la misma fila:
            // solo el documento global, que es el que mueve la billetera, fija el saldo.
            .bind(id == discord_id)
            .execute(&mut **tx)
            .await;
        }
        "yape_operations" => {
            let discord_id = text(val, "discordId").unwrap_or_default();
            let guild_id = text(val, "guildId");
            let amount_cents = amount_of_zero(val.get("amount"))
                .map(|a| (a * 100.0).round() as i64)
                .unwrap_or(0);
            let credited = val.get("credited").and_then(Value::as_bool).unwrap_or(false);
            let status = text(val, "status").unwrap_or_else(|| "credited".into());
            let _ = sqlx::query(
                r#"
                INSERT INTO yape_operations (operation_number, discord_id, guild_id, amount_cents, credited, status)
                VALUES (?, ?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE
                    credited = VALUES(credited),
                    status = VALUES(status)
                "#,
            )
            .bind(id)
            .bind(discord_id)
            .bind(guild_id)
            .bind(amount_cents)
            .bind(credited)
            .bind(status)
            .execute(&mut **tx)
            .await;
        }
        "yape_withdrawals" => {
            let discord_id = text(val, "discordId").unwrap_or_default();
            let status = text(val, "status").unwrap_or_else(|| "pending".into());
            let amount_cents = val
                .get("amountCents")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let phone = text(val, "phoneNumber").unwrap_or_default();
            let before_cents = amount_of_zero(val.get("balanceBefore"))
                .map(|a| (a * 100.0).round() as i64)
                .unwrap_or(0);
            let after_cents = amount_of_zero(val.get("balanceAfter"))
                .map(|a| (a * 100.0).round() as i64)
                .unwrap_or(0);
            let source = text(val, "source").unwrap_or_else(|| "web".into());
            let admin_note = text(val, "reviewReason").map(|r| format!("Revisión: {r}"));
            // Esta fila es la que lee el worker de Yape para pagar, así que no es best-effort:
            // si falla, falla el retiro entero (no queda saldo descontado sin solicitud).
            // Tampoco se pisa el estado de una fila existente: el worker la pudo haber llevado
            // a `processing`/`completed`, y volverla a `pending` la pagaría dos veces.
            sqlx::query(
                r#"
                INSERT INTO yape_withdrawals (
                    id, discord_id, status, amount_cents, phone_number,
                    balance_before_cents, balance_after_cents, source, admin_note
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE id = id
                "#,
            )
            .bind(id)
            .bind(discord_id)
            .bind(status)
            .bind(amount_cents)
            .bind(phone)
            .bind(before_cents)
            .bind(after_cents)
            .bind(source)
            .bind(admin_note)
            .execute(&mut **tx)
            .await
            .map_err(sqlx_to_data_err)?;
        }
        "yape_audit_logs" => {
            let event = text(val, "event").unwrap_or_else(|| "info".into());
            let source = text(val, "source").unwrap_or_else(|| "system".into());
            let discord_id = text(val, "discordId");
            let guild_id = text(val, "guildId");
            let op_num = text(val, "operationNumber");
            let amount_cents = amount_of_zero(val.get("amount"))
                .map(|a| (a * 100.0).round() as i64);
            let _ = sqlx::query(
                r#"
                INSERT INTO yape_audit_logs (
                    id, event, source, discord_id, guild_id, operation_number, amount_cents
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE event = VALUES(event)
                "#,
            )
            .bind(id)
            .bind(event)
            .bind(source)
            .bind(discord_id)
            .bind(guild_id)
            .bind(op_num)
            .bind(amount_cents)
            .execute(&mut **tx)
            .await;
        }
        "faceit_config" if id == "settings" => {
            sync_relational_guild_settings(tx, "GLOBAL", val).await?;
        }
        "faceit_config" if id == "messages" => {
            let welcome_title = text(val, "welcomeTitle")
                .or_else(|| text(val, "welcome_title"))
                .unwrap_or_else(|| "¡Bienvenido al Servidor! 🎉".into());
            let welcome_desc = text(val, "welcomeDescription")
                .or_else(|| text(val, "welcome_description"))
                .unwrap_or_default();
            let welcome_color = text(val, "welcomeColor")
                .or_else(|| text(val, "welcome_color"))
                .unwrap_or_else(|| "#FF5500".into());
            let support_title = text(val, "supportTitle")
                .or_else(|| text(val, "support_title"))
                .unwrap_or_else(|| "🚨 Soporte y Llamado de Admin".into());
            let support_desc = text(val, "supportDescription")
                .or_else(|| text(val, "support_description"))
                .unwrap_or_default();
            let support_color = text(val, "supportColor")
                .or_else(|| text(val, "support_color"))
                .unwrap_or_else(|| "#FF0000".into());

            let _ = sqlx::query(
                r#"
                INSERT INTO message_templates (
                    guild_id, welcome_title, welcome_description, welcome_color,
                    support_title, support_description, support_color
                ) VALUES ('GLOBAL', ?, ?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE
                    welcome_title = VALUES(welcome_title),
                    welcome_description = VALUES(welcome_description),
                    welcome_color = VALUES(welcome_color),
                    support_title = VALUES(support_title),
                    support_description = VALUES(support_description),
                    support_color = VALUES(support_color),
                    updated_at = CURRENT_TIMESTAMP(3)
                "#,
            )
            .bind(welcome_title)
            .bind(welcome_desc)
            .bind(welcome_color)
            .bind(support_title)
            .bind(support_desc)
            .bind(support_color)
            .execute(&mut **tx)
            .await;
        }
        "guild_settings" => {
            sync_relational_guild_settings(tx, id, val).await?;
        }
        "faceit_bot_logs" => {
            let level = text(val, "level").unwrap_or_else(|| "info".into());
            let source = text(val, "source").unwrap_or_else(|| "system".into());
            let message = text(val, "message").unwrap_or_default();
            let guild_id = text(val, "guild_id");

            let _ = sqlx::query(
                r#"
                INSERT INTO bot_logs (log_id, guild_id, level, source, message)
                VALUES (?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE
                    level = VALUES(level),
                    source = VALUES(source),
                    message = VALUES(message)
                "#,
            )
            .bind(id)
            .bind(guild_id)
            .bind(level)
            .bind(source)
            .bind(message)
            .execute(&mut **tx)
            .await;
        }
        "faceit_active_matches" | "matches" | "faceit_matches_history" => {
            let state = if collection == "faceit_active_matches" {
                "active".to_string()
            } else if collection == "faceit_matches_history" {
                text(val, "lifecycle_state")
                    .unwrap_or_else(|| "finished".into())
            } else {
                text(val, "lifecycle_state")
                    .unwrap_or_else(|| "active".into())
            };
            let status = text(val, "status").unwrap_or_else(|| {
                if state == "finished" {
                    "FINISHED".into()
                } else {
                    "ONGOING".into()
                }
            });
            let guild_id = text(val, "guild_id").or_else(|| text(val, "guildId"));
            if let Some(ref gid) = guild_id {
                let _ = sqlx::query("INSERT IGNORE INTO guilds (guild_id) VALUES (?)")
                    .bind(gid)
                    .execute(&mut **tx)
                    .await;
            }
            let queue_id = text(val, "queue_id").or_else(|| text(val, "queueId"));
            let map_name = text(val, "map_name")
                .or_else(|| text(val, "map"))
                .or_else(|| val.get("map").and_then(|m| text(m, "name")));
            let server_ip = text(val, "server_ip").or_else(|| text(val, "serverIp"));
            let assigned_port = val
                .get("assigned_port")
                .or_else(|| val.get("port"))
                .and_then(Value::as_u64)
                .map(|p| p as u16);
            let bet_amount_cents = val
                .get("bet_amount_cents")
                .or_else(|| val.get("betAmountCents"))
                .or_else(|| val.get("betAmount"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let total_pot_cents = val
                .get("total_pot_cents")
                .or_else(|| val.get("totalPotCents"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let f1_name = text(val, "faction1_name")
                .or_else(|| val.get("team1").and_then(|t| text(t, "name")))
                .unwrap_or_else(|| "Equipo 1".into());
            let f2_name = text(val, "faction2_name")
                .or_else(|| val.get("team2").and_then(|t| text(t, "name")))
                .unwrap_or_else(|| "Equipo 2".into());
            let f1_score = val
                .get("faction1_score")
                .or_else(|| val.get("team1_score"))
                .and_then(Value::as_u64)
                .map(|s| s as u32)
                .unwrap_or(0);
            let f2_score = val
                .get("faction2_score")
                .or_else(|| val.get("team2_score"))
                .and_then(Value::as_u64)
                .map(|s| s as u32)
                .unwrap_or(0);
            let winner = text(val, "winner");
            let f1_roster = val
                .get("faction1_roster")
                .or_else(|| val.get("team1").and_then(|t| t.get("players")));
            let f2_roster = val
                .get("faction2_roster")
                .or_else(|| val.get("team2").and_then(|t| t.get("players")));

            let _ = sqlx::query(
                r#"
                INSERT INTO matches (
                    match_id, guild_id, queue_id, lifecycle_state, status,
                    map_name, server_ip, assigned_port, bet_amount_cents, total_pot_cents,
                    faction1_name, faction2_name, faction1_score, faction2_score,
                    winner, faction1_roster, faction2_roster
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE
                    guild_id = COALESCE(VALUES(guild_id), guild_id),
                    queue_id = COALESCE(VALUES(queue_id), queue_id),
                    lifecycle_state = VALUES(lifecycle_state),
                    status = VALUES(status),
                    map_name = COALESCE(VALUES(map_name), map_name),
                    server_ip = COALESCE(VALUES(server_ip), server_ip),
                    assigned_port = COALESCE(VALUES(assigned_port), assigned_port),
                    bet_amount_cents = COALESCE(VALUES(bet_amount_cents), bet_amount_cents),
                    total_pot_cents = COALESCE(VALUES(total_pot_cents), total_pot_cents),
                    faction1_name = VALUES(faction1_name),
                    faction2_name = VALUES(faction2_name),
                    faction1_score = VALUES(faction1_score),
                    faction2_score = VALUES(faction2_score),
                    winner = COALESCE(VALUES(winner), winner),
                    faction1_roster = COALESCE(VALUES(faction1_roster), faction1_roster),
                    faction2_roster = COALESCE(VALUES(faction2_roster), faction2_roster),
                    updated_at = CURRENT_TIMESTAMP(3)
                "#,
            )
            .bind(id)
            .bind(guild_id)
            .bind(queue_id)
            .bind(state)
            .bind(status)
            .bind(map_name)
            .bind(server_ip)
            .bind(assigned_port)
            .bind(bet_amount_cents)
            .bind(total_pot_cents)
            .bind(f1_name)
            .bind(f2_name)
            .bind(f1_score)
            .bind(f2_score)
            .bind(winner)
            .bind(f1_roster.map(Value::to_string))
            .bind(f2_roster.map(Value::to_string))
            .execute(&mut **tx)
            .await;
        }
        "match_stats" | "faceit_match_stats" | "matches_stats" => {
            let _ = sqlx::query(
                r#"
                INSERT INTO matches (match_id, status, lifecycle_state)
                VALUES (?, 'FINISHED', 'finished')
                ON DUPLICATE KEY UPDATE status = VALUES(status)
                "#,
            )
            .bind(id)
            .execute(&mut **tx)
            .await;

            let rounds = val
                .get("rounds_count")
                .or_else(|| val.get("rounds"))
                .and_then(Value::as_u64)
                .map(|r| r as u8)
                .unwrap_or(1);

            let _ = sqlx::query(
                r#"
                INSERT INTO match_stats (match_id, rounds_count, raw_stats_json)
                VALUES (?, ?, CAST(? AS JSON))
                ON DUPLICATE KEY UPDATE
                    rounds_count = VALUES(rounds_count),
                    raw_stats_json = VALUES(raw_stats_json),
                    updated_at = CURRENT_TIMESTAMP(3)
                "#,
            )
            .bind(id)
            .bind(rounds)
            .bind(&val.to_string())
            .execute(&mut **tx)
            .await;
        }
        "match_payouts" => {
            let _ = sqlx::query(
                r#"
                INSERT INTO matches (match_id, status, lifecycle_state)
                VALUES (?, 'FINISHED', 'finished')
                ON DUPLICATE KEY UPDATE status = VALUES(status)
                "#,
            )
            .bind(id)
            .execute(&mut **tx)
            .await;

            let status = text(val, "status").unwrap_or_else(|| "paid".into());
            let bet_cents = val
                .get("betCents")
                .or_else(|| val.get("bet_cents"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let total_pot_cents = val
                .get("totalPotCents")
                .or_else(|| val.get("total_pot_cents"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let source = text(val, "source").unwrap_or_else(|| "matchzy".into());

            let _ = sqlx::query(
                r#"
                INSERT INTO match_payouts (match_id, status, bet_cents, total_pot_cents, source)
                VALUES (?, ?, ?, ?, ?)
                ON DUPLICATE KEY UPDATE
                    status = VALUES(status),
                    bet_cents = VALUES(bet_cents),
                    total_pot_cents = VALUES(total_pot_cents)
                "#,
            )
            .bind(id)
            .bind(status)
            .bind(bet_cents)
            .bind(total_pot_cents)
            .bind(source)
            .execute(&mut **tx)
            .await;

            if let Some(winners_arr) = val.get("winners").and_then(Value::as_array) {
                for w in winners_arr {
                    let did = text(w, "discordId").or_else(|| text(w, "discord_id"));
                    if let Some(discord_id) = did {
                        let amount_cents = w
                            .get("amountCents")
                            .or_else(|| w.get("amount_cents"))
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let _ = sqlx::query(
                            "INSERT IGNORE INTO users (discord_id) VALUES (?)",
                        )
                        .bind(&discord_id)
                        .execute(&mut **tx)
                        .await;

                        let _ = sqlx::query(
                            r#"
                            INSERT INTO match_payout_winners (match_id, discord_id, amount_cents)
                            VALUES (?, ?, ?)
                            "#,
                        )
                        .bind(id)
                        .bind(&discord_id)
                        .bind(amount_cents)
                        .execute(&mut **tx)
                        .await;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn sync_relational_guild_settings(
    tx: &mut Transaction<'_, MySql>,
    guild_id: &str,
    val: &Value,
) -> Result<(), DataError> {
    let name = text(val, "name")
        .or_else(|| text(val, "guildName"))
        .unwrap_or_else(|| if guild_id == "GLOBAL" { "Configuración Global MachuPeek".into() } else { format!("Servidor {guild_id}") });
    let icon_url = text(val, "iconUrl").or_else(|| text(val, "icon"));
    let member_count = val.get("memberCount").and_then(Value::as_u64).unwrap_or(0) as u32;

    let link_ch = text(val, "discordLinkChannelId");
    let matches_ch = text(val, "discordMatchesChannelId");
    let mixton_ch = text(val, "discordMixtonChannelId");
    let mixton_chat_ch = text(val, "discordMixtonChatChannelId");
    let roles_ch = text(val, "discordRolesChannelId");
    let status_ch = text(val, "discordStatusChannelId");
    let wait_ch = text(val, "discordWaitingRoomChannelId");
    let alerts_ch = text(val, "discordAdminAlertsChannelId");
    let sanctions_ch = text(val, "discordSanctionsChannelId");
    let support_ch = text(val, "discordSupportChannelId");
    let music_ch = text(val, "discordMusicChannelId");
    let recharge_ch = text(val, "discordRechargeChannelId");

    let match_role = text(val, "discordMatchAccessRoleId");
    let hub_role = text(val, "hubRoleId");
    let discord_hub_role = text(val, "discordHubRoleId");
    let admin_role = text(val, "adminRoleId");
    let admin_name = text(val, "adminRoleName");

    let faceit_club = text(val, "faceitClubId");
    let faceit_hub = text(val, "faceitHubId");
    let faceit_lb = text(val, "faceitLeaderboardId");

    let timeout = val.get("matchTimeoutMinutes").and_then(Value::as_u64).unwrap_or(120) as u32;
    let games = val.get("gamesPerMatch")
        .or_else(|| val.get("roundsPerGame"))
        .and_then(Value::as_u64)
        .map(|n| if n == 1 || n == 3 || n == 5 { n as u8 } else { 1 })
        .unwrap_or(1);

    let _ = sqlx::query(
        r#"
        INSERT INTO guilds (
            guild_id, name, icon_url, member_count,
            discord_link_channel_id, discord_matches_channel_id,
            discord_mixton_channel_id, discord_mixton_chat_channel_id,
            discord_roles_channel_id, discord_status_channel_id,
            discord_waiting_room_channel_id, discord_admin_alerts_channel_id,
            discord_sanctions_channel_id, discord_support_channel_id,
            discord_music_channel_id, discord_recharge_channel_id,
            discord_match_access_role_id, hub_role_id, discord_hub_role_id,
            admin_role_id, admin_role_name,
            faceit_club_id, faceit_hub_id, faceit_leaderboard_id,
            match_timeout_minutes, games_per_match, rounds_per_game
        ) VALUES (
            ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, ?, ?,
            ?, ?, ?
        )
        ON DUPLICATE KEY UPDATE
            name = COALESCE(VALUES(name), name),
            icon_url = COALESCE(VALUES(icon_url), icon_url),
            member_count = IF(VALUES(member_count) > 0, VALUES(member_count), member_count),
            discord_link_channel_id = VALUES(discord_link_channel_id),
            discord_matches_channel_id = VALUES(discord_matches_channel_id),
            discord_mixton_channel_id = VALUES(discord_mixton_channel_id),
            discord_mixton_chat_channel_id = VALUES(discord_mixton_chat_channel_id),
            discord_roles_channel_id = VALUES(discord_roles_channel_id),
            discord_status_channel_id = VALUES(discord_status_channel_id),
            discord_waiting_room_channel_id = VALUES(discord_waiting_room_channel_id),
            discord_admin_alerts_channel_id = VALUES(discord_admin_alerts_channel_id),
            discord_sanctions_channel_id = VALUES(discord_sanctions_channel_id),
            discord_support_channel_id = VALUES(discord_support_channel_id),
            discord_music_channel_id = VALUES(discord_music_channel_id),
            discord_recharge_channel_id = VALUES(discord_recharge_channel_id),
            discord_match_access_role_id = VALUES(discord_match_access_role_id),
            hub_role_id = VALUES(hub_role_id),
            discord_hub_role_id = VALUES(discord_hub_role_id),
            admin_role_id = VALUES(admin_role_id),
            admin_role_name = VALUES(admin_role_name),
            faceit_club_id = VALUES(faceit_club_id),
            faceit_hub_id = VALUES(faceit_hub_id),
            faceit_leaderboard_id = VALUES(faceit_leaderboard_id),
            match_timeout_minutes = VALUES(match_timeout_minutes),
            games_per_match = VALUES(games_per_match),
            rounds_per_game = VALUES(rounds_per_game),
            updated_at = CURRENT_TIMESTAMP(3)
        "#,
    )
    .bind(guild_id)
    .bind(&name)
    .bind(&icon_url)
    .bind(member_count)
    .bind(&link_ch)
    .bind(&matches_ch)
    .bind(&mixton_ch)
    .bind(&mixton_chat_ch)
    .bind(&roles_ch)
    .bind(&status_ch)
    .bind(&wait_ch)
    .bind(&alerts_ch)
    .bind(&sanctions_ch)
    .bind(&support_ch)
    .bind(&music_ch)
    .bind(&recharge_ch)
    .bind(&match_role)
    .bind(&hub_role)
    .bind(&discord_hub_role)
    .bind(&admin_role)
    .bind(&admin_name)
    .bind(&faceit_club)
    .bind(&faceit_hub)
    .bind(&faceit_lb)
    .bind(timeout)
    .bind(games)
    .bind(games)
    .execute(&mut **tx)
    .await;

    // Sincronizar feature flags
    let auto_welcome = val.get("autoWelcomeDm").and_then(Value::as_bool).unwrap_or(true);
    let auto_role = val.get("autoRoleSync").and_then(Value::as_bool).unwrap_or(true);
    let auto_nick = val.get("autoNickSync").and_then(Value::as_bool).unwrap_or(true);
    let auto_vc = val.get("autoVoiceChannels").and_then(Value::as_bool).unwrap_or(true);
    let auto_move = val.get("autoMovePlayers").and_then(Value::as_bool).unwrap_or(true);
    let match_ann = val.get("matchAnnouncements").and_then(Value::as_bool).unwrap_or(true);
    let status_ann = val.get("serverStatusAnnouncements").and_then(Value::as_bool).unwrap_or(true);
    let live_match = val.get("liveMatchPanel").and_then(Value::as_bool).unwrap_or(true);
    let webhook_val = val.get("webhookValidation").and_then(Value::as_bool).unwrap_or(false);
    let admin_call = val.get("adminCallNotifications").and_then(Value::as_bool).unwrap_or(true);
    let hub_bans = val.get("hubBansNotifications").and_then(Value::as_bool).unwrap_or(true);
    let music_bot = val.get("musicBot").and_then(Value::as_bool).unwrap_or(true);
    let recharge_sys = val.get("rechargeWalletSystem").and_then(Value::as_bool).unwrap_or(true);
    let mixton_ready = val.get("mixtonReadyPanel").and_then(Value::as_bool).unwrap_or(true);

    let cmd_verificar = val.get("command_verificar").and_then(Value::as_bool).unwrap_or(true);
    let cmd_desvincular = val.get("command_desvincular").and_then(Value::as_bool).unwrap_or(true);
    let cmd_leaderboard = val.get("command_leaderboard").and_then(Value::as_bool).unwrap_or(true);
    let cmd_stats = val.get("command_stats").and_then(Value::as_bool).unwrap_or(true);
    let cmd_info = val.get("command_info").and_then(Value::as_bool).unwrap_or(true);
    let cmd_sincronizar = val.get("command_sincronizar").and_then(Value::as_bool).unwrap_or(true);
    let cmd_partidas = val.get("command_partidas").and_then(Value::as_bool).unwrap_or(true);
    let cmd_crear_sala = val.get("command_crear_sala").and_then(Value::as_bool).unwrap_or(true);
    let cmd_mixton_listo = val.get("command_mixton_listo").and_then(Value::as_bool).unwrap_or(true);
    let cmd_mixton_no_listo = val.get("command_mixton_no_listo").and_then(Value::as_bool).unwrap_or(true);
    let cmd_cs2_play = val.get("command_cs2_play").and_then(Value::as_bool).unwrap_or(true);
    let cmd_sancionar = val.get("command_sancionar").and_then(Value::as_bool).unwrap_or(true);
    let cmd_desancionar = val.get("command_desancionar").and_then(Value::as_bool).unwrap_or(true);
    let cmd_say = val.get("command_say").and_then(Value::as_bool).unwrap_or(true);
    let cmd_recargar = val.get("command_recargar").and_then(Value::as_bool).unwrap_or(true);
    let cmd_setup_recargas = val.get("command_setup_recargas").and_then(Value::as_bool).unwrap_or(true);
    let cmd_setup_soporte = val.get("command_setup_soporte").and_then(Value::as_bool).unwrap_or(true);
    let cmd_llamar_admin = val.get("command_llamar_admin").and_then(Value::as_bool).unwrap_or(true);

    let _ = sqlx::query(
        r#"
        INSERT INTO guild_feature_flags (
            guild_id,
            auto_welcome_dm, auto_role_sync, auto_nick_sync, auto_voice_channels, auto_move_players,
            match_announcements, server_status_announcements, live_match_panel, webhook_validation,
            admin_call_notifications, hub_bans_notifications, music_bot, recharge_wallet_system,
            mixton_ready_panel, command_verificar, command_desvincular, command_leaderboard,
            command_stats, command_info, command_sincronizar, command_partidas, command_crear_sala,
            command_mixton_listo, command_mixton_no_listo, command_cs2_play, command_sancionar,
            command_desancionar, command_say, command_recargar, command_setup_recargas,
            command_setup_soporte, command_llamar_admin
        ) VALUES (
            ?,
            ?, ?, ?, ?, ?,
            ?, ?, ?, ?,
            ?, ?, ?, ?,
            ?, ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, ?, ?, ?,
            ?, ?, ?, ?,
            ?, ?
        )
        ON DUPLICATE KEY UPDATE
            auto_welcome_dm = VALUES(auto_welcome_dm),
            auto_role_sync = VALUES(auto_role_sync),
            auto_nick_sync = VALUES(auto_nick_sync),
            auto_voice_channels = VALUES(auto_voice_channels),
            auto_move_players = VALUES(auto_move_players),
            match_announcements = VALUES(match_announcements),
            server_status_announcements = VALUES(server_status_announcements),
            live_match_panel = VALUES(live_match_panel),
            webhook_validation = VALUES(webhook_validation),
            admin_call_notifications = VALUES(admin_call_notifications),
            hub_bans_notifications = VALUES(hub_bans_notifications),
            music_bot = VALUES(music_bot),
            recharge_wallet_system = VALUES(recharge_wallet_system),
            mixton_ready_panel = VALUES(mixton_ready_panel),
            command_verificar = VALUES(command_verificar),
            command_desvincular = VALUES(command_desvincular),
            command_leaderboard = VALUES(command_leaderboard),
            command_stats = VALUES(command_stats),
            command_info = VALUES(command_info),
            command_sincronizar = VALUES(command_sincronizar),
            command_partidas = VALUES(command_partidas),
            command_crear_sala = VALUES(command_crear_sala),
            command_mixton_listo = VALUES(command_mixton_listo),
            command_mixton_no_listo = VALUES(command_mixton_no_listo),
            command_cs2_play = VALUES(command_cs2_play),
            command_sancionar = VALUES(command_sancionar),
            command_desancionar = VALUES(command_desancionar),
            command_say = VALUES(command_say),
            command_recargar = VALUES(command_recargar),
            command_setup_recargas = VALUES(command_setup_recargas),
            command_setup_soporte = VALUES(command_setup_soporte),
            command_llamar_admin = VALUES(command_llamar_admin),
            updated_at = CURRENT_TIMESTAMP(3)
        "#,
    )
    .bind(guild_id)
    .bind(auto_welcome).bind(auto_role).bind(auto_nick).bind(auto_vc).bind(auto_move)
    .bind(match_ann).bind(status_ann).bind(live_match).bind(webhook_val)
    .bind(admin_call).bind(hub_bans).bind(music_bot).bind(recharge_sys)
    .bind(mixton_ready).bind(cmd_verificar).bind(cmd_desvincular).bind(cmd_leaderboard)
    .bind(cmd_stats).bind(cmd_info).bind(cmd_sincronizar).bind(cmd_partidas).bind(cmd_crear_sala)
    .bind(cmd_mixton_listo).bind(cmd_mixton_no_listo).bind(cmd_cs2_play).bind(cmd_sancionar)
    .bind(cmd_desancionar).bind(cmd_say).bind(cmd_recargar).bind(cmd_setup_recargas)
    .bind(cmd_setup_soporte).bind(cmd_llamar_admin)
    .execute(&mut **tx)
    .await;

    // Sincronizar colas si existen en el JSON
    if let Some(queues) = val.get("queues").and_then(Value::as_array) {
        let _ = sqlx::query("DELETE FROM guild_queues WHERE guild_id = ?")
            .bind(guild_id)
            .execute(&mut **tx)
            .await;

        for (idx, q) in queues.iter().enumerate() {
            let q_code = text(q, "id").unwrap_or_else(|| format!("queue_{idx}"));
            let q_id = text(q, "queueId").or_else(|| text(q, "id")).unwrap_or_else(|| q_code.clone());
            let q_name = text(q, "name").unwrap_or_else(|| format!("Cola {}", idx + 1));
            let q_match = text(q, "discordMatchesChannelId");
            let q_wait = text(q, "discordWaitingRoomChannelId");
            let q_mixton = text(q, "discordMixtonChannelId");
            let q_mixton_chat = text(q, "discordMixtonChatChannelId");
            let q_access = text(q, "discordMatchAccessRoleId");
            let q_hub = text(q, "hubRoleId");
            let q_dhub = text(q, "discordHubRoleId");

            let _ = sqlx::query(
                r#"
                INSERT INTO guild_queues (
                    guild_id, queue_code, queue_id, name,
                    discord_matches_channel_id, discord_waiting_room_channel_id,
                    discord_mixton_channel_id, discord_mixton_chat_channel_id,
                    discord_match_access_role_id, hub_role_id, discord_hub_role_id,
                    sort_order
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(guild_id)
            .bind(q_code)
            .bind(q_id)
            .bind(q_name)
            .bind(q_match)
            .bind(q_wait)
            .bind(q_mixton)
            .bind(q_mixton_chat)
            .bind(q_access)
            .bind(q_hub)
            .bind(q_dhub)
            .bind(idx as u32)
            .execute(&mut **tx)
            .await;
        }
    }

    Ok(())
}

async fn sync_relational_delete(
    tx: &mut Transaction<'_, MySql>,
    collection: &str,
    id: &str,
) -> Result<(), DataError> {
    if collection == "faceit_users" || collection == "users" {
        let discord_id = id
            .rsplit_once('_')
            .map(|(_, d)| d)
            .unwrap_or(id);
        let _ = sqlx::query("DELETE FROM users WHERE discord_id = ?")
            .bind(discord_id)
            .execute(&mut **tx)
            .await;
    }
    Ok(())
}

// ============================================================================
// UTILIDADES PURAS DE VALIDACIÓN FINANCIERA
// ============================================================================

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn amount_of(v: Option<&Value>) -> Option<f64> {
    let n = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        _ => return None,
    };
    (n.is_finite() && n > 0.0).then_some(n)
}

fn amount_of_zero(v: Option<&Value>) -> Option<f64> {
    let n = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().replace(',', ".").parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn same_amount(a: f64, b: f64) -> bool {
    (a * 100.0).round() == (b * 100.0).round()
}

fn balance_cents(user: &Value) -> i64 {
    match amount_of_zero(user.get("wallet_balance")) {
        Some(b) if b > 0.0 => (b * 100.0).round() as i64,
        _ => 0,
    }
}

/// La cuenta ya tiene un celular registrado (texto no vacío).
fn has_phone(user: &Map<String, Value>) -> bool {
    user.get("phoneNumber")
        .and_then(Value::as_str)
        .is_some_and(|p| !p.trim().is_empty())
}

fn clean_winners(winners: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    winners
        .iter()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn realtime_watch_collection_emits_initial_snapshot_and_live_changes() {
        let client = MysqlClient::in_memory();
        client
            .set_doc("faceit_users", "u2", &json!({"faceit_nickname": "beta"}))
            .await
            .unwrap();
        client
            .set_doc("faceit_users", "u1", &json!({"faceit_nickname": "alpha"}))
            .await
            .unwrap();

        let mut stream = Box::pin(client.watch_collection("faceit_users"));

        // 1. Instantánea inicial ordenada + Synced
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Added("u1".into())
        );
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Added("u2".into())
        );
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Synced(vec!["u1".into(), "u2".into()])
        );

        // 2. Nuevo documento -> Added
        client
            .set_doc("faceit_users", "u3", &json!({"faceit_nickname": "gamma"}))
            .await
            .unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Added("u3".into())
        );

        // 3. Modificación -> Modified
        client
            .merge_doc("faceit_users", "u1", &json!({"faceit_elo": 2100}))
            .await
            .unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Modified("u1".into())
        );

        // 4. Borrado -> Removed
        client.delete_doc("faceit_users", "u2").await.unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Removed("u2".into())
        );
    }

    #[tokio::test]
    async fn realtime_watch_document_tracks_single_document() {
        let client = MysqlClient::in_memory();
        let mut stream = Box::pin(client.watch_document("faceit_config", "settings"));

        // No existe al abrir: Synced([])
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Synced(vec![])
        );

        // Escritura en otro documento de la misma colección se ignora
        client
            .set_doc("faceit_config", "other", &json!({"x": 1}))
            .await
            .unwrap();

        // Escritura en settings llega como Added y luego Modified
        client
            .set_doc("faceit_config", "settings", &json!({"gamesPerMatch": 1}))
            .await
            .unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Added("settings".into())
        );

        client
            .merge_doc("faceit_config", "settings", &json!({"gamesPerMatch": 3}))
            .await
            .unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ChangeEvent::Modified("settings".into())
        );
    }

    #[tokio::test]
    async fn un_pago_yape_sirve_para_recarga_o_compra_pero_no_ambas() {
        let client = MysqlClient::in_memory();
        for op in ["OP111", "OP222"] {
            client
                .set_doc(
                    "yape_transacciones",
                    op,
                    &json!({ "numero_operacion": op, "tipo": "ENTRADA", "monto": 10.0 }),
                )
                .await
                .unwrap();
        }
        let items = json!([{ "id": "prod_pack_10", "quantity": 1 }]);
        let customer = json!({ "name": "A", "email": "a@b.c", "discordId": "user_1" });

        // Recargada primero: la tienda la rechaza.
        client
            .credit_yape_operation("OP111", "user_1", None, 10.0, None, "web")
            .await
            .unwrap();
        assert!(matches!(
            client.checkout_purchase("OP111", 10.0, &customer, &items).await,
            Err(CheckoutError::AlreadyUsed)
        ));

        // Comprada primero: la recarga ya no acredita.
        client.checkout_purchase("OP222", 10.0, &customer, &items).await.unwrap();
        let again = client
            .credit_yape_operation("OP222", "user_1", None, 10.0, None, "web")
            .await
            .unwrap();
        assert_eq!(again, YapeCredit::AlreadyCredited { own: false });
        assert!(matches!(
            client.checkout_purchase("OP222", 10.0, &customer, &items).await,
            Err(CheckoutError::AlreadyUsed)
        ));
    }

    #[tokio::test]
    async fn acid_yape_recharge_withdraw_and_payout_work_end_to_end() {
        let client = MysqlClient::in_memory();

        // 1. El monitor registra un depósito Yape de S/ 25.00
        client
            .set_doc(
                "yape_transacciones",
                "OP998877",
                &json!({
                    "numero_operacion": "OP998877",
                    "tipo": "ENTRADA",
                    "monto": 25.0,
                    "fecha_hora": "2026-11-07 20:00:00"
                }),
            )
            .await
            .unwrap();

        let credit = client
            .credit_yape_operation("OP998877", "user_1", None, 25.0, Some("987654321"), "web")
            .await
            .unwrap();
        assert_eq!(
            credit,
            YapeCredit::Credited {
                operation: "OP998877".into(),
                amount: 25.0,
                new_balance: 25.0
            }
        );

        // Intentar acreditar por segunda vez devuelve AlreadyCredited
        let dup = client
            .credit_yape_operation("OP998877", "user_1", None, 25.0, None, "web")
            .await
            .unwrap();
        assert_eq!(dup, YapeCredit::AlreadyCredited { own: true });

        // 2. Solicitar un retiro de S/ 10.00
        let wd = client
            .request_withdrawal("user_1", 10.0, "987654321", "web")
            .await
            .unwrap();
        assert_eq!(wd.amount, 10.0);
        assert_eq!(wd.new_balance, 15.0);

        // 3. Al lanzar la partida se cobra S/ 5.00 a los 4 jugadores (user_1: 15 -> 10).
        for u in ["user_2", "user_3", "user_4"] {
            client
                .set_doc("faceit_users", u, &json!({ "wallet_balance": 5.0 }))
                .await
                .unwrap();
        }
        let players: Vec<String> = (1..=4).map(|i| format!("user_{i}")).collect();
        assert_eq!(
            client.collect_match_stakes("match_100", 500, &players, &[]).await.unwrap(),
            StakeOutcome::Collected { total_cents: 2000 }
        );

        // 4. Pagar premios a 2 ganadores (pozo S/ 20.00 -> S/ 10.00 c/u)
        let outcome = client
            .pay_match_winners(
                "match_100",
                500,
                4,
                &["user_1".to_string(), "user_2".to_string()],
                "matchzy",
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            PayoutOutcome::Paid(vec![
                ("user_1".into(), 1000),
                ("user_2".into(), 1000)
            ])
        );

        // Reintentar pago de la misma partida es idempotente
        let dup_payout = client
            .pay_match_winners(
                "match_100",
                500,
                4,
                &["user_1".to_string(), "user_2".to_string()],
                "matchzy",
            )
            .await
            .unwrap();
        assert_eq!(dup_payout, PayoutOutcome::AlreadyPaid);

        // Saldo final de user_1: 15.0 - 5.0 (apuesta) + 10.0 (premio) = 20.0
        let u1: Value = client.get_doc("faceit_users", "user_1").await.unwrap().unwrap();
        assert_eq!(u1["wallet_balance"], json!(20.0));
    }

    #[tokio::test]
    async fn finish_and_cancel_match_in_memory_updates_mysql_and_history() {
        let client = MysqlClient::in_memory();

        // 1. Crear partida activa con pozo
        let active_match = json!({
            "matchId": "match_cs2_99",
            "status": "ONGOING",
            "lifecycle_state": "active",
            "faction1_name": "CTs",
            "faction2_name": "Ts",
            "betAmount": 10.0,
            "faction1_roster": [
                { "discordId": "winner_player_1", "steam_id_64": "76561198000000001" },
                { "discordId": "winner_player_2", "steam_id_64": "76561198000000002" }
            ],
            "faction2_roster": [
                { "discordId": "loser_player_1", "steam_id_64": "76561198000000003" },
                { "discordId": "loser_player_2", "steam_id_64": "76561198000000004" }
            ]
        });
        client
            .set_doc("faceit_active_matches", "match_cs2_99", &active_match)
            .await
            .unwrap();

        assert!(client
            .get_doc::<Value>("faceit_active_matches", "match_cs2_99")
            .await
            .unwrap()
            .is_some());

        // 1b. Al lanzarla se cobró S/ 10 a cada uno de los 4 jugadores.
        let roster = ["winner_player_1", "winner_player_2", "loser_player_1", "loser_player_2"];
        for p in roster {
            client
                .set_doc("faceit_users", p, &json!({ "wallet_balance": 10.0 }))
                .await
                .unwrap();
        }
        let roster: Vec<String> = roster.iter().map(|s| s.to_string()).collect();
        client
            .collect_match_stakes("match_cs2_99", 1000, &roster, &[])
            .await
            .unwrap();

        // 2. Finalizar partida con stats de MatchZy
        let stats = json!({
            "map": "de_mirage",
            "duration": "42 min",
            "teams": [
                {
                    "name": "CTs",
                    "score": 13,
                    "players": [
                        { "name": "Winner 1", "steamid64": "76561198000000001", "kills": 24, "deaths": 12, "kd": 2.0, "damage": 95.4, "mvps": 4, "headshots": 10 },
                        { "name": "Winner 2", "steamid64": "76561198000000002", "kills": 18, "deaths": 14, "kd": 1.28, "damage": 82.1, "mvps": 3, "headshots": 8 }
                    ]
                },
                {
                    "name": "Ts",
                    "score": 8,
                    "players": [
                        { "name": "Loser 1", "steamid64": "76561198000000003", "kills": 15, "deaths": 19, "kd": 0.78, "damage": 68.0, "mvps": 1, "headshots": 5 },
                        { "name": "Loser 2", "steamid64": "76561198000000004", "kills": 11, "deaths": 21, "kd": 0.52, "damage": 55.3, "mvps": 0, "headshots": 4 }
                    ]
                }
            ]
        });

        let outcome = client
            .finish_match(
                "match_cs2_99",
                Some("team1"),
                Some(13),
                Some(8),
                Some(&stats),
                true,
            )
            .await
            .unwrap();

        assert_eq!(outcome.match_id, "match_cs2_99");
        assert_eq!(outcome.winner, "team1");
        assert_eq!(outcome.score_a, 13);
        assert_eq!(outcome.score_b, 8);
        assert!(outcome.stats_saved);
        assert_eq!(outcome.payouts.len(), 2);
        assert_eq!(outcome.payouts[0].amount, 20.0);
        assert_eq!(outcome.payouts[1].amount, 20.0);

        // 3. Ya no debe existir en faceit_active_matches
        assert!(client
            .get_doc::<Value>("faceit_active_matches", "match_cs2_99")
            .await
            .unwrap()
            .is_none());

        // 4. Debe existir en faceit_matches_history con status FINISHED
        let history: Value = client
            .get_doc("faceit_matches_history", "match_cs2_99")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(history["status"], json!("FINISHED"));
        assert_eq!(history["lifecycle_state"], json!("finished"));
        assert_eq!(history["winner"], json!("team1"));
        assert_eq!(history["faction1_score"], json!(13));
        assert_eq!(history["faction2_score"], json!(8));

        // 5. Estadísticas archivadas
        let s_doc: Value = client
            .get_doc("matches_stats", "match_cs2_99")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s_doc["map"], json!("de_mirage"));

        // 6. Probar cancelación
        client
            .set_doc(
                "faceit_active_matches",
                "match_cancel_test",
                &json!({ "matchId": "match_cancel_test" }),
            )
            .await
            .unwrap();
        client
            .cancel_match("match_cancel_test", Some("Jugador ausente"))
            .await
            .unwrap();
        assert!(client
            .get_doc::<Value>("faceit_active_matches", "match_cancel_test")
            .await
            .unwrap()
            .is_none());
        let cancel_history: Value = client
            .get_doc("faceit_matches_history", "match_cancel_test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancel_history["status"], json!("CANCELLED"));
        assert_eq!(cancel_history["lifecycle_state"], json!("cancelled"));
    }
}
