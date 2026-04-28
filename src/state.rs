//! Persisted TUI state — saved before exec'ing the resume target so a
//! follow-up `trs --continue` can restore the search context.
//!
//! Stored at `$XDG_STATE_HOME/trs/last.json` (fallback `~/.local/state/trs/last.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub pinned_branch: Option<String>,
    #[serde(default)]
    pub pinned_project: Option<String>,
    #[serde(default)]
    pub selected_session_id: Option<String>,
    #[serde(default)]
    pub saved_at: String,
}

pub fn state_path() -> PathBuf {
    xdg_state_home().join("trs").join("last.json")
}

/// Persist state for later restoration. Errors are logged but non-fatal.
pub fn save(state: &TuiState) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("failed to create {}: {e}", parent.display());
            return;
        }
    }
    let mut to_write = state.clone();
    to_write.version = STATE_VERSION;
    if to_write.saved_at.is_empty() {
        to_write.saved_at = chrono::Utc::now().to_rfc3339();
    }
    match serde_json::to_string_pretty(&to_write) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("failed to write {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("failed to serialize state: {e}"),
    }
}

/// Load previously saved state, or `None` if missing or unparseable.
pub fn load() -> Option<TuiState> {
    let path = state_path();
    let contents = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<TuiState>(&contents) {
        Ok(s) if s.version == STATE_VERSION => Some(s),
        Ok(s) => {
            tracing::warn!(
                "state version mismatch (got {}, want {}): {}",
                s.version,
                STATE_VERSION,
                path.display()
            );
            None
        }
        Err(e) => {
            tracing::warn!("failed to parse {}: {e}", path.display());
            None
        }
    }
}

fn xdg_state_home() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".local").join("state"))
                .unwrap_or_else(|| PathBuf::from(".local/state"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests are single-threaded enough here; XDG override is local.
        unsafe {
            std::env::set_var("XDG_STATE_HOME", dir.path());
        }

        let state = TuiState {
            input: "project:foo bar".into(),
            pinned_branch: Some("main".into()),
            pinned_project: None,
            selected_session_id: Some("abc-123".into()),
            ..TuiState::default()
        };
        save(&state);

        let loaded = load().expect("state present");
        assert_eq!(loaded.input, "project:foo bar");
        assert_eq!(loaded.pinned_branch.as_deref(), Some("main"));
        assert_eq!(loaded.selected_session_id.as_deref(), Some("abc-123"));
        assert_eq!(loaded.version, STATE_VERSION);
    }
}
