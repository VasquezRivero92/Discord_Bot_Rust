# Migración a Rust, fases 0 y 1: plan de implementación

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dejar a Rust (axum) como puerta de entrada de `machupeek.com`, sirviendo la web y las rutas ya migradas y reenviando el resto al Node legacy, con RAM y latencia medidas contra una línea base.

**Architecture:** Un workspace Rust con tres crates (`common`, `data`, `web`) más un crate de pruebas (`contrast`). `web` es un servidor axum que atiende las rutas migradas y hace proxy inverso (HTTP y WebSocket) a Node para las demás. `data` encapsula Firestore y caché detrás de repositorios. Un arnés de contraste compara respuestas Node vs Rust por endpoint.

**Tech Stack:** Rust 2021, axum, tokio, tower-http, governor, moka, jsonwebtoken, reqwest, firestore (firestore-rs), thiserror, anyhow, tracing. Node 20 solo para herramientas (`tools/*.mjs`) y como sistema legacy.

**Spec:** `docs/superpowers/specs/2026-11-02-migracion-rust-design.md`

Este plan cubre las fases 0 y 1 de la spec. Las fases 2 (lobby, webhooks, servicios) y 3 (bot de Discord) tendrán planes propios.

## Global Constraints

- Rust escucha en `:15633`; Node legacy en `:3001` (configurable con `LEGACY_URL`, por defecto `http://127.0.0.1:3001`). Steam bot en `:15634`, no se toca.
- `D:\Discord_Faceit` es solo lectura para este proyecto: ninguna tarea lo modifica.
- Sin cambios de esquema en Firestore; `firestore.rules` e índices no se tocan.
- Un solo dueño de escritura por colección: en las fases 0 y 1 todas las escrituras siguen en Node; Rust solo lee. Las rutas que escriben se reenvían a Node.
- Sesiones: mismo `ADMIN_JWT_SECRET`, HS256, expiración 8 h, cookies `auth_token` y `admin_token` (ambas se escriben; se lee `auth_token` primero y `admin_token` como respaldo), `httpOnly`, `sameSite=lax`, `secure` si `NODE_ENV=production` o `BASE_URL` empieza por `https`, `maxAge` 8 h, `path=/`.
- Claims del JWT: `userId`, `username`, `avatar`, `isAdmin`, `role`, `servidor`. Es admin si `role` es `global_admin` o `admin`, o `isAdmin == true`.
- Mensajes de error idénticos a Node: 401 `{"error":"No autenticado"}` sin cookie, 401 `{"error":"Token inválido o expirado"}` con token malo, 403 `{"error":"No autorizado. Se requieren permisos de administrador."}`, 429 `{"error":"Demasiadas peticiones. Intenta de nuevo más tarde."}`.
- Rate limit global: 100 peticiones por minuto por IP. Límite de cuerpo JSON: 64 kb. Cabecera `Permissions-Policy: camera=(), microphone=(), geolocation=()`.
- Caché de estáticos: `.html` con `Cache-Control: no-cache, no-store, must-revalidate`; rutas bajo `assets` con `public, max-age=31536000, immutable`.
- Errores: `thiserror` en cada crate, `anyhow` solo en `main`.
- Las pruebas nunca apuntan a Firestore de producción: usan el emulador (`FIRESTORE_EMULATOR_HOST`) o un proyecto de pruebas.
- Los secretos se leen del `.env` y nunca se commitean (`.gitignore` los excluye desde la Tarea 1).

## Review Focus

- Cookie con `auth_token` caducado y `admin_token` válido (o al revés): debe autenticar con el que sea válido, sin 500.
- JWT corrupto, vacío, con firma de otro secreto o algoritmo `none`: siempre 401 con el mensaje fijado, nunca panic.
- Node caído o reiniciándose durante una petición o un upgrade de WebSocket: 502 rápido, sin colgar y sin tumbar a Rust.
- Detrás de Cloudflare todas las peticiones llegan de pocas IP: el rate limit debe usar `CF-Connecting-IP` / `X-Forwarded-For` (un salto de proxy de confianza) y no agrupar a todos los usuarios en un solo cubo.
- Listener de Firestore caído o con retraso: el caché no puede servir datos más allá de su TTL; al reconectar se invalida.

---

### Task 1: Workspace y herramientas base

