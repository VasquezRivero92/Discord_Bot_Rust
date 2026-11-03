# Runbook de corte: Rust como frente de entrada de machupeek.com (fase 1)

Al terminar este runbook, el túnel de Cloudflare entrega todo el tráfico de
machupeek.com al binario Rust `web` (`:15633`). Rust contesta lo migrado y pasa
todo lo demás, tal cual, al Node legado, que ahora escucha en `:3001`.

```
visitante -> Cloudflare -> cloudflared (tunnel) -> web (Rust, :15633) -> Node legado (:3001)
```

## 1. Qué contesta Rust y qué sigue en Node

Precedencia dentro de `web::build_app` (gana la primera que coincide):

1. Rutas migradas: `GET /api/ping`, `GET /api/products`, `GET /auth/discord`,
   `GET /auth/logout` y `GET /api/reclamaciones/:code`. Esta última solo existe
   si Firestore está configurado (ver 3.3); si no, va a Node.
2. Un método equivocado en esas rutas (p. ej. `POST /api/ping`, `POST /auth/discord`)
   va a Node, no a un 405 de axum (`method_not_allowed_fallback`).
3. Archivos estáticos y rutas SPA (`/`, `/tienda`, `/terms`, `/admin/*`, `/halloween*`, ...),
   servidos desde `STATIC_DIR`, con las mismas cabeceras de caché que el legado.
4. Todo lo demás pasa a Node por el proxy (HTTP y WebSocket). Eso incluye
   `/auth/discord/callback`, `/auth/steam*`, `/auth/demo-login`, `/health`,
   `/api/admin/*`, `/api/lobby/*` (incluido el WebSocket `/api/lobby/mixton-ws`),
   `/api/cs2/*`, `/webhook*`, `POST /api/reclamaciones` y `POST /api/checkout/purchase`.

Por fuera de todo va `middleware::apply`: cabeceras de seguridad (helmet sin CSP),
redirección HTTPS en producción, CORS, rate limit de 100 peticiones por minuto
por cliente y límite de 64 kB para cuerpos JSON. Cubre también lo que va a Node,
así que el rate limit de Rust y el de Node se suman. Si Node no responde, el
proxy devuelve `502 {"error":"Servicio no disponible"}` y lo migrado sigue
funcionando.

## 2. Requisitos previos

- Docker Desktop (Windows) o Docker Engine con compose v2, y el árbol
  `D:\MACHUPEEK` en la rama a desplegar.
- El build de `public/` del legado (`D:\Discord_Faceit\public`). Rust lo sirve en
  solo lectura y nunca lo modifica.
- Acceso al panel de Cloudflare Zero Trust del túnel de machupeek.com (o un
  token de API con permiso `Cloudflare Tunnel: Edit` para el rollback por API).
- **Anota antes de empezar** el destino actual del hostname público del túnel
  (Zero Trust > Networks > Tunnels > *túnel* > Public Hostname > Service). Lo
  necesitas para el rollback.
- Para cualquier prueba previa (contraste, medición): un entorno de PRUEBA con
  bot de Discord de prueba y Firestore de prueba o emulador. **Nunca** arranques
  el legado con el `.env` de producción fuera del corte en sí: conectaría el bot
  de producción.

## 3. Configuración

### 3.1 `.env` de Rust (`D:\MACHUPEEK\.env`, nunca se versiona)

Parte de `.env.example`. Pon solo lo que Rust lee. No copies el `.env` entero del
legado: tiene secretos (Faceit, Yape, Steam...) que Rust no usa.

