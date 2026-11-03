//! `contrast/cases/*.toml` del repositorio deben cargar con el esquema vigente.

use std::path::Path;

#[test]
fn repo_cases_load_and_include_public_routes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contrast/cases");
    let cases = contrast::load_cases(&dir).expect("contrast/cases carga");
    let paths: Vec<_> = cases.iter().map(|c| c.path.as_str()).collect();
    for expected in ["/api/ping", "/api/products"] {
        assert!(paths.contains(&expected), "falta {expected}: {paths:?}");
    }
    assert!(
        paths.iter().any(|p| p.starts_with("/api/reclamaciones/")),
        "{paths:?}"
    );
    let ping = cases.iter().find(|c| c.path == "/api/ping").unwrap();
    assert!(ping.ignore_fields.iter().any(|f| f == "timestamp"));
}
