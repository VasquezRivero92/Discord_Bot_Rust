# Riesgos: Firestore desde Rust (Tarea 5)

Punto de decisión de la Fase 0: ¿sirve el crate `firestore` (firestore-rs) como capa de datos?

**Conclusión provisional: sí, con la verificación en vivo pendiente.** El crate cubre las tres
operaciones que necesitamos (`get`, `list` y listeners de colección) y admite el emulador. Todo
compila y las pruebas sin red pasan, pero en este entorno **no hay emulador de Firestore ni Java**,
así que nada que hable con Firestore se ha ejecutado de verdad. Antes de dar por cerrada la
decisión (y antes de apoyarse en el listener en la Tarea 11), hay que ejecutar las pruebas de
emulador (ver "Cómo verificar lo pendiente"). No se propone por ahora un cliente alternativo: no
se ha encontrado nada que lo justifique.

## Crate y features elegidos

| Dependencia | Versión | Features | Motivo |
|---|---|---|---|
| `firestore` | `0.61` (resuelve 0.61.1) | por defecto: `tls-roots`, `auth-default-crypto` | Cliente gRPC con listeners, soporte del emulador y deserializador serde propio. `tls-roots` usa los certificados raíz del sistema; `auth-default-crypto` instala el proveedor criptográfico de rustls que hace falta para firmar con una clave de cuenta de servicio. No activamos `caching*` (la caché es de la Tarea 11) ni `admin` (índices, no lo necesitamos). |
| `futures` | `0.3` | — | `Stream` y combinadores. |
| `serde` | `1` | `derive` | `DeserializeOwned` y los tipos de prueba. |
| `tokio` | workspace | + `sync` (canal mpsc); en dev: `net`, `io-util`, `time` | El canal une el callback del listener con el `Stream`; en pruebas, el PATCH REST al emulador y los timeouts. |
| `thiserror` | `2` | — | `DataError`. |

No hace falta depender de `tonic` directamente: el crate reexporta `firestore::gcloud_sdk::tonic`
(tonic 0.14), así que las pruebas usan exactamente la misma versión de `Status`.

Observaciones del crate 0.61:

- Los traits de bajo nivel (`FirestoreGetByIdSupport`, `FirestoreListingSupport`) son privados; la
  API pública es la *fluent* (`db.fluent().select().by_id_in(..).one(id)`,
  `db.fluent().list().from(..).stream_all_with_errors()`), que es la que usamos.
- Compila en Windows con MSVC (incluido `aws-lc-rs`) sin herramientas adicionales.

## Qué se ha verificado aquí

- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` y
  `cargo fmt --check`, limpios.
- Mapeo de errores (pruebas unitarias con `tonic::Status` reales, pasados por la conversión del
  propio crate): `RESOURCE_EXHAUSTED` → `DataError::QuotaExceeded`, `NOT_FOUND` →
  `DataError::NotFound`, otros códigos → `DataError::Firestore(mensaje)`; error de
  deserialización → `DataError::Decode`.
- Decodificación de un documento como los que escribe firebase-admin (texto, entero, booleano,
  `Timestamp`, mapa, array, un campo de más y campos ausentes): los `Timestamp` llegan como texto
  RFC 3339, los `Option` ausentes como `None`, `#[serde(default)]` funciona y los campos de más
  se ignoran. Un tipo incompatible da `DataError::Decode` con la ruta del documento.
- Extracción del id de documento a partir del nombre de recurso (incluye subcolecciones).
- Traducción pura de los eventos del listener (`ListenResponse` de Firestore) a `ChangeEvent`
  (`WatchState::on_event`), con pruebas para cada caso descrito más abajo.
- Las tres pruebas de emulador compilan y aparecen como ignoradas
  (`cargo test -p data --tests -- --ignored --list`).

## Qué NO se ha verificado (requiere Firestore o el emulador)

- `FirestoreClient::connect` contra el emulador (o contra producción con credenciales).
- `get_doc` y `list_docs` contra un servidor real (`get_doc_returns_none_for_missing`,
  `get_doc_roundtrip`).
