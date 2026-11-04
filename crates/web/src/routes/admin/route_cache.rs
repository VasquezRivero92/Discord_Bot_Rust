//! Cachés en memoria de las rutas del panel con la semántica de los `Map` del legacy
//! (`botLogsCache` de `/logs`, `statsCache` de `/stats`): cada clave guarda la respuesta y el
//! instante en que **empezó** la petición que la leyó; vale mientras `now - instante < ttl`. Sin
//! carga única por clave (el legacy tampoco la tiene). Con [`ROUTE_CACHE_MAX_KEYS`] claves, al
//! meter una nueva se purgan las caducadas y, si siguen sin caber, se vacía (los `Map` del
//! legacy no tienen tope).
//!
//! El reloj es el de tokio (`tokio::time::Instant`) para poder pausarlo en las pruebas.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

/// Tope de claves de cada caché.
pub const ROUTE_CACHE_MAX_KEYS: usize = 1000;

/// Caché por clave con vida `ttl`.
pub(crate) struct RouteCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, (Instant, Arc<Value>)>>,
}

impl RouteCache {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::default(),
        }
    }

    pub(crate) fn get(&self, key: &str, now: Instant) -> Option<Arc<Value>> {
        let entries = self.entries.lock().expect("mutex envenenado");
        entries
            .get(key)
            .filter(|(at, _)| now.saturating_duration_since(*at) < self.ttl)
            .map(|(_, v)| v.clone())
    }

    pub(crate) fn insert(&self, key: String, at: Instant, value: Arc<Value>) {
        let mut entries = self.entries.lock().expect("mutex envenenado");
        if entries.len() >= ROUTE_CACHE_MAX_KEYS && !entries.contains_key(&key) {
            entries.retain(|_, (t, _)| at.saturating_duration_since(*t) < self.ttl);
            if entries.len() >= ROUTE_CACHE_MAX_KEYS {
                entries.clear();
            }
        }
        entries.insert(key, (at, value));
    }

    /// `cache.clear()`.
    pub(crate) fn clear(&self) {
        self.entries.lock().expect("mutex envenenado").clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().expect("mutex envenenado").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test(start_paused = true)]
    async fn cache_entry_expires_after_ttl_and_is_bounded() {
        let ttl = Duration::from_secs(30);
        let c = RouteCache::new(ttl);
        let t0 = Instant::now();
        c.insert("global:50".into(), t0, Arc::new(json!([1])));
        assert!(c
            .get("global:50", t0 + Duration::from_millis(29_999))
            .is_some());
        assert!(c.get("global:50", t0 + ttl).is_none());
        for i in 0..ROUTE_CACHE_MAX_KEYS {
            c.insert(format!("k{i}"), t0, Arc::new(json!([])));
        }
        assert!(c.len() <= ROUTE_CACHE_MAX_KEYS);
        c.clear();
        assert_eq!(c.len(), 0);
    }
}
