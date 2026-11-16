//! Server configuration, loaded from a TOML file.
//!
//! The file is looked up at `$TOHDOH_CONFIG`, else at
//! `<XDG config dir>/tohdohboard/config.toml` (see [`config_path`]). A missing
//! file at the XDG path means "all defaults", which reproduces the historic
//! behaviour: one board in `~/.tohdohboard`, listening on `0.0.0.0:3001`. A
//! missing file named by `$TOHDOH_CONFIG` is an error, as are unknown keys, so
//! typos do not silently fall back to defaults. `config.example.toml` in the
//! repository documents every key.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Environment variable that overrides the config file location.
pub const CONFIG_ENV: &str = "TOHDOH_CONFIG";

/// The whole configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Address to listen on. Default `0.0.0.0:3001`.
    pub bind: SocketAddr,
    /// Directory holding `data.json` and `archive.json`. A leading `~` is
    /// expanded to the home directory. Default `~/.tohdohboard`.
    pub data_dir: PathBuf,
    /// Automatic archiving of finished tasks.
    pub archive: ArchiveConfig,
    /// Who may use the server, and whose board they get.
    pub auth: AuthConfig,
}

/// The `[archive]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArchiveConfig {
    /// Run the automatic sweep (archiving and deletion) on `GET /api/data`.
    /// The manual archive endpoints work either way.
    pub enabled: bool,
    /// Column id whose tasks count as finished.
    pub done_column: String,
    /// Finished tasks untouched for this many days are archived. `0`
    /// archives them on the next sweep.
    pub after_days: u32,
    /// Archived tasks are deleted this many days after being archived.
    /// `0` (the default) never deletes anything.
    pub delete_after_days: u32,
}

/// How requests are authenticated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// No authentication; everyone shares the board in `data_dir`.
    #[default]
    None,
    /// A trusted reverse proxy names the user in `user_header`; each user
    /// gets a board in `data_dir/users/<user>`.
    Proxy,
}

/// The `[auth]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthConfig {
    /// The authentication mode.
    pub mode: AuthMode,
    /// Proxy mode: the request header carrying the user id.
    pub user_header: String,
    /// Proxy mode: allow a non-loopback `bind`. Only safe if nothing but the
    /// proxy can reach the server (e.g. a container network).
    pub allow_non_loopback: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::None,
            user_header: "Tailscale-User-Login".to_owned(),
            allow_non_loopback: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 3001)),
            data_dir: PathBuf::from("~/.tohdohboard"),
            archive: ArchiveConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            done_column: "done".to_owned(),
            after_days: 14,
            delete_after_days: 0,
        }
    }
}

impl Config {
    /// Parses a config file's contents and expands `~` in `data_dir`.
    ///
    /// # Errors
    ///
    /// Fails on invalid TOML, unknown keys, wrongly typed values, an unsafe
    /// combination of settings (see [`Config::validate`]), or when `~` is used
    /// but the home directory cannot be determined.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut config: Config = toml::from_str(text)?;
        config.data_dir = expand_tilde(&config.data_dir)?;
        config.validate()?;
        Ok(config)
    }

    /// Rejects settings that would be unsafe or cannot work.
    ///
    /// In proxy mode the identity header is only trustworthy if every request
    /// passes the proxy, so `bind` must be a loopback address unless
    /// `allow_non_loopback` is set, and `user_header` must not be one that
    /// clients or the HTTP layer set ([`UNSAFE_USER_HEADERS`]).
    ///
    /// # Errors
    ///
    /// Describes the first problem found.
    pub fn validate(&self) -> Result<()> {
        if self.auth.mode == AuthMode::Proxy {
            let name = axum::http::HeaderName::try_from(self.auth.user_header.as_str())
                .with_context(|| {
                    format!(
                        "auth.user_header {:?} is not a valid header name",
                        self.auth.user_header
                    )
                })?;
            if UNSAFE_USER_HEADERS.contains(&name.as_str()) {
                anyhow::bail!(
                    "auth.user_header {:?} is set by clients or the HTTP layer, not by an \
                     identity proxy",
                    self.auth.user_header
                );
            }
            if !self.bind.ip().is_loopback() && !self.auth.allow_non_loopback {
                anyhow::bail!(
                    "auth.mode = \"proxy\" trusts the {} header, so bind must be a loopback address \
                     (e.g. 127.0.0.1:3001), not {}; anyone reaching the port directly could forge it. \
                     Set auth.allow_non_loopback = true only if just the proxy can reach the port.",
                    self.auth.user_header,
                    self.bind
                );
            }
        }
        Ok(())
    }

    /// Loads the config from `path`; a missing file yields the defaults
    /// unless `required` is set.
    ///
    /// Returns the config and whether the file existed.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read or parsed, or is missing although
    /// `required`.
    pub fn load(path: &Path, required: bool) -> Result<(Self, bool)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok((
                Self::from_toml(&text)
                    .with_context(|| format!("invalid config {}", path.display()))?,
                true,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => {
                Ok((Self::from_toml("")?, false))
            }
            Err(e) => Err(e).with_context(|| format!("cannot read config {}", path.display())),
        }
    }
}

