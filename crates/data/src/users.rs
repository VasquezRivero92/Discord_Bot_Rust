//! Usuarios vinculados (colección `faceit_users`): modelo y repositorio de lectura con caché.
//!
//! Ids de documento del legacy: `discordId` (documento global) o `${guildId}_${discordId}`
//! (documento por servidor). Lecturas que se portan aquí:
//!
//! * [`UsersRepo::get`] = `getFullUser(discordId)` sin servidor (`database.js` ~2539; lo usan
//!   `routes/api.js` ~642/~797/~836/~915): documento `faceit_users/{discordId}` y, si le falta
//!   `faceit_nickname` o `faceit_player_id`, todos los documentos `where('discordId', '==', id)`
//!   mezclados con las reglas de abajo. Caché de 5 min (`USER_CACHE_TTL`), también para `null`.
//! * [`UsersRepo::all`] = `getAllUsersCached` (`routes/api.js` ~263): la colección entera, cada
//!   documento como `{ id: doc.id, ...doc.data() }`. Caché de 45 s (`ALL_USERS_CACHE_TTL`), sin
//!   listener (colección grande). `/users/backup` (~670) lee lo mismo sin caché y filtra por
//!   [`FaceitUser::guild`] en la ruta.
//! * [`UsersRepo::by_nickname`] = la búsqueda de `getDiscordIdByNickname` (`database.js` ~1244):
//!   `where('faceit_nickname_lower', '==', nick.toLowerCase())` (sin distinguir mayúsculas), con
//!   servidor primero `.where('guild_id', '==', guildId).limit(1)` y, si no hay, **sin filtro de
//!   servidor** `.limit(1)` (respaldo global del legacy). Devuelve el documento (el legacy solo
//!   devuelve el `discordId`: ver [`FaceitUser::discord_id`]). Caché de 5 min.
//!
//! `/stats/:nickname` (~1178) no busca por apodo en `faceit_users` sino por `faceit_player_id`
//! (tras pedir el jugador a FACEIT), con filtro estricto de servidor: no es esta lectura.

use crate::cache::TtlCache;
use crate::error::DataError;
use crate::firestore::{FirestoreClient, Op, Query, Value as QValue};
use crate::model::{node_timestamp_verbatim, present};
use crate::source::{from_json, strip_synthetic, DocSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const COLLECTION: &str = "faceit_users";

/// Vida de [`UsersRepo::all`] en caché (`ALL_USERS_CACHE_TTL` de `routes/api.js`, 45 s).
pub const USERS_ALL_TTL: Duration = Duration::from_secs(45);
/// Vida de [`UsersRepo::get`] y [`UsersRepo::by_nickname`] (`USER_CACHE_TTL`, 5 min).
pub const USER_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_USERS: u64 = 50_000;

/// Documento de `faceit_users` tal como lo devuelve el legacy con `res.json`. Todo lo que no es
/// fecha ni id se conserva tal cual en [`FaceitUser::extra`] (`discordId`, `faceit_nickname`,
/// `faceit_player_id`, `guild_id`, `servidor`, `rol`, `wallet_balance`, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FaceitUser {
    /// `id`: en [`UsersRepo::all`] el del documento salvo que el documento traiga un campo `id`
    /// propio; en las demás lecturas solo si el documento lo trae.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<Value>,
    /// `createdAt`: `serverTimestamp()`; se serializa como `{ _seconds, _nanoseconds }`.
    #[serde(
        rename = "createdAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<Value>,
    /// `updatedAt`: igual que `createdAt`.
    #[serde(
        rename = "updatedAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<Value>,
    /// `linkedAt`: igual que `createdAt` (vinculación de la cuenta).
    #[serde(
        rename = "linkedAt",
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub linked_at: Option<Value>,
    /// `last_hub_match_at`: igual que `createdAt`.
    #[serde(
        default,
        deserialize_with = "node_timestamp_verbatim",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_hub_match_at: Option<Value>,
    /// El resto de campos del documento, sin tocar.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    /// Id del documento leído (no sale en el JSON). `None` en [`UsersRepo::get`], que mezcla
    /// varios documentos.
    #[serde(skip)]
    pub doc_id: Option<String>,
}

