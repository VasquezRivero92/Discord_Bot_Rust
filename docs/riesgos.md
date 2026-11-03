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

Solo se emite `Err` en el stream si el bucle del listener se detiene por un error que el crate
no reintenta (petición inválida tras descartar tokens, parámetros inválidos, pánico); después el
stream termina. Un error al abrir el listener se emite como único elemento.

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
7. **Dobles de prueba**: no se ha introducido un trait sobre `FirestoreClient`. Para simular
   errores realistas en un doble, `DataError::from(tonic::Status::...)` aplica el mismo mapeo que
   el cliente real. Si la Tarea 11 necesita intercambiar el cliente, el trait de repositorio es
   suyo.
