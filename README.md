# MACHUPEEK

Migración incremental del bot de Discord y la web de Machupeek (hoy en Node.js,
`D:\Discord_Faceit`) a Rust.

**Objetivo:** bajar el consumo de RAM/CPU del servidor y la latencia de la web,
sin cambiar nada de lo que ve la comunidad (mismas rutas, mismas respuestas,
mismo esquema de Firestore, mismos frontends React).

**Cómo:** Rust pasa a ser la puerta de entrada del sitio y reenvía por proxy todo
lo que aún no migró a Node. Cada ruta que se migra se atiende en Rust; revertirla
es quitarla de Rust. El Node legado es solo de referencia (solo lectura) y sigue
corriendo con el bot y las rutas pendientes.

## Arquitectura

```
Cloudflare Tunnel
      │
      ▼
 ┌─────────────┐   ruta aún no migrada    ┌──────────────────┐
 │ Rust (axum) │ ───── proxy inverso ───▶ │ Node (legado)    │
 │  :15633     │   (HTTP y WebSocket)     │  bot + rutas     │
 └─────────────┘                          │  restantes :3001 │
   │                                      └──────────────────┘
   └─ Firestore (solo lectura)             ┌──────────────────┐
                                           │ Node: Steam bot  │
                                           │  :15634          │
                                           └──────────────────┘
```

Rust ya sirve: archivos estáticos y rutas de la SPA, el login de Discord (redirect y
logout), `/api/ping`, `/api/products`, `/api/reclamaciones/:code` y diez lecturas del
panel de administración. Antes de las rutas pasa por las capas equivalentes a las del
legado: cabeceras de seguridad, redirect a HTTPS, CORS, límite de peticiones (100/min)
y límite de cuerpo JSON (64 KiB).

## Estado por fase

| Fase | Contenido | Estado |
|---|---|---|
| 0 | Workspace, inventario de endpoints, herramientas de medición y contraste | Hecha. **La medición real de Node contra Rust sigue pendiente** (necesita un entorno de pruebas) |
| 1 | Rust como puerta de entrada: proxy, estáticos, SPA, auth parcial, rutas públicas | Hecha |
| 2A | Sesión y datos endurecidos, repositorios de lectura y 10 lecturas de `/api/admin/*` | Hecha |
| 2B | Lecturas con dependencias externas (API de Faceit, REST de Discord) y callback de OAuth | Pendiente |
| 2C | Lobby y su WebSocket, `cs2Api`, escrituras de administración, Halloween (SSE), webhooks de Yape y MatchZy | Pendiente |
| 3 | Bot de Discord (comandos, eventos, voz) | Pendiente |

La RAM solo baja de verdad cuando Node deja de correr, es decir, al terminar las fases
2C y 3. Mientras tanto, la ganancia esperada es de latencia en las rutas migradas.

### Qué sirve Rust en la fase 2A

Con Firestore configurado: `GET /api/admin/claims`, `/claims/:code`, `/orders`, `/logs`,
`/support-calls`, `/messages`, `/matches/history`, `/stats`, `/users/backup` y
`/yape/transactions`. Sin Firestore siguen en Node. Todo lo demás de `/api/admin/*`
(escrituras y el resto de lecturas, entre otras `/users`, `/matches`, `/guilds`,
`/stats/:nickname`, `/matches/:id/demo` y `/settings`) sigue en Node por el proxy; la
lista completa está en el runbook, §3.4.

## Estructura

- `crates/common`: configuración (`Config::from_env`, `firestore_mode`).
- `crates/data`: acceso a Firestore (solo lectura): repositorios, consultas, cachés y listeners.
- `crates/web`: servidor axum. `web::build_app` monta el frente de entrada: rutas
  migradas, estáticos y SPA, y proxy hacia Node para todo lo demás.
- `crates/contrast`: compara respuestas de Node y Rust (`contrast/cases`).
- `tools/`: inventario de endpoints, línea base de latencia y generadores de vectores.
- `docs/`: runbook de corte, riesgos, inventario de endpoints, especificación y planes.

## Arranque

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cp .env.example .env          # rellenar; nunca versionar .env
cargo run -p web              # escucha en :15633; Node legado en LEGACY_URL (por defecto :3001)
cargo build --release -p web  # target/release/web(.exe)
target/release/web --healthcheck   # GET /api/ping local; salida 0 = sano
```

Códigos de salida: 2 = configuración inválida (sin mostrar secretos), 1 = fallo de
arranque (bind, Firestore).

Docker: `docker compose up -d --build`, con `LEGACY_PUBLIC_DIR` y `TUNNEL_TOKEN`
exportados en la shell. Despliegue, verificación y rollback:
[`docs/runbook-corte.md`](docs/runbook-corte.md).

## Configuración

Todas las variables están comentadas en [`.env.example`](.env.example). Las principales:

| Variable | Para qué sirve |
|---|---|
| `DISCORD_TOKEN`, `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`, `DISCORD_GUILD_ID` | Aplicación de Discord y servidor principal |
| `ADMIN_JWT_SECRET` | Firma de sesiones. Debe ser idéntico al de Node, byte a byte (los espacios de los bordes cuentan) |
| `LEGACY_URL`, `STEAM_BOT_URL`, `STATIC_DIR` | Dónde está Node, el Steam bot y los estáticos compilados |
| `CORS_ORIGINS`, `BASE_URL`, `PORT`, `NODE_ENV` | Servidor y CORS |
| `TRUST_CF_CONNECTING_IP` | Solo `1` si el origen es alcanzable únicamente por el túnel de Cloudflare |
| `FIRESTORE_PROJECT_ID` | Activa Firestore. Sin él, las rutas que lo necesitan siguen en Node |
| `FIRESTORE_EMULATOR_HOST` / `FIREBASE_SERVICE_ACCOUNT_PATH` / `FIRESTORE_USE_ADC=1` | Fuente de credenciales; con proyecto hace falta exactamente una explícita |
| `PROXY_FIRST_BYTE_TIMEOUT_SECS`, `WS_IDLE_TIMEOUT_SECS` | Guardas de tiempo del proxy, desactivadas por defecto |

## Documentación

- [`docs/runbook-corte.md`](docs/runbook-corte.md): activación, requisito de que las
  escrituras del panel pasen por Rust, rollback por ruta y modos de fallo.
- [`docs/riesgos.md`](docs/riesgos.md): diferencias conocidas con Node y lo que no se ha
  verificado en vivo (sección «Fase 2A»).
- [`docs/endpoint-inventory.md`](docs/endpoint-inventory.md): cada endpoint del legado y su fase.
- [`docs/superpowers/specs/2026-11-02-migracion-rust-design.md`](docs/superpowers/specs/2026-11-02-migracion-rust-design.md):
  diseño completo de la migración.
- Planes: [fases 0 y 1](docs/superpowers/plans/2026-11-02-migracion-rust-fases-0-1.md) y
  [fase 2A](docs/superpowers/plans/2026-11-03-migracion-rust-fase-2a.md) (su sección
  «Fuera de este plan» describe 2B, 2C y la fase 3).

## Pruebas

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

`crates/web/tests/e2e.rs` levanta la app ensamblada contra un Node simulado. Nada
en las pruebas por defecto conecta con Firestore ni con Discord; los tests del
emulador están marcados `#[ignore]`.
