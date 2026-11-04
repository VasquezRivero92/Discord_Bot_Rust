//! Lectura de un parámetro de la query como lo haría `qs.parse` (Express 4 con el
//! parser `extended`), limitada a lo que necesitan las rutas migradas: saber si
//! `req.query[name]` es una cadena, un array de cadenas o algo que Rust no
//! reproduce (objetos y claves con corchetes raros).

/// `parameterLimit` por defecto de `qs` (Express 4 con el parser `extended`).
pub(crate) const QS_PARAMETER_LIMIT: usize = 1000;

/// Valor de `req.query[name]`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QsParam {
    /// La clave no aparece (`=== undefined`).
    Absent,
    /// Una sola aparición `name=v` (o `name` sin `=`, que vale `''`).
    One(String),
    /// `qs` da un array: la clave repetida o con `[]` (`name[]=v`), en orden.
    Many(Vec<String>),
    /// `qs` daría un objeto o una clave literal con corchetes (`name[x]=v`,
    /// `name[0]=v`, `name[=v`): Rust no adivina qué valor vería Node.
    Unsupported,
}

/// `req.query[name]` según `qs.parse` (`qs/lib/parse.js`): `+` es espacio,
/// `%5B`/`%5D` son corchetes, solo cuentan los primeros 1000 parámetros.
pub(crate) fn query_param(raw: &str, name: &str) -> QsParam {
    let clean = replace_brackets(raw);
    let array_key = format!("{name}[]");
    let bracket_prefix = format!("{name}[");
    let mut values: Vec<String> = Vec::new();
    let mut array = false;
    for part in clean.split('&').take(QS_PARAMETER_LIMIT) {
        let pos = match part.find("]=") {
            Some(p) => Some(p + 1),
            None => part.find('='),
        };
        let (raw_key, raw_val) = match pos {
            Some(p) => (&part[..p], Some(&part[p + 1..])),
            None => (part, None),
        };
        let key = qs_decode(raw_key);
        if key == name || key == array_key {
            array |= key == array_key;
            values.push(raw_val.map(qs_decode).unwrap_or_default());
        } else if key.starts_with(&bracket_prefix) {
            return QsParam::Unsupported;
        }
    }
    match values.len() {
        0 => QsParam::Absent,
        1 if !array => QsParam::One(values.pop().unwrap_or_default()),
        _ => QsParam::Many(values),
    }
}

/// `cleanStr.replace(/%5B/gi, '[').replace(/%5D/gi, ']')`.
fn replace_brackets(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let pair = [b[i + 1].to_ascii_uppercase(), b[i + 2].to_ascii_uppercase()];
            if pair == *b"5B" {
                out.push('[');
                i += 3;
                continue;
            }
            if pair == *b"5D" {
                out.push(']');
                i += 3;
                continue;
            }
        }
        let ch = raw[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `utils.decode` de `qs` (utf-8): `+` -> espacio y `decodeURIComponent`; si
/// este falla (`%` sin dos hexadecimales o UTF-8 inválido) se devuelve la
/// cadena solo con los `+` sustituidos.
fn qs_decode(s: &str) -> String {
    let without_plus = s.replace('+', " ");
    percent_decode_strict(&without_plus).unwrap_or(without_plus)
}

/// `decodeURIComponent`: `None` donde JS lanzaría `URIError` (`%` sin dos
/// hexadecimales o bytes que no son UTF-8).
pub(crate) fn percent_decode_strict(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hi = hex(*b.get(i + 1)?)?;
            let lo = hex(*b.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_like_qs() {
        let one = |v: &str| QsParam::One(v.to_owned());
        let many = |v: &[&str]| QsParam::Many(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(query_param("", "limit"), QsParam::Absent);
        assert_eq!(query_param("limit=5", "limit"), one("5"));
        assert_eq!(query_param("limit", "limit"), one(""));
        assert_eq!(query_param("limit=+5", "limit"), one(" 5"));
        assert_eq!(query_param("lim%69t=%2B5", "limit"), one("+5"));
        assert_eq!(query_param("limit=1&limit=2", "limit"), many(&["1", "2"]));
        assert_eq!(query_param("limit[]=1", "limit"), many(&["1"]));
        assert_eq!(
            query_param("limit=1&limit%5B%5D=2", "limit"),
            many(&["1", "2"])
        );
        assert_eq!(query_param("limit[0]=1", "limit"), QsParam::Unsupported);
        assert_eq!(query_param("limit[a]=1", "limit"), QsParam::Unsupported);
        assert_eq!(query_param("limits=1", "limit"), QsParam::Absent);
    }

    #[test]
    fn decode_strict_rejects_what_js_rejects() {
        assert_eq!(percent_decode_strict("a%20b").as_deref(), Some("a b"));
        assert_eq!(percent_decode_strict("a+b").as_deref(), Some("a+b"));
        assert_eq!(percent_decode_strict("%ZZ"), None);
        assert_eq!(percent_decode_strict("%F"), None);
        assert_eq!(percent_decode_strict("%FF"), None);
    }
}