**Files:**
- Create: `Cargo.toml` (workspace), `crates/common/Cargo.toml`, `crates/common/src/lib.rs`, `crates/data/Cargo.toml`, `crates/data/src/lib.rs`, `crates/web/Cargo.toml`, `crates/web/src/main.rs`, `crates/web/src/lib.rs`
- Create: `.gitignore`, `.env.example`, `rust-toolchain.toml`, `README.md`

**Interfaces:**
- Produces: workspace con `cargo test --workspace`, `cargo clippy --workspace -- -D warnings` y `cargo fmt --check` funcionando; `crates/web` expone `pub fn app() -> axum::Router` en `lib.rs` (por ahora responde 404).

- [ ] **Step 1: Write the failing test** en `crates/web/tests/smoke.rs`: `unknown_route_returns_404` hace `tower::ServiceExt::oneshot` de `GET /nada` contra `web::app()` y afirma estado 404.
- [ ] **Step 2: Run it to verify it fails**: `cargo test -p web --test smoke` → error de compilación (`web::app` no existe).
- [ ] **Step 3: Implement** el workspace y `pub fn app() -> Router` (router vacío). `.gitignore` excluye `target/`, `.env`, los `*.json` de service accounts y `cookies.txt`. `.env.example` lista las variables de la spec sin valores. `README.md` explica cómo ejecutar tests y de qué trata el repo en 10 líneas.
- [ ] **Step 4: Run** `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` → todo pasa.
- [ ] **Step 5: Commit** `chore: workspace rust con crates common, data y web`.

### Task 2: Configuración tipada

**Files:**
- Create: `crates/common/src/config.rs`; Modify: `crates/common/src/lib.rs`
- Test: `crates/common/tests/config.rs`

**Interfaces:**
- Produces: `pub struct Config { pub port: u16, pub legacy_url: String, pub steam_bot_url: String, pub static_dir: PathBuf, pub admin_jwt_secret: String, pub base_url: String, pub discord_client_id: String, pub discord_client_secret: String, pub discord_token: String, pub discord_guild_id: String, pub cors_origins: Vec<String>, pub production: bool }` y `pub fn Config::from_map(vars: &HashMap<String,String>) -> Result<Config, ConfigError>`; `Config::from_env()` la envuelve con `dotenvy`.
- `ConfigError::Missing(&'static str)` para cada variable obligatoria.

- [ ] **Step 1: Write the failing tests**: `missing_jwt_secret_is_error` (falta `ADMIN_JWT_SECRET` → `ConfigError::Missing("ADMIN_JWT_SECRET")`); `defaults_apply` (`port == 15633`, `legacy_url == "http://127.0.0.1:3001"`); `production_true_when_node_env_production_or_https_base_url`; `cors_origins_split_and_trimmed` (`"https://a.com, https://b.com"` → 2 elementos); `cors_defaults_to_base_url_origin_in_production` (reproduce `index.js:132-135`).
- [ ] **Step 2: Run** `cargo test -p common` → FAIL.
- [ ] **Step 3: Implement** `Config` en `config.rs` según la firma; `CORS_ORIGINS` con respaldo `ADMIN_ALLOWED_ORIGINS`, como `index.js:128-130`.
- [ ] **Step 4: Run** `cargo test -p common` → PASS.
- [ ] **Step 5: Commit** `feat(common): configuración tipada desde entorno`.

### Task 3: Inventario de endpoints

**Files:**
- Create: `tools/inventory.mjs`, `tools/inventory.test.mjs`, `docs/endpoint-inventory.md` (generado)

**Interfaces:**
- Produces: `export function parseRoutes(source: string, mountPrefix: string): Array<{method, path, auth, line}>` y CLI `node tools/inventory.mjs D:\Discord_Faceit` que escribe `docs/endpoint-inventory.md`.

- [ ] **Step 1: Write the failing test** (`node --test tools/inventory.test.mjs`): `parseRoutes("router.get('/me', requireAuth, async (req,res)=>{})", "/api/admin")` devuelve `[{method:"GET", path:"/api/admin/me", auth:"requireAuth", line:1}]`; otro test cubre `router.post(['/demo-login','/login'], ...)` → dos entradas; otro, rutas `app.get('/health', ...)` sin auth → `auth: "none"`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** con regex sobre `router.<verbo>(` y `app.<verbo>(`, aplicando los prefijos de montaje de `index.js:391-456` (`/auth`, `/api/admin`, `/api/lobby`, `/api/cs2`, `/api`). Incluir las rutas de `index.js` y de los cinco archivos de `routes/`.
- [ ] **Step 4: Run** los tests → PASS; ejecutar el CLI. El documento debe listar el recuento por archivo (hoy: `api.js` 56, `auth.js` 6, `cs2Api.js` 4, `lobby.js` 48, `publicApi.js` 4) y las rutas propias de `index.js`.
- [ ] **Step 5: Edit** `docs/endpoint-inventory.md` añadiendo por fila tres columnas manuales: `colecciones`, `usa Discord` (REST / gateway / no), `fase` (1/2/3/proxy). Rellenar al menos las rutas de las Tareas 12-16.
- [ ] **Step 6: Commit** `docs: inventario de endpoints del legacy`.

