//! Layout and rendering for the TUI.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::display;
use crate::session::{MessageRole, SearchResult};

use super::app::{App, Mode};

/// TUI color mapping for roles (matches CLI's role_color).
fn role_color(role: &MessageRole) -> Color {
    match role {
        MessageRole::User => Color::Green,
        MessageRole::Assistant => Color::Blue,
        MessageRole::Summary => Color::Yellow,
        MessageRole::Teammate => Color::Cyan,
    }
}

/// Main draw function dispatching to the active mode.
pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search input
            Constraint::Min(5),    // results or detail
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    draw_search_input(f, app, chunks[0]);

    match app.mode {
        Mode::Normal => draw_results(f, app, chunks[1]),
        Mode::Detail => draw_detail(f, app, chunks[1]),
        Mode::Help => {
            match app.help_return_mode {
                Mode::Detail => draw_detail(f, app, chunks[1]),
                _ => draw_results(f, app, chunks[1]),
            }
            draw_help_overlay(f, f.area());
        }
    }

    draw_status_bar(f, app, chunks[2]);
}

fn draw_search_input(f: &mut Frame, app: &App, area: Rect) {
    let input_value = app.input.value();
    let cursor_pos = app.input.visual_cursor();

    let border_color = if app.mode == Mode::Normal {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let title = if app.pinned.is_empty() {
        Line::from(" Search (FTS5) ")
    } else {
        Line::from(vec![
            Span::raw(" Search (FTS5) "),
            Span::styled(
                format!("[{}] ", app.pinned.display()),
                Style::default().fg(Color::Yellow),
            ),
        ])
    };

    let input_display = Paragraph::new(input_value)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title),
        );

    f.render_widget(input_display, area);

    // Place cursor in the input when in Normal mode
    if app.mode == Mode::Normal {
        // +1 for border
        f.set_cursor_position((area.x + cursor_pos as u16 + 1, area.y + 1));
    }
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    if app.results.is_empty() {
        let msg = if app.input.value().is_empty() {
            "Type to search sessions. Press Ctrl-/ for help."
        } else {
            "No results."
        };
        let p = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title(" Results "));
        f.render_widget(p, area);
        return;
    }

    let visible_height = area.height.saturating_sub(2) as usize; // minus borders
    let offset = if app.selected >= app.scroll_offset + visible_height {
        app.selected - visible_height + 1
    } else if app.selected < app.scroll_offset {
        app.selected
    } else {
        app.scroll_offset
    };

    let items: Vec<ListItem> = app
        .results
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible_height)
        .map(|(i, result)| {
            let is_selected = i == app.selected;
            result_list_item(result, is_selected, &app.search_terms)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Results ({}) ", app.results.len())),
    );

    f.render_widget(list, area);
}

