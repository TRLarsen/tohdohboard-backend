//! TohdohBoard backend: a tiny HTTP server that stores one shared task board.
//!
//! The `tohdohboard-web` and `tohdohboard-cli` clients use this server as
//! their single source of truth. The whole board is one JSON document
//! (`tasks`, `contexts` and `columns`), and the server treats it as opaque
//! JSON: it does not validate or interpret the contents.
//!
//! # Endpoints
//!
//! * `GET /api/data` ([`get_data`]) returns the stored board document, or a
//!   built-in default board if nothing usable is stored.
//! * `PUT /api/data` ([`save_data`]) replaces the stored board document with
//!   the request body.
//!
//! There is no authentication. Any origin may call the API (CORS allows all
//! origins and headers, for `GET` and `PUT`), so run it only on a trusted
//! network such as a Tailnet.
//!
//! # Where data lives
//!
//! The board is stored as pretty-printed JSON in `~/.tohdohboard/data.json`
//! (see [`get_file_path`]). The directory is created on first use. The server
//! listens on `0.0.0.0:3001`; both the address and the data location are
//! currently hard-coded.
//!
//! # Viewing these docs
//!
//! This is a binary crate, so private items are documented too. Build and open
//! the docs with:
//!
//! ```text
//! cargo doc --no-deps --document-private-items --open
//! ```

#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

use axum::{
    routing::{get, put},
    Router, Json, http::Method,
};
use tower_http::cors::{Any, CorsLayer};
use std::net::SocketAddr;
use std::fs;
use std::path::PathBuf;
use anyhow::{Context, Result};
use serde_json::Value;

/// Starts the HTTP server.
///
/// Registers `GET` and `PUT` on `/api/data` behind a permissive CORS layer
/// (any origin, any header), binds `0.0.0.0:3001` and serves until the process
/// is killed. Failing to bind the port or serve panics (`unwrap`) rather than
/// returning an error; the `Result` return type is not used for those cases.
#[tokio::main]
async fn main() -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::PUT])
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/data", get(get_data))
        .route("/api/data", put(save_data))
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3001));
    println!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

/// Returns the path of the board file, `~/.tohdohboard/data.json`.
///
/// Creates the `~/.tohdohboard` directory if it does not exist yet; the file
/// itself is not created.
///
/// # Errors
///
/// Fails if the home directory cannot be determined or the directory cannot be
/// created.
fn get_file_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let config_dir = home_dir.join(".tohdohboard");
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).context("Could not create config directory")?;
    }
    Ok(config_dir.join("data.json"))
}

/// Handler for `GET /api/data`: returns the board document.
///
/// Responds with the contents of the file from [`get_file_path`] when it can
/// be read and parsed as JSON. Any failure (no home directory, missing or
/// unreadable file, invalid JSON) is silently swallowed and the default board
/// is returned instead, with status 200. A corrupt file therefore looks like a
/// fresh board to clients, and a following `PUT` overwrites it. The default
/// board is not written to disk by this handler.
///
/// The default board has no tasks, the contexts `work`, `school` and
/// `projects`, and the columns `todo`, `doing` and `done`. The parsed file is
/// returned as-is; its shape is not checked.
async fn get_data() -> Json<Value> {
    println!("GET /api/data requested");
    if let Ok(file_path) = get_file_path() {
        println!("Reading from: {:?}", file_path);
        if let Ok(content) = fs::read_to_string(&file_path) {
            if let Ok(data) = serde_json::from_str(&content) {
                return Json(data);
            }
        }
    }
    
    println!("Returning default schema");
    // Default schema structure
    let default_data = serde_json::json!({
        "tasks": [],
        "contexts": [
            { "id": "work", "name": "Work", "position": 0 },
            { "id": "school", "name": "School", "position": 1 },
            { "id": "projects", "name": "Projects", "position": 2 }
        ],
        "columns": [
            { "id": "todo", "name": "To Do", "position": 0 },
            { "id": "doing", "name": "Doing", "position": 1 },
            { "id": "done", "name": "Done", "position": 2 }
        ]
    });
    
    Json(default_data)
}

/// Handler for `PUT /api/data`: replaces the stored board document.
///
/// The JSON body is written, pretty-printed, to the file from
/// [`get_file_path`] with a plain [`fs::write`]. The write is not atomic and
/// requests are not serialized, so concurrent saves are last-writer-wins. The
/// body is stored without validation of its shape (any JSON value is accepted
/// by the extractor).
///
/// Returns `200 OK` on success, `500 Internal Server Error` if the file path
/// cannot be determined or the write fails (the error is logged to stderr),
/// and `400 Bad Request` if the payload cannot be serialized. Bodies that are
/// not valid JSON are rejected by axum's `Json` extractor before this handler
/// runs.
async fn save_data(Json(payload): Json<Value>) -> axum::http::StatusCode {
    println!("PUT /api/data received");
    if let Ok(file_path) = get_file_path() {
        if let Ok(content) = serde_json::to_string_pretty(&payload) {
            match fs::write(&file_path, content) {
                Ok(_) => {
                    println!("Successfully saved data to {:?}", file_path);
                    axum::http::StatusCode::OK
                },
                Err(e) => {
                    eprintln!("Failed to write to file {:?}: {}", file_path, e);
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        } else {
            axum::http::StatusCode::BAD_REQUEST
        }
    } else {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    }
}
