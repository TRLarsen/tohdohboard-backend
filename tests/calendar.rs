//! `GET /api/calendar.ics` and its authentication.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;
use tohdohboard_server::auth::TokenHash;

fn board() -> serde_json::Value {
    json!({ "tasks": [
        { "id": "due", "title": "Dentist", "status": "todo", "dueDate": "2026-12-01" },
        { "id": "done", "title": "Old", "status": "done", "dueDate": "2026-12-02" },
        { "id": "nodate", "title": "Whenever", "status": "todo" },
        { "id": "gone", "title": "Archived", "status": "todo", "dueDate": "2026-12-03" }
    ] })
}

#[tokio::test]
async fn mode_none_serves_the_feed() {
    let t = TestApp::new("");
    t.send("PUT", "/api/data", &[], Some(board())).await;
    t.send(
        "POST",
        "/api/archive",
        &[],
        Some(json!({ "taskIds": ["gone"] })),
    )
    .await;
    let r = t.get("/api/calendar.ics").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "text/calendar; charset=utf-8");
    assert!(r.text.contains("UID:due@tohdohboard\r\n"));
    assert!(
        r.text
            .contains("DTSTART;VALUE=DATE:20261201\r\nDTEND;VALUE=DATE:20261202\r\n")
    );
    assert!(r.text.contains("SUMMARY:Dentist\r\n"));
    assert_eq!(r.text.matches("BEGIN:VEVENT").count(), 1);
}

#[tokio::test]
async fn missing_board_gives_empty_calendar() {
    let r = TestApp::new("").get("/api/calendar.ics").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.text.contains("VEVENT"));
}

fn token_app(mode: &str) -> TestApp {
    TestApp::new(&format!(
        "bind = \"127.0.0.1:1\"\n[auth]\nmode = \"{mode}\"\n\
         [auth.tokens]\nalice = \"{}\"\n[auth.feed_tokens]\nalice = \"{}\"\n",
        TokenHash::of("full"),
        TokenHash::of("feed-only"),
    ))
}

#[tokio::test]
async fn token_mode_accepts_query_and_feed_tokens_only_for_the_feed() {
    let t = token_app("token");
    let full = [("authorization", "Bearer full")];
    t.send("PUT", "/api/data", &full, Some(board())).await;

    for uri in [
        "/api/calendar.ics?token=full",
        "/api/calendar.ics?token=feed-only",
    ] {
        let r = t.get(uri).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        assert!(r.text.contains("SUMMARY:Dentist"));
    }
    let feed_bearer = [("authorization", "Bearer feed-only")];
    assert_eq!(
        t.send("GET", "/api/calendar.ics", &feed_bearer, None)
            .await
            .status,
        StatusCode::OK
    );

    // The feed token opens nothing else, and ?token= works nowhere else.
    assert_eq!(
        t.send("GET", "/api/data", &feed_bearer, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        t.get("/api/data?token=full").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        t.get("/api/archive?token=feed-only").await.status,
        StatusCode::UNAUTHORIZED
    );

    for uri in [
        "/api/calendar.ics",
        "/api/calendar.ics?token=wrong",
        "/api/calendar.ics?token=",
    ] {
        let r = t.get(uri).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert!(r.headers.contains_key("www-authenticate"));
    }
}

#[tokio::test]
async fn proxy_mode_uses_header_or_token_fallback() {
    let t = token_app("proxy");
    let tailnet = [("tailscale-user-login", "alice")];
    t.send("PUT", "/api/data", &tailnet, Some(board())).await;
    let r = t.send("GET", "/api/calendar.ics", &tailnet, None).await;
    assert!(r.text.contains("SUMMARY:Dentist"));
    // Public (no identity header): same user's feed via the feed token.
    let r = t.get("/api/calendar.ics?token=feed-only").await;
    assert!(r.text.contains("SUMMARY:Dentist"));
    assert_eq!(
        t.get("/api/calendar.ics").await.status,
        StatusCode::UNAUTHORIZED
    );

    // Without any tokens configured, proxy mode ignores ?token=.
    let t = TestApp::new("bind = \"127.0.0.1:1\"\n[auth]\nmode = \"proxy\"\n");
    assert_eq!(
        t.get("/api/calendar.ics?token=x").await.status,
        StatusCode::UNAUTHORIZED
    );
}