- El listener en vivo: que el emulador acepte el target de consulta de colección, que se reciban
  la instantánea inicial y luego `Modified` en menos de 5 s (`watch_emits_modified_after_update`),
  y el comportamiento real de las reconexiones, `RESET` y filtros de existencia.
- Paginación de `list_docs` con más de 300 documentos.
- Comportamiento real ante cuota agotada (el mapeo está probado; el código que devuelve el
  servidor de verdad, no).

### Cómo verificar lo pendiente

```text
firebase emulators:start --only firestore --project demo-machupeek   # localhost:8080
FIRESTORE_EMULATOR_HOST=localhost:8080 cargo test -p data -- --ignored
```

El emulador no necesita datos: cada prueba crea sus documentos con la API REST del emulador
(`PATCH /v1/projects/demo-machupeek/databases/(default)/documents/...`, token `owner`) en
colecciones con nombre único. Las pruebas abortan si `FIRESTORE_EMULATOR_HOST` no está definida,
para no tocar nunca producción.

## Conexión y autenticación (emulador)

`FirestoreClient::connect(project_id, credentials)`:

- `FIRESTORE_EMULATOR_HOST` definida y `credentials = None`: el crate apunta al emulador y no
  busca credenciales (envía el token de prueba `owner`). Es lo que usan las pruebas.
- `credentials = Some(ruta)`: clave de cuenta de servicio de esa ruta
  (`FirestoreDb::with_options_service_account_key_file`); con la variable definida se usa igual,
  contra el emulador.
- Ni variable ni ruta: Application Default Credentials. **Ojo:** en una máquina con ADC
  configuradas eso es producción; ninguna prueba llama a `connect` sin la variable.

`FirestoreClient::from_db(FirestoreDb)` permite construir el cliente con opciones propias del
crate (reintentos, tiempos de espera).

## Listener: cómo funciona y cómo se reconecta

`watch_collection(colección)` crea un `FirestoreListener` del crate con almacenamiento de resume
tokens en memoria (`FirestoreMemListenStateStorage`) y un único target (id 1) que es la consulta
de toda la colección. El callback del crate traduce cada evento con `WatchState` y lo envía por
un canal; el `Stream` devuelto lee de ese canal. El stream es perezoso (el listener se abre al
pedir el primer elemento) y soltarlo cierra el listener. Tipo de retorno:
`impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static` (no hizo falta un `Box`).

`watch_document(colección, id)` es lo mismo con un target de documentos (`DocumentsTarget`) que
lleva solo ese id (el `doc(id).onSnapshot` de Node): Firestore no envía ni cobra los cambios de
los demás documentos de la colección. La traducción es la misma; un documento que no existe no
llega en la instantánea y su `CURRENT` da `Synced([])`. Es lo que usa el watcher de ajustes
globales (`faceit_config/settings`); el target se construye con `listen_target_type`, una función
pura con prueba. No se ha ejecutado contra un servidor (prueba `#[ignore]`
`watch_document_ignores_other_documents`).

Traducción de eventos:

| Evento de Firestore | `ChangeEvent` |
|---|---|
| `DocumentChange` con nuestro target en `target_ids`, documento no visto antes en este stream | `Added(id)` |
| ídem, documento ya visto | `Modified(id)` |
| `DocumentChange` con nuestro target en `removed_target_ids`, `DocumentDelete`, `DocumentRemove` | `Removed(id)` |
| `TargetChange::ADD` (el primero) | nada |
| `TargetChange::ADD` posteriores (el crate volvió a abrir el stream: reconexión) | `Reset` |
| `TargetChange::RESET` (Firestore reenviará el estado completo) | `Reset`, y se olvidan los ids conocidos |
| `ExistenceFilter` cuyo `count` no coincide con los documentos conocidos | `Reset`, se olvidan los ids y se pide al crate `resync_target` (resincronizar desde cero) |
| `CURRENT`, `NO_CHANGE`, `REMOVE` | nada |

Firestore no distingue alta de modificación; como los SDK oficiales, se distingue por si el id ya
se vio en este stream. Consecuencia: la instantánea inicial produce un `Added` por cada documento
existente.

