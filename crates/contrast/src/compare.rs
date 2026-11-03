//! Comparacion pura de dos respuestas observadas.

use std::collections::BTreeSet;

use serde_json::{Number, Value};

use crate::{truncate, Case, Diff, Observed};

const MAX_VALUE: usize = 200;

/// Compara `node` con `rust` segun las reglas del arnes (estado, cuerpo y
/// cabeceras seleccionadas). Funcion pura, sin red.
pub fn compare(node: &Observed, rust: &Observed, case: &Case) -> Result<(), Diff> {
    let diff = |part: String, lines: Vec<String>| Diff {
        case: case.name.clone(),
        part,
        lines,
    };

    if node.status != rust.status {
        return Err(diff(
            "status".into(),
            vec![
                format!("node: {}", node.status),
                format!("rust: {}", rust.status),
            ],
        ));
    }
    if let Some(expected) = case.expect_status {
        for (side, o) in [("node", node), ("rust", rust)] {
            if o.status != expected {
                return Err(diff(
                    "status".into(),
                    vec![
                        format!("expected: {expected}"),
                        format!("{side}: {}", o.status),
                    ],
                ));
            }
        }
    }

    if let Some((part, lines)) = compare_body(node, rust, case) {
        return Err(diff(part, lines));
    }
    if let Some((part, lines)) = compare_headers(node, rust, case) {
        return Err(diff(part, lines));
    }
    Ok(())
}

// ------------------------------------------------------------------ cuerpo

fn compare_body(node: &Observed, rust: &Observed, case: &Case) -> Option<(String, Vec<String>)> {
    if node.body == rust.body {
        return None;
    }
    let parsed = (
        serde_json::from_slice::<Value>(&node.body),
        serde_json::from_slice::<Value>(&rust.body),
    );
    if let (Ok(mut a), Ok(mut b)) = parsed {
        for p in &case.ignore_fields {
            let segs: Vec<&str> = p.split('.').collect();
            ignore_path(&mut a, &segs);
            ignore_path(&mut b, &segs);
        }
        return first_diff(&a, &b, "$").map(|(path, x, y)| {
            (
                format!("body at {path}"),
                vec![show("node", x.as_ref()), show("rust", y.as_ref())],
            )
        });
    }
    // Al menos uno no es JSON: byte a byte.
    let offset = node
        .body
        .iter()
        .zip(&rust.body)
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| node.body.len().min(rust.body.len()));
    Some((
        format!("body (bytes, first difference at offset {offset})"),
        vec![
            format!(
                "node: {}",
                truncate(&String::from_utf8_lossy(&node.body), MAX_VALUE)
            ),
            format!(
                "rust: {}",
                truncate(&String::from_utf8_lossy(&rust.body), MAX_VALUE)
            ),
        ],
    ))
}

fn show(side: &str, v: Option<&Value>) -> String {
    match v {
        Some(v) => format!("{side}: {}", truncate(&v.to_string(), MAX_VALUE)),
        None => format!("{side}: (absent)"),
    }
}

/// Elimina (u omite, en arrays) el valor al que apunta `segs`.
fn ignore_path(v: &mut Value, segs: &[&str]) {
    let Some((first, rest)) = segs.split_first() else {
        return;
    };
    match v {
        Value::Object(m) => {
            if *first == "*" {
                for x in m.values_mut() {
                    ignore_tail(x, rest);
                }
            } else if rest.is_empty() {
                m.remove(*first);
            } else if let Some(x) = m.get_mut(*first) {
                ignore_path(x, rest);
            }
        }
        Value::Array(a) => {
            if *first == "*" {
                for x in a.iter_mut() {
                    ignore_tail(x, rest);
                }
            } else if let Ok(i) = first.parse::<usize>() {
                if let Some(x) = a.get_mut(i) {
                    ignore_tail(x, rest);
                }
            }
        }
        _ => {}
    }
}

/// Ultimo tramo de una ruta sobre un elemento ya localizado: si no quedan
/// segmentos se neutraliza el valor (`null`), si no se sigue descendiendo.
fn ignore_tail(x: &mut Value, rest: &[&str]) {
    if rest.is_empty() {
        *x = Value::Null;
    } else {
        ignore_path(x, rest);
    }
}

fn num_eq(a: &Number, b: &Number) -> bool {
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
        return x == y;
    }
    a.as_f64() == b.as_f64()
}

type Found = (String, Option<Value>, Option<Value>);

/// Primera diferencia estructural: orden de claves irrelevante, orden de
/// arrays significativo, numeros por valor.
fn first_diff(a: &Value, b: &Value, path: &str) -> Option<Found> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            (!num_eq(x, y)).then(|| (path.to_string(), Some(a.clone()), Some(b.clone())))
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let p = format!("{path}.{k}");
                match (x.get(k), y.get(k)) {
                    (Some(l), Some(r)) => {
                        if let Some(d) = first_diff(l, r, &p) {
                            return Some(d);
                        }
                    }
                    (l, r) => return Some((p, l.cloned(), r.cloned())),
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                let p = format!("{path}.{i}");
                match (x.get(i), y.get(i)) {
                    (Some(l), Some(r)) => {
                        if let Some(d) = first_diff(l, r, &p) {
                            return Some(d);
                        }
                    }
                    (l, r) => return Some((p, l.cloned(), r.cloned())),
                }
            }
            None
        }
        _ => (a != b).then(|| (path.to_string(), Some(a.clone()), Some(b.clone()))),
    }
}

// --------------------------------------------------------------- cabeceras

const COMPARED_HEADERS: [&str; 4] = ["content-type", "cache-control", "location", "set-cookie"];

