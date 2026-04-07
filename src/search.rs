use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

use crate::config;
use crate::db;
use crate::indexer;
use crate::output;
use crate::session::{App, Message};

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        .expect("valid regex")
});

/// Check if a string looks like a UUID (8-4-4-4-12 hex digits with hyphens).
pub fn is_uuid(s: &str) -> bool {
    UUID_RE.is_match(s.trim())
}

/// Resolve a project filter value: if it looks like a path (starts with `.`,
/// `/`, or `~`), canonicalize it to an absolute path. Plain names are
/// returned as-is for substring matching.
///
/// Trailing `/*` or `*` enables prefix matching (children included).
/// Without a wildcard, paths match exactly and plain names use substring match.
pub fn resolve_project_filter(value: &str) -> String {
    let (value, wildcard) = if let Some(v) = value.strip_suffix("/*") {
        (v, true)
    } else if let Some(v) = value.strip_suffix('*') {
        (v, true)
    } else {
        (value, false)
    };

    let resolved = if value.starts_with('.') || value.starts_with('/') || value.starts_with('~') {
        let expanded = if let Some(rest) = value.strip_prefix('~') {
            let home = directories::BaseDirs::new()
                .map(|d| d.home_dir().to_path_buf())
                .unwrap_or_else(|| Path::new("~").to_path_buf());
            home.join(rest.strip_prefix('/').unwrap_or(rest))
        } else {
            Path::new(value).to_path_buf()
        };
        // canonicalize resolves `.`, `..`, and symlinks; fall back to
        // the expanded (but not canonicalized) path so ~ and . still work
        // even when the directory doesn't exist
        match expanded.canonicalize() {
            Ok(abs) => abs.to_string_lossy().into_owned(),
            Err(_) => expanded.to_string_lossy().into_owned(),
        }
    } else {
        value.to_string()
    };

    if wildcard {
        format!("{resolved}*")
    } else {
        resolved
    }
}

/// Comparison operator for date filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOp {
    Gt,
    Gte,
    Eq,
    Lte,
    Lt,
}

/// A parsed date filter with operator and resolved ISO date string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateFilter {
    pub op: DateOp,
    pub date: String,
}

impl DateFilter {
    /// SQL operator string.
    pub fn sql_op(&self) -> &'static str {
        match self.op {
            DateOp::Gt => ">",
            DateOp::Gte => ">=",
            DateOp::Eq => "LIKE",
            DateOp::Lte => "<=",
            DateOp::Lt => "<",
        }
    }

    /// SQL value — for `=` we use prefix match (LIKE 'date%'), otherwise raw.
    pub fn sql_value(&self) -> String {
        match self.op {
            DateOp::Eq => format!("{}%", self.date),
            _ => self.date.clone(),
        }
    }
}

/// Parse operator prefix from a date filter value, returning (op, rest).
fn parse_date_op(value: &str) -> (DateOp, &str) {
    if let Some(rest) = value.strip_prefix(">=") {
        (DateOp::Gte, rest)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (DateOp::Lte, rest)
    } else if let Some(rest) = value.strip_prefix('>') {
        (DateOp::Gt, rest)
    } else if let Some(rest) = value.strip_prefix('<') {
        (DateOp::Lt, rest)
    } else if let Some(rest) = value.strip_prefix('=') {
        (DateOp::Eq, rest)
    } else {
        (DateOp::Eq, value)
    }
}

/// Resolve relative date shorthands to YYYY-MM-DD strings.
/// Supports: `today`, `yesterday`, `Nd` (e.g. `7d` = 7 days ago).
pub fn resolve_date_value(value: &str) -> Option<String> {
    let today = chrono::Local::now().date_naive();
    match value {
        "today" => Some(today.format("%Y-%m-%d").to_string()),
        "yesterday" => Some(
            (today - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string(),
        ),
        s if s.ends_with('d') => {
            let n: i64 = s.strip_suffix('d')?.parse().ok()?;
            Some(
                (today - chrono::Duration::days(n))
                    .format("%Y-%m-%d")
                    .to_string(),
            )
        }
        // Already a date-like string (YYYY-MM-DD or YYYY-MM or YYYY)
        s if s.len() >= 4 && s.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            Some(s.to_string())
        }
        _ => None,
    }
}