**Reconexión.** El crate gestiona solo las caídas: ante un error reintentable o un cierre del
stream espera 5 s (`retry_delay` por defecto) y vuelve a abrirlo, reanudando con el último resume
token guardado en memoria; si Firestore rechaza el token (`INVALID_ARGUMENT`) lo descarta y
reintenta una vez desde cero. El crate **no** expone un evento explícito de "reconectado", pero
sí entrega al callback todos los `TargetChange`, y al reabrir el stream Firestore confirma de
nuevo el target con un `ADD`. Por eso añadimos la variante **`ChangeEvent::Reset`**: sale de
forma natural del API del listener (segundo `ADD`, `RESET`, filtro de existencia) y le da a la
Tarea 11 la señal para invalidar la caché entera (`watcher_reconnect_invalidates_all`). Al
reanudar con token, Firestore además envía los cambios perdidos como eventos normales.

**Errores terminales.** Si Firestore retira el target con una causa que el crate considera
definitiva (`PERMISSION_DENIED`, `NOT_FOUND`, `UNIMPLEMENTED`, y `INVALID_ARGUMENT` /
`FAILED_PRECONDITION` cuando el target no se reanudó), el crate lo des-registra y deja el bucle
esperando para siempre sin targets: sin más, el stream se quedaría parado sin error ni fin. Por
eso, ante un `REMOVE` con `cause` de nuestro target que ya no está registrado
(`has_target == false`), el stream emite un `Err(DataError::Firestore(..))` terminal con el
código gRPC y el mensaje de la causa, y a continuación termina. Si el target sigue registrado, el
crate lo volverá a añadir y el `ADD` siguiente produce `Reset`. También se emite un `Err`
terminal si el bucle del listener se detiene por un error que el crate no reintenta (petición
inválida tras descartar tokens, parámetros inválidos, pánico), y un error al abrir el listener se
emite como único elemento. En todos los casos, quien consume debe volver a llamar a
`watch_collection`.

Fuera de esos casos, quien consume **no puede saber** que el listener está caído: los errores de
escucha `UNAUTHENTICATED`, `UNAVAILABLE`, `RESOURCE_EXHAUSTED` y un `PERMISSION_DENIED`
transitorio se reintentan cada 5 s para siempre y nunca llegan al stream (en particular,
`QuotaExceeded` nunca aparece en el stream de `watch_collection`), así que el TTL de la caché es
la única cota de lo desactualizados que pueden estar los datos.

## Preguntas abiertas para la Tarea 11

1. **¿Se reconecta de verdad como se espera contra el emulador?** Validarlo con las pruebas
   ignoradas antes de construir la caché encima; idealmente añadir una prueba que reinicie el
   emulador y espere `Reset`.
2. **`Reset` puede llegar repetido** (p. ej. filtro de existencia → `Reset`, y al re-añadir el
   target, otro `Reset` por el nuevo `ADD`). La caché debe tratarlo como idempotente.
3. **Tras una reconexión no se olvidan los ids conocidos** (con token, Firestore solo manda la
   diferencia). Si el crate reanudó sin token, un documento borrado durante la desconexión puede
   quedar como "conocido"; solo afecta a si un evento posterior sale como `Added` o `Modified`
   (y al conteo del filtro de existencia). La caché no debería depender de esa distinción:
   invalidar la clave en `Added`, `Modified` y `Removed` por igual.
4. **Coste de la instantánea inicial**: cada `watch_collection` lee la colección entera al
   empezar (cuenta como lecturas en la cuota). Abrir un solo watcher por colección y compartirlo.
5. **Un listener por llamada**: cada `watch_collection` abre su propio stream gRPC. Si hacen falta
   muchas colecciones, valorar un único `FirestoreListener` con varios targets.
6. **Reintentos internos**: las lecturas del crate reintentan hasta 3 veces (con espera) los
   errores reintentables, incluido `RESOURCE_EXHAUSTED`; `QuotaExceeded` llega al llamador solo
   tras agotar esos reintentos. Si se quiere fallar antes, construir el `FirestoreDb` con
   `max_retries` propio y usar `FirestoreClient::from_db`.
7. **`list_docs` falla entero con un solo documento legado malformado** (`DataError::Decode`
   al primer documento que no encaja en el tipo). Si hay datos legados irregulares, puede hacer
   falta una variante tolerante que omita (y registre) los documentos que no se decodifican.
