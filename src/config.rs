use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_paths() {
        // Just ensure these don't panic
        let _ = projects_dir();
        let _ = default_db_path();
    }
}