/// Valor "verdadero" de JS (`!!v`).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// Texto verdadero de un campo.
fn truthy_str<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

impl FaceitUser {
    /// `discordId` resuelto como el legacy: `data.discordId || (doc.id.includes('_') ?
    /// doc.id.split('_')[1] : doc.id)`.
    pub fn discord_id(&self) -> Option<String> {
        if let Some(id) = truthy_str(&self.extra, "discordId") {
            return Some(id.to_owned());
        }
        let doc_id = self.doc_id.as_deref()?;
        Some(match doc_id.split('_').nth(1) {
            Some(part) if doc_id.contains('_') => part.to_owned(),
            _ => doc_id.to_owned(),
        })
    }

    /// Servidor del usuario: `data.guild_id || data.servidor || data.mixton_guild_id || null`
    /// (filtro de `/users` y `/users/backup`).
    pub fn guild(&self) -> Option<&str> {
        ["guild_id", "servidor", "mixton_guild_id"]
            .iter()
            .find_map(|k| truthy_str(&self.extra, k))
    }

    /// `faceit_nickname`, si es un texto.
    pub fn nickname(&self) -> Option<&str> {
        self.extra.get("faceit_nickname").and_then(Value::as_str)
    }
}

fn decode(doc_id: &str, doc: Value) -> Result<FaceitUser, DataError> {
    let mut user: FaceitUser = from_json(doc_id, strip_synthetic(doc))?;
    user.doc_id = Some(doc_id.to_owned());
    Ok(user)
}

/// Lecturas de `faceit_users` para la administración.
///
/// **Errores:** se devuelven tal cual (el legacy los traga y responde `null`/`[]`/500 según la
/// ruta; la ruta debe reproducirlo). Con [`DataError::QuotaExceeded`] y una copia anterior en
/// caché, el repo con caché sirve esa copia.
#[async_trait]
pub trait UsersRepo: Send + Sync {
    /// `getFullUser(discordId)` sin servidor; `Ok(None)` si no hay ningún dato.
    async fn get(&self, discord_id: &str) -> Result<Option<FaceitUser>, DataError>;
    /// Toda la colección (`{ id: doc.id, ...data }`).
    async fn all(&self) -> Result<Vec<FaceitUser>, DataError>;
    /// Usuario por apodo de FACEIT (sin distinguir mayúsculas), del servidor primero y si no de
    /// cualquiera (ver la documentación del módulo).
    async fn by_nickname(
        &self,
        nick: &str,
        guild: Option<&str>,
    ) -> Result<Option<FaceitUser>, DataError>;
}

// --- Lecturas sin caché (compartidas por el repo real y el doble) -----------------------------

fn eq(field: &str, value: &str) -> (String, Op, QValue) {
    (field.into(), Op::Eq, QValue::String(value.into()))
}

/// Mezcla de `getFullUser` (`database.js` ~2569-2602) sobre el documento global y los
/// documentos del usuario. Pura.
pub(crate) fn merge_full_user(
    discord_id: &str,
    global: Option<Map<String, Value>>,
    lookup: impl FnOnce() -> Vec<Map<String, Value>>,
) -> Option<Map<String, Value>> {
    let mut combined = global.unwrap_or_default();
    if !truthy(combined.get("faceit_nickname")) || !truthy(combined.get("faceit_player_id")) {
        for data in lookup() {
            if truthy(data.get("faceit_nickname")) {
                // combined = { ...data, ...combined }
                let mut merged = data.clone();
                merged.extend(std::mem::take(&mut combined));
                combined = merged;
            }
            if let Some(rol @ Value::String(r)) = data.get("rol") {
                if r == "global_admin" || r == "admin" {
                    combined.insert("rol".into(), rol.clone());
                }
            }
            if let Some(balance) = data.get("wallet_balance") {
                let current = combined.get("wallet_balance");
                if current.is_none() || current.and_then(Value::as_f64) == Some(0.0) {
                    combined.insert("wallet_balance".into(), balance.clone());
                }
            }
            for key in ["phoneNumber", "steam_id_64", "steam_nickname"] {
                if truthy(data.get(key)) && !truthy(combined.get(key)) {
                    combined.insert(key.into(), data[key].clone());
                }
            }
        }
    }
    if combined.is_empty() {
        return None;
    }
    if !truthy(combined.get("discordId")) {
        combined.insert("discordId".into(), Value::String(discord_id.into()));
    }
    Some(combined)
}