fn result_list_item<'a>(
    result: &SearchResult,
    is_selected: bool,
    terms: &[String],
) -> ListItem<'a> {
    let branches: Vec<String> = serde_json::from_str(&result.git_branches).unwrap_or_default();

    let has_title = result
        .custom_title
        .as_deref()
        .is_some_and(|t| !t.is_empty());
    let primary = if !result.cwd.is_empty() {
        display::project_slug(&result.cwd).to_string()
    } else {
        result.session_id.clone()
    };

    let mut meta_parts = Vec::new();
    if !branches.is_empty() {
        meta_parts.push(format!("@{}", branches.join(",")));
    }
    if result.start_time.len() >= 10 {
        meta_parts.push(result.start_time[..10].to_string());
    }
    if result.message_count > 0 {
        meta_parts.push(format!("{} msgs", result.message_count));
    }

    let meta_str = if meta_parts.is_empty() {
        String::new()
    } else {
        format!("  {}", meta_parts.join("  "))
    };

    // Line 2: first_message preview
    let preview = if !result.first_message.is_empty() {
        let fm = result.first_message.replace('\n', " ");
        let fm = fm.trim();
        if fm.len() > 120 {
            format!("  {}...", &fm[..117])
        } else {
            format!("  {fm}")
        }
    } else if !result.summary.is_empty() {
        let s = result.summary.replace('\n', " ");
        let s = s.trim().to_string();
        if s.len() > 120 {
            format!("  {}...", &s[..117])
        } else {
            format!("  {s}")
        }
    } else {
        String::new()
    };

    let select_style = if is_selected {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let header_style = if is_selected {
        Style::default()
            .fg(Color::Magenta)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    };

    let title_style = if is_selected {
        Style::default()
            .fg(Color::Cyan)
            .bg(Color::DarkGray)
    } else {
        Style::default()
            .fg(Color::Cyan)
    };

    let meta_style = if is_selected {
        Style::default().fg(Color::Gray).bg(Color::DarkGray)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let preview_style = if is_selected {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Gray)
    };

    let indicator = if is_selected { "> " } else { "  " };

    let highlight_style = Style::default()
        .fg(Color::Red)
        .add_modifier(Modifier::BOLD);

    let mut header_spans = vec![Span::styled(indicator, select_style)];
    header_spans.extend(highlight_spans(&primary, terms, header_style, highlight_style));
    header_spans.extend(highlight_spans(&meta_str, terms, meta_style, highlight_style));
    if has_title {
        let title_text = format!("  {}", result.custom_title.as_deref().unwrap());
        header_spans.extend(highlight_spans(&title_text, terms, title_style, highlight_style));
    }

    let mut lines = vec![Line::from(header_spans)];

    if !preview.is_empty() {
        let mut preview_spans = vec![Span::raw("  ")];
        preview_spans.extend(highlight_spans(&preview, terms, preview_style, highlight_style));
        lines.push(Line::from(preview_spans));
    }

    ListItem::new(lines)
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    if app.detail_messages.is_empty() {
        let p = Paragraph::new("No messages to display.")
            .block(Block::default().borders(Borders::ALL).title(" Detail "));
        f.render_widget(p, area);
        return;
    }

    let result = match app.selected_result() {
        Some(r) => r,
        None => return,
    };

    let has_title = result
        .custom_title
        .as_deref()
        .is_some_and(|t| !t.is_empty());
    let title = if has_title {
        let slug = display::project_slug(&result.cwd);
        format!(" {} ({slug}) ", result.custom_title.as_deref().unwrap())
    } else if !result.cwd.is_empty() {
        format!(" {} ", display::project_slug(&result.cwd))
    } else {
        format!(" {} ", result.session_id)
    };

    let match_info = if !app.detail_match_indices.is_empty() {
        format!(
            " match {}/{} ",
            app.detail_current_match + 1,
            app.detail_match_indices.len()
        )
    } else {
        String::new()
    };

    let visible_height = area.height.saturating_sub(2) as usize;

    let lines: Vec<Line> = app
        .detail_messages
        .iter()
        .skip(app.detail_scroll)
        .take(visible_height)
        .flat_map(|msg| {
            let is_match = app.detail_match_indices.contains(&msg.index);
            message_to_lines(msg, is_match, &app.search_terms)
        })
        .collect();

    let title_line = Line::from(vec![
        Span::styled(title, Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
    ]);

    let bottom_line = if match_info.is_empty() {
        Line::from("")
    } else {
        Line::from(Span::styled(match_info, Style::default().fg(Color::DarkGray))).right_aligned()
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(title_line)
                .title_bottom(bottom_line),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

/// Style constants matching chat's conversation rendering.
const INDENT: &str = "   ";
const MAX_BODY_LINES: usize = 40;

fn message_to_lines<'a>(
    msg: &crate::session::Message,
    is_match: bool,
    terms: &[String],
) -> Vec<Line<'a>> {
    let marker = display::role_marker(&msg.role);
    let label = display::role_label(msg);
    let rc = role_color(&msg.role);

    let mut lines = Vec::new();

    let num_style = Style::default().fg(Color::DarkGray);
    let marker_style = Style::default().fg(rc);
    let label_style = Style::default()
        .fg(rc)
        .add_modifier(Modifier::BOLD);

    let mut header_spans = vec![
        Span::styled(format!("{:>3} ", msg.index + 1), num_style),
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(label, label_style),
    ];

    if is_match {
        header_spans.push(Span::styled(
            " \u{25c0}",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    lines.push(Line::from(header_spans));

    // Message body
    let text = msg.text.trim();
    let text_lines: Vec<&str> = text.lines().collect();
    let show_count = text_lines.len().min(MAX_BODY_LINES);

    for text_line in &text_lines[..show_count] {
        let styled_line = if text_line.starts_with("$ ") {
            Line::from(vec![
                Span::raw(INDENT),
                Span::styled(
                    text_line.to_string(),
                    Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
                ),
            ])
        } else if text_line.starts_with('[') && text_line.ends_with(']') {
            Line::from(vec![
                Span::raw(INDENT),
                Span::styled(
                    text_line.to_string(),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ),
            ])
        } else if is_match && !terms.is_empty() {
            highlight_line(text_line, terms)
        } else {
            let body_color = if is_match {
                Color::White
            } else {
                Color::Gray
            };
            Line::from(vec![
                Span::raw(INDENT),
                Span::styled(text_line.to_string(), Style::default().fg(body_color)),
            ])
        };
        lines.push(styled_line);
    }

    if text_lines.len() > MAX_BODY_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "{INDENT}\u{2026} +{} more lines",
                text_lines.len() - MAX_BODY_LINES
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));

    lines
}

/// Split text into spans, highlighting substrings that match any search term.
fn highlight_spans<'a>(
    text: &str,
    terms: &[String],
    base_style: Style,
    hl_style: Style,
) -> Vec<Span<'a>> {
    if text.is_empty() {
        return vec![];
    }
    if terms.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let escaped: Vec<String> = terms.iter().map(|t| regex::escape(t)).collect();
    let pattern = match regex::RegexBuilder::new(&escaped.join("|"))
        .case_insensitive(true)
        .build()
    {
        Ok(re) => re,
        Err(_) => return vec![Span::styled(text.to_string(), base_style)],
    };

    let mut spans = Vec::new();
    let mut last = 0;
    for m in pattern.find_iter(text) {
        if m.start() > last {
            spans.push(Span::styled(
                text[last..m.start()].to_string(),
                base_style,
            ));
        }
        spans.push(Span::styled(m.as_str().to_string(), hl_style));
        last = m.end();
    }
    if last < text.len() {
        spans.push(Span::styled(text[last..].to_string(), base_style));
    }
    spans
}

