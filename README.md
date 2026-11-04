# MACHUPEEK

Migración incremental del bot y la web de Machupeek de Node.js a Rust.

- `crates/common`: configuración (`Config::from_env`, `firestore_mode`).
- `crates/data`: acceso a Firestore (solo lectura): repositorios, consultas y listeners.
- `crates/web`: servidor axum. `web::build_app` monta el frente de entrada: rutas
  migradas, estáticos y SPA, y proxy hacia el Node legado para todo lo demás.
- `crates/contrast`: compara respuestas de Node y Rust (`contrast/cases`).

El Node legado vive en `D:\Discord_Faceit` y es solo de referencia (solo lectura).

## Frente de entrada (fase 1)

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cp .env.example .env          # rellenar; nunca versionar .env
cargo run -p web              # escucha en :15633; Node legado en LEGACY_URL (por defecto :3001)
cargo build --release -p web  # target/release/web(.exe)
target/release/web --healthcheck   # GET /api/ping local; salida 0 = sano
```

Códigos de salida: 2 = configuración inválida (sin mostrar secretos), 1 = fallo de
arranque (bind, Firestore). Con `FIRESTORE_PROJECT_ID` hace falta una fuente de
credenciales explícita (emulador, clave o `FIRESTORE_USE_ADC=1`).

Docker: `docker compose up -d --build`, con `LEGACY_PUBLIC_DIR` y `TUNNEL_TOKEN`
exportados en la shell. Despliegue, verificación y rollback:
[`docs/runbook-corte.md`](docs/runbook-corte.md).

## Estado de la fase 2A

Con Firestore configurado, Rust sirve también diez lecturas del panel de
administración: `GET /api/admin/claims`, `/claims/:code`, `/orders`, `/logs`,
`/support-calls`, `/messages`, `/matches/history`, `/stats`, `/users/backup` y
`/yape/transactions`. Sin Firestore siguen en Node. Todo lo demás de
`/api/admin/*` sigue en Node por el proxy: las escrituras y el resto de
lecturas, entre otras `/users`, `/matches`, `/guilds`, `/stats/:nickname`,
`/matches/:id/demo` y `/settings` (lista completa en el runbook, §3.4). El proxy tiene además dos
guardas de tiempo opcionales (`PROXY_FIRST_BYTE_TIMEOUT_SECS`,
`WS_IDLE_TIMEOUT_SECS`), desactivadas por defecto.

Activación, requisito de que las escrituras del panel pasen por Rust y rollback
por ruta: [`docs/runbook-corte.md`](docs/runbook-corte.md) §3.4. Diferencias
conocidas y lo que no se ha verificado en vivo: [`docs/riesgos.md`](docs/riesgos.md),
«Fase 2A». La medición (Tarea 0 del plan) está pendiente.

Queda para los planes siguientes
([`docs/superpowers/plans/2026-11-03-migracion-rust-fase-2a.md`](docs/superpowers/plans/2026-11-03-migracion-rust-fase-2a.md),
«Fuera de este plan»):

- **2B:** lecturas con dependencias externas (API de Faceit, REST de Discord para
  `/users`, `/matches`, `/discord/roles`, `/guilds`, `/status`; `/system`,
  `/leaderboard`, `/faceit-club-queues`, `/matches/:id/stats` y el callback de
  OAuth).
- **2C:** lobby (`routes/lobby.js` y su WebSocket), `cs2Api`, las escrituras de
  administración, Halloween (SSE) y los webhooks de Yape y MatchZy. El webhook de
  Faceit y los avisos de Yape dependen del gateway de Discord.
- **Fase 3:** el bot de Discord. La RAM solo baja de verdad cuando Node deja de
  correr.

## Pruebas

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

`crates/web/tests/e2e.rs` levanta la app ensamblada contra un Node simulado. Nada
en las pruebas por defecto conecta con Firestore ni con Discord; los tests del
emulador están marcados `#[ignore]`.
