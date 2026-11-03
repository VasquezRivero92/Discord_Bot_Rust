# Comparativa de rendimiento de la fase 1: Node vs Rust

**Estado: PENDIENTE.** No hay medición comparable de antes y después. No existen
`docs/baseline-node.json` ni `docs/baseline-rust-fase1.json`.

## Por qué está pendiente

Para comparar hay que medir el Node legado y el frente Rust (con el legado
detrás) en la misma máquina y con la misma carga. No hay un entorno de prueba
del legado. Arrancarlo con su `.env` de producción conectaría el bot de Discord
y el Firestore de producción, y eso está prohibido. Sin el legado en marcha,
tampoco se puede medir Rust en condiciones reales, porque lo no migrado pasa
por el proxy hacia Node.

## Criterio de salida (spec, `docs/superpowers/specs/2026-11-02-migracion-rust-design.md`)

En los endpoints medidos, el **p95** y la **RAM en reposo** del frente Rust
tienen que ser **iguales o mejores** que la línea base de Node. Si no lo son,
hay que anotar la causa antes del corte.

## Cómo medir (cuando exista el entorno de prueba)

Protocolo completo y entorno necesario: `docs/baseline-node.md` (bot de prueba,
Firestore de prueba o emulador, `ADMIN_JWT_SECRET` de prueba, máquina sin
carga, fecha y máquina anotadas).

1. Node solo, en `:3001`:
   ```
   node tools/baseline.mjs --base http://127.0.0.1:3001 --pid <pid de node> --out docs/baseline-node.json --jwt-secret <secreto-de-prueba>
   ```
2. Rust en `:15633` con el mismo Node detrás (`LEGACY_URL=http://127.0.0.1:3001`),
   en la misma máquina y la misma sesión:
   ```
   node tools/baseline.mjs --base http://127.0.0.1:15633 --pid <pid de web> --out docs/baseline-rust-fase1.json --jwt-secret <secreto-de-prueba>
   ```
3. Rellenar la tabla de abajo y escribir la conclusión.

**Rate limit.** Tanto Node (`globalLimiter`) como Rust limitan a 100
peticiones por minuto por cliente. `tools/baseline.mjs` envía un
`X-Forwarded-For` aleatorio en cada petición para que cada una caiga en su
propio cubo en ambos servidores. Mide siempre directo (no a través de
Cloudflare), con `PROXY_APPEND_PEER_TO_XFF` vacío y `TRUST_CF_CONNECTING_IP`
desactivado en Rust, y reinicia cada proceso medido después de la ejecución,
porque las IP aleatorias hacen crecer los mapas del limitador (ver
`docs/baseline-node.md`).

## Tabla (a rellenar; ms, salvo RAM)

Fecha: ____ · Máquina: ____ · Commit Rust: ____ · Commit legado: ____

| Endpoint | Node p50 | Node p95 | Node p99 | Rust p50 | Rust p95 | Rust p99 | ¿p95 Rust <= Node? |
|---|---|---|---|---|---|---|---|
| `GET /api/ping` (Rust) | | | | | | | |
| `GET /api/products` (Rust) | | | | | | | |
| `GET /health` (proxy) | | | | | | | |
| `GET /api/lobby/me` (proxy) | | | | | | | |
| `GET /api/admin/me` (proxy) | | | | | | | |
| `GET /api/admin/guilds/<id>/settings` (proxy) | | | | | | | |
| `GET /api/admin/map-catalog` (proxy) | | | | | | | |
| Ráfaga concurrente (50), p95 de cada endpoint | | | | | | | |

| RAM en reposo (RSS) | Node | Rust (`web`) | Node con Rust delante (Node + `web`) |
|---|---|---|---|
| MB | | | |

Las rutas proxificadas siempre suman un salto, así que en esas filas Rust no
puede ganar a Node. Lo que hay que comprobar ahí es que el sobrecoste del proxy
sea pequeño. La RAM total en la fase 1 es Node + `web`, porque Node sigue
corriendo. El criterio se aplica a lo que Rust sirve por sí mismo y al
sobrecoste razonable del proxy.

Conclusión: ____

## Lo único medido aquí (NO comparable con la línea base de Node)

Binario release de `web` (Windows 11, 2026-11-03), con un Node **falso** de una
línea detrás (contesta texto plano, sin bot ni Firestore). Lo medido es el coste
propio de Rust y el de un salto de proxy a un upstream trivial, no el sistema
real. Cliente: `curl` con conexión nueva por petición, 200 peticiones
secuenciales y un `X-Forwarded-For` distinto en cada una para no tocar el
límite.

| Medida | Valor |
|---|---|
| Tamaño del binario `target/release/web.exe` | 14,1 MB |
| RSS (working set) al arrancar | 21,4 MB (privada: ~3 MB) |
| RSS tras ~1000 peticiones | 21,3 MB |
| `GET /api/ping` (Rust) | p50 1,37 ms · p95 1,80 ms · p99 2,29 ms |
| `GET /health` (proxy a Node falso) | p50 1,87 ms · p95 2,31 ms · p99 2,66 ms |

Nota: en esta máquina, un cliente `fetch` de Node 24 daba unos 15 ms planos por
petición, tanto contra Rust como contra el Node falso directamente. Es un
artefacto del cliente en Windows y por eso se descartó en favor de `curl`.