| Variable | Valor en producción | Notas |
|---|---|---|
| `ADMIN_JWT_SECRET` | el mismo que Node | obligatorio; las cookies las firma Node |
| `BASE_URL` | `https://machupeek.com` | obligatorio. Se le quitan las `/` finales una sola vez en `Config`. Con `https` activa el modo producción (redirección HTTPS) |
| `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`, `DISCORD_TOKEN`, `DISCORD_GUILD_ID` | los de Node | obligatorios. El `redirect_uri` de `/auth/discord` es `BASE_URL/auth/discord/callback`, y ese callback lo atiende Node |
| `NODE_ENV` | `production` | con `CORS_ORIGINS` vacío, CORS acepta solo el origen de `BASE_URL` |
| `CORS_ORIGINS` | igual que Node | opcional |
| `PORT` | (lo fija compose: `15633`) | |
| `LEGACY_URL` | (lo fija compose: `WEB_LEGACY_URL` o `http://host.docker.internal:3001`) | en Windows sin Docker: `http://127.0.0.1:3001` |
| `STATIC_DIR` | (lo fija compose: `/srv/public`) | |
| `TRUST_CF_CONNECTING_IP` | `1` **solo si se cumple 3.2** | ver abajo |
| `PROXY_APPEND_PEER_TO_XFF` | vacío (no) | ver abajo |
| `FIRESTORE_*`, `FIREBASE_SERVICE_ACCOUNT_PATH` | ver 3.3 | |

**`TRUST_CF_CONNECTING_IP`.** Con `1`, el rate limit usa la cabecera
`CF-Connecting-IP` como identidad del cliente. Si no, usa la entrada más a la
derecha de `X-Forwarded-For`, y si tampoco hay, la IP del socket. Cualquiera que
llegue al origen sin pasar por Cloudflare puede inventarse esas cabeceras. Por
eso solo se activa cuando el origen es alcanzable **únicamente** a través del
túnel (3.2). Con el valor por defecto (vacío) sigue funcionando: cloudflared
añade la IP real del visitante al final de `X-Forwarded-For`.

**`PROXY_APPEND_PEER_TO_XFF`.** Déjalo vacío. Node usa `trust proxy 1` y toma la
entrada derecha de `X-Forwarded-For`, que tiene que seguir siendo la que añadió
cloudflared. Si Rust añadiera la suya, Node vería como cliente la IP del
contenedor cloudflared. Por defecto el proxy reenvía `X-Forwarded-For` byte a byte.

### 3.2 El origen solo se alcanza por el túnel

- El servicio `web` de `D:\MACHUPEEK\docker-compose.yml` **no publica puertos**
  en el host. No añadas `ports:`.
- En el `docker-compose.yml` del legado (lo edita el dueño, no esta migración),
  **elimina el mapeo `80:15633`** antes del corte. Con ese mapeo, cualquiera que
  llegue a la IP de la máquina entra al origen sin Cloudflare y falsea
  `X-Forwarded-For` y `CF-Connecting-IP`. Publica Node solo en loopback:
  `127.0.0.1:3001:3001`. Si el puerto `15634` (API del Steam bot) no hace falta
  desde fuera, publícalo también en `127.0.0.1`.
- **Token del túnel.** El `docker-compose.yml` del legado tiene el
  `TUNNEL_TOKEN` escrito en texto plano. Si ese archivo se compartió alguna vez
  (repositorio, copia, captura, chat), **rota el token** en Zero Trust (Networks >
  Tunnels > *túnel* > Configure > rotar o refrescar el token; si no está
  disponible, crea un túnel nuevo) y quita el literal del archivo. En este
  repositorio el token solo se lee del entorno (`${TUNNEL_TOKEN:?...}`) y no va
  en `.env`, porque `env_file: .env` lo metería en el contenedor `web`.

### 3.3 Firestore (modo de credenciales explícito)

Rust solo usa Firestore para `GET /api/reclamaciones/:code`. El modo se decide
con `common::config::firestore_mode`. Se escribe en el log de arranque y nunca
cae en las credenciales por defecto sin pedirlo:

