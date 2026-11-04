//! Cliente Firestore sobre el crate `firestore` (firestore-rs, gRPC).
//!
//! Solo lectura: obtener un documento, listar una colección y observar sus cambios. Los
//! documentos son los que escribe el bot Node con firebase-admin; se decodifican con el
//! deserializador serde del crate, que entrega los `Timestamp` como texto RFC 3339 y deja en
//! `None` los campos `Option` ausentes.

use crate::error::DataError;
use firestore::gcloud_sdk::google::firestore::v1::target_change::TargetChangeType;
use firestore::gcloud_sdk::google::firestore::v1::{value::ValueType, Value as PbValue};
use firestore::{
    FirestoreCollectionDocuments, FirestoreDb, FirestoreDbOptions, FirestoreDocument,
    FirestoreListenEvent, FirestoreListener, FirestoreListenerTarget,
    FirestoreListenerTargetParams, FirestoreMemListenStateStorage, FirestoreQueryCollection,
    FirestoreQueryDirection, FirestoreQueryFilter, FirestoreQueryFilterCompare,
    FirestoreQueryFilterComposite, FirestoreQueryFilterCompositeOperator, FirestoreQueryOrder,
    FirestoreQueryParams, FirestoreTargetType, FirestoreValue,
};
use futures::{Stream, StreamExt, TryStreamExt};
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Id del único target que registra cada listener de [`FirestoreClient::watch_collection`] y
/// [`FirestoreClient::watch_document`].
const WATCH_TARGET_ID: u32 = 1;

/// Qué observa un listener: una colección entera o un solo documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchScope {
    /// Toda la colección, sin filtros (un target de consulta).
    Collection(String),
    /// Solo `collection/id` (un target de documentos con ese único id): Firestore no envía ni
    /// cobra los cambios de los demás documentos de la colección.
    Document { collection: String, id: String },
}

/// Target de escucha del crate `firestore` para `scope`. Pura (sin red): es exactamente lo que
/// [`FirestoreClient::watch_collection`] / [`FirestoreClient::watch_document`] registran.
pub(crate) fn listen_target_type(scope: &WatchScope) -> FirestoreTargetType {
    match scope {
        WatchScope::Collection(collection) => FirestoreTargetType::Query(
            FirestoreQueryParams::new(FirestoreQueryCollection::Single(collection.clone())),
        ),
        WatchScope::Document { collection, id } => FirestoreTargetType::Documents(
            FirestoreCollectionDocuments::new(collection.clone(), vec![id.clone()]),
        ),
    }
}

/// Tamaño de página de `list_docs` (máximo que acepta Firestore por página: 300 es holgado).
const LIST_PAGE_SIZE: usize = 300;

/// Alias del resultado de la agregación de [`FirestoreClient::count`] (el de Node).
const COUNT_ALIAS: &str = "count";

/// Cambio en un documento de una colección observada; lleva el id del documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeEvent {
    /// Documento visto por primera vez en este stream (incluye la instantánea inicial: cada
    /// documento que ya existe al empezar a observar llega como `Added`).
    Added(String),
    /// Documento ya visto que ha cambiado.
    Modified(String),
    /// Documento borrado o que ha dejado de pertenecer a la colección observada.
    Removed(String),
    /// La vista del stream ya no es fiable entera: el listener se reconectó, Firestore reinició
    /// el target (`RESET`) o el filtro de existencia no cuadró. Quien cachee la colección debe
    /// invalidarla completa (puede llegar varias veces seguidas: debe ser idempotente); los
    /// eventos posteriores vuelven a describir el estado.
    Reset,
    /// La instantánea de esta apertura ya es actual (primer `CURRENT` tras cada `ADD` o `RESET`
    /// del target) y estos son **todos** los ids presentes en la colección, ordenados. Lo
    /// cacheado entre la apertura y este punto pudo borrarse sin evento (la instantánea
    /// simplemente no lo trae): quien cachee debe tratarlo como un `Reset` y además descartar
    /// todo lo que guarde (también copias viejas) de ids que no estén en la lista. Idempotente.
    Synced(Vec<String>),
}

/// Operador de comparación de un filtro de [`Query`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Dirección de ordenación de [`Query::order_by`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

/// Valor con el que se compara un campo en un filtro de [`Query`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Double(f64),
    Bool(bool),
}

impl Value {
    fn to_firestore(&self) -> FirestoreValue {
        let value_type = match self {
            Value::String(v) => ValueType::StringValue(v.clone()),
            Value::Integer(v) => ValueType::IntegerValue(*v),
            Value::Double(v) => ValueType::DoubleValue(*v),
            Value::Bool(v) => ValueType::BooleanValue(*v),
        };
        FirestoreValue::from(PbValue {
            value_type: Some(value_type),
        })
    }
}

/// Consulta estructurada sobre una colección: filtros (todos a la vez, AND), un orden y un
/// límite. `Query::default()` equivale a listar la colección entera (sin orden).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Filtros `(campo, operador, valor)`; se combinan con AND.
    pub filters: Vec<(String, Op, Value)>,
    /// Un único criterio de orden `(campo, dirección)`.
    pub order_by: Option<(String, Dir)>,
    /// Número máximo de documentos a devolver.
    pub limit: Option<u32>,
}

fn compare_filter(field: &str, op: Op, value: &Value) -> FirestoreQueryFilter {
    let field = field.to_string();
    let value = value.to_firestore();
    FirestoreQueryFilter::Compare(Some(match op {
        Op::Eq => FirestoreQueryFilterCompare::Equal(field, value),
        Op::Ne => FirestoreQueryFilterCompare::NotEqual(field, value),
        Op::Lt => FirestoreQueryFilterCompare::LessThan(field, value),
        Op::Le => FirestoreQueryFilterCompare::LessThanOrEqual(field, value),
        Op::Gt => FirestoreQueryFilterCompare::GreaterThan(field, value),
        Op::Ge => FirestoreQueryFilterCompare::GreaterThanOrEqual(field, value),
    }))
}

