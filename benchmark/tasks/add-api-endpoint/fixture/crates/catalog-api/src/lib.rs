use axum::{routing::get, Json, Router};
use catalog_core::{sample_products, Product};

pub fn app() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/products", get(products))
}

async fn health() -> &'static str {
    "ok"
}

async fn products() -> Json<Vec<Product>> {
    Json(sample_products())
}
