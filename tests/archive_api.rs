//! Board and archive endpoints against a temporary data dir.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;

fn ids(v: &serde_json::Value) -> Vec<String> {
    v["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn missing_board_is_default_and_not_written() {
    let t = TestApp::new("");
    let r = t.get("/api/data").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["columns"][2]["id"], "done");
    assert!(!t.path("data.json").exists());
}

#[tokio::test]
async fn corrupt_board_is_500_and_not_replaced_by_default() {
    let t = TestApp::new("");
    std::fs::write(t.path("data.json"), "{\"tasks\": [").unwrap();
    assert_eq!(
        t.get("/api/data").await.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        std::fs::read_to_string(t.path("data.json")).unwrap(),
        "{\"tasks\": ["
    );
}

#[tokio::test]
async fn put_round_trips_unknown_fields() {
    let t = TestApp::new("");
    let doc = json!({ "tasks": [{ "id": "a", "status": "todo", "x-custom": [1, { "y": null }] }],
                      "contexts": [], "columns": [], "settings": { "theme": "dark" } });
    assert_eq!(
        t.send("PUT", "/api/data", &[], Some(doc.clone()))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(t.get("/api/data").await.json(), doc);
}

#[tokio::test]
async fn non_json_put_is_rejected() {
    let t = TestApp::new("");
    let r = t
        .send("PUT", "/api/data", &[("content-type", "text/plain")], None)
        .await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn non_object_put_is_rejected() {
    let t = TestApp::new("");
    for body in [json!(42), json!(null), json!([]), json!("x")] {
        let r = t.send("PUT", "/api/data", &[], Some(body.clone())).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    assert!(!t.path("data.json").exists());
    let r = t.send("PUT", "/api/data", &[], Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn sweep_archive_restore_and_tombstone() {
    let t = TestApp::new("[archive]\nafter_days = 1\n");
    let stale = json!({ "tasks": [
        { "id": "old", "status": "done", "updatedAt": "2020-01-01T00:00:00.000Z" },
        { "id": "fresh", "status": "done", "updatedAt": "2999-01-01T00:00:00.000Z" },
        { "id": "todo", "status": "todo", "updatedAt": "2020-01-01T00:00:00.000Z" }
    ], "columns": [] });
    t.send("PUT", "/api/data", &[], Some(stale.clone())).await;

    // Lazy sweep on GET.
    assert_eq!(ids(&t.get("/api/data").await.json()), ["fresh", "todo"]);
    let a = t.get("/api/archive").await.json();
    assert_eq!(ids(&a), ["old"]);
    assert!(a["tasks"][0]["archivedAt"].is_string());

    // A stale client PUTting the old document does not resurrect it.
    t.send("PUT", "/api/data", &[], Some(stale.clone())).await;
    assert_eq!(ids(&t.get("/api/data").await.json()), ["fresh", "todo"]);

    // Manual archive; unknown ids are ignored.
    let r = t
        .send(
            "POST",
            "/api/archive",
            &[],
            Some(json!({ "taskIds": ["todo", "nope"] })),
        )
        .await;
    assert_eq!(r.json(), json!({ "archived": 1 }));
    assert_eq!(ids(&t.get("/api/archive").await.json()), ["todo", "old"]);

    // Restore keeps status and bumps updatedAt so it is not re-swept.
    let r = t
        .send(
            "POST",
            "/api/archive/restore",
            &[],
            Some(json!({ "taskIds": ["old", "nope"] })),
        )
        .await;
    assert_eq!(r.json(), json!({ "restored": 1 }));
    let board = t.get("/api/data").await.json();
    assert_eq!(ids(&board), ["fresh", "old"]);
    assert_eq!(board["tasks"][1]["status"], "done");
    assert!(board["tasks"][1].get("archivedAt").is_none());
    assert_ne!(board["tasks"][1]["updatedAt"], "2020-01-01T00:00:00.000Z");
    assert_eq!(ids(&t.get("/api/archive").await.json()), ["todo"]);
}

#[tokio::test]
async fn sweep_can_be_disabled() {
    let t = TestApp::new("[archive]\nenabled = false\n");
    let doc = json!({ "tasks": [{ "id": "old", "status": "done", "updatedAt": "2020-01-01T00:00:00Z" }] });
    t.send("PUT", "/api/data", &[], Some(doc)).await;
    assert_eq!(ids(&t.get("/api/data").await.json()), ["old"]);
    let r = t
        .send(
            "POST",
            "/api/archive",
            &[],
            Some(json!({ "taskIds": ["old"] })),
        )
        .await;
    assert_eq!(r.json(), json!({ "archived": 1 }));
}

#[tokio::test]
async fn bad_archive_body_is_client_error() {
    let t = TestApp::new("");
    let r = t
        .send("POST", "/api/archive", &[], Some(json!({ "ids": ["x"] })))
        .await;
    assert!(r.status.is_client_error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_archive_requests_do_not_lose_tasks() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let t = TestApp::new("");
    let tasks: Vec<_> = (0..20)
        .map(|i| json!({ "id": format!("t{i}"), "status": "todo" }))
        .collect();
    t.send("PUT", "/api/data", &[], Some(json!({ "tasks": tasks })))
        .await;
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let req = Request::post("/api/archive")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "taskIds": [format!("t{i}")] }).to_string(),
                ))
                .unwrap();
            tokio::spawn(t.app.clone().oneshot(req))
        })
        .collect();
    for h in handles {
        assert_eq!(h.await.unwrap().unwrap().status(), StatusCode::OK);
    }
    assert_eq!(
        t.get("/api/archive").await.json()["tasks"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    assert!(
        t.get("/api/data").await.json()["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