/// Traduce `q` a los parámetros de consulta del crate `firestore`. Pura (sin red).
fn query_params(collection: &str, q: &Query) -> FirestoreQueryParams {
    let mut filters: Vec<FirestoreQueryFilter> = q
        .filters
        .iter()
        .map(|(field, op, value)| compare_filter(field, *op, value))
        .collect();
    let filter = match filters.len() {
        0 => None,
        1 => filters.pop(),
        _ => Some(FirestoreQueryFilter::Composite(
            FirestoreQueryFilterComposite {
                for_all_filters: filters,
                operator: FirestoreQueryFilterCompositeOperator::And,
            },
        )),
    };
    let order_by = q.order_by.as_ref().map(|(field, dir)| {
        vec![FirestoreQueryOrder {
            field_name: field.clone(),
            direction: match dir {
                Dir::Asc => FirestoreQueryDirection::Ascending,
                Dir::Desc => FirestoreQueryDirection::Descending,
            },
        }]
    });
    FirestoreQueryParams::new(FirestoreQueryCollection::Single(collection.to_string()))
        .opt_filter(filter)
        .opt_order_by(order_by)
        .opt_limit(q.limit)
}

/// Cliente Firestore de solo lectura. Barato de clonar (comparte la conexión).
#[derive(Clone)]
pub struct FirestoreClient {
    db: FirestoreDb,
}

impl FirestoreClient {
    /// Conecta con el proyecto `project_id`.
    ///
    /// - Con `FIRESTORE_EMULATOR_HOST` definida y `credentials = None`, se conecta al emulador
    ///   sin buscar credenciales (el crate envía el token de prueba `owner`).
    /// - Con `credentials = Some(ruta)`, usa esa clave de cuenta de servicio (también contra el
    ///   emulador si la variable está definida).
    /// - Sin variable ni ruta, usa las Application Default Credentials del entorno.
    pub async fn connect(project_id: &str, credentials: Option<&Path>) -> Result<Self, DataError> {
        let options = FirestoreDbOptions::new(project_id.to_string());
        let db = match credentials {
            Some(path) => {
                FirestoreDb::with_options_service_account_key_file(options, path.to_path_buf())
                    .await?
            }
            None => FirestoreDb::with_options(options).await?,
        };
        Ok(Self::from_db(db))
    }

    /// Envuelve un [`FirestoreDb`] ya construido (p. ej. con opciones propias de reintentos o
    /// de tiempo de espera).
    pub fn from_db(db: FirestoreDb) -> Self {
        Self { db }
    }

    /// Acceso directo al FirestoreDb subyacente.
    pub fn db(&self) -> &FirestoreDb {
        &self.db
    }

