//! Helpers shared by the integration tests.
#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tohdohboard_server::{AppState, Config, router};
use tower::ServiceExt;

/// A router over a fresh temporary data dir, configured by `toml` (the
/// `data_dir` key is added automatically).
pub struct TestApp {
    pub dir: tempfile::TempDir,
    pub app: Router,
}

impl TestApp {
    pub fn new(toml: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("data_dir = {:?}\n{toml}", dir.path().to_str().unwrap());
        let config = Config::from_toml(&text).unwrap();
        Self {
            app: router(AppState::new(config)),
            dir,
        }
    }

    /// Sends a request; `body` is sent as JSON when given.
    pub async fn send(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<serde_json::Value>,
    ) -> Response {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Response {
            status,
            headers,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    pub async fn get(&self, uri: &str) -> Response {
        self.send("GET", uri, &[], None).await
    }

    pub fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
}

impl Response {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.text).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text))
    }
}
