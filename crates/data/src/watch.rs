//! Bucle de watcher compartido: mantiene una [`TtlCache`] indexada por id de documento al día con
//! los cambios de una colección (lo usan `guild_settings` y `global_settings`).

use crate::cache::TtlCache;
use crate::error::DataError;
use crate::firestore::ChangeEvent;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub(crate) const BACKOFF_MIN: Duration = Duration::from_secs(1);
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Un stream que aguanta más que esto se considera sano: la espera vuelve al mínimo.
pub(crate) const STABLE_AFTER: Duration = Duration::from_secs(60);

/// Tarea del watcher; soltarla (o `abort`) la detiene.
pub struct WatcherHandle {
    task: JoinHandle<()>,
}

impl WatcherHandle {
    pub(crate) fn new(task: JoinHandle<()>) -> Self {
        Self { task }
    }

    /// Detiene el watcher.
    pub fn abort(&self) {
        self.task.abort();
    }

    /// `true` si la tarea ya terminó.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bucle infinito del watcher sobre `cache` (claves = ids de documento de la colección).
///
/// `Added`/`Modified` invalidan la clave (conservan la copia vieja para la cuota); `Removed` la
/// purga (entrada y copia vieja); `Reset` y cualquier error invalidan todo (`Reset` es idempotente
/// y puede llegar varias veces). Al abrir (y reabrir) el stream se invalida todo, y otra vez con el
/// `Synced` que emite el listener cuando su instantánea es actual, que además purga la copia vieja
/// de las claves que no están en ella (cubre lo cacheado y borrado entre la apertura y ese
/// momento, que nunca recibe `Removed`). Los eventos de documentos que no están en caché no
/// afectan a las demás claves. Cuando el stream termina (error o fin) se invalida todo y se reabre
/// con espera exponencial (1 s, el doble cada vez, tope 60 s; vuelve a 1 s tras un stream que duró
/// más de 60 s). `restarts` cuenta las reaperturas. Un stream que termina con error deja un
/// `warn` con el tipo de error y la espera (uno por reapertura).
pub(crate) async fn keep_cache_in_sync<V>(
    cache: &TtlCache<String, V>,
    restarts: &AtomicU64,
    open: impl Fn() -> BoxStream<'static, Result<ChangeEvent, DataError>>,
) where
    V: Clone + Send + Sync + 'static,
{
    let mut backoff = BACKOFF_MIN;
    let mut first = true;
    loop {
        if !first {
            restarts.fetch_add(1, Ordering::Relaxed);
        }
        first = false;
        let opened = Instant::now();
        let mut failed: Option<&'static str> = None;
        let mut stream = open();
        // Al (re)abrir también se invalida todo: un borrado durante la espera no emite evento.
        cache.invalidate_all();
        while let Some(item) = stream.next().await {
            match item {
                Ok(ChangeEvent::Added(id)) | Ok(ChangeEvent::Modified(id)) => {
                    cache.invalidate(&id).await
                }
                // Borrado: se descarta también la copia vieja (no servirla ante cuota).
                Ok(ChangeEvent::Removed(id)) => cache.purge(&id).await,
                Ok(ChangeEvent::Reset) => cache.invalidate_all(),
                // Instantánea actual: lo que no está en ella se borró (quizá sin evento) y
                // tampoco se sirve su copia vieja ante cuota.
                Ok(ChangeEvent::Synced(ids)) => {
                    let present: HashSet<String> = ids.into_iter().collect();
                    cache
                        .invalidate_all_and_purge_stale_unless(|id| present.contains(id))
                        .await
                }
                Err(err) => {
                    failed = Some(error_kind(&err));
                    cache.invalidate_all();
                    break;
                }
            }
        }
        drop(stream);
        cache.invalidate_all();
        if opened.elapsed() > STABLE_AFTER {
            backoff = BACKOFF_MIN;
        }
        if let Some(kind) = failed {
            // Una línea por reapertura: como mucho una por segundo, y una por minuto si el error
            // persiste (la espera exponencial ya lo limita). Solo el tipo de error, nunca su
            // mensaje (podría llevar nombres o contenido de documentos).
            tracing::warn!(
                error_kind = kind,
                backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX),
                "watcher de Firestore: el stream terminó con error; se reabre tras la espera"
            );
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Tipo de un [`DataError`] para el log, sin su mensaje.
fn error_kind(err: &DataError) -> &'static str {
    match err {
        DataError::Firestore(_) => "firestore",
        DataError::QuotaExceeded => "quota_exceeded",
        DataError::NotFound => "not_found",
        DataError::Decode(_) => "decode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kind_never_carries_the_message() {
        let secret = "projects/p/databases/(default)/documents/faceit_config/settings";
        for (err, want) in [
            (DataError::Firestore(secret.into()), "firestore"),
            (DataError::QuotaExceeded, "quota_exceeded"),
            (DataError::NotFound, "not_found"),
            (DataError::Decode(secret.into()), "decode"),
        ] {
            let kind = error_kind(&err);
            assert_eq!(kind, want);
            assert!(!kind.contains("faceit_config"));
        }
    }
}
