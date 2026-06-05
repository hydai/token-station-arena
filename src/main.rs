use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    token_station_arena::cli::run().await
}
