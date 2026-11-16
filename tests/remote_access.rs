//! Auth mode "token", the proxy + token fallback, CORS allow-list and body limit.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;
use tohdohboard_server::auth::TokenHash;

fn token_config(mode: &str, extra: &str) -> String {
    format!(
        "bind = \"127.0.0.1:3001\"\n[auth]\nmode = \"{mode}\"\n{extra}\n[auth.tokens]\n\
         \"alice@example.com\" = \"{}\"\nbob = \"{}\"\n",
        TokenHash::of("alice-token"),
        TokenHash::of("bob-token"),
    )
}

#[tokio::test]
async fn token_mode_requires_a_valid_bearer_token() {
    let t = TestApp::new(&token_config("token", ""));
    for auth in [
        None,
        Some("Bearer wrong"),
        Some("Basic YWxpY2U6eA=="),
        Some("Bearer "),
    ] {
        let h: Vec<_> = auth.map(|a| ("authorization", a)).into_iter().collect();
        let r = t.send("GET", "/api/data", &h, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{auth:?}");
        assert_eq!(
            r.headers["www-authenticate"],
            "Bearer realm=\"tohdohboard\""
        );
    }
    // The proxy header means nothing in token mode.
    let r = t
        .send("GET", "/api/data", &[("tailscale-user-login", "bob")], None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Neither does a token in the query string.
    assert_eq!(
        t.get("/api/data?token=alice-token").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(t.get("/healthz").await.status, StatusCode::OK);
}

#[tokio::test]
async fn tokens_select_the_users_board() {
    let t = TestApp::new(&token_config("token", ""));
    let alice = [("authorization", "Bearer alice-token")];
    let bob = [("authorization", "bearer bob-token")];
    let doc = json!({ "tasks": [{ "id": "a" }] });
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
    assert!(t.path("users/alice@example.com/data.json").is_file());
}

#[tokio::test]
async fn proxy_mode_falls_back_to_tokens_without_identity_header() {
    let t = TestApp::new(&token_config("proxy", ""));
    let doc = json!({ "tasks": [{ "id": "shared" }] });
    // Tailnet request: identity header from the proxy.
    let tailnet = [("tailscale-user-login", "alice@example.com")];
    t.send("PUT", "/api/data", &tailnet, Some(doc.clone()))
        .await;
    // Funnel request: no identity header, bearer token for the same user id.
    let public = [("authorization", "Bearer alice-token")];
    assert_eq!(t.send("GET", "/api/data", &public, None).await.json(), doc);
    // No header and no token: challenge for a token.
    let r = t.get("/api/data").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.headers.contains_key("www-authenticate"));
    // A present identity header is never overridden by a token.
    let both = [
        ("tailscale-user-login", "carol"),
        ("authorization", "Bearer alice-token"),
    ];
    assert_eq!(
        t.send("GET", "/api/data", &both, None).await.json()["tasks"],
        json!([])
    );
}

#[tokio::test]
async fn funnel_requests_never_use_the_identity_header() {
    let t = TestApp::new(&token_config("proxy", ""));
    let funnel = ("tailscale-funnel-request", "?1");
    let forged = [funnel, ("tailscale-user-login", "alice@example.com")];
    let r = t.send("GET", "/api/data", &forged, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.headers.contains_key("www-authenticate"));
    let with_token = [funnel, ("authorization", "Bearer bob-token")];
    let r = t.send("GET", "/api/data", &with_token, None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn proxy_mode_without_tokens_sends_no_bearer_challenge() {
    let t = TestApp::new("bind = \"127.0.0.1:1\"\n[auth]\nmode = \"proxy\"\n");
    let r = t
        .send("GET", "/api/data", &[("authorization", "Bearer x")], None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(!r.headers.contains_key("www-authenticate"));
}

#[tokio::test]
async fn cors_allow_list_permits_listed_origins_with_authorization() {
    let t = TestApp::new(&token_config(
        "token",
        "cors_origins = [\"tauri://localhost\", \"https://board.example.ts.net\"]",
    ));
    let preflight = |origin: &'static str| {
        [
            ("origin", origin),
            ("access-control-request-method", "PUT"),
            (
                "access-control-request-headers",
                "authorization, content-type",
            ),
        ]
    };
    let r = t
        .send(
            "OPTIONS",
            "/api/data",
            &preflight("tauri://localhost"),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.headers["access-control-allow-origin"],
        "tauri://localhost"
    );
    let allowed = r.headers["access-control-allow-headers"]
        .to_str()
        .unwrap()
        .to_lowercase();
    assert!(
        allowed.contains("authorization") && allowed.contains("content-type"),
        "{allowed}"
    );
    assert!(
        r.headers["access-control-allow-methods"]
            .to_str()
            .unwrap()
            .contains("PUT")
    );

    let r = t
        .send(
            "OPTIONS",
            "/api/data",
            &preflight("https://evil.example"),
            None,
        )
        .await;
    assert!(r.headers.get("access-control-allow-origin").is_none());

    let h = [
        ("origin", "https://board.example.ts.net"),
        ("authorization", "Bearer bob-token"),
    ];
    let r = t.send("GET", "/api/data", &h, None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.headers["access-control-allow-origin"],
        "https://board.example.ts.net"
    );
}

#[tokio::test]
async fn cors_allow_list_also_restricts_mode_none() {
    let t = TestApp::new("[auth]\ncors_origins = [\"https://ok.example\"]\n");
    let r = t
        .send(
            "GET",
            "/api/data",
            &[("origin", "https://ok.example")],
            None,
        )
        .await;
    assert_eq!(
        r.headers["access-control-allow-origin"],
        "https://ok.example"
    );
    let r = t
        .send(
            "GET",
            "/api/data",
            &[("origin", "https://evil.example")],
            None,
        )
        .await;
    assert!(r.headers.get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn oversized_bodies_are_rejected() {
    let t = TestApp::new(&token_config("token", ""));
    let big = json!({ "tasks": [], "pad": "x".repeat(tohdohboard_server::routes::MAX_BODY_BYTES) });
    let alice = [("authorization", "Bearer alice-token")];
    let r = t.send("PUT", "/api/data", &alice, Some(big.clone())).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Unauthenticated: rejected before the body is read.
    assert_eq!(
        t.send("PUT", "/api/data", &[], Some(big)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(!t.path("users").exists());
}