### Task 4: Línea base de rendimiento del Node actual

**Files:**
- Create: `tools/baseline.mjs`, `tools/baseline.test.mjs`, `docs/baseline-node.json`, `docs/baseline-node.md`

**Interfaces:**
- Produces: `node tools/baseline.mjs --base http://127.0.0.1:3001 --pid <pid> --out docs/baseline-node.json` mide RAM del proceso (RSS en reposo) y, por endpoint, p50/p95/p99 en ms con 200 peticiones secuenciales y luego 50 concurrentes. Salida JSON `{ rssMb, endpoints: { "<METHOD> <path>": { p50, p95, p99, errors } } }`. Exporta `percentile(values: number[], p: number): number`.

- [ ] **Step 1: Write the failing test**: `percentile([1,2,3,4,5,6,7,8,9,10], 95) === 10` y `percentile([5], 50) === 5`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `percentile` y el medidor con `fetch`. Endpoints: `GET /api/ping`, `GET /health`, `GET /api/products`, `GET /api/lobby/me` (sin cookie), `GET /api/admin/me`, `GET /api/admin/guilds/<id>/settings` y `GET /api/admin/map-catalog` (con la cookie de un JWT de prueba firmado con un secreto de prueba).
- [ ] **Step 4: Ejecutar** contra una instancia local de Node apuntando a Firestore de pruebas o emulador (`npm start` en `D:\Discord_Faceit` con un `.env` de pruebas; no se modifica ese repo). Guardar `docs/baseline-node.json` y resumir en `docs/baseline-node.md` con fecha y máquina.
- [ ] **Step 5: Commit** `docs: línea base de rendimiento del Node legacy`.

### Task 5: Validación de Firestore desde Rust (riesgo principal)

**Files:**
- Create: `crates/data/src/firestore.rs`, `crates/data/src/error.rs`; Modify: `crates/data/src/lib.rs`
- Test: `crates/data/tests/firestore_emulator.rs` (marcado `#[ignore]`, requiere emulador)

**Interfaces:**
- Produces: `pub struct FirestoreClient`, `FirestoreClient::connect(project_id: &str, credentials: Option<&Path>) -> Result<Self, DataError>`, `get_doc<T: DeserializeOwned>(&self, collection: &str, id: &str) -> Result<Option<T>, DataError>`, `list_docs<T>(&self, collection: &str) -> Result<Vec<(String, T)>, DataError>`, `watch_collection(&self, collection: &str) -> impl Stream<Item = Result<ChangeEvent, DataError>>` con `ChangeEvent::{Added(String), Modified(String), Removed(String)}` (id del documento).
- `DataError` con variantes `Firestore(String)`, `QuotaExceeded`, `NotFound`, `Decode(String)`.

- [ ] **Step 1: Write the failing tests** (con emulador): `get_doc_returns_none_for_missing`; `get_doc_roundtrip` (insertar con la REST del emulador y leer); `watch_emits_modified_after_update` (actualizar un documento y recibir `Modified(id)` en menos de 5 s); `quota_error_maps_to_quota_exceeded` (unitario, con un `tonic::Status::resource_exhausted`).
- [ ] **Step 2: Run** `FIRESTORE_EMULATOR_HOST=localhost:8080 cargo test -p data -- --ignored` → FAIL.
- [ ] **Step 3: Implement** sobre el crate `firestore`; mapear `resource_exhausted` a `QuotaExceeded`. Si el crate no soporta listeners de colección contra el emulador, documentar el hallazgo en `docs/riesgos.md` y proponer en él un cliente alternativo antes de seguir con la Tarea 11 (es el punto de decisión previsto en la spec).
- [ ] **Step 4: Run** → PASS (o hallazgo documentado).
- [ ] **Step 5: Commit** `feat(data): cliente firestore con get, list y watch`.

