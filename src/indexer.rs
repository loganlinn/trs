use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::db;
use crate::session::{App, Message, MessageRole, Session};

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

/// Glob all session JSONL files for a specific app.
pub fn glob_sessions_for(app: &App) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in app.sessions_dirs() {
        if dir.exists() {
            match app {
                App::ClaudeCode => walk_jsonl_claude(&dir, &mut paths),
                App::Codex => walk_jsonl_all(&dir, &mut paths),
            }
        }
    }
    paths
}

/// Glob all session JSONL files across all apps. Returns (app, path) pairs.
pub fn glob_all_sessions(apps: &[App]) -> Vec<(App, PathBuf)> {
    let mut result = Vec::new();
    for app in apps {
        for path in glob_sessions_for(app) {
            result.push((*app, path));
        }
    }
    result
}

/// Glob session JSONL files under a specific directory (Claude Code rules: skip subagents/tool-results).
#[cfg(test)]
pub fn glob_sessions_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if !dir.exists() {
        return paths;
    }
    walk_jsonl_claude(dir, &mut paths);
    paths
}

fn walk_jsonl_claude(dir: &Path, out: &mut Vec<PathBuf>) {
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
            walk_jsonl_claude(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn walk_jsonl_all(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl_all(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Parse a session JSONL file into a Session, dispatching by app.
pub fn parse_session(path: &Path, app: &App) -> Result<Session> {
    match app {
        App::ClaudeCode => parse_claude_session(path),
        App::Codex => parse_codex_session(path),
    }
}

/// Extract ordered messages from a session JSONL, dispatching by app.
pub fn extract_messages_for(path: &Path, app: &App) -> Result<Vec<Message>> {
    match app {
        App::ClaudeCode => extract_messages(path),
        App::Codex => extract_codex_messages(path),
    }
}

/// Parse a Claude Code session JSONL file into a Session.
fn parse_claude_session(path: &Path) -> Result<Session> {
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

/// Codex tool names that indicate file operations.
const CODEX_FILE_TOOLS: &[&str] = &["read_file", "write_file", "edit_file", "patch_file"];

/// Parse a Codex session JSONL file into a Session.
fn parse_codex_session(path: &Path) -> Result<Session> {
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
    let mut session_id = String::new();

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

        if let Some(ts) = rec.get("timestamp").and_then(|v| v.as_str()) {
            if !ts.is_empty() {
                if start_time.is_empty() {
                    start_time = ts.to_string();
                }
                end_time = ts.to_string();
            }
        }

        let rtype = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = rec.get("payload").unwrap_or(&serde_json::Value::Null);

        match rtype {
            "session_meta" => {
                if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
                if let Some(c) = payload.get("cwd").and_then(|v| v.as_str()) {
                    if cwd.is_empty() && !c.is_empty() {
                        cwd = c.to_string();
                    }
                }
                if let Some(git) = payload.get("git") {
                    if let Some(b) = git.get("branch").and_then(|v| v.as_str()) {
                        if !b.is_empty() {
                            branches.insert(b.to_string());
                        }
                    }
                }
            }
            "turn_context" => {
                if let Some(s) = payload.get("summary").and_then(|v| v.as_str()) {
                    if !s.is_empty() {
                        summary = s.to_string();
                    }
                }
            }
            "response_item" => {
                let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let content_arr = match payload.get("content").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => continue,
                };

                match role {
                    "user" => {
                        for block in content_arr {
                            let btype =
                                block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            if btype == "input_text" {
                                let text = block
                                    .get("text")
                                    .or_else(|| block.get("input_text"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !text.is_empty()
                                    && !text.starts_with('<')
                                    && !text.contains("<permissions")
                                    && !text.contains("<environment_context")
                                {
                                    message_count += 1;
                                    if first_message.is_empty() {
                                        first_message = truncate(text, 500).to_string();
                                    }
                                    body_parts.push(truncate(text, 1000).to_string());
                                }
                            }
                        }
                    }
                    "assistant" => {
                        message_count += 1;
                        for block in content_arr {
                            let btype =
                                block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match btype {
                                "output_text" => {
                                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                        if !t.is_empty() {
                                            body_parts.push(truncate(t, 1000).to_string());
                                        }
                                    }
                                }
                                "function_call" => {
                                    let name = block
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if !name.is_empty() {
                                        tools.insert(name.to_string());
                                    }
                                    // Parse arguments (JSON string)
                                    if let Some(args_str) =
                                        block.get("arguments").and_then(|v| v.as_str())
                                    {
                                        if let Ok(args) =
                                            serde_json::from_str::<serde_json::Value>(args_str)
                                        {
                                            // Extract file paths from common arg patterns
                                            for key in &["path", "file_path", "file"] {
                                                if let Some(fp) =
                                                    args.get(key).and_then(|v| v.as_str())
                                                {
                                                    if !fp.is_empty() {
                                                        files.insert(fp.to_string());
                                                    }
                                                }
                                            }
                                            // Extract shell commands
                                            if let Some(cmd) =
                                                args.get("command").and_then(|v| v.as_array())
                                            {
                                                let cmd_str: Vec<&str> = cmd
                                                    .iter()
                                                    .filter_map(|v| v.as_str())
                                                    .collect();
                                                if !cmd_str.is_empty() {
                                                    body_parts.push(
                                                        truncate(&cmd_str.join(" "), 200)
                                                            .to_string(),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    if CODEX_FILE_TOOLS.contains(&name) {
                                        // Already handled via args parsing
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    // Skip developer (system prompt) role
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Derive slug from cwd (last meaningful path component)
    let slug = derive_slug_from_cwd(&cwd);

    // Fall back to filename-based session_id if session_meta was missing
    if session_id.is_empty() {
        session_id = path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
    }

    Ok(Session {
        session_id,
        slug,
        source: App::Codex.source_str().into(),
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

/// Derive a project slug from a working directory path.
fn derive_slug_from_cwd(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    let p = Path::new(cwd);
    // For paths like /Users/x/src/github.com/org/repo, use "org/repo" or just "repo"
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Extract ordered messages from a Codex session JSONL (for display with context).
pub fn extract_codex_messages(path: &Path) -> Result<Vec<Message>> {
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
        if rtype != "response_item" {
            continue;
        }

        let payload = rec.get("payload").unwrap_or(&serde_json::Value::Null);
        let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content_arr = match payload.get("content").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => continue,
        };

        match role {
            "user" => {
                for block in content_arr {
                    let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if btype == "input_text" {
                        let text = block
                            .get("text")
                            .or_else(|| block.get("input_text"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !text.is_empty()
                            && !text.starts_with('<')
                            && !text.contains("<permissions")
                            && !text.contains("<environment_context")
                        {
                            messages.push(Message {
                                index: idx,
                                role: MessageRole::User,
                                text: text.to_string(),
                                teammate_id: String::new(),
                                teammate_summary: String::new(),
                                teammate_color: String::new(),
                            });
                            idx += 1;
                        }
                    }
                }
            }
            "assistant" => {
                let mut parts = Vec::new();
                for block in content_arr {
                    let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match btype {
                        "output_text" => {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                if !t.is_empty() {
                                    parts.push(t.to_string());
                                }
                            }
                        }
                        "function_call" => {
                            let name =
                                block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(args_str) =
                                block.get("arguments").and_then(|v| v.as_str())
                            {
                                if let Ok(args) =
                                    serde_json::from_str::<serde_json::Value>(args_str)
                                {
                                    if let Some(fp) = args
                                        .get("path")
                                        .or_else(|| args.get("file_path"))
                                        .and_then(|v| v.as_str())
                                    {
                                        parts.push(format!("[{name} {fp}]"));
                                    } else if let Some(cmd) =
                                        args.get("command").and_then(|v| v.as_array())
                                    {
                                        let cmd_str: Vec<&str> =
                                            cmd.iter().filter_map(|v| v.as_str()).collect();
                                        parts.push(format!(
                                            "$ {}",
                                            truncate(&cmd_str.join(" "), 120)
                                        ));
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
            _ => {}
        }
    }

    Ok(messages)
}

/// Run the indexer: scan sessions, parse, upsert, prune dead.
/// When `app_filter` is None, indexes all apps.
pub fn run_index(db_path: &Path, full: bool, app_filter: Option<&App>) -> Result<()> {
    let conn = db::open_db(db_path, true)?;

    let stored = if full {
        std::collections::HashMap::new()
    } else {
        db::get_stored_mtimes(&conn)?
    };
    let db_ids = db::get_file_backed_ids(&conn)?;

    let apps: &[App] = match app_filter {
        Some(app) => std::slice::from_ref(app),
        None => App::ALL,
    };
    let all_paths = glob_all_sessions(apps);

    // For Codex, session_id comes from inside the file, not the filename.
    // We need to track live IDs after parsing.
    let mut live_ids: HashSet<String> = HashSet::new();

    let total = all_paths.len();
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

    for (app, path) in &all_paths {
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

        // For Claude Code, we can use the filename as a quick mtime check key.
        // For Codex, we also use the filename (unique enough for mtime tracking).
        let mtime_key = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        if !full {
            if let Some(&stored_mtime) = stored.get(&mtime_key) {
                if stored_mtime >= mtime {
                    // Still need to track the live ID for pruning.
                    // For Claude Code, filename stem IS the session_id.
                    // For Codex, we stored the session_id when we indexed it,
                    // but for incremental skip we can't know it without parsing.
                    // Use the mtime_key as a proxy — it's in stored mtimes, so
                    // the corresponding session_id is already in the DB.
                    live_ids.insert(mtime_key);
                    skipped += 1;
                    continue;
                }
            }
        }

        match parse_session(path, app) {
            Ok(sess) => {
                live_ids.insert(sess.session_id.clone());
                if let Err(e) = db::upsert_session(&conn, &sess, mtime) {
                    eprintln!("Warning: {}: {e}", path.display());
                    continue;
                }
                indexed += 1;
            }
            Err(e) => {
                eprintln!("Warning: {}: {e}", path.display());
            }
        }
    }

    pb.finish_and_clear();

    // Prune dead sessions (only for the apps we're indexing)
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

        let sess = parse_session(&path, &App::ClaudeCode).unwrap();
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

        let sess = parse_session(&path, &App::ClaudeCode).unwrap();
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

    fn write_codex_fixture(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(format!("{name}.jsonl"));
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn test_parse_codex_session_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_codex_fixture(
            dir.path(),
            "rollout-2026-03-18T15-00-00-abc123",
            &[
                r#"{"timestamp":"2026-03-18T22:00:00.000Z","type":"session_meta","payload":{"id":"abc-123-uuid","timestamp":"2026-03-18T22:00:00.000Z","cwd":"/Users/me/project","originator":"codex-tui","git":{"branch":"main"}}}"#,
                r#"{"timestamp":"2026-03-18T22:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"help me fix this bug"}]}}"#,
                r#"{"timestamp":"2026-03-18T22:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I'll take a look at the code."}]}}"#,
                r#"{"timestamp":"2026-03-18T22:00:03.000Z","type":"turn_context","payload":{"summary":"Fixed a bug in the parser"}}"#,
            ],
        );

        let sess = parse_codex_session(&path).unwrap();
        assert_eq!(sess.session_id, "abc-123-uuid");
        assert_eq!(sess.source, "codex");
        assert_eq!(sess.cwd, "/Users/me/project");
        assert_eq!(sess.slug, "project");
        assert_eq!(sess.message_count, 2);
        assert_eq!(sess.first_message, "help me fix this bug");
        assert_eq!(sess.summary, "Fixed a bug in the parser");
        assert!(sess.body.contains("help me fix this bug"));
        assert!(sess.body.contains("I'll take a look at the code."));
        assert!(sess.git_branches.contains(&"main".to_string()));
    }

    #[test]
    fn test_parse_codex_session_tool_use() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_codex_fixture(
            dir.path(),
            "rollout-tools",
            &[
                r#"{"timestamp":"2026-03-18T22:00:00.000Z","type":"session_meta","payload":{"id":"tool-sess","cwd":"/tmp/proj"}}"#,
                r#"{"timestamp":"2026-03-18T22:00:01.000Z","type":"response_item","payload":{"role":"assistant","content":[{"type":"function_call","name":"read_file","arguments":"{\"path\":\"/tmp/proj/main.rs\"}"},{"type":"function_call","name":"shell","arguments":"{\"command\":[\"cargo\",\"build\"]}"}]}}"#,
            ],
        );

        let sess = parse_codex_session(&path).unwrap();
        assert!(sess.files_touched.contains(&"/tmp/proj/main.rs".to_string()));
        assert!(sess.tools_used.contains(&"read_file".to_string()));
        assert!(sess.tools_used.contains(&"shell".to_string()));
        assert!(sess.body.contains("cargo build"));
    }

    #[test]
    fn test_parse_codex_skips_system_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_codex_fixture(
            dir.path(),
            "rollout-sys",
            &[
                r#"{"timestamp":"2026-03-18T22:00:00.000Z","type":"session_meta","payload":{"id":"sys-sess","cwd":"/tmp"}}"#,
                r#"{"timestamp":"2026-03-18T22:00:01.000Z","type":"response_item","payload":{"role":"developer","content":[{"type":"input_text","text":"<permissions instructions>You are..."}]}}"#,
                r#"{"timestamp":"2026-03-18T22:00:02.000Z","type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"<environment_context>\n<cwd>/tmp</cwd>"}]}}"#,
                r#"{"timestamp":"2026-03-18T22:00:03.000Z","type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"actual user message"}]}}"#,
            ],
        );

        let sess = parse_codex_session(&path).unwrap();
        assert_eq!(sess.message_count, 1);
        assert_eq!(sess.first_message, "actual user message");
    }

    #[test]
    fn test_extract_codex_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_codex_fixture(
            dir.path(),
            "rollout-msgs",
            &[
                r#"{"timestamp":"2026-03-18T22:00:00.000Z","type":"session_meta","payload":{"id":"msg-sess","cwd":"/tmp"}}"#,
                r#"{"timestamp":"2026-03-18T22:00:01.000Z","type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"hello codex"}]}}"#,
                r#"{"timestamp":"2026-03-18T22:00:02.000Z","type":"response_item","payload":{"role":"assistant","content":[{"type":"output_text","text":"hi there"}]}}"#,
            ],
        );

        let msgs = extract_codex_messages(&path).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[0].text, "hello codex");
        assert_eq!(msgs[1].role, MessageRole::Assistant);
        assert_eq!(msgs[1].text, "hi there");
    }

    #[test]
    fn test_derive_slug_from_cwd() {
        assert_eq!(
            derive_slug_from_cwd("/Users/me/src/github.com/org/repo"),
            "repo"
        );
        assert_eq!(derive_slug_from_cwd(""), "");
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