    /// Lee `collection/id`. `Ok(None)` si el documento no existe.
    pub async fn get_doc<T: DeserializeOwned>(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<T>, DataError> {
        let doc = self
            .db
            .fluent()
            .select()
            .by_id_in(collection)
            .one(id)
            .await?;
        doc.map(|doc| decode_document(&doc).map(|(_, value)| value))
            .transpose()
    }

    /// Lista todos los documentos de `collection` como `(id, valor)`, paginando. Un documento
    /// que no se puede decodificar hace fallar la llamada con [`DataError::Decode`].
    pub async fn list_docs<T: DeserializeOwned>(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, T)>, DataError> {
        let docs = self.list_raw_docs(collection).await?;
        docs.iter().map(decode_document).collect()
    }

    /// Lista todos los documentos de `collection` sin decodificar (para quien necesita los tipos
    /// de Firestore, p. ej. distinguir un `Timestamp` de un texto). Mismo orden que
    /// [`FirestoreClient::list_docs`] (por id de documento).
    pub async fn list_raw_docs(
        &self,
        collection: &str,
    ) -> Result<Vec<FirestoreDocument>, DataError> {
        Ok(self
            .db
            .fluent()
            .list()
            .from(collection)
            .page_size(LIST_PAGE_SIZE)
            .stream_all_with_errors()
            .await?
            .try_collect()
            .await?)
    }

    /// `query.count().get()` de Node: número de documentos de `collection` que cumplen los
    /// filtros de `q` (se ignoran su orden y su límite), con una agregación `COUNT` de Firestore
    /// (una lectura por cada 1000 entradas de índice, sin leer los documentos).
    pub async fn count(&self, collection: &str, q: Query) -> Result<u64, DataError> {
        let params = query_params(
            collection,
            &Query {
                order_by: None,
                limit: None,
                ..q
            },
        );
        let mut select = self.db.fluent().select().from(collection);
        if let Some(filter) = params.filter {
            select = select.filter(move |_| Some(filter.clone()));
        }
        let rows = select
            .aggregate(|a| a.fields([a.field(COUNT_ALIAS).count()]))
            .query()
            .await?;
        count_from_rows(&rows)
    }

    /// Consulta `collection` con los filtros, el orden y el límite de `q` y devuelve los
    /// documentos como `(id, valor)`, decodificados igual que en [`FirestoreClient::list_docs`]
    /// (un documento no decodificable hace fallar la llamada con [`DataError::Decode`]).
    ///
    /// **Índices:** Firestore resuelve con sus índices automáticos las consultas con filtros de un solo campo o con un
    /// único campo ordenado, pero una consulta con un filtro sobre un campo y `order_by` sobre
    /// *otro* campo (p. ej. `guild_id == x` ordenado por `createdAt`) necesita un índice
    /// compuesto, y sin él Firestore responde `FAILED_PRECONDITION` (llega como
    /// [`DataError::Firestore`]). Este cliente nunca crea índices; hay que desplegarlos aparte
    /// (ver "Fase 2A" en `docs/riesgos.md`).
    pub async fn query<T: DeserializeOwned>(
        &self,
        collection: &str,
        q: Query,
    ) -> Result<Vec<(String, T)>, DataError> {
        let params = query_params(collection, &q);
        let mut select = self.db.fluent().select().from(collection);
        if let Some(filter) = params.filter {
            select = select.filter(move |_| Some(filter.clone()));
        }
        if let Some(orders) = params.order_by {
            select = select.order(move |_| orders.clone());
        }
        if let Some(limit) = params.limit {
            select = select.limit(limit);
        }
        let docs = select.query().await?;
        docs.iter().map(decode_document).collect()
    }

    /// Observa los cambios de `collection` (toda la colección, sin filtros).
    ///
    /// El stream es perezoso: el listener se abre al pedir el primer elemento y se cierra al
    /// soltar el stream. Primero llega la instantánea inicial (un `Added` por documento
    /// existente), cerrada por un [`ChangeEvent::Synced`] cuando es actual, y luego los cambios.
    /// Las reconexiones las hace el crate por dentro (cada 5 s tras un fallo, reanudando con el
    /// resume token en memoria) y se señalan con [`ChangeEvent::Reset`]. Se emite un `Err` terminal (y el stream termina) si Firestore da
    /// el target por terminado para siempre (p. ej. `PERMISSION_DENIED` por reglas) o si el bucle
    /// del listener se detiene por un error que el crate no reintenta; quien consume debe volver a
    /// llamar a `watch_collection`. Los errores reintentables nunca llegan al stream.
    pub fn watch_collection(
        &self,
        collection: &str,
    ) -> impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static {
        self.watch(WatchScope::Collection(collection.to_string()))
    }

    /// Como [`FirestoreClient::watch_collection`] pero solo para `collection/id` (el
    /// `doc(id).onSnapshot` de Node): Firestore solo envía (y cobra) los cambios de ese documento.
    ///
    /// Mismo stream y misma semántica: `Added`/`Modified`/`Removed` llevan siempre `id`, cada
    /// apertura (y reconexión, con `Reset`) termina en un `Synced` con `[id]` si el documento
    /// existe o con `[]` si no (un documento que no existe no genera ningún evento propio en la
    /// instantánea), y los errores terminales igual.
    pub fn watch_document(
        &self,
        collection: &str,
        id: &str,
    ) -> impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static {
        self.watch(WatchScope::Document {
            collection: collection.to_string(),
            id: id.to_string(),
        })
    }

    /// Stream de cambios de `scope` (ver [`FirestoreClient::watch_collection`]).
    pub fn watch(
        &self,
        scope: WatchScope,
    ) -> impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static {
        let db = self.db.clone();
        futures::stream::once(async move { start_watch(db, scope).await }).flat_map(|started| {
            match started {
                Ok(events) => events.left_stream(),
                Err(err) => futures::stream::once(async move { Err(err) }).right_stream(),
            }
        })
    }
}

type Listener = FirestoreListener<FirestoreDb, FirestoreMemListenStateStorage>;

/// Abre el listener de `scope` y devuelve el stream de eventos ya traducidos.
async fn start_watch(
    db: FirestoreDb,
    scope: WatchScope,
) -> Result<impl Stream<Item = Result<ChangeEvent, DataError>> + Send + 'static, DataError> {
    let mut listener: Listener = db
        .create_listener(FirestoreMemListenStateStorage::new())
        .await?;
    let target = FirestoreListenerTarget::new(WATCH_TARGET_ID);
    // Lo mismo que `db.fluent().select()...listen().add_target(..)` (sin etiquetas ni opciones
    // de petición), pero con el target construido por `listen_target_type`, que es puro.
    listener.add_target(FirestoreListenerTargetParams::new(
        target.clone(),
        listen_target_type(&scope),
        HashMap::new(),
    ))?;

    let (tx, rx) = mpsc::unbounded_channel::<Result<ChangeEvent, DataError>>();
    let state = Arc::new(Mutex::new(WatchState::new(WATCH_TARGET_ID as i32)));
    let control = listener.control_handle();

    listener
        .start(move |event| {
            let (item, resync) = {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match state.terminal_error(&event, control.has_target(&target)) {
                    Some(err) => (Some(Err(err)), false),
                    None => {
                        let change = state.on_event(event);
                        (change.map(Ok), state.take_resync_request())
                    }
                }
            };
            if resync {
                control.resync_target(target.clone());
            }
            if let Some(item) = item {
                // Si el receptor ya no existe, el stream se soltó y el listener se está cerrando.
                let _ = tx.send(item);
            }
            futures::future::ready(Ok(()))
        })
        .await?;

    Ok(futures::stream::unfold(
        (Some(listener), rx),
        |(listener, mut rx)| async move {
            let mut listener = listener?;
            match rx.recv().await {
                Some(Ok(change)) => Some((Ok(change), (Some(listener), rx))),
                // Error terminal (target terminado por Firestore): se entrega y el stream
                // termina; soltar `listener` aquí detiene su bucle.
                Some(Err(err)) => {
                    drop(listener);
                    Some((Err(err), (None, rx)))
                }
                // El bucle del listener terminó (soltó el callback y con él el emisor):
                // `shutdown` devuelve por qué, si fue por un error.
                None => match listener.shutdown().await {
                    Ok(()) => None,
                    Err(err) => Some((Err(err.into()), (None, rx))),
                },
            }
        },
    ))
}