/// Parse a `date:` filter value into a DateFilter.
pub fn parse_date_filter(raw: &str) -> Option<DateFilter> {
    let (op, rest) = parse_date_op(raw);
    let date = resolve_date_value(rest)?;
    Some(DateFilter { op, date })
}

/// Parsed search input with extracted filter prefixes.
#[derive(Debug, Default, Clone)]
pub struct ParsedQuery {
    pub text: String,
    pub app: Option<String>,
    pub project: Option<String>,
    pub file: Option<String>,
    pub branch: Option<String>,
    pub date: Option<DateFilter>,
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
                "project" | "p" => q.project = Some(resolve_project_filter(&value)),
                "file" | "f" => q.file = Some(value),
                "branch" | "b" => q.branch = Some(value),
                "date" | "d" => q.date = parse_date_filter(&value),
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
pub fn run_search(
    query: &str,
    db_path: &Path,
    filter: &db::SearchFilter,
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
    let rows = match db::search(&conn, query, filter, limit) {
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

    // Build display models
    let displays: Vec<crate::display::ResultDisplay> = rows
        .iter()
        .map(|row| {
            let messages = session_jsonl_path(&row.session_id, &row.slug, &row.source)
                .and_then(|(app, path)| indexer::extract_messages_for(&path, &app).ok())
                .unwrap_or_default();
            crate::display::prepare_result(row, &messages, &terms, context_before, context_after)
        })
        .collect();

    let groups = crate::display::group_results(displays);

    let mut writer = std::io::stdout();
    for group in &groups {
        output::print_group(&mut writer, group, &terms, color)?;
    }

    Ok(true)
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
    fn test_resolve_project_filter_dot() {
        let resolved = resolve_project_filter(".");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolved, cwd.to_string_lossy());
    }

    #[test]
    fn test_resolve_project_filter_plain_name() {
        assert_eq!(resolve_project_filter("myproject"), "myproject");
    }

    #[test]
    fn test_resolve_project_filter_absolute() {
        let expected = std::fs::canonicalize("/tmp")
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolve_project_filter("/tmp"), expected);
    }

    #[test]
    fn test_resolve_project_filter_tilde() {
        let resolved = resolve_project_filter("~/.dotfiles");
        let home = directories::BaseDirs::new().unwrap();
        let expected = home.home_dir().join(".dotfiles");
        assert_eq!(resolved, expected.to_string_lossy());
    }

    #[test]
    fn test_resolve_project_filter_nonexistent() {
        // falls back to raw value when canonicalize fails
        assert_eq!(
            resolve_project_filter("./nonexistent-dir-xyz"),
            "./nonexistent-dir-xyz"
        );
    }

    #[test]
    fn test_resolve_project_filter_wildcard() {
        let canonical_tmp = std::fs::canonicalize("/tmp")
            .unwrap()
            .to_string_lossy()
            .into_owned();

        // trailing /* preserves wildcard marker
        let resolved = resolve_project_filter("/tmp/*");
        assert_eq!(resolved, format!("{canonical_tmp}*"));

        // trailing * also works
        let resolved = resolve_project_filter("/tmp*");
        assert_eq!(resolved, format!("{canonical_tmp}*"));

        // plain name with wildcard
        assert_eq!(resolve_project_filter("gamma*"), "gamma*");
    }

    #[test]
    fn test_resolve_project_filter_no_wildcard() {
        // exact path without wildcard has no trailing *
        let resolved = resolve_project_filter("/tmp");
        assert!(!resolved.ends_with('*'));
    }

    #[test]
    fn test_parse_query_resolves_project_dot() {
        let q = parse_query("p:. search terms");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(q.project.as_deref(), Some(cwd.to_str().unwrap()));
        assert_eq!(q.text, "search terms");
    }

