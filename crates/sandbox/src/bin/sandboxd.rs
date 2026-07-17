//! sandboxd — the sandbox daemon.
//!
//! Owns the Docker socket.  Listens on a Unix socket and accepts typed
//! sandbox lifecycle requests from Housebot.
//!
//! # Usage
//!
//! ```sh
//! # default socket path: /run/housebot-sandbox/sandbox.sock
//! sandboxd
//!
//! # custom socket path
//! SANDBOX_SOCKET_PATH=/tmp/sandbox.sock sandboxd
//! ```

use housebot_sandbox::server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let socket_path = std::env::var("SANDBOX_SOCKET_PATH")
        .unwrap_or_else(|_| "/run/housebot-sandbox/sandbox.sock".to_string());

    tracing::info!(socket_path, "sandboxd starting");

    server::run_daemon(&socket_path).await
}
