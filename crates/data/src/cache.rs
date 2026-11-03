//! Caché con TTL, de una sola carga por clave y con copia "vieja" para tolerar fallos.

use moka::future::Cache;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Caché asíncrona con TTL sobre `moka`.
///
/// - **Una sola carga por clave** (single-flight): si varias tareas fallan a la vez en la misma
///   clave, solo una ejecuta el cargador y las demás esperan su resultado (también si es un error).
/// - **Copia vieja**: además de la entrada fresca (con TTL), se guarda el último valor cargado
///   con éxito de cada clave en un mapa acotado que no caduca. Si una recarga falla con un error
///   que el llamador considera tolerable (p. ej. cuota agotada), se puede servir esa copia con
///   [`TtlCache::get_or_load_stale_on`].
/// - **Época global**: `invalidate`, `invalidate_all` y `purge` incrementan un contador. Si cambia
///   mientras una carga está en curso, el llamador recibe su valor pero la entrada no se queda en
///   caché (la siguiente lectura recarga), y la copia vieja tampoco se escribe (así una carga que
///   corrió contra un `purge` no la resucita; es conservador también para `invalidate`). La
///   época se vuelve a mirar **después** de cada escritura (fresca y vieja): si cambió entre la
///   comprobación y la escritura, lo escrito se descarta. Si el llamador se cancela entre una
///   escritura y esa segunda comprobación, el descarte se hace en una tarea aparte al soltarse
///   (si no hay runtime de tokio en ese momento, la cota es el TTL).
/// - [`TtlCache::invalidate`] e [`TtlCache::invalidate_all`] solo descartan la entrada fresca
///   (fuerzan una recarga) y **conservan** la copia vieja; [`TtlCache::purge`] descarta ambas.
pub struct TtlCache<K, V> {
    fresh: Cache<K, V>,
    stale: Cache<K, V>,
    epoch: Arc<AtomicU64>,
    #[cfg(test)]
    hooks: Arc<tests::Hooks>,
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
            epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            hooks: Arc::default(),
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
        let epoch = self.epoch.clone();
        let seen = self.epoch.load(Ordering::SeqCst);
        #[cfg(test)]
        let hooks = self.hooks.clone();
        let init = async move {
            let started = epoch.load(Ordering::SeqCst);
            let value = loader().await?;
            if epoch.load(Ordering::SeqCst) == started {
                #[cfg(test)]
                hooks.before_stale_insert.pass().await;
                // Un `purge` entre la comprobación de arriba y la escritura la resucitaría: se
                // vuelve a mirar la época después de escribir y, si cambió, se descarta.
                let guard = EpochGuard::new(stale.clone(), stale_key.clone(), epoch, started);
                stale.insert(stale_key, value.clone()).await;
                guard.check().await;
            }
            Ok(value)
        };
        // Cubre que `moka` inserte la entrada fresca y el llamador se cancele antes de la
        // comprobación posterior (en un punto de espera interno de `try_get_with`).
        let guard = EpochGuard::new(self.fresh.clone(), key.clone(), self.epoch.clone(), seen);
        match self.fresh.try_get_with(key.clone(), init).await {
            Ok(value) => {
                #[cfg(test)]
                self.hooks.after_fresh_insert.pass().await;
                // Hubo una invalidación durante la carga: no dejar el valor cacheado.
                guard.check().await;
                Ok(value)
            }
            Err(err) => {
                guard.disarm();
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
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.fresh.invalidate(key).await;
    }

    /// Descarta todas las entradas frescas (idempotente); conserva las copias viejas.
    pub fn invalidate_all(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.fresh.invalidate_all();
    }

    /// Descarta la entrada fresca y la copia vieja de `key`.
    pub async fn purge(&self, key: &K) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.fresh.invalidate(key).await;
        self.stale.invalidate(key).await;
    }

    /// Como [`TtlCache::invalidate_all`], y además descarta la copia vieja de toda clave para la
    /// que `keep` es `false` (p. ej. claves que ya no existen en la fuente). Idempotente. Una
    /// carga en curso no reescribe después una copia vieja descartada (la época cambia).
    pub async fn invalidate_all_and_purge_stale_unless(&self, keep: impl Fn(&K) -> bool) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.fresh.invalidate_all();
        let absent: Vec<Arc<K>> = self
            .stale
            .iter()
            .map(|(key, _)| key)
            .filter(|key| !keep(key))
            .collect();
        for key in absent {
            self.stale.invalidate(&*key).await;
        }
    }
}

/// Descarta `key` de `cache` si la época cambió desde `seen`. Se comprueba con
/// [`EpochGuard::check`] tras escribir; si se suelta sin comprobar (el futuro se canceló), hace
/// la comprobación al soltarse y, si toca, el descarte en una tarea aparte. Un descarte de más
/// solo cuesta una recarga.
struct EpochGuard<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    armed: Option<(Cache<K, V>, K)>,
    epoch: Arc<AtomicU64>,
    seen: u64,
}

impl<K, V> EpochGuard<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn new(cache: Cache<K, V>, key: K, epoch: Arc<AtomicU64>, seen: u64) -> Self {
        Self {
            armed: Some((cache, key)),
            epoch,
            seen,
        }
    }

    fn changed(&self) -> bool {
        self.epoch.load(Ordering::SeqCst) != self.seen
    }

    /// Comprueba la época y descarta si cambió. Si este futuro se cancela a medias, el guardia
    /// sigue armado y repite el descarte al soltarse.
    async fn check(mut self) {
        if self.changed() {
            if let Some((cache, key)) = &self.armed {
                cache.invalidate(key).await;
            }
        }
        self.armed = None;
    }

    /// No hay nada que descartar (p. ej. la carga falló y no se escribió nada).
    fn disarm(mut self) {
        self.armed = None;
    }
}

