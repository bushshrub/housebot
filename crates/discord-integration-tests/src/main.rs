mod driver;
mod mock_llm;
mod scenarios;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("mock-llm") => mock_llm::run().await,
        Some("driver") => driver::run().await,
        _ => anyhow::bail!("usage: discord-integration-tests <mock-llm|driver>"),
    }
}