    #[test]
    fn test_parse_query_resolves_project_tilde() {
        let q = parse_query("project:~/.dotfiles");
        let home = directories::BaseDirs::new().unwrap();
        let expected = home.home_dir().join(".dotfiles").to_string_lossy().to_string();
        assert_eq!(q.project.as_deref(), Some(expected.as_str()));
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

    #[test]
    fn test_parse_date_filter_operators() {
        let f = parse_date_filter(">2025-03-01").unwrap();
        assert_eq!(f.op, DateOp::Gt);
        assert_eq!(f.date, "2025-03-01");

        let f = parse_date_filter(">=2025-03-01").unwrap();
        assert_eq!(f.op, DateOp::Gte);

        let f = parse_date_filter("<=2025-03-01").unwrap();
        assert_eq!(f.op, DateOp::Lte);

        let f = parse_date_filter("<2025-03-01").unwrap();
        assert_eq!(f.op, DateOp::Lt);

        let f = parse_date_filter("=2025-03-01").unwrap();
        assert_eq!(f.op, DateOp::Eq);

        // bare date defaults to Eq
        let f = parse_date_filter("2025-03-01").unwrap();
        assert_eq!(f.op, DateOp::Eq);
        assert_eq!(f.date, "2025-03-01");
    }

    #[test]
    fn test_parse_date_filter_shorthands() {
        let today = chrono::Local::now().date_naive();

        let f = parse_date_filter("today").unwrap();
        assert_eq!(f.date, today.format("%Y-%m-%d").to_string());

        let f = parse_date_filter("yesterday").unwrap();
        assert_eq!(
            f.date,
            (today - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string()
        );

        let f = parse_date_filter(">=7d").unwrap();
        assert_eq!(f.op, DateOp::Gte);
        assert_eq!(
            f.date,
            (today - chrono::Duration::days(7))
                .format("%Y-%m-%d")
                .to_string()
        );
    }

    #[test]
    fn test_parse_date_filter_sql() {
        let f = parse_date_filter(">2025-03-01").unwrap();
        assert_eq!(f.sql_op(), ">");
        assert_eq!(f.sql_value(), "2025-03-01");

        let f = parse_date_filter("2025-03-01").unwrap();
        assert_eq!(f.sql_op(), "LIKE");
        assert_eq!(f.sql_value(), "2025-03-01%");
    }

    #[test]
    fn test_parse_date_filter_invalid() {
        assert!(parse_date_filter("garbage").is_none());
        assert!(parse_date_filter(">notadate").is_none());
    }

    #[test]
    fn test_parse_query_date_filter() {
        let q = parse_query("date:>2025-03-01 terraform");
        assert_eq!(q.text, "terraform");
        let df = q.date.unwrap();
        assert_eq!(df.op, DateOp::Gt);
        assert_eq!(df.date, "2025-03-01");

        // d: alias
        let q = parse_query("d:>=7d search");
        assert!(q.date.is_some());
        assert_eq!(q.date.unwrap().op, DateOp::Gte);
    }

    #[test]
    fn test_is_uuid() {
        assert!(is_uuid("01020304-0506-0708-090a-0b0c0d0e0f10"));
        assert!(is_uuid("ABCDEF01-2345-6789-abcd-ef0123456789"));
        assert!(is_uuid("  01020304-0506-0708-090a-0b0c0d0e0f10  ")); // trimmed

        assert!(!is_uuid(""));
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("01020304050607080090a0b0c0d0e0f10")); // no hyphens
        assert!(!is_uuid("01020304-0506-0708-090a-0b0c0d0e0f1")); // too short
        assert!(!is_uuid("01020304-0506-0708-090a-0b0c0d0e0f100")); // too long
        assert!(!is_uuid("g1020304-0506-0708-090a-0b0c0d0e0f10")); // non-hex
        assert!(!is_uuid("LaunchDarkly migration")); // normal search
    }

    #[test]
    fn test_parse_date_filter_partial_date() {
        // year-month prefix for matching all days in a month
        let f = parse_date_filter("2025-03").unwrap();
        assert_eq!(f.op, DateOp::Eq);
        assert_eq!(f.date, "2025-03");
        assert_eq!(f.sql_value(), "2025-03%");
    }
}
