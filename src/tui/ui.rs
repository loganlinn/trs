//! Layout and rendering for the TUI.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Table, Wrap,
};
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
pub fn draw(f: &mut Frame, app: &mut App) {
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
            let return_mode = app.help_return_mode.clone();
            match return_mode {
                Mode::Detail => draw_detail(f, app, chunks[1]),
                _ => draw_results(f, app, chunks[1]),
            }
            draw_help_overlay(f, app, f.area());
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

fn draw_results(f: &mut Frame, app: &mut App, area: Rect) {
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

    // Split area: table on top, preview pane on bottom
    let show_preview = area.height >= 16;
    let chunks = if show_preview {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area)
    } else {
        // Too short for preview — table takes full height
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(100)])
            .split(area)
    };

    draw_table(f, app, chunks[0]);
    if show_preview && chunks.len() > 1 {
        draw_preview(f, app, chunks[1]);
    }
}

fn draw_table(f: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Date"),
        Cell::from("Project"),
        Cell::from("Title"),
        Cell::from("Msgs"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let hl_style = Style::default()
        .fg(Color::Red)
        .add_modifier(Modifier::BOLD);
    let terms = &app.search_terms;

    let rows: Vec<Row> = app
        .results
        .iter()
        .map(|result| result_table_row(result, terms, hl_style))
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(15),
        Constraint::Fill(1),
        Constraint::Length(4),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .column_spacing(1);

    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn result_table_row<'a>(result: &SearchResult, terms: &[String], hl_style: Style) -> Row<'a> {
    let date = display::relative_date(&result.start_time);
    let project = if !result.cwd.is_empty() {
        display::project_slug(&result.cwd).to_string()
    } else {
        result.session_id[..8.min(result.session_id.len())].to_string()
    };

    let title = if let Some(ref t) = result.custom_title {
        if !t.is_empty() {
            t.clone()
        } else {
            first_line_preview(&result.summary, &result.first_message)
        }
    } else {
        first_line_preview(&result.summary, &result.first_message)
    };

    let msgs = if result.message_count > 0 {
        format!("{:>4}", result.message_count)
    } else {
        String::new()
    };

    let date_style = Style::default().fg(Color::DarkGray);
    let project_style = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);
    let title_style = Style::default().fg(Color::White);
    let msgs_style = Style::default().fg(Color::DarkGray);

    Row::new(vec![
        Cell::from(Span::styled(date, date_style)),
        Cell::from(Line::from(highlight_spans(&project, terms, project_style, hl_style))),
        Cell::from(Line::from(highlight_spans(&title, terms, title_style, hl_style))),
        Cell::from(Span::styled(msgs, msgs_style)),
    ])
}

/// Extract a one-line preview from summary or first_message.
fn first_line_preview(summary: &str, first_message: &str) -> String {
    let src = if !summary.is_empty() {
        summary
    } else if !first_message.is_empty() {
        first_message
    } else {
        return String::new();
    };
    let line = src.lines().next().unwrap_or("").trim();
    let line = line.replace('\t', " ");
    if line.len() > 120 {
        format!("{}…", &line[..119])
    } else {
        line.to_string()
    }
}

fn draw_preview(f: &mut Frame, app: &App, area: Rect) {
    let result = match app.selected_result() {
        Some(r) => r,
        None => return,
    };

    // Build title bar
    let project = if !result.cwd.is_empty() {
        display::project_slug(&result.cwd)
    } else {
        &result.session_id
    };

    let title_text = if let Some(ref t) = result.custom_title {
        if !t.is_empty() {
            format!(" {project} \u{b7} {t} ")
        } else {
            format!(" {project} ")
        }
    } else {
        format!(" {project} ")
    };

    // Right side: date · msgs · duration
    let mut meta_parts = Vec::new();
    let rel_date = display::relative_date(&result.start_time);
    if !rel_date.is_empty() {
        meta_parts.push(rel_date);
    }
    if result.message_count > 0 {
        meta_parts.push(format!("{} msgs", result.message_count));
    }
    let duration = display::format_duration(&result.start_time, &result.end_time);
    if !duration.is_empty() {
        meta_parts.push(duration);
    }
    let branches: Vec<String> =
        serde_json::from_str(&result.git_branches).unwrap_or_default();
    if !branches.is_empty() {
        meta_parts.push(format!("@{}", branches.join(",")));
    }

    let meta_str = if meta_parts.is_empty() {
        String::new()
    } else {
        format!(" {} ", meta_parts.join(" \u{b7} "))
    };

    let title_line = Line::from(Span::styled(
        title_text,
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));

    let bottom_line =
        Line::from(Span::styled(meta_str, Style::default().fg(Color::DarkGray))).right_aligned();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title_line)
        .title_bottom(bottom_line);

    let inner = block.inner(area);

    // Build preview content
    let lines = build_preview_lines(app, result, inner.width as usize);

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(block, area);
    f.render_widget(paragraph, inner);
}

