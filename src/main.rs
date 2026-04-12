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

fn get_file_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let config_dir = home_dir.join(".tohdohboard");
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).context("Could not create config directory")?;
    }
    Ok(config_dir.join("data.json"))
}

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
