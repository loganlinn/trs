//! Shared display model consumed by both CLI (output.rs) and TUI (tui/ui.rs).
//!
//! All formatting decisions (truncation, snippeting, header hierarchy, marker
//! choice) happen in `prepare_result()`. Renderers handle only styling.

use chrono::Datelike;

use crate::search;
use crate::session::{Message, MessageRole, SearchResult};

/// Role marker characters matching the TUI convention.
pub fn role_marker(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::User => "\u{276f}",     // ❯
        MessageRole::Assistant => "\u{25cf}", // ●
        MessageRole::Summary => "\u{25c6}",  // ◆
        MessageRole::Teammate => "\u{25cf}",  // ●
    }
}

/// Human-readable role label.
pub fn role_label(msg: &Message) -> String {
    match msg.role {
        MessageRole::User => "user".into(),
        MessageRole::Assistant => "claude".into(),
        MessageRole::Summary => "summary".into(),
        MessageRole::Teammate => {
            if msg.teammate_id.is_empty() {
                "claude[teammate]".into()
            } else {
                format!("claude[{}]", msg.teammate_id)
            }
        }
    }
}

/// Extract project slug from cwd path (last component).
pub fn project_slug(cwd: &str) -> &str {
    cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or(cwd)
}

/// Intermediate representation of a search result for display.
#[derive(Debug, Clone)]
pub struct ResultDisplay {
    pub project_name: String,
    pub full_path: String,
    pub branches: Vec<String>,
    pub date: String,
    pub duration: String,
    pub message_count: i64,
    pub session_id: String,
    pub source: String,
    pub first_message: String,
    pub summary: String,
    pub custom_title: Option<String>,
    pub snippets: Vec<MessageSnippet>,
}

/// A single message snippet for display.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MessageSnippet {
    pub index: usize,
    pub role: MessageRole,
    pub marker: &'static str,
    pub label: String,
    pub is_match: bool,
    pub lines: Vec<SnippetLine>,
    pub teammate_summary: String,
}

/// A single line within a snippet.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SnippetLine {
    pub text: String,
    pub is_gap: bool,
    pub highlights: Vec<(usize, usize)>,
}

const MAX_MATCH_LINES: usize = 4;
const CONTEXT_CHAR_LIMIT: usize = 100;

