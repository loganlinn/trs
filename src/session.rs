use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Supported source applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    ClaudeCode,
    Codex,
}

impl App {
    /// All known apps.
    pub const ALL: &[App] = &[App::ClaudeCode, App::Codex];

    /// Canonical source string stored in the database.
    pub fn source_str(&self) -> &'static str {
        match self {
            App::ClaudeCode => "claude-code",
            App::Codex => "codex",
        }
    }

    /// Parse from a source string or CLI alias.
    pub fn parse(s: &str) -> Option<App> {
        match s {
            "claude-code" | "claude" | "cc" => Some(App::ClaudeCode),
            "codex" | "cx" => Some(App::Codex),
            _ => None,
        }
    }

    /// Root directory where this app stores sessions.
    pub fn sessions_dirs(&self) -> Vec<PathBuf> {
        let home = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("~"));
        match self {
            App::ClaudeCode => vec![home.join(".claude").join("projects")],
            App::Codex => vec![
                home.join(".codex").join("sessions"),
                home.join(".codex").join("archived_sessions"),
            ],
        }
    }

    /// Build the resume command for a session from this app.
    pub fn resume_cmd(&self, session_id: &str) -> String {
        match self {
            App::ClaudeCode => format!("claude --resume {session_id}"),
            App::Codex => format!("codex --resume {session_id}"),
        }
    }

    /// The CLI binary name.
    pub fn bin_name(&self) -> &'static str {
        match self {
            App::ClaudeCode => "claude",
            App::Codex => "codex",
        }
    }
}

impl std::fmt::Display for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.source_str())
    }
}

/// Core session model stored in the database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub slug: String,
    pub source: String,
    pub cwd: String,
    pub git_branches: Vec<String>,
    pub start_time: String,
    pub end_time: String,
    pub files_touched: Vec<String>,
    pub tools_used: Vec<String>,
    pub message_count: i64,
    pub first_message: String,
    pub summary: String,
    pub body: String,
    pub content_hash: Option<String>,
    pub custom_title: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// NDJSON ingest record — the canonical external format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRecord {
    pub session_id: String,
    pub source: String,
    pub body: String,
    #[serde(default)]
    pub start_time: String,
    #[serde(default)]
    pub end_time: String,
    #[serde(default)]
    pub first_message: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub git_branches: Vec<String>,
    #[serde(default)]
    pub files_touched: Vec<String>,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Any extra fields not in the known set.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl IngestRecord {
    pub fn to_session(&self) -> Session {
        Session {
            session_id: self.session_id.clone(),
            source: self.source.clone(),
            body: self.body.clone(),
            start_time: self.start_time.clone(),
            end_time: self.end_time.clone(),
            first_message: self.first_message.clone(),
            summary: self.summary.clone(),
            message_count: self.message_count,
            cwd: self.cwd.clone(),
            slug: self.slug.clone(),
            git_branches: self.git_branches.clone(),
            files_touched: self.files_touched.clone(),
            tools_used: self.tools_used.clone(),
            content_hash: self.content_hash.clone(),
            custom_title: None,
            metadata: self.extra.clone(),
        }
    }
}

/// Extracted message from a session JSONL file.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Message {
    pub index: usize,
    pub role: MessageRole,
    pub text: String,
    pub teammate_id: String,
    pub teammate_summary: String,
    pub teammate_color: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    Summary,
    Teammate,
}

impl MessageRole {
    pub fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Summary => "summary",
            Self::Teammate => "teammate",
        }
    }
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A search result row from the database.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchResult {
    pub session_id: String,
    pub source: String,
    pub cwd: String,
    pub slug: String,
    pub git_branches: String,
    pub start_time: String,
    pub end_time: String,
    pub files_touched: String,
    pub tools_used: String,
    pub message_count: i64,
    pub first_message: String,
    pub summary: String,
    pub content_hash: Option<String>,
    pub custom_title: Option<String>,
    pub metadata: Option<String>,
    pub rank: f64,
}

