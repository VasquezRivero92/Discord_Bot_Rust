# Migración a Rust, fase 2A: base de sesión/datos y lecturas de administración (plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Que Rust atienda las lecturas de `/api/admin/*` que solo dependen de Firestore y de la sesión, sobre una base de datos y sesión ya endurecida, y medir con datos reales si la migración reduce RAM y latencia.

**Architecture:** Sigue el estrangulamiento de la fase 1: Rust es la puerta de entrada y envía al legacy todo lo no migrado. Se añaden repositorios de lectura en `crates/data` (consulta con filtro/orden/límite), el puerto de `requireAuth` en `crates/web`, y un router de administración que se monta antes del proxy. Las escrituras (POST/PUT/DELETE) y todo lo que necesita el gateway de Discord, la API de Faceit o estado en memoria de Node siguen en el proxy.

**Tech Stack:** Rust 2021 (toolchain de `rust-toolchain.toml`), axum, `firestore` 0.61.1, moka, `jsonwebtoken` (único backend `aws_lc_rs`), crate `contrast` y `tools/*.mjs` ya existentes.

**Spec:** `docs/superpowers/specs/2026-11-02-migracion-rust-design.md` (sección 5, fase 2; este plan cubre solo el tramo 2A, ver «Fuera de este plan»).

## Global Constraints

- `D:\Discord_Faceit` es de solo lectura; el comportamiento se deriva de su código y se fija con casos de `contrast/cases/*.toml`.
- Sin cambios de esquema de Firestore; un solo dueño de escritura por colección: Rust solo lee en este plan.
- Mismos códigos HTTP y formato JSON de error que el legacy (spec §6). Ejemplo: `500 {"error":"Error al obtener logs"}`.
- Cuota de Firestore excedida: servir desde caché y registrar (spec §6).
- Nada ejecuta el legacy con el `.env` de producción ni toca Firestore de producción: las pruebas usan emulador o proyecto de pruebas (spec §7).
- Los secretos se leen del mismo `.env` y se validan al arrancar.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` y `cargo test --workspace` en verde antes de cada commit.
- Mensajes de commit en español, convencionales (`feat(data): ...`), con el trailer de coautoría que fije la sesión.

## Review Focus

1. Sesiones emitidas por Node con `ADMIN_JWT_SECRET` que contiene espacios al borde: Rust debe validarlas igual (hoy `Config` recorta el secreto y el legacy no). Test en Tarea 1.
2. Un documento de `guild_settings` borrado justo después de abrir el listener: no debe servirse hasta 6 h. Test en Tarea 2.
3. `role=admin` con `servidor` distinto del `guildId` pedido: el legacy responde 403, no datos de otro servidor. Test en Tarea 6.
4. `limit` no numérico, cero o negativo en `/logs`, `/orders`, `/claims`, `/matches/history`: el legacy aplica `parseInt(x) || default`; Rust debe dar el mismo conjunto. Test en Tarea 7.
5. Firestore no disponible o con cuota agotada en una lectura de administración: error JSON del legacy (500) o dato en caché, nunca texto interno ni pánico. Test en Tarea 7 y 8.

---

## File Structure

- `crates/common/src/config.rs`: secreto JWT sin recortar (Tarea 1).
- `crates/web/src/session.rs`: endurecimiento de verificación (Tarea 1).
- `crates/data/src/firestore.rs`: `Reset` en `Current` (Tarea 2) y `query` (Tarea 3).
- `crates/data/src/{logs,orders,support_calls,match_history,messages}.rs` y ampliación de `claims.rs`: repos de lectura simples (Tarea 4).
- `crates/data/src/{users,global_settings}.rs` y ampliación de `guild_settings.rs`: repos con caché (Tarea 5).
- `crates/web/src/routes/admin/{mod,auth,target_guild}.rs`: `requireAuth`, `resolveTargetGuildId` (Tarea 6); `routes/admin/{simple_reads,derived_reads}.rs` (Tareas 7 y 8).
- `crates/web/src/proxy.rs`: guarda de primer byte y reposo de WebSocket (Tarea 9).
- `contrast/cases/admin.toml`, `docs/endpoint-inventory.md`, `docs/runbook-corte.md`, `docs/riesgos.md`, `docs/comparativa-fase1.md`, `docs/baseline-node.md`.

---

### Task 0: Entorno de pruebas y medición real (puerta de decisión)

**Files:**
- Modify: `docs/baseline-node.md`, `docs/comparativa-fase1.md`
- Create: `docs/entorno-pruebas.md`

**Interfaces:**
- Consumes: `tools/baseline.mjs` (`--no-xff`, bloque `concurrent`), `crates/contrast` (CLI), `docker-compose.yml`.
- Produces: cifras reales de RAM en reposo y p50/p95/p99 de Node y de Rust; decisión escrita de si seguir.

Esta tarea necesita datos del usuario: token de Discord de **prueba**, proyecto de Firestore de **pruebas** o emulador (requiere Java), y un `ADMIN_JWT_SECRET` conocido. Sin ellos la tarea queda BLOCKED y el resto del plan puede empezar, pero el criterio de éxito de la spec sigue sin cumplirse.

- [ ] **Step 1: Documentar el entorno en `docs/entorno-pruebas.md`**: variables necesarias, cómo arrancar el legacy en `:3001` con `.env.test`, cómo arrancar Rust en `:15633` con `PROXY_TARGET` apuntando a él, y la regla «nunca el `.env` de producción».
- [ ] **Step 2: Medir Node directo** con `node tools/baseline.mjs --base http://127.0.0.1:3001 --no-xff` sobre `/api/ping`, `/api/products`, `/api/reclamaciones/<código de prueba>`; anotar además RAM del proceso Node en reposo (`Get-Process node | Select WS`).
- [ ] **Step 3: Medir Rust** con las mismas rutas contra `:15633` y la RAM del proceso `web`; ejecutar `contrast` con `contrast/cases/public.toml` y `auth.toml` contra ambos.
- [ ] **Step 4: Escribir las tablas** en `docs/baseline-node.md` y `docs/comparativa-fase1.md` y una sección «Decisión» con una recomendación: si la ganancia de p95 en rutas migradas es menor que el ruido, o la RAM total (Node + Rust) sube, dilo explícitamente; la ganancia de RAM real solo llega al retirar Node (fases 2C y 3).
- [ ] **Step 5: Commit** `docs: medición real de fase 1 y decisión`.