/// Format duration between two ISO timestamps.
pub fn format_duration(start: &str, end: &str) -> String {
    if start.is_empty() || end.is_empty() {
        return String::new();
    }
    let parse = |s: &str| -> Option<chrono::NaiveDateTime> {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ").ok()
    };
    let (t0, t1) = match (parse(start), parse(end)) {
        (Some(a), Some(b)) => (a, b),
        _ => return String::new(),
    };
    let secs = (t1 - t0).num_seconds();
    if secs < 0 {
        return String::new();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    format!("{}h {}m", mins / 60, mins % 60)
}

/// Extract short date (YYYY-MM-DD) from ISO timestamp.
pub fn short_date(ts: &str) -> &str {
    if ts.len() >= 10 {
        &ts[..10]
    } else {
        ""
    }
}

/// Compact relative date for table display.
///
/// Returns: "today", "1d"–"6d", "1w"–"3w", "Mon D", or "YYYY-MM".
pub fn relative_date(ts: &str) -> String {
    let parse = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ").ok();
    let dt = match parse(ts) {
        Some(d) => d.date(),
        None => return short_date(ts).to_string(),
    };
    let today = chrono::Local::now().date_naive();
    let days = (today - dt).num_days();
    if days < 0 {
        return short_date(ts).to_string();
    }
    match days {
        0 => "today".into(),
        1..=6 => format!("{days}d"),
        7..=27 => format!("{}w", days / 7),
        _ => {
            if dt.year() == today.year() {
                dt.format("%b %-d").to_string()
            } else {
                dt.format("%Y-%m").to_string()
            }
        }
    }
}

/// Build a `ResultDisplay` from a search result and its messages.
pub fn prepare_result(
    result: &SearchResult,
    messages: &[Message],
    terms: &[String],
    context_before: usize,
    context_after: usize,
) -> ResultDisplay {
    let branches: Vec<String> =
        serde_json::from_str(&result.git_branches).unwrap_or_default();
    let project_name = if !result.cwd.is_empty() {
        project_slug(&result.cwd).to_string()
    } else {
        result.session_id.clone()
    };

    let snippets = build_snippets(messages, terms, context_before, context_after);

    ResultDisplay {
        project_name,
        full_path: result.cwd.clone(),
        branches,
        date: short_date(&result.start_time).to_string(),
        duration: format_duration(&result.start_time, &result.end_time),
        message_count: result.message_count,
        session_id: result.session_id.clone(),
        source: result.source.clone(),
        first_message: result.first_message.clone(),
        summary: result.summary.clone(),
        custom_title: result.custom_title.clone(),
        snippets,
    }
}

fn build_snippets(
    messages: &[Message],
    terms: &[String],
    before: usize,
    after: usize,
) -> Vec<MessageSnippet> {
    use std::collections::HashSet;

    let match_indices: HashSet<usize> = messages
        .iter()
        .filter(|m| search::message_matches(m, terms))
        .map(|m| m.index)
        .collect();

    if match_indices.is_empty() {
        // Show first message as fallback
        if let Some(msg) = messages.first() {
            return vec![build_snippet(msg, false, terms)];
        }
        return vec![];
    }

    let valid: HashSet<usize> = messages.iter().map(|m| m.index).collect();
    let mut show_indices: HashSet<usize> = HashSet::new();
    for &mi in &match_indices {
        let start = mi.saturating_sub(before);
        for idx in start..=mi + after {
            if valid.contains(&idx) {
                show_indices.insert(idx);
            }
        }
    }

    let mut sorted: Vec<usize> = show_indices.into_iter().collect();
    sorted.sort();

    sorted
        .iter()
        .filter_map(|&idx| {
            let msg = messages.iter().find(|m| m.index == idx)?;
            let is_match = match_indices.contains(&idx);
            Some(build_snippet(msg, is_match, terms))
        })
        .collect()
}

fn build_snippet(msg: &Message, is_match: bool, terms: &[String]) -> MessageSnippet {
    let text = msg.text.trim();
    let lines = if is_match {
        build_match_lines(text, terms)
    } else {
        build_context_lines(text, msg)
    };

    MessageSnippet {
        index: msg.index,
        role: msg.role.clone(),
        marker: role_marker(&msg.role),
        label: role_label(msg),
        is_match,
        lines,
        teammate_summary: msg.teammate_summary.clone(),
    }
}

fn build_match_lines(text: &str, terms: &[String]) -> Vec<SnippetLine> {
    let text_lines: Vec<&str> = text.lines().collect();
    if text_lines.len() <= MAX_MATCH_LINES {
        return text_lines
            .iter()
            .map(|l| SnippetLine {
                text: l.to_string(),
                is_gap: false,
                highlights: find_highlights(l, terms),
            })
            .collect();
    }

    // Find lines containing matches and build windows
    let match_line_indices: Vec<usize> = text_lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let lower = l.to_lowercase();
            terms.iter().any(|t| lower.contains(t.as_str()))
        })
        .map(|(i, _)| i)
        .collect();

    if match_line_indices.is_empty() {
        return text_lines[..MAX_MATCH_LINES]
            .iter()
            .map(|l| SnippetLine {
                text: l.to_string(),
                is_gap: false,
                highlights: find_highlights(l, terms),
            })
            .collect();
    }

    let mut include = Vec::new();
    for &mi in &match_line_indices {
        let start = mi.saturating_sub(1);
        let end = (mi + 1).min(text_lines.len() - 1);
        for idx in start..=end {
            include.push(idx);
        }
    }
    include.sort();
    include.dedup();
    if include.len() > MAX_MATCH_LINES * 2 {
        include.truncate(MAX_MATCH_LINES * 2);
    }

    let mut result = Vec::new();
    let mut prev: Option<usize> = None;
    for idx in &include {
        if prev.is_some_and(|p| *idx > p + 1) {
            result.push(SnippetLine {
                text: "...".into(),
                is_gap: true,
                highlights: vec![],
            });
        }
        result.push(SnippetLine {
            text: text_lines[*idx].to_string(),
            is_gap: false,
            highlights: find_highlights(text_lines[*idx], terms),
        });
        prev = Some(*idx);
    }

    // Trailing indicator
    if include.last().is_some_and(|&last| last < text_lines.len() - 1) {
        let remaining = text_lines.len() - include.last().unwrap() - 1;
        result.push(SnippetLine {
            text: format!("... (+{remaining} lines)"),
            is_gap: true,
            highlights: vec![],
        });
    }

    result
}

fn build_context_lines(text: &str, msg: &Message) -> Vec<SnippetLine> {
    // Teammates with summary: use that instead
    if msg.role == MessageRole::Teammate && !msg.teammate_summary.is_empty() {
        return vec![SnippetLine {
            text: msg.teammate_summary.clone(),
            is_gap: false,
            highlights: vec![],
        }];
    }

    let first_line = text.lines().next().unwrap_or("");
    let truncated = if first_line.len() > CONTEXT_CHAR_LIMIT {
        format!("{}...", &first_line[..CONTEXT_CHAR_LIMIT - 3])
    } else {
        first_line.to_string()
    };
    vec![SnippetLine {
        text: truncated,
        is_gap: false,
        highlights: vec![],
    }]
}