/// Número de la agregación `COUNT` de [`FirestoreClient::count`] a partir de las filas que
/// devuelve el crate. Pura (sin red).
///
/// Una respuesta sin filas, sin el alias [`COUNT_ALIAS`] o con un valor que no es un entero no
/// negativo es un [`DataError::Decode`], nunca un 0: así quien llama (p. ej. `/stats`) puede caer
/// a su respaldo en lugar de cachear y servir un recuento falso.
fn count_from_rows(rows: &[FirestoreDocument]) -> Result<u64, DataError> {
    let row = rows
        .first()
        .ok_or_else(|| DataError::Decode("la agregación COUNT no devolvió filas".into()))?;
    let value = row.fields.get(COUNT_ALIAS).ok_or_else(|| {
        DataError::Decode(format!(
            "la agregación COUNT no trae el alias {COUNT_ALIAS:?}"
        ))
    })?;
    match value.value_type {
        Some(ValueType::IntegerValue(n)) => u64::try_from(n).map_err(|_| {
            DataError::Decode(format!(
                "la agregación COUNT devolvió un entero negativo: {n}"
            ))
        }),
        ref other => Err(DataError::Decode(format!(
            "la agregación COUNT devolvió un valor que no es entero: {other:?}"
        ))),
    }
}

/// Id de documento (último segmento) de un nombre de recurso de Firestore como
/// `projects/p/databases/(default)/documents/coleccion/id`. `None` si está vacío.
pub fn document_id(resource_name: &str) -> Option<&str> {
    resource_name
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
}

/// Decodifica un documento crudo a `(id, T)`.
pub(crate) fn decode_document<T: DeserializeOwned>(
    doc: &FirestoreDocument,
) -> Result<(String, T), DataError> {
    let id = document_id(&doc.name)
        .ok_or_else(|| DataError::Decode(format!("nombre de documento sin id: {:?}", doc.name)))?
        .to_string();
    let value =
        FirestoreDb::deserialize_doc_to::<T>(doc).map_err(|err| match DataError::from(err) {
            DataError::Decode(msg) => DataError::Decode(format!("{}: {msg}", doc.name)),
            other => other,
        })?;
    Ok((id, value))
}

/// Estado de un stream de `watch_collection`: traduce los eventos crudos del listener del crate
/// (`ListenResponse` de Firestore) a [`ChangeEvent`]. Es puro (sin red) para poder probarlo.
///
/// Firestore no distingue alta de modificación en `DocumentChange`; como los SDK oficiales, se
/// distingue por si el documento ya se había visto en este stream.
pub(crate) struct WatchState {
    target_id: i32,
    /// Documentos que el stream considera presentes ahora.
    known: HashSet<String>,
    /// Se ha recibido ya el primer `ADD` del target (los siguientes son reconexiones).
    added_once: bool,
    /// Falta el `CURRENT` de la apertura en curso: el próximo emite `Synced`. Se arma al crear el
    /// estado, en cada `ADD` (también las reconexiones y la resincronización que pide el filtro
    /// de existencia, que quita y vuelve a añadir el target) y en cada `RESET` del target.
    awaiting_current: bool,
    /// El filtro de existencia no cuadró: hay que pedir al crate que resincronice el target.
    resync_requested: bool,
}

impl WatchState {
    pub(crate) fn new(target_id: i32) -> Self {
        Self {
            target_id,
            known: HashSet::new(),
            added_once: false,
            awaiting_current: true,
            resync_requested: false,
        }
    }

    fn concerns_me(&self, target_ids: &[i32]) -> bool {
        target_ids.contains(&self.target_id)
    }

    fn removed(&mut self, resource_name: &str) -> Option<ChangeEvent> {
        let id = document_id(resource_name)?;
        self.known.remove(id);
        Some(ChangeEvent::Removed(id.to_string()))
    }

    pub(crate) fn on_event(&mut self, event: FirestoreListenEvent) -> Option<ChangeEvent> {
        match event {
            FirestoreListenEvent::DocumentChange(change) => {
                let doc = change.document?;
                if self.concerns_me(&change.removed_target_ids) {
                    self.removed(&doc.name)
                } else if self.concerns_me(&change.target_ids) {
                    let id = document_id(&doc.name)?.to_string();
                    if self.known.insert(id.clone()) {
                        Some(ChangeEvent::Added(id))
                    } else {
                        Some(ChangeEvent::Modified(id))
                    }
                } else {
                    None
                }
            }
            FirestoreListenEvent::DocumentDelete(delete) => {
                if delete.removed_target_ids.is_empty()
                    || self.concerns_me(&delete.removed_target_ids)
                {
                    self.removed(&delete.document)
                } else {
                    None
                }
            }
            FirestoreListenEvent::DocumentRemove(remove) => {
                if remove.removed_target_ids.is_empty()
                    || self.concerns_me(&remove.removed_target_ids)
                {
                    self.removed(&remove.document)
                } else {
                    None
                }
            }
            FirestoreListenEvent::Filter(filter) => {
                let expected = usize::try_from(filter.count).unwrap_or(0);
                if filter.target_id == self.target_id && expected != self.known.len() {
                    self.known.clear();
                    self.resync_requested = true;
                    Some(ChangeEvent::Reset)
                } else {
                    None
                }
            }
            FirestoreListenEvent::TargetChange(change) => {
                // Sin target_ids, el cambio se aplica a todos los targets del stream.
                if !change.target_ids.is_empty() && !self.concerns_me(&change.target_ids) {
                    return None;
                }
                // `CURRENT` = la instantánea de esta apertura ya está entera. Lo cacheado entre la
                // apertura y este punto pudo borrarse sin evento (la instantánea simplemente no
                // lo trae), así que el primer `CURRENT` tras cada `ADD`/`RESET` emite `Synced`
                // con los ids presentes (`known` está completo en ese punto: tras un `RESET` o la
                // resincronización se vació y se volvió a llenar; tras reanudar con token, los
                // borrados intermedios llegan como eventos). Un `CURRENT` repetido no emite nada.
                // Así llega un evento extra por apertura además del `Reset` de la reconexión o
                // del `RESET`; quien consume debe ser idempotente.
                match TargetChangeType::try_from(change.target_change_type).ok()? {
                    TargetChangeType::Add if self.added_once => {
                        self.awaiting_current = true;
                        Some(ChangeEvent::Reset)
                    }
                    TargetChangeType::Add => {
                        self.added_once = true;
                        self.awaiting_current = true;
                        None
                    }
                    TargetChangeType::Reset => {
                        self.known.clear();
                        self.awaiting_current = true;
                        Some(ChangeEvent::Reset)
                    }
                    TargetChangeType::Current if self.awaiting_current => {
                        self.awaiting_current = false;
                        let mut ids: Vec<String> = self.known.iter().cloned().collect();
                        ids.sort();
                        Some(ChangeEvent::Synced(ids))
                    }
                    TargetChangeType::NoChange
                    | TargetChangeType::Current
                    | TargetChangeType::Remove => None,
                }
            }
        }
    }

