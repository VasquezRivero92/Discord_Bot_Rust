# Runbook de corte: Rust como frente de entrada de machupeek.com (fases 1 y 2A)

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
   si Firestore está configurado (ver 3.3); si no, va a Node. Con Firestore
   configurado se montan además las lecturas del panel de la fase 2A (ver 3.4):
   `GET /api/admin/claims`, `/claims/:code`, `/orders`, `/logs`,
   `/support-calls`, `/messages` y `/matches/history` (Tarea 7), y
   `GET /api/admin/stats`, `/users/backup` y `/yape/transactions` (Tarea 8).
2. Un método equivocado en esas rutas (p. ej. `POST /api/ping`, `POST /auth/discord`)
   va a Node, no a un 405 de axum (`method_not_allowed_fallback`).
3. Archivos estáticos y rutas SPA (`/`, `/tienda`, `/terms`, `/admin/*`, `/halloween*`, ...),
   servidos desde `STATIC_DIR`, con las mismas cabeceras de caché que el legado.
4. Todo lo demás pasa a Node por el proxy (HTTP y WebSocket). Eso incluye
   `/auth/discord/callback`, `/auth/steam*`, `/auth/demo-login`, `/health`,
   el resto de `/api/admin/*` (todas las escrituras y las demás lecturas, ver
   3.4),
   `/api/lobby/*` (incluido el WebSocket `/api/lobby/mixton-ws`),
   `/api/cs2/*`, `/webhook*`, `POST /api/reclamaciones` y `POST /api/checkout/purchase`.

Por fuera de todo van las cabeceras de seguridad (helmet sin CSP), la
redirección HTTPS en producción, CORS y el límite de 64 kB para cuerpos JSON
(`middleware::apply_except_rate_limit`). Se aplican a todo, incluido lo que va a
Node.

El **rate limit** (100 peticiones por minuto por cliente) es un único limitador
con un solo cubo por cliente, compartido por las rutas migradas (1 y 2) y por
todo lo que pasa al proxy (4). Lo que contesta `static_files` (3) está
**exento**: archivos, `index.html` y rutas SPA y legales. En el legado,
`express.static` corría antes del `globalLimiter`, y un visitante detrás de
CGNAT no debe recibir 429 en los assets. Hay una desviación consciente: en el
legado, las rutas SPA y legales (`/`, `/tienda`, `/terms`, `/admin`...) eran
`app.get` posteriores al limitador y sí consumían cuota; en Rust solo sirven
HTML estático y quedan exentas. Lo que `static_files` no contesta y pasa a Node
(p. ej. un asset que no existe) sí cuenta. Lo proxificado consume cuota en
Rust y otra vez en Node.

Si Node no responde, el proxy devuelve
`502 {"error":"Servicio no disponible"}` y lo migrado sigue funcionando.

El proxy tiene dos guardas de tiempo opcionales, **desactivadas por defecto**
(sin ellas se comporta como en la fase 1). Ver 3.1:

- `PROXY_FIRST_BYTE_TIMEOUT_SECS`: si la cabecera de respuesta de Node (estado y
  cabeceras) no llega en ese plazo, Rust responde
  `504 {"error":"Servicio no disponible"}`. El plazo cuenta desde que se envía la
  petición, incluida la subida del cuerpo si Node lo lee antes de contestar. Una
  vez llegada la cabecera, el cuerpo fluye sin límite: nunca se corta una
  respuesta empezada. No se aplica a los WebSocket ni a las rutas de
  `proxy::FIRST_BYTE_TIMEOUT_EXEMPT`, que se comparan como Express (sin
  distinguir mayúsculas, con una `/` final opcional y, bajo un montaje, una `/`
  de más tras él), solo por la ruta:
  - `GET /api/halloween/stream`: SSE, la única respuesta en streaming del legado.
  - Las que esperan al servidor de CS2 o a la máquina EC2 antes de contestar:
    `POST /api/lobby/mixton-create-match` (en modo steam arranca la partida:
    `launchMatchInternal` -> `cs2ServerManager.startServer` ->
    `ensureEc2InstanceRunning`), y las que apagan el servidor
    (`cs2ServerManager.stopServer`, directamente o con `resolveMixtonMatchEnd`):
    `POST /api/matchzy/event`, `POST /api/lobby/tournament-report-winner`,
    `POST /api/admin/matches/:id/cleanup`, `POST /api/cs2/match-finish` y
    `POST /api/cs2/match-cancel`.

  No hay más rutas así en los manejadores HTTP del legado (`index.js`,
  `routes/*.js`, `webhooks/*.js`). Las llamadas a la API de Faceit reintentan
  solo errores de red, con esperas de 0,5 s y 1 s, y no están exentas.
