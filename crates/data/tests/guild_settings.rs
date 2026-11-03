//! Pruebas del repositorio de ajustes por servidor con una fuente falsa (sin Firestore).

use async_trait::async_trait;
use data::{
    ChangeEvent, DataError, FirestoreGuildSettingsRepo, GuildSettings, GuildSettingsRepo,
    GuildSettingsSource, InMemoryGuildSettingsRepo, TtlCache,
};
use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};

type Item = Result<ChangeEvent, DataError>;

struct FakeSource {
    docs: Mutex<HashMap<String, GuildSettings>>,
    fetches: AtomicUsize,
    failure: Mutex<Option<DataError>>,
    gate: Option<Arc<Semaphore>>,
    streams: Mutex<VecDeque<mpsc::UnboundedReceiver<Item>>>,
    watch_calls: AtomicUsize,
}

impl FakeSource {
    fn new() -> Arc<Self> {
        Self::build(None)
    }
    fn gated(gate: Arc<Semaphore>) -> Arc<Self> {
        Self::build(Some(gate))
    }
    fn build(gate: Option<Arc<Semaphore>>) -> Arc<Self> {
        Arc::new(Self {
            docs: Mutex::new(HashMap::new()),
            fetches: AtomicUsize::new(0),
            failure: Mutex::new(None),
            gate,
            streams: Mutex::new(VecDeque::new()),
            watch_calls: AtomicUsize::new(0),
        })
    }
    fn put(&self, id: &str, admin_role: &str) {
        self.docs
            .lock()
            .unwrap()
            .insert(id.into(), settings(admin_role));
    }
    fn fail_with(&self, err: Option<DataError>) {
        *self.failure.lock().unwrap() = err;
    }
    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }
    fn watch_calls(&self) -> usize {
        self.watch_calls.load(Ordering::SeqCst)
    }
    /// Encola un stream de `watch()` y devuelve su emisor.
    fn queue_stream(&self) -> mpsc::UnboundedSender<Item> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.streams.lock().unwrap().push_back(rx);
        tx
    }
}

/// Adaptador: `Arc<FakeSource>` como fuente (el repo es dueño de la fuente, la prueba conserva
/// otro `Arc`).
struct Shared(Arc<FakeSource>);

#[async_trait]
impl GuildSettingsSource for Shared {
    async fn fetch(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError> {
        let src = &self.0;
        src.fetches.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &src.gate {
            gate.acquire().await.unwrap().forget();
        }
        if let Some(err) = src.failure.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(src.docs.lock().unwrap().get(guild_id).cloned())
    }

    fn watch(&self) -> BoxStream<'static, Item> {
        self.0.watch_calls.fetch_add(1, Ordering::SeqCst);
        let rx = self.0.streams.lock().unwrap().pop_front();
        match rx {
            Some(rx) => futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            })
            .boxed(),
            None => futures::stream::pending().boxed(),
        }
    }
}

fn settings(admin_role: &str) -> GuildSettings {
    GuildSettings {
        admin_role_id: Some(admin_role.into()),
        ..Default::default()
    }
}

fn repo(src: &Arc<FakeSource>, ttl: Duration) -> Arc<FirestoreGuildSettingsRepo<Shared>> {
    Arc::new(FirestoreGuildSettingsRepo::new(Shared(src.clone()), ttl))
}

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

const LONG: Duration = Duration::from_secs(3600);

#[tokio::test]
async fn second_get_hits_cache_not_firestore() {
    let src = FakeSource::new();
    src.put("g1", "r1");
    let repo = repo(&src, LONG);
    let a = repo.get("g1").await.unwrap();
    let b = repo.get("g1").await.unwrap();
    assert_eq!(a, Some(settings("r1")));
    assert_eq!(a, b);
    assert_eq!(src.fetches(), 1);
}

#[tokio::test(start_paused = true)]
async fn watch_modified_invalidates_key() {
    let src = FakeSource::new();
    src.put("g1", "old");
    src.put("g2", "other");
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _watcher = repo.clone().spawn_watcher();
    settle().await;
    repo.get("g1").await.unwrap();
    repo.get("g2").await.unwrap();
    assert_eq!(src.fetches(), 2);

    src.put("g1", "new");
    tx.send(Ok(ChangeEvent::Modified("g1".into()))).unwrap();
    settle().await;
    assert_eq!(repo.get("g1").await.unwrap(), Some(settings("new")));
    assert_eq!(src.fetches(), 3);
    // La otra clave sigue en caché.
    repo.get("g2").await.unwrap();
    assert_eq!(src.fetches(), 3);
}

