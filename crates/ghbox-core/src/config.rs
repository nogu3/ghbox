use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub db_path: PathBuf,
    pub extra_patterns: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            db_path: default_db_path(),
            extra_patterns: Vec::new(),
        }
    }
}

fn default_db_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        })
        .join("ghbox")
        .join("state.db")
}

fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("ghbox")
        .join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let cfg = Config::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(cfg.poll_interval_secs, 300);
        assert!(cfg.extra_patterns.is_empty());
        assert!(cfg.db_path.ends_with("ghbox/state.db"));
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "poll_interval_secs = 60\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.poll_interval_secs, 60);
        assert!(cfg.extra_patterns.is_empty());
    }

    #[test]
    fn full_file_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "poll_interval_secs = 120\ndb_path = \"/nas/ghbox/state.db\"\nextra_patterns = [\"ship it\"]\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.poll_interval_secs, 120);
        assert_eq!(cfg.db_path, PathBuf::from("/nas/ghbox/state.db"));
        assert_eq!(cfg.extra_patterns, vec!["ship it".to_string()]);
    }

    #[test]
    fn unknown_key_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "typo_key = 1\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }
}
