//! Servidor objetivo de una petición del panel.
//!
//! Legacy (`routes/api.js:24-43`):
//!
//! ```js
//! router.use((req, res, next) => {
//!   req.guildId = req.query.guildId || req.body.guildId || req.headers['x-guild-id'] || null;
//!   next();
//! });
//! const resolveTargetGuildId = (req) => {
//!   if (req.query?.guildId !== undefined) return req.query.guildId ? req.query.guildId : null;
//!   if (req.body?.guildId !== undefined) return req.body.guildId ? req.body.guildId : null;
//!   if (req.guildId !== undefined && req.guildId !== null) return req.guildId ? req.guildId : null;
//!   if (req.user?.role === 'admin' && req.user.servidor) return req.user.servidor;
//!   return null;
//! };
//! ```
//!
//! Al tercer paso solo se llega sin `guildId` en la query ni en el cuerpo, así
//! que `req.guildId` es la cabecera `x-guild-id` (o `null`).
//!
//! **No es una comprobación de permisos**: un `admin` puede pedir cualquier
//! `guildId` y el legacy lo acepta. La única regla de servidor ligada al
//! `guildId` pedido es la de `/guilds/:guildId/settings` ([`ensure_manages_guild`]);
//! el resto de comprobaciones `403` del legacy comparan con datos del documento
//! afectado y son propias de cada ruta.

use axum::http::HeaderMap;
use serde_json::Value;

use super::qs::{query_param, QsParam};
use super::{AdminContext, AdminError};
use crate::session::SessionClaims;

const GUILD_ID: &str = "guildId";
const X_GUILD_ID: &str = "x-guild-id";

/// Mensaje de `GET/POST /guilds/:guildId/settings` (`routes/api.js:2067`, `:2083`).
pub const MSG_MANAGE_GUILD_FORBIDDEN: &str = "No autorizado para gestionar este servidor.";

/// `guildId` en una fuente (query o cuerpo).
#[derive(Debug, PartialEq, Eq)]
enum Source {
    /// La clave no existe (`=== undefined`): se pasa a la siguiente fuente.
    Absent,
    /// La clave existe: `None` si su valor es falsy en JS.
    Present(Option<String>),
    /// Express daría un array u objeto (o el cuerpo trae un valor que no es
    /// cadena y es truthy): no se puede representar como una cadena.
    Unsupported,
}

/// Servidor objetivo como `resolveTargetGuildId(req)`.
///
/// * `raw_query`: la query sin `?` (`RawQuery`); se interpreta como `qs`
///   (Express 4): `+` es espacio, `%5B`/`%5D` son corchetes, `?guildId` sin
///   `=` vale `''`, solo cuentan los primeros 1000 parámetros.
/// * `body`: el cuerpo JSON ya parseado; `None` en un GET (en Express `req.body`
///   es `{}` y no aporta `guildId`).
/// * `headers`: para `x-guild-id` (varias cabeceras se unen con `", "` y los
///   bytes se leen como latin1, como Node).
///
/// Devuelve [`AdminError::InvalidGuildId`] cuando el legacy obtendría un array
/// u objeto (`guildId` repetido o con corchetes en la query; en el cuerpo, un
/// número distinto de 0, `true`, un array o un objeto): Rust no adivina qué
/// haría Node con ese valor.
pub fn resolve_target_guild_id(
    ctx: &AdminContext,
    raw_query: Option<&str>,
    body: Option<&Value>,
    headers: &HeaderMap,
) -> Result<Option<String>, AdminError> {
    resolve_for_user(&ctx.user, raw_query, body, headers)
}

fn resolve_for_user(
    user: &SessionClaims,
    raw_query: Option<&str>,
    body: Option<&Value>,
    headers: &HeaderMap,
) -> Result<Option<String>, AdminError> {
    for source in [
        raw_query.map_or(Source::Absent, query_guild_id),
        body_guild_id(body),
    ] {
        match source {
            Source::Absent => {}
            Source::Present(v) => return Ok(v),
            Source::Unsupported => return Err(AdminError::InvalidGuildId),
        }
    }
    if let Some(h) = header_guild_id(headers) {
        return Ok(Some(h));
    }
    if user.role == "admin" {
        if let Some(s) = user.servidor.as_deref().filter(|s| !s.is_empty()) {
            return Ok(Some(s.to_owned()));
        }
    }
    Ok(None)
}

/// Regla de `GET/POST /guilds/:guildId/settings` (`routes/api.js:2066`, `:2082`):
/// `req.user?.role === 'admin' && req.user.servidor !== guildId` -> 403. Un
/// `admin` sin `servidor` también recibe 403; `global_admin` (y quien solo
/// tiene `isAdmin`) pasa.
pub fn ensure_manages_guild(ctx: &AdminContext, guild_id: &str) -> Result<(), AdminError> {
    if ctx.user.role == "admin" && ctx.user.servidor.as_deref() != Some(guild_id) {
        Err(AdminError::Forbidden(MSG_MANAGE_GUILD_FORBIDDEN))
    } else {
        Ok(())
    }
}