- `WS_IDLE_TIMEOUT_SECS`: cierra un WebSocket proxificado (frame de cierre
  1001 a ambos lados) tras ese tiempo sin ningún frame en ninguna dirección.
  Los ping/pong y los cierres cuentan como actividad. Un socket activo no se
  cierra nunca.

Diferencias deliberadas con el legado:

- **Estáticos por http en producción.** Rust redirige a https también los
  archivos estáticos. En el legado, `express.static` (index.js:156) servía antes
  de la redirección (169). Detrás de Cloudflare todo llega como https, así que
  la diferencia no tiene efecto práctico.
- **WebSocket.** Las actualizaciones a WebSocket (`/api/lobby/mixton-ws`) pasan
  ahora por CORS, la redirección HTTPS y el rate limit de Rust. En el legado,
  `server.on('upgrade')` (index.js:~601) no pasaba por ningún middleware de
  Express. Esto tiene dos riesgos:
  - Si `CORS_ORIGINS` está definido y no incluye el origen del propio sitio
    (`https://machupeek.com`), el navegador recibe **403** al abrir el WebSocket
    del lobby, porque envía `Origin` en el upgrade. Con `CORS_ORIGINS` vacío y
    `NODE_ENV=production` se acepta el origen de `BASE_URL`.
  - Cada upgrade cuenta como una petición en el rate limit, y con el cubo
    agotado se rechaza con 429. Una vez abierta, la conexión no consume más
    cuota.
- **Proto con varias entradas.** `X-Forwarded-Proto` se interpreta como en
  Express (`trust proxy 1`): cuenta la primera entrada, sin espacios. Rust la
  compara sin distinguir mayúsculas (Express sí distingue; Cloudflare siempre
  envía minúsculas).

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
| `ADMIN_JWT_SECRET` | el mismo que Node, byte a byte | obligatorio; las cookies las firma Node. Ver abajo |
| `BASE_URL` | `https://machupeek.com` | obligatorio. Se le quitan las `/` finales una sola vez en `Config`. Con `https` activa el modo producción (redirección HTTPS) |
| `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`, `DISCORD_TOKEN`, `DISCORD_GUILD_ID` | los de Node | obligatorios. El `redirect_uri` de `/auth/discord` es `BASE_URL/auth/discord/callback`, y ese callback lo atiende Node |
| `NODE_ENV` | `production` | con `CORS_ORIGINS` vacío, CORS acepta solo el origen de `BASE_URL` |
| `CORS_ORIGINS` | igual que Node | opcional |
| `PORT` | (lo fija compose: `15633`) | |
| `LEGACY_URL` | (lo fija compose: `WEB_LEGACY_URL` o `http://host.docker.internal:3001`) | en Windows sin Docker: `http://127.0.0.1:3001` |
| `STATIC_DIR` | (lo fija compose: `/srv/public`) | |
| `TRUST_CF_CONNECTING_IP` | `1` **solo si se cumple 3.2** | ver abajo |
| `PROXY_APPEND_PEER_TO_XFF` | vacío (no) | ver abajo |
| `PROXY_FIRST_BYTE_TIMEOUT_SECS` | vacío en el primer corte; después `90` | segundos enteros; vacío o `0` = sin tope. Ver abajo |
| `WS_IDLE_TIMEOUT_SECS` | vacío en el primer corte; después `3600` | segundos enteros; vacío o `0` = nunca. Ver abajo |
| `FIRESTORE_*`, `FIREBASE_SERVICE_ACCOUNT_PATH` | ver 3.3 | |