### Task 6: Proxy inverso hacia Node (HTTP y WebSocket)

**Files:**
- Create: `crates/web/src/proxy.rs`; Modify: `crates/web/src/lib.rs`
- Test: `crates/web/tests/proxy.rs`

**Interfaces:**
- Consumes: `Config` (Tarea 2).
- Produces: `pub fn fallback_proxy(legacy_url: String) -> impl Handler` usado como `Router::fallback`; reenvía método, ruta, query, cabeceras (añadiendo `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`) y cuerpo; soporta `Upgrade: websocket` con túnel bidireccional.

- [ ] **Step 1: Write the failing tests** con un Node simulado (axum en puerto efímero): `proxies_get_with_query_and_headers`; `proxies_post_body_and_status`; `preserves_set_cookie_headers`; `returns_502_json_when_legacy_down` (cuerpo `{"error":"Servicio no disponible"}` en menos de 2 s); `proxies_websocket_echo` (mensaje de ida y vuelta); `websocket_upgrade_returns_502_when_legacy_down`.
- [ ] **Step 2: Run** `cargo test -p web --test proxy` → FAIL.
- [ ] **Step 3: Implement** con `reqwest` (sin seguir redirects, para que los `3xx` de Node lleguen intactos) y `tokio-tungstenite` para el túnel WebSocket. Timeout de conexión 2 s.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(web): proxy inverso http y websocket hacia el legacy`.

### Task 7: Middleware de seguridad, CORS y rate limit

**Files:**
- Create: `crates/web/src/middleware.rs`; Modify: `crates/web/src/lib.rs`
- Test: `crates/web/tests/middleware.rs`

**Interfaces:**
- Produces: `pub fn apply(router: Router, cfg: &Config) -> Router` que añade: cabeceras tipo helmet (sin CSP) más `Permissions-Policy`, redirect 301 a HTTPS cuando `production` y `X-Forwarded-Proto != https`, CORS con `cors_origins` y credenciales, rate limit 100/min por IP, límite de cuerpo 64 kb (413 al excederlo).

- [ ] **Step 1: Write the failing tests**: `sets_permissions_policy_header`; `redirects_http_to_https_in_production` (301 a `https://<host><url>`); `no_redirect_when_forwarded_proto_https`; `cors_rejects_unlisted_origin`; `cors_allows_listed_origin_with_credentials`; `rate_limit_returns_429_with_spanish_message_after_100`; `rate_limit_keys_on_cf_connecting_ip` (dos IP distintas por `CF-Connecting-IP` tienen cubos independientes); `body_over_64kb_returns_413`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** con `tower-http` (`SetResponseHeader`, `CorsLayer`, `RequestBodyLimit`) y `governor` con extractor de clave que lee `CF-Connecting-IP`, luego `X-Forwarded-For`, luego la IP del socket.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(web): seguridad, cors y rate limit equivalentes a node`.

### Task 8: Estáticos y rutas públicas de la SPA

**Files:**
- Create: `crates/web/src/static_files.rs`; Modify: `crates/web/src/lib.rs`
- Test: `crates/web/tests/static_files.rs` con un `static_dir` temporal

**Interfaces:**
- Consumes: `Config.static_dir`.
- Produces: `pub fn routes(static_dir: PathBuf) -> Router` con: archivos de `static_dir`; `/admin` y `/admin/*` sirven `admin/index.html`; `/terms`, `/terms-of-service`, `/legal/terms` → `terms.html`; `/privacy`, `/privacy-policy`, `/legal/privacy` → `privacy.html`; `/support`, `/help`, `/legal/support` → `support.html`; las rutas SPA públicas y los alias `/halloween*` replican `index.js:458-527` (leer ese rango y listar las rutas exactas en el test).

