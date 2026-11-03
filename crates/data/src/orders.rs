//! Órdenes de la tienda (colección `orders`): modelo y repositorio de solo lectura.
//!
//! Las escribe `recordOrder` (`database.js` ~3200: id del documento = `orderId`, con `{ orderId,
//! ...orderData, createdAt: serverTimestamp(), createdAtIso, status: 'COMPLETADA' }`) y las
//! actualiza `updateOrderStatus` (`status`, `adminNote`, `updatedAt: serverTimestamp()`,
//! `updatedAtIso`). Las lee `getOrders` (~3268) para `GET /api/orders` (`routes/api.js` ~1956),
//! que responde `{ orders }`.
//!
//! Lectura del legacy: la colección **entera** (con `status`, solo `where('status', '==', status)`;
//! sin orden ni límite en Firestore), cada orden como `{ id: doc.id, ...doc.data() }`, orden en
//! memoria de más nueva a más vieja por `createdAtIso` (o `createdAt._seconds`) y `slice(0, limit)`.

use crate::error::DataError;
use crate::firestore::{FirestoreClient, Op, Query, Value as QValue};
use crate::model::{created_iso_sort_key, node_timestamp_verbatim, present, FakeStore};
use crate::source::{from_json, DocSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const COLLECTION: &str = "orders";

/// Una orden tal como la devuelve `GET /api/orders` del legacy. Todo lo que no son las fechas ni
/// el id se conserva tal cual en [`Order::extra`] (`orderId`, `status`, `createdAtIso`, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Order {
    /// `id`: el del documento salvo que el documento traiga un campo `id` propio.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<Value>,
    /// `createdAt`: `serverTimestamp()`; se serializa como `{ _seconds, _nanoseconds }`.
    #[serde(
        rename = "createdAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<Value>,
    /// `updatedAt`: `serverTimestamp()` de `updateOrderStatus`.
    #[serde(
        rename = "updatedAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<Value>,
    /// El resto de campos del documento, sin tocar.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Order {
    /// `status` si es un texto.
    pub fn status(&self) -> Option<&str> {
        self.extra.get("status").and_then(Value::as_str)
    }

    fn sort_key(&self) -> i64 {
        created_iso_sort_key(self.extra.get("createdAtIso"), self.created_at.as_ref())
    }
}

/// Lectura de las órdenes.
///
/// **Errores:** se devuelven tal cual (`DataError::QuotaExceeded` incluido). El legacy
/// (`getOrders`) traga *cualquier* error y devuelve `[]`, así que la ruta (Tarea 7) debe
/// responder `{ orders: [] }` ante un `Err`. No necesita índices compuestos.
#[async_trait]
pub trait OrdersRepo: Send + Sync {
    /// Las `limit` órdenes más recientes, solo las de estado `status` si se da. Lee la colección
    /// entera (o todas las de ese estado) y ordena en memoria, como el legacy.
    async fn list(&self, limit: u32, status: Option<&str>) -> Result<Vec<Order>, DataError>;
}

/// Consulta de `getOrders`. Pura (sin red).
pub(crate) fn list_query(status: Option<&str>) -> Query {
    Query {
        filters: status
            .map(|s| vec![("status".into(), Op::Eq, QValue::String(s.into()))])
            .unwrap_or_default(),
        order_by: None,
        limit: None,
    }
}

/// Orden en memoria y corte del legacy (estable: los empates conservan el orden de llegada).
fn newest_first(mut orders: Vec<Order>, limit: u32) -> Vec<Order> {
    orders.sort_by_key(|o| std::cmp::Reverse(o.sort_key()));
    orders.truncate(limit as usize);
    orders
}

/// Repo sobre Firestore (o sobre cualquier [`DocSource`]), sin caché.
pub struct FirestoreOrdersRepo<S = FirestoreClient> {
    source: S,
}