/// Highlight search terms in a single line of text (for detail view).
fn highlight_line<'a>(text: &str, terms: &[String]) -> Line<'a> {
    let hl_style = Style::default()
        .fg(Color::Red)
        .add_modifier(Modifier::BOLD);
    let base_style = Style::default().fg(Color::White);
    let mut spans = vec![Span::raw(INDENT.to_string())];
    spans.extend(highlight_spans(text, terms, base_style, hl_style));
    Line::from(spans)
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let left = if !app.status_message.is_empty() {
        app.status_message.clone()
    } else {
        match app.mode {
            Mode::Normal => {
                if app.results.is_empty() {
                    String::new()
                } else {
                    format!("{}/{}", app.selected + 1, app.results.len())
                }
            }
            Mode::Detail => {
                let pos_info = format!("scroll: {}", app.detail_scroll + 1);
                pos_info
            }
            Mode::Help => "Help".to_string(),
        }
    };

    let right = match app.mode {
        Mode::Normal => "Enter:resume  S-Enter:fork  Tab:detail  C-/:help  Esc:quit",
        Mode::Detail => "Esc:back  n/N:matches  j/k:scroll  C-/:help",
        Mode::Help => "Esc:close",
    };

    let available = area.width as usize;
    let right_len = right.len();
    let left_max = available.saturating_sub(right_len + 2);
    let left_display = if left.len() > left_max {
        &left[..left_max]
    } else {
        &left
    };
    let padding = available.saturating_sub(left_display.len() + right_len);

    let bar = Line::from(vec![
        Span::styled(left_display.to_string(), Style::default().fg(Color::Cyan)),
        Span::raw(" ".repeat(padding)),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ]);

    let p = Paragraph::new(bar).style(Style::default().bg(Color::Black));
    f.render_widget(p, area);
}

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let help_width = 60u16;
    let help_height = 28u16;
    let x = area.width.saturating_sub(help_width) / 2;
    let y = area.height.saturating_sub(help_height) / 2;
    let overlay = Rect::new(
        x,
        y,
        help_width.min(area.width),
        help_height.min(area.height),
    );

    f.render_widget(Clear, overlay);

    let help_text = vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Results View",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Enter         Resume session (--resume)"),
        Line::from("  Shift-Enter   Fork session (--fork-session)"),
        Line::from("  Tab           Open session detail"),
        Line::from("  Esc           Quit (or clear input)"),
        Line::from("  Up/Ctrl-P     Previous result"),
        Line::from("  Down/Ctrl-N   Next result"),
        Line::from("  Ctrl-U        Clear search"),
        Line::from("  Ctrl-D/Ctrl-B Half-page scroll"),
        Line::from("  y             Show session ID"),
        Line::from("  r             Show resume command"),
        Line::from("  Ctrl-/        Toggle this help"),
        Line::from(""),
        Line::from(Span::styled(
            "Detail View",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Esc/q         Back to results"),
        Line::from("  j/k           Scroll down/up"),
        Line::from("  g/G           Top/bottom"),
        Line::from("  n/N           Next/prev match"),
        Line::from("  /             Focus search input"),
        Line::from("  y             Show session ID"),
        Line::from("  r             Show resume command"),
        Line::from(""),
        Line::from(Span::styled(
            "Search Filters",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  app:codex     Filter by app (claude/cc, codex/cx)"),
        Line::from("  p:gamma       Filter by project/cwd"),
        Line::from("  f:*.rs        Filter by file path"),
        Line::from("  b:main        Filter by git branch"),
    ];

    let paragraph = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Help "),
    );

    f.render_widget(paragraph, overlay);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Style = Style::new();
    const HL: Style = Style {
        fg: Some(Color::Red),
        ..Style::new()
    };

    fn terms(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    fn span_texts<'a>(spans: &'a [Span<'a>]) -> Vec<(&'a str, Style)> {
        spans.iter().map(|s| (s.content.as_ref(), s.style)).collect()
    }

    #[test]
    fn empty_text_returns_empty() {
        assert!(highlight_spans("", &terms(&["foo"]), BASE, HL).is_empty());
    }

    #[test]
    fn no_terms_returns_whole_text() {
        let spans = highlight_spans("hello world", &[], BASE, HL);
        assert_eq!(span_texts(&spans), vec![("hello world", BASE)]);
    }

    #[test]
    fn single_match_at_start() {
        let spans = highlight_spans("foo bar", &terms(&["foo"]), BASE, HL);
        assert_eq!(span_texts(&spans), vec![("foo", HL), (" bar", BASE)]);
    }

    #[test]
    fn single_match_at_end() {
        let spans = highlight_spans("hello world", &terms(&["world"]), BASE, HL);
        assert_eq!(span_texts(&spans), vec![("hello ", BASE), ("world", HL)]);
    }

    #[test]
    fn multiple_matches() {
        let spans = highlight_spans("foo bar foo", &terms(&["foo"]), BASE, HL);
        assert_eq!(
            span_texts(&spans),
            vec![("foo", HL), (" bar ", BASE), ("foo", HL)]
        );
    }

    #[test]
    fn case_insensitive() {
        let spans = highlight_spans("Hello HELLO hello", &terms(&["hello"]), BASE, HL);
        assert_eq!(
            span_texts(&spans),
            vec![("Hello", HL), (" ", BASE), ("HELLO", HL), (" ", BASE), ("hello", HL)]
        );
    }

    #[test]
    fn multiple_terms() {
        let spans = highlight_spans("the quick brown fox", &terms(&["quick", "fox"]), BASE, HL);
        assert_eq!(
            span_texts(&spans),
            vec![("the ", BASE), ("quick", HL), (" brown ", BASE), ("fox", HL)]
        );
    }

    #[test]
    fn no_match_returns_whole_text() {
        let spans = highlight_spans("hello world", &terms(&["xyz"]), BASE, HL);
        assert_eq!(span_texts(&spans), vec![("hello world", BASE)]);
    }

    #[test]
    fn regex_special_chars_escaped() {
        let spans = highlight_spans("foo (bar) baz", &terms(&["(bar)"]), BASE, HL);
        assert_eq!(
            span_texts(&spans),
            vec![("foo ", BASE), ("(bar)", HL), (" baz", BASE)]
        );
    }
}