- [ ] **Step 1: Write the failing tests**: `html_has_no_store_cache_control`; `assets_have_immutable_cache_control`; `admin_subpath_serves_admin_index`; `terms_aliases_serve_terms_html`; `halloween_case_variants_and_trailing_slash` (`/halloween`, `/Halloween/`, `/halloween/admin/`); `unknown_api_path_falls_to_proxy_not_spa` (una ruta `/api/x` no devuelve `index.html`).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** con `tower-http::services::ServeDir`/`ServeFile` y cabeceras por tipo. El `GET *` final de `index.js:577` decide qué rutas no-API caen en la SPA; replicarlo exactamente.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(web): estáticos y rutas públicas de la spa`.

### Task 9: Sesión compatible con Node (JWT y cookies)

**Files:**
- Create: `crates/web/src/session.rs`, `tools/gen-jwt-vectors.mjs`, `crates/web/tests/fixtures/jwt_vectors.json`
- Test: `crates/web/tests/session.rs`

**Interfaces:**
- Produces: `pub struct SessionClaims { pub user_id: String (rename "userId"), pub username: String, pub avatar: Option<String>, pub is_admin: bool (rename "isAdmin"), pub role: String, pub servidor: Option<String>, pub exp: u64 }`; extractores axum `OptionalUser(Option<SessionClaims>)`, `AuthUser(SessionClaims)` (401 si falta o es inválido) y `AdminUser(SessionClaims)` (403 si no es admin); `pub fn issue_cookies(claims: &SessionClaims, cfg: &Config) -> [Cookie; 2]` y `pub fn clear_cookies() -> [Cookie; 2]`.
- Los errores usan los mensajes fijados en Global Constraints.

- [ ] **Step 1: Write the failing tests**: vectores generados por `tools/gen-jwt-vectors.mjs` con `jsonwebtoken` de Node (token válido admin, válido no-admin, caducado, secreto distinto, `alg:none`, basura): `node_token_validates_in_rust`; `rust_token_validates_in_node` (el test emite un token con Rust y lo verifica ejecutando `node -e` con `jsonwebtoken` desde `D:\Discord_Faceit\node_modules`); `auth_token_expired_but_admin_token_valid_authenticates`; `admin_token_only_works`; `garbage_token_is_401_never_panics`; `alg_none_is_rejected`; `non_admin_gets_403_on_admin_routes`; `cookies_have_expected_flags` (httpOnly, SameSite=Lax, Max-Age=28800, Path=/, Secure según `production`).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** con `jsonwebtoken` fijando `Validation::new(Algorithm::HS256)`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(web): sesiones jwt y cookies compatibles con node`.

### Task 10: Arnés de contraste Node vs Rust

**Files:**
- Create: `crates/contrast/Cargo.toml`, `crates/contrast/src/lib.rs`, `crates/contrast/tests/contrast_self.rs`, `contrast/cases/*.toml`

**Interfaces:**
- Produces: `pub struct Case { name, method, path, headers, body, cookie_jwt: Option<Claims>, ignore_fields: Vec<String> }`, `pub async fn run_case(node: &str, rust: &str, case: &Case) -> Result<(), Diff>`; compara estado, cuerpo JSON normalizado (se ignoran campos de `ignore_fields`, p. ej. `timestamp`) y cabeceras `content-type`, `cache-control`, `set-cookie` (solo nombre y atributos). CLI: `cargo run -p contrast -- --node http://127.0.0.1:3001 --rust http://127.0.0.1:15633 contrast/cases`.

- [ ] **Step 1: Write the failing tests** contra dos servidores axum de juguete: `identical_responses_pass`; `status_mismatch_reports_diff`; `ignored_field_is_not_compared`; `extra_json_key_reports_diff`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** el comparador y el cargador de casos TOML.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `test: arnés de contraste node vs rust`.

### Task 11: Capa de caché y repositorio de ajustes por servidor

**Files:**
- Create: `crates/data/src/cache.rs`, `crates/data/src/guild_settings.rs`; Modify: `crates/data/src/lib.rs`
- Test: `crates/data/tests/guild_settings.rs`

**Interfaces:**
- Consumes: `FirestoreClient::{get_doc, watch_collection}` (Tarea 5).
- Produces: `pub struct TtlCache<K, V>` (sobre `moka`) con `get_or_load`, `invalidate`, `invalidate_all`; `#[async_trait] pub trait GuildSettingsRepo { async fn get(&self, guild_id: &str) -> Result<Option<GuildSettings>, DataError>; }`; `FirestoreGuildSettingsRepo::new(client, ttl: Duration)` con `spawn_watcher(self: Arc<Self>)` que invalida la clave al recibir `Modified`/`Removed` y todo el caché al reconectar; `InMemoryGuildSettingsRepo` para pruebas. TTL de `guild_settings`: 6 horas (como `GUILD_SETTINGS_TTL`).
- `GuildSettings` modela los campos que lee `GET /api/admin/guilds/:guildId/settings` (`routes/api.js:2063-2078`).