fn build_preview_lines<'a>(
    app: &App,
    result: &SearchResult,
    _width: usize,
) -> Vec<Line<'a>> {
    let terms = &app.search_terms;
    let hl_style = Style::default()
        .fg(Color::Red)
        .add_modifier(Modifier::BOLD);

    // Search mode: show matched snippets with tree connectors
    if !terms.is_empty() && !app.preview_snippets.is_empty() {
        let mut lines = Vec::new();
        let count = app.preview_snippets.len();

        for (i, snippet) in app.preview_snippets.iter().enumerate() {
            let connector = if i + 1 < count { "\u{251c}" } else { "\u{2514}" }; // ├ or └
            let rc = role_color(&snippet.role);
            let marker_span = Span::styled(
                format!(" {connector} {} ", snippet.marker),
                Style::default().fg(rc),
            );
            let idx_span = Span::styled(
                format!("{:>3}  ", snippet.index + 1),
                Style::default().fg(Color::DarkGray),
            );

            // First line of snippet
            if let Some(first) = snippet.lines.first() {
                let text_style = if snippet.is_match {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let mut spans = vec![marker_span, idx_span];
                if snippet.is_match && !terms.is_empty() {
                    spans.extend(highlight_spans(&first.text, terms, text_style, hl_style));
                } else {
                    spans.push(Span::styled(first.text.clone(), text_style));
                }
                lines.push(Line::from(spans));
            }

            // Continuation lines (indented under tree)
            let indent = if i + 1 < count { " \u{2502}       " } else { "         " }; // │ or spaces
            for line in snippet.lines.iter().skip(1) {
                if line.is_gap {
                    lines.push(Line::from(Span::styled(
                        format!("{indent}{}", line.text),
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    let text_style = if snippet.is_match {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    let mut spans = vec![Span::raw(indent.to_string())];
                    if snippet.is_match && !terms.is_empty() {
                        spans.extend(highlight_spans(&line.text, terms, text_style, hl_style));
                    } else {
                        spans.push(Span::styled(line.text.clone(), text_style));
                    }
                    lines.push(Line::from(spans));
                }
            }
        }
        return lines;
    }

    // Browse mode: show first_message as a simple preview
    let preview_text = if !result.first_message.is_empty() {
        &result.first_message
    } else if !result.summary.is_empty() {
        &result.summary
    } else {
        return vec![Line::from(Span::styled(
            "No preview available",
            Style::default().fg(Color::DarkGray),
        ))];
    };

    let marker = Span::styled(
        "\u{276f} ",
        Style::default().fg(Color::Green),
    );

    let text_style = Style::default().fg(Color::Gray);
    let text_lines: Vec<&str> = preview_text.lines().collect();
    let mut lines = Vec::new();
    for (i, text_line) in text_lines.iter().take(8).enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(marker.clone());
        } else {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(text_line.to_string(), text_style));
        lines.push(Line::from(spans));
    }
    if text_lines.len() > 8 {
        lines.push(Line::from(Span::styled(
            format!("  \u{2026} +{} more lines", text_lines.len() - 8),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
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

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    let mut scrollbar_state = ScrollbarState::new(app.detail_messages.len().saturating_sub(1))
        .position(app.detail_scroll);
    f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
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
                    format!("{}/{}", app.selected_index() + 1, app.results.len())
                }
            }
            Mode::Detail => format!("scroll: {}", app.detail_scroll + 1),
            Mode::Help => "Help".to_string(),
        }
    };

    let keys = &app.keys;
    let right = match app.mode {
        Mode::Normal => format!(
            "{}:resume  {}:fork  {}:detail  {}:help  Esc:quit",
            keys.normal.resume_session.display(),
            keys.normal.fork_session.display(),
            keys.normal.open_detail.display(),
            keys.normal.toggle_help.display(),
        ),
        Mode::Detail => format!(
            "{}:back  {}:matches  {}:scroll  {}:help",
            keys.detail.back.display(),
            keys.detail.next_match.display(),
            keys.detail.scroll_down.display(),
            keys.detail.toggle_help.display(),
        ),
        Mode::Help => format!("{}:close", keys.help.close.display()),
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

fn draw_help_overlay(f: &mut Frame, app: &App, area: Rect) {
    let keys = &app.keys;

    fn help_row(key: &str, desc: &str) -> Line<'static> {
        Line::from(format!("  {key:<16}{desc}"))
    }

    let section_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let help_text = vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("Results View", section_style)),
        help_row(&keys.normal.resume_session.display(), "Resume session (--resume)"),
        help_row(&keys.normal.fork_session.display(), "Fork session (--fork-session)"),
        help_row(&keys.normal.open_detail.display(), "Open session detail"),
        Line::from("  Esc             Quit (or clear input)"),
        help_row(&keys.normal.select_prev.display(), "Previous result"),
        help_row(&keys.normal.select_next.display(), "Next result"),
        help_row(&keys.normal.clear_input.display(), "Clear search"),
        help_row(
            &format!(
                "{}/{}",
                keys.normal.scroll_half_down.display(),
                keys.normal.scroll_half_up.display()
            ),
            "Half-page scroll",
        ),
        help_row(&keys.normal.copy_session_id.display(), "Show session ID"),
        help_row(&keys.normal.toggle_help.display(), "Toggle this help"),
        Line::from(""),
        Line::from(Span::styled("Detail View", section_style)),
        help_row(&keys.detail.back.display(), "Back to results"),
        help_row(&keys.detail.scroll_down.display(), "Scroll down/up"),
        help_row(
            &format!("{}/{}", keys.detail.top.display(), keys.detail.bottom.display()),
            "Top/bottom",
        ),
        help_row(
            &format!("{}/{}", keys.detail.next_match.display(), keys.detail.prev_match.display()),
            "Next/prev match",
        ),
        help_row(&keys.detail.focus_search.display(), "Focus search input"),
        help_row(&keys.detail.copy_session_id.display(), "Show session ID"),
        Line::from(""),
        Line::from(Span::styled("Search Filters", section_style)),
        Line::from("  app:codex       Filter by app (claude/cc, codex/cx)"),
        Line::from("  p:gamma         Filter by project/cwd"),
        Line::from("  f:*.rs          Filter by file path"),
        Line::from("  b:main          Filter by git branch"),
    ];

    let help_width = 60u16;
    let help_height = help_text.len() as u16 + 2; // +2 for border
    let x = area.width.saturating_sub(help_width) / 2;
    let y = area.height.saturating_sub(help_height) / 2;
    let overlay = Rect::new(
        x,
        y,
        help_width.min(area.width),
        help_height.min(area.height),
    );

    f.render_widget(Clear, overlay);

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