**Secretos de Discord sin uso en esta fase.** `Config` exige `DISCORD_TOKEN`
(el token del bot de producción) y `DISCORD_CLIENT_SECRET` aunque ninguna ruta
montada en la fase 1 los use: el callback OAuth sigue en Node. Con eso, el token
del bot vive también en un segundo contenedor. Es una limitación conocida.
**Fase 2:** hacerlos opcionales hasta que una ruta los necesite.

**`ADMIN_JWT_SECRET`.** Rust usa el valor tal cual, sin recortar: un espacio o
un salto de línea al principio o al final forman parte del secreto y cambian la
firma (solo se rechaza un valor vacío o que solo tenga espacios). Cópialo
exactamente igual que en el `.env` de Node. Además, el legado
(`config/index.js:12`) usa un secreto de desarrollo por defecto si la variable
falta y `NODE_ENV` no es `production` (en producción falla al arrancar). Rust
siempre exige la variable. Si Node arrancó con el secreto por defecto, las
cookies que emite no valdrán en Rust: en el corte, define la variable en los
dos con el mismo valor.

**`PROXY_FIRST_BYTE_TIMEOUT_SECS` y `WS_IDLE_TIMEOUT_SECS`.** Solo aceptan
dígitos (`120`); `+5`, `1.5`, `30s` o `-1` son un error de configuración
(código 2), como los demás flags. `0` equivale a no definirlas. Valores de
partida:

- Primer corte: **vacías**, para no cambiar nada respecto a la fase 1.
- `PROXY_FIRST_BYTE_TIMEOUT_SECS=90`. Cloudflare espera como mucho 100 s la
  cabecera del origen (planes no Enterprise) y después devuelve su página 524;
  con 90 s, el visitante recibe un 504 JSON y Rust libera la conexión antes. Un
  valor mayor de 100 no cambia nada para quien llega por el túnel. Valores por
  debajo de 2 no tienen sentido: el proxy tarda hasta 2 s (`CONNECT_TIMEOUT`)
  en dar por caído a Node, y con un tope menor un Node inalcanzable respondería
  504 en lugar de 502. Los valores bajos cortan peticiones lentas legítimas que
  no están exentas: sin medir, no bajes de 60.
  Las rutas exentas pueden tardar mucho más que 100 s. En un arranque en frío,
  `POST /api/lobby/mixton-create-match` (modo steam) espera hasta 120 s a que
  la instancia EC2 termine de pararse (si se estaba parando), hasta 120 s a que
  arranque
  (`waitUntilInstanceStopped`/`waitUntilInstanceRunning`, `maxWaitTime: 120`)
  y el SSH hasta 15 intentos, cada uno con un tope de 4 s
  (`executeRemoteCommand('uptime', 4000)`) y 3 s de pausa entre ellos
  (`waitForSshReady`, `services/awsEc2Service.js`): solo esa espera llega a
  unos 100 s en el peor caso (15 × 4 s + 14 × 3 s = 102 s). Después vienen MCSManager y sus
  alternativas. `stopServer` encadena peticiones a MCSManager con un tope de
  8 s cada una y una pausa de 2,5 s (unos 26 s en el peor caso). Más allá de
  100 s, Cloudflare corta y el usuario ve el 524 **mientras Node sigue
  lanzando la partida**: un reintento llega con el primer lanzamiento aún en
  curso (no he comprobado qué hace el legado con dos a la vez). Rust no lo
  evita (la ruta está exenta para no empeorarlo cortando antes); es un riesgo
  del legado que el corte no cambia.
