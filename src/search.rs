use anyhow::Result;
use regex::Regex;
use std::path::Path;

use crate::config;
use crate::db;
use crate::indexer;
use crate::output;
use crate::session::{App, Message, SearchResult};

/// Parsed search input with extracted filter prefixes.
#[derive(Debug, Default, Clone)]
pub struct ParsedQuery {
    pub text: String,
    pub app: Option<String>,
    pub project: Option<String>,
    pub file: Option<String>,
    pub branch: Option<String>,
}

impl ParsedQuery {
    pub fn source_filter(&self) -> Option<&str> {
        self.app
            .as_deref()
            .and_then(App::parse)
            .map(|a| a.source_str())
    }
}

/// Parse a search string, extracting `key:value` filter prefixes.
///
/// Supported filters: `app:`, `project:`/`p:`, `file:`/`f:`, `branch:`/`b:`.
/// Values can be quoted: `project:"my app"`. Remaining tokens form the FTS query.
pub fn parse_query(input: &str) -> ParsedQuery {
    let mut q = ParsedQuery::default();
    let mut text_parts = Vec::new();

    let mut chars = input.chars().peekable();
    while chars.peek().is_some() {
        // skip whitespace
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        // collect a token
        let mut token = String::new();
        while let Some(&c) = chars.peek() {
            if c == ' ' {
                break;
            }
            token.push(c);
            chars.next();
        }

        // check for key:value
        if let Some(colon_pos) = token.find(':') {
            let key = &token[..colon_pos];
            let mut value = token[colon_pos + 1..].to_string();

            // handle quoted values: key:"value with spaces"
            if value.starts_with('"') && !value.ends_with('"') {
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    value.push(c);
                }
            }
            let value = value.trim_matches('"').to_string();

            match key {
                "app" | "a" => q.app = Some(value),
                "project" | "p" => q.project = Some(value),
                "file" | "f" => q.file = Some(value),
                "branch" | "b" => q.branch = Some(value),
                _ => text_parts.push(token), // not a known filter, keep as search text
            }
        } else {
            text_parts.push(token);
        }
    }

    q.text = text_parts.join(" ");
    q
}

/// Extract plain search terms from an FTS5 query for highlighting.
pub fn query_terms(query: &str) -> Vec<String> {
    // Strip FTS5 operators and quotes, extract words
    let cleaned = query.replace(['"', '\''], " ");
    let cleaned = Regex::new(r"(?i)\b(AND|OR|NOT|NEAR)\b")
        .expect("valid regex")
        .replace_all(&cleaned, " ");
    let mut terms = Vec::new();
    for t in cleaned.split_whitespace() {
        let t = t.trim_matches(|c: char| "*(){}[]".contains(c));
        if t.len() > 1 {
            terms.push(t.to_lowercase());
        }
    }
    terms
}

/// Check if a message matches any of the search terms.
pub fn message_matches(msg: &Message, terms: &[String]) -> bool {
    let text_lower = msg.text.to_lowercase();
    terms.iter().any(|t| text_lower.contains(t.as_str()))
}

/// Locate the JSONL file for a session, checking the appropriate app directories.
pub fn session_jsonl_path(session_id: &str, slug: &str, source: &str) -> Option<(App, std::path::PathBuf)> {
    let app = App::parse(source);

    // Try Claude Code paths
    if app.is_none() || app == Some(App::ClaudeCode) {
        let projects = config::projects_dir();
        if !slug.is_empty() {
            let candidate = projects.join(slug).join(format!("{session_id}.jsonl"));
            if candidate.exists() {
                return Some((App::ClaudeCode, candidate));
            }
        }
        if let Ok(entries) = std::fs::read_dir(&projects) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let candidate = entry.path().join(format!("{session_id}.jsonl"));
                    if candidate.exists() {
                        return Some((App::ClaudeCode, candidate));
                    }
                }
            }
        }
    }

    // Try Codex paths — Codex filenames contain the session ID as a suffix
    if app.is_none() || app == Some(App::Codex) {
        for dir in App::Codex.sessions_dirs() {
            if let Some(path) = find_codex_session_file(&dir, session_id) {
                return Some((App::Codex, path));
            }
        }
    }

    None
}

/// Search recursively for a Codex session file whose name contains the session_id.
fn find_codex_session_file(dir: &Path, session_id: &str) -> Option<std::path::PathBuf> {
    if !dir.exists() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_codex_session_file(&path, session_id) {
                return Some(found);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let name = path.file_name()?.to_string_lossy();
            if name.contains(session_id) {
                return Some(path);
            }
        }
    }
    None
}

/// Run search: query the DB, format and display results.
#[allow(clippy::too_many_arguments)]
pub fn run_search(
    query: &str,
    db_path: &Path,
    file_pat: Option<&str>,
    branch_pat: Option<&str>,
    project_pat: Option<&str>,
    source_filter: Option<&str>,
    limit: i64,
    context_before: usize,
    context_after: usize,
    color: bool,
) -> Result<bool> {
    if !db_path.exists() {
        eprintln!("Index not found. Run `trs index` first.");
        return Ok(false);
    }

    let conn = db::open_db(db_path, false)?;
    let rows = match db::search(&conn, query, file_pat, branch_pat, project_pat, source_filter, limit) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Query error: {e}");
            eprintln!("Hint: wrap multi-word queries in quotes, e.g. trs '\"my phrase\"'");
            return Ok(false);
        }
    };

    if rows.is_empty() {
        eprintln!("No results.");
        return Ok(false);
    }

    let terms = query_terms(query);
    eprintln!("{} result(s)\n", rows.len());

    let mut writer = std::io::stdout();
    for row in &rows {
        display_result(
            &mut writer,
            row,
            &terms,
            context_before,
            context_after,
            color,
        )?;
    }

    Ok(true)
}