fn find_highlights(line: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if terms.is_empty() {
        return vec![];
    }
    let escaped: Vec<String> = terms.iter().map(|t| regex::escape(t)).collect();
    let Ok(pattern) = regex::RegexBuilder::new(&escaped.join("|"))
        .case_insensitive(true)
        .build()
    else {
        return vec![];
    };
    pattern
        .find_iter(line)
        .map(|m| (m.start(), m.end()))
        .collect()
}

/// A group of results sharing the same project.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResultGroup {
    pub project_name: String,
    pub full_path: String,
    pub branches: Vec<String>,
    pub results: Vec<ResultDisplay>,
}

/// Group results by project (cwd slug or custom_title fallback to project_name).
pub fn group_results(results: Vec<ResultDisplay>) -> Vec<ResultGroup> {
    use std::collections::BTreeMap;

    // Group by full_path first (more specific), fall back to project_name
    let mut groups: BTreeMap<String, Vec<ResultDisplay>> = BTreeMap::new();
    for r in results {
        let key = if !r.full_path.is_empty() {
            r.full_path.clone()
        } else {
            r.project_name.clone()
        };
        groups.entry(key).or_default().push(r);
    }

    groups
        .into_values()
        .map(|results| {
            let first = &results[0];
            ResultGroup {
                project_name: first.project_name.clone(),
                full_path: first.full_path.clone(),
                branches: first.branches.clone(),
                results,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_slug() {
        assert_eq!(project_slug("/Users/logan/src/trs"), "trs");
        assert_eq!(project_slug("/tmp"), "tmp");
        assert_eq!(project_slug(""), "");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(
            format_duration("2024-01-01T00:00:00.000Z", "2024-01-01T00:00:30.000Z"),
            "30s"
        );
        assert_eq!(
            format_duration("2024-01-01T00:00:00.000Z", "2024-01-01T00:05:00.000Z"),
            "5m"
        );
        assert_eq!(format_duration("", ""), "");
    }

    #[test]
    fn test_short_date() {
        assert_eq!(short_date("2024-01-15T00:00:00.000Z"), "2024-01-15");
        assert_eq!(short_date(""), "");
    }

    #[test]
    fn test_find_highlights() {
        let hl = find_highlights("hello world", &["world".into()]);
        assert_eq!(hl, vec![(6, 11)]);
    }

    #[test]
    fn test_build_context_lines_truncation() {
        let long_text = "a".repeat(200);
        let msg = Message {
            index: 0,
            role: MessageRole::User,
            text: long_text,
            teammate_id: String::new(),
            teammate_summary: String::new(),
            teammate_color: String::new(),
        };
        let lines = build_context_lines(&msg.text, &msg);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.ends_with("..."));
        assert!(lines[0].text.len() <= CONTEXT_CHAR_LIMIT);
    }

    #[test]
    fn test_build_match_lines_short() {
        let lines = build_match_lines("line one\nline two", &["one".into()]);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].is_gap);
    }

    #[test]
    fn test_build_match_lines_long_with_snippet() {
        let text: String = (0..20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let lines = build_match_lines(&text, &["line 10".into()]);
        // Should include windowed lines, not all 20
        assert!(lines.len() < 20);
        assert!(lines.iter().any(|l| l.text.contains("line 10")));
    }

    #[test]
    fn test_group_results() {
        let r1 = ResultDisplay {
            project_name: "trs".into(),
            full_path: "/home/user/trs".into(),
            branches: vec![],
            date: String::new(),
            duration: String::new(),
            message_count: 10,
            session_id: "s1".into(),
            source: "claude-code".into(),
            first_message: String::new(),
            summary: String::new(),
            custom_title: None,
            snippets: vec![],
        };
        let r2 = ResultDisplay {
            session_id: "s2".into(),
            ..r1.clone()
        };
        let r3 = ResultDisplay {
            project_name: "other".into(),
            full_path: "/home/user/other".into(),
            session_id: "s3".into(),
            ..r1.clone()
        };
        let groups = group_results(vec![r1, r2, r3]);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn test_role_marker() {
        assert_eq!(role_marker(&MessageRole::User), "❯");
        assert_eq!(role_marker(&MessageRole::Assistant), "●");
        assert_eq!(role_marker(&MessageRole::Summary), "◆");
    }
}
