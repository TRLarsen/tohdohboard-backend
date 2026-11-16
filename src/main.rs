//! `tohdohboard-server`: loads the config and serves the API.
//!
//! See the library crate docs for the endpoints and `config.example.toml` for
//! the configuration file.
//!
//! ```text
//! tohdohboard-server              serve the API
//! tohdohboard-server hash-token   read a token on stdin, print its SHA-256
//! ```

use std::io::Read;

use anyhow::{Context, Result};
use tohdohboard_server::auth::TokenHash;
use tohdohboard_server::{AppState, Config, config, router};

/// Usage text for `--help` and unknown arguments.
const USAGE: &str = "usage: tohdohboard-server [hash-token]

  (no command)  serve the API (config: $TOHDOH_CONFIG or ~/.config/tohdohboard/config.toml)
  hash-token    read a token on stdin and print its SHA-256 for [auth.tokens]";

/// Dispatches to a subcommand, or serves.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => serve(),
        ["hash-token"] => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .context("cannot read stdin")?;
            let token = input.trim();
            anyhow::ensure!(!token.is_empty(), "no token on stdin");
            println!("{}", TokenHash::of(token));
            Ok(())
        }
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Loads the config, binds the listen address and serves until killed.
#[tokio::main]
async fn serve() -> Result<()> {
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
