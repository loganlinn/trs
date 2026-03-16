use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::config;
use crate::db;
use crate::session::{Message, MessageRole, Session};

/// File-modifying tool names (for tracking files_touched).
const FILE_TOOLS: &[&str] = &["Write", "Edit", "Read", "MultiEdit"];

static TEAMMATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<teammate-message\b([^>]*)>(.*?)</teammate-message>").expect("valid regex")
});

static TEAMMATE_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(\w+)="([^"]*)""#).expect("valid regex"));

#[derive(Debug)]
struct TeammateMsg {
    teammate_id: String,
    summary: String,
    body: String,
    #[allow(dead_code)]
    color: String,
}

/// Types of teammate lifecycle messages to skip.
const NOISE_TYPES: &[&str] = &[
    "idle_notification",
    "shutdown_approved",
    "shutdown_request",
    "teammate_terminated",
    "teammate_started",
];

/// Extract <teammate-message> blocks from text. Returns (remaining_text, teammates).
fn parse_teammate_messages(text: &str) -> (String, Vec<TeammateMsg>) {
    let mut teammates = Vec::new();
    let mut remaining = text.to_string();

    for cap in TEAMMATE_RE.captures_iter(text) {
        let full_match = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        let attrs_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = cap.get(2).map(|m| m.as_str()).unwrap_or("").trim();

        let mut attrs = std::collections::HashMap::new();
        for acap in TEAMMATE_ATTR_RE.captures_iter(attrs_str) {
            let key = acap.get(1).map(|m| m.as_str()).unwrap_or("");
            let val = acap.get(2).map(|m| m.as_str()).unwrap_or("");
            attrs.insert(key.to_string(), val.to_string());
        }

        let tid = attrs.get("teammate_id").cloned().unwrap_or_default();
        let summary = attrs.get("summary").cloned().unwrap_or_default();
        let color = attrs.get("color").cloned().unwrap_or_default();

        // Skip system/lifecycle noise
        if tid == "system" {
            remaining = remaining.replacen(full_match, "", 1);
            continue;
        }
        if summary.is_empty() && body.starts_with('{') {
            if let Ok(jbody) = serde_json::from_str::<serde_json::Value>(body) {
                if let Some(t) = jbody.get("type").and_then(|v| v.as_str()) {
                    if NOISE_TYPES.contains(&t) {
                        remaining = remaining.replacen(full_match, "", 1);
                        continue;
                    }
                }
            }
        }
        teammates.push(TeammateMsg {
            teammate_id: tid,
            summary,
            body: body.to_string(),
            color,
        });
        remaining = remaining.replacen(full_match, "", 1);
    }

    (remaining.trim().to_string(), teammates)
}

/// Extract text from a JSON content field (string or array of blocks).
fn text_from_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut parts = Vec::new();
            for block in arr {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        parts.push(text.to_string());
                    }
                }
            }
            parts.join(" ")
        }
        _ => String::new(),
    }
}

/// Extract message content from a JSONL record.
fn rec_content(rec: &serde_json::Value) -> Option<serde_json::Value> {
    let msg = rec.get("message")?;
    if msg.is_object() {
        msg.get("content").cloned()
    } else {
        None
    }
}

/// Truncate a string to at most `max` bytes at a char boundary.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Glob all session JSONL files under the projects directory.
pub fn glob_sessions() -> Vec<PathBuf> {
    glob_sessions_in(&config::projects_dir())
}

/// Glob session JSONL files under a specific directory.
pub fn glob_sessions_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if !dir.exists() {
        return paths;
    }
    walk_jsonl(dir, &mut paths);
    paths
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "subagents" || name_str == "tool-results" {
                continue;
            }
            walk_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Parse a session JSONL file into a Session.