- `WS_IDLE_TIMEOUT_SECS=3600`. Ni el lobby (`routes/lobby.js`) ni el cliente
  (`web_panel_src/src/App.tsx`) envían ping, así que el temporizador mide
  silencio real: un lobby sin cambios no emite nada. El cliente se reconecta
  5 s después de cualquier cierre (`socket.onclose`), así que cerrar un socket
  inactivo solo cuesta una reconexión. Un valor corto provoca reconexiones
  periódicas en lobbies tranquilos.

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

Rust usa Firestore para `GET /api/reclamaciones/:code` y para las lecturas del
panel de la fase 2A (3.4). Sin Firestore, todas esas rutas siguen en Node. El modo se decide
con `common::config::firestore_mode`. Se escribe en el log de arranque y nunca
cae en las credenciales por defecto sin pedirlo:

| `FIRESTORE_PROJECT_ID` | `FIRESTORE_EMULATOR_HOST` | `FIREBASE_SERVICE_ACCOUNT_PATH` | `FIRESTORE_USE_ADC` | Modo |
|---|---|---|---|---|
| vacío | cualquiera | cualquiera | cualquiera | `Disabled`: la ruta queda en Node (aviso en el log si hay variables de credenciales) |
| sí | sí | cualquiera | cualquiera | `Emulator` (gana siempre: nunca datos reales) |
| sí | no | sí | cualquiera | `KeyFile` |
| sí | no | no | `1`/`true`/`yes`/`on` | `Adc` (Application Default Credentials, solo si lo activas tú) |
| sí | no | no | no | **error de arranque, código 2** |

Recomendación para el primer corte: `Disabled`, con las reclamaciones y el
panel todavía en Node. Así el corte no depende de Firestore. Antes de pasar a
`KeyFile` o `Adc` en producción, haz la comprobación de índices de 3.4. Para activar `KeyFile` en
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

### 3.4 Rutas de la fase 2A: activación, escrituras y rollback

**Qué sirve Rust.** Solo con Firestore configurado (3.3): las diez lecturas
`GET /api/admin/*` del punto 1 de la sección 1. Se montan todas o ninguna: no
hay un interruptor por ruta. Siguen en Node, por el proxy, todas las escrituras
del panel y el resto de lecturas `GET /api/admin/*`: `/me`, `/steam/status`, `/system`, `/faceit-club-queues`, `/firestore-metrics`, `/users`, `/discord/roles`, `/matches`, `/leaderboard`, `/stats/:nickname`, `/settings`, `/matches/:id/demo`, `/matches/:id/stats`, `/status`, `/guilds`, `/guilds/:guildId/settings` y `/map-catalog`. Entre ellas,
`/stats/:nickname` usa la API de Faceit, `/matches/:id/demo` el estado en
memoria de Node y la API de Faceit, y `/settings` valores de `process.env` que
`POST /settings` reescribe en caliente.

**Antes de activar Firestore: índices compuestos.** Rust nunca crea índices.
En la consola de Firebase del proyecto (Firestore Database > Índices >
Compuestos), confirma que existen, con estado *Habilitado*, estos dos índices
(`guild_id` ascendente, `createdAt` descendente):

- `faceit_bot_logs` (`GET /api/admin/logs`);
- `faceit_support_calls` (`GET /api/admin/support-calls`).

Si falta alguno, créalo (o añádelo al `firestore.indexes.json` del legado y
despliégalo) y espera a que termine de construirse antes de activar Firestore.
Sin ellos, Firestore rechaza la consulta de un admin de servidor (que siempre
filtra por su servidor) y Rust responde `{ logs: [] }` / `{ calls: [] }` con
200, igual que el legado, y solo deja el error en el log. Es decir, el panel
muestra listas vacías sin avisar. `faceit_matches_history` tiene su índice en
el `firestore.indexes.json` del legado, y `/matches/history` tiene además un
respaldo sin índice.

