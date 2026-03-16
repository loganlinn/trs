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
            .fg(Color::Cyan)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
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

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_bottom(Line::from(match_info).right_aligned()),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn message_to_lines<'a>(
    msg: &crate::session::Message,
    is_match: bool,
    _terms: &[String],
) -> Vec<Line<'a>> {
    let role_color = match msg.role {
        MessageRole::User => Color::Green,
        MessageRole::Assistant => Color::Blue,
        MessageRole::Summary => Color::Yellow,
        MessageRole::Teammate => Color::Cyan,
    };

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

    let mut lines = Vec::new();

    // Separator / header line
    let header_style = if is_match {
        Style::default().fg(role_color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(role_color)
    };

    let match_marker = if is_match { "*" } else { " " };

    lines.push(Line::from(vec![
        Span::styled(
            format!("{match_marker}{:>3} ", msg.index + 1),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(role_label, header_style),
    ]));

    // Message text lines
    let text_style = if is_match {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };

    for text_line in msg.text.lines().take(30) {
        lines.push(Line::from(Span::styled(
            format!("      {text_line}"),
            text_style,
        )));
    }

    if msg.text.lines().count() > 30 {
        lines.push(Line::from(Span::styled(
            format!("      ... (+{} lines)", msg.text.lines().count() - 30),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Blank line between messages
    lines.push(Line::from(""));

    lines
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
        Mode::Normal => "Enter:open  ?:help  Esc:quit",
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
    let help_height = 22u16;
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
        Line::from("  Enter         Open session detail"),
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
