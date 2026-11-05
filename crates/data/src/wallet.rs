//! Monedero de jugadores: lectura fusionada (servidor/global), recarga y débito atómico.
//!
//! Colección Firestore `faceit_users`:
//! - Documento de servidor: `{guild_id}_{discord_id}`.
//! - Documento global: `{discord_id}`.
//! - Saldo fusionado: `wallet_balance` del documento del servidor si existe y no es nulo;
//!   en caso contrario, el del documento global; si ninguno existe, 0.0.
//! - Todas las actualizaciones (`adjust`, `debit_exact`) se aplican sobre el documento del
//!   servidor `{guild_id}_{discord_id}` (o `{discord_id}` si no hay servidor).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use firestore::errors::FirestoreErrorInTransaction;
use firestore::{FirestoreDocument, FirestoreTransactionOps};
use futures::FutureExt;
use serde_json::Value;

use crate::error::DataError;
use crate::firestore::FirestoreClient;

/// Errores derivados de operaciones con el monedero.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WalletError {
    /// Saldo insuficiente para realizar el débito estricto.
    #[error("saldo insuficiente: saldo actual {balance}")]
    Insufficient { balance: f64 },

    /// Parámetro no numérico, negativo o inválido.
    #[error("parámetro de saldo inválido: {0}")]
    Invalid(String),

    /// Error de persistencia o comunicación con Firestore.
    #[error("error de almacenamiento: {0}")]
    Store(String),
}

impl From<DataError> for WalletError {
    fn from(err: DataError) -> Self {
        WalletError::Store(err.to_string())
    }
}

/// Redondeo a 2 cifras decimales según la semántica del legado: `(x * 100).round() / 100`.
/// Evita también el `-0.0` de punto flotante de IEEE 754.
pub fn round2(x: f64) -> f64 {
    let r = (x * 100.0).round() / 100.0;
    if r.abs() == 0.0 {
        0.0
    } else {
        r
    }
}

/// Obtiene el identificador del documento de servidor en `faceit_users`.
pub fn server_doc_id(guild_id: Option<&str>, discord_id: &str) -> String {
    match guild_id {
        Some(g) if !g.trim().is_empty() => format!("{}_{}", g.trim(), discord_id.trim()),
        _ => discord_id.trim().to_string(),
    }
}

/// Obtiene el identificador del documento global en `faceit_users`.
pub fn global_doc_id(discord_id: &str) -> String {
    discord_id.trim().to_string()
}

#[derive(serde::Serialize)]
struct BalanceUpdatePayload {
    wallet_balance: f64,
    last_balance_update: i64,
    last_balance_reason: String,
}

#[derive(serde::Serialize)]
struct BalanceSetPayload {
    #[serde(rename = "discordId")]
    discord_id: String,
    guild_id: Option<String>,
    wallet_balance: f64,
    #[serde(rename = "createdAt")]
    created_at: std::time::SystemTime,
    last_balance_update: i64,
    last_balance_reason: String,
}