pub fn parse_session(path: &Path) -> Result<Session> {
    let slug = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let session_id = path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut body_parts: Vec<String> = Vec::new();
    let mut branches: BTreeSet<String> = BTreeSet::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut tools: BTreeSet<String> = BTreeSet::new();
    let mut cwd = String::new();
    let mut start_time = String::new();
    let mut end_time = String::new();
    let mut message_count: i64 = 0;
    let mut first_message = String::new();
    let mut summary = String::new();

    let content = std::fs::read_to_string(path)?;
    for raw in content.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let rec: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(c) = rec.get("cwd").and_then(|v| v.as_str()) {
            if cwd.is_empty() && !c.is_empty() {
                cwd = c.to_string();
            }
        }
        if let Some(b) = rec.get("gitBranch").and_then(|v| v.as_str()) {
            if !b.is_empty() {
                branches.insert(b.to_string());
            }
        }
        if let Some(ts) = rec.get("timestamp").and_then(|v| v.as_str()) {
            if !ts.is_empty() {
                if start_time.is_empty() {
                    start_time = ts.to_string();
                }
                end_time = ts.to_string();
            }
        }

        let rtype = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match rtype {
            "user" => {
                let content_val = rec_content(&rec)
                    .or_else(|| rec.get("content").cloned())
                    .unwrap_or(serde_json::Value::Null);
                let text = text_from_content(&content_val);
                if !text.is_empty() {
                    let (remaining, teammates) = parse_teammate_messages(&text);
                    for tm in &teammates {
                        message_count += 1;
                        let body_text = truncate(&tm.body, 500);
                        body_parts
                            .push(format!("{}: {} {}", tm.teammate_id, tm.summary, body_text));
                    }
                    if !remaining.is_empty() {
                        message_count += 1;
                        if first_message.is_empty() {
                            first_message = truncate(&remaining, 500).to_string();
                        }
                        body_parts.push(truncate(&remaining, 1000).to_string());
                    }
                }
            }
            "assistant" => {
                let content_val = match rec_content(&rec) {
                    Some(v) => v,
                    None => continue,
                };
                let arr = match content_val.as_array() {
                    Some(a) => a,
                    None => continue,
                };
                message_count += 1;
                for block in arr {
                    let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match btype {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                if !t.is_empty() {
                                    body_parts.push(truncate(t, 1000).to_string());
                                }
                            }
                        }
                        "tool_use" => {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if !name.is_empty() {
                                tools.insert(name.to_string());
                            }
                            let inp = block.get("input").unwrap_or(&serde_json::Value::Null);
                            if FILE_TOOLS.contains(&name) {
                                if let Some(fp) = inp.get("file_path").and_then(|v| v.as_str()) {
                                    if !fp.is_empty() {
                                        files.insert(fp.to_string());
                                    }
                                }
                            } else if name == "Bash" {
                                if let Some(cmd) = inp.get("command").and_then(|v| v.as_str()) {
                                    if !cmd.is_empty() {
                                        body_parts.push(truncate(cmd, 200).to_string());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "summary" => {
                if let Some(s) = rec.get("summary").and_then(|v| v.as_str()) {
                    if !s.is_empty() {
                        summary = s.to_string();
                    }
                }
            }
            _ => {}
        }
    }

    Ok(Session {
        session_id,
        slug,
        source: "claude-code".into(),
        cwd,
        start_time,
        end_time,
        message_count,
        first_message,
        summary,
        git_branches: branches.into_iter().collect(),
        files_touched: files.into_iter().collect(),
        tools_used: tools.into_iter().collect(),
        body: body_parts.join(" "),
        content_hash: None,
        metadata: Default::default(),
    })
}

/// Extract ordered messages from a session JSONL (for display with context).
pub fn extract_messages(path: &Path) -> Result<Vec<Message>> {
    let mut messages = Vec::new();
    let mut idx: usize = 0;

    let content = std::fs::read_to_string(path)?;
    for raw in content.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let rec: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let rtype = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match rtype {
            "user" => {
                let content_val = rec_content(&rec)
                    .or_else(|| rec.get("content").cloned())
                    .unwrap_or(serde_json::Value::Null);
                let text = text_from_content(&content_val);
                if !text.is_empty() {
                    let (remaining, teammates) = parse_teammate_messages(&text);
                    for tm in teammates {
                        messages.push(Message {
                            index: idx,
                            role: MessageRole::Teammate,
                            text: tm.body,
                            teammate_id: tm.teammate_id,
                            teammate_summary: tm.summary,
                            teammate_color: tm.color,
                        });
                        idx += 1;
                    }
                    if !remaining.is_empty() {
                        messages.push(Message {
                            index: idx,
                            role: MessageRole::User,
                            text: remaining,
                            teammate_id: String::new(),
                            teammate_summary: String::new(),
                            teammate_color: String::new(),
                        });
                        idx += 1;
                    }
                }
            }
            "assistant" => {
                let content_val = match rec_content(&rec) {
                    Some(v) => v,
                    None => continue,
                };
                let arr = match content_val.as_array() {
                    Some(a) => a,
                    None => continue,
                };
                let mut parts = Vec::new();
                for block in arr {
                    let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match btype {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                if !t.is_empty() {
                                    parts.push(t.to_string());
                                }
                            }
                        }
                        "tool_use" => {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let inp = block.get("input").unwrap_or(&serde_json::Value::Null);
                            if FILE_TOOLS.contains(&name) {
                                if let Some(fp) = inp.get("file_path").and_then(|v| v.as_str()) {
                                    if !fp.is_empty() {
                                        parts.push(format!("[{name} {fp}]"));
                                    }
                                }
                            } else if name == "Bash" {
                                if let Some(cmd) = inp.get("command").and_then(|v| v.as_str()) {
                                    if !cmd.is_empty() {
                                        parts.push(format!("$ {}", truncate(cmd, 120)));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    messages.push(Message {
                        index: idx,
                        role: MessageRole::Assistant,
                        text: parts.join("\n"),
                        teammate_id: String::new(),
                        teammate_summary: String::new(),
                        teammate_color: String::new(),
                    });
                    idx += 1;
                }
            }
            "summary" => {
                if let Some(s) = rec.get("summary").and_then(|v| v.as_str()) {
                    if !s.is_empty() {
                        messages.push(Message {
                            index: idx,
                            role: MessageRole::Summary,
                            text: s.to_string(),
                            teammate_id: String::new(),
                            teammate_summary: String::new(),
                            teammate_color: String::new(),
                        });
                        idx += 1;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(messages)
}

/// Run the indexer: scan sessions, parse, upsert, prune dead.
pub fn run_index(db_path: &Path, full: bool) -> Result<()> {
    let conn = db::open_db(db_path, true)?;

    let stored = if full {
        std::collections::HashMap::new()
    } else {
        db::get_stored_mtimes(&conn)?
    };
    let db_ids = db::get_file_backed_ids(&conn)?;

    let paths = glob_sessions();
    let live_ids: HashSet<String> = paths
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();

    let total = paths.len();
    let mut skipped: usize = 0;
    let mut indexed: usize = 0;

    let pb = indicatif::ProgressBar::new(total as u64);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {pos}/{len}")
            .expect("valid template")
            .progress_chars("=> "),
    );
    pb.set_message("Indexing");

    for path in &paths {
        pb.inc(1);

        let mtime = match path.metadata() {
            Ok(m) => m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            Err(_) => continue,
        };

        let sid = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        if !full {
            if let Some(&stored_mtime) = stored.get(&sid) {
                if stored_mtime >= mtime {
                    skipped += 1;
                    continue;
                }
            }
        }

        match parse_session(path) {
            Ok(sess) => {
                if let Err(e) = db::upsert_session(&conn, &sess, mtime) {
                    eprintln!("Warning: {}: {e}", path.display());
                    continue;
                }
                indexed += 1;
                if indexed.is_multiple_of(50) {
                    // Explicit checkpoint not needed with rusqlite's auto-commit,
                    // but we can batch if we wrapped in a transaction.
                }
            }
            Err(e) => {
                eprintln!("Warning: {}: {e}", path.display());
            }
        }
    }

    pb.finish_and_clear();

    // Prune dead sessions
    let dead_ids: Vec<String> = db_ids.difference(&live_ids).cloned().collect();
    if !dead_ids.is_empty() {
        db::delete_sessions(&conn, &dead_ids)?;
    }

    // Summary
    eprint!("Done. {indexed} indexed, {skipped} skipped");
    if !dead_ids.is_empty() {
        eprint!(", {} pruned", dead_ids.len());
    }
    eprintln!(" (of {total} total).");
    eprintln!("DB: {}", db_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(dir: &Path, slug: &str, session_id: &str, lines: &[&str]) -> PathBuf {
        let session_dir = dir.join(slug);
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join(format!("{session_id}.jsonl"));
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn test_parse_session_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(
            dir.path(),
            "my-project",
            "sess-001",
            &[
                r#"{"type":"user","timestamp":"2024-01-01T00:00:00.000Z","cwd":"/tmp/proj","message":{"content":"help me write rust"}}"#,
                r#"{"type":"assistant","timestamp":"2024-01-01T00:01:00.000Z","message":{"content":[{"type":"text","text":"Sure, here is some Rust code."}]}}"#,
                r#"{"type":"summary","summary":"Helped with Rust code"}"#,
            ],
        );

        let sess = parse_session(&path).unwrap();
        assert_eq!(sess.session_id, "sess-001");
        assert_eq!(sess.slug, "my-project");
        assert_eq!(sess.cwd, "/tmp/proj");
        assert_eq!(sess.message_count, 2);
        assert_eq!(sess.first_message, "help me write rust");
        assert_eq!(sess.summary, "Helped with Rust code");
        assert!(sess.body.contains("help me write rust"));
        assert!(sess.body.contains("Sure, here is some Rust code."));
    }

    #[test]
    fn test_parse_session_tool_use() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(
            dir.path(),
            "proj",
            "sess-002",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/tmp/test.rs"}},{"type":"tool_use","name":"Bash","input":{"command":"cargo build"}}]}}"#,
            ],
        );

        let sess = parse_session(&path).unwrap();
        assert!(sess.files_touched.contains(&"/tmp/test.rs".to_string()));
        assert!(sess.tools_used.contains(&"Bash".to_string()));
        assert!(sess.tools_used.contains(&"Write".to_string()));
        assert!(sess.body.contains("cargo build"));
    }

    #[test]
    fn test_extract_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(
            dir.path(),
            "proj",
            "sess-003",
            &[
                r#"{"type":"user","message":{"content":"hello"}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi there"}]}}"#,
            ],
        );

        let msgs = extract_messages(&path).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[0].text, "hello");
        assert_eq!(msgs[1].role, MessageRole::Assistant);
        assert_eq!(msgs[1].text, "hi there");
    }

    #[test]
    fn test_teammate_parsing() {
        let text = r#"<teammate-message teammate_id="worker1" summary="doing stuff" color="blue">worked on feature</teammate-message> remaining text"#;
        let (remaining, teammates) = parse_teammate_messages(text);
        assert_eq!(teammates.len(), 1);
        assert_eq!(teammates[0].teammate_id, "worker1");
        assert_eq!(teammates[0].summary, "doing stuff");
        assert_eq!(teammates[0].body, "worked on feature");
        assert_eq!(remaining, "remaining text");
    }

    #[test]
    fn test_teammate_system_skip() {
        let text = r#"<teammate-message teammate_id="system">noise</teammate-message>keep"#;
        let (remaining, teammates) = parse_teammate_messages(text);
        assert!(teammates.is_empty());
        assert_eq!(remaining, "keep");
    }

    #[test]
    fn test_glob_sessions_excludes_subagents() {
        let dir = tempfile::tempdir().unwrap();
        // Valid session
        write_fixture(dir.path(), "proj", "s1", &[r#"{"type":"user"}"#]);
        // Should be excluded
        let subagent_dir = dir.path().join("proj").join("subagents");
        std::fs::create_dir_all(&subagent_dir).unwrap();
        std::fs::write(subagent_dir.join("s2.jsonl"), r#"{"type":"user"}"#).unwrap();

        let paths = glob_sessions_in(dir.path());
        assert_eq!(paths.len(), 1);
        assert!(paths[0].to_string_lossy().contains("s1.jsonl"));
    }

    #[test]
    fn test_text_from_content_string() {
        let v = serde_json::json!("hello");
        assert_eq!(text_from_content(&v), "hello");
    }

    #[test]
    fn test_text_from_content_blocks() {
        let v = serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "image", "data": "..."},
            {"type": "text", "text": "world"}
        ]);
        assert_eq!(text_from_content(&v), "hello world");
    }
}