impl<K, V> Drop for EpochGuard<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// La invalidación lanzada en una tarea aparte corre más tarde y puede descartar también una
    /// entrada más nueva y válida de la misma clave escrita entretanto (solo cuesta una recarga).
    fn drop(&mut self) {
        if !self.changed() {
            return;
        }
        if let Some((cache, key)) = self.armed.take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move { cache.invalidate(&key).await });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DataError;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tokio::sync::Semaphore;

    /// Puertas de prueba en los puntos de carrera de [`TtlCache::get_or_load_stale_on`].
    #[derive(Default)]
    pub(super) struct Hooks {
        /// Tras comprobar la época y antes de escribir la copia vieja.
        pub(super) before_stale_insert: Gate,
        /// Tras insertar `moka` la entrada fresca y antes de la comprobación posterior (simula
        /// un punto de espera interno de `moka` donde el llamador puede ser cancelado).
        pub(super) after_fresh_insert: Gate,
    }

    /// Puerta de un solo uso: inactiva salvo que se arme; al pasar avisa y espera permiso.
    pub(super) struct Gate {
        armed: AtomicBool,
        reached: Semaphore,
        proceed: Semaphore,
    }

    impl Default for Gate {
        fn default() -> Self {
            Self {
                armed: AtomicBool::new(false),
                reached: Semaphore::new(0),
                proceed: Semaphore::new(0),
            }
        }
    }

    impl Gate {
        pub(super) async fn pass(&self) {
            if self.armed.swap(false, Ordering::SeqCst) {
                self.reached.add_permits(1);
                self.proceed.acquire().await.expect("abierta").forget();
            }
        }
        fn arm(&self) {
            self.armed.store(true, Ordering::SeqCst);
        }
        async fn reached(&self) {
            self.reached.acquire().await.expect("abierta").forget();
        }
        fn release(&self) {
            self.proceed.add_permits(1);
        }
    }

    const LONG: Duration = Duration::from_secs(3600);

    fn key() -> String {
        "k".to_string()
    }

    async fn settle() {
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
    }

    async fn quota_read(cache: &TtlCache<String, u32>) -> Result<u32, DataError> {
        cache
            .get_or_load_stale_on(
                key(),
                || async { Err::<u32, _>(DataError::QuotaExceeded) },
                |e| matches!(e, DataError::QuotaExceeded),
            )
            .await
    }

    /// `purge` entre la comprobación de época y `stale.insert`: la copia vieja no debe quedar.
    #[tokio::test]
    async fn stale_write_after_purge_is_dropped() {
        let cache = Arc::new(TtlCache::<String, u32>::new(LONG, 10));
        cache.hooks.before_stale_insert.arm();
        let leader = {
            let cache = cache.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(key(), || async { Ok::<_, DataError>(1) })
                    .await
            })
        };
        // La carga ya pasó la comprobación de época y está a punto de escribir la copia vieja.
        cache.hooks.before_stale_insert.reached().await;
        cache.purge(&key()).await;
        cache.hooks.before_stale_insert.release();
        assert_eq!(leader.await.unwrap(), Ok(1));
        assert_eq!(
            quota_read(&cache).await,
            Err(DataError::QuotaExceeded),
            "la copia vieja escrita tras el purge se descarta"
        );
    }

    /// Sin carrera la copia vieja sí se escribe (control del anterior).
    #[tokio::test]
    async fn stale_write_without_race_is_kept() {
        let cache = Arc::new(TtlCache::<String, u32>::new(LONG, 10));
        cache.hooks.before_stale_insert.arm();
        let leader = {
            let cache = cache.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(key(), || async { Ok::<_, DataError>(1) })
                    .await
            })
        };
        cache.hooks.before_stale_insert.reached().await;
        cache.hooks.before_stale_insert.release();
        assert_eq!(leader.await.unwrap(), Ok(1));
        cache.invalidate(&key()).await;
        assert_eq!(quota_read(&cache).await, Ok(1));
    }

    /// El llamador que dirige la carga se cancela tras insertar `moka` la entrada fresca y antes
    /// de la comprobación posterior, con una invalidación durante la carga: la entrada no debe
    /// quedarse en caché.
    #[tokio::test]
    async fn cancelled_leader_after_fresh_insert_does_not_keep_entry() {
        let cache = Arc::new(TtlCache::<String, u32>::new(LONG, 10));
        let loads = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));
        cache.hooks.after_fresh_insert.arm();
        let leader = {
            let (cache, loads, gate) = (cache.clone(), loads.clone(), gate.clone());
            tokio::spawn(async move {
                cache
                    .get_or_load(key(), || async move {
                        let n = loads.fetch_add(1, Ordering::SeqCst) as u32 + 1;
                        gate.acquire().await.expect("abierta").forget();
                        Ok::<_, DataError>(n)
                    })
                    .await
            })
        };
        settle().await;
        assert_eq!(loads.load(Ordering::SeqCst), 1, "la carga está retenida");
        cache.invalidate(&key()).await;
        gate.add_permits(1);
        cache.hooks.after_fresh_insert.reached().await;
        leader.abort();
        assert!(leader.await.unwrap_err().is_cancelled());
        settle().await;

        let loads2 = loads.clone();
        let again = cache
            .get_or_load(key(), || async move {
                Ok::<_, DataError>(loads2.fetch_add(1, Ordering::SeqCst) as u32 + 1)
            })
            .await;
        assert_eq!(
            again,
            Ok(2),
            "recarga: la entrada de la carga invalidada no quedó"
        );
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }
}