### Task 1: Endurecimiento de la sesión (pendientes de la fase 1)

**Files:**
- Modify: `crates/common/src/config.rs`, `crates/web/src/session.rs`
- Test: tests existentes de ambos módulos, ampliados

**Interfaces:**
- Consumes: `Config`, `SessionState`, `OptionalUser/AuthUser/AdminUser`.
- Produces: `Config.admin_jwt_secret_raw: String` (sin recortar; solo se rechaza si queda vacío tras recortar); `SessionState::new(secret: &str) -> Result<SessionState, SessionError>` que falla con secreto vacío.

- [ ] **Step 1: Tests que fallan** — `jwt_secret_with_edge_whitespace_verifies_token_signed_with_raw_secret` (token firmado con `" s3cr3t "`, verifica OK), `empty_secret_is_rejected_by_constructor`, `token_expiring_exactly_now_is_rejected` (frontera de `exp`), `token_without_iat_is_accepted_like_legacy` (el legacy solo exige firma y `exp`; confirmar contra `middleware/auth` del legacy y fijar el comportamiento), `verify_attempt_cap` (más de 16 candidatos de cookie no se prueban).
- [ ] **Step 2: Ejecutar y ver fallar:** `cargo test -p common -p web session`.
- [ ] **Step 3: Implementar** lectura del secreto sin recortar en `Config`, constructor con error, frontera de `exp`, tope de intentos de verificación y atributo `Expires` en la cookie de borrado igual que el legacy.
- [ ] **Step 4: Ejecutar:** `cargo test --workspace` en verde.
- [ ] **Step 5: Commit** `fix(web): secreto JWT sin recortar y verificación endurecida`.

