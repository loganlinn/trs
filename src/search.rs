use anyhow::Result;
use regex::Regex;
use std::path::Path;

use crate::config;
use crate::db;
use crate::indexer;
use crate::output;
use crate::session::{Message, SearchResult};

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

/// Locate the JSONL file for a session.
pub fn session_jsonl_path(session_id: &str, slug: &str) -> Option<std::path::PathBuf> {
    let projects = config::projects_dir();
    if !slug.is_empty() {
        let candidate = projects.join(slug).join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // Fallback: search all project dirs
    if let Ok(entries) = std::fs::read_dir(&projects) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let candidate = entry.path().join(format!("{session_id}.jsonl"));
                if candidate.exists() {
                    return Some(candidate);
                }
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
    let rows = match db::search(&conn, query, file_pat, branch_pat, project_pat, limit) {
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
    if let Some(jsonl_path) = session_jsonl_path(&row.session_id, &row.slug) {
        if !terms.is_empty() {
            if let Ok(messages) = indexer::extract_messages(&jsonl_path) {
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
