# Migración de Discord_Faceit (Node.js) a Rust: diseño

Fecha: 2026-11-02
Estado: pendiente de revisión

## 1. Objetivo y alcance

**Objetivo:** reducir el consumo de RAM/CPU del servidor y la latencia de la web, migrando de forma incremental el backend de `D:\Discord_Faceit` (Node.js) a Rust, sin cambiar el comportamiento visible para la comunidad.

**Criterio de éxito:** menor RAM en reposo y menor p50/p95 en los endpoints principales, medidos contra una línea base tomada antes de migrar.

**Dentro del alcance**
- Servidor web/API (116 endpoints en `routes/`), WebSocket del lobby, SSE de Halloween.
- Webhooks (Faceit, Yape, MatchZy) y servicios (roles, hub bans, EC2/SSH/MCSM, Yape).
- Bot de Discord: 18 comandos, 5 eventos y voz.

**Fuera del alcance**
- Reescribir los frontends React (`web_panel_src`, `admin_panel_src`); Rust sirve sus estáticos ya compilados.
- Cambiar el esquema de Firestore.
- Migrar el Steam bot (`steamBot.js`, `server.js`) a Rust; queda como sidecar Node.
- Funciones nuevas.

## 2. Ubicación

- `D:\MACHUPEEK`: proyecto nuevo en Rust, con su propio repo git.
- `D:\Discord_Faceit`: código legacy. Sigue corriendo en Node (bot, rutas aún no migradas, Steam bot). Se usa solo como referencia y no se modifica desde este proyecto.
- Los estáticos compilados (`public/`, `public/admin/`) se copian o se referencian por configuración (`STATIC_DIR`). Se decide al implementar la fase 1.

## 3. Arquitectura

```
Cloudflare Tunnel
      │
      ▼
 ┌─────────────┐   ruta aún no migrada    ┌──────────────────┐
 │ Rust (axum) │ ───── proxy inverso ───▶ │ Node (legacy)    │
 │  :15633     │                          │  bot + rutas     │
 └─────────────┘                          │  restantes :3001 │
   │   │                                  └──────────────────┘
   │   └─ Discord REST (token del bot, sin gateway)
   └─ Firestore                            ┌──────────────────┐
                                           │ Node: Steam bot  │
                                           │  :15634          │
                                           └──────────────────┘
```

- Rust recibe todo el tráfico. Cada ruta migrada se atiende en Rust; las demás se reenvían a Node (incluidos los WebSockets). Revertir una ruta es quitarla de Rust.
- Rust habla con Discord por REST con el token del bot (`twilight-http` o `serenity::http`), sin gateway. Lo que dependa del caché del gateway (listas de miembros) se resuelve por endpoint: REST con caché propio, o consulta a Node mientras el bot siga allí.
- El bot de Discord (gateway, comandos, voz) permanece en Node hasta la fase 3.
- El Steam bot es un sidecar Node permanente; Rust consume su API (`/api/matchmaking/*`, con API key).
- Sesiones: Rust valida los JWT y cookies que emite Node (mismo `ADMIN_JWT_SECRET` y claims), de modo que nadie pierde la sesión durante el cambio. El inventario de endpoints (fase 0) fija los claims exactos.

## 4. Capa de datos

**Situación actual:** unos 15 cachés en memoria con TTL de 5 min a 6 h, invalidados en el mismo proceso al escribir. Tres colecciones usan `onSnapshot` (`settings`, `guild_settings`, `active_matches`). Existe manejo de cuota de Firestore excedida. Colecciones principales: `faceit_users`, `guild_settings`, `faceit_active_matches`, `faceit_matches_history`, `faceit_support_calls`, `claims`, `yape_*`, `orders`, `hub_bans_cache`, `halloween_*`.

**Problema:** si Node y Rust escriben las mismas colecciones, el caché de un proceso queda desactualizado respecto a las escrituras del otro.

**Diseño**
1. **Un solo dueño de escritura por colección** durante la transición. Si Rust necesita una escritura que pertenece a Node, la envía por `/internal/*` (protegida con clave compartida) y la invalidación sigue siendo local.
2. **Listeners en Rust** (equivalente de `onSnapshot`) para lo que Rust lee pero no escribe, empezando por `settings`, `guild_settings` y `active_matches`.
3. **Caché con `moka`** (TTL, tamaño máximo, invalidación explícita). Cada módulo de datos expone una interfaz (`UsersRepo`, `GuildSettingsRepo`, `MatchesRepo`, etc.) con Firestore y caché detrás. Las rutas no tocan Firestore directamente, y los tests usan un repositorio falso.
4. **Cliente de Firestore:** crate `firestore` (firestore-rs, gRPC con service account), por sus listeners, transacciones y batch writes. Riesgo: no es un SDK oficial de Google.
5. **Sin cambios de esquema.** Mismos documentos, campos, `firestore.rules` e índices, lo que permite revertir cualquier fase.
6. **Lobby:** el estado en memoria (`LOBBY_STATE`) y su WebSocket tienen un único dueño. Al migrar el lobby en la fase 2, Node deja de tocarlo.

