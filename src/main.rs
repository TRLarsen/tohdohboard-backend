//! `tohdohboard-server`: loads the config and serves the API.
//!
//! See the library crate docs for the endpoints and `config.example.toml` for
//! the configuration file.

use anyhow::{Context, Result};
use tohdohboard_server::{AppState, Config, config, router};

/// Loads the config, binds the listen address and serves until killed.
#[tokio::main]
async fn main() -> Result<()> {
    let (path, explicit) = config::config_path()?;
    let (config, found) = Config::load(&path, explicit)?;
    let status = if found {
        "loaded"
    } else {
        "not found, using defaults"
    };
    eprintln!("config: {} ({status})", path.display());
    eprintln!("data dir: {}", config.data_dir.display());
    eprintln!("auth: {:?}", config.auth.mode);

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot listen on {}", config.bind))?;
    eprintln!("listening on {}", config.bind);
    axum::serve(listener, router(AppState::new(config))).await?;
    Ok(())
}