/// `guildId` de la query según `qs.parse` (ver [`super::qs`]).
fn query_guild_id(raw: &str) -> Source {
    match query_param(raw, GUILD_ID) {
        QsParam::Absent => Source::Absent,
        QsParam::One(v) => Source::Present((!v.is_empty()).then_some(v)),
        // Repetido o con corchetes: `qs` da un array u objeto (truthy).
        QsParam::Many(_) | QsParam::Unsupported => Source::Unsupported,
    }
}

/// `req.body?.guildId` con la veracidad de JS.
fn body_guild_id(body: Option<&Value>) -> Source {
    let Some(Value::Object(map)) = body else {
        // Sin cuerpo (`{}` en Express) o un array JSON: `guildId` es undefined.
        return Source::Absent;
    };
    match map.get(GUILD_ID) {
        None => Source::Absent,
        Some(Value::Null | Value::Bool(false)) => Source::Present(None),
        Some(Value::String(s)) => Source::Present((!s.is_empty()).then(|| s.clone())),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => Source::Present(None),
        Some(_) => Source::Unsupported,
    }
}

/// `req.headers['x-guild-id']` (o `None` si falta o queda vacía).
fn header_guild_id(headers: &HeaderMap) -> Option<String> {
    let values: Vec<String> = headers
        .get_all(X_GUILD_ID)
        .iter()
        .map(|v| v.as_bytes().iter().map(|&b| char::from(b)).collect())
        .collect();
    if values.is_empty() {
        return None;
    }
    let joined = values.join(", ");
    (!joined.is_empty()).then_some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::admin::qs::QS_PARAMETER_LIMIT;

    #[test]
    fn query_parsing_follows_qs() {
        let p = |v: &str| Source::Present(Some(v.to_owned()));
        assert_eq!(query_guild_id(""), Source::Absent);
        assert_eq!(query_guild_id("a=1"), Source::Absent);
        assert_eq!(query_guild_id("guildId=123"), p("123"));
        assert_eq!(query_guild_id("guildId="), Source::Present(None));
        assert_eq!(query_guild_id("guildId"), Source::Present(None));
        assert_eq!(query_guild_id("guild%49d=7"), p("7"));
        assert_eq!(query_guild_id("guildId=a+b%20c"), p("a b c"));
        assert_eq!(query_guild_id("guildId=%ZZ+x"), p("%ZZ x"));
        assert_eq!(query_guild_id("guildId=%FF"), p("%FF"));
        assert_eq!(query_guild_id("guildId=%C3%B1"), p("ñ"));
        assert_eq!(query_guild_id("guildId=1&guildId=2"), Source::Unsupported);
        assert_eq!(query_guild_id("guildId=&guildId="), Source::Unsupported);
        assert_eq!(query_guild_id("guildId[]=1"), Source::Unsupported);
        assert_eq!(query_guild_id("guildId%5B%5D=1"), Source::Unsupported);
        assert_eq!(query_guild_id("guildId[x]=1"), Source::Unsupported);
        assert_eq!(query_guild_id("guildId[=1"), Source::Unsupported);
        assert_eq!(query_guild_id("guildIdx=1"), Source::Absent);
        assert_eq!(query_guild_id("&&guildId=5&"), p("5"));
    }

    #[test]
    fn only_first_thousand_parameters_count() {
        let mut q = "a=1&".repeat(QS_PARAMETER_LIMIT);
        q.push_str("guildId=9");
        assert_eq!(query_guild_id(&q), Source::Absent);
        let mut q = "a=1&".repeat(QS_PARAMETER_LIMIT - 1);
        q.push_str("guildId=9");
        assert_eq!(query_guild_id(&q), Source::Present(Some("9".into())));
    }

    #[test]
    fn body_truthiness_follows_js() {
        use serde_json::json;
        let b = |v: Value| body_guild_id(Some(&v));
        assert_eq!(body_guild_id(None), Source::Absent);
        assert_eq!(b(json!({})), Source::Absent);
        assert_eq!(b(json!([{"guildId": "1"}])), Source::Absent);
        assert_eq!(
            b(json!({"guildId": "1"})),
            Source::Present(Some("1".into()))
        );
        for falsy in [json!(""), json!(null), json!(false), json!(0), json!(0.0)] {
            assert_eq!(b(json!({ "guildId": falsy })), Source::Present(None));
        }
        for odd in [json!(1), json!(true), json!(["1"]), json!({"a": 1})] {
            assert_eq!(b(json!({ "guildId": odd })), Source::Unsupported);
        }
    }

    #[test]
    fn header_is_latin1_and_joined() {
        let mut h = HeaderMap::new();
        assert_eq!(header_guild_id(&h), None);
        h.append(X_GUILD_ID, "".parse().unwrap());
        assert_eq!(header_guild_id(&h), None);
        h.append(X_GUILD_ID, "b".parse().unwrap());
        assert_eq!(header_guild_id(&h).as_deref(), Some(", b"));
        let mut h = HeaderMap::new();
        h.insert(
            X_GUILD_ID,
            axum::http::HeaderValue::from_bytes(&[0xF1]).unwrap(),
        );
        assert_eq!(header_guild_id(&h).as_deref(), Some("ñ"));
    }
}
