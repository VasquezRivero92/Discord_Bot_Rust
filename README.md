# MACHUPEEK

Migración incremental del bot y la web de Machupeek de Node.js a Rust.

- `crates/common`: configuración (`Config::from_env`, `firestore_mode`).
- `crates/data`: acceso a Firestore (solo lectura).
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

## Pruebas

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

`crates/web/tests/e2e.rs` levanta la app ensamblada contra un Node simulado. Nada
en las pruebas por defecto conecta con Firestore ni con Discord; los tests del
emulador están marcados `#[ignore]`.
