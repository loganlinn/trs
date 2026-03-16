use thiserror::Error;

#[allow(dead_code)]
#[derive(Error, Debug)]
pub enum TrsError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("index not found — run `trs index` first")]
    IndexNotFound,

    #[error("no results")]
    NoResults,

    #[error("query error: {0}")]
    QueryError(String),

    #[error("profile {0:?} not found in {1}")]
    ProfileNotFound(String, String),

    #[error("validation error on line {line}: {message}")]
    Validation { line: usize, message: String },

    #[error("{0}")]
    Config(String),
}
