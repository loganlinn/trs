use std::io::Write;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use regex::Regex;

use crate::display::{MessageSnippet, ResultDisplay, ResultGroup};
use crate::session::{App, MessageRole};

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

// ---------------------------------------------------------------------------
// Group / Result rendering from display model
// ---------------------------------------------------------------------------

/// Print a result group (one or more sessions sharing a project).
pub fn print_group(
    w: &mut dyn Write,
    group: &ResultGroup,
    terms: &[String],
    color: bool,
) -> std::io::Result<()> {
    let single = group.results.len() == 1;

    if !single {
        // Group header
        if color {
            write!(
                w,
                "{}{}{}{}",
                SetForegroundColor(Color::Magenta),
                SetAttribute(Attribute::Bold),
                group.project_name,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(w, "{}", group.project_name)?;
        }
        if !group.branches.is_empty() {
            let branch_str = format!("  @{}", group.branches.join(","));
            if color {
                write!(
                    w,
                    "{}{}{}",
                    SetAttribute(Attribute::Dim),
                    branch_str,
                    SetAttribute(Attribute::Reset),
                )?;
            } else {
                write!(w, "{branch_str}")?;
            }
        }
        writeln!(w)?;
    }

    for (i, result) in group.results.iter().enumerate() {
        let prefix = if single { "" } else { "  " };
        print_result(w, result, terms, color, prefix)?;
        if i + 1 < group.results.len() {
            writeln!(w)?;
        }
    }
    Ok(())
}

/// Print a single result with its header, snippets, and footer.
fn print_result(
    w: &mut dyn Write,
    result: &ResultDisplay,
    terms: &[String],
    color: bool,
    prefix: &str,
) -> std::io::Result<()> {
    // Header line
    write!(w, "{prefix}")?;
    print_result_header(w, result, color)?;

    // Snippets
    if !result.snippets.is_empty() {
        let mut prev_idx: Option<usize> = None;
        for snippet in &result.snippets {
            if let Some(prev) = prev_idx {
                if snippet.index > prev + 1 {
                    if color {
                        writeln!(
                            w,
                            "{prefix}{}{}{}",
                            SetAttribute(Attribute::Dim),
                            GROUP_SEP,
                            SetAttribute(Attribute::Reset),
                        )?;
                    } else {
                        writeln!(w, "{prefix}{GROUP_SEP}")?;
                    }
                }
            }
            write!(w, "{prefix}")?;
            print_snippet(w, snippet, terms, color)?;
            writeln!(w)?;
            prev_idx = Some(snippet.index);
        }
    } else {
        // Fallback: show first_message/summary
        let desc = if !result.summary.is_empty() {
            &result.summary
        } else {
            &result.first_message
        };
        if !desc.is_empty() {
            let desc = desc.replace('\n', " ");
            let desc = desc.trim().to_string();
            let desc = if desc.len() > 200 {
                format!("{}...", &desc[..197])
            } else {
                desc
            };
            if color && !terms.is_empty() {
                write!(w, "{prefix}     ")?;
                write_highlighted(w, &desc, terms)?;
                writeln!(w)?;
            } else {
                writeln!(w, "{prefix}     {desc}")?;
            }
        }
    }

    // Footer
    write!(w, "{prefix}")?;
    print_result_footer(w, result, color)?;
    writeln!(w)?;
    Ok(())
}

/// Print result header from ResultDisplay.
fn print_result_header(
    w: &mut dyn Write,
    result: &ResultDisplay,
    color: bool,
) -> std::io::Result<()> {
    let has_title = result
        .custom_title
        .as_deref()
        .is_some_and(|t| !t.is_empty());

    if color {
        write!(
            w,
            "{}{}{}{}",
            SetForegroundColor(Color::Magenta),
            SetAttribute(Attribute::Bold),
            result.project_name,
            SetAttribute(Attribute::Reset),
        )?;
    } else {
        write!(w, "{}", result.project_name)?;
    }

    // Metadata
    let mut meta_parts = Vec::new();
    if !result.branches.is_empty() {
        meta_parts.push(format!("@{}", result.branches.join(",")));
    }
    if !result.date.is_empty() {
        meta_parts.push(result.date.clone());
    }
    if !result.duration.is_empty() {
        meta_parts.push(result.duration.clone());
    }
    if result.message_count > 0 {
        meta_parts.push(format!("{} msgs", result.message_count));
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

    // Append custom title after metadata
    if has_title {
        let title = result.custom_title.as_deref().unwrap();
        if color {
            write!(
                w,
                "  {}{}{}",
                SetForegroundColor(Color::Cyan),
                title,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            write!(w, "  {}", title)?;
        }
    }

    writeln!(w)?;
    write!(w, "{}", ResetColor)?;
    Ok(())
}

/// Print result footer (resume command).
fn print_result_footer(
    w: &mut dyn Write,
    result: &ResultDisplay,
    color: bool,
) -> std::io::Result<()> {
    let cmd = match App::parse(&result.source) {
        Some(app) => app.resume_cmd(&result.session_id),
        None => format!("{}  {}", result.source, result.session_id),
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

/// Print a message snippet line.
fn print_snippet(
    w: &mut dyn Write,
    snippet: &MessageSnippet,
    terms: &[String],
    color: bool,
) -> std::io::Result<()> {
    let num_width = 4;
    let rc = role_color(&snippet.role);

    // Line number (right-aligned, dim)
    if color {
        write!(
            w,
            "{}{:>width$}{}",
            SetAttribute(Attribute::Dim),
            snippet.index + 1,
            SetAttribute(Attribute::Reset),
            width = num_width,
        )?;
    } else {
        write!(w, "{:>width$}", snippet.index + 1, width = num_width)?;
    }

    // Marker + role label
    let attr = if snippet.is_match {
        Attribute::Bold
    } else {
        Attribute::Dim
    };
    if color {
        write!(
            w,
            " {}{}{}{} {}{}{}{}",
            SetForegroundColor(rc),
            SetAttribute(attr),
            snippet.marker,
            SetAttribute(Attribute::Reset),
            SetForegroundColor(rc),
            SetAttribute(attr),
            snippet.label,
            SetAttribute(Attribute::Reset),
        )?;
    } else {
        write!(w, " {} {}", snippet.marker, snippet.label)?;
    }

    // Snippet lines
    let prefix_width = num_width + 1 + 1 + 1 + snippet.label.len() + 1;
    let indent = " ".repeat(prefix_width);

    for (i, line) in snippet.lines.iter().enumerate() {
        if line.is_gap {
            if i > 0 {
                writeln!(w)?;
            }
            if color {
                write!(
                    w,
                    "{}{}{}{}",
                    indent,
                    SetAttribute(Attribute::Dim),
                    line.text,
                    SetAttribute(Attribute::Reset),
                )?;
            } else {
                write!(w, "{}{}", indent, line.text)?;
            }
            continue;
        }

        if i == 0 {
            if snippet.is_match && !terms.is_empty() && color {
                write!(w, " ")?;
                write_highlighted(w, &line.text, terms)?;
            } else if color && !snippet.is_match {
                write!(
                    w,
                    " {}{}{}",
                    SetAttribute(Attribute::Dim),
                    line.text,
                    SetAttribute(Attribute::Reset),
                )?;
            } else {
                write!(w, " {}", line.text)?;
            }
        } else {
            writeln!(w)?;
            if snippet.is_match && !terms.is_empty() && color {
                write!(w, "{indent}")?;
                write_highlighted(w, &line.text, terms)?;
            } else if color && !snippet.is_match {
                write!(
                    w,
                    "{}{}{}{}",
                    indent,
                    SetAttribute(Attribute::Dim),
                    line.text,
                    SetAttribute(Attribute::Reset),
                )?;
            } else {
                write!(w, "{}{}", indent, line.text)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::SnippetLine;

    #[test]
    fn test_write_highlighted() {
        let mut buf = Vec::new();
        write_highlighted(&mut buf, "hello world", &["world".to_string()]).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("world"));
    }

    #[test]
    fn test_print_result_header_no_color() {
        let result = ResultDisplay {
            project_name: "project".into(),
            full_path: "/tmp/project".into(),
            branches: vec![],
            date: "2024-01-01".into(),
            duration: "5m".into(),
            message_count: 10,
            session_id: "s1".into(),
            source: "claude-code".into(),
            first_message: String::new(),
            summary: String::new(),
            custom_title: None,
            snippets: vec![],
        };
        let mut buf = Vec::new();
        print_result_header(&mut buf, &result, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("project"));
        assert!(s.contains("2024-01-01"));
        assert!(s.contains("5m"));
        assert!(s.contains("10 msgs"));
    }

    #[test]
    fn test_print_result_header_custom_title() {
        let result = ResultDisplay {
            project_name: "trs".into(),
            full_path: "/Users/logan/src/trs".into(),
            branches: vec!["main".into()],
            date: "2024-01-01".into(),
            duration: "5m".into(),
            message_count: 32,
            session_id: "s1".into(),
            source: "claude-code".into(),
            first_message: String::new(),
            summary: String::new(),
            custom_title: Some("tui-search-perf".into()),
            snippets: vec![],
        };
        let mut buf = Vec::new();
        print_result_header(&mut buf, &result, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("trs"), "should start with project slug: {s}");
        assert!(
            s.contains("tui-search-perf"),
            "should contain custom title: {s}"
        );
        assert!(s.contains("@main"));
    }

    #[test]
    fn test_print_snippet_match() {
        let snippet = MessageSnippet {
            index: 0,
            role: MessageRole::User,
            marker: "❯",
            label: "user".into(),
            is_match: true,
            lines: vec![SnippetLine {
                text: "help me with search".into(),
                is_gap: false,
                highlights: vec![(13, 19)],
            }],
            teammate_summary: String::new(),
        };
        let mut buf = Vec::new();
        print_snippet(&mut buf, &snippet, &["search".into()], false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("❯"));
        assert!(s.contains("user"));
        assert!(s.contains("help me with search"));
    }

    #[test]
    fn test_print_group_single() {
        let result = ResultDisplay {
            project_name: "trs".into(),
            full_path: "/tmp/trs".into(),
            branches: vec![],
            date: "2024-01-01".into(),
            duration: "5m".into(),
            message_count: 10,
            session_id: "s1".into(),
            source: "claude-code".into(),
            first_message: "hello".into(),
            summary: String::new(),
            custom_title: None,
            snippets: vec![],
        };
        let group = ResultGroup {
            project_name: "trs".into(),
            full_path: "/tmp/trs".into(),
            branches: vec![],
            results: vec![result],
        };
        let mut buf = Vec::new();
        print_group(&mut buf, &group, &[], false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Single result: no group header prefix
        assert!(s.starts_with("trs"));
    }

    #[test]
    fn test_print_group_multiple() {
        let mk = |id: &str| ResultDisplay {
            project_name: "trs".into(),
            full_path: "/tmp/trs".into(),
            branches: vec![],
            date: "2024-01-01".into(),
            duration: "5m".into(),
            message_count: 10,
            session_id: id.into(),
            source: "claude-code".into(),
            first_message: "hello".into(),
            summary: String::new(),
            custom_title: None,
            snippets: vec![],
        };
        let group = ResultGroup {
            project_name: "trs".into(),
            full_path: "/tmp/trs".into(),
            branches: vec!["main".into()],
            results: vec![mk("s1"), mk("s2")],
        };
        let mut buf = Vec::new();
        print_group(&mut buf, &group, &[], false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Multi result: group header first, then indented results
        assert!(s.starts_with("trs"));
        assert!(s.contains("  trs")); // indented child
    }
}
