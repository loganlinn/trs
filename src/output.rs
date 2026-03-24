use std::collections::HashSet;
use std::io::Write;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use regex::Regex;

use crate::search;
use crate::session::{App, Message, MessageRole, SearchResult};

const GROUP_SEP: &str = "--";

/// Color mapping for message roles.
fn role_color(role: &MessageRole) -> Color {
    match role {
        MessageRole::User => Color::Green,
        MessageRole::Assistant => Color::Blue,
        MessageRole::Summary => Color::Yellow,
        MessageRole::Teammate => Color::Cyan,
    }
}

/// Format duration between two ISO timestamps.
fn duration(start: &str, end: &str) -> String {
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
fn short_date(ts: &str) -> &str {
    if ts.len() >= 10 {
        &ts[..10]
    } else {
        ""
    }
}

/// Print session header (ripgrep-style filename line).
pub fn print_session_header(
    w: &mut dyn Write,
    row: &SearchResult,
    color: bool,
) -> std::io::Result<()> {
    let branches: Vec<String> = serde_json::from_str(&row.git_branches).unwrap_or_default();
    let date = short_date(&row.start_time);
    let dur = duration(&row.start_time, &row.end_time);

    // Primary: cwd or session_id
    let primary = if !row.cwd.is_empty() {
        &row.cwd
    } else {
        &row.session_id
    };

    if color {
        write!(
            w,
            "{}{}{}{}",
            SetForegroundColor(Color::Magenta),
            SetAttribute(Attribute::Bold),
            primary,
            SetAttribute(Attribute::Reset),
        )?;
    } else {
        write!(w, "{primary}")?;
    }

    // Session name (from --name or /rename)
    if let Some(title) = &row.custom_title {
        if color {
            write!(
                w,
                "  {}{}({title}){}",
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(w, "  ({title})")?;
        }
    }

    // Metadata
    let mut meta_parts = Vec::new();
    if !branches.is_empty() {
        meta_parts.push(format!("@ {}", branches.join(", ")));
    }
    if !date.is_empty() {
        meta_parts.push(date.to_string());
    }
    if !dur.is_empty() {
        meta_parts.push(dur);
    }
    if row.message_count > 0 {
        meta_parts.push(format!("{} msgs", row.message_count));
    }

    if !meta_parts.is_empty() {
        if color {
            write!(
                w,
                "  {}{}{}",
                SetAttribute(Attribute::Dim),
                meta_parts.join("  "),
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(w, "  {}", meta_parts.join("  "))?;
        }
    }
    writeln!(w)?;
    write!(w, "{}", ResetColor)?;
    Ok(())
}

/// Print session footer (resume command).
pub fn print_session_footer(
    w: &mut dyn Write,
    row: &SearchResult,
    color: bool,
) -> std::io::Result<()> {
    let cmd = match App::parse(&row.source) {
        Some(app) => app.resume_cmd(&row.session_id),
        None => format!("{}  {}", row.source, row.session_id),
    };
    if color {
        writeln!(
            w,
            "{}{}{}",
            SetAttribute(Attribute::Dim),
            cmd,
            SetAttribute(Attribute::Reset),
        )
    } else {
        writeln!(w, "{cmd}")
    }
}

/// Write text with matching terms highlighted in bold red.
pub fn write_highlighted(w: &mut dyn Write, text: &str, terms: &[String]) -> std::io::Result<()> {
    if terms.is_empty() {
        return write!(w, "{text}");
    }
    let escaped: Vec<String> = terms.iter().map(|t| regex::escape(t)).collect();
    let pattern = Regex::new(&escaped.join("|"))
        .unwrap_or_else(|_| Regex::new("$^").expect("valid fallback"));

    let mut last = 0;
    for m in pattern.find_iter(text) {
        write!(w, "{}", &text[last..m.start()])?;
        write!(
            w,
            "{}{}{}{}",
            SetForegroundColor(Color::Red),
            SetAttribute(Attribute::Bold),
            m.as_str(),
            SetAttribute(Attribute::Reset),
        )?;
        last = m.end();
    }
    write!(w, "{}", &text[last..])?;
    Ok(())
}

/// Display matching messages with context (ripgrep-style).
pub fn display_conversation(
    w: &mut dyn Write,
    messages: &[Message],
    terms: &[String],
    before: usize,
    after: usize,
    color: bool,
) -> std::io::Result<()> {
    // Find matching message indices
    let match_indices: HashSet<usize> = messages
        .iter()
        .filter(|m| search::message_matches(m, terms))
        .map(|m| m.index)
        .collect();

    if match_indices.is_empty() {
        // Show first message as fallback
        if let Some(msg) = messages.first() {
            format_message_line(w, msg, terms, false, color)?;
            writeln!(w)?;
        }
        return Ok(());
    }

    // Build set of indices to show (matches + context)
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

    let mut sorted_show: Vec<usize> = show_indices.into_iter().collect();
    sorted_show.sort();

    let mut prev_idx: Option<usize> = None;
    for &idx in &sorted_show {
        if let Some(prev) = prev_idx {
            if idx > prev + 1 {
                if color {
                    writeln!(
                        w,
                        "{}{}{}",
                        SetAttribute(Attribute::Dim),
                        GROUP_SEP,
                        SetAttribute(Attribute::Reset),
                    )?;
                } else {
                    writeln!(w, "{GROUP_SEP}")?;
                }
            }
        }
        let msg = match messages.iter().find(|m| m.index == idx) {
            Some(m) => m,
            None => continue,
        };
        let is_match = match_indices.contains(&idx);
        format_message_line(w, msg, terms, is_match, color)?;
        writeln!(w)?;
        prev_idx = Some(idx);
    }

    Ok(())
}

const MAX_DISPLAY_LINES: usize = 6;

/// Format a single message line, ripgrep-style.
fn format_message_line(
    w: &mut dyn Write,
    msg: &Message,
    terms: &[String],
    is_match: bool,
    color: bool,
) -> std::io::Result<()> {
    let num_width = 4;
    let sep = if is_match { ":" } else { "-" };

    // Role label
    let role_label = match msg.role {
        MessageRole::Teammate => {
            if msg.teammate_id.is_empty() {
                "claude[teammate]".to_string()
            } else {
                format!("claude[{}]", msg.teammate_id)
            }
        }
        MessageRole::Assistant => "claude".to_string(),
        _ => msg.role.as_str().to_string(),
    };

    let rc = role_color(&msg.role);

    // Line number
    if color {
        if is_match {
            write!(
                w,
                "{}{}",
                SetForegroundColor(Color::Green),
                SetAttribute(Attribute::Dim),
            )?;
        } else {
            write!(w, "{}", SetAttribute(Attribute::Dim))?;
        }
        write!(w, "{:>width$}", msg.index + 1, width = num_width)?;
        write!(w, "{}", SetAttribute(Attribute::Reset))?;
    } else {
        write!(w, "{:>width$}", msg.index + 1, width = num_width)?;
    }

    // Separator
    if color {
        if is_match {
            write!(
                w,
                "{}{sep}{}",
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            write!(
                w,
                "{}{sep}{}",
                SetAttribute(Attribute::Dim),
                SetAttribute(Attribute::Reset)
            )?;
        }
    } else {
        write!(w, "{sep}")?;
    }

    // Role
    if color {
        if is_match {
            write!(
                w,
                "{}{}{}{}",
                SetForegroundColor(rc),
                SetAttribute(Attribute::Bold),
                role_label,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(
                w,
                "{}{}{}{}",
                SetForegroundColor(rc),
                SetAttribute(Attribute::Dim),
                role_label,
                SetAttribute(Attribute::Reset),
            )?;
        }
    } else {
        write!(w, "{role_label}")?;
    }

    // Separator again
    if color {
        if is_match {
            write!(
                w,
                "{}{sep}{}",
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            write!(
                w,
                "{}{sep}{}",
                SetAttribute(Attribute::Dim),
                SetAttribute(Attribute::Reset)
            )?;
        }
    } else {
        write!(w, "{sep}")?;
    }

    // Teammate context: show summary only when not a match
    if msg.role == MessageRole::Teammate && !is_match && !msg.teammate_summary.is_empty() {
        if color {
            write!(
                w,
                " {}{}{}",
                SetAttribute(Attribute::Dim),
                msg.teammate_summary,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(w, " {}", msg.teammate_summary)?;
        }
        return Ok(());
    }

    // Message text (truncated)
    let text = msg.text.trim();
    let text_lines: Vec<&str> = text.lines().collect();
    let display_text = if text_lines.len() > MAX_DISPLAY_LINES {
        let truncated: Vec<&str> = text_lines[..MAX_DISPLAY_LINES].to_vec();
        let remaining = text_lines.len() - MAX_DISPLAY_LINES;
        format!("{}\n  ... (+{remaining} lines)", truncated.join("\n"))
    } else {
        text_lines.join("\n")
    };

    if is_match && !terms.is_empty() && color {
        write!(w, " ")?;
        write_highlighted(w, &display_text, terms)?;
    } else if color && !is_match {
        write!(
            w,
            " {}{}{}",
            SetAttribute(Attribute::Dim),
            display_text,
            SetAttribute(Attribute::Reset),
        )?;
    } else {
        write!(w, " {display_text}")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration_seconds() {
        assert_eq!(
            duration("2024-01-01T00:00:00.000Z", "2024-01-01T00:00:30.000Z"),
            "30s"
        );
    }

    #[test]
    fn test_duration_minutes() {
        assert_eq!(
            duration("2024-01-01T00:00:00.000Z", "2024-01-01T00:05:00.000Z"),
            "5m"
        );
    }

    #[test]
    fn test_duration_hours() {
        assert_eq!(
            duration("2024-01-01T00:00:00.000Z", "2024-01-01T02:30:00.000Z"),
            "2h 30m"
        );
    }

    #[test]
    fn test_duration_empty() {
        assert_eq!(duration("", "2024-01-01T00:00:00.000Z"), "");
        assert_eq!(duration("2024-01-01T00:00:00.000Z", ""), "");
    }

    #[test]
    fn test_short_date() {
        assert_eq!(short_date("2024-01-15T00:00:00.000Z"), "2024-01-15");
        assert_eq!(short_date(""), "");
    }

    #[test]
    fn test_write_highlighted() {
        let mut buf = Vec::new();
        write_highlighted(&mut buf, "hello world", &["world".to_string()]).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("world"));
    }

    #[test]
    fn test_print_session_header_no_color() {
        let row = SearchResult {
            session_id: "s1".into(),
            source: "claude-code".into(),
            cwd: "/tmp/project".into(),
            slug: String::new(),
            git_branches: "[]".into(),
            start_time: "2024-01-01T00:00:00.000Z".into(),
            end_time: "2024-01-01T00:05:00.000Z".into(),
            files_touched: "[]".into(),
            tools_used: "[]".into(),
            message_count: 10,
            first_message: String::new(),
            summary: String::new(),
            content_hash: None,
            custom_title: None,
            metadata: None,
            rank: 0.0,
        };
        let mut buf = Vec::new();
        print_session_header(&mut buf, &row, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("/tmp/project"));
        assert!(s.contains("2024-01-01"));
        assert!(s.contains("5m"));
        assert!(s.contains("10 msgs"));
    }
}