fn as_object(doc: Value) -> Map<String, Value> {
    match strip_synthetic(doc) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

pub(crate) async fn load_user<S: DocSource + ?Sized>(
    source: &S,
    discord_id: &str,
) -> Result<Option<FaceitUser>, DataError> {
    if discord_id.is_empty() {
        return Ok(None);
    }
    let global = source.get(COLLECTION, discord_id).await?.map(as_object);
    let needs_lookup = global
        .as_ref()
        .is_none_or(|g| !truthy(g.get("faceit_nickname")) || !truthy(g.get("faceit_player_id")));
    let docs = if needs_lookup {
        let q = Query {
            filters: vec![eq("discordId", discord_id)],
            ..Query::default()
        };
        source.query(COLLECTION, q).await?
    } else {
        Vec::new()
    };
    let merged = merge_full_user(discord_id, global, || {
        docs.into_iter().map(|(_, d)| as_object(d)).collect()
    });
    merged
        .map(|m| {
            let user: FaceitUser = from_json(discord_id, Value::Object(m))?;
            Ok(user)
        })
        .transpose()
}

pub(crate) async fn load_all<S: DocSource + ?Sized>(
    source: &S,
) -> Result<Vec<FaceitUser>, DataError> {
    let docs = source.query(COLLECTION, Query::default()).await?;
    docs.into_iter()
        .map(|(id, doc)| {
            let mut user = decode(&id, doc)?;
            user.id.get_or_insert(Value::String(id));
            Ok(user)
        })
        .collect()
}

/// Consultas de `getDiscordIdByNickname`: con servidor la filtrada y luego la global.
pub(crate) fn nickname_queries(nick: &str, guild: Option<&str>) -> Vec<Query> {
    let lower = nick.to_lowercase();
    let mut queries = Vec::new();
    if let Some(guild) = guild.filter(|g| !g.is_empty()) {
        queries.push(Query {
            filters: vec![eq("faceit_nickname_lower", &lower), eq("guild_id", guild)],
            order_by: None,
            limit: Some(1),
        });
    }
    queries.push(Query {
        filters: vec![eq("faceit_nickname_lower", &lower)],
        order_by: None,
        limit: Some(1),
    });
    queries
}

pub(crate) async fn load_by_nickname<S: DocSource + ?Sized>(
    source: &S,
    nick: &str,
    guild: Option<&str>,
) -> Result<Option<FaceitUser>, DataError> {
    if nick.is_empty() {
        return Ok(None);
    }
    for q in nickname_queries(nick, guild) {
        if let Some((id, doc)) = source.query(COLLECTION, q).await?.into_iter().next() {
            return decode(&id, doc).map(Some);
        }
    }
    Ok(None)
}

// --- Repo con caché -------------------------------------------------------------------------

/// Repo sobre Firestore (o cualquier [`DocSource`]) con cachés TTL y sin listener.
pub struct FirestoreUsersRepo<S = FirestoreClient> {
    source: Arc<S>,
    by_id: TtlCache<String, Option<FaceitUser>>,
    by_nick: TtlCache<(String, Option<String>), Option<FaceitUser>>,
    all: TtlCache<(), Arc<Vec<FaceitUser>>>,
}

impl<S: DocSource + 'static> FirestoreUsersRepo<S> {
    /// Repo con las vidas del legacy ([`USER_TTL`], [`USERS_ALL_TTL`]).
    pub fn new(source: S) -> Self {
        Self::with_ttls(source, USER_TTL, USERS_ALL_TTL)
    }

    /// Repo con vidas propias (`user_ttl` para `get`/`by_nickname`, `all_ttl` para `all`).
    pub fn with_ttls(source: S, user_ttl: Duration, all_ttl: Duration) -> Self {
        Self {
            source: Arc::new(source),
            by_id: TtlCache::new(user_ttl, MAX_USERS),
            by_nick: TtlCache::new(user_ttl, MAX_USERS),
            all: TtlCache::new(all_ttl, 1),
        }
    }

    /// Descarta todo lo cacheado (el `invalidateAdminUsersCache` del legacy tras escribir);
    /// conserva las copias viejas para la cuota.
    pub fn invalidate_all(&self) {
        self.by_id.invalidate_all();
        self.by_nick.invalidate_all();
        self.all.invalidate_all();
    }
}

