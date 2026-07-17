use std::{collections::HashMap, env, net::SocketAddr};

use alpha::config::Settings;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "alpha=info".into()))
        .init();

    let settings = Settings::from_pairs(env::vars().collect::<HashMap<_, _>>())?;
    let address = env::var("BIND_ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
        .parse::<SocketAddr>()?;
    let listener = TcpListener::bind(address).await?;

    info!(%address, app_env = %settings.app_env, "ALPHA API 시작");
    axum::serve(listener, alpha::http::router()).await?;
    Ok(())
}