- [ ] **Step 1: Write the failing tests**: `second_get_hits_cache_not_firestore` (contador de lecturas); `watch_modified_invalidates_key`; `ttl_expiry_reloads` (TTL corto de prueba); `quota_exceeded_serves_stale_value_if_present`; `watcher_reconnect_invalidates_all` (simulando caída del stream).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `TtlCache` y el repo; ante `QuotaExceeded` devolver el último valor cacheado y registrar un warning, como `database.js:2006`.
- [ ] **Step 4: Run** → PASS (los de emulador con `--ignored`).
- [ ] **Step 5: Commit** `feat(data): caché ttl y repo de ajustes por servidor con watcher`.

### Task 12: Rutas de diagnóstico y API pública de solo lectura

**Files:**
- Create: `crates/web/src/routes/mod.rs`, `crates/web/src/routes/diagnostic.rs`, `crates/web/src/routes/public_api.rs`, `crates/data/src/claims.rs`
- Test: `crates/web/tests/public_api.rs`; Create: `contrast/cases/public.toml`

**Interfaces:**
- Produces: `GET /api/ping` (JSON `ok`, `cookies`, `hasAdminToken`, `secure`, `protocol`, `timestamp`, como `index.js:399-407`); `GET /health` (mismo cuerpo que `index.js:280-289`); `GET /api/products` (catálogo `STORE_PRODUCTS` de `routes/publicApi.js`, copiado a `crates/web/src/routes/products.json` sin cambios); `GET /api/reclamaciones/:code` vía `ClaimsRepo::get_by_code(&self, code: &str) -> Result<Option<Claim>, DataError>`.
- Los POST `/api/reclamaciones` y `/api/checkout/purchase` quedan en el proxy (escriben `claims` y `orders`).

- [ ] **Step 1: Write the failing tests**: `ping_lists_cookie_names_and_has_admin_token_flag`; `products_matches_node_catalog` (compara con el JSON copiado); `claim_by_code_returns_404_json_for_unknown` (mismo mensaje que `routes/publicApi.js:209-223`); `claim_by_code_returns_public_fields_only`; `post_reclamaciones_is_proxied` (el Node simulado recibe la petición).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** los handlers y `ClaimsRepo` (lectura de la colección `claims`).
- [ ] **Step 4: Run** → PASS; luego `cargo run -p contrast -- … contrast/cases/public.toml` con ambos servidores levantados → sin diferencias.
- [ ] **Step 5: Commit** `feat(web): diagnóstico y api pública de solo lectura`.

### Task 13: Autenticación con Discord

**Files:**
- Create: `crates/web/src/routes/auth.rs`, `crates/web/src/discord.rs`
- Test: `crates/web/tests/auth.rs`; Create: `contrast/cases/auth.toml`

**Interfaces:**
- Consumes: `SessionClaims`, `issue_cookies`, `clear_cookies` (Tarea 9); `GuildSettingsRepo` (Tarea 11).
- Produces: `#[async_trait] pub trait DiscordApi { async fn exchange_code(&self, code: &str) -> Result<DiscordUser, DiscordError>; async fn guild_member(&self, guild_id: &str, user_id: &str) -> Result<Option<GuildMember>, DiscordError>; async fn guild_roles(&self, guild_id: &str) -> Result<Vec<Role>, DiscordError>; }` con `RestDiscordApi` (API v10, `User-Agent` explícito para evitar el bloqueo 1015 que corrigió el último commit del legacy) y un `FakeDiscordApi` para pruebas; `pub async fn is_admin(member: &GuildMember, roles: &[Role], settings: &GuildSettings, env_admin_role_id: Option<&str>) -> bool` que reproduce `services/adminCheck.js` (permiso Administrator, o rol por ID o por nombre sin distinguir mayúsculas).
- Rutas: `GET /auth/discord` (redirección con `scope=identify`), `GET /auth/discord/callback`, `GET /auth/logout`. `/auth/steam*` y `/auth/demo-login` siguen en el proxy.

- [ ] **Step 1: Write the failing tests** con `FakeDiscordApi`: `is_admin_true_with_administrator_permission`; `is_admin_true_with_role_id_match`; `is_admin_true_with_role_name_case_insensitive`; `is_admin_false_without_role`; `callback_without_code_redirects_with_error` y los demás caminos de `routes/auth.js:48-222` (leerlo entero y escribir un test por rama: usuario sin permiso → HTML 403 «Acceso Restringido», sesión existente válida, errores de Discord); `callback_sets_both_cookies_for_authorized_user`; `logout_clears_both_cookies_and_redirects`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement**. Cálculo de `role`, `servidor` e `isAdmin` idéntico al de `routes/auth.js`; la página HTML de acceso restringido se copia literal de `routes/auth.js` (bloque `Acceso Restringido`).
- [ ] **Step 4: Run** → PASS; contraste de `/auth/discord` y `/auth/logout` (el callback se verifica manualmente con un login real en un servidor de pruebas).
- [ ] **Step 5: Commit** `feat(web): login con discord, logout y comprobación de admin`.

