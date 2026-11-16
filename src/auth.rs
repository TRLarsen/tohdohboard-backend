//! Authentication: which user a request belongs to, and so which board.
//!
//! [`Board`] is an axum extractor. Handlers that take a `Board` only run for
//! authenticated requests; others get `401 Unauthorized`. The mode is chosen
//! by `[auth] mode` in the config:
//!
//! * `none`: everyone shares the board in `data_dir` (no authentication).
//! * `proxy`: a trusted reverse proxy (Tailscale Serve, oauth2-proxy, ...)
//!   puts the user id in `user_header`. The value is treated as an opaque id
//!   and mapped to `data_dir/users/<sanitized id>` by [`sanitize_user`].

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::config::AuthMode;
use crate::store::Board;

/// The authenticated user of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum User {
    /// Mode `none`: the single shared board.
    Anonymous,
    /// A named user, with the id already sanitized for use as a directory
    /// name.
    Named(String),
}

/// Rejection for unauthenticated requests: `401 Unauthorized`.
#[derive(Debug)]
pub struct AuthError;

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response()
    }
}

/// Determines the user of a request according to the configured mode.
///
/// # Errors
///
/// [`AuthError`] if the request carries no acceptable identity.
pub fn authenticate(parts: &Parts, state: &AppState) -> Result<User, AuthError> {
    let auth = &state.config.auth;
    match auth.mode {
        AuthMode::None => Ok(User::Anonymous),
        AuthMode::Proxy => proxy_user(&parts.headers, &auth.user_header).ok_or(AuthError),
    }
}

/// Reads the user id from the proxy's header.
///
/// Requests with the header missing, empty or repeated are rejected: a
/// repeated header means something other than the proxy set one of them.
fn proxy_user(headers: &HeaderMap, name: &str) -> Option<User> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    sanitize_user(value.as_bytes()).map(User::Named)
}

/// Maps an opaque user id to a safe, single path component.
///
/// Lowercase ASCII letters, digits, `-`, `_`, `@` and (not in first position)
/// `.` are kept; every other byte becomes `%XX` with uppercase hex. The mapping
/// is injective even on case-insensitive file systems (uppercase letters are
/// encoded), and the result can never be `.`, `..`, hidden, or contain a path
/// separator. Returns `None` for an empty id or one whose encoding exceeds 255
/// bytes (a common file name limit).
///
/// Header values such as RFC 2047 encoded words (`=?utf-8?q?...?=`) are not
/// decoded: any stable byte string works as an id.
pub fn sanitize_user(id: &[u8]) -> Option<String> {
    use std::fmt::Write;
    let mut out = String::with_capacity(id.len());
    for (i, &b) in id.iter().enumerate() {
        let keep =
            matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'@') || (b == b'.' && i > 0);
        if keep {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    (!out.is_empty() && out.len() <= 255).then_some(out)
}

/// The board of the authenticated user.
#[axum::async_trait]
impl FromRequestParts<AppState> for Board {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state)?;
        Ok(state.board_for(&user))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_typical_logins_readable() {
        assert_eq!(
            sanitize_user(b"alice@example.com").unwrap(),
            "alice@example.com"
        );
        assert_eq!(sanitize_user(b"bob_2-x@github").unwrap(), "bob_2-x@github");
    }

    #[test]
    fn encodes_everything_dangerous() {
        assert_eq!(sanitize_user(b"..").unwrap(), "%2E.");
        assert_eq!(sanitize_user(b".").unwrap(), "%2E");
        assert_eq!(
            sanitize_user(b"../../etc/passwd").unwrap(),
            "%2E.%2F..%2Fetc%2Fpasswd"
        );
        assert_eq!(sanitize_user(b"a\\b").unwrap(), "a%5Cb");
        assert_eq!(sanitize_user(b"a\0b").unwrap(), "a%00b");
        assert_eq!(sanitize_user(b"Alice").unwrap(), "%41lice");
        assert_eq!(sanitize_user(b"100%").unwrap(), "100%25");
        assert_eq!(sanitize_user("j\u{f6}rg".as_bytes()).unwrap(), "j%C3%B6rg");
        let q = sanitize_user(b"=?utf-8?q?j=C3=B6rg?=").unwrap();
        assert!(
            q.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"%-_@.".contains(&b))
        );
    }

    #[test]
    fn is_injective_on_tricky_inputs() {
        let inputs: [&[u8]; 6] = [b"a", b"A", b"%41", b"%2E", b".a", b"a."];
        let mut seen = std::collections::HashSet::new();
        for i in inputs {
            assert!(
                seen.insert(sanitize_user(i).unwrap().to_ascii_lowercase()),
                "collision for {i:?}"
            );
        }
    }

    #[test]
    fn rejects_empty_and_overlong() {
        assert_eq!(sanitize_user(b""), None);
        assert!(sanitize_user(&[b'a'; 255]).is_some());
        assert_eq!(sanitize_user(&[b'a'; 256]), None);
        assert_eq!(sanitize_user(&[b'A'; 86]), None);
    }

    #[test]
    fn proxy_header_must_be_single() {
        let mut h = HeaderMap::new();
        assert_eq!(proxy_user(&h, "x-user"), None);
        h.append("x-user", "alice".parse().unwrap());
        assert_eq!(proxy_user(&h, "x-user"), Some(User::Named("alice".into())));
        h.append("x-user", "mallory".parse().unwrap());
        assert_eq!(proxy_user(&h, "x-user"), None);
    }
}