| `FIRESTORE_PROJECT_ID` | `FIRESTORE_EMULATOR_HOST` | `FIREBASE_SERVICE_ACCOUNT_PATH` | `FIRESTORE_USE_ADC` | Modo |
|---|---|---|---|---|
| vacío | cualquiera | cualquiera | cualquiera | `Disabled`: la ruta queda en Node (aviso en el log si hay variables de credenciales) |
| sí | sí | cualquiera | cualquiera | `Emulator` (gana siempre: nunca datos reales) |
| sí | no | sí | cualquiera | `KeyFile` |
| sí | no | no | `1`/`true`/`yes`/`on` | `Adc` (Application Default Credentials, solo si lo activas tú) |
| sí | no | no | no | **error de arranque, código 2** |

Recomendación para el primer corte: `Disabled`, con las reclamaciones todavía
en Node. Así el corte no depende de Firestore. Para activar `KeyFile` en
compose, monta la clave en solo lectura con un
`docker-compose.override.yml` (no versionado):

```yaml
services:
  web:
    environment:
      FIRESTORE_PROJECT_ID: <proyecto>
      FIREBASE_SERVICE_ACCOUNT_PATH: /run/secrets/firebase-sa.json
    volumes:
      - type: bind
        source: ${FIREBASE_SA_FILE:?set FIREBASE_SA_FILE}
        target: /run/secrets/firebase-sa.json
        read_only: true
```

Usa una cuenta de servicio con permisos de **solo lectura**. El contenedor corre
como el usuario sin privilegios `10001` y ese usuario tiene que poder leer el
archivo. Si la conexión falla, el binario sale con código 1 y un mensaje claro
(el tope es de 30 s).

## 4. Arranque

### 4.1 Comprobaciones previas (sin tocar producción)

```bash
cd /d/MACHUPEEK
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
docker compose build web
```

### 4.2 Orden del corte

1. **Node a `:3001`.** En el `.env` del legado pon `PORT=3001`. En su
   `docker-compose.yml`, quita `80:15633`, publica `127.0.0.1:3001:3001` y
   **para su servicio `tunnel`** (si quedan dos conectores del mismo túnel, el
   tráfico se reparte entre ambos). Reinicia Node:
   ```bash
   cd /d/Discord_Faceit
   docker compose stop tunnel
   docker compose up -d discord-faceit-bot
   docker compose logs --tail 50 discord-faceit-bot   # "Servidor Express escuchando en http://0.0.0.0:3001"
   ```
   Desde este momento machupeek.com no responde hasta el paso 4. Hazlo en una
   ventana de bajo tráfico. Node sigue atendiendo todo lo que todavía le
   pertenece: bot, webhooks y API admin.
2. **Comprueba que un contenedor llega a Node** por `host.docker.internal`:
   ```bash
   docker run --rm --add-host host.docker.internal:host-gateway curlimages/curl:8.10.1 \
     -s -o /dev/null -w "%{http_code}\n" http://host.docker.internal:3001/health
   ```
   Tiene que imprimir `200`. En Linux, `host-gateway` es la IP del puente docker
   y un puerto publicado solo en `127.0.0.1` no se alcanza. En ese caso publica
   Node en la IP del puente o, mejor, conecta ambos proyectos a una red docker
   externa común y usa `WEB_LEGACY_URL=http://discord-faceit-bot:3001`.
3. **Rust y el túnel nuevo:**
   ```bash
   cd /d/MACHUPEEK
   export LEGACY_PUBLIC_DIR=/d/Discord_Faceit/public
   export TUNNEL_TOKEN='<token del túnel>'    # en la shell, nunca en un archivo versionado
   docker compose up -d --build
   docker compose ps                          # web: healthy
   docker compose logs --tail 30 web
   ```
   El banner de arranque muestra `port`, `legacy_url`, `static_dir`, `base_url`,
   `production`, `trust_cf_connecting_ip`, `proxy_append_peer_to_xff` y
   `firestore` (modo de credenciales). No muestra ningún secreto.