### Task 14: Lote 1 de la API de administración (lecturas de ajustes)

**Files:**
- Create: `crates/web/src/routes/admin/mod.rs`, `crates/web/src/routes/admin/settings.rs`
- Test: `crates/web/tests/admin_batch1.rs`; Create: `contrast/cases/admin_batch1.toml`

**Interfaces:**
- Consumes: `AdminUser` (Tarea 9), `GuildSettingsRepo` (Tarea 11), `RestDiscordApi` (Tarea 13).
- Produces: `GET /api/admin/me` (`routes/api.js:135`), `GET /api/admin/guilds/:guildId/settings` (`:2063`), `GET /api/admin/map-catalog` (`:2124`), `GET /api/admin/guilds` (`:1851`). Las escrituras homónimas (`POST …/settings`, `POST /map-catalog`) siguen en el proxy.
- `requireAuth` de `routes/api.js` determina si cada ruta usa `AuthUser` o `AdminUser`; leerlo antes de elegir.

- [ ] **Step 1: Write the failing tests** por ruta: sin cookie → 401 con el mensaje fijado; no admin → 403; admin → cuerpo igual al de Node en un caso con datos de `InMemoryGuildSettingsRepo`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** los cuatro handlers copiando el formato JSON de Node.
- [ ] **Step 4: Run** → PASS; contraste con ambos servidores contra el mismo Firestore de pruebas → sin diferencias.
- [ ] **Step 5: Commit** `feat(web): lote 1 de la api admin (lecturas de ajustes)`.

### Task 15: Resto de la API de administración, por lotes

**Files:**
- Create: un módulo por lote en `crates/web/src/routes/admin/`, un repositorio por colección en `crates/data/src/`, un archivo `contrast/cases/admin_batch<N>.toml` por lote.

**Interfaces:**
- Consumes: todo lo anterior.
- Produces: las rutas de `routes/api.js` migradas, en este orden, un commit por lote y solo después de que el contraste del lote pase:
  - **Lote 2, usuarios y estadísticas (lecturas):** `GET /stats` (`:336`), `GET /users` (`:417`), `GET /users/backup` (`:670`), `GET /stats/:nickname` (`:1178`), `GET /leaderboard` (`:1107`). Repositorio `UsersRepo` sobre `faceit_users` con caché de usuarios de 5 min y watcher.
  - **Lote 3, partidas (lecturas):** `GET /matches` (`:942`), `GET /matches/history` (`:1486`), `GET /matches/:id/demo` (`:1521`), `GET /matches/:id/stats` (`:1552`). Repositorios `ActiveMatchesRepo` (watcher sobre `faceit_active_matches`) y `MatchHistoryRepo` (TTL 5 min; estadísticas 15 min).
  - **Lote 4, soporte y comercio (lecturas):** `GET /support-calls` (`:1731`), `GET /claims` y `GET /claims/:code` (`:1880`, `:1892`), `GET /orders` (`:1956`), `GET /yape/transactions` (`:1989`), `GET /logs` (`:1596`), `GET /messages` (`:1825`).
  - **Lote 5, sistema:** `GET /system` (`:206`), `GET /status` (`:1641`), `GET /settings` (`:1225`), `GET /discord/roles` (`:869`), `GET /faceit-club-queues` (`:229`), `GET /firestore-metrics` (`:326`; reportar las métricas propias de Rust).
  - **Escrituras:** se quedan en el proxy durante la fase 1. Cada una se moverá junto con la transferencia de propiedad de su colección, que se decide en el plan de la fase 2.
- Receta por ruta, aplicada dentro de cada lote: leer la ruta en Node; escribir el test (401/403/éxito/error con el cuerpo exacto de Node); implementar; añadir el caso de contraste; actualizar la fila en `docs/endpoint-inventory.md` (`fase = 1, migrado`).

