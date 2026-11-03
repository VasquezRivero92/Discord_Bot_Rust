//! Piezas comunes de los modelos tolerantes: deserializadores indulgentes y fechas al estilo Node.
//!
//! Los documentos los escribe el bot Node con firebase-admin y la API del legacy los devuelve con
//! `res.json(...)`. Un `Timestamp` de Firestore sale de ahí como `{ "_seconds": n,
//! "_nanoseconds": n }`; el cliente de Rust, en cambio, entrega los `Timestamp` como texto RFC 3339
//! (ver `firestore.rs`). Estas funciones deshacen esa diferencia en los campos de fecha conocidos.

use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

/// Texto (o número convertido a texto); cualquier otra cosa se lee como ausente.
pub(crate) fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

/// Solo objetos; cualquier otra cosa se lee como ausente.
pub(crate) fn lenient_object<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(obj @ Value::Object(_)) => Some(obj),
        _ => None,
    })
}

/// Solo números; cualquier otra cosa se lee como ausente.
pub(crate) fn lenient_number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(n @ Value::Number(_)) => Some(n),
        _ => None,
    })
}

/// `{ _seconds, _nanoseconds }` a partir de un texto RFC 3339, o `None` si no lo es.
fn rfc3339_to_node(s: &str) -> Option<Value> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| {
        json!({
            "_seconds": t.timestamp(),
            "_nanoseconds": t.timestamp_subsec_nanos(),
        })
    })
}

/// Marca de tiempo normalizada: un texto RFC 3339 pasa a `{ _seconds, _nanoseconds }`, un objeto
/// ya con esa forma se conserva y cualquier otra cosa se lee como ausente.
pub(crate) fn node_timestamp<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => rfc3339_to_node(&s),
        Some(obj @ Value::Object(_)) => Some(obj),
        _ => None,
    })
}

/// Como [`node_timestamp`] pero sin perder nada: un texto RFC 3339 pasa a `{ _seconds,
/// _nanoseconds }` y **cualquier otro valor presente se conserva tal cual** (también `null`, un
/// número de `Date.now()` o un texto que no es fecha), igual que lo reenviaría `res.json`. Ausente
/// sigue siendo `None` (y no se serializa).
pub(crate) fn node_timestamp_verbatim<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<Value>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(Some(match &value {
        Value::String(s) => rfc3339_to_node(s).unwrap_or(value),
        _ => value,
    }))
}

/// Cualquier valor presente se conserva (también `null`); ausente es `None`.
pub(crate) fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// Milisegundos de una marca de tiempo, como `Timestamp.toDate().getTime()` en Node: para
/// `{ _seconds, _nanoseconds }` es `segundos * 1000 + nanos / 1e6` (truncado); un texto se
/// interpreta como fecha RFC 3339. `None` si no es una fecha reconocible.
pub(crate) fn timestamp_millis(value: &Value) -> Option<i64> {
    match value {
        Value::Object(obj) => {
            let seconds = obj.get("_seconds")?.as_i64()?;
            let nanos = obj.get("_nanoseconds").and_then(Value::as_i64).unwrap_or(0);
            Some(seconds * 1000 + nanos / 1_000_000)
        }
        Value::String(s) => iso_millis(s),
        _ => None,
    }
}

/// Milisegundos de un texto ISO-8601/RFC 3339 (lo que escribe `new Date().toISOString()`).
pub(crate) fn iso_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// Clave de orden de `getClaims`/`getOrders` (`database.js` ~3231 y ~3279):
/// `createdAtIso ? new Date(createdAtIso).getTime() : (createdAt?._seconds ? _seconds * 1000 : 0)`.
///
/// Diferencia conocida: un `createdAtIso` presente pero no interpretable da `NaN` en Node (y un
/// orden indefinido); aquí cuenta como 0.
pub(crate) fn created_iso_sort_key(
    created_at_iso: Option<&Value>,
    created_at: Option<&Value>,
) -> i64 {
    match created_at_iso {
        Some(Value::String(s)) if !s.is_empty() => return iso_millis(s).unwrap_or(0),
        _ => {}
    }
    created_at
        .and_then(|v| v.get("_seconds"))
        .and_then(Value::as_i64)
        .filter(|s| *s != 0)
        .map(|s| s * 1000)
        .unwrap_or(0)
}

