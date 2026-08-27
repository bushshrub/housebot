//! sandbox-api — the HTTP front end for the sandbox tier.
//!
//! Stateless by construction: with `SANDBOX_RUNTIME_BACKEND=kubernetes` every
//! sandbox lives in a Pod and the session index is the Pod's own labels, so
//! any number of replicas can sit behind one Service.
//!
//! # Usage
//!
//! ```sh
//! SANDBOX_API_TOKEN=… SANDBOX_RUNTIME_BACKEND=kubernetes sandbox-api
//! ```

use housebot_sandbox::{http, server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let token = http::token_from_env()?;
    let addr = std::env::var("SANDBOX_API_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let server = server::start();
    server.cleanup_stale().await;

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr, "sandbox-api listening");

    axum::serve(listener, http::router(server, token)).await?;
    Ok(())
}