/// Ingest record schema field descriptor (for `trs schema` output).
pub struct SchemaField {
    pub name: &'static str,
    pub type_name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

pub const INGEST_SCHEMA: &[SchemaField] = &[
    SchemaField {
        name: "session_id",
        type_name: "string",
        description: "Stable unique ID for this conversation",
        required: true,
    },
    SchemaField {
        name: "source",
        type_name: "string",
        description: "Application name (e.g. \"codex\", \"slack\")",
        required: true,
    },
    SchemaField {
        name: "body",
        type_name: "string",
        description: "Full text content for full-text search",
        required: true,
    },
    SchemaField {
        name: "start_time",
        type_name: "string",
        description: "ISO 8601 timestamp",
        required: false,
    },
    SchemaField {
        name: "end_time",
        type_name: "string",
        description: "ISO 8601 timestamp",
        required: false,
    },
    SchemaField {
        name: "first_message",
        type_name: "string",
        description: "Opening message (shown in search results)",
        required: false,
    },
    SchemaField {
        name: "summary",
        type_name: "string",
        description: "Short description (shown in search results)",
        required: false,
    },
    SchemaField {
        name: "message_count",
        type_name: "integer",
        description: "Number of messages in conversation",
        required: false,
    },
    SchemaField {
        name: "cwd",
        type_name: "string",
        description: "Working directory or context path",
        required: false,
    },
    SchemaField {
        name: "slug",
        type_name: "string",
        description: "Conversation title / channel name",
        required: false,
    },
    SchemaField {
        name: "git_branches",
        type_name: "array",
        description: "Git branches active during session",
        required: false,
    },
    SchemaField {
        name: "files_touched",
        type_name: "array",
        description: "File paths referenced",
        required: false,
    },
    SchemaField {
        name: "tools_used",
        type_name: "array",
        description: "Tool/command names used",
        required: false,
    },
    SchemaField {
        name: "content_hash",
        type_name: "string",
        description: "SHA-256 of source content (enables dedup on re-ingest)",
        required: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_record_to_session() {
        let record = IngestRecord {
            session_id: "abc123".into(),
            source: "codex".into(),
            body: "hello world".into(),
            start_time: String::new(),
            end_time: String::new(),
            first_message: "hi".into(),
            summary: String::new(),
            message_count: 5,
            cwd: "/tmp".into(),
            slug: "my-project".into(),
            git_branches: vec!["main".into()],
            files_touched: vec![],
            tools_used: vec![],
            content_hash: Some("deadbeef".into()),
            extra: HashMap::new(),
        };
        let sess = record.to_session();
        assert_eq!(sess.session_id, "abc123");
        assert_eq!(sess.source, "codex");
        assert_eq!(sess.message_count, 5);
        assert_eq!(sess.content_hash.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn test_ingest_record_deserialize() {
        let json = r#"{"session_id":"s1","source":"test","body":"text","custom_field":"extra"}"#;
        let rec: IngestRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.session_id, "s1");
        assert_eq!(rec.source, "test");
        assert!(rec.extra.contains_key("custom_field"));
    }

    #[test]
    fn test_message_role_display() {
        assert_eq!(MessageRole::User.as_str(), "user");
        assert_eq!(MessageRole::Assistant.to_string(), "assistant");
    }

    #[test]
    fn test_app_parse() {
        assert_eq!(App::parse("claude-code"), Some(App::ClaudeCode));
        assert_eq!(App::parse("claude"), Some(App::ClaudeCode));
        assert_eq!(App::parse("cc"), Some(App::ClaudeCode));
        assert_eq!(App::parse("codex"), Some(App::Codex));
        assert_eq!(App::parse("cx"), Some(App::Codex));
        assert_eq!(App::parse("unknown"), None);
    }

    #[test]
    fn test_app_source_str() {
        assert_eq!(App::ClaudeCode.source_str(), "claude-code");
        assert_eq!(App::Codex.source_str(), "codex");
    }

    #[test]
    fn test_app_resume_cmd() {
        assert_eq!(
            App::ClaudeCode.resume_cmd("abc-123"),
            "claude --resume abc-123"
        );
        assert_eq!(
            App::Codex.resume_cmd("abc-123"),
            "codex --resume abc-123"
        );
    }

    #[test]
    fn test_app_roundtrip() {
        for app in App::ALL {
            assert_eq!(App::parse(app.source_str()), Some(*app));
        }
    }
}