/// Almacén de los dobles `Fake*Repo`: entradas en memoria y un error opcional que, si está
/// puesto, devuelven todas las lecturas.
pub(crate) struct FakeStore<T> {
    entries: std::sync::Mutex<Vec<T>>,
    error: std::sync::Mutex<Option<crate::error::DataError>>,
}

impl<T> Default for FakeStore<T> {
    fn default() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
            error: std::sync::Mutex::new(None),
        }
    }
}

impl<T: Clone> FakeStore<T> {
    pub(crate) fn push(&self, entry: T) {
        self.entries.lock().expect("mutex envenenado").push(entry);
    }

    pub(crate) fn fail_with(&self, error: Option<crate::error::DataError>) {
        *self.error.lock().expect("mutex envenenado") = error;
    }

    /// Copia de las entradas, o el error puesto con [`FakeStore::fail_with`].
    pub(crate) fn snapshot(&self) -> Result<Vec<T>, crate::error::DataError> {
        if let Some(err) = self.error.lock().expect("mutex envenenado").clone() {
            return Err(err);
        }
        Ok(self.entries.lock().expect("mutex envenenado").clone())
    }
}

/// Lo que haría Firestore con `orderBy(campo, 'desc').limit(n)` sobre `entries`: descarta las que
/// no tienen el campo (Firestore no devuelve documentos sin el campo ordenado), ordena de más nueva
/// a más vieja (estable) y corta en `limit`.
pub(crate) fn newest_first<T>(
    mut entries: Vec<T>,
    created_at: impl Fn(&T) -> Option<&Value>,
    limit: Option<usize>,
) -> Vec<T> {
    entries.retain(|e| created_at(e).is_some());
    entries.sort_by_key(|e| {
        std::cmp::Reverse(created_at(e).and_then(timestamp_millis).unwrap_or(i64::MIN))
    });
    if let Some(limit) = limit {
        entries.truncate(limit);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "node_timestamp_verbatim")]
        at: Option<Value>,
    }

    fn probe(v: Value) -> Option<Value> {
        serde_json::from_value::<Probe>(v).unwrap().at
    }

    #[test]
    fn verbatim_timestamp_converts_rfc3339_and_keeps_everything_else() {
        assert_eq!(
            probe(json!({"at": "2026-11-02T10:00:00.005Z"})),
            Some(json!({"_seconds": 1_793_613_600_i64, "_nanoseconds": 5_000_000}))
        );
        assert_eq!(probe(json!({"at": null})), Some(Value::Null));
        assert_eq!(
            probe(json!({"at": 1_700_000_000_000_i64})),
            Some(json!(1_700_000_000_000_i64))
        );
        assert_eq!(probe(json!({"at": "ayer"})), Some(json!("ayer")));
        assert_eq!(probe(json!({})), None);
    }

    #[test]
    fn millis_match_node_to_date_get_time() {
        assert_eq!(
            timestamp_millis(&json!({"_seconds": 10, "_nanoseconds": 999_999_999})),
            Some(10_999)
        );
        assert_eq!(
            timestamp_millis(&json!("1970-01-01T00:00:01.5Z")),
            Some(1_500)
        );
        assert_eq!(timestamp_millis(&json!(5)), None);
    }

    #[test]
    fn created_iso_sort_key_prefers_iso_then_seconds() {
        let ts = json!({"_seconds": 7, "_nanoseconds": 900_000_000});
        assert_eq!(
            created_iso_sort_key(Some(&json!("1970-01-01T00:00:02.000Z")), Some(&ts)),
            2_000
        );
        // Sin `createdAtIso`: solo los segundos (sin nanos), como el legacy.
        assert_eq!(created_iso_sort_key(None, Some(&ts)), 7_000);
        assert_eq!(created_iso_sort_key(Some(&json!("")), Some(&ts)), 7_000);
        assert_eq!(created_iso_sort_key(None, None), 0);
        assert_eq!(created_iso_sort_key(Some(&json!("basura")), Some(&ts)), 0);
    }
}