**Las escrituras del panel tienen que pasar por Rust.** `/stats` tiene una caché
de 60 s por servidor y `/yape/transactions` una de 30 s, como en el legado. Node
vacía las suyas al escribir, pero Rust no lo ve. Por eso Rust vacía las suyas
cuando Node contesta, a través del proxy, `POST /api/admin/users/:id/unlink`,
`POST /api/admin/users/verify` (caché de `/stats`) y
`POST /api/admin/yape/manual-approve` (caché de Yape). Si alguien escribe
directamente en Node (`:3001`, sin pasar por Rust), las lecturas de Rust pueden
quedar desactualizadas hasta 60 s (`/stats`) o 30 s (`/yape/transactions`).
Los cambios que no hace el panel (p. ej. un usuario que se vincula por Discord)
solo los cubre el TTL, igual que en Node. Con `PROXY_FIRST_BYTE_TIMEOUT_SECS`,
esas tres escrituras no están exentas: si Node tarda y Rust responde 504, Rust
ya ha vaciado su caché, pero Node puede terminar la escritura después. Una
lectura entre medias vuelve a guardar el dato anterior, que dura hasta el TTL
(60 s o 30 s).

**Rollback por ruta.** No hay variable ni lista de rutas configurable: el
reparto lo decide el código (`routes::admin::simple_reads::router` y
`routes::admin::derived_reads::router`). Lo que no está montado cae al proxy.
Opciones, de menor a mayor alcance:

1. **Una ruta:** borra su línea `.route("/api/admin/...", get(...))` en el
   router correspondiente, recompila y redespliega `web`
   (`docker compose up -d --build web`). Esa ruta vuelve a Node; el resto sigue
   en Rust.
2. **Todas las rutas con Firestore:** vacía `FIRESTORE_PROJECT_ID` en el `.env`
   de Rust y reinicia `web` (`docker compose up -d web`). Las diez lecturas del
   panel y `GET /api/reclamaciones/:code` vuelven a Node, sin recompilar.
3. **Todo:** rollback del túnel a Node (6.1).

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

En cada paso, el sitio solo depende de algo que ya se comprobó. El conector
del legado se para al final, cuando el conector nuevo ya está sirviendo
tráfico. Antes de cambiar nada en el túnel, cada paso tiene un punto de
abandono. Si algo falla después, ve a la sección 6.

Plazo de abandono: si un servicio no está `healthy` o conectado en **60 s**, no
sigas. Aplica la rama de abandono de ese paso.

1. **Arranca solo `web`, sin tráfico.** Node aún no ha cambiado de puerto, y
   `/api/ping` lo contesta Rust de todas formas.
   ```bash
   cd /d/MACHUPEEK
   export LEGACY_PUBLIC_DIR=/d/Discord_Faceit/public
   export TUNNEL_TOKEN=unused-until-step-3   # compose exige la variable para interpretar el archivo
   docker compose up -d --build web
   docker compose ps web                                                # STATUS: (healthy)
   docker compose exec web /usr/local/bin/web --healthcheck; echo $?    # 0
   docker compose logs --tail 30 web
   ```
   El banner de arranque muestra `port`, `legacy_url`, `static_dir`, `base_url`,
   `production`, `trust_cf_connecting_ip`, `proxy_append_peer_to_xff`,
   `proxy_first_byte_timeout_secs` y `ws_idle_timeout_secs` (`0` =
   desactivado) y `firestore` (modo de credenciales). No muestra ningún secreto. Un `.env`
   inválido produce `configuration error: ...` y un bucle de reinicios con
   código 2.
   **Abandono:** `docker compose down`. El sitio no se ha tocado; corrige `.env`
   o el montaje y repite.