fn compare_headers(node: &Observed, rust: &Observed, case: &Case) -> Option<(String, Vec<String>)> {
    for name in COMPARED_HEADERS {
        if case
            .ignore_headers
            .iter()
            .any(|h| h.trim().eq_ignore_ascii_case(name))
        {
            continue;
        }
        let a = normalize(name, &values(node, name));
        let b = normalize(name, &values(rust, name));
        if a != b {
            let render = |side: &str, v: &Option<Vec<String>>| match v {
                Some(v) => format!("{side}: {}", truncate(&v.join(" | "), MAX_VALUE)),
                None => format!("{side}: (absent)"),
            };
            let mut lines = vec![render("node", &a), render("rust", &b)];
            if name == "set-cookie" {
                lines.extend(cookie_marker_diffs(a.as_deref(), b.as_deref()));
            }
            return Some((format!("header {name}"), lines));
        }
    }
    None
}

fn values<'a>(o: &'a Observed, name: &str) -> Vec<&'a str> {
    o.headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .collect()
}

/// Forma canonica comparable de una cabecera (`None` si no esta presente).
/// Nunca contiene valores de cookies.
fn normalize(name: &str, vals: &[&str]) -> Option<Vec<String>> {
    if vals.is_empty() {
        return None;
    }
    let mut out: Vec<String> = match name {
        "content-type" => vals.iter().map(|v| norm_content_type(v)).collect(),
        "cache-control" => vals
            .iter()
            .flat_map(|v| v.split(','))
            .map(norm_directive)
            .filter(|d| !d.is_empty())
            .collect(),
        "set-cookie" => vals.iter().map(|v| norm_cookie(v)).collect(),
        _ => vals.iter().map(|v| v.to_string()).collect(),
    };
    out.sort();
    Some(out)
}

fn norm_content_type(v: &str) -> String {
    let mut parts = v.split(';');
    let media = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut params: Vec<String> = parts
        .filter_map(|p| {
            let p = p.trim();
            if p.is_empty() {
                return None;
            }
            Some(match p.split_once('=') {
                Some((k, val)) => format!(
                    "{}={}",
                    k.trim().to_ascii_lowercase(),
                    val.trim().trim_matches('"').to_ascii_lowercase()
                ),
                None => p.to_ascii_lowercase(),
            })
        })
        .collect();
    params.sort();
    if params.is_empty() {
        media
    } else {
        format!("{media}; {}", params.join("; "))
    }
}

fn norm_directive(d: &str) -> String {
    let d = d.trim();
    match d.split_once('=') {
        Some((k, v)) => format!("{}={}", k.trim().to_ascii_lowercase(), v.trim()),
        None => d.to_ascii_lowercase(),
    }
}

/// `nombre: attr1, attr2=v` con atributos en minusculas, ordenados; el valor
/// de la cookie y `expires` se descartan.
fn norm_cookie(v: &str) -> String {
    let mut parts = v.split(';');
    let first = parts.next().unwrap_or("").trim();
    let (name, value) = first.split_once('=').unwrap_or((first, ""));
    let name = name.trim();
    let mut has_expires = false;
    let mut attrs: Vec<String> = parts
        .filter_map(|a| {
            let a = a.trim();
            let (k, val) = match a.split_once('=') {
                Some((k, val)) => (k.trim().to_ascii_lowercase(), Some(val.trim())),
                None => (a.to_ascii_lowercase(), None),
            };
            match (k.as_str(), val) {
                ("", _) => None,
                ("expires", _) => {
                    has_expires = true;
                    None
                }
                ("path", Some(v)) => Some(format!("path={v}")),
                ("max-age" | "samesite" | "domain", Some(v)) => {
                    Some(format!("{k}={}", v.to_ascii_lowercase()))
                }
                _ => Some(k),
            }
        })
        .collect();
    // Marcadores gruesos: el valor y la fecha exacta no se comparan, pero una
    // cookie borrada (valor vacio) o de sesion (sin expires) si se distingue.
    attrs.push(
        if value.trim().is_empty() {
            "value=empty"
        } else {
            "value=nonempty"
        }
        .into(),
    );
    attrs.push(
        if has_expires {
            "expires=present"
        } else {
            "expires=absent"
        }
        .into(),
    );
    attrs.sort();
    format!("{name}: {}", attrs.join(", "))
}

/// Explica por cookie que atributos/marcadores difieren
/// (`auth_token: value=empty vs value=nonempty`).
fn cookie_marker_diffs(a: Option<&[String]>, b: Option<&[String]>) -> Vec<String> {
    let split = |c: &String| {
        let (n, t) = c.split_once(": ").unwrap_or((c.as_str(), ""));
        (
            n.to_string(),
            t.split(", ").map(str::to_string).collect::<Vec<_>>(),
        )
    };
    let ma: std::collections::BTreeMap<_, _> = a.unwrap_or(&[]).iter().map(split).collect();
    let mb: std::collections::BTreeMap<_, _> = b.unwrap_or(&[]).iter().map(split).collect();
    let names: BTreeSet<&String> = ma.keys().chain(mb.keys()).collect();
    let mut out = Vec::new();
    for n in names {
        match (ma.get(n), mb.get(n)) {
            (Some(x), Some(y)) if x != y => {
                let only = |l: &Vec<String>, o: &Vec<String>| {
                    l.iter()
                        .filter(|t| !o.contains(t))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                out.push(format!("{n}: {} vs {}", only(x, y), only(y, x)));
            }
            (Some(_), None) => out.push(format!("{n}: only in node")),
            (None, Some(_)) => out.push(format!("{n}: only in rust")),
            _ => {}
        }
    }
    out
}