4. **Apunta el túnel a Rust.** En Zero Trust > Tunnels > *túnel* > Public
   Hostname `machupeek.com` > Service, pon `http://web:15633`. El túnel nuevo
   vive en la red de compose de `D:\MACHUPEEK` y resuelve `web`.

### 4.3 Desarrollo en Windows (sin Docker)

```bash
cd /d/MACHUPEEK
export PATH="$HOME/.cargo/bin:$PATH"
# .env de desarrollo: BASE_URL=http://localhost:15633, LEGACY_URL=http://127.0.0.1:3001,
# STATIC_DIR=D:/Discord_Faceit/public, secretos de PRUEBA.
cargo run -p web                    # o: cargo build --release -p web && target/release/web.exe
curl -s http://127.0.0.1:15633/api/ping
target/release/web.exe --healthcheck; echo $?   # 0 = sano
```

Ctrl-C cierra con orden. Las conexiones abiertas tienen 8 s para terminar; los
WebSocket y SSE proxificados se cortan al acabar ese plazo. En Linux y en el
contenedor, SIGTERM (`docker stop`) hace lo mismo. El legado en local solo se
arranca con un `.env` de PRUEBA.

## 5. Verificación tras el corte

| Comprobación | Cómo | Esperado |
|---|---|---|
| Rust vivo | `curl -s https://machupeek.com/api/ping` | JSON `{"ok":true,...,"protocol":"https"}` |
| Cabeceras | `curl -sI https://machupeek.com/` | `x-frame-options: SAMEORIGIN`, `cache-control: no-cache, no-store, must-revalidate` |
| Node por el proxy | `curl -s https://machupeek.com/health` | JSON del legado con `discord` |
| Login Discord de punta a punta | navegador: `https://machupeek.com/auth/discord` -> autorizar -> callback (Node, por el proxy) | vuelve al sitio con sesión; `/api/admin/me` responde con el usuario |
| Página admin | `https://machupeek.com/admin` con un usuario admin | carga y sus llamadas `/api/admin/*` responden |
| WebSocket del lobby | abrir `/mixton` y el chat del lobby | conecta (`/api/lobby/mixton-ws` tunelizado) y llegan mensajes |
| Webhook | enviar un webhook de prueba desde Faceit (o esperar uno real) | llega a Node: `docker compose -f /d/Discord_Faceit/docker-compose.yml logs discord-faceit-bot` |
| Logs Rust | `docker compose logs -f web` | sin errores; sin 502 continuos |
| IP real en Node | logs de Node o rate limit de Node | IP del visitante, no la de un contenedor |

### 5.1 Contraste Node vs Rust (solo entorno de PRUEBA)

```bash
cargo run -p contrast -- --node http://127.0.0.1:3001 --rust http://127.0.0.1:15633 \
  --jwt-secret "$CONTRAST_JWT_SECRET" contrast/cases
```

Se ejecuta contra una instancia de prueba (bot de prueba, Firestore de prueba o
emulador, `ADMIN_JWT_SECRET` de prueba igual en ambos), nunca contra producción.
Los casos actuales son solo GET y no escriben, pero la regla vale igual para los
casos futuros. Salida 0 = sin diferencias. Las rutas no migradas comparan Node
directo con Node a través del proxy Rust, lo que sirve para comprobar que el
proxy es transparente.

## 6. Rollback (menos de un minuto)

El rollback no reinicia Node, que sigue en `:3001`.