#[tokio::test(start_paused = true)]
async fn removed_invalidates_key() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _watcher = repo.clone().spawn_watcher();
    settle().await;
    assert!(repo.get("g1").await.unwrap().is_some());
    src.docs.lock().unwrap().remove("g1");
    tx.send(Ok(ChangeEvent::Removed("g1".into()))).unwrap();
    settle().await;
    assert_eq!(repo.get("g1").await.unwrap(), None);
    assert_eq!(src.fetches(), 2);
}

#[tokio::test]
async fn ttl_expiry_reloads() {
    // moka usa su propio reloj (no el pausable de tokio): TTL diminuto y espera holgada.
    let src = FakeSource::new();
    src.put("g1", "r");
    let repo = repo(&src, Duration::from_millis(30));
    repo.get("g1").await.unwrap();
    repo.get("g1").await.unwrap();
    assert_eq!(src.fetches(), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    repo.get("g1").await.unwrap();
    assert_eq!(src.fetches(), 2);
}

#[tokio::test]
async fn quota_exceeded_serves_stale_value_if_present() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let repo = repo(&src, Duration::from_millis(30));
    let good = repo.get("g1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await; // caduca la entrada fresca
    src.fail_with(Some(DataError::QuotaExceeded));
    let stale = repo.get("g1").await.expect("sirve el valor viejo");
    assert_eq!(stale, good);
    assert!(src.fetches() >= 2, "intentó recargar");
}

#[tokio::test]
async fn quota_exceeded_without_stale_value_propagates() {
    let src = FakeSource::new();
    src.fail_with(Some(DataError::QuotaExceeded));
    let repo = repo(&src, LONG);
    assert_eq!(repo.get("g1").await, Err(DataError::QuotaExceeded));
}

#[tokio::test]
async fn other_errors_propagate_and_do_not_serve_stale() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let repo = repo(&src, Duration::from_millis(30));
    repo.get("g1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    src.fail_with(Some(DataError::Firestore("boom".into())));
    assert_eq!(
        repo.get("g1").await,
        Err(DataError::Firestore("boom".into()))
    );
}

#[tokio::test]
async fn invalidate_keeps_stale_copy_for_quota_fallback() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let repo = repo(&src, LONG);
    repo.get("g1").await.unwrap();
    repo.invalidate("g1").await;
    src.fail_with(Some(DataError::QuotaExceeded));
    assert_eq!(repo.get("g1").await.unwrap(), Some(settings("r")));
}

#[tokio::test(start_paused = true)]
async fn watcher_reconnect_invalidates_all() {
    let src = FakeSource::new();
    src.put("g1", "a");
    src.put("g2", "b");
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _watcher = repo.clone().spawn_watcher();
    settle().await;
    repo.get("g1").await.unwrap();
    repo.get("g2").await.unwrap();
    assert_eq!(src.fetches(), 2);

    // Reset (dos veces: idempotente) invalida todo.
    tx.send(Ok(ChangeEvent::Reset)).unwrap();
    tx.send(Ok(ChangeEvent::Reset)).unwrap();
    settle().await;
    repo.get("g1").await.unwrap();
    repo.get("g2").await.unwrap();
    assert_eq!(src.fetches(), 4);
    assert_eq!(repo.watcher_restarts(), 0);

    // Error terminal + fin de stream: invalida todo y reabre tras la espera.
    let tx2 = src.queue_stream();
    tx.send(Err(DataError::Firestore("target retirado".into())))
        .unwrap();
    drop(tx);
    settle().await;
    repo.get("g1").await.unwrap();
    assert_eq!(src.fetches(), 5);
    assert_eq!(src.watch_calls(), 1, "aún no reabre: espera de 1 s");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(src.watch_calls(), 2);
    assert_eq!(repo.watcher_restarts(), 1);

    // El nuevo stream funciona.
    repo.get("g2").await.unwrap(); // 6
    src.put("g2", "b2");
    tx2.send(Ok(ChangeEvent::Added("g2".into()))).unwrap();
    settle().await;
    assert_eq!(repo.get("g2").await.unwrap(), Some(settings("b2")));
}