fn extract_balance_from_map(map: &serde_json::Map<String, Value>) -> Option<f64> {
    match map.get("wallet_balance") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Contrato para interactuar con los saldos de los jugadores.
#[async_trait]
pub trait WalletStore: Send + Sync {
    /// Consulta el saldo fusionado de un jugador (gana servidor, luego global, luego 0.0).
    async fn balance(&self, guild_id: Option<&str>, discord_id: &str) -> Result<f64, WalletError>;

    /// Documentos fusionados por jugador (campos del global y encima los no nulos del servidor)
    /// para leer `wallet_balance`, `phoneNumber`, `steam_id_64`, etc. (equivalente a `getFullUsersMap`).
    async fn users_map(
        &self,
        guild_id: Option<&str>,
        discord_ids: &[String],
    ) -> Result<HashMap<String, Value>, WalletError>;

    /// Ajusta el saldo sumando `delta` (positivo o negativo) con tope inferior 0.0.
    /// Devuelve el nuevo saldo resultante tras la operación atómica.
    async fn adjust(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        delta: f64,
        reason: &str,
    ) -> Result<f64, WalletError>;

    /// Débito estricto: descuenta `amount` (debe ser > 0 y finito) de forma atómica.
    /// Si el saldo actual fusionado es inferior a `amount`, falla con `WalletError::Insufficient`
    /// sin modificar la base de datos.
    async fn debit_exact(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        amount: f64,
        reason: &str,
    ) -> Result<f64, WalletError>;
}

/// Implementación en memoria de `WalletStore` con ejecución atómica bajo Mutex.
#[derive(Debug, Default)]
pub struct FakeWalletStore {
    docs: Mutex<HashMap<String, Value>>,
}

impl FakeWalletStore {
    /// Crea una nueva instancia en memoria vacía.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserta o reemplaza un documento crudo en la colección `faceit_users` simulada.
    pub fn insert_doc(&self, id: impl Into<String>, val: Value) {
        if let Ok(mut docs) = self.docs.lock() {
            docs.insert(id.into(), val);
        }
    }

    /// Fija el saldo de un jugador en servidor o globalmente con campos estándar.
    pub fn set_balance(&self, guild_id: Option<&str>, discord_id: &str, balance: f64) {
        let doc_id = server_doc_id(guild_id, discord_id);
        let val = serde_json::json!({
            "discordId": discord_id,
            "guild_id": guild_id,
            "wallet_balance": round2(balance),
            "createdAt": crate::write::timestamp_now(),
            "last_balance_update": chrono::Utc::now().timestamp_millis(),
            "last_balance_reason": "test setup"
        });
        self.insert_doc(doc_id, val);
    }
}

#[async_trait]
impl WalletStore for FakeWalletStore {
    async fn balance(&self, guild_id: Option<&str>, discord_id: &str) -> Result<f64, WalletError> {
        let docs = self
            .docs
            .lock()
            .map_err(|e| WalletError::Store(e.to_string()))?;

        let s_id = server_doc_id(guild_id, discord_id);
        let g_id = global_doc_id(discord_id);

        let s_bal = docs
            .get(&s_id)
            .and_then(Value::as_object)
            .and_then(extract_balance_from_map);

        let g_bal = if s_id != g_id {
            docs.get(&g_id)
                .and_then(Value::as_object)
                .and_then(extract_balance_from_map)
        } else {
            None
        };

        let actual = s_bal.or(g_bal).unwrap_or(0.0);
        Ok(round2(actual))
    }

    async fn users_map(
        &self,
        guild_id: Option<&str>,
        discord_ids: &[String],
    ) -> Result<HashMap<String, Value>, WalletError> {
        let docs = self
            .docs
            .lock()
            .map_err(|e| WalletError::Store(e.to_string()))?;

        let mut res = HashMap::new();

        for d_id in discord_ids {
            let s_id = server_doc_id(guild_id, d_id);
            let g_id = global_doc_id(d_id);

            let mut merged = serde_json::Map::new();

            if let Some(Value::Object(g_map)) = docs.get(&g_id) {
                merged = g_map.clone();
            }

            if s_id != g_id {
                if let Some(Value::Object(s_map)) = docs.get(&s_id) {
                    for (k, v) in s_map {
                        if !v.is_null() {
                            merged.insert(k.clone(), v.clone());
                        }
                    }
                }
            }

            res.insert(d_id.clone(), Value::Object(merged));
        }

        Ok(res)
    }

    async fn adjust(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        delta: f64,
        reason: &str,
    ) -> Result<f64, WalletError> {
        if !delta.is_finite() {
            return Err(WalletError::Invalid(format!("delta inválido: {delta}")));
        }

        let mut docs = self
            .docs
            .lock()
            .map_err(|e| WalletError::Store(e.to_string()))?;

        let s_id = server_doc_id(guild_id, discord_id);
        let g_id = global_doc_id(discord_id);

        let s_bal = docs
            .get(&s_id)
            .and_then(Value::as_object)
            .and_then(extract_balance_from_map);

        let g_bal = if s_id != g_id {
            docs.get(&g_id)
                .and_then(Value::as_object)
                .and_then(extract_balance_from_map)
        } else {
            None
        };

        let actual = round2(s_bal.or(g_bal).unwrap_or(0.0));
        let mut nuevo = round2(actual + delta);
        if nuevo < 0.0 {
            nuevo = 0.0;
        }

        let now_ms = chrono::Utc::now().timestamp_millis();

        if let Some(Value::Object(existing)) = docs.get_mut(&s_id) {
            existing.insert("wallet_balance".into(), serde_json::json!(nuevo));
            existing.insert("last_balance_update".into(), serde_json::json!(now_ms));
            existing.insert("last_balance_reason".into(), serde_json::json!(reason));
        } else {
            let doc = serde_json::json!({
                "discordId": discord_id,
                "guild_id": guild_id,
                "wallet_balance": nuevo,
                "createdAt": crate::write::timestamp_now(),
                "last_balance_update": now_ms,
                "last_balance_reason": reason,
            });
            docs.insert(s_id, doc);
        }

        Ok(nuevo)
    }

    async fn debit_exact(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        amount: f64,
        reason: &str,
    ) -> Result<f64, WalletError> {
        if !amount.is_finite() || amount <= 0.0 {
            return Err(WalletError::Invalid(format!(
                "monto a debitar debe ser positivo y finito: {amount}"
            )));
        }

        let mut docs = self
            .docs
            .lock()
            .map_err(|e| WalletError::Store(e.to_string()))?;

        let s_id = server_doc_id(guild_id, discord_id);
        let g_id = global_doc_id(discord_id);

        let s_bal = docs
            .get(&s_id)
            .and_then(Value::as_object)
            .and_then(extract_balance_from_map);

        let g_bal = if s_id != g_id {
            docs.get(&g_id)
                .and_then(Value::as_object)
                .and_then(extract_balance_from_map)
        } else {
            None
        };

        let actual = round2(s_bal.or(g_bal).unwrap_or(0.0));
        if actual < amount {
            return Err(WalletError::Insufficient { balance: actual });
        }

        let nuevo = round2(actual - amount);
        let now_ms = chrono::Utc::now().timestamp_millis();

        if let Some(Value::Object(existing)) = docs.get_mut(&s_id) {
            existing.insert("wallet_balance".into(), serde_json::json!(nuevo));
            existing.insert("last_balance_update".into(), serde_json::json!(now_ms));
            existing.insert("last_balance_reason".into(), serde_json::json!(reason));
        } else {
            let doc = serde_json::json!({
                "discordId": discord_id,
                "guild_id": guild_id,
                "wallet_balance": nuevo,
                "createdAt": crate::write::timestamp_now(),
                "last_balance_update": now_ms,
                "last_balance_reason": reason,
            });
            docs.insert(s_id, doc);
        }

        Ok(nuevo)
    }
}

#[async_trait]
impl WalletStore for FirestoreClient {
    async fn balance(&self, guild_id: Option<&str>, discord_id: &str) -> Result<f64, WalletError> {
        let s_id = server_doc_id(guild_id, discord_id);
        let g_id = global_doc_id(discord_id);
        let is_different = s_id != g_id;

        let s_doc: Option<FirestoreDocument> = self
            .db
            .fluent()
            .select()
            .by_id_in("faceit_users")
            .one(&s_id)
            .await
            .map_err(|e| WalletError::Store(e.to_string()))?;

        let s_map = s_doc
            .as_ref()
            .map(|d| crate::node_docs::node_fields(&d.fields));
        let s_bal = s_map.as_ref().and_then(extract_balance_from_map);

        let g_bal = if s_bal.is_none() && is_different {
            let g_doc: Option<FirestoreDocument> = self
                .db
                .fluent()
                .select()
                .by_id_in("faceit_users")
                .one(&g_id)
                .await
                .map_err(|e| WalletError::Store(e.to_string()))?;
            let g_map = g_doc
                .as_ref()
                .map(|d| crate::node_docs::node_fields(&d.fields));
            g_map.as_ref().and_then(extract_balance_from_map)
        } else {
            None
        };

        let actual = s_bal.or(g_bal).unwrap_or(0.0);
        Ok(round2(actual))
    }

    async fn users_map(
        &self,
        guild_id: Option<&str>,
        discord_ids: &[String],
    ) -> Result<HashMap<String, Value>, WalletError> {
        let mut res = HashMap::new();

        for d_id in discord_ids {
            let s_id = server_doc_id(guild_id, d_id);
            let g_id = global_doc_id(d_id);
            let is_different = s_id != g_id;

            let mut merged = serde_json::Map::new();

            let g_doc: Option<FirestoreDocument> = self
                .db
                .fluent()
                .select()
                .by_id_in("faceit_users")
                .one(&g_id)
                .await
                .map_err(|e| WalletError::Store(e.to_string()))?;

            if let Some(doc) = g_doc {
                merged = crate::node_docs::node_fields(&doc.fields);
            }

            if is_different {
                let s_doc: Option<FirestoreDocument> = self
                    .db
                    .fluent()
                    .select()
                    .by_id_in("faceit_users")
                    .one(&s_id)
                    .await
                    .map_err(|e| WalletError::Store(e.to_string()))?;

                if let Some(doc) = s_doc {
                    let s_map = crate::node_docs::node_fields(&doc.fields);
                    for (k, v) in s_map {
                        if !v.is_null() {
                            merged.insert(k, v);
                        }
                    }
                }
            }

            res.insert(d_id.clone(), Value::Object(merged));
        }

        Ok(res)
    }

    async fn adjust(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        delta: f64,
        reason: &str,
    ) -> Result<f64, WalletError> {
        if !delta.is_finite() {
            return Err(WalletError::Invalid(format!("delta inválido: {delta}")));
        }

        let s_id = server_doc_id(guild_id, discord_id);
        let g_id = global_doc_id(discord_id);
        let is_different = s_id != g_id;

        let res = self
            .db
            .run_transaction(|db, transaction| {
                let s_id = s_id.clone();
                let g_id = g_id.clone();
                let discord_id = discord_id.to_string();
                let guild_id = guild_id.map(|s| s.to_string());
                let reason = reason.to_string();

                async move {
                    let s_doc: Option<FirestoreDocument> = db
                        .fluent()
                        .select()
                        .by_id_in("faceit_users")
                        .one(&s_id)
                        .await?;

                    let g_doc: Option<FirestoreDocument> = if is_different {
                        db.fluent()
                            .select()
                            .by_id_in("faceit_users")
                            .one(&g_id)
                            .await?
                    } else {
                        None
                    };

                    let s_map = s_doc
                        .as_ref()
                        .map(|d| crate::node_docs::node_fields(&d.fields));
                    let g_map = g_doc
                        .as_ref()
                        .map(|d| crate::node_docs::node_fields(&d.fields));

                    let s_bal = s_map.as_ref().and_then(extract_balance_from_map);
                    let g_bal = g_map.as_ref().and_then(extract_balance_from_map);

                    let actual = round2(s_bal.or(g_bal).unwrap_or(0.0));
                    let mut nuevo = round2(actual + delta);
                    if nuevo < 0.0 {
                        nuevo = 0.0;
                    }

                    let now_ms = chrono::Utc::now().timestamp_millis();

                    if s_doc.is_some() {
                        let payload = BalanceUpdatePayload {
                            wallet_balance: nuevo,
                            last_balance_update: now_ms,
                            last_balance_reason: reason.clone(),
                        };
                        transaction.update_object(
                            "faceit_users",
                            &s_id,
                            &payload,
                            Some(vec![
                                "wallet_balance".to_string(),
                                "last_balance_update".to_string(),
                                "last_balance_reason".to_string(),
                            ]),
                            None,
                            vec![],
                        )?;
                    } else {
                        let payload = BalanceSetPayload {
                            discord_id: discord_id.clone(),
                            guild_id: guild_id.clone(),
                            wallet_balance: nuevo,
                            created_at: std::time::SystemTime::now(),
                            last_balance_update: now_ms,
                            last_balance_reason: reason.clone(),
                        };
                        transaction.update_object(
                            "faceit_users",
                            &s_id,
                            &payload,
                            None,
                            None,
                            vec![],
                        )?;
                    }

                    Ok(nuevo)
                }
                .boxed()
            })
            .await;

        match res {
            Ok(v) => Ok(v),
            Err(err) => match err {
                firestore::errors::FirestoreError::ErrorInTransaction(e) => {
                    if let Some(we) = e.source.downcast_ref::<WalletError>() {
                        Err(we.clone())
                    } else {
                        Err(WalletError::Store(e.to_string()))
                    }
                }
                other => Err(WalletError::Store(other.to_string())),
            },
        }
    }

    async fn debit_exact(
        &self,
        guild_id: Option<&str>,
        discord_id: &str,
        amount: f64,
        reason: &str,
    ) -> Result<f64, WalletError> {
        if !amount.is_finite() || amount <= 0.0 {
            return Err(WalletError::Invalid(format!(
                "monto a debitar debe ser positivo y finito: {amount}"
            )));
        }

        let s_id = server_doc_id(guild_id, discord_id);
        let g_id = global_doc_id(discord_id);
        let is_different = s_id != g_id;

        let res = self
            .db
            .run_transaction(|db, transaction| {
                let s_id = s_id.clone();
                let g_id = g_id.clone();
                let discord_id = discord_id.to_string();
                let guild_id = guild_id.map(|s| s.to_string());
                let reason = reason.to_string();

                async move {
                    let s_doc: Option<FirestoreDocument> = db
                        .fluent()
                        .select()
                        .by_id_in("faceit_users")
                        .one(&s_id)
                        .await?;

                    let g_doc: Option<FirestoreDocument> = if is_different {
                        db.fluent()
                            .select()
                            .by_id_in("faceit_users")
                            .one(&g_id)
                            .await?
                    } else {
                        None
                    };

                    let s_map = s_doc
                        .as_ref()
                        .map(|d| crate::node_docs::node_fields(&d.fields));
                    let g_map = g_doc
                        .as_ref()
                        .map(|d| crate::node_docs::node_fields(&d.fields));

                    let s_bal = s_map.as_ref().and_then(extract_balance_from_map);
                    let g_bal = g_map.as_ref().and_then(extract_balance_from_map);

                    let actual = round2(s_bal.or(g_bal).unwrap_or(0.0));
                    if actual < amount {
                        return Err(FirestoreErrorInTransaction::permanent(
                            transaction,
                            WalletError::Insufficient { balance: actual },
                        ));
                    }

                    let nuevo = round2(actual - amount);
                    let now_ms = chrono::Utc::now().timestamp_millis();

                    if s_doc.is_some() {
                        let payload = BalanceUpdatePayload {
                            wallet_balance: nuevo,
                            last_balance_update: now_ms,
                            last_balance_reason: reason.clone(),
                        };
                        transaction.update_object(
                            "faceit_users",
                            &s_id,
                            &payload,
                            Some(vec![
                                "wallet_balance".to_string(),
                                "last_balance_update".to_string(),
                                "last_balance_reason".to_string(),
                            ]),
                            None,
                            vec![],
                        )?;
                    } else {
                        let payload = BalanceSetPayload {
                            discord_id: discord_id.clone(),
                            guild_id: guild_id.clone(),
                            wallet_balance: nuevo,
                            created_at: std::time::SystemTime::now(),
                            last_balance_update: now_ms,
                            last_balance_reason: reason.clone(),
                        };
                        transaction.update_object(
                            "faceit_users",
                            &s_id,
                            &payload,
                            None,
                            None,
                            vec![],
                        )?;
                    }

                    Ok(nuevo)
                }
                .boxed()
            })
            .await;

        match res {
            Ok(v) => Ok(v),
            Err(err) => match err {
                firestore::errors::FirestoreError::ErrorInTransaction(e) => {
                    if let Some(we) = e.source.downcast_ref::<WalletError>() {
                        Err(we.clone())
                    } else {
                        Err(WalletError::Store(e.to_string()))
                    }
                }
                other => Err(WalletError::Store(other.to_string())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_lectura_fusionada_servidor_gana_luego_global_luego_cero() {
        let store = FakeWalletStore::new();

        // 1. Sin registros -> saldo 0.0
        let b = store.balance(Some("g1"), "u1").await.unwrap();
        assert_eq!(b, 0.0);

        // 2. Solo global -> devuelve saldo global
        store.set_balance(None, "u1", 25.50);
        let b_global = store.balance(Some("g1"), "u1").await.unwrap();
        assert_eq!(b_global, 25.50);

        // 3. Documento de servidor creado -> servidor gana sobre global
        store.set_balance(Some("g1"), "u1", 100.0);
        let b_server = store.balance(Some("g1"), "u1").await.unwrap();
        assert_eq!(b_server, 100.0);

        // El usuario en otro servidor "g2" sin doc propio sigue viendo el global
        let b_g2 = store.balance(Some("g2"), "u1").await.unwrap();
        assert_eq!(b_g2, 25.50);
    }

    #[tokio::test]
    async fn test_adjust_crea_doc_si_no_existe_suma_y_no_baja_de_cero() {
        let store = FakeWalletStore::new();

        // Doc no existe -> adjust crea doc con saldo = delta (si delta > 0)
        let nuevo = store
            .adjust(Some("g1"), "u2", 50.0, "deposito inicial")
            .await
            .unwrap();
        assert_eq!(nuevo, 50.0);
        assert_eq!(store.balance(Some("g1"), "u2").await.unwrap(), 50.0);

        // Suma saldo
        let suma = store.adjust(Some("g1"), "u2", 20.25, "bono").await.unwrap();
        assert_eq!(suma, 70.25);

        // Resta sin bajar de 0
        let resta_excesiva = store
            .adjust(Some("g1"), "u2", -100.0, "penalizacion")
            .await
            .unwrap();
        assert_eq!(resta_excesiva, 0.0);
        assert_eq!(store.balance(Some("g1"), "u2").await.unwrap(), 0.0);
    }

    #[tokio::test]
    async fn test_debit_exact_suficiente_insuficiente_invalido() {
        let store = FakeWalletStore::new();
        store.set_balance(Some("g1"), "u3", 50.0);

        // Monto inválido (<= 0 o no finito)
        let err_cero = store
            .debit_exact(Some("g1"), "u3", 0.0, "invalido")
            .await
            .unwrap_err();
        assert!(matches!(err_cero, WalletError::Invalid(_)));

        let err_negativo = store
            .debit_exact(Some("g1"), "u3", -10.0, "invalido")
            .await
            .unwrap_err();
        assert!(matches!(err_negativo, WalletError::Invalid(_)));

        // Saldo insuficiente
        let err_insuf = store
            .debit_exact(Some("g1"), "u3", 60.0, "demasiado")
            .await
            .unwrap_err();
        assert_eq!(err_insuf, WalletError::Insufficient { balance: 50.0 });
        // No debe haberse descontado nada
        assert_eq!(store.balance(Some("g1"), "u3").await.unwrap(), 50.0);

        // Saldo suficiente
        let nuevo = store
            .debit_exact(Some("g1"), "u3", 20.0, "compra")
            .await
            .unwrap();
        assert_eq!(nuevo, 30.0);
        assert_eq!(store.balance(Some("g1"), "u3").await.unwrap(), 30.0);
    }

    #[tokio::test]
    async fn test_20_debitos_concurrentes_de_10_sobre_saldo_100() {
        let store = Arc::new(FakeWalletStore::new());
        store.set_balance(Some("g1"), "u_concurrente", 100.0);

        let mut handles = Vec::new();
        for _ in 0..20 {
            let st = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                st.debit_exact(Some("g1"), "u_concurrente", 10.0, "debito concurrente")
                    .await
            }));
        }

        let mut oks = 0;
        let mut errs = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(_) => oks += 1,
                Err(WalletError::Insufficient { .. }) => errs += 1,
                Err(other) => panic!("error inesperado: {other:?}"),
            }
        }

        assert_eq!(oks, 10, "Exactamente 10 debitos de 10 deben ser exitosos");
        assert_eq!(
            errs, 10,
            "Exactamente 10 debitos deben fallar por saldo insuficiente"
        );

        let saldo_final = store.balance(Some("g1"), "u_concurrente").await.unwrap();
        assert_eq!(saldo_final, 0.0);
    }

    #[tokio::test]
    async fn test_redondeo_a_2_decimales() {
        let suma = 0.1 + 0.2;
        assert_ne!(suma, 0.3); // Prueba de imprecisión típica de f64
        assert_eq!(round2(suma), 0.3);
        assert_eq!(round2(1.234), 1.23);
        assert_eq!(round2(1.236), 1.24);
        assert_eq!(round2(-0.0001), 0.0);
    }
}
