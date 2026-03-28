//! TUI module -- interactive search interface using ratatui.

mod app;
mod event;
mod ui;

use std::io::{self, stdout};

use anyhow::{Context, Result};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::config;
use crate::db;
use app::App;

pub use app::ExitAction;
pub use app::PinnedFilters;

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        original_hook(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout())).context("failed to initialize terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Run the interactive TUI search interface.
pub fn run(initial_input: &str, pinned: PinnedFilters) -> Result<Option<ExitAction>> {
    let db_path = config::default_db_path();

    // Auto-index before launching TUI
    if db_path.exists() {
        // Incremental index (errors are non-fatal for TUI launch)
        let _ = crate::indexer::run_index(&db_path, false, None);
    }

    let conn = db::open_db(&db_path, true)?;
    let mut terminal = init_terminal()?;
    let mut app = App::new(conn, initial_input, pinned);

    let result = run_loop(&mut terminal, &mut app);

    restore_terminal(&mut terminal)?;
    result?;
    Ok(app.exit_action.clone())
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if let Some(msg) = event::handle_event(app)? {
            app.update(msg);
        }

        app.tick();

        if app.should_quit {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::app::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn test_app() -> App {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        App::new(conn, "", PinnedFilters::default())
    }

    #[test]
    fn test_quit_from_normal_mode() {
        let mut app = test_app();
        assert!(!app.should_quit);
        let msg = app.handle_key(key(KeyCode::Esc));
        assert!(matches!(msg, Some(Message::Quit)));
        if let Some(m) = msg {
            app.update(m);
        }
        assert!(app.should_quit);
    }

    #[test]
    fn test_enter_produces_resume_message() {
        let mut app = test_app();
        // Insert a fake result so Enter has something to act on
        app.results.push(crate::session::SearchResult {
            session_id: "test-1".into(),
            source: "claude-code".into(),
            cwd: "/tmp".into(),
            slug: "proj".into(),
            git_branches: "[]".into(),
            start_time: String::new(),
            end_time: String::new(),
            files_touched: "[]".into(),
            tools_used: "[]".into(),
            message_count: 2,
            first_message: "hello".into(),
            summary: String::new(),
            content_hash: None,
            custom_title: None,
            metadata: None,
            rank: 0.0,
        });

        let msg = app.handle_key(key(KeyCode::Enter));
        assert!(matches!(msg, Some(Message::ResumeSession)));
    }

    #[test]
    fn test_enter_no_results_is_noop() {
        let mut app = test_app();
        // No results: Enter should produce None
        let msg = app.handle_key(key(KeyCode::Enter));
        assert!(msg.is_none());
    }

    #[test]
    fn test_back_from_detail() {
        let mut app = test_app();
        app.mode = Mode::Detail;
        let msg = app.handle_key(key(KeyCode::Esc));
        assert!(matches!(msg, Some(Message::Back)));
        if let Some(m) = msg {
            app.update(m);
        }
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn test_help_toggle() {
        let mut app = test_app();
        let msg = app.handle_key(key_ctrl(KeyCode::Char('/')));
        assert!(matches!(msg, Some(Message::ToggleHelp)));
        if let Some(m) = msg {
            app.update(m);
        }
        assert!(matches!(app.mode, Mode::Help));

        let msg = app.handle_key(key(KeyCode::Esc));
        assert!(matches!(msg, Some(Message::Back)));
        if let Some(m) = msg {
            app.update(m);
        }
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn test_navigate_down_up() {
        let mut app = test_app();
        // Add two results
        for i in 0..2 {
            app.results.push(crate::session::SearchResult {
                session_id: format!("s{i}"),
                source: String::new(),
                cwd: String::new(),
                slug: String::new(),
                git_branches: "[]".into(),
                start_time: String::new(),
                end_time: String::new(),
                files_touched: "[]".into(),
                tools_used: "[]".into(),
                message_count: 0,
                first_message: String::new(),
                summary: String::new(),
                content_hash: None,
                custom_title: None,
                metadata: None,
                rank: 0.0,
            });
        }
        assert_eq!(app.selected, 0);

        let msg = app.handle_key(key(KeyCode::Down));
        if let Some(m) = msg {
            app.update(m);
        }
        assert_eq!(app.selected, 1);

        let msg = app.handle_key(key(KeyCode::Up));
        if let Some(m) = msg {
            app.update(m);
        }
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn test_clear_input() {
        let mut app = test_app();
        // Type something
        app.input = "hello".into();
        let msg = app.handle_key(key_ctrl(KeyCode::Char('u')));
        assert!(matches!(msg, Some(Message::ClearInput)));
        if let Some(m) = msg {
            app.update(m);
        }
        assert!(app.input.value().is_empty());
    }
}
