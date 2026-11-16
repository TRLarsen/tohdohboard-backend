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
//!   Requests *without* the header may authenticate with a bearer token if
//!   `[auth.tokens]` is not empty (e.g. public traffic via Tailscale Funnel,
//!   which carries no identity header).
//! * `token`: `Authorization: Bearer <token>`. The token's SHA-256 is compared
//!   in constant time with every entry of `[auth.tokens]` ([`Tokens`]); the
//!   matching entry's key is the user id.
//!
//! Failures answer `401`, with `WWW-Authenticate: Bearer` whenever a token
//! would be accepted.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

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
pub struct AuthError {
    /// Whether a bearer token would have been accepted, which adds a
    /// `WWW-Authenticate: Bearer` challenge.
    bearer: bool,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let mut res = (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response();
        if self.bearer {
            res.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"tohdohboard\""),
            );
        }
        res
    }
}

/// SHA-256 of a bearer token, parsed from 64 hex digits in the config.
#[derive(Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    /// Hashes a token.
    pub fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }
}

impl TryFrom<String> for TokenHash {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        let bytes = s.as_bytes();
        let mut out = [0; 32];
        let digit = |b: u8| char::from(b).to_digit(16);
        if bytes.len() != 64 {
            return Err("expected a SHA-256 as 64 hex digits (see `hash-token`)".to_owned());
        }
        for (o, pair) in out.iter_mut().zip(bytes.chunks(2)) {
            match (digit(pair[0]), digit(pair[1])) {
                (Some(hi), Some(lo)) => *o = (hi * 16 + lo) as u8,
                _ => {
                    return Err("expected a SHA-256 as 64 hex digits (see `hash-token`)".to_owned());
                }
            }
        }
        Ok(Self(out))
    }
}

impl std::fmt::Display for TokenHash {
    /// Lowercase hex, as `hash-token` prints it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl std::fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenHash(..)")
    }
}

/// A table of users and their token hashes.
#[derive(Debug)]
pub struct Tokens<'a>(pub &'a std::collections::BTreeMap<String, TokenHash>);

impl Tokens<'_> {
    /// Returns the user whose token this is.
    ///
    /// Every entry is compared, in constant time, so the response time does
    /// not reveal how much of a hash matched or how many users there are
    /// before the match.
    pub fn verify(&self, token: &str) -> Option<&str> {
        let presented = TokenHash::of(token);
        let mut found = None;
        for (user, hash) in self.0 {
            if bool::from(hash.0.ct_eq(&presented.0)) {
                found = Some(user.as_str());
            }
        }
        found
    }
}

/// Determines the user of a request according to the configured mode.
///
/// # Errors
///
/// [`AuthError`] if the request carries no acceptable identity.
pub fn authenticate(parts: &Parts, state: &AppState) -> Result<User, AuthError> {
    let auth = &state.config.auth;
    let tokens = Tokens(&auth.tokens);
    let by_token = || {
        bearer_token(&parts.headers)
            .and_then(|t| tokens.verify(t))
            .and_then(|u| sanitize_user(u.as_bytes()))
            .map(User::Named)
            .ok_or(AuthError { bearer: true })
    };
    // Funnel traffic is public: never let it claim an identity by header,
    // even if a client managed to smuggle one past the proxy.
    let funnel = parts.headers.contains_key(FUNNEL_HEADER);
    match auth.mode {
        AuthMode::None => Ok(User::Anonymous),
        AuthMode::Token => by_token(),
        AuthMode::Proxy if !funnel && parts.headers.contains_key(auth.user_header.as_str()) => {
            proxy_user(&parts.headers, &auth.user_header).ok_or(AuthError { bearer: false })
        }
        AuthMode::Proxy if !auth.tokens.is_empty() => by_token(),
        AuthMode::Proxy => Err(AuthError { bearer: false }),
    }
}

/// Header that `tailscale serve` sets (to `?1`) on requests that arrived via
/// Funnel, i.e. from the public internet; it deletes any client-sent copy.
/// In proxy mode such requests never authenticate by `user_header`.
pub const FUNNEL_HEADER: &str = "tailscale-funnel-request";

/// Extracts the token from a single `Authorization: Bearer <token>` header.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
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
    fn token_hash_parses_hex_and_round_trips() {
        let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let h = TokenHash::try_from(hex.to_uppercase()).unwrap();
        assert_eq!(h, TokenHash::of("test"));
        assert_eq!(h.to_string(), hex);
        assert!(TokenHash::try_from(hex[..62].to_owned()).is_err());
        assert!(TokenHash::try_from(format!("{}zz", &hex[..62])).is_err());
        assert!(TokenHash::try_from(format!("{}\u{e9}", &hex[..62])).is_err());
        assert!(!format!("{h:?}").contains("9f86"));
    }

    #[test]
    fn verifies_tokens() {
        let table = [("alice", "a-secret"), ("bob", "b-secret")]
            .into_iter()
            .map(|(u, t)| (u.to_owned(), TokenHash::of(t)))
            .collect();
        let tokens = Tokens(&table);
        assert_eq!(tokens.verify("a-secret"), Some("alice"));
        assert_eq!(tokens.verify("b-secret"), Some("bob"));
        assert_eq!(tokens.verify("a-secre"), None);
        assert_eq!(tokens.verify(""), None);
    }

    #[test]
    fn parses_bearer_header() {
        let h = |vals: &[&str]| {
            let mut m = HeaderMap::new();
            for v in vals {
                m.append(header::AUTHORIZATION, v.parse().unwrap());
            }
            m
        };
        assert_eq!(bearer_token(&h(&["Bearer abc"])), Some("abc"));
        assert_eq!(bearer_token(&h(&["bearer  abc "])), Some("abc"));
        assert_eq!(bearer_token(&h(&["Basic abc"])), None);
        assert_eq!(bearer_token(&h(&["Bearer"])), None);
        assert_eq!(bearer_token(&h(&["Bearer "])), None);
        assert_eq!(bearer_token(&h(&["Bearer abc", "Bearer def"])), None);
        assert_eq!(bearer_token(&h(&[])), None);
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
