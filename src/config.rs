use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default Claude Code projects directory.
pub fn projects_dir() -> PathBuf {
    dirs_home().join(".claude").join("projects")
}

/// Default database path: $XDG_DATA_HOME/trs/index.db
pub fn default_db_path() -> PathBuf {
    xdg_data_home().join("trs").join("index.db")
}

/// Default profiles config path: $XDG_CONFIG_HOME/trs/profiles.toml
pub fn default_profiles_path() -> PathBuf {
    xdg_config_home().join("trs").join("profiles.toml")
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

/// A single ingest profile: field mappings and defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldProfile {
    pub source: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, String>,
    #[serde(default)]
    pub defaults: HashMap<String, serde_json::Value>,
}

/// Top-level profiles config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilesConfig {
    #[serde(default)]
    pub profiles: HashMap<String, FieldProfile>,
}

/// Load profiles from a TOML file. Returns empty config if file doesn't exist.
pub fn load_profiles(config_path: &Path) -> Result<ProfilesConfig> {
    if !config_path.exists() {
        return Ok(ProfilesConfig::default());
    }
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let config: ProfilesConfig =
        toml::from_str(&content).with_context(|| format!("parsing {}", config_path.display()))?;
    Ok(config)
}

/// Walk a dotted path (e.g. "a.b.c") into a nested JSON value.
pub fn resolve_dot_path<'a>(
    obj: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = obj;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Apply a profile's field mappings and defaults to a raw JSON record.
pub fn apply_profile(
    record: &serde_json::Map<String, serde_json::Value>,
    profile: &FieldProfile,
) -> serde_json::Map<String, serde_json::Value> {
    let mut result = serde_json::Map::new();

    // Start with defaults
    for (k, v) in &profile.defaults {
        result.insert(k.clone(), v.clone());
    }

    let record_value = serde_json::Value::Object(record.clone());
    let mut remapped_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Apply field mappings
    for (src_path, dst_field) in &profile.fields {
        if let Some(value) = resolve_dot_path(&record_value, src_path) {
            result.insert(dst_field.clone(), value.clone());
        }
        if let Some(root_key) = src_path.split('.').next() {
            remapped_keys.insert(root_key.to_string());
        }
    }

    // Pass through unmapped keys
    for (key, value) in record {
        if !remapped_keys.contains(key) && !result.contains_key(key) {
            result.insert(key.clone(), value.clone());
        }
    }

    // Override source if specified
    if let Some(ref source) = profile.source {
        result.insert(
            "source".to_string(),
            serde_json::Value::String(source.clone()),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_dot_path() {
        let val: serde_json::Value = serde_json::json!({
            "a": { "b": { "c": 42 } },
            "x": "hello"
        });
        assert_eq!(
            resolve_dot_path(&val, "x"),
            Some(&serde_json::json!("hello"))
        );
        assert_eq!(
            resolve_dot_path(&val, "a.b.c"),
            Some(&serde_json::json!(42))
        );
        assert_eq!(resolve_dot_path(&val, "a.b.missing"), None);
        assert_eq!(resolve_dot_path(&val, "missing"), None);
    }

    #[test]
    fn test_apply_profile() {
        let profile = FieldProfile {
            source: Some("custom".into()),
            fields: HashMap::from([("data.text".into(), "body".into())]),
            defaults: HashMap::from([("slug".into(), serde_json::json!("default-slug"))]),
        };

        let mut record = serde_json::Map::new();
        record.insert("session_id".into(), serde_json::json!("s1"));
        record.insert("data".into(), serde_json::json!({"text": "hello world"}));
        record.insert("extra_key".into(), serde_json::json!("pass-through"));

        let result = apply_profile(&record, &profile);
        assert_eq!(result.get("body"), Some(&serde_json::json!("hello world")));
        assert_eq!(result.get("source"), Some(&serde_json::json!("custom")));
        assert_eq!(result.get("slug"), Some(&serde_json::json!("default-slug")));
        assert_eq!(
            result.get("extra_key"),
            Some(&serde_json::json!("pass-through"))
        );
        // "data" was remapped, so it should not be passed through
        assert!(!result.contains_key("data"));
    }

    #[test]
    fn test_default_paths() {
        // Just ensure these don't panic
        let _ = projects_dir();
        let _ = default_db_path();
        let _ = default_profiles_path();
    }
}