### Task 2: Listener de Firestore sin ventana residual

**Files:**
- Modify: `crates/data/src/firestore.rs` (~320-330), `crates/data/src/guild_settings.rs`, `crates/data/src/cache.rs`
- Test: `crates/data/src/*` con el listener falso; emulador `#[ignore]` si hay Java

**Interfaces:**
- Consumes: `ChangeEvent::{Added,Modified,Removed,Reset}`, `TtlCache::purge`, `invalidate_all`.
- Produces: el stream emite `Reset` en el primer `TargetChangeType::Current` de cada apertura; `TtlCache` re-comprueba la época tras escribir en la caché de obsoletos.

- [ ] **Step 1: Tests que fallan** — `doc_deleted_before_initial_snapshot_is_not_served` (carga, borrado antes del `Current`, tras `Reset` la lectura recarga y devuelve `NotFound`), `stale_write_after_purge_is_dropped` (carrera determinista con puertas de sincronización).
- [ ] **Step 2: Ejecutar y ver fallar:** `cargo test -p data`.
- [ ] **Step 3: Implementar** emisión de `Reset` en `Current` (idempotente: puede llegar más de una vez) y re-comprobación de época tras `stale.insert`; corregir el comentario obsoleto en `guild_settings.rs`.
- [ ] **Step 4: Ejecutar:** `cargo test --workspace`.
- [ ] **Step 5: Commit** `fix(data): Reset al estar el listener actual y escritura obsoleta`.

### Task 3: Consultas con filtro, orden y límite

**Files:**
- Modify: `crates/data/src/firestore.rs`, `crates/data/src/error.rs`
- Test: `crates/data/src/firestore.rs` (módulo de pruebas) + emulador `#[ignore]`

**Interfaces:**
- Consumes: `FirestoreClient`.
- Produces: `FirestoreClient::query(&self, collection: &str, q: Query) -> Result<Vec<Doc>, DataError>` con `Query { filters: Vec<(String, Op, Value)>, order_by: Option<(String, Dir)>, limit: Option<u32> }`; la cuota agotada mapea a `DataError::QuotaExceeded`.

- [ ] **Step 1: Test que falla** — `query_builds_expected_structured_query` (sin red: construye la petición y compara filtro, orden y límite) y, en el emulador, `query_orders_and_limits`.
- [ ] **Step 2: Ejecutar y ver fallar.**
- [ ] **Step 3: Implementar** `query` sobre `firestore::FirestoreQueryParams`; sin índices compuestos nuevos: si una consulta del legacy los necesita, anotarlo en `docs/riesgos.md` y no crear índices desde Rust.
- [ ] **Step 4: Ejecutar** `cargo test -p data` y clippy.
- [ ] **Step 5: Commit** `feat(data): consultas con filtro, orden y límite`.

### Task 4: Repositorios de lectura simples

**Files:**
- Create: `crates/data/src/{logs,orders,support_calls,match_history,messages}.rs`; Modify: `claims.rs`, `lib.rs`
- Test: un módulo de pruebas por repositorio con un `FirestoreClient` falso

**Interfaces:**
- Consumes: `FirestoreClient::query` (Tarea 3).
- Produces (todas `async`, devuelven `Result<_, DataError>` y existe un doble `Fake*Repo` por trait):
  - `LogsRepo::recent(limit: u32, guild: Option<&str>) -> Vec<LogEntry>` (colección `faceit_bot_logs`)
  - `OrdersRepo::list(limit: u32, status: Option<&str>) -> Vec<Order>` (`orders`)
  - `SupportCallsRepo::list(limit: u32, guild: Option<&str>) -> Vec<SupportCall>` (`faceit_support_calls`)
  - `MatchHistoryRepo::list(limit: u32, guild: Option<&str>) -> Vec<MatchHistoryEntry>` (`faceit_matches_history`)
  - `MessagesRepo::templates() -> serde_json::Value` (`faceit_config`, documento de plantillas)
  - `ClaimsRepo::list(limit: u32, status: Option<&str>) -> Vec<Claim>` (ampliar el repo existente)