2. **Node a `:3001`.** En el `.env` del legado pon `PORT=3001`. En su
   `docker-compose.yml`, quita `80:15633` y publica `127.0.0.1:3001:3001`. Deja
   su `tunnel` en marcha. Reinicia Node y, nada más arrancar, apunta el Service
   del hostname público a Node en su puerto nuevo, en una dirección que puedan
   resolver **los dos** conectores (el del legado y el nuevo del paso 3):
   `http://host.docker.internal:3001`. Docker Desktop define
   `host.docker.internal` en todos los contenedores; en Linux el conector del
   legado no lo tiene, así que usa `http://discord-faceit-bot:3001` y no
   arranques el conector nuevo hasta haber parado el del legado (pasos 3 y 4 en
   orden inverso, aceptando un corte breve).
   ```bash
   cd /d/Discord_Faceit
   docker compose up -d discord-faceit-bot
   docker compose logs --tail 50 discord-faceit-bot   # "Servidor Express escuchando en http://0.0.0.0:3001"
   # Panel o API (6.1): Service = http://host.docker.internal:3001
   curl -sI https://machupeek.com/api/ping            # 200, servido por Node
   ```
   Entre el reinicio de Node y el cambio del Service el sitio da error. Hazlo en
   una ventana de bajo tráfico.
   **Abandono:** si Node no arranca en `:3001`, restaura su `PORT` anterior y
   vuelve a poner el Service que anotaste en los requisitos previos.
3. **Comprueba que un contenedor llega a Node** y arranca el conector nuevo:
   ```bash
   docker run --rm --add-host host.docker.internal:host-gateway curlimages/curl:8.10.1 \
     -s -o /dev/null -w "%{http_code}\n" http://host.docker.internal:3001/health   # 200
   cd /d/MACHUPEEK
   read -rs TUNNEL_TOKEN && export TUNNEL_TOKEN   # se pega sin que se vea ni quede en el historial
   docker compose up -d tunnel
   docker compose logs --tail 30 tunnel           # "Registered tunnel connection"
   ```
   Durante unos minutos el túnel tiene dos conectores: el del legado y el
   nuevo. Cloudflare admite varias réplicas de `cloudflared` para el mismo
   túnel y reparte el tráfico entre ellas. **Comprueba en el panel** (Tunnels >
   *túnel* > Connectors) que aparecen ambos como sanos. Como el Service apunta a
   una dirección que los dos resuelven, cualquier conector sirve a Node.
   En Linux, el `curl` de arriba falla si Node solo está publicado en
   `127.0.0.1`. En ese caso publica Node en la IP del puente docker o conecta
   ambos proyectos a una red externa común y usa
   `WEB_LEGACY_URL=http://discord-faceit-bot:3001`.
   **Abandono:** `docker compose stop tunnel`. El conector del legado sigue
   sirviendo y no hay que hacer nada más.
4. **Para el conector del legado**, cuando el nuevo ya sirve tráfico:
   ```bash
   docker compose -f /d/Discord_Faceit/docker-compose.yml stop tunnel
   curl -sI https://machupeek.com/api/ping       # sigue 200 (Node, por el conector nuevo)
   ```
   **Abandono:** `docker compose -f /d/Discord_Faceit/docker-compose.yml up -d tunnel`.
5. **Apunta el túnel a Rust.** Zero Trust > Tunnels > *túnel* > Public Hostname
   `machupeek.com` > Service: `http://web:15633`. Solo queda el conector nuevo,
   que vive en la red de compose de `D:\MACHUPEEK` y resuelve `web`. Verifica la
   sección 5 de inmediato. Ante cualquier fallo, ve a 6.1.

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
| WebSocket del lobby | abrir `/mixton` y el chat del lobby | conecta (`/api/lobby/mixton-ws` tunelizado) y llegan mensajes. Si falla con 403, revisa que `CORS_ORIGINS` incluya el origen del sitio; si falla con 429, es el rate limit (sección 1) |
| Webhook | enviar un webhook de prueba desde Faceit (o esperar uno real) | llega a Node: `docker compose -f /d/Discord_Faceit/docker-compose.yml logs discord-faceit-bot` |
| Logs Rust | `docker compose logs -f web` | sin errores; sin 502 continuos |
| IP real en Node | logs de Node o rate limit de Node | IP del visitante, no la de un contenedor |
| HTTPS sin bucle | `curl -s https://machupeek.com/api/ping` | `"protocol":"https"` y `"secure":true` (ver 5.2) |

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

