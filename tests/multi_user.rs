//! Auth mode "proxy": per-user boards from a trusted header, and /healthz.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;

const PROXY: &str = "bind = \"127.0.0.1:3001\"\n[auth]\nmode = \"proxy\"\n";

#[tokio::test]
async fn healthz_needs_no_auth() {
    for cfg in ["", PROXY] {
        let r = TestApp::new(cfg).get("/healthz").await;
        assert_eq!((r.status, r.text.as_str()), (StatusCode::OK, "ok"));
    }
}

#[tokio::test]
async fn missing_or_repeated_header_is_401() {
    let t = TestApp::new(PROXY);
    for uri in ["/api/data", "/api/archive"] {
        assert_eq!(t.get(uri).await.status, StatusCode::UNAUTHORIZED);
    }
    let r = t
        .send("PUT", "/api/data", &[], Some(json!({ "tasks": [] })))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let two = [
        ("tailscale-user-login", "alice@x"),
        ("tailscale-user-login", "bob@x"),
    ];
    assert_eq!(
        t.send("GET", "/api/data", &two, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let empty = [("tailscale-user-login", "")];
    assert_eq!(
        t.send("GET", "/api/data", &empty, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(!t.path("data.json").exists());
    assert!(!t.path("users").exists());
}

#[tokio::test]
async fn users_get_separate_boards() {
    let t = TestApp::new(PROXY);
    let alice = [("Tailscale-User-Login", "alice@example.com")];
    let bob = [("Tailscale-User-Login", "bob@example.com")];
    let doc = json!({ "tasks": [{ "id": "a1", "status": "todo" }] });
    assert_eq!(
        t.send("PUT", "/api/data", &alice, Some(doc.clone()))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(t.send("GET", "/api/data", &alice, None).await.json(), doc);
    assert_eq!(
        t.send("GET", "/api/data", &bob, None).await.json()["tasks"],
        json!([])
    );
    assert!(t.path("users/alice@example.com/data.json").exists());
    assert!(!t.path("data.json").exists());

    // Archive is per user too.
    let ids = json!({ "taskIds": ["a1"] });
    assert_eq!(
        t.send("POST", "/api/archive", &bob, Some(ids.clone()))
            .await
            .json()["archived"],
        0
    );
    assert_eq!(
        t.send("POST", "/api/archive", &alice, Some(ids))
            .await
            .json()["archived"],
        1
    );
    assert!(t.path("users/alice@example.com/archive.json").exists());
}

#[tokio::test]
async fn hostile_user_ids_stay_inside_users_dir() {
    let t = TestApp::new(PROXY);
    let doc = json!({ "tasks": [] });
    for id in [
        "..",
        "../..",
        "../../../tmp/x",
        "/etc",
        "a/../../b",
        "=?utf-8?q?j=C3=B6rg?=",
    ] {
        let h = [("Tailscale-User-Login", id)];
        assert_eq!(
            t.send("PUT", "/api/data", &h, Some(doc.clone()))
                .await
                .status,
            StatusCode::OK
        );
    }
    // Only directories directly below users/, and nothing written elsewhere.
    let top: Vec<_> = std::fs::read_dir(t.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(top, ["users"]);
    for e in std::fs::read_dir(t.path("users")).unwrap() {
        let e = e.unwrap();
        assert!(e.path().join("data.json").is_file(), "{:?}", e.path());
        assert!(!e.file_name().to_string_lossy().contains('/'));
    }
}

#[tokio::test]
async fn custom_header_and_no_cors_when_authenticated() {
    let t = TestApp::new(
        "bind = \"127.0.0.1:1\"\n[auth]\nmode = \"proxy\"\nuser_header = \"X-Forwarded-User\"\n",
    );
    let h = [
        ("x-forwarded-user", "carol"),
        ("origin", "https://evil.example"),
    ];
    let r = t.send("GET", "/api/data", &h, None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers.get("access-control-allow-origin").is_none());
    let r = t
        .send(
            "GET",
            "/api/data",
            &[("tailscale-user-login", "carol")],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mode_none_keeps_shared_board_and_open_cors() {
    let t = TestApp::new("");
    let r = t
        .send("GET", "/api/data", &[("origin", "https://x.example")], None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["access-control-allow-origin"], "*");
    // The header is ignored without proxy mode.
    let h = [("tailscale-user-login", "alice")];
    t.send("PUT", "/api/data", &h, Some(json!({ "tasks": [] })))
        .await;
    assert!(t.path("data.json").exists());
}

#[tokio::test]
async fn cross_site_simple_requests_cannot_write() {
    // A page on another site can send a "simple" POST without a preflight, and
    // Tailscale would still attach the identity header. It cannot set
    // `Content-Type: application/json`, so the JSON extractor refuses it.
    let t = TestApp::new(PROXY);
    let h = [
        ("tailscale-user-login", "alice"),
        ("content-type", "text/plain"),
    ];
    for (m, uri) in [
        ("POST", "/api/archive"),
        ("POST", "/api/archive/restore"),
        ("PUT", "/api/data"),
    ] {
        assert_eq!(
            t.send(m, uri, &h, None).await.status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
}