Tarea por lotes: las seis son lecturas de la misma forma; el comportamiento (orden, campo de filtro, forma del JSON, normalización de fechas) se deriva de `db.getRecentLogs`, `getOrders`, `getSupportCalls`, `getMatchHistory`, `getMessageTemplates`, `getClaims` en `D:\Discord_Faceit\database` y se registra en el informe.

- [ ] **Step 1: Un test por repositorio que falla**, p. ej. `logs_recent_filters_by_guild_and_limits`, `orders_list_filters_by_status`, `match_history_without_guild_mixes_sources_in_chronological_order`; más `list_propagates_quota_error`.
- [ ] **Step 2: Ejecutar y ver fallar:** `cargo test -p data`.
- [ ] **Step 3: Implementar** cada repo sin caché propia (la caché de 30 s de `/logs` vive en la ruta, Tarea 7).
- [ ] **Step 4: Ejecutar:** `cargo test --workspace`.
- [ ] **Step 5: Commit** `feat(data): repositorios de lectura de administración`.

### Task 5: Repos con caché: usuarios y ajustes globales

**Files:**
- Create: `crates/data/src/{users,global_settings}.rs`; Modify: `guild_settings.rs`, `crates/common/src/config.rs`
- Test: módulos de pruebas con fakes

**Interfaces:**
- Consumes: `TtlCache`, `FirestoreClient`, `GuildSettingsRepo`.
- Produces:
  - `UsersRepo::get(discord_id: &str) -> Option<FaceitUser>`; `UsersRepo::all() -> Vec<FaceitUser>`; `UsersRepo::by_nickname(nick: &str, guild: Option<&str>) -> Option<FaceitUser>` (`faceit_users`)
  - `GlobalSettingsRepo::get() -> GlobalSettings` (`faceit_config/settings`, con listener)
  - `effective_config_for_guild(global: &GlobalSettings, guild: Option<&GuildSettings>, env: &Config) -> EffectiveConfig` (puerto de `getEffectiveConfigForGuild`)
  - `is_default_guild(guild_id: &str, global: &GlobalSettings, env: &Config) -> bool` con la regla del legacy: `guild_id == (settings.discordGuildId || env DISCORD_GUILD_ID)` y esa id no vacía (`config/index.js` ~135 y ~192).

- [ ] **Step 1: Tests que fallan** — `is_default_guild_requires_non_empty_id`, `effective_config_prefers_guild_over_global_over_env`, `global_admin_role_layer_matches_legacy_chain` (alimenta a `is_admin`), `users_all_is_cached_and_invalidated_by_watcher`.
- [ ] **Step 2: Ejecutar y ver fallar.**
- [ ] **Step 3: Implementar**; `GlobalSettingsRepo` usa el mismo patrón de watcher con backoff 1→60 s que `GuildSettingsRepo`. `UsersRepo::all` con TTL corto porque lo lee el panel, y sin listener (colección grande).
- [ ] **Step 4: Ejecutar:** `cargo test --workspace`.
- [ ] **Step 5: Commit** `feat(data): repos de usuarios y ajustes globales con caché`.

### Task 6: `requireAuth`, `resolveTargetGuildId` y router de administración

**Files:**
- Create: `crates/web/src/routes/admin/{mod,auth,target_guild}.rs`; Modify: `crates/web/src/lib.rs` (montar antes del proxy), `contrast/cases/admin.toml`
- Test: `crates/web/tests/admin_auth.rs`