impl<S: DocSource> FirestoreOrdersRepo<S> {
    /// Repo sobre `source` (normalmente un [`FirestoreClient`]).
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

#[async_trait]
impl<S: DocSource> OrdersRepo for FirestoreOrdersRepo<S> {
    async fn list(&self, limit: u32, status: Option<&str>) -> Result<Vec<Order>, DataError> {
        let docs = self.source.query(COLLECTION, list_query(status)).await?;
        let orders = docs
            .into_iter()
            .map(|(id, doc)| {
                let mut order: Order = from_json(&id, doc)?;
                order.id.get_or_insert(Value::String(id));
                Ok(order)
            })
            .collect::<Result<Vec<_>, DataError>>()?;
        Ok(newest_first(orders, limit))
    }
}

/// Doble en memoria para pruebas de otros crates: filtra por estado, ordena y corta como el
/// legacy.
#[derive(Default)]
pub struct FakeOrdersRepo {
    store: FakeStore<Order>,
}

impl FakeOrdersRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Añade una orden.
    pub fn push(&self, order: Order) {
        self.store.push(order);
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        self.store.fail_with(error);
    }
}

#[async_trait]
impl OrdersRepo for FakeOrdersRepo {
    async fn list(&self, limit: u32, status: Option<&str>) -> Result<Vec<Order>, DataError> {
        let mut orders = self.store.snapshot()?;
        if let Some(status) = status {
            orders.retain(|o| o.status() == Some(status));
        }
        Ok(newest_first(orders, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::FakeSource;
    use serde_json::json;

    #[tokio::test]
    async fn orders_list_filters_by_status() {
        let source = FakeSource::new().reply(Ok(vec![
            (
                "ORD-1".into(),
                json!({"orderId": "ORD-1", "status": "PENDIENTE",
                       "createdAtIso": "2026-11-02T10:00:00.000Z",
                       "createdAt": "2026-11-02T10:00:00Z"}),
            ),
            (
                "ORD-2".into(),
                json!({"orderId": "ORD-2", "status": "PENDIENTE",
                       "createdAtIso": "2026-11-02T11:00:00.000Z",
                       "createdAt": "2026-11-02T11:00:00Z",
                       "updatedAt": "2026-11-02T11:30:00Z", "adminNote": ""}),
            ),
            // Sin `createdAtIso`: se ordena por `createdAt._seconds`.
            (
                "ORD-3".into(),
                json!({"orderId": "ORD-3", "status": "PENDIENTE", "createdAt": "2026-11-02T10:30:00Z"}),
            ),
        ]));
        let repo = FirestoreOrdersRepo::new(source);
        let orders = repo.list(2, Some("PENDIENTE")).await.unwrap();

        assert_eq!(
            repo.source.queries(),
            vec![(
                "orders".to_string(),
                Query {
                    filters: vec![("status".into(), Op::Eq, QValue::String("PENDIENTE".into()))],
                    order_by: None,
                    limit: None,
                }
            )]
        );
        assert_eq!(
            serde_json::to_value(&orders).unwrap(),
            json!([
                {"id": "ORD-2", "orderId": "ORD-2", "status": "PENDIENTE",
                 "createdAtIso": "2026-11-02T11:00:00.000Z",
                 "createdAt": {"_seconds": 1_793_617_200_i64, "_nanoseconds": 0},
                 "updatedAt": {"_seconds": 1_793_619_000_i64, "_nanoseconds": 0},
                 "adminNote": ""},
                {"id": "ORD-3", "orderId": "ORD-3", "status": "PENDIENTE",
                 "createdAt": {"_seconds": 1_793_615_400_i64, "_nanoseconds": 0}}
            ])
        );
    }

    #[test]
    fn without_status_reads_the_whole_collection() {
        assert_eq!(list_query(None), Query::default());
    }

    #[tokio::test]
    async fn list_propagates_quota_error() {
        let source = FakeSource::new().reply(Err(DataError::QuotaExceeded));
        assert_eq!(
            FirestoreOrdersRepo::new(source).list(100, None).await,
            Err(DataError::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn fake_filters_by_status_and_orders() {
        let fake = FakeOrdersRepo::new();
        for (id, status, iso) in [
            ("a", "COMPLETADA", "2026-11-02T10:00:00.000Z"),
            ("b", "PENDIENTE", "2026-11-02T11:00:00.000Z"),
            ("c", "COMPLETADA", "2026-11-02T12:00:00.000Z"),
        ] {
            fake.push(
                serde_json::from_value(json!({"id": id, "status": status, "createdAtIso": iso}))
                    .unwrap(),
            );
        }
        let ids: Vec<_> = fake
            .list(100, Some("COMPLETADA"))
            .await
            .unwrap()
            .into_iter()
            .map(|o| o.id.unwrap())
            .collect();
        assert_eq!(ids, vec![json!("c"), json!("a")]);
    }
}
