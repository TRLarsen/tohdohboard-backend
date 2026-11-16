//! TohdohBoard backend: a small HTTP server that stores a task board.
//!
//! The `tohdohboard-web` and `tohdohboard-cli` clients use this server as
//! their single source of truth. The board is one JSON document (`tasks`,
//! `contexts` and `columns`) that clients replace as a whole. The server keeps
//! it as untyped JSON so fields it does not know about survive a round trip.
//!
//! # Endpoints
//!
//! * `GET /api/data` returns the board document, after running the archive
//!   sweep ([`archive::sweep`]). Without a stored board it returns a default
//!   one ([`store::default_board`]).
//! * `PUT /api/data` replaces the board document. Tasks whose id is archived
//!   are dropped ([`archive::drop_archived`]).
//! * `GET /api/archive` returns `{ "tasks": [...] }`, newest archived first.
//! * `POST /api/archive` and `POST /api/archive/restore` take
//!   `{ "taskIds": [...] }` and move those tasks into or out of the archive.
//! * `GET /healthz` answers `ok`, without authentication.
//!
//! By default there is no authentication, and CORS allows any origin, so run
//! it only on a trusted network such as a tailnet. With `[auth] mode =
//! "proxy"` or `"token"` each user gets their own board ([`auth`]).
//!
//! # Modules
//!
//! * [`config`]: the TOML config file.
//! * [`auth`]: who the user is, and so which board a request uses.
//! * [`store`]: atomic, per-board-locked file storage.
//! * [`archive`]: pure archive and sweep logic.
//! * [`routes`]: the HTTP handlers.

#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

pub mod archive;
pub mod auth;
pub mod config;
pub mod routes;
pub mod store;

use std::sync::Arc;

use auth::User;

pub use config::Config;
pub use routes::router;

/// State shared by all handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The loaded configuration.
    pub config: Arc<Config>,
    /// Per-board locks and handles.
    pub boards: Arc<store::Boards>,
}

impl AppState {
    /// Creates the state for a configuration.
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            boards: Arc::default(),
        }
    }

    /// Returns the board of `user`: `data_dir` itself for
    /// [`User::Anonymous`], else `data_dir/users/<user>`.
    pub fn board_for(&self, user: &User) -> store::Board {
        let dir = match user {
            User::Anonymous => self.config.data_dir.clone(),
            User::Named(name) => self.config.data_dir.join("users").join(name),
        };
        self.boards.board(dir)
    }
}