/// Headers that can never carry a proxy-asserted identity: clients set them
/// themselves, or they are hop-by-hop / framing headers. Lowercase.
pub const UNSAFE_USER_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "host",
    "origin",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
];

/// Returns the config file location, and whether it was chosen explicitly:
/// `($TOHDOH_CONFIG, true)`, else `(<XDG config dir>/tohdohboard/config.toml,
/// false)`. Pass the flag as `required` to [`Config::load`], so a mistyped
/// `$TOHDOH_CONFIG` fails instead of silently serving the defaults.
///
/// # Errors
///
/// Fails if neither the variable nor a config directory is available.
pub fn config_path() -> Result<(PathBuf, bool)> {
    if let Some(path) = std::env::var_os(CONFIG_ENV) {
        return Ok((PathBuf::from(path), true));
    }
    let dir =
        dirs::config_dir().context("cannot determine the config directory; set TOHDOH_CONFIG")?;
    Ok((dir.join("tohdohboard").join("config.toml"), false))
}

/// Replaces a leading `~` component with the home directory.
fn expand_tilde(path: &Path) -> Result<PathBuf> {
    match path.strip_prefix("~") {
        Ok(rest) => Ok(dirs::home_dir()
            .context("cannot determine the home directory")?
            .join(rest)),
        Err(_) => Ok(path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.bind.to_string(), "0.0.0.0:3001");
        assert!(c.data_dir.ends_with(".tohdohboard"));
        assert!(!c.data_dir.starts_with("~"));
        assert!(c.archive.enabled);
        assert_eq!(c.archive.done_column, "done");
        assert_eq!(c.archive.after_days, 14);
        assert_eq!(c.archive.delete_after_days, 0);
    }

    #[test]
    fn parses_all_keys() {
        let c = Config::from_toml(
            r#"
            bind = "127.0.0.1:4000"
            data_dir = "/srv/tohdoh"
            [archive]
            enabled = false
            done_column = "finished"
            after_days = 3
            delete_after_days = 90
            "#,
        )
        .unwrap();
        assert_eq!(c.bind.to_string(), "127.0.0.1:4000");
        assert_eq!(c.data_dir, PathBuf::from("/srv/tohdoh"));
        assert!(!c.archive.enabled);
        assert_eq!(c.archive.done_column, "finished");
        assert_eq!(c.archive.after_days, 3);
        assert_eq!(c.archive.delete_after_days, 90);
    }

    #[test]
    fn missing_file_is_an_error_only_when_required() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let (c, found) = Config::load(&missing, false).unwrap();
        assert!(!found);
        assert_eq!(c.bind.to_string(), "0.0.0.0:3001");
        let err = format!("{:#}", Config::load(&missing, true).unwrap_err());
        assert!(err.contains("nope.toml"), "{err}");
    }

    #[test]
    fn proxy_mode_requires_loopback_unless_allowed() {
        let base = "[auth]\nmode = \"proxy\"\n";
        let err = Config::from_toml(base).unwrap_err().to_string();
        assert!(err.contains("loopback"), "{err}");
        let c = Config::from_toml(&format!("bind = \"127.0.0.1:3001\"\n{base}")).unwrap();
        assert_eq!(c.auth.mode, AuthMode::Proxy);
        assert_eq!(c.auth.user_header, "Tailscale-User-Login");
        assert!(Config::from_toml(&format!("bind = \"[::1]:3001\"\n{base}")).is_ok());
        assert!(Config::from_toml(&format!("{base}allow_non_loopback = true\n")).is_ok());
        let bad_header = format!("bind = \"127.0.0.1:1\"\n{base}user_header = \"Bad Header\"\n");
        assert!(Config::from_toml(&bad_header).is_err());
        for unsafe_header in ["Authorization", "cookie", "Host", "Content-Length"] {
            let text = format!("bind = \"127.0.0.1:1\"\n{base}user_header = {unsafe_header:?}\n");
            assert!(Config::from_toml(&text).is_err(), "{unsafe_header}");
        }
        assert!(Config::from_toml("[auth]\nmode = \"magic\"\n").is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::from_toml("bnd = \"127.0.0.1:1\"").is_err());
        assert!(Config::from_toml("[archive]\nafter_dayz = 1").is_err());
    }

    #[test]
    fn tilde_only_expands_leading_component() {
        assert_eq!(
            expand_tilde(Path::new("/a/~/b")).unwrap(),
            PathBuf::from("/a/~/b")
        );
        assert_eq!(expand_tilde(Path::new("~x")).unwrap(), PathBuf::from("~x"));
        assert_eq!(
            expand_tilde(Path::new("~/x")).unwrap(),
            dirs::home_dir().unwrap().join("x")
        );
    }
}
