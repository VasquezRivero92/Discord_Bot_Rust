//! Caché con TTL, de una sola carga por clave y con copia "vieja" para tolerar fallos.

use moka::future::Cache;
use std::future::Future;
use std::hash::Hash;
use std::time::Duration;

/// Caché asíncrona con TTL sobre `moka`.
///
/// - **Una sola carga por clave** (single-flight): si varias tareas fallan a la vez en la misma
///   clave, solo una ejecuta el cargador y las demás esperan su resultado (también si es un error).
/// - **Copia vieja**: además de la entrada fresca (con TTL), se guarda el último valor cargado
///   con éxito de cada clave en un mapa acotado que no caduca. Si una recarga falla con un error
///   que el llamador considera tolerable (p. ej. cuota agotada), se puede servir esa copia con
///   [`TtlCache::get_or_load_stale_on`].
/// - [`TtlCache::invalidate`] e [`TtlCache::invalidate_all`] solo descartan la entrada fresca
///   (fuerzan una recarga) y **conservan** la copia vieja; [`TtlCache::purge`] descarta ambas.
pub struct TtlCache<K, V> {
    fresh: Cache<K, V>,
    stale: Cache<K, V>,
}

impl<K, V> TtlCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// `ttl`: vida de una entrada fresca; `max_capacity`: máximo de claves (frescas y viejas).
    pub fn new(ttl: Duration, max_capacity: u64) -> Self {
        Self {
            fresh: Cache::builder()
                .max_capacity(max_capacity)
                .time_to_live(ttl)
                .build(),
            stale: Cache::builder().max_capacity(max_capacity).build(),
        }
    }

    /// Valor fresco de `key`, o lo carga con `loader` (una sola ejecución por clave a la vez).
    /// Un error no se cachea.
    pub async fn get_or_load<F, Fut, E>(&self, key: K, loader: F) -> Result<V, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>> + Send,
        E: Clone + Send + Sync + 'static,
    {
        self.get_or_load_stale_on(key, loader, |_| false).await
    }

    /// Como [`TtlCache::get_or_load`], pero si el cargador falla con un error para el que
    /// `is_stale_ok` es `true` y existe una copia vieja, devuelve esa copia en vez del error.
    pub async fn get_or_load_stale_on<F, Fut, E>(
        &self,
        key: K,
        loader: F,
        is_stale_ok: impl Fn(&E) -> bool,
    ) -> Result<V, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>> + Send,
        E: Clone + Send + Sync + 'static,
    {
        let stale = self.stale.clone();
        let stale_key = key.clone();
        let init = async move {
            let value = loader().await?;
            stale.insert(stale_key, value.clone()).await;
            Ok(value)
        };
        match self.fresh.try_get_with(key.clone(), init).await {
            Ok(value) => Ok(value),
            Err(err) => {
                if is_stale_ok(&err) {
                    if let Some(old) = self.stale.get(&key).await {
                        return Ok(old);
                    }
                }
                Err((*err).clone())
            }
        }
    }

    /// Descarta la entrada fresca de `key` (la próxima lectura recarga); conserva la copia vieja.
    pub async fn invalidate(&self, key: &K) {
        self.fresh.invalidate(key).await;
    }

    /// Descarta todas las entradas frescas (idempotente); conserva las copias viejas.
    pub fn invalidate_all(&self) {
        self.fresh.invalidate_all();
    }

    /// Descarta la entrada fresca y la copia vieja de `key`.
    pub async fn purge(&self, key: &K) {
        self.fresh.invalidate(key).await;
        self.stale.invalidate(key).await;
    }
}