## 5. Fases

**Fase 0: base y medición**
- Workspace Rust en `D:\MACHUPEEK` (crates `web`, `data`, `common`) con `cargo fmt`, `clippy` y `test`.
- Línea base: RAM en reposo y p50/p95 de los endpoints principales (`/api/ping`, lectura del lobby, `/api/admin/*` más usados, leaderboard).
- Inventario de los 116 endpoints: método, ruta, auth, colecciones que toca y dependencia de Discord.

**Fase 1: Rust como puerta de entrada y servidor web**
- Axum con proxy transparente a Node (incluidos WebSockets).
- Estáticos, redirects de rutas públicas (`/terms`, `/privacy`, `/support`) y SPA.
- `/auth/*` (OAuth de Discord, JWT, cookies compatibles) y los 2 GET de `routes/publicApi.js` (`/products`, `/reclamaciones/:code`; los 2 POST escriben y siguen en Node) primero; luego `routes/api.js` (56 rutas) y `routes/cs2Api.js`, endpoint por endpoint.
- Equivalentes de `helmet`, `cors` y rate limiting (`tower-http`, `governor`).
- Criterio de salida: tests de contraste pasan y p95 y RAM son iguales o mejores que la línea base.

**Fase 2: lobby, webhooks y servicios**
- `routes/lobby.js` (48 rutas) con WebSocket y `LOBBY_STATE`.
- Webhooks de Faceit, Yape y MatchZy con la misma verificación de firma.
- Servicios: roles, hub bans, EC2/SSH/MCSM, Halloween (SSE).
- El bot Node llama a Rust por `/internal/*` para lo que antes eran funciones locales.

**Fase 3: bot de Discord en Rust**
- `poise`/`serenity` para los 18 comandos y 5 eventos; `songbird` para voz.
- Comandos de solo lectura primero (`/info`, `/stats`, `/leaderboard`); voz y TTS al final (ffmpeg y yt-dlp como hoy).
- Al terminar se apaga Node salvo el sidecar de Steam y se elimina el proxy.

Cada fase es un despliegue independiente. El rollback consiste en quitar las rutas de Rust para que el proxy las envíe a Node.

## 6. Manejo de errores

- Un tipo de error por crate con `thiserror`; `anyhow` solo en `main`.
- Errores de Firestore, Discord y Faceit se traducen a los mismos códigos HTTP y formato JSON que hoy.
- Cuota de Firestore excedida: servir desde caché y registrar, como el comportamiento actual.
- Node caído: el proxy devuelve 502 sin afectar a Rust.
- Los secretos se leen del mismo `.env` y se validan al arrancar (referencia: `validateConfig`).

## 7. Pruebas

- **Contraste con Node:** el mismo request contra Node y Rust debe dar la misma respuesta (estado, cuerpo, cabeceras relevantes). Se ejecuta contra un proyecto de Firestore de pruebas o el emulador, nunca contra producción.
- **Unitarias:** repositorios con Firestore falso; JWT/cookies compatibles en ambos sentidos; firmas de webhook con vectores reales.
- **Integración:** lobby con WebSocket (ciclo de listos, draft, creación de partida); OAuth de Discord simulado.
- **Carga:** `oha` o `k6` sobre los endpoints de la línea base, comparando RAM y p95 antes y después.
- **Bot (fase 3):** comandos probados contra un servidor de Discord de pruebas.

## 8. Riesgos

- Firestore sin SDK oficial en Rust (`firestore-rs`): validar listeners y transacciones temprano en la fase 0.
- Endpoints que dependen del caché del gateway de Discord: resolver uno a uno en el inventario.
- Lobby con estado en memoria y WebSocket: único dueño por fase.
- Compatibilidad de sesiones (JWT y cookies) entre Node y Rust.
- La ganancia de latencia depende más de reducir lecturas a Firestore que de Rust en sí; la línea base permite comprobarlo.
- Seguridad: `docker-compose.yml` de `D:\Discord_Faceit` contiene en texto plano el `TUNNEL_TOKEN` de Cloudflare; moverlo a variable de entorno y rotarlo si ese archivo se ha compartido.
