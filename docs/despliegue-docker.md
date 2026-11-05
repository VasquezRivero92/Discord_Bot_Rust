# Encender MachuPeek con Docker (machupeek.com)

Stack: **web** (Rust: web + bot de Discord + lobby del Mixton) y **tunnel** (Cloudflare Zero
Trust hacia `machupeek.com`). No hay contenedor de Node.

```
visitante -> Cloudflare -> cloudflared (tunnel) -> web (Rust, :15633)
```

## 1. Archivos que debe haber junto a `docker-compose.yml`

| Archivo | Qué es | En git |
|---|---|---|
| `.env` | variables (copiar de `.env.example` y rellenar) | no |
| `firebase-service-account.json` | clave de cuenta de servicio de Firestore | no |
| `clave_cs2.pem` | clave SSH del servidor CS2 | no |
| `public/` | build del frontend (debe traer `public/index.html`) | no |

Nada de lo anterior va dentro de la imagen: se monta en solo lectura al arrancar.

**Crear la carpeta de subidas antes del primer arranque:**

```bash
mkdir -p public/uploads
```

`public/` se monta en solo lectura y las imágenes del chat van a un volumen que se monta encima en
`public/uploads`. Docker necesita que esa carpeta exista en el host: sin ella el contenedor no
arranca (`mkdirat ... read-only file system`).

## 2. El token del túnel (importante)

`TUNNEL_TOKEN` **no** va en `.env` (compose pasa `.env` entero al contenedor web) ni en el
repositorio. Se exporta en la shell:

```bash
export TUNNEL_TOKEN='<token del túnel de Cloudflare>'
```

Si no está definido, `docker compose up` falla con un mensaje claro en vez de arrancar con un
valor por defecto.

> **Rotar el token.** Una versión anterior de `docker-compose.yml` llevaba un token escrito como
> valor por defecto y esa versión estuvo publicada en la rama `fix/lobby-parity` de GitHub. Hay
> que invalidarlo: Cloudflare Zero Trust > Networks > Tunnels > *túnel* > Configure > refrescar el
> token, y usar el nuevo en `export TUNNEL_TOKEN`.

## 3. Variables que importan para machupeek.com

En `.env`:

- `BASE_URL=https://machupeek.com`
- `CORS_ORIGINS=https://machupeek.com` (quitar los `localhost` en producción)
- `STATIC_DIR=/srv/public` (compose ya lo fija)
- `FIRESTORE_PROJECT_ID=<proyecto>`; la clave la monta compose en `/run/secrets/firebase-sa.json`
- `DISCORD_*` (token, client id/secret, guild) y los canales que use el bot
- `TRUST_CF_CONNECTING_IP=1` solo porque el puerto está publicado en `127.0.0.1` y el origen solo
  es alcanzable por el túnel; con `0` el límite de peticiones usa `X-Forwarded-For`.

En Cloudflare, el *Public Hostname* de `machupeek.com` debe apuntar a `http://web:15633`.

## 4. Encender

```bash
export TUNNEL_TOKEN='...'
docker compose up -d --build        # la primera vez compila Rust: tarda varios minutos
docker compose ps                   # web debe quedar (healthy)
curl -s http://127.0.0.1:15633/api/ping
curl -s https://machupeek.com/api/ping
docker compose logs -f web
```

Si el equipo va justo de memoria al compilar: `docker compose build --build-arg CARGO_BUILD_JOBS=2 web`.

Actualizar: `git pull && docker compose up -d --build`. Apagar: `docker compose down` (el volumen
de imágenes del chat se conserva; `docker compose down -v` lo borra).

## 5. Imágenes del chat

El contenedor es de solo lectura. Las imágenes que suben los jugadores al chat del Mixton se
guardan en el volumen `machupeek-uploads` (montado en `/srv/public/uploads`) y sobreviven a los
reinicios y a las actualizaciones. Para respaldarlas:
`docker run --rm -v machupeek-uploads:/d -v "$PWD":/b busybox tar czf /b/uploads.tgz -C /d .`

## 6. Apuestas con saldo (desactivadas por defecto)

La apuesta por defecto es 0. El cobro de apuestas solo se habilita si están definidos **a la
vez** Firestore, el monedero y `CS2_API_SECRET`. Con `CS2_API_SECRET` vacío (lo recomendado
hasta probar todo) las partidas sin apuesta funcionan normal y las de apuesta se rechazan.

Para activarlas:

1. Elegir un secreto largo y aleatorio y ponerlo en `CS2_API_SECRET` (en `.env`).
2. Configurar el plugin MatchZy del servidor CS2 para que envíe `x-cs2-secret: <secreto>` en
   sus eventos hacia `https://machupeek.com/api/matchzy/event` y, si se usan, hacia
   `/api/cs2/match-finish` y `/api/cs2/match-cancel`. Revisar en la documentación de la versión
   de MatchZy instalada el nombre exacto de las variables de cabecera del webhook remoto
   (suelen ser `matchzy_remote_log_header_key` / `matchzy_remote_log_header_value`).
3. Reiniciar `web` y probar con una partida de apuesta mínima en un servidor de ensayo.

Sin el paso 2 las partidas con apuesta no se cierran y la cuota queda retenida.

## 7. Limitaciones conocidas

- **No hay Node.** `LEGACY_URL` (`http://bot:3001`) ya no existe en este compose: cualquier ruta
  que siga dependiendo del legado responde `502`.
- **Recarga con Yape sin migrar:** `/api/lobby/wallet/recharge-yape` y `/api/lobby/wallet` no
  están en Rust (hoy caen al legado, es decir, `502`), y `/webhook/yape-transaction` solo
  registra en el log: **no valida la firma del escáner ni acredita saldo.** Hasta migrarlo, el
  saldo solo se recarga con `quick-add-balance` (admin).
- **Webhook de FACEIT** (`/webhook/faceit`): solo registra el evento.
- **Discord al lanzar partida:** se anuncia en el canal de partidas y se envía `connect <ip>` por
  mensaje directo, pero no se crean canales de voz ni se mueve a los jugadores.
- **Modo FACEIT y torneo** de `mixton-create-match`: no están migrados (solo modo STEAM).
- El estado del lobby vive en memoria: reiniciar `web` lo vacía.
- Las transacciones de Firestore del monedero y del cierre de partida solo se probaron con
  dobles en memoria: probar con el emulador o una base de pruebas antes de usar dinero real.