fn quota(err: &DataError) -> bool {
    matches!(err, DataError::QuotaExceeded)
}

#[async_trait]
impl<S: DocSource + 'static> UsersRepo for FirestoreUsersRepo<S> {
    async fn get(&self, discord_id: &str) -> Result<Option<FaceitUser>, DataError> {
        let source = self.source.clone();
        let id = discord_id.to_owned();
        self.by_id
            .get_or_load_stale_on(
                id.clone(),
                move || async move { load_user(&*source, &id).await },
                quota,
            )
            .await
    }

    async fn all(&self) -> Result<Vec<FaceitUser>, DataError> {
        let source = self.source.clone();
        let users = self
            .all
            .get_or_load_stale_on(
                (),
                move || async move { load_all(&*source).await.map(Arc::new) },
                quota,
            )
            .await?;
        Ok((*users).clone())
    }

    async fn by_nickname(
        &self,
        nick: &str,
        guild: Option<&str>,
    ) -> Result<Option<FaceitUser>, DataError> {
        // La clave incluye el servidor (el legacy usa solo el apodo, y una búsqueda de un
        // servidor podía responder a otro: no se reproduce).
        let guild = guild.filter(|g| !g.is_empty()).map(str::to_owned);
        let key = (nick.to_lowercase(), guild.clone());
        let source = self.source.clone();
        let nick = nick.to_owned();
        self.by_nick
            .get_or_load_stale_on(
                key,
                move || async move { load_by_nickname(&*source, &nick, guild.as_deref()).await },
                quota,
            )
            .await
    }
}

// --- Doble en memoria -----------------------------------------------------------------------

/// Origen en memoria que evalúa las consultas como Firestore (solo filtros `==` sobre textos,
/// orden por id de documento, `limit`).
#[derive(Default)]
pub(crate) struct MemDocs {
    docs: Mutex<std::collections::BTreeMap<String, Value>>,
    error: Mutex<Option<DataError>>,
}

