# Inventario de endpoints del legacy

Generado por `node tools/inventory.mjs <ruta-legacy>`. Las columnas `colecciones`, `usa Discord`, `fase` y `notas` son manuales y se conservan al regenerar (clave: metodo + ruta + origen).

`usa Discord`: REST (fetch/REST de Discord), gateway (cache del gateway de discord.js), no. `fase`: 1, 2, 3 o proxy. `?` = sin clasificar.

## Recuento por archivo (llamadas `router.<verbo>` / `app.<verbo>`)

| origen | prefijo | llamadas | entradas (rutas expandidas) |
|---|---|---|---|
| index.js | - | 23 | 61 |
| routes/api.js | /api/admin | 57 | 58 |
| routes/auth.js | /auth | 6 | 7 |
| routes/cs2Api.js | /api/cs2 | 4 | 4 |
| routes/lobby.js | /api/lobby | 48 | 49 |
| routes/publicApi.js | /api | 4 | 4 |

## index.js

| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |
|---|---|---|---|---|---|---|---|---|
| GET | /health | none | index.js | 280 | - | gateway | proxy | Sigue en el proxy: el cuerpo incluye discord (client.readyAt, estado del gateway) y uptime del proceso Node; Rust no puede reportarlo. Tarea 17: `static_files` lo excluye del catch-all SPA (antes lo tapaba con index.html) |
| GET | /webhook/faceit | none | index.js | 290 | - | no | 2 | Verificacion GET del webhook |
| GET | /webhooks/faceit | none | index.js | 290 | - | no | 2 | Alias de verificacion GET |
| POST | /webhook/faceit | webhookLimiter | index.js | 299 | (via webhooks/faceit.js) | ? | 2 | Webhook Faceit |
| POST | /webhooks/faceit | webhookLimiter | index.js | 299 | (via webhooks/faceit.js) | ? | 2 | Alias del webhook Faceit |
| POST | /webhook/yape-transaction | yapeWebhookLimiter | index.js | 317 | yape_operations, yape_transacciones, yape_audit_logs, faceit_users | ? | 2 | Webhook Yape |
| GET | /api/matchmaking/status | requireSteamApiKey | index.js | 350 | - | no | proxy | Steam/matchmaking |
| POST | /api/matchmaking/launch | requireSteamApiKey | index.js | 353 | - | no | proxy | Steam/matchmaking |
| POST | /api/matchzy/event | none | index.js | 360 | ? | ? | 2 | Eventos MatchZy |
| GET | /api/ping | none | index.js | 399 | - | no | 1 | migrado (Tarea 12): diagnostico cookies/protocolo |
| ALL | /api/points | none | index.js | 413 | - | no | 1 | Faceit API (fetchLatestSeasonPoints); publica |
| ALL | /api/club/points | none | index.js | 413 | - | no | 1 | Faceit API (fetchLatestSeasonPoints); publica |
| ALL | /api/leaderboard/points | none | index.js | 413 | - | no | 1 | Faceit API (fetchLatestSeasonPoints); publica |
| GET | /admin | none | index.js | 458 | ? | ? | ? |  |
| GET | /admin/* | none | index.js | 462 | ? | ? | ? |  |
| GET | /terms | none | index.js | 471 | ? | ? | ? |  |
| GET | /terms-of-service | none | index.js | 471 | ? | ? | ? |  |
| GET | /legal/terms | none | index.js | 471 | ? | ? | ? |  |
| GET | /privacy | none | index.js | 476 | ? | ? | ? |  |
| GET | /privacy-policy | none | index.js | 476 | ? | ? | ? |  |
| GET | /legal/privacy | none | index.js | 476 | ? | ? | ? |  |
| GET | /support | none | index.js | 481 | ? | ? | ? |  |
| GET | /help | none | index.js | 481 | ? | ? | ? |  |
| GET | /legal/support | none | index.js | 481 | ? | ? | ? |  |
| GET | / | none | index.js | 508 | ? | ? | ? |  |
| GET | /tienda | none | index.js | 508 | ? | ? | ? |  |
| GET | /tienda/* | none | index.js | 508 | ? | ? | ? |  |
| GET | /productos | none | index.js | 508 | ? | ? | ? |  |
| GET | /productos/* | none | index.js | 508 | ? | ? | ? |  |
| GET | /contacto | none | index.js | 508 | ? | ? | ? |  |
| GET | /terminos | none | index.js | 508 | ? | ? | ? |  |
| GET | /terminos-y-condiciones | none | index.js | 508 | ? | ? | ? |  |
| GET | /devoluciones | none | index.js | 508 | ? | ? | ? |  |
| GET | /politica-de-devoluciones | none | index.js | 508 | ? | ? | ? |  |
| GET | /reclamaciones | none | index.js | 508 | ? | ? | ? |  |
| GET | /libro-de-reclamaciones | none | index.js | 508 | ? | ? | ? |  |
| GET | /libro-de-reclamaciones/* | none | index.js | 508 | ? | ? | ? |  |
| GET | /arena | none | index.js | 508 | ? | ? | ? |  |
| GET | /draft | none | index.js | 508 | ? | ? | ? |  |
| GET | /mixton | none | index.js | 508 | ? | ? | ? |  |
| GET | /mixton/* | none | index.js | 508 | ? | ? | ? |  |
| GET | /checkout | none | index.js | 508 | ? | ? | ? |  |
| GET | /halloween | none | index.js | 516 | ? | ? | ? |  |
| GET | /Halloween | none | index.js | 516 | ? | ? | ? |  |
| GET | /halloween/ | none | index.js | 516 | ? | ? | ? |  |
| GET | /Halloween/ | none | index.js | 516 | ? | ? | ? |  |
| GET | /halloween/admin | none | index.js | 522 | ? | ? | ? |  |
| GET | /Halloween/admin | none | index.js | 522 | ? | ? | ? |  |
| GET | /halloween/admin/ | none | index.js | 522 | ? | ? | ? |  |
| GET | /Halloween/admin/ | none | index.js | 522 | ? | ? | ? |  |
| GET | /api/halloween/status | none | index.js | 528 | ? | ? | ? |  |
| POST | /api/halloween/toggle | none | index.js | 537 | ? | ? | ? |  |
| POST | /api/halloween/reset | none | index.js | 551 | ? | ? | ? |  |
| GET | /api/halloween/stream | none | index.js | 560 | ? | ? | ? |  |
| GET | /halloween/user:id | none | index.js | 564 | ? | ? | ? |  |
| GET | /Halloween/user:id | none | index.js | 564 | ? | ? | ? |  |
| GET | /halloween/User:id | none | index.js | 564 | ? | ? | ? |  |
| GET | /Halloween/User:id | none | index.js | 564 | ? | ? | ? |  |
| GET | /halloween/:id([1-8]) | none | index.js | 564 | ? | ? | ? |  |
| GET | /Halloween/:id([1-8]) | none | index.js | 564 | ? | ? | ? |  |
| GET | * | none | index.js | 577 | ? | ? | ? |  |

## routes/api.js

| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |
|---|---|---|---|---|---|---|---|---|
| GET | /api/admin/me | requireAuth | routes/api.js | 135 | faceit_users | no | proxy | db.getFullUser (faceit_users) sin repo en data; hasSavedPhone no reproducible (Tarea 14) |
| GET | /api/admin/steam/status | requireAuth | routes/api.js | 149 | - | no | proxy | Estado del bot de Steam en memoria |
| POST | /api/admin/steam/guard | requireAuth | routes/api.js | 157 | - | no | proxy | Bot de Steam |
| POST | /api/admin/steam/login | requireAuth | routes/api.js | 177 | - | no | proxy | Bot de Steam |
| POST | /api/admin/steam/reset | requireAuth | routes/api.js | 196 | - | no | proxy | Bot de Steam |
| GET | /api/admin/system | requireAuth | routes/api.js | 206 | faceit_config (settings) | no | 1 | Llama a Faceit API (hub info) |
| GET | /api/admin/faceit-club-queues | requireAuth | routes/api.js | 229 | faceit_users | no | 1 | Faceit API |
| GET | /api/admin/firestore-metrics | requireAuth | routes/api.js | 326 | - | no | 1 | Metricas en memoria del proceso Node; sin equivalente directo en Rust |
| GET | /api/admin/stats | requireAuth | routes/api.js | 336 | faceit_users, faceit_active_matches | no | 1 |  |
| GET | /api/admin/users | requireAuth | routes/api.js | 417 | faceit_users | REST | 1 | guilds.fetch + members.fetch() de todos los miembros + roles.cache; REST: list guild members paginado (members.fetch() sin args usa opcode de gateway en djs) |
| POST | /api/admin/users/:id/unlink | requireAuth | routes/api.js | 470 | faceit_users | gateway? | proxy | Usa cache + fetch de miembros |
| POST | /api/admin/users/sync-all | requireAuth | routes/api.js | 509 | faceit_users, faceit_bot_logs | REST | proxy | Fetch de miembros/nicks |
| POST | /api/admin/users/sync-nicks | requireAuth | routes/api.js | 580 | faceit_users, faceit_bot_logs | REST | proxy | Fetch de miembros/nicks |
| POST | /api/admin/users/:id/sync-nick | requireAuth | routes/api.js | 639 | faceit_users | REST | proxy |  |
| GET | /api/admin/users/backup | requireAuth | routes/api.js | 670 | faceit_users | no | 1 |  |
| POST | /api/admin/users/restore | requireAuth | routes/api.js | 694 | faceit_users, faceit_bot_logs | no | proxy |  |
| POST | /api/admin/users/verify | requireAuth | routes/api.js | 746 | faceit_users, faceit_pending_verifications | REST | proxy | Fetch de miembro y asignacion de rol |
| POST | /api/admin/users/:id/roles | requireAuth | routes/api.js | 790 | faceit_users | REST | proxy | Asigna roles |
| POST | /api/admin/users/:id/balance | requireAuth | routes/api.js | 829 | faceit_users, faceit_bot_logs | no | proxy |  |
| GET | /api/admin/discord/roles | requireAuth | routes/api.js | 869 | - | REST | 1 | guilds.fetch + roles.fetch; REST: GET /guilds/{id}/roles |
| POST | /api/admin/users/:id/admin-role | requireAuth | routes/api.js | 892 | faceit_users | no | proxy |  |
| GET | /api/admin/matches | requireAuth | routes/api.js | 942 | faceit_active_matches | REST | 1 | guilds.fetch + channels.fetch |
| POST | /api/admin/matches/:id/announce-mixton | requireAuth | routes/api.js | 1006 | faceit_active_matches, faceit_bot_logs | gateway? | proxy | Pasa client a helpers; verificar si requiere cache |
| POST | /api/admin/matches/:id/cleanup | requireAuth | routes/api.js | 1065 | faceit_active_matches | REST | proxy | Fetch de canales/mensajes |
| GET | /api/admin/leaderboard | requireAuth | routes/api.js | 1107 | guild_settings (config) | no | 1 | Faceit API |
| ALL | /api/admin/points | requireAuth | routes/api.js | 1146 | - | no | 1 | Faceit API (fetchLatestSeasonPoints); sin Firestore |
| ALL | /api/admin/club/points | requireAuth | routes/api.js | 1146 | - | no | 1 | Faceit API (fetchLatestSeasonPoints); sin Firestore |
| GET | /api/admin/stats/:nickname | requireAuth | routes/api.js | 1178 | faceit_users | no | 1 |  |
| GET | /api/admin/settings | requireAuth | routes/api.js | 1225 | faceit_config (settings), guild_settings | no | 1 |  |
| POST | /api/admin/settings | requireAuth | routes/api.js | 1263 | faceit_config, guild_settings, faceit_bot_logs | gateway? | proxy | Pasa client a helpers; verificar |
| POST | /api/admin/features/:key | requireAuth | routes/api.js | 1376 | guild_settings, faceit_config, faceit_bot_logs | gateway? | proxy | Usa client; verificar |
| POST | /api/admin/features-batch | requireAuth | routes/api.js | 1431 | guild_settings, faceit_config, faceit_bot_logs | no | proxy |  |
| GET | /api/admin/matches/history | requireAuth | routes/api.js | 1486 | faceit_matches_history | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ history }`; `parseInt(limit)` o `20` si es falsy y `safeLimit` 1..50 de `getMatchHistory`; servidor objetivo; error de lectura -> `{ history: [] }`. Sin la caché de 5 min de `database.js` (la invalidan escrituras de Node que Rust no ve). Solo con Firestore configurado |
| DELETE | /api/admin/matches/history/:id | requireAuth | routes/api.js | 1505 | faceit_matches_history, faceit_bot_logs | no | proxy |  |
| GET | /api/admin/matches/:id/demo | requireAuth | routes/api.js | 1521 | faceit_active_matches | no | 1 |  |
| GET | /api/admin/matches/:id/stats | requireAuth | routes/api.js | 1552 | - | no | 1 | Proxy a Faceit API (open.faceit.com) |
| POST | /api/admin/simulate-webhook | requireAuth | routes/api.js | 1575 | - | ? | proxy | Invoca handleFaceitWebhook(payload, client, db); solo global_admin |
| GET | /api/admin/logs | requireAuth | routes/api.js | 1596 | faceit_bot_logs | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ logs }`; caché de 30 s por `"{guild o global}:{limit}"` en la ruta (también guarda el `[]` de un error); `parseInt(limit)` o `50` si es falsy; límite negativo -> `[]`. Solo con Firestore configurado |
| POST | /api/admin/panel/refresh | requireAuth | routes/api.js | 1617 | faceit_config | gateway? | proxy | updateLivePanel(client, channelId): edita mensaje del panel; candidato a fase 3 (bot-only) |
| GET | /api/admin/status | requireAuth | routes/api.js | 1641 | - | gateway? | 1 | client.readyAt + guilds.fetch; estado del bot |
| POST | /api/admin/broadcast | requireAuth | routes/api.js | 1659 | faceit_users, faceit_bot_logs | REST | proxy | Envia DMs/mensajes |
| GET | /api/admin/support-calls | requireAuth | routes/api.js | 1731 | faceit_support_calls | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ calls }`, 50 fijas, servidor objetivo; error de lectura -> `{ calls: [] }`. Solo con Firestore configurado |
| POST | /api/admin/support-calls/:id/resolve | requireAuth | routes/api.js | 1742 | faceit_support_calls, faceit_bot_logs | gateway? | proxy | guilds.cache.get con fallback fetch; envia DM y alerta |
| GET | /api/admin/messages | requireAuth | routes/api.js | 1825 | faceit_config (messages) | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ templates }`; sin documento, las de por defecto; error -> últimas leídas o las de por defecto. Sin la caché de 6 h (la invalida `POST /messages`, que sigue en Node). Solo con Firestore configurado |
| POST | /api/admin/messages | requireAuth | routes/api.js | 1835 | faceit_config (messages), faceit_bot_logs | no | proxy |  |
| GET | /api/admin/guilds | requireAuth | routes/api.js | 1851 | - | gateway? | proxy | client.guilds.cache (gateway), memberCount/iconURL no reproducibles por REST de forma exacta; fallback a DISCORD_GUILD_ID; REST: GET /users/@me/guilds |
| GET | /api/admin/claims | requireAuth | routes/api.js | 1880 | claims | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ claims }`; `status` o `null` si es falsy, `parseInt(limit, 10)` o `100` si es falsy y `slice(0, limit)` (negativo quita los últimos); error -> `{ claims: [] }`. Solo con Firestore configurado |
| GET | /api/admin/claims/:code | requireAuth | routes/api.js | 1892 | claims | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ claim }`; inexistente o error de lectura -> 404 `Reclamación no encontrada`. Solo con Firestore configurado |
| POST | /api/admin/claims/:code/respond | requireAuth | routes/api.js | 1905 | claims, faceit_bot_logs | no | proxy |  |
| DELETE | /api/admin/claims/:code | requireAuth | routes/api.js | 1939 | claims | no | proxy |  |
| GET | /api/admin/orders | requireAuth | routes/api.js | 1956 | orders | no | 1 | migrado (Tarea 7, `routes::admin::simple_reads`): `{ orders }`; como `/claims`; error -> `{ orders: [] }`. Solo con Firestore configurado |
| POST | /api/admin/orders/:id/status | requireAuth | routes/api.js | 1968 | orders, faceit_bot_logs | no | proxy |  |
| GET | /api/admin/yape/transactions | requireAuth | routes/api.js | 1989 | yape_transacciones, yape_operations, yape_audit_logs | no | 1 |  |
| POST | /api/admin/yape/manual-approve | requireAuth | routes/api.js | 2024 | yape_operations, yape_audit_logs, faceit_users, faceit_bot_logs | no | proxy | Transaccion Firestore |
| GET | /api/admin/guilds/:guildId/settings | requireAuth | routes/api.js | 2063 | guild_settings | no | proxy | getEffectiveConfigForGuild mezcla env + settings globales + guild (config/index.js:178), no solo guild_settings (Tarea 14) |
| POST | /api/admin/guilds/:guildId/settings | requireAuth | routes/api.js | 2079 | guild_settings, faceit_config, faceit_bot_logs | gateway? | proxy | Usa client; verificar |
| GET | /api/admin/map-catalog | requireAuth | routes/api.js | 2124 | faceit_config (mixton lobby state) | no | proxy | getMixtonLobbyState escribe estado por defecto si falta el doc y cae a estado por defecto ante error; catálogo estático en lobby.js (Tarea 14) |
| POST | /api/admin/map-catalog | requireAuth | routes/api.js | 2138 | faceit_config (mixton lobby state) | no | proxy |  |

## routes/auth.js

| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |
|---|---|---|---|---|---|---|---|---|
| GET | /auth/discord | none | routes/auth.js | 36 | - | no | 1 | Migrado (Tarea 13, `routes::auth`). Redirect OAuth2 a discord.com: client_id, redirect_uri, response_type=code, scope=identify, state=ruta saneada (`redirect`), prompt=consent. `state` es la ruta de retorno, NO un anti-CSRF (paridad con el legacy) |
| GET | /auth/discord/callback | none | routes/auth.js | 48 | faceit_users (lectura/rol), faceit_active_matches (capitan) | no | proxy | OAuth2 por HTTP a discord.com; escribe rol/guild del usuario y firma cookie; NO migrado (Tarea 13): `db.setUserRoleAndGuild` escribe en Firestore para GLOBAL_ADMIN_DISCORD_IDS (L137, un solo escritor), el rol/servidor salen de `db.getFullUser` (faceit_users) y `db.isUserCaptain` (capitanes, sin repo en `data`); no usa `checkIsAdmin`. Bloques listos para la fase siguiente: `DiscordApi`/`RestDiscordApi`/`is_admin` en `web::discord` |
| GET | /auth/steam | none | routes/auth.js | 280 | - | no | proxy | Redirect OpenID Steam |
| GET | /auth/steam/callback | none | routes/auth.js | 310 | faceit_users, faceit_bot_logs, faceit_pending_verifications | gateway? | proxy | Usa client.guilds.cache.get con fallback fetch (REST); escribe steamId/link |
| POST | /auth/demo-login | none | routes/auth.js | 454 | - | no | proxy | Login demo con credenciales fijas |
| POST | /auth/login | none | routes/auth.js | 454 | - | no | proxy | Alias de demo-login |
| GET | /auth/logout | none | routes/auth.js | 499 | - | no | 1 | Migrado (Tarea 13, `routes::auth`). Borra `auth_token` y `admin_token` (Path=/) y redirige a `/` |

## routes/cs2Api.js

| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |
|---|---|---|---|---|---|---|---|---|
| GET | /api/cs2/current-match | none | routes/cs2Api.js | 16 | cs2_pending_matches, faceit_users | no | proxy | Tambien escribe (saveUser); consumido por plugin CS2 |
| POST | /api/cs2/match-finish | none | routes/cs2Api.js | 120 | faceit_match_stats, faceit_matches_history | no | proxy | Escritura de stats |
| POST | /api/cs2/match-cancel | none | routes/cs2Api.js | 206 | - | no | proxy | Delegado a cs2ServerManager (estado en memoria) |
| GET | /api/cs2/server-info | none | routes/cs2Api.js | 230 | - | no | proxy | Responde serverIp (MIXTON_SERVER_IP + normalizacion) y activeMatch = getCurrentMatch() (estado en memoria de cs2ServerManager, migra en fase 2); no migrable solo con Config |

## routes/lobby.js

| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |
|---|---|---|---|---|---|---|---|---|
| GET | /api/lobby/mixton-chat-history | optionalAuth | routes/lobby.js | 1568 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-chat-send | requireAuth, chatUpload.single('image') | routes/lobby.js | 1578 | ? | ? | 2 |  |
| GET | /api/lobby/me | optionalAuth | routes/lobby.js | 1595 | ? | ? | 2 |  |
| POST | /api/lobby/verify-faceit | requireAuth | routes/lobby.js | 1688 | ? | ? | 2 |  |
| POST | /api/lobby/link-steam | requireAuth | routes/lobby.js | 1785 | ? | ? | 2 |  |
| POST | /api/lobby/sync-steam | requireAuth | routes/lobby.js | 1785 | ? | ? | 2 |  |
| POST | /api/lobby/sync-faceit-from-steam | requireAuth | routes/lobby.js | 1846 | ? | ? | 2 |  |
| GET | /api/lobby/guilds | requireAuth | routes/lobby.js | 1945 | ? | ? | 2 |  |
| GET | /api/lobby/users | requireAuth | routes/lobby.js | 1981 | ? | ? | 2 |  |
| POST | /api/lobby/create-match | requireAuth | routes/lobby.js | 2039 | ? | ? | 2 |  |
| GET | /api/lobby/mixton-state | optionalAuth | routes/lobby.js | 2353 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-setup | requireAuth, verifyGuildAdmin | routes/lobby.js | 2377 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-assign-captain | requireAuth, verifyGuildAdmin | routes/lobby.js | 2430 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-captain-proposal | requireAuth, verifyGuildAccess | routes/lobby.js | 2533 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-captain-ready | requireAuth, verifyGuildAccess | routes/lobby.js | 2599 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-auto-assign-captains | requireAuth, verifyGuildAdmin | routes/lobby.js | 2686 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-force-roulette | requireAuth, verifyGuildAdmin | routes/lobby.js | 2700 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-pick-order | requireAuth, verifyGuildAdmin | routes/lobby.js | 2730 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-start | requireAuth, verifyGuildAdmin | routes/lobby.js | 3059 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-pick | requireAuth, verifyGuildAccess | routes/lobby.js | 3116 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-auto-assign | requireAuth, verifyGuildAdmin | routes/lobby.js | 3207 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-reset | requireAuth, verifyGuildAdmin | routes/lobby.js | 3307 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-start-veto | requireAuth, verifyGuildAccess | routes/lobby.js | 3352 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-ban-map | requireAuth, verifyGuildAccess | routes/lobby.js | 3460 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-cancel-veto | requireAuth, verifyGuildAdmin | routes/lobby.js | 3531 | ? | ? | 2 |  |
| GET | /api/lobby/map-catalog | none | routes/lobby.js | 3554 | ? | ? | 2 |  |
| POST | /api/lobby/map-catalog | requireAuth, verifyGuildAdmin | routes/lobby.js | 3567 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-check-bet-status | requireAuth, verifyGuildAccess | routes/lobby.js | 3605 | ? | ? | 2 |  |
| POST | /api/lobby/quick-add-balance | requireAuth, verifyGuildAdmin | routes/lobby.js | 3711 | ? | ? | 2 |  |
| GET | /api/lobby/player-balance/:discordId | requireAuth, verifyGuildAccess | routes/lobby.js | 3748 | ? | ? | 2 |  |
| POST | /api/lobby/wallet/recharge-yape | requireAuth, yapeRechargeAccountLimiter, yapeRechargeIpLimiter | routes/lobby.js | 3761 | ? | ? | 2 |  |
| GET | /api/lobby/mixton-queues | requireAuth | routes/lobby.js | 3948 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-create-match | requireAuth, verifyGuildAccess | routes/lobby.js | 4730 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-player-ready | requireAuth | routes/lobby.js | 4788 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-games-config | requireAuth, verifyGuildAdmin | routes/lobby.js | 4868 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-rounds-config | requireAuth, verifyGuildAdmin | routes/lobby.js | 4869 | ? | ? | 2 |  |
| POST | /api/lobby/tournament-config | requireAuth, verifyGuildAccess | routes/lobby.js | 4872 | ? | ? | 2 |  |
| POST | /api/lobby/tournament-report-winner | requireAuth, verifyGuildAccess | routes/lobby.js | 4968 | ? | ? | 2 |  |
| GET | /api/lobby/matches-history | optionalAuth | routes/lobby.js | 5030 | ? | ? | 2 |  |
| GET | /api/lobby/matches-history/:matchId | optionalAuth | routes/lobby.js | 5045 | ? | ? | 2 |  |
| GET | /api/lobby/steam-avatar/:steamId | none | routes/lobby.js | 5065 | ? | ? | 2 |  |
| GET | /api/lobby/steam-avatars | none | routes/lobby.js | 5086 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-load-team | requireAuth, verifyGuildAdmin | routes/lobby.js | 5101 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-clear-team | requireAuth, verifyGuildAdmin | routes/lobby.js | 5228 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-reset-players | requireAuth, verifyGuildAdmin | routes/lobby.js | 5270 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-remove-player | requireAuth, verifyGuildAdmin | routes/lobby.js | 5313 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-toggle-command | requireAuth, verifyGuildAdmin | routes/lobby.js | 5359 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-override-tier | requireAuth, verifyGuildAdmin | routes/lobby.js | 5386 | ? | ? | 2 |  |
| POST | /api/lobby/mixton-assign-player | requireAuth, verifyGuildAdmin | routes/lobby.js | 5416 | ? | ? | 2 |  |

## routes/publicApi.js

| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |
|---|---|---|---|---|---|---|---|---|
| GET | /api/products | none | routes/publicApi.js | 108 | - | no | 1 | migrado (Tarea 12): catalogo estatico en crates/web/src/routes/products.json (tools/gen-products.mjs) |
| POST | /api/reclamaciones | none | routes/publicApi.js | 123 | claims | no | proxy | Escritura; sigue en el proxy |
| GET | /api/reclamaciones/:code | none | routes/publicApi.js | 209 | claims | no | 1 | migrado (Tarea 12): lectura por id de documento (claims/{code}). Tarea 17: solo se monta con Firestore configurado (`firestore_mode` distinto de `Disabled`); sin él la ruta sigue en el proxy |
| POST | /api/checkout/purchase | none | routes/publicApi.js | 224 | orders, faceit_users | no | proxy | Escritura (descuenta saldo) |
