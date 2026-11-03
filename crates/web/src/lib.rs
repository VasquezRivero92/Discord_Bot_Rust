use axum::Router;

pub mod proxy;

/// Builds the application router. Unknown routes return 404.
pub fn app() -> Router {
    Router::new()
}

/// Builds the application router with every unmatched route reverse-proxied
/// to the Node legacy at `legacy_url`.
pub fn app_with_proxy(legacy_url: String) -> Router {
    Router::new().fallback_service(proxy::fallback_proxy(legacy_url))
}