impl MemDocs {
    fn check(&self) -> Result<(), DataError> {
        match self.error.lock().expect("mutex envenenado").clone() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl DocSource for MemDocs {
    async fn query(&self, _collection: &str, q: Query) -> Result<Vec<(String, Value)>, DataError> {
        self.check()?;
        let docs = self.docs.lock().expect("mutex envenenado");
        let mut out: Vec<(String, Value)> = docs
            .iter()
            .filter(|(_, doc)| {
                q.filters
                    .iter()
                    .all(|(field, op, value)| match (op, value) {
                        (Op::Eq, QValue::String(want)) => {
                            doc.get(field).and_then(Value::as_str) == Some(want.as_str())
                        }
                        _ => unimplemented!("MemDocs solo evalúa filtros == sobre textos"),
                    })
            })
            .map(|(id, doc)| (id.clone(), doc.clone()))
            .collect();
        if let Some(limit) = q.limit {
            out.truncate(limit as usize);
        }
        Ok(out)
    }

    async fn get(&self, _collection: &str, id: &str) -> Result<Option<Value>, DataError> {
        self.check()?;
        Ok(self.docs.lock().expect("mutex envenenado").get(id).cloned())
    }
}

/// Doble en memoria para pruebas de otros crates: guarda documentos crudos de `faceit_users` y
/// aplica las mismas lecturas que el repo real (sin caché).
#[derive(Default)]
pub struct FakeUsersRepo {
    docs: MemDocs,
}

impl FakeUsersRepo {
    /// Repo vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Crea o reemplaza el documento `faceit_users/{doc_id}` (JSON como lo guarda el legacy).
    pub fn insert(&self, doc_id: &str, doc: Value) {
        self.docs
            .docs
            .lock()
            .expect("mutex envenenado")
            .insert(doc_id.to_owned(), doc);
    }

    /// Con `Some(err)`, toda lectura falla con `err`; con `None`, vuelve a funcionar.
    pub fn fail_with(&self, error: Option<DataError>) {
        *self.docs.error.lock().expect("mutex envenenado") = error;
    }
}

#[async_trait]
impl UsersRepo for FakeUsersRepo {
    async fn get(&self, discord_id: &str) -> Result<Option<FaceitUser>, DataError> {
        load_user(&self.docs, discord_id).await
    }

    async fn all(&self) -> Result<Vec<FaceitUser>, DataError> {
        load_all(&self.docs).await
    }

    async fn by_nickname(
        &self,
        nick: &str,
        guild: Option<&str>,
    ) -> Result<Option<FaceitUser>, DataError> {
        load_by_nickname(&self.docs, nick, guild).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::FakeSource;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => panic!("objeto"),
        }
    }

    /// Origen que cuenta lecturas y deja cambiar los documentos.
    #[derive(Default)]
    struct Counting {
        docs: MemDocs,
        queries: AtomicUsize,
    }

    #[async_trait]
    impl DocSource for Counting {
        async fn query(&self, c: &str, q: Query) -> Result<Vec<(String, Value)>, DataError> {
            self.queries.fetch_add(1, Ordering::SeqCst);
            self.docs.query(c, q).await
        }
        async fn get(&self, c: &str, id: &str) -> Result<Option<Value>, DataError> {
            self.docs.get(c, id).await
        }
    }

    impl Counting {
        fn put(&self, id: &str, doc: Value) {
            self.docs.docs.lock().unwrap().insert(id.into(), doc);
        }
        fn fail(&self, err: Option<DataError>) {
            *self.docs.error.lock().unwrap() = err;
        }
        fn queries(&self) -> usize {
            self.queries.load(Ordering::SeqCst)
        }
    }

    /// `all()` no tiene listener (colección grande): la caché se cae por TTL o por
    /// `invalidate_all` explícito. Determinista: TTL largo para "cacheado" y TTL de 1 ns
    /// (siempre caducado) para "recarga", sin esperas.
    #[tokio::test]
    async fn users_all_is_cached_until_ttl_or_invalidate() {
        assert_eq!(USERS_ALL_TTL, Duration::from_secs(45));
        let repo =
            FirestoreUsersRepo::with_ttls(Counting::default(), USER_TTL, Duration::from_secs(3600));
        repo.source.put("1", json!({"faceit_nickname": "a"}));
        assert_eq!(repo.all().await.unwrap().len(), 1);
        repo.source.put("2", json!({"faceit_nickname": "b"}));
        assert_eq!(repo.all().await.unwrap().len(), 1, "cacheado");
        assert_eq!(repo.source.queries(), 1);

        repo.invalidate_all();
        assert_eq!(
            repo.all().await.unwrap().len(),
            2,
            "recargado tras invalidar"
        );
        assert_eq!(repo.source.queries(), 2);

        // Ante cuota se sirve la copia anterior; otro error se propaga.
        repo.invalidate_all();
        repo.source.fail(Some(DataError::QuotaExceeded));
        assert_eq!(repo.all().await.unwrap().len(), 2);
        repo.invalidate_all();
        repo.source.fail(Some(DataError::Firestore("x".into())));
        assert!(repo.all().await.is_err());

        // TTL: con 1 ns cada lectura recarga.
        let repo =
            FirestoreUsersRepo::with_ttls(Counting::default(), USER_TTL, Duration::from_nanos(1));
        repo.all().await.unwrap();
        repo.all().await.unwrap();
        assert_eq!(repo.source.queries(), 2, "caducado: recarga");
    }

    #[tokio::test]
    async fn users_all_adds_document_id_and_strips_synthetic_fields() {
        let source = FakeSource::new().reply(Ok(vec![
            (
                "g1_42".into(),
                json!({
                    "discordId": "42", "faceit_nickname": "Neo", "guild_id": "g1",
                    "createdAt": "2026-11-02T10:00:00Z", "linkedAt": null,
                    "_firestore_full_id": "projects/p/databases/(default)/documents/faceit_users/g1_42",
                }),
            ),
            ("7".into(), json!({"id": "propio", "servidor": "g2"})),
        ]));
        let repo = FirestoreUsersRepo::new(source);
        let users = repo.all().await.unwrap();
        assert_eq!(
            repo.source.queries(),
            vec![("faceit_users".into(), Query::default())]
        );
        assert_eq!(
            serde_json::to_value(&users[0]).unwrap(),
            json!({
                "id": "g1_42", "discordId": "42", "faceit_nickname": "Neo", "guild_id": "g1",
                "createdAt": {"_seconds": 1_793_613_600_i64, "_nanoseconds": 0},
                "linkedAt": null,
            })
        );
        assert_eq!(users[0].guild(), Some("g1"));
        assert_eq!(users[1].id, Some(json!("propio")));
        assert_eq!(users[1].guild(), Some("g2"));
        assert_eq!(users[1].discord_id().as_deref(), Some("7"));
    }

    #[test]
    fn discord_id_resolution_matches_legacy() {
        let user = |doc: Value, id: &str| FaceitUser {
            doc_id: Some(id.into()),
            ..serde_json::from_value(doc).unwrap()
        };
        assert_eq!(
            user(json!({"discordId": "1"}), "g_2")
                .discord_id()
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            user(json!({"discordId": ""}), "g_2")
                .discord_id()
                .as_deref(),
            Some("2")
        );
        assert_eq!(user(json!({}), "a_b_c").discord_id().as_deref(), Some("b"));
        assert_eq!(user(json!({}), "99").discord_id().as_deref(), Some("99"));
        assert_eq!(user(json!({}), "g_").discord_id().as_deref(), Some(""));
        // guild(): guild_id || servidor || mixton_guild_id, vacíos como ausentes
        let u = user(
            json!({"guild_id": "", "servidor": null, "mixton_guild_id": "m"}),
            "1",
        );
        assert_eq!(u.guild(), Some("m"));
        assert_eq!(user(json!({}), "1").guild(), None);
    }

    #[tokio::test]
    async fn by_nickname_is_case_insensitive_guild_first_then_any() {
        let fake = FakeUsersRepo::new();
        fake.insert(
            "g1_1",
            json!({"faceit_nickname_lower": "neo", "guild_id": "g1", "discordId": "1"}),
        );
        fake.insert(
            "g2_2",
            json!({"faceit_nickname_lower": "neo", "guild_id": "g2", "discordId": "2"}),
        );
        fake.insert("3", json!({"faceit_nickname_lower": "trinity"}));

        let got = |u: Option<FaceitUser>| u.and_then(|u| u.discord_id());
        assert_eq!(
            got(fake.by_nickname("NeO", Some("g2")).await.unwrap()).as_deref(),
            Some("2")
        );
        // sin coincidencia en el servidor: respaldo global (primer documento por id)
        assert_eq!(
            got(fake.by_nickname("neo", Some("g9")).await.unwrap()).as_deref(),
            Some("1")
        );
        assert_eq!(
            got(fake.by_nickname("neo", None).await.unwrap()).as_deref(),
            Some("1")
        );
        assert_eq!(
            got(fake.by_nickname("Trinity", Some("")).await.unwrap()).as_deref(),
            Some("3")
        );
        // el campo comparado es `faceit_nickname_lower`, no `faceit_nickname`
        fake.insert("4", json!({"faceit_nickname": "morpheus"}));
        assert_eq!(fake.by_nickname("morpheus", None).await.unwrap(), None);
        assert_eq!(fake.by_nickname("", None).await.unwrap(), None);

        assert_eq!(
            nickname_queries("NeO", Some("g2")),
            vec![
                Query {
                    filters: vec![eq("faceit_nickname_lower", "neo"), eq("guild_id", "g2")],
                    order_by: None,
                    limit: Some(1),
                },
                Query {
                    filters: vec![eq("faceit_nickname_lower", "neo")],
                    order_by: None,
                    limit: Some(1),
                },
            ]
        );
    }

    #[tokio::test]
    async fn by_nickname_cache_key_includes_guild() {
        let repo = FirestoreUsersRepo::new(Counting::default());
        repo.source.put(
            "g1_1",
            json!({"faceit_nickname_lower": "neo", "guild_id": "g1"}),
        );
        repo.source.put(
            "g2_2",
            json!({"faceit_nickname_lower": "neo", "guild_id": "g2"}),
        );
        let a = repo.by_nickname("neo", Some("g1")).await.unwrap().unwrap();
        let b = repo.by_nickname("NEO", Some("g2")).await.unwrap().unwrap();
        assert_eq!((a.guild(), b.guild()), (Some("g1"), Some("g2")));
        let n = repo.source.queries();
        repo.by_nickname("Neo", Some("g1")).await.unwrap();
        assert_eq!(
            repo.source.queries(),
            n,
            "cacheado por (apodo en minúsculas, servidor)"
        );
    }

    #[test]
    fn full_user_merge_follows_get_full_user() {
        // Documento global completo: no hay búsqueda.
        let merged = merge_full_user(
            "42",
            Some(obj(
                json!({"faceit_nickname": "n", "faceit_player_id": "p"}),
            )),
            || panic!("no debe buscar"),
        );
        assert_eq!(
            merged,
            Some(obj(
                json!({"faceit_nickname": "n", "faceit_player_id": "p", "discordId": "42"})
            ))
        );

        // Global incompleto: se mezclan los documentos del usuario.
        let merged = merge_full_user(
            "42",
            Some(obj(
                json!({"wallet_balance": 0, "rol": "user", "faceit_player_id": ""}),
            )),
            || {
                vec![
                    obj(
                        json!({"faceit_nickname": "n", "faceit_player_id": "p", "wallet_balance": 5,
                               "steam_id_64": "s1", "extra": 1}),
                    ),
                    obj(
                        json!({"rol": "admin", "phoneNumber": "999", "wallet_balance": 7,
                               "steam_id_64": "s2"}),
                    ),
                    obj(json!({"rol": "mod"})),
                ]
            },
        )
        .unwrap();
        // { ...data, ...combined }: lo del global gana (incluido su faceit_player_id vacío)
        assert_eq!(merged["faceit_nickname"], json!("n"));
        assert_eq!(merged["faceit_player_id"], json!(""));
        assert_eq!(merged["extra"], json!(1));
        // wallet: el global era 0, el primero lo pone a 5 y ya no se toca
        assert_eq!(merged["wallet_balance"], json!(5));
        assert_eq!(merged["rol"], json!("admin"));
        assert_eq!(merged["phoneNumber"], json!("999"));
        assert_eq!(merged["steam_id_64"], json!("s1"));
        assert_eq!(merged["discordId"], json!("42"));

        // Nada en ningún sitio.
        assert_eq!(merge_full_user("42", None, Vec::new), None);
        // Solo documentos de servidor sin apodo: igual cuentan rol/saldo.
        let merged = merge_full_user("42", None, || vec![obj(json!({"wallet_balance": 3}))]);
        assert_eq!(
            merged,
            Some(obj(json!({"wallet_balance": 3, "discordId": "42"})))
        );
    }

    #[tokio::test]
    async fn get_reads_global_doc_then_user_docs_and_caches_misses() {
        let repo = FirestoreUsersRepo::new(Counting::default());
        repo.source
            .put("42", json!({"discordId": "42", "rol": "user"}));
        repo.source.put(
            "g1_42",
            json!({"discordId": "42", "faceit_nickname": "Neo",
                                         "faceit_player_id": "p", "guild_id": "g1"}),
        );
        let u = repo.get("42").await.unwrap().unwrap();
        assert_eq!(u.nickname(), Some("Neo"));
        assert_eq!(u.extra["guild_id"], json!("g1"));
        assert_eq!(u.extra["rol"], json!("user"));
        assert_eq!(u.doc_id, None);

        assert_eq!(repo.get("nadie").await.unwrap(), None);
        let n = repo.source.queries();
        assert_eq!(repo.get("nadie").await.unwrap(), None);
        assert_eq!(repo.source.queries(), n, "el null también se cachea");
        assert_eq!(repo.get("").await.unwrap(), None);

        let fake = FakeUsersRepo::new();
        fake.fail_with(Some(DataError::QuotaExceeded));
        assert_eq!(fake.get("1").await, Err(DataError::QuotaExceeded));
        assert_eq!(fake.all().await, Err(DataError::QuotaExceeded));
    }
}
