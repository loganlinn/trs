//! Layout and rendering for the TUI.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::session::{MessageRole, SearchResult};

use super::app::{App, Mode};

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
            draw_results(f, app, chunks[1]);
            draw_help_overlay(f, f.area());
        }
    }

    draw_status_bar(f, app, chunks[2]);
}

fn draw_search_input(f: &mut Frame, app: &App, area: Rect) {
    let input_value = app.input.value();
    let cursor_pos = app.input.visual_cursor();

    let input_display = Paragraph::new(input_value)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if app.mode == Mode::Normal {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }))
                .title(" Search (FTS5) "),
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
            "Type to search sessions. Press ? for help."
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
    _terms: &[String],
) -> ListItem<'a> {
    let branches: Vec<String> = serde_json::from_str(&result.git_branches).unwrap_or_default();

    // Line 1: cwd/path + metadata
    let primary = if !result.cwd.is_empty() {
        shorten_path(&result.cwd)
    } else {
        result.session_id.clone()
    };

    let mut meta_parts = Vec::new();
    if !branches.is_empty() {
        meta_parts.push(format!("@ {}", branches.join(", ")));
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

    let mut lines = vec![Line::from(vec![
        Span::styled(indicator, select_style),
        Span::styled(primary, header_style),
        Span::styled(meta_str, meta_style),
    ])];

    if !preview.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(preview, preview_style),
        ]));
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

    let title = if !result.cwd.is_empty() {
        format!(" {} ", shorten_path(&result.cwd))
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
    let (role_color, marker, role_label) = match msg.role {
        MessageRole::User => (Color::Magenta, "❯", "user".to_string()),
        MessageRole::Assistant => (Color::Blue, "●", "claude".to_string()),
        MessageRole::Summary => (Color::Yellow, "◆", "summary".to_string()),
        MessageRole::Teammate => {
            let label = if msg.teammate_id.is_empty() {
                "claude[teammate]".to_string()
            } else {
                format!("claude[{}]", msg.teammate_id)
            };
            (Color::Cyan, "●", label)
        }
    };

    let mut lines = Vec::new();

    // Role header: "● claude" or "❯ user" — bold, colored, like Claude Code
    let num_style = Style::default().fg(Color::DarkGray);
    let marker_style = Style::default().fg(role_color);
    let label_style = Style::default()
        .fg(role_color)
        .add_modifier(Modifier::BOLD);

    let mut header_spans = vec![
        Span::styled(format!("{:>3} ", msg.index + 1), num_style),
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(role_label, label_style),
    ];

    // Match indicator — subtle tag after the role label
    if is_match {
        header_spans.push(Span::styled(
            " ◀",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    lines.push(Line::from(header_spans));

    // Message body — indented, with term highlighting for matches
    let text = msg.text.trim();
    let text_lines: Vec<&str> = text.lines().collect();
    let show_count = text_lines.len().min(MAX_BODY_LINES);

    for text_line in &text_lines[..show_count] {
        let styled_line = if text_line.starts_with("$ ") {
            // Shell command — dim green, like a terminal prompt
            Line::from(vec![
                Span::raw(INDENT),
                Span::styled(
                    text_line.to_string(),
                    Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
                ),
            ])
        } else if text_line.starts_with('[') && text_line.ends_with(']') {
            // Tool use like [Read /path/to/file] — dim cyan
            Line::from(vec![
                Span::raw(INDENT),
                Span::styled(
                    text_line.to_string(),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ),
            ])
        } else if is_match && !terms.is_empty() {
            // Highlight matching terms in body text
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
                "{INDENT}… +{} more lines",
                text_lines.len() - MAX_BODY_LINES
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Blank line separator between messages
    lines.push(Line::from(""));

    lines
}

/// Highlight search terms in a single line of text.
fn highlight_line<'a>(text: &str, terms: &[String]) -> Line<'a> {
    let escaped: Vec<String> = terms.iter().map(|t| regex::escape(t)).collect();
    let pattern = match regex::RegexBuilder::new(&escaped.join("|"))
        .case_insensitive(true)
        .build()
    {
        Ok(re) => re,
        Err(_) => {
            return Line::from(vec![
                Span::raw(INDENT.to_string()),
                Span::styled(text.to_string(), Style::default().fg(Color::White)),
            ]);
        }
    };

    let mut spans = vec![Span::raw(INDENT.to_string())];
    let mut last = 0;
    for m in pattern.find_iter(text) {
        if m.start() > last {
            spans.push(Span::styled(
                text[last..m.start()].to_string(),
                Style::default().fg(Color::White),
            ));
        }
        spans.push(Span::styled(
            m.as_str().to_string(),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
        last = m.end();
    }
    if last < text.len() {
        spans.push(Span::styled(
            text[last..].to_string(),
            Style::default().fg(Color::White),
        ));
    }
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
        Mode::Normal => "Enter:resume  S-Enter:fork  Tab:detail  ?:help  Esc:quit",
        Mode::Detail => "Esc:back  n/N:matches  j/k:scroll  ?:help",
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
        Line::from("  ?             Toggle this help"),
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

/// Shorten a path for display: show last 2 components.
fn shorten_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        path.to_string()
    } else {
        format!(".../{}", parts[parts.len() - 2..].join("/"))
    }
}