**Interfaces:**
- Consumes: `AuthUser/AdminUser`, `SessionState`, `is_admin`, `effective_config_for_guild`, repos de la Tarea 5.
- Produces: `AdminContext { user: SessionClaims }` extractor equivalente a `verifyAdminToken` (mismos 401 JSON del legacy, cookie `admin_token` y `auth_token`); `resolve_target_guild_id(ctx: &AdminContext, query: &Query) -> Result<Option<String>, AdminError>` con la regla `servidor` del legacy (403 si `role=admin` pide otro servidor); `admin::router(state) -> Router` que **no** define ninguna ruta no migrada.

- [ ] **Step 1: Tests que fallan** — `no_token_is_401_with_legacy_body`, `tampered_token_is_401`, `admin_role_with_other_servidor_is_403`, `global_admin_may_target_any_guild`, `unmigrated_admin_route_falls_through_to_proxy` (ver la ruta `POST /api/admin/orders/x/status` llegar al proxy falso).
- [ ] **Step 2: Ejecutar y ver fallar:** `cargo test -p web admin_auth`.
- [ ] **Step 3: Implementar** derivando mensajes y códigos de `verifyAdminToken` y `resolveTargetGuildId` en el legacy (`routes/api.js` ~21 y helpers); montar `SessionState` en el estado de la app por `FromRef`.
- [ ] **Step 4: Ejecutar:** `cargo test --workspace`.
- [ ] **Step 5: Commit** `feat(web): autenticación y servidor objetivo de administración`.

### Task 7: Lecturas de administración de una sola colección

**Files:**
- Create: `crates/web/src/routes/admin/simple_reads.rs`; Modify: `admin/mod.rs`, `contrast/cases/admin.toml`, `docs/endpoint-inventory.md`
- Test: `crates/web/tests/admin_simple_reads.rs`

**Interfaces:**
- Consumes: repos de la Tarea 4, `AdminContext` y `resolve_target_guild_id` (Tarea 6).
- Produces: rutas `GET /api/admin/{claims, claims/:code, orders, logs, support-calls, messages, matches/history}` con las formas `{claims}`, `{claim}` (404 como el legacy), `{orders}`, `{logs}`, `{calls}`, `{templates}`, `{history}`; `/logs` con caché de 30 s por clave `"{guild|global}:{limit}"` (`BOT_LOGS_TTL` del legacy, línea ~1596 de `routes/api.js`).

- [ ] **Step 1: Tests que fallan** — `limit_falls_back_like_parse_int_or_default` (valores `abc`, `0`, `-5`, `7x` frente a `parseInt(x)||default`), `logs_second_call_within_ttl_hits_cache`, `claims_filters_by_status`, `repo_error_maps_to_legacy_500_body` (cada ruta con su mensaje exacto, p. ej. `Error al obtener logs`), `quota_error_does_not_leak_text`.
- [ ] **Step 2: Ejecutar y ver fallar.**
- [ ] **Step 3: Implementar** cada handler; añadir los casos a `contrast/cases/admin.toml` con la sesión de prueba y marcar las filas del inventario como `migrado (Tarea 7)`.
- [ ] **Step 4: Ejecutar:** `cargo test --workspace`.
- [ ] **Step 5: Commit** `feat(web): lecturas de administración de una colección`.

### Task 8: Lecturas de administración derivadas

**Files:**
- Create: `crates/web/src/routes/admin/derived_reads.rs`; Modify: `admin/mod.rs`, `contrast/cases/admin.toml`, `docs/endpoint-inventory.md`
- Test: `crates/web/tests/admin_derived_reads.rs`

**Interfaces:**
- Consumes: `UsersRepo`, `GlobalSettingsRepo`, `effective_config_for_guild`, repos de la Tarea 4.
- Produces: `GET /api/admin/{stats, stats/:nickname, users/backup, settings, matches/:id/demo, yape/transactions}` con el cuerpo y la caché (yape: 30 s con deduplicación de peticiones simultáneas, `routes/api.js` ~1989) que dicta el legacy. Quedan **fuera** `/users`, `/matches`, `/discord/roles`, `/status`, `/guilds`, `/system`, `/leaderboard`, `/faceit-club-queues`, `/matches/:id/stats`, `/firestore-metrics` (Discord, Faceit API o estado de Node; fase 2B).

