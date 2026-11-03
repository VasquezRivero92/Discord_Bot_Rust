use axum::{body::Body, http::Request};
use tower::ServiceExt;

#[tokio::test]
async fn unknown_route_returns_404() {
    let response = web::app()
        .oneshot(Request::builder().uri("/nada").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}
