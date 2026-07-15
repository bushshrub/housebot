//! Entry point: initialize logging and run the Discord bot.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _sentry = sentry::init(sentry::ClientOptions {
        dsn: std::env::var("SENTRY_DSN")
            .ok()
            .and_then(|dsn| dsn.parse().ok()),
        environment: std::env::var("SENTRY_ENVIRONMENT").ok().map(Into::into),
        release: sentry::release_name!(),
        ..Default::default()
    });

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    if std::env::args().nth(1).as_deref() == Some("migrate") {
        return housebot::database::migrate_from_env().await;
    }

    housebot::bot::run().await
}
