use axum::Router;

/// Builds the application router. Unknown routes return 404.
pub fn app() -> Router {
    Router::new()
}