Los casos de `contrast/cases/admin.toml` necesitan el secreto de prueba
(`CONTRAST_JWT_SECRET` o `--jwt-secret`) para firmar sus cookies. Sin él, la
suite de administración no puede ejecutarse entera. Para que Rust sirva las
lecturas del panel, los dos lados tienen que usar el mismo proyecto o emulador
de Firestore. Algunos casos pueden fallar de forma intermitente sin que haya un
error, porque cada lado tiene sus propias cachés y los datos cambian entre las
dos lecturas:

- `admin logs limit 3`: caché de 30 s en cada lado, y el bot escribe logs sin
  parar.
- `admin stats global` y `admin stats servidor inexistente`: caché de 60 s por
  servidor en cada lado.
- `admin yape transactions`: caché de 30 s en cada lado.
- `admin messages` y los casos `admin matches history*`: Node los guarda en
  caché (plantillas, 6 h; historial, 5 min; `database.js`) y Rust lee siempre
  Firestore, así que difieren si los datos cambiaron dentro de ese plazo.
- `admin users backup*`: sin caché, difieren si alguien se vincula entre las
  dos lecturas.

Ante una diferencia en esos casos, repite con `--only <nombre>` antes de darla
por buena o por mala.

### 5.2 Modo de fallo: redirección HTTPS en bucle

En producción (`BASE_URL` con `https` o `NODE_ENV=production`), Rust responde
`301 https://<host><ruta>` a toda petición cuya **primera entrada** de
`X-Forwarded-Proto` (separada por comas, sin espacios y sin distinguir
mayúsculas) no sea `https`. Con `https, http` no redirige; con `http, https`,
sí. `/api/ping` informa `protocol` y `secure` con la misma regla. cloudflared reenvía esa
cabecera que pone Cloudflare. Si se pierde por el camino (un proxy intermedio,
un Service mal configurado), el sitio entra en un bucle: todas las respuestas
son 301 a la misma URL y el navegador termina con "demasiadas redirecciones".

Diagnóstico desde la red de compose (el nombre de la red suele ser
`machupeek_default`, compruébalo con `docker network ls`):

```bash
docker run --rm --network machupeek_default curlimages/curl:8.10.1 -sI http://web:15633/api/ping
#   -> 301 (esperado sin la cabecera)
docker run --rm --network machupeek_default curlimages/curl:8.10.1 -sI \
  -H 'X-Forwarded-Proto: https' http://web:15633/api/ping
#   -> 200
curl -s https://machupeek.com/api/ping   # tiene que decir "protocol":"https","secure":true
```

En el binario local (Windows, 4.3) es lo mismo con
`curl -sI [-H 'X-Forwarded-Proto: https'] http://127.0.0.1:15633/api/ping`.

Si `https://machupeek.com/api/ping` da 301 o no informa `"secure":true`, **haz
rollback (6.1)**. Poner `NODE_ENV` sin `production` con un `BASE_URL` `http`
desactiva la redirección y solo sirve **para diagnosticar** en un entorno de
prueba. No es un arreglo: también apaga el modo producción. La causa real es
que la cabecera no llega y hay que corregirla.

## 6. Rollback

### 6.1 Después del paso 5 (el conector nuevo está en marcha), menos de un minuto

