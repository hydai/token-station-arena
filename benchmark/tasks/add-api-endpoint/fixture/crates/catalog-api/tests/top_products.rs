use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use catalog_api::app;
use catalog_core::Product;
use tower::ServiceExt;

#[tokio::test]
async fn top_products_endpoint_returns_most_popular_products() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/products/top?limit=2")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let products: Vec<Product> = serde_json::from_slice(&body).expect("JSON products");
    let ids: Vec<&str> = products.iter().map(|product| product.id.as_str()).collect();

    assert_eq!(ids, vec!["pro-compiler", "runtime-tracer"]);
}