    /// Error terminal si `event` es la baja definitiva de nuestro target.
    ///
    /// Firestore retira un target con un `REMOVE` que lleva `cause`. El crate decide si lo
    /// vuelve a añadir (causa reintentable) o lo da por terminado para siempre
    /// (`PERMISSION_DENIED`, `NOT_FOUND`, `UNIMPLEMENTED`, y `INVALID_ARGUMENT` /
    /// `FAILED_PRECONDITION` si no se reanudó), y en ese caso lo des-registra **antes** de
    /// llamar al callback. `target_registered` es esa respuesta (`has_target` del crate). Sin
    /// esta señal el stream se quedaría parado para siempre sin error ni fin.
    pub(crate) fn terminal_error(
        &self,
        event: &FirestoreListenEvent,
        target_registered: bool,
    ) -> Option<DataError> {
        let FirestoreListenEvent::TargetChange(change) = event else {
            return None;
        };
        if target_registered
            || TargetChangeType::try_from(change.target_change_type).ok()
                != Some(TargetChangeType::Remove)
            || (!change.target_ids.is_empty() && !self.concerns_me(&change.target_ids))
        {
            return None;
        }
        let cause = change.cause.as_ref()?;
        let code = firestore::gcloud_sdk::tonic::Code::from(cause.code);
        Some(DataError::Firestore(format!(
            "Firestore terminó el listener de la colección ({code:?}, código {}): {}",
            cause.code, cause.message
        )))
    }