- [ ] **Step 1: Tests que fallan** — `stats_counts_match_fixture`, `settings_merges_global_and_guild_like_legacy`, `users_backup_is_admin_only` (403 con el mensaje del legacy para quien no es `global_admin`, si el legacy lo exige: confirmarlo en la línea ~670), `yape_transactions_deduplicates_concurrent_requests`, `demo_for_missing_match_is_legacy_404`.
- [ ] **Step 2: Ejecutar y ver fallar.**
- [ ] **Step 3: Implementar** derivando de los handlers citados; si alguno resulta depender de estado en memoria de Node o de Discord, no migrarlo: dejarlo en el proxy, corregir el inventario y registrar un `Ruling:` en el ledger.
- [ ] **Step 4: Ejecutar:** `cargo test --workspace`.
- [ ] **Step 5: Commit** `feat(web): lecturas de administración derivadas`.

### Task 9: Protección del proxy y cierre documental

**Files:**
- Modify: `crates/web/src/proxy.rs`, `crates/common/src/config.rs`, `docs/runbook-corte.md`, `docs/riesgos.md`, `docs/endpoint-inventory.md`, `README.md`
- Test: `crates/web/tests/proxy_timeouts.rs`

**Interfaces:**
- Consumes: `ProxyOptions`.
- Produces: `ProxyOptions { first_byte_timeout: Option<Duration>, ws_idle_timeout: Option<Duration> }`; el tiempo hasta el primer byte aplica a rutas no SSE (504 JSON `{"error":"Servicio no disponible"}`; las rutas SSE `/api/halloween/stream` y de larga espera quedan exentas); `WS_IDLE_TIMEOUT_SECS` y `PROXY_FIRST_BYTE_TIMEOUT_SECS` en `Config` (desactivados por defecto = comportamiento actual).

- [ ] **Step 1: Tests que fallan** — `slow_upstream_without_first_byte_gets_504`, `sse_route_is_exempt_from_first_byte_timeout`, `idle_websocket_is_closed_after_timeout`, `active_websocket_is_not_closed`.
- [ ] **Step 2: Ejecutar y ver fallar:** `cargo test -p web proxy_timeouts`.
- [ ] **Step 3: Implementar** y actualizar runbook (nuevas variables, rutas migradas, rollback por ruta), inventario y `docs/riesgos.md`.
- [ ] **Step 4: Ejecutar** la suite completa, clippy y fmt.
- [ ] **Step 5: Commit** `feat(web): guardas de tiempo del proxy y documentación de fase 2A`.

---

## Fuera de este plan (planes siguientes, a redactar tras medir en la Tarea 0)

- **2B, lecturas con dependencias externas:** cliente de la API de Faceit; REST de Discord para `/users`, `/matches`, `/discord/roles`, `/guilds`, `/status`; `/system`, `/leaderboard`, `/faceit-club-queues`, `/matches/:id/stats`; callback de OAuth (escribe `faceit_users` y consulta capitán).
- **2C, lobby y escrituras:** `routes/lobby.js` (5497 líneas, 48 rutas, estado `LOBBY_STATE` y WebSocket con un único dueño), `cs2Api`, los POST/PUT/DELETE de administración, Halloween (SSE), webhooks de Yape y MatchZy. El webhook de Faceit (`webhooks/faceit.js`, 1426 líneas) y los avisos de Yape usan `client.channels`, `guild.members` y voz del gateway: no pueden salir de Node hasta que Rust tenga su propia capa de Discord (REST completo) o hasta la fase 3 (bot). Es el riesgo principal de la fase 2 y debe resolverse en el plan 2C antes de tocar estas rutas.
- **Fase 3:** bot de Discord. La RAM solo baja de verdad cuando Node deja de correr; mientras el lobby y el bot sigan en Node, la ganancia de la fase 2A es de latencia en rutas migradas, no de memoria.
