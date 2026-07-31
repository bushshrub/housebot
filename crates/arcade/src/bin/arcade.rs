use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

const DEFAULT_ADDR: &str = "127.0.0.1:8088";
const DEFAULT_SCORES: &str = "arcade_scores.json";
const DEFAULT_ROMS: &str = "roms";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = std::env::var("HOUSEBOT_ARCADE_ADDR")
        .unwrap_or_else(|_| DEFAULT_ADDR.to_string())
        .parse()?;
    let scores =
        std::env::var("HOUSEBOT_ARCADE_SCORES").unwrap_or_else(|_| DEFAULT_SCORES.to_string());
    let roms = std::env::var("HOUSEBOT_ARCADE_ROMS").unwrap_or_else(|_| DEFAULT_ROMS.to_string());

    housebot_arcade::serve(addr, scores, roms).await
}