    /// Devuelve (y consume) si hay que resincronizar el target desde cero.
    pub(crate) fn take_resync_request(&mut self) -> bool {
        std::mem::take(&mut self.resync_requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use firestore::gcloud_sdk::google::firestore::v1::{
        target_change::TargetChangeType, value::ValueType, ArrayValue, Document, DocumentChange,
        DocumentDelete, DocumentRemove, ExistenceFilter, MapValue, TargetChange, Value as PbValue,
    };
    use firestore::gcloud_sdk::google::rpc::Status as RpcStatus;
    use firestore::gcloud_sdk::prost_types::Timestamp;
    use firestore::gcloud_sdk::tonic::Code;
    use serde::Deserialize;
    use std::collections::HashMap;

    const TARGET: i32 = 1;
    const ROOT: &str = "projects/demo-p/databases/(default)/documents";

    fn name(collection: &str, id: &str) -> String {
        format!("{ROOT}/{collection}/{id}")
    }

    fn val(value_type: ValueType) -> PbValue {
        PbValue {
            value_type: Some(value_type),
        }
    }

    fn doc(id: &str) -> Document {
        Document {
            name: name("faceit_users", id),
            ..Default::default()
        }
    }

    fn changed(id: &str) -> FirestoreListenEvent {
        FirestoreListenEvent::DocumentChange(DocumentChange {
            document: Some(doc(id)),
            target_ids: vec![TARGET],
            removed_target_ids: vec![],
        })
    }

    fn target_change(kind: TargetChangeType) -> FirestoreListenEvent {
        FirestoreListenEvent::TargetChange(TargetChange {
            target_change_type: kind as i32,
            target_ids: vec![TARGET],
            ..Default::default()
        })
    }

    fn filter(count: i32) -> FirestoreListenEvent {
        FirestoreListenEvent::Filter(ExistenceFilter {
            target_id: TARGET,
            count,
            unchanged_names: None,
        })
    }

    // --- document_id -----------------------------------------------------------------------

    #[test]
    fn document_id_is_last_segment_of_resource_name() {
        assert_eq!(document_id(&name("faceit_users", "123")), Some("123"));
    }

    #[test]
    fn document_id_of_subcollection_document() {
        let nested = format!("{ROOT}/guild_settings/g1/members/m9");
        assert_eq!(document_id(&nested), Some("m9"));
    }

    #[test]
    fn document_id_rejects_empty_or_trailing_slash() {
        assert_eq!(document_id(""), None);
        assert_eq!(document_id(&format!("{ROOT}/faceit_users/")), None);
    }

    // --- decode_document --------------------------------------------------------------------

    #[derive(Debug, Deserialize, PartialEq)]
    struct LegacyDoc {
        nickname: String,
        elo: i64,
        verified: bool,
        #[serde(rename = "linkedAt")]
        linked_at: String,
        settings: HashMap<String, String>,
        roles: Vec<String>,
        #[serde(rename = "steamId")]
        steam_id: Option<String>,
        #[serde(default)]
        banned: bool,
    }

    /// Documento como lo escribiría firebase-admin desde Node: tipos mezclados, un Timestamp y
    /// campos ausentes que el tipo Rust declara opcionales o con valor por defecto.
    fn legacy_document() -> Document {
        let mut fields = HashMap::new();
        fields.insert("nickname".into(), val(ValueType::StringValue("neo".into())));
        fields.insert("elo".into(), val(ValueType::IntegerValue(2150)));
        fields.insert("verified".into(), val(ValueType::BooleanValue(true)));
        fields.insert(
            "linkedAt".into(),
            val(ValueType::TimestampValue(Timestamp {
                seconds: 1_793_613_600, // 2026-11-02T10:00:00Z
                nanos: 0,
            })),
        );
        fields.insert(
            "settings".into(),
            val(ValueType::MapValue(MapValue {
                fields: [("lang".to_string(), val(ValueType::StringValue("es".into())))]
                    .into_iter()
                    .collect(),
            })),
        );
        fields.insert(
            "roles".into(),
            val(ValueType::ArrayValue(ArrayValue {
                values: vec![val(ValueType::StringValue("admin".into()))],
            })),
        );
        fields.insert("extraField".into(), val(ValueType::DoubleValue(1.5)));
        Document {
            name: name("faceit_users", "u42"),
            fields,
            ..Default::default()
        }
    }

    #[test]
    fn decode_document_tolerates_node_written_fields() {
        let (id, decoded): (String, LegacyDoc) =
            decode_document(&legacy_document()).expect("decodifica");
        assert_eq!(id, "u42");
        assert_eq!(decoded.nickname, "neo");
        assert_eq!(decoded.elo, 2150);
        assert!(decoded.verified);
        assert!(
            decoded.linked_at.starts_with("2026-11-02T10:00:00"),
            "{}",
            decoded.linked_at
        );
        assert_eq!(decoded.settings.get("lang").map(String::as_str), Some("es"));
        assert_eq!(decoded.roles, vec!["admin".to_string()]);
        assert_eq!(decoded.steam_id, None);
        assert!(!decoded.banned);
    }

    #[test]
    fn decode_document_type_mismatch_is_decode_error() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Wrong {
            nickname: i64,
        }
        let err = decode_document::<Wrong>(&legacy_document()).unwrap_err();
        assert!(matches!(err, DataError::Decode(_)), "{err:?}");
    }

    // --- WatchState: eventos del listener -> ChangeEvent ------------------------------------

    #[test]
    fn first_sighting_is_added_then_modified() {
        let mut state = WatchState::new(TARGET);
        assert_eq!(
            state.on_event(changed("a")),
            Some(ChangeEvent::Added("a".into()))
        );
        assert_eq!(
            state.on_event(changed("a")),
            Some(ChangeEvent::Modified("a".into()))
        );
    }

    #[test]
    fn delete_and_remove_are_removed() {
        let mut state = WatchState::new(TARGET);
        state.on_event(changed("a"));
        let deleted = FirestoreListenEvent::DocumentDelete(DocumentDelete {
            document: name("faceit_users", "a"),
            removed_target_ids: vec![TARGET],
            read_time: None,
        });
        assert_eq!(
            state.on_event(deleted),
            Some(ChangeEvent::Removed("a".into()))
        );
        // Tras borrarse, volver a verlo es un alta nueva.
        assert_eq!(
            state.on_event(changed("a")),
            Some(ChangeEvent::Added("a".into()))
        );

        let removed = FirestoreListenEvent::DocumentRemove(DocumentRemove {
            document: name("faceit_users", "b"),
            removed_target_ids: vec![TARGET],
            read_time: None,
        });
        assert_eq!(
            state.on_event(removed),
            Some(ChangeEvent::Removed("b".into()))
        );
    }

    #[test]
    fn change_leaving_the_target_is_removed() {
        let mut state = WatchState::new(TARGET);
        state.on_event(changed("a"));
        let left = FirestoreListenEvent::DocumentChange(DocumentChange {
            document: Some(doc("a")),
            target_ids: vec![],
            removed_target_ids: vec![TARGET],
        });
        assert_eq!(state.on_event(left), Some(ChangeEvent::Removed("a".into())));
    }

    #[test]
    fn events_for_other_targets_are_ignored() {
        let mut state = WatchState::new(TARGET);
        let other = FirestoreListenEvent::DocumentChange(DocumentChange {
            document: Some(doc("a")),
            target_ids: vec![TARGET + 1],
            removed_target_ids: vec![],
        });
        assert_eq!(state.on_event(other), None);
    }

    fn synced(ids: &[&str]) -> Option<ChangeEvent> {
        Some(ChangeEvent::Synced(
            ids.iter().map(|id| id.to_string()).collect(),
        ))
    }

    #[test]
    fn first_add_is_silent_and_later_adds_are_reset() {
        let mut state = WatchState::new(TARGET);
        assert_eq!(state.on_event(target_change(TargetChangeType::Add)), None);
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            synced(&[])
        );
        assert_eq!(
            state.on_event(target_change(TargetChangeType::NoChange)),
            None
        );
        // Un segundo ADD del mismo target = el listener se reconectó.
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Add)),
            Some(ChangeEvent::Reset)
        );
    }

    #[test]
    fn first_current_of_each_opening_is_synced_with_known_ids() {
        let mut state = WatchState::new(TARGET);
        assert_eq!(state.on_event(target_change(TargetChangeType::Add)), None);
        state.on_event(changed("b"));
        state.on_event(changed("a"));
        // La instantánea inicial ya es actual: lleva todos los ids presentes, ordenados.
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            synced(&["a", "b"])
        );
        // Un CURRENT repetido sin nueva apertura no vuelve a disparar.
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            None
        );
        // Reconexión del crate (reanuda con token: `known` sigue valiendo; el borrado llega
        // como evento): ADD (Reset) y su CURRENT (Synced, uno extra).
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Add)),
            Some(ChangeEvent::Reset)
        );
        let deleted = FirestoreListenEvent::DocumentDelete(DocumentDelete {
            document: name("faceit_users", "b"),
            removed_target_ids: vec![TARGET],
            read_time: None,
        });
        state.on_event(deleted);
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            synced(&["a"])
        );
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            None
        );
    }

    #[test]
    fn current_after_target_reset_is_synced_once_with_resent_ids() {
        let mut state = WatchState::new(TARGET);
        state.on_event(target_change(TargetChangeType::Add));
        state.on_event(changed("a"));
        state.on_event(target_change(TargetChangeType::Current));
        // RESET del target: Firestore reenvía el estado y vuelve a mandar CURRENT.
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Reset)),
            Some(ChangeEvent::Reset)
        );
        state.on_event(changed("c"));
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            synced(&["c"]),
            "lo no reenviado tras el RESET ya no está"
        );
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            None
        );
    }

    #[test]
    fn current_for_other_target_is_ignored() {
        let mut state = WatchState::new(TARGET);
        state.on_event(target_change(TargetChangeType::Add));
        let other = FirestoreListenEvent::TargetChange(TargetChange {
            target_change_type: TargetChangeType::Current as i32,
            target_ids: vec![TARGET + 1],
            ..Default::default()
        });
        assert_eq!(state.on_event(other), None);
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Current)),
            synced(&[])
        );
    }

    #[test]
    fn target_reset_is_reset_and_forgets_known_documents() {
        let mut state = WatchState::new(TARGET);
        state.on_event(changed("a"));
        assert_eq!(
            state.on_event(target_change(TargetChangeType::Reset)),
            Some(ChangeEvent::Reset)
        );
        // Firestore reenvía el estado inicial: vuelve a ser un alta.
        assert_eq!(
            state.on_event(changed("a")),
            Some(ChangeEvent::Added("a".into()))
        );
    }

    fn removed_with_cause(code: Code, message: &str) -> FirestoreListenEvent {
        FirestoreListenEvent::TargetChange(TargetChange {
            target_change_type: TargetChangeType::Remove as i32,
            target_ids: vec![TARGET],
            cause: Some(RpcStatus {
                code: code as i32,
                message: message.to_string(),
                details: vec![],
            }),
            ..Default::default()
        })
    }

    #[test]
    fn remove_with_permanent_cause_is_terminal_error() {
        let mut state = WatchState::new(TARGET);
        let event = removed_with_cause(
            Code::PermissionDenied,
            "Missing or insufficient permissions.",
        );
        // El crate ya des-registró el target (fin definitivo) antes de llamar al callback.
        let err = state
            .terminal_error(&event, false)
            .expect("un target terminado para siempre es un error terminal");
        match err {
            DataError::Firestore(msg) => {
                assert!(msg.contains("PermissionDenied"), "{msg}");
                assert!(
                    msg.contains("Missing or insufficient permissions."),
                    "{msg}"
                );
            }
            other => panic!("se esperaba DataError::Firestore, llegó {other:?}"),
        }
        assert_eq!(state.on_event(event), None);
    }

    #[test]
    fn remove_with_retryable_cause_is_silent_while_target_registered() {
        let mut state = WatchState::new(TARGET);
        let event = removed_with_cause(Code::Unavailable, "backend unavailable");
        // El crate volverá a añadir el target (sigue registrado): el ADD posterior da Reset.
        assert_eq!(state.terminal_error(&event, true), None);
        assert_eq!(state.on_event(event), None);
    }

    #[test]
    fn remove_without_cause_or_for_other_target_is_not_terminal() {
        let state = WatchState::new(TARGET);
        let ack = target_change(TargetChangeType::Remove);
        assert_eq!(state.terminal_error(&ack, false), None);
        let other = FirestoreListenEvent::TargetChange(TargetChange {
            target_change_type: TargetChangeType::Remove as i32,
            target_ids: vec![TARGET + 1],
            cause: Some(RpcStatus {
                code: Code::NotFound as i32,
                message: "gone".into(),
                details: vec![],
            }),
            ..Default::default()
        });
        assert_eq!(state.terminal_error(&other, false), None);
    }

    #[test]
    fn existence_filter_mismatch_is_reset_and_requests_resync() {
        let mut state = WatchState::new(TARGET);
        state.on_event(changed("a"));
        state.on_event(changed("b"));
        assert_eq!(state.on_event(filter(2)), None);
        assert!(!state.take_resync_request());

        assert_eq!(state.on_event(filter(1)), Some(ChangeEvent::Reset));
        assert!(state.take_resync_request());
        assert!(
            !state.take_resync_request(),
            "la petición se consume una vez"
        );
    }

    // --- listen_target_type ----------------------------------------------------------------

    #[test]
    fn collection_scope_is_unfiltered_query_target() {
        match listen_target_type(&WatchScope::Collection("guild_settings".into())) {
            FirestoreTargetType::Query(q) => {
                assert_eq!(
                    q.collection_id,
                    FirestoreQueryCollection::Single("guild_settings".into())
                );
                assert!(q.filter.is_none());
                assert!(q.parent.is_none());
            }
            other => panic!("se esperaba un target de consulta: {other:?}"),
        }
    }

    #[test]
    fn document_scope_is_single_document_target() {
        let scope = WatchScope::Document {
            collection: "faceit_config".into(),
            id: "settings".into(),
        };
        match listen_target_type(&scope) {
            FirestoreTargetType::Documents(docs) => {
                assert_eq!(docs.collection, "faceit_config");
                assert_eq!(docs.documents, vec!["settings".to_string()]);
                assert!(docs.parent.is_none());
            }
            other => panic!("se esperaba un target de documentos: {other:?}"),
        }
    }

    // --- count: decodificación del resultado de la agregación ------------------------------

    fn count_row(fields: &[(&str, ValueType)]) -> Document {
        Document {
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), val(v.clone())))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn count_reads_integer_under_alias() {
        let rows = [count_row(&[(COUNT_ALIAS, ValueType::IntegerValue(42))])];
        assert_eq!(count_from_rows(&rows), Ok(42));
    }

    #[test]
    fn count_legit_zero_is_ok_zero() {
        let rows = [count_row(&[(COUNT_ALIAS, ValueType::IntegerValue(0))])];
        assert_eq!(count_from_rows(&rows), Ok(0));
    }

    #[test]
    fn count_without_rows_is_decode_error() {
        let err = count_from_rows(&[]).unwrap_err();
        assert!(matches!(err, DataError::Decode(_)), "{err:?}");
    }

    #[test]
    fn count_row_without_alias_is_decode_error() {
        let rows = [count_row(&[("otro", ValueType::IntegerValue(7))])];
        let err = count_from_rows(&rows).unwrap_err();
        assert!(matches!(err, DataError::Decode(_)), "{err:?}");
        let err = count_from_rows(&[count_row(&[])]).unwrap_err();
        assert!(matches!(err, DataError::Decode(_)), "{err:?}");
    }

    #[test]
    fn count_non_integer_or_negative_is_decode_error() {
        for bad in [
            ValueType::StringValue("42".into()),
            ValueType::DoubleValue(42.0),
            ValueType::NullValue(0),
            ValueType::IntegerValue(-1),
        ] {
            let rows = [count_row(&[(COUNT_ALIAS, bad.clone())])];
            let err = count_from_rows(&rows).unwrap_err();
            assert!(matches!(err, DataError::Decode(_)), "{bad:?}: {err:?}");
        }
        // Valor sin tipo (campo presente pero vacío).
        let mut row = Document::default();
        row.fields
            .insert(COUNT_ALIAS.to_string(), PbValue { value_type: None });
        let err = count_from_rows(&[row]).unwrap_err();
        assert!(matches!(err, DataError::Decode(_)), "{err:?}");
    }

    // --- query: construcción de la consulta estructurada ------------------------------------

    use firestore::gcloud_sdk::google::firestore::v1::{
        structured_query::{composite_filter, field_filter, filter::FilterType, Direction},
        StructuredQuery,
    };

    fn structured(collection: &str, q: &Query) -> StructuredQuery {
        StructuredQuery::try_from(query_params(collection, q)).expect("consulta válida")
    }

    fn field_filter_of(filter: &structured_query::Filter) -> &structured_query::FieldFilter {
        match filter.filter_type.as_ref().expect("filtro") {
            FilterType::FieldFilter(f) => f,
            other => panic!("se esperaba un filtro de campo, llegó {other:?}"),
        }
    }

    use firestore::gcloud_sdk::google::firestore::v1::structured_query;

    #[test]
    fn query_builds_expected_structured_query() {
        let q = Query {
            filters: vec![
                ("guild_id".into(), Op::Eq, Value::String("g1".into())),
                ("level".into(), Op::Ge, Value::Integer(3)),
            ],
            order_by: Some(("createdAt".into(), Dir::Desc)),
            limit: Some(50),
        };
        let sq = structured("faceit_bot_logs", &q);

        assert_eq!(sq.from.len(), 1);
        assert_eq!(sq.from[0].collection_id, "faceit_bot_logs");
        assert!(!sq.from[0].all_descendants);
        assert_eq!(sq.limit, Some(50));
        assert_eq!(sq.order_by.len(), 1);
        assert_eq!(
            sq.order_by[0].field.as_ref().unwrap().field_path,
            "createdAt"
        );
        assert_eq!(sq.order_by[0].direction, Direction::Descending as i32);

        let filter = sq.r#where.expect("filtro");
        let FilterType::CompositeFilter(composite) = filter.filter_type.expect("compuesto") else {
            panic!("varios filtros deben ir en un AND");
        };
        assert_eq!(composite.op, composite_filter::Operator::And as i32);
        assert_eq!(composite.filters.len(), 2);
        let first = field_filter_of(&composite.filters[0]);
        assert_eq!(first.field.as_ref().unwrap().field_path, "guild_id");
        assert_eq!(first.op, field_filter::Operator::Equal as i32);
        assert_eq!(
            first.value.as_ref().unwrap().value_type,
            Some(ValueType::StringValue("g1".into()))
        );
        let second = field_filter_of(&composite.filters[1]);
        assert_eq!(second.field.as_ref().unwrap().field_path, "level");
        assert_eq!(second.op, field_filter::Operator::GreaterThanOrEqual as i32);
        assert_eq!(
            second.value.as_ref().unwrap().value_type,
            Some(ValueType::IntegerValue(3))
        );
    }

    #[test]
    fn query_single_filter_is_not_wrapped_in_composite() {
        let q = Query {
            filters: vec![("status".into(), Op::Eq, Value::String("pendiente".into()))],
            ..Query::default()
        };
        let sq = structured("orders", &q);
        let filter = sq.r#where.expect("filtro");
        let f = field_filter_of(&filter);
        assert_eq!(f.field.as_ref().unwrap().field_path, "status");
        assert_eq!(f.op, field_filter::Operator::Equal as i32);
    }

    #[test]
    fn query_default_has_no_filter_order_or_limit() {
        let sq = structured("claims", &Query::default());
        assert!(sq.r#where.is_none());
        assert!(sq.order_by.is_empty());
        assert_eq!(sq.limit, None);
    }

    #[test]
    fn query_values_and_ops_map_to_firestore_types() {
        let cases = [
            (Op::Eq, field_filter::Operator::Equal),
            (Op::Ne, field_filter::Operator::NotEqual),
            (Op::Lt, field_filter::Operator::LessThan),
            (Op::Le, field_filter::Operator::LessThanOrEqual),
            (Op::Gt, field_filter::Operator::GreaterThan),
            (Op::Ge, field_filter::Operator::GreaterThanOrEqual),
        ];
        for (op, expected) in cases {
            let q = Query {
                filters: vec![("f".into(), op, Value::Bool(true))],
                order_by: Some(("f".into(), Dir::Asc)),
                limit: None,
            };
            let sq = structured("c", &q);
            let filter = sq.r#where.clone().expect("filtro");
            let f = field_filter_of(&filter);
            assert_eq!(f.op, expected as i32);
            assert_eq!(
                f.value.as_ref().unwrap().value_type,
                Some(ValueType::BooleanValue(true))
            );
            assert_eq!(sq.order_by[0].direction, Direction::Ascending as i32);
        }
        assert_eq!(
            Value::Double(1.5).to_firestore().value.value_type,
            Some(ValueType::DoubleValue(1.5))
        );
    }
}