#[tokio::test(start_paused = true)]
async fn watcher_backoff_doubles_and_caps() {
    let src = FakeSource::new();
    // Cada stream termina en cuanto se abre.
    for _ in 0..8 {
        drop(src.queue_stream());
    }
    let repo = repo(&src, LONG);
    let _watcher = repo.clone().spawn_watcher();
    settle().await;
    assert_eq!(src.watch_calls(), 1);
    // esperas: 1, 2, 4, 8, 16, 32, 60, 60
    let start = tokio::time::Instant::now();
    let mut expected_calls = 1;
    let mut reopen_at_ms = 0u64;
    for wait_ms in [1000u64, 2000, 4000, 8000, 16000, 32000, 60000, 60000] {
        reopen_at_ms += wait_ms;
        let target = start + Duration::from_millis(reopen_at_ms);
        tokio::time::sleep_until(target - Duration::from_millis(1)).await;
        assert_eq!(src.watch_calls(), expected_calls, "antes de {wait_ms} ms");
        tokio::time::sleep(Duration::from_millis(2)).await;
        expected_calls += 1;
        assert_eq!(src.watch_calls(), expected_calls, "tras {wait_ms} ms");
    }
    assert_eq!(repo.watcher_restarts(), 8);
}

#[tokio::test(start_paused = true)]
async fn backoff_resets_after_a_long_lived_stream() {
    let src = FakeSource::new();
    for _ in 0..2 {
        drop(src.queue_stream());
    }
    let repo = repo(&src, LONG);
    let _w = repo.clone().spawn_watcher();
    settle().await;
    // stream 1 corto: espera 1 s; stream 2 corto: espera 2 s => backoff actual 4 s
    tokio::time::sleep(Duration::from_millis(1002)).await;
    assert_eq!(src.watch_calls(), 2);
    // Tercer stream: vive 61 s y luego termina.
    let tx3 = src.queue_stream();
    tokio::time::sleep(Duration::from_millis(2002)).await;
    assert_eq!(src.watch_calls(), 3);
    tokio::time::sleep(Duration::from_secs(61)).await;
    drop(tx3);
    settle().await;
    // Reinicia a 1 s (no 4 s).
    tokio::time::sleep(Duration::from_millis(999)).await;
    assert_eq!(src.watch_calls(), 3);
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(src.watch_calls(), 4);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_handle_stops_the_watcher() {
    let src = FakeSource::new();
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let watcher = repo.clone().spawn_watcher();
    settle().await;
    assert!(!tx.is_closed());
    drop(watcher);
    settle().await;
    assert!(tx.is_closed(), "la tarea soltó el stream");
}

#[tokio::test]
async fn concurrent_misses_run_one_load() {
    let gate = Arc::new(Semaphore::new(0));
    let src = FakeSource::gated(gate.clone());
    src.put("g1", "r");
    let repo = repo(&src, LONG);
    let barrier = Arc::new(tokio::sync::Barrier::new(9));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let repo = repo.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.get("g1").await
        }));
    }
    barrier.wait().await;
    settle().await;
    gate.add_permits(100);
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap(), Some(settings("r")));
    }
    assert_eq!(src.fetches(), 1);
}

#[tokio::test(start_paused = true)]
async fn negative_result_is_cached_until_added_event() {
    let src = FakeSource::new();
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _w = repo.clone().spawn_watcher();
    settle().await;
    assert_eq!(repo.get("nuevo").await.unwrap(), None);
    assert_eq!(repo.get("nuevo").await.unwrap(), None);
    assert_eq!(src.fetches(), 1);
    src.put("nuevo", "r");
    tx.send(Ok(ChangeEvent::Added("nuevo".into()))).unwrap();
    settle().await;
    assert_eq!(repo.get("nuevo").await.unwrap(), Some(settings("r")));
    assert_eq!(src.fetches(), 2);
}

#[tokio::test]
async fn ttl_cache_purge_drops_stale_copy() {
    let cache: TtlCache<String, u32> = TtlCache::new(LONG, 10);
    let ok = |v: u32| async move { Ok::<_, DataError>(v) };
    assert_eq!(cache.get_or_load("k".into(), || ok(1)).await, Ok(1));
    cache.purge(&"k".to_string()).await;
    let res = cache
        .get_or_load_stale_on(
            "k".into(),
            || async { Err::<u32, _>(DataError::QuotaExceeded) },
            |e| matches!(e, DataError::QuotaExceeded),
        )
        .await;
    assert_eq!(res, Err(DataError::QuotaExceeded));
}

#[tokio::test]
async fn in_memory_repo_behind_dyn_trait() {
    let mem = InMemoryGuildSettingsRepo::new();
    mem.insert("g1", settings("r"));
    let dynrepo: Arc<dyn GuildSettingsRepo> = Arc::new(mem);
    assert_eq!(dynrepo.get("g1").await.unwrap(), Some(settings("r")));
    assert_eq!(dynrepo.get("nope").await.unwrap(), None);
}

