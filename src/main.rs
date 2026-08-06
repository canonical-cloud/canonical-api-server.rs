#[tokio::main]
async fn main() -> anyhow::Result<()> {
    canonical_api_server::run().await
}
