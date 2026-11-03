//! Carga de casos desde `*.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{Case, CaseClaims};

/// Error al cargar casos; siempre nombra el fichero (y el caso si aplica).
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{file}: {message}")]
    File { file: String, message: String },
    #[error("duplicate case name '{case}': {first} and {second}")]
    Duplicate {
        case: String,
        first: String,
        second: String,
    },
    #[error("{file}: case '{case}': {message}")]
    Case {
        file: String,
        case: String,
        message: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCase {
    name: String,
    #[serde(default)]
    method: Option<String>,
    path: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    cookie_jwt: Option<CaseClaims>,
    #[serde(default)]
    ignore_fields: Vec<String>,
    #[serde(default)]
    ignore_headers: Vec<String>,
    #[serde(default)]
    expect_status: Option<u16>,
}

/// Lee todos los `*.toml` del directorio (sin recursion, ordenados por nombre
/// de fichero). Cada fichero contiene tablas `[[case]]`; los campos
/// desconocidos se rechazan.
pub fn load_cases(dir: &Path) -> Result<Vec<Case>, LoadError> {
    let io = |path: &Path, source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| io(dir, e))? {
        let path = entry.map_err(|e| io(dir, e))?.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "toml") {
            files.push(path);
        }
    }
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut cases = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for path in files {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
        for (i, case) in parse_file(&file, &text)?.into_iter().enumerate() {
            let here = format!("{file} ([[case]] #{})", i + 1);
            if let Some(first) = seen.insert(case.name.clone(), here.clone()) {
                return Err(LoadError::Duplicate {
                    case: case.name,
                    first,
                    second: here,
                });
            }
            cases.push(case);
        }
    }
    Ok(cases)
}

fn parse_file(file: &str, text: &str) -> Result<Vec<Case>, LoadError> {
    let file_err = |message: String| LoadError::File {
        file: file.to_string(),
        message,
    };
    let table: toml::Table = text
        .parse()
        .map_err(|e| file_err(format!("invalid TOML: {e}")))?;
    if let Some(k) = table.keys().find(|k| k.as_str() != "case") {
        return Err(file_err(format!(
            "unknown top-level key `{k}` (only [[case]] is allowed)"
        )));
    }
    let items = match table.get("case") {
        Some(toml::Value::Array(a)) if !a.is_empty() => a,
        _ => return Err(file_err("expected at least one [[case]] table".into())),
    };
    items
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let label = v
                .get("name")
                .and_then(toml::Value::as_str)
                .map_or_else(|| format!("#{}", i + 1), str::to_string);
            let case_err = |message: String| LoadError::Case {
                file: file.to_string(),
                case: label.clone(),
                message,
            };
            let raw: RawCase = v.clone().try_into().map_err(|e| case_err(e.to_string()))?;
            build(raw).map_err(case_err)
        })
        .collect()
}

fn build(raw: RawCase) -> Result<Case, String> {
    if raw.name.trim().is_empty() {
        return Err("`name` must not be empty".into());
    }
    if !raw.path.starts_with('/') {
        return Err(format!("`path` must start with '/': {}", raw.path));
    }
    let method = raw
        .method
        .unwrap_or_else(|| "GET".into())
        .to_ascii_uppercase();
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(format!("invalid `method`: {method}"));
    }
    if let Some(s) = raw.expect_status {
        if !(100..=599).contains(&s) {
            return Err(format!("`expect_status` out of range: {s}"));
        }
    }
    Ok(Case {
        name: raw.name,
        method,
        path: raw.path,
        headers: raw.headers.into_iter().collect(),
        body: raw.body,
        cookie_jwt: raw.cookie_jwt,
        ignore_fields: raw.ignore_fields,
        ignore_headers: raw.ignore_headers,
        expect_status: raw.expect_status,
    })
}