1. Devuelve el tráfico a Node cambiando el Service del hostname público a
   `http://host.docker.internal:3001` (el túnel nuevo llega al host por
   `host-gateway`, ver `extra_hosts` del servicio `tunnel`). Desde el panel
   (Zero Trust > Tunnels > *túnel* > Public Hostname > Service) o por API:
   ```bash
   # CF_API_TOKEN, CF_ACCOUNT_ID y CF_TUNNEL_ID en el entorno; no los pegues en archivos.
   curl -s -X PUT \
     "https://api.cloudflare.com/client/v4/accounts/$CF_ACCOUNT_ID/cfd_tunnel/$CF_TUNNEL_ID/configurations" \
     -H "Authorization: Bearer $CF_API_TOKEN" -H "Content-Type: application/json" \
     --data '{"config":{"ingress":[{"hostname":"machupeek.com","service":"http://host.docker.internal:3001"},{"service":"http_status:404"}]}}'
   ```
   Si la configuración tiene más hostnames, inclúyelos todos: el PUT reemplaza
   la configuración entera.
2. Para Rust: `docker compose -f /d/MACHUPEEK/docker-compose.yml stop web`.
3. Comprueba que `https://machupeek.com/health` responde con el JSON del legado.

Para volver del todo al estado anterior al corte, en una ventana tranquila
(implica reiniciar el bot): `docker compose -f /d/MACHUPEEK/docker-compose.yml down`,
y en el legado `PORT` vuelve a su valor anterior, se arranca de nuevo su
`tunnel` y el Service vuelve al destino que anotaste en el paso 2. Sin el
mapeo `80:15633`.

## 7. Limitaciones conocidas

- **Firestore no verificado en vivo.** Los tests con emulador (`#[ignore]`) no
  se ejecutaron nunca: el comportamiento de los listeners y de la conexión real
  no está probado (`docs/riesgos.md`). Por eso se recomienda `Disabled` en el
  primer corte.
- **Medición pendiente.** Ni la línea base de Node ni la comparativa de Rust se
  han medido en vivo (`docs/baseline-node.md`, `docs/comparativa-fase1.md`). El
  criterio de salida de la spec (p95 y RAM iguales o mejores) está sin comprobar.
- **Datos personales en las reclamaciones.** `GET /api/reclamaciones/:code`
  devuelve el documento completo (consumidor, documento de identidad, contacto)
  a cualquiera que tenga el código. Es el comportamiento del legado y Rust lo
  replica tal cual. Está señalado para que el dueño lo decida.
- **`state` de OAuth no es anti-CSRF.** El legado usa `state` para llevar la
  ruta de retorno, así que el login es vulnerable a CSRF. Rust lo replica.
- **Caché de guild-settings.** El TTL es la única cota de obsolescencia. Ninguna
  ruta montada lo usa todavía. Queda como mejora recomendada emitir `Reset` en
  el primer cambio de destino `Current` del listener.
- **405.** Con `method_not_allowed_fallback`, un método equivocado en una ruta
  migrada va a Node, que responde como siempre (normalmente 404). Las rutas
  migradas nunca dan un 405 propio.
- **Solo HTTP/1.** El proxy hacia Node usa HTTP/1.1. cloudflared habla con el
  origen en HTTP/1.1, así que esto no cambia nada para el visitante.
- **Doble rate limit.** Rust (100/min por cliente) y Node (su `globalLimiter`)
  limitan por separado. Lo que pasa por el proxy consume cuota en ambos.
- **Cierre de conexiones largas.** Al parar, los WebSocket y SSE abiertos se
  cortan a los 8 s y los clientes tienen que reconectar.

## 8. Secretos

- `.env` (Rust) y el `.env` del legado nunca se versionan (`.gitignore` y
  `.dockerignore` los excluyen). La imagen no contiene secretos.
- `TUNNEL_TOKEN` solo en la shell que lanza compose. Rótalo si alguna vez
  estuvo en un archivo compartido (ver 3.2).
- La clave de Firestore se monta en solo lectura y nunca se copia a la imagen
  ni al repositorio. Usa una cuenta de solo lectura.
- Los logs de Rust no incluyen secretos: el `Debug` de `Config` los redacta y
  el banner solo muestra datos de despliegue.
- Para contraste y medición se usan siempre secretos de PRUEBA
  (`CONTRAST_JWT_SECRET`, `BASELINE_JWT_SECRET`).