/// Display a single search result with optional conversation context.
fn display_result(
    w: &mut dyn std::io::Write,
    row: &SearchResult,
    terms: &[String],
    context_before: usize,
    context_after: usize,
    color: bool,
) -> Result<()> {
    output::print_session_header(w, row, color)?;

    // Try to load and display matching messages with context
    if let Some((app, jsonl_path)) = session_jsonl_path(&row.session_id, &row.slug, &row.source) {
        if !terms.is_empty() {
            if let Ok(messages) = indexer::extract_messages_for(&jsonl_path, &app) {
                if !messages.is_empty() {
                    output::display_conversation(
                        w,
                        &messages,
                        terms,
                        context_before,
                        context_after,
                        color,
                    )?;
                    output::print_session_footer(w, row, color)?;
                    writeln!(w)?;
                    return Ok(());
                }
            }
        }
    }

    // Fallback: show first_message/summary
    let desc = if !row.summary.is_empty() {
        &row.summary
    } else {
        &row.first_message
    };
    if !desc.is_empty() {
        let desc = desc.replace('\n', " ");
        let desc = desc.trim();
        let desc = if desc.len() > 200 {
            format!("{}...", &desc[..197])
        } else {
            desc.to_string()
        };
        if color && !terms.is_empty() {
            write!(w, "     ")?;
            output::write_highlighted(w, &desc, terms)?;
            writeln!(w)?;
        } else {
            writeln!(w, "     {desc}")?;
        }
    }

    output::print_session_footer(w, row, color)?;
    writeln!(w)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_terms() {
        let terms = query_terms("LaunchDarkly migration");
        assert_eq!(terms, vec!["launchdarkly", "migration"]);
    }

    #[test]
    fn test_query_terms_strips_operators() {
        let terms = query_terms("rust AND wasm NOT js");
        assert_eq!(terms, vec!["rust", "wasm", "js"]);
    }

    #[test]
    fn test_query_terms_strips_quotes() {
        let terms = query_terms("\"exact phrase\"");
        assert_eq!(terms, vec!["exact", "phrase"]);
    }

    #[test]
    fn test_query_terms_strips_wildcards() {
        let terms = query_terms("migrat*");
        assert_eq!(terms, vec!["migrat"]);
    }

    #[test]
    fn test_parse_query_plain() {
        let q = parse_query("hello world");
        assert_eq!(q.text, "hello world");
        assert!(q.app.is_none());
        assert!(q.project.is_none());
    }

    #[test]
    fn test_parse_query_app_filter() {
        let q = parse_query("app:codex bug fix");
        assert_eq!(q.text, "bug fix");
        assert_eq!(q.app.as_deref(), Some("codex"));
    }

    #[test]
    fn test_parse_query_multiple_filters() {
        let q = parse_query("p:gamma b:main terraform");
        assert_eq!(q.text, "terraform");
        assert_eq!(q.project.as_deref(), Some("gamma"));
        assert_eq!(q.branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_parse_query_quoted_value() {
        let q = parse_query("project:\"my app\" search terms");
        assert_eq!(q.project.as_deref(), Some("my app"));
        assert_eq!(q.text, "search terms");
    }

    #[test]
    fn test_parse_query_filter_only() {
        let q = parse_query("app:claude");
        assert_eq!(q.text, "");
        assert_eq!(q.app.as_deref(), Some("claude"));
    }

    #[test]
    fn test_parse_query_unknown_prefix_kept() {
        let q = parse_query("foo:bar baz");
        assert_eq!(q.text, "foo:bar baz");
    }

    #[test]
    fn test_parse_query_file_filter() {
        let q = parse_query("f:*.rs error handling");
        assert_eq!(q.file.as_deref(), Some("*.rs"));
        assert_eq!(q.text, "error handling");
    }

    #[test]
    fn test_parse_query_source_filter() {
        let q = parse_query("a:cc hello");
        assert_eq!(q.source_filter(), Some("claude-code"));

        let q = parse_query("app:codex hello");
        assert_eq!(q.source_filter(), Some("codex"));

        let q = parse_query("hello");
        assert_eq!(q.source_filter(), None);
    }

    #[test]
    fn test_message_matches() {
        use crate::session::{Message, MessageRole};
        let msg = Message {
            index: 0,
            role: MessageRole::User,
            text: "help me with LaunchDarkly migration".into(),
            teammate_id: String::new(),
            teammate_summary: String::new(),
            teammate_color: String::new(),
        };
        let terms = vec!["launchdarkly".to_string()];
        assert!(message_matches(&msg, &terms));

        let terms = vec!["nonexistent".to_string()];
        assert!(!message_matches(&msg, &terms));
    }
}
