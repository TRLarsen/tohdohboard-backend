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

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 3001)),
            data_dir: PathBuf::from("~/.tohdohboard"),
            archive: ArchiveConfig::default(),
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
    /// Fails on invalid TOML, unknown keys, wrongly typed values, or when `~`
    /// is used but the home directory cannot be determined.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut config: Config = toml::from_str(text)?;
        config.data_dir = expand_tilde(&config.data_dir)?;
        Ok(config)
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
