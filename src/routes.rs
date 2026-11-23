//! HTTP handlers and the [`router`] that wires them up.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::cors::{Any, CorsLayer};

use crate::AppState;
use crate::archive;
use crate::store::Board;

/// Builds the application: all routes, CORS and request logging.
pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::PUT, Method::POST])
        .allow_headers(Any);
    Router::new()
        .route("/api/data", get(get_data).put(put_data))
        .route("/api/archive", get(get_archive).post(post_archive))
        .route("/api/archive/restore", post(post_restore))
        .layer(cors)
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

/// Logs one line per request to stderr: method, path, status, duration.
///
/// The query string is deliberately not logged.
async fn log_request(req: Request, next: Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let start = std::time::Instant::now();
    let res = next.run(req).await;
    eprintln!(
        "{method} {path} {} {}ms",
        res.status().as_u16(),
        start.elapsed().as_millis()
    );
    res
}

/// An internal error, answered with `500` and logged to stderr.
#[derive(Debug)]
pub struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        eprintln!("error: {:#}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error\n").into_response()
    }
}

/// Every handler's board: the single board in `data_dir`.
#[axum::async_trait]
impl FromRequestParts<AppState> for Board {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(state.boards.board(state.config.data_dir.clone()))
    }
}

/// `GET /api/data`: runs the archive sweep, then returns the board document.
///
/// A missing `data.json` yields the default board; an unreadable or corrupt
/// one yields `500` rather than a default board a client might save over it.
async fn get_data(State(state): State<AppState>, board: Board) -> Result<Json<Value>, AppError> {
    let cfg = state.config.archive.clone();
    let doc = board
        .with_files(move |files| {
            let mut doc = files.read_board()?;
            let mut archived = files.read_archive()?;
            let report = archive::sweep(&mut doc, &mut archived, &cfg, Utc::now());
            if report.changed() {
                // Archive first: if we crash in between, the task is in both
                // files and the next sweep drops the board copy.
                files.write_archive(&archived)?;
                if report.dropped + report.archived > 0 {
                    files.write_board(&doc)?;
                }
                eprintln!(
                    "sweep: archived {}, deleted {}",
                    report.archived, report.deleted
                );
            }
            Ok(doc)
        })
        .await?;
    Ok(Json(doc))
}

/// `PUT /api/data`: replaces the board document.
///
/// Tasks whose id is archived are dropped first, so a stale client cannot
/// resurrect them. The body must be a JSON object (else `422`); its contents,
/// including unknown fields, are stored otherwise unchanged.
async fn put_data(
    board: Board,
    Json(doc): Json<serde_json::Map<String, Value>>,
) -> Result<StatusCode, AppError> {
    let mut doc = Value::Object(doc);
    board
        .with_files(move |files| {
            archive::drop_archived(&mut doc, &files.read_archive()?);
            files.write_board(&doc)
        })
        .await?;
    Ok(StatusCode::OK)
}

/// `GET /api/archive`: `{ "tasks": [...] }`, newest archived first.
async fn get_archive(board: Board) -> Result<Json<Value>, AppError> {
    let mut tasks = board.with_files(|files| files.read_archive()).await?;
    archive::sort_newest_first(&mut tasks);
    Ok(Json(json!({ "tasks": tasks })))
}

/// Request body of the archive and restore endpoints.
#[derive(Debug, Deserialize)]
struct TaskIds {
    /// Ids of the tasks to move.
    #[serde(rename = "taskIds")]
    task_ids: Vec<String>,
}

/// `POST /api/archive`: moves the given tasks off the board into the archive.
async fn post_archive(board: Board, Json(body): Json<TaskIds>) -> Result<Json<Value>, AppError> {
    let n = board
        .with_files(move |files| {
            let mut doc = files.read_board()?;
            let mut archived = files.read_archive()?;
            let n = archive::archive_tasks(&mut doc, &mut archived, &body.task_ids, Utc::now());
            if n > 0 {
                files.write_archive(&archived)?;
                files.write_board(&doc)?;
            }
            Ok(n)
        })
        .await?;
    Ok(Json(json!({ "archived": n })))
}

/// `POST /api/archive/restore`: moves the given tasks back onto the board.
async fn post_restore(board: Board, Json(body): Json<TaskIds>) -> Result<Json<Value>, AppError> {
    let n = board
        .with_files(move |files| {
            let mut doc = files.read_board()?;
            let mut archived = files.read_archive()?;
            let n = archive::restore_tasks(&mut doc, &mut archived, &body.task_ids, Utc::now());
            if n > 0 {
                // Board first: if we crash in between, the task stays archived.
                files.write_board(&doc)?;
                files.write_archive(&archived)?;
            }
            Ok(n)
        })
        .await?;
    Ok(Json(json!({ "restored": n })))
}
