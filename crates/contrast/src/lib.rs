//! Arnes de contraste Node vs Rust.
//!
//! Dado un servidor Node (legacy) y uno Rust, ejecuta la MISMA peticion contra
//! ambos y compara estado, cuerpo (JSON normalizado) y un subconjunto de
//! cabeceras. Los casos viven en `contrast/cases/*.toml` (ver
//! `contrast/cases/README.md`).
//!
//! # Escrituras
//!
//! Las peticiones GET/HEAD/OPTIONS se lanzan a la vez contra ambos lados. Los
//! metodos con efectos (POST/PUT/PATCH/DELETE y cualquier otro) se ejecutan en
//! SECUENCIA, Node primero y Rust despues: un caso con escrituras debe ser
//! autocontenido (crear y limpiar sus propios datos, o ser idempotente entre
//! ambas ejecuciones), porque la segunda peticion ve el estado que dejo la
//! primera si ambos lados comparten base de datos.

mod compare;
mod load;
mod run;

use std::fmt;

pub use compare::compare;
pub use load::{load_cases, LoadError};
pub use run::{run_case, Ctx};

/// Claims con los que se firma la cookie de sesion de un caso. Se mapean a los
/// nombres del legacy (`userId`, `username`, `avatar`, `isAdmin`, `role`,
/// `servidor`). El secreto NO forma parte del caso: llega por [`Ctx`].
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseClaims {
    pub user_id: String,
    pub username: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub servidor: Option<String>,
}

fn default_role() -> String {
    "user".to_string()
}

/// Una peticion a contrastar.
#[derive(Debug, Clone, Default)]
pub struct Case {
    pub name: String,
    /// Metodo HTTP en mayusculas (`GET`, `POST`...).
    pub method: String,
    /// Ruta con query, empezando por `/`; se anade a cada URL base.
    pub path: String,
    pub headers: Vec<(String, String)>,
    /// Cuerpo crudo.
    pub body: Option<String>,
    /// Si esta presente se firma un JWT y se envia como `auth_token` y
    /// `admin_token`.
    pub cookie_jwt: Option<CaseClaims>,
    /// Rutas con puntos (`users.*.last_seen`, `data.0.id`; `*` comodin) que se
    /// eliminan de ambos JSON antes de comparar.
    pub ignore_fields: Vec<String>,
    /// Cabeceras (`content-type`, `cache-control`, `location`, `set-cookie`)
    /// que no se comparan.
    pub ignore_headers: Vec<String>,
    /// Si esta fijado, ambos lados deben responder con este estado.
    pub expect_status: Option<u16>,
}

impl Case {
    /// Caso GET minimo.
    pub fn get(name: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            method: "GET".to_string(),
            path: path.to_string(),
            ..Self::default()
        }
    }
}

/// Lo observado en una respuesta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Diferencia entre Node y Rust (o fallo al obtenerla).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub case: String,
    /// Parte que difiere: `status`, `body at $.a.b`, `header content-type`,
    /// `transport error (node)`...
    pub part: String,
    /// Lineas de detalle (valores de ambos lados, ya truncados).
    pub lines: Vec<String>,
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "case '{}': {}", self.case, self.part)?;
        for l in &self.lines {
            write!(f, "\n  {l}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diff {}

/// Trunca a `max` caracteres anadiendo una marca.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(max).collect();
    if it.next().is_some() {
        format!("{head}... (truncated)")
    } else {
        head
    }
}