8. **Watcher terminado**: tras un `Err` terminal el stream acaba; la Tarea 11 debe reabrir el
   watcher (con espera) e invalidar la caché de esa colección.
9. **Dobles de prueba**: no se ha introducido un trait sobre `FirestoreClient`. Para simular
   errores realistas en un doble, `DataError::from(tonic::Status::...)` aplica el mismo mapeo que
   el cliente real. Si la Tarea 11 necesita intercambiar el cliente, el trait de repositorio es
   suyo.

## Fase 2A

- **Índices compuestos (requisito de despliegue; Rust nunca los crea).** `FirestoreClient::query`
  con un filtro sobre un campo y `order_by` sobre *otro* necesita un índice compuesto en Firestore;
  sin él responde `FAILED_PRECONDITION` (llega como `DataError::Firestore`). Combinaciones que usa
  el legacy (`database.js`):
  - `faceit_matches_history`: `guild_id ==` + `createdAt desc`. Está en
    `firestore.indexes.json` del legacy, así que el índice ya se despliega.
  - `faceit_bot_logs` (`getRecentLogs`) y `faceit_support_calls` (`getSupportCalls`): `guild_id ==`
    + `createdAt desc` cuando se pasa `guildId`. **No constan** en `firestore.indexes.json`; no he
    podido comprobar si existen en la consola de Firebase del proyecto real. Esas lecturas ya están
    portadas (`/logs`, `/support-calls`), así que antes de activar Firestore en Rust hay que
    confirmar que el índice (`guild_id` asc, `createdAt` desc) de cada colección está desplegado,
    o añadirlo al archivo de índices del legacy (paso previo en `runbook-corte.md` 3.4). Sin
    `guildId` solo ordenan por `createdAt` (índice automático de un campo).
  - `claims` y `orders` (`getClaims`/`getOrders`): solo filtro `status ==`, sin orden en Firestore
    (el legacy ordena en memoria). No necesitan índice si el repositorio hace lo mismo.
  - `faceit_users` (`mixton_ready ==` + `mixton_guild_id ==`): solo igualdades, índice ya listado en
    `firestore.indexes.json`.
- **Listas vacías que pueden esconder un índice que falta (Tarea 4).** El legacy traga cualquier
  error de `getRecentLogs` y `getSupportCalls` y responde `{ logs: [] }` / `{ calls: [] }` con 200;
  si en producción falta el índice compuesto `guild_id` + `createdAt desc` de `faceit_bot_logs` o
  de `faceit_support_calls`, el panel de un admin de servidor (que siempre filtra por su guild)
  puede llevar tiempo mostrando listas vacías sin que nadie lo note. Los repos de Rust
  (`LogsRepo`, `SupportCallsRepo`) **no** tragan: devuelven el `DataError` (`Firestore` con
  `FAILED_PRECONDITION`, o `QuotaExceeded`) y es la ruta (Tarea 7) la que, para no cambiar la
  respuesta, debe convertirlo en la lista vacía del legacy, pero registrándolo en el log, de modo
  que el índice que falte salga a la luz. `MatchHistoryRepo` reproduce el respaldo del legacy
  (barrido por `createdAt` y filtro en memoria) ante cualquier error salvo `QuotaExceeded`; ahí el
  índice sí consta en `firestore.indexes.json`. `claims`/`orders` no necesitan índices.
- **`/matches/history` sin la caché del legacy.** `getMatchHistory` (`database.js` ~1669) guarda
  cada resultado 5 min (`MATCH_HISTORY_TTL`) por servidor y límite, y comparte la lectura en
  curso entre peticiones iguales. Rust no cachea: cada petición lee hasta 50 documentos, y hasta
  200 si cae al respaldo sin índice (`max(limit * 4, 60)`). Con la cuota agotada Rust responde
  `{ history: [] }`, mientras que el legacy seguiría sirviendo su copia (si la tenía de los
  últimos 5 min). Más lecturas por visita al panel y un historial vacío ante cuota agotada.
