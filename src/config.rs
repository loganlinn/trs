use std::path::PathBuf;

use serde::Deserialize;

use crate::keys::KeyBindings;

/// Top-level configuration loaded from `$XDG_CONFIG_HOME/trs/config.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub keys: KeyBindings,
}

impl Config {
    /// Load config from the XDG config path, falling back to defaults.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!("failed to parse {}: {e}", path.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }
}

/// Default Claude Code projects directory.
pub fn projects_dir() -> PathBuf {
    dirs_home().join(".claude").join("projects")
}

/// Default database path: $XDG_DATA_HOME/trs/index.db
pub fn default_db_path() -> PathBuf {
    xdg_data_home().join("trs").join("index.db")
}

/// Default log directory: $XDG_DATA_HOME/trs/
pub fn log_dir() -> PathBuf {
    xdg_data_home().join("trs")
}

/// Config file path: $XDG_CONFIG_HOME/trs/config.toml
pub fn config_path() -> PathBuf {
    xdg_config_home().join("trs").join("config.toml")
}

fn dirs_home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~"))
}

fn xdg_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".local").join("share"))
}

fn xdg_config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_paths() {
        let _ = projects_dir();
        let _ = default_db_path();
        let _ = config_path();
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(!config.keys.normal.quit.display().is_empty());
    }
}
