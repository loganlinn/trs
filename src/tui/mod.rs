//! TUI module -- interactive search interface using ratatui.

mod app;
mod event;
mod ui;

use std::io;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::config::{self, Config};
use crate::db;
use app::App;
use event::{Event, EventHandler};

pub use app::ExitAction;
pub use app::PinnedFilters;

/// Wrapper around [`Terminal`] with lifecycle management and guaranteed cleanup.
///
/// Renders to stderr so stdout remains available for piping.
struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    mouse: bool,
}

impl Tui {
    fn new(mouse: bool) -> Result<Self> {
        let backend = CrosstermBackend::new(io::stderr());
        let terminal = Terminal::new(backend).context("failed to initialize terminal")?;
        let mut tui = Self { terminal, mouse };
        tui.enter()?;
        Ok(tui)
    }

    fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(io::stderr(), EnterAlternateScreen)?;
        if self.mouse {
            execute!(io::stderr(), EnableMouseCapture)?;
        }

        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(io::stderr(), DisableMouseCapture, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            original_hook(info);
        }));

        Ok(())
    }

    fn exit(&mut self) -> Result<()> {
        if self.mouse {
            execute!(self.terminal.backend_mut(), DisableMouseCapture)?;
        }
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        disable_raw_mode()?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Deref for Tui {
    type Target = Terminal<CrosstermBackend<io::Stderr>>;
    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl DerefMut for Tui {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}

/// Run the interactive TUI search interface.
pub fn run(initial_input: &str, pinned: PinnedFilters) -> Result<Option<ExitAction>> {
    let db_path = config::default_db_path();

    // Auto-index before launching TUI
    if db_path.exists() {
        // Incremental index (errors are non-fatal for TUI launch)
        let _ = crate::indexer::run_index(&db_path, false, None);
    }

    let config = Config::load();
    let conn = db::open_db(&db_path, true)?;
    let mut tui = Tui::new(true)?;
    let mut app = App::new(conn, initial_input, pinned, config.keys);
    let events = EventHandler::new(Duration::from_millis(100));

    let result = run_loop(&mut tui, &mut app, &events);

    drop(tui);
    result?;
    Ok(app.exit_action.clone())
}

fn run_loop(tui: &mut Tui, app: &mut App, events: &EventHandler) -> Result<()> {
    loop {
        tui.draw(|f| ui::draw(f, app))?;

        match events
            .next()
            .map_err(|_| anyhow::anyhow!("event channel closed"))?
        {
            Event::Key(key) => {
                if let Some(msg) = app.handle_key(key) {
                    app.update(msg);
                }
            }
            Event::Mouse(mouse) => {
                if let Some(msg) = app.handle_mouse(mouse) {
                    app.update(msg);
                }
            }
            Event::Tick | Event::Resize(_, _) => {}
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
        App::new(conn, "", PinnedFilters::default(), Default::default())
    }

    fn make_result(id: &str) -> crate::session::SearchResult {
        crate::session::SearchResult {
            session_id: id.into(),
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
        }
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
        app.results.push(make_result("test-1"));
        app.table_state.select(Some(0));

        let msg = app.handle_key(key(KeyCode::Enter));
        assert!(matches!(msg, Some(Message::ResumeSession)));
    }

    #[test]
    fn test_enter_no_results_is_noop() {
        let mut app = test_app();
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
        app.table_state.select(Some(0));
        assert_eq!(app.selected_index(), 0);

        let msg = app.handle_key(key(KeyCode::Down));
        if let Some(m) = msg {
            app.update(m);
        }
        assert_eq!(app.selected_index(), 1);

        let msg = app.handle_key(key(KeyCode::Up));
        if let Some(m) = msg {
            app.update(m);
        }
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn test_clear_input() {
        let mut app = test_app();
        app.input = "hello".into();
        let msg = app.handle_key(key_ctrl(KeyCode::Char('u')));
        assert!(matches!(msg, Some(Message::ClearInput)));
        if let Some(m) = msg {
            app.update(m);
        }
        assert!(app.input.value().is_empty());
    }

    // --- Snapshot tests ---

    fn buffer_view(buf: &ratatui::buffer::Buffer) -> String {
        let w = buf.area.width as usize;
        let content = buf.content();
        let mut lines = Vec::new();
        for row in 0..buf.area.height as usize {
            let mut line = String::with_capacity(w);
            for col in 0..w {
                line.push_str(content[row * w + col].symbol());
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    #[test]
    fn snapshot_empty_state() {
        let mut app = test_app();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        insta::assert_snapshot!(buffer_view(terminal.backend().buffer()));
    }

    #[test]
    fn snapshot_with_results() {
        let mut app = test_app();
        for i in 0..5 {
            app.results.push(crate::session::SearchResult {
                session_id: format!("session-{i}"),
                source: "claude-code".into(),
                cwd: format!("/home/user/project-{i}"),
                slug: format!("project-{i}"),
                git_branches: r#"["main"]"#.into(),
                start_time: format!("2026-03-{:02}T10:00:00Z", i + 1),
                end_time: String::new(),
                files_touched: "[]".into(),
                tools_used: "[]".into(),
                message_count: (i + 1) * 3,
                first_message: format!("Help me with task {i}"),
                summary: String::new(),
                content_hash: None,
                custom_title: None,
                metadata: None,
                rank: 0.0,
            });
        }
        app.table_state.select(Some(0));
        app.status_message = "5 session(s)".into();

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        insta::assert_snapshot!(buffer_view(terminal.backend().buffer()));
    }

    #[test]
    fn snapshot_help_overlay() {
        let mut app = test_app();
        app.mode = Mode::Help;
        app.help_return_mode = Mode::Normal;

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        insta::assert_snapshot!(buffer_view(terminal.backend().buffer()));
    }
}