- **`/claims`: campos tipados con pérdida.** `Claim` tipa sus campos conocidos con lectores
  tolerantes: un número donde se espera texto sale como texto (`lenient_string`), un valor de
  otro tipo o un `null` se lee como ausente y no sale en el JSON, un objeto esperado que no lo
  es también se omite, y `createdAt` solo se conserva si es una fecha o un objeto (solo
  `updatedAt` y `respondedAt` se reenvían tal cual). Los campos no tipados (`extra`) no pierden
  nada. Node reenvía el documento tal cual (`res.json`), así que con documentos
  irregulares la salida de `/claims` (y de `/claims/:code`) puede diferir de la de Node. Con
  datos bien formados coinciden. El contraste de la Tarea 0 del plan de la fase 2A
  (`contrast`, runbook 5.1) tiene que incluir reclamaciones con esos casos (número en un campo de
  texto, `null`, tipo cambiado) para medir la diferencia antes del corte.
- **Sin verificación en vivo.** `FirestoreClient::query`, `count` y `list_raw_docs`, y el único
  listener que corre en producción, nunca se han ejecutado aquí contra el emulador ni contra
  Firestore real: solo hay pruebas sin red (dobles de los repositorios y construcción de las
  consultas y del target de escucha). Ese listener es el watcher de ajustes globales, que arranca
  `routes::admin` con Firestore configurado y escucha **solo el documento**
  `faceit_config/settings` (`watch_document`), como el `doc('settings').onSnapshot` del legacy; los
  demás documentos de `faceit_config` (`mixton_chat_<guild>`, `mixton_lobby_<guild>`, `panel_*`,
  `status_panel`, `mixton_ready_panel_*`, `messages`), que Node escribe a menudo, no le llegan ni
  cuestan lecturas. El watcher de ajustes por servidor (`FirestoreGuildSettingsRepo`, que sí
  escucharía la colección `guild_settings` entera) existe y está probado con dobles, pero **no se
  lanza en producción**: ninguna ruta montada lo crea. Los índices de arriba, el código que
  devuelve Firestore ante un índice que falta y la agregación `COUNT` de `/stats` están sin
  comprobar contra un servidor. `count` devuelve `DataError::Decode` (no 0) si la respuesta no
  trae el recuento, y `/stats` cae entonces a la lectura completa.
- **Capa de entorno congelada al arrancar.** `effective_config_for_guild` toma la capa de entorno
  de `Config` (`EnvDefaults`), leída una sola vez al arrancar. En Node, `POST /settings` reescribe
  `process.env` en caliente, así que un consumidor futuro en Rust vería los valores anteriores
  hasta reiniciar `web`. Hoy ninguna ruta montada lo usa (`GET /settings` sigue en Node por esto).
- **Lectura entre servidores (heredada, decisión del dueño).** `resolveTargetGuildId` no comprueba
  permisos: un `admin` de servidor puede leer los datos de **cualquier** servidor con `?guildId=` o
  `x-guild-id`, y con `guildId` vacío lee el ámbito global. Incluye `/users/backup`, que devuelve el
  documento completo de cada usuario (teléfono y saldo). `/users/backup` está abierto a cualquier
  admin, como en el legacy. Rust lo replica por paridad; endurecerlo rompe la paridad y necesita
  una decisión del dueño.
- **400 que el legacy no tiene.** Rust responde 400 donde Express haría otra cosa:
  `guildId` como array u objeto de `qs` (`{"error":"guildId inválido"}`); `limit` en forma no
  soportada (`limit[x]=`, `limit[0]=`) y `status` como array u objeto (`limit inválido`,
  `status inválido`; el legacy uniría el array o filtraría por él); y un `:code` de
  `/claims/:code` con codificación `%` mal formada (`400 Bad Request` en texto plano; Express da
  su 400 en HTML). Solo afecta a consultas fabricadas a mano: el panel nunca las envía.
- **Solo HS256.** Rust solo acepta cookies firmadas con HS256; `jsonwebtoken` de Node acepta
  también HS384 y HS512 con el mismo secreto. El legacy solo emite HS256, así que no afecta a
  cookies reales.
- **`auth_token` inválido con `admin_token` válido.** Rust deja pasar la petición con el
  `admin_token`; Node responde 401. Solo afecta a la disponibilidad (ninguna vía de privilegio) y
  está fijado por un test; la paridad estricta es un cambio pequeño si se quiere.