Activa el rollback si ocurre cualquiera de estas cosas: los checks de la
sección 5 fallan, aparece el bucle 301 de 5.2, hay 502 continuos o el login se
rompe. El rollback no reinicia Node, que sigue en `:3001`.

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
3. Haz la comprobación de recuperación (al final de esta sección).

### 6.2 El conector nuevo no está en marcha (o `web` no llega a `healthy`)

El servicio `tunnel` tiene `depends_on: web: condition: service_healthy`. Si
`web` nunca llega a estar sano (`.env` inválido con reinicios en código 2, error
de montaje, Firestore inaccesible), **el conector nuevo no arranca**. Lo mismo
pasa si cloudflared no registra la conexión. Si eso ocurre después del paso 4
(conector del legado parado), el sitio está caído. Reacciona en cuanto se
cumpla el plazo de 60 s:

1. Arranca de nuevo el conector del legado:
   ```bash
   docker compose -f /d/Discord_Faceit/docker-compose.yml up -d tunnel
   docker compose -f /d/Discord_Faceit/docker-compose.yml logs --tail 20 tunnel   # "Registered tunnel connection"
   ```
2. Pon el Service del hostname en Node en su puerto nuevo, con una dirección que
   resuelva el conector del legado. Dentro de la red de compose del legado es el
   nombre del servicio, `http://discord-faceit-bot:3001`. En Docker Desktop
   también vale `http://host.docker.internal:3001`. Usa el panel o el PUT de 6.1
   con ese `service`.
3. Para lo que quede del stack nuevo:
   `docker compose -f /d/MACHUPEEK/docker-compose.yml down`.

### Comprobación de recuperación (6.1 y 6.2)

```bash
curl -sI https://machupeek.com/api/ping     # 200; sin bucle 301
curl -s  https://machupeek.com/health       # JSON del legado con "discord"
curl -sI https://machupeek.com/             # 200 (el SPA lo sirve Node)
```

Además, un login con Discord en el navegador.

### 6.3 Volver del todo al estado anterior

En una ventana tranquila, porque implica reiniciar el bot:
`docker compose -f /d/MACHUPEEK/docker-compose.yml down`. En el legado, `PORT`
vuelve a su valor anterior, se arranca de nuevo su `tunnel` y el Service vuelve
al destino que anotaste en los requisitos previos, todo sin el mapeo
`80:15633`.

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
  limitan por separado. Lo que pasa por el proxy consume cuota en ambos. Lo
  que sirve `static_files` no consume cuota en ninguno; las rutas SPA y legales,
  que el legado sí limitaba, quedan exentas a propósito (ver sección 1).
- **WebSocket por los middlewares de Rust.** CORS (403 si el origen del sitio
  falta en `CORS_ORIGINS`) y rate limit (cada upgrade cuenta). En el legado no
  pasaban por ningún middleware (ver sección 1).
- **Secretos de Discord exigidos sin uso** (ver 3.1). Fase 2: hacerlos opcionales.
- **Imagen de cloudflared sin fijar ni endurecer.** `docker-compose.yml` usa
  `cloudflare/cloudflared:latest`, como el legado. Recomendación: fijar la
  etiqueta estable vigente tras consultarla en el registro (Docker Hub
  `cloudflare/cloudflared`). Añadir al servicio `tunnel` el mismo
  endurecimiento que `web` (`read_only: true`, `cap_drop: [ALL]`,
  `security_opt: [no-new-privileges:true]`) **solo después de probar** que el
  túnel funciona así en un entorno de prueba. No se ha cambiado aquí porque la
  etiqueta no se pudo verificar.
- **Cierre de conexiones largas.** Al parar, los WebSocket y SSE abiertos se
  cortan a los 8 s y los clientes tienen que reconectar.
- **Fase 2A.** Las escrituras directas a Node dejan desactualizadas las cachés
  de Rust (3.4). Las diferencias conocidas con el legado (400 nuevos, solo
  HS256, acceso entre servidores heredado...) están en `docs/riesgos.md`,
  sección «Fase 2A».

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
