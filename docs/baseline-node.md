# Línea base de rendimiento del Node legacy

**Estado: PENDIENTE.** No hay números medidos todavía; no existe `docs/baseline-node.json`.

## Por qué está pendiente

Medir requiere ejecutar el Node legacy (`D:\Discord_Faceit`). Arrancarlo con su `.env` de producción conectaría el bot de Discord de producción y el Firestore de producción, lo cual no está autorizado. La herramienta (`tools/baseline.mjs`) ya está lista y probada contra un servidor stub local; solo falta la medición en vivo contra una instancia segura.

## Entorno necesario

- Instancia del legacy arrancada con un token de Discord que NO sea el de producción (bot de prueba) y con un emulador de Firestore o un proyecto Firestore de pruebas.
- `ADMIN_JWT_SECRET` fijado a un valor de prueba conocido (el mismo que se pasa al medidor). Nunca un secreto real.
- El PID del proceso Node de esa instancia (para leer el RSS en reposo).
- Una máquina sin otra carga pesada; anotar fecha y máquina al guardar el resultado.

## Comando

```
node tools/baseline.mjs --base http://127.0.0.1:3001 --pid <pid> --out docs/baseline-node.json --jwt-secret <secreto-de-prueba>
```

Opciones: `--jwt-secret` (o variable `BASELINE_JWT_SECRET`), `--guild-id` (por defecto `0`), `--sequential` (por defecto 200), `--concurrent` (por defecto 50), `--timeout-ms` (por defecto 10000; un timeout cuenta como error). Los números deben ser enteros positivos; ante entrada inválida o un `--pid` cuyo RSS no se puede leer, el medidor imprime un error en español y sale con código 2. El mismo comando, con `--base` y `--pid` del servidor Rust, produce la medición comparable (Tarea 17).

Salida: `{ rssMb, endpoints: { "<METHOD> <path>": { p50, p95, p99, errors, concurrent: { p50, p95, p99, errors } } } }` (ms). `p50/p95/p99` de primer nivel salen de la fase secuencial; `concurrent` contiene los de la ráfaga. Ambos usan solo respuestas 2xx (si no hay ninguna, los percentiles son `null`). `errors` de primer nivel suma los fallos (no 2xx, conexión fallida o timeout) de ambas fases; `concurrent.errors` solo los de la ráfaga.

## Protocolo y endpoints medidos

Por endpoint: 200 peticiones secuenciales y después 50 concurrentes; los percentiles (rango más cercano) se calculan por fase, sin mezclarlas.

Sin cookie:
- `GET /api/ping`
- `GET /health`
- `GET /api/products`
- `GET /api/lobby/me`

Con cookie `auth_token` (JWT HS256 de prueba, 1 h, `isAdmin: true`, `role: "admin"`, `servidor: null`):
- `GET /api/admin/me`
- `GET /api/admin/guilds/<guild-id>/settings`
- `GET /api/admin/map-catalog`

## Pendiente

- [ ] Ejecutar contra instancia segura, guardar `docs/baseline-node.json` y resumir aquí con fecha y máquina.
