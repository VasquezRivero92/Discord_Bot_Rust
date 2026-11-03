use axum::Router;

pub mod middleware;
pub mod proxy;
pub mod static_files;

/// Builds the application router. Unknown routes return 404.
pub fn app() -> Router {
    Router::new()
}

/// Builds the application router with every unmatched route reverse-proxied
/// to the Node legacy at `legacy_url`.
pub fn app_with_proxy(legacy_url: String) -> Router {
    app_with_proxy_options(legacy_url, proxy::ProxyOptions::default())
}

/// Like [`app_with_proxy`] with explicit [`proxy::ProxyOptions`].
pub fn app_with_proxy_options(legacy_url: String, options: proxy::ProxyOptions) -> Router {
    Router::new().fallback_service(proxy::fallback_proxy(legacy_url, options))
}