#[test]
fn guild_settings_roundtrip_keeps_unknown_fields() {
    let json = serde_json::json!({
        "adminRoleId": "123",
        "discordLinkChannelId": 456,
        "matchTimeoutMinutes": "15",
        "gamesPerMatch": 3,
        "queues": [{"id": "q1"}],
        "campoNuevo": {"a": [1, 2]},
        "otro": true
    });
    let parsed: GuildSettings = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.admin_role_id.as_deref(), Some("123"));
    assert_eq!(parsed.discord_link_channel_id.as_deref(), Some("456"));
    assert_eq!(parsed.match_timeout_minutes, Some(15));
    assert_eq!(parsed.games_per_match, Some(3));
    assert_eq!(parsed.queues.len(), 1);
    assert_eq!(parsed.extra["otro"], serde_json::json!(true));
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(back["campoNuevo"], serde_json::json!({"a": [1, 2]}));
    assert_eq!(back["adminRoleId"], "123");
    assert_eq!(back["otro"], true);
    assert!(back.get("hubRoleId").is_none());
}

#[derive(Clone, Copy)]
enum Inval {
    Key,
    All,
    Purge,
    Nothing,
}

async fn invalidation_during_load(kind: Inval) -> usize {
    let gate = Arc::new(Semaphore::new(0));
    let src = FakeSource::gated(gate.clone());
    src.put("g1", "r");
    let repo = repo(&src, LONG);
    let first = {
        let repo = repo.clone();
        tokio::spawn(async move { repo.get("g1").await })
    };
    settle().await;
    assert_eq!(src.fetches(), 1, "la carga está retenida");
    match kind {
        Inval::Key => repo.invalidate("g1").await,
        Inval::All => repo.invalidate_all(),
        Inval::Purge => repo.purge("g1").await,
        Inval::Nothing => {}
    }
    gate.add_permits(100);
    assert_eq!(first.await.unwrap().unwrap(), Some(settings("r")));
    repo.get("g1").await.unwrap();
    src.fetches()
}

#[tokio::test]
async fn invalidate_during_load_forces_reload() {
    assert_eq!(invalidation_during_load(Inval::Key).await, 2);
}

#[tokio::test]
async fn invalidate_all_during_load_forces_reload() {
    assert_eq!(invalidation_during_load(Inval::All).await, 2);
}

#[tokio::test]
async fn purge_during_load_forces_reload() {
    assert_eq!(invalidation_during_load(Inval::Purge).await, 2);
}

#[tokio::test]
async fn no_invalidation_during_load_still_caches() {
    assert_eq!(invalidation_during_load(Inval::Nothing).await, 1);
}

#[tokio::test]
async fn purge_during_load_does_not_resurrect_stale_copy() {
    let gate = Arc::new(Semaphore::new(0));
    let src = FakeSource::gated(gate.clone());
    src.put("g1", "r");
    let repo = repo(&src, LONG);
    let first = {
        let repo = repo.clone();
        tokio::spawn(async move { repo.get("g1").await })
    };
    settle().await;
    repo.purge("g1").await;
    gate.add_permits(100);
    first.await.unwrap().unwrap();
    src.fail_with(Some(DataError::QuotaExceeded));
    assert_eq!(repo.get("g1").await, Err(DataError::QuotaExceeded));
}

#[tokio::test(start_paused = true)]
async fn deletion_during_backoff_gap_is_seen_after_reopen() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _w = repo.clone().spawn_watcher();
    settle().await;
    drop(tx); // el stream termina; espera 1 s
    settle().await;
    assert_eq!(repo.get("g1").await.unwrap(), Some(settings("r")));
    src.docs.lock().unwrap().remove("g1");
    assert_eq!(
        repo.get("g1").await.unwrap(),
        Some(settings("r")),
        "aún cacheado"
    );
    let _tx2 = src.queue_stream();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(src.watch_calls(), 2);
    assert_eq!(repo.get("g1").await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn removed_then_quota_never_serves_old_settings() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _w = repo.clone().spawn_watcher();
    settle().await;
    repo.get("g1").await.unwrap();
    tx.send(Ok(ChangeEvent::Removed("g1".into()))).unwrap();
    settle().await;
    src.fail_with(Some(DataError::QuotaExceeded));
    assert_eq!(repo.get("g1").await, Err(DataError::QuotaExceeded));
}

#[tokio::test(start_paused = true)]
async fn modified_then_quota_still_serves_stale_copy() {
    let src = FakeSource::new();
    src.put("g1", "r");
    let tx = src.queue_stream();
    let repo = repo(&src, LONG);
    let _w = repo.clone().spawn_watcher();
    settle().await;
    repo.get("g1").await.unwrap();
    tx.send(Ok(ChangeEvent::Modified("g1".into()))).unwrap();
    settle().await;
    src.fail_with(Some(DataError::QuotaExceeded));
    assert_eq!(repo.get("g1").await.unwrap(), Some(settings("r")));
}
