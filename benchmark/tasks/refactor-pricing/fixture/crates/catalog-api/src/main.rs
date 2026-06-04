use catalog_api::app;

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("bind API listener");
    axum::serve(listener, app()).await.expect("serve API");
}
