mod adapters;
mod config;
mod domain;
mod error;
mod http;
mod service;
mod state;

use anyhow::Context;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::AppConfig;
use crate::http::router::build_router;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .compact()
        .init();

    let config = AppConfig::from_env().context("failed to load app config")?;
    let listener = TcpListener::bind((config.host.as_str(), config.port))
        .await
        .context("failed to bind TCP listener")?;
    let state = AppState::new(config.clone());
    let app = build_router(state);

    info!(
        host = %config.host,
        port = config.port,
        fixture_mode = config.fixture_mode,
        static_dir = %config.static_dir().display(),
        "server_started"
    );

    axum::serve(listener, app)
        .await
        .context("axum server failed")?;

    Ok(())
}