- [ ] **Step 1:** Para cada lote, escribir los tests de todas sus rutas y verificar que fallan.
- [ ] **Step 2:** Implementar los repositorios y handlers del lote.
- [ ] **Step 3:** `cargo test --workspace` y `cargo run -p contrast -- … contrast/cases/admin_batch<N>.toml` → sin diferencias.
- [ ] **Step 4:** Commit `feat(web): lote <N> de la api admin`, uno por lote.

### Task 16: API de CS2 de solo lectura

**Files:**
- Create: `crates/web/src/routes/cs2.rs`; Test: `crates/web/tests/cs2.rs`; `contrast/cases/cs2.toml`

**Interfaces:**
- Produces: `GET /api/cs2/server-info` (`routes/cs2Api.js:230`). `GET /current-match`, `POST /match-finish` y `POST /match-cancel` siguen en el proxy porque tocan el estado de partidas que migra en la fase 2.

- [ ] **Step 1: Write the failing tests**: `server_info_matches_node_shape` con las variables `CS2_SERVER_PUBLIC_IP`/`MIXTON_SERVER_IP` del Config; `current_match_is_proxied`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** el handler leyendo la configuración.
- [ ] **Step 4: Run** → PASS; contraste sin diferencias.
- [ ] **Step 5: Commit** `feat(web): api cs2 de solo lectura`.

### Task 17: Despliegue, medición final y corte

**Files:**
- Create: `crates/web/src/main.rs` (arranque completo), `Dockerfile`, `docker-compose.yml`, `docs/runbook-corte.md`, `docs/baseline-rust-fase1.json`, `docs/comparativa-fase1.md`

**Interfaces:**
- Consumes: todas las rutas y middlewares anteriores.
- Produces: binario `web` que arranca con `Config::from_env()`, monta rutas migradas → estáticos → proxy como `fallback`, cierra con gracia ante `SIGINT`/`SIGTERM`; `docker-compose.yml` con `web` (puerto 15633) y el túnel de Cloudflare leyendo `TUNNEL_TOKEN` desde el entorno (nunca escrito en el archivo).

- [ ] **Step 1: Write the failing test** `crates/web/tests/e2e.rs`: levanta Node simulado y `web::serve`, y comprueba `GET /api/ping` (Rust), `GET /api/algo-no-migrado` (proxy) y que `POST /api/reclamaciones` llega al simulado.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `main.rs`, `Dockerfile` multi-stage (builder `rust` → runtime `debian:bookworm-slim`) y el compose.
- [ ] **Step 4: Run** `cargo test --workspace` → PASS; arrancar Node y Rust juntos y ejecutar todo `contrast/cases` → sin diferencias.
- [ ] **Step 5:** Ejecutar `node tools/baseline.mjs --base http://127.0.0.1:15633 --pid <pid de web> --out docs/baseline-rust-fase1.json` y redactar `docs/comparativa-fase1.md` con RAM y p50/p95/p99 de Node vs Rust. Criterio de salida de la spec: p95 y RAM iguales o mejores; si no, anotar la causa antes del corte.
- [ ] **Step 6:** Escribir `docs/runbook-corte.md`: orden de arranque (Node en `:3001`, luego Rust en `:15633`), cambio del destino del túnel de Cloudflare al puerto de Rust, verificación posterior (`/api/ping`, login con Discord, un endpoint admin) y rollback (apuntar el túnel de nuevo a Node y parar Rust).
- [ ] **Step 7: Commit** `feat: arranque, despliegue y medición final de la fase 1`.

---

## Auto-revisión

- **Cobertura de la spec:** §3 arquitectura (T6, T17), §4 capa de datos (T5, T11, T15), §5 fases 0 y 1 (T1-T17), §6 errores (Global Constraints, T6, T9, T11), §7 pruebas (T9, T10 y los tests por tarea; la carga con `oha`/`k6` se sustituye por `tools/baseline.mjs` en T4 y T17, sin dependencia extra), §8 riesgos (T5 para Firestore, T9 para sesiones, Review Focus). Las fases 2 y 3 quedan para planes posteriores.
- **Consistencia de tipos:** `Config`, `SessionClaims`, `AuthUser`/`AdminUser`, `FirestoreClient`, `GuildSettingsRepo`, `DiscordApi`, `Case`/`run_case` se definen una vez y se consumen con la misma firma.
- **Corrección respecto a la spec:** `routes/publicApi.js` tiene 2 GET y 2 POST; solo se migran los GET, y los POST (que escriben) se quedan en el proxy. La spec decía «4 endpoints de solo lectura».
