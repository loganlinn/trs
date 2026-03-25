//! App state and update logic (TEA pattern).

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rusqlite::Connection;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use crate::db;
use crate::indexer;
use crate::search;
use crate::session::{App as SourceApp, Message as SessionMessage, SearchResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Browsing results with search input focused.
    Normal,
    /// Viewing a session's full conversation.
    Detail,
    /// Help overlay.
    Help,
}

#[derive(Debug, Clone)]
pub enum Message {
    Quit,
    Back,
    SearchChanged,
    ClearInput,
    SelectNext,
    SelectPrev,
    ScrollHalfDown,
    ScrollHalfUp,
    OpenDetail,
    ToggleHelp,
    FocusSearch,
    CopySessionId,
    CopyResumeCmd,
    DetailScrollDown,
    DetailScrollUp,
    DetailTop,
    DetailBottom,
    NextMatch,
    PrevMatch,
    ResumeSession,
    ForkSession,
}

/// Action to perform after exiting the TUI.
#[derive(Debug, Clone)]
pub enum ExitAction {
    Resume {
        session_id: String,
        cwd: String,
        source: String,
    },
    Fork {
        session_id: String,
        cwd: String,
        source: String,
    },
}

pub struct App {
    pub mode: Mode,
    pub input: Input,
    pub results: Vec<SearchResult>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub should_quit: bool,
    pub exit_action: Option<ExitAction>,
    pub status_message: String,

    // Detail view state
    pub detail_messages: Vec<SessionMessage>,
    pub detail_scroll: usize,
    pub detail_match_indices: Vec<usize>,
    pub detail_current_match: usize,

    // Help overlay state — mode to restore when help is dismissed
    pub help_return_mode: Mode,

    // Search state
    pub last_query: String,
    pub search_terms: Vec<String>,
    search_deadline: Option<Instant>,

    // Database connection
    conn: Connection,
}

impl App {
    pub fn new(conn: Connection) -> Self {
        let results = db::list_recent(&conn, 50, None).unwrap_or_default();
        let status_message = format!("{} session(s)", results.len());
        Self {
            mode: Mode::Normal,
            input: Input::default(),
            results,
            selected: 0,
            scroll_offset: 0,
            should_quit: false,
            exit_action: None,
            status_message,
            detail_messages: Vec::new(),
            detail_scroll: 0,
            detail_match_indices: Vec::new(),
            detail_current_match: 0,
            help_return_mode: Mode::Normal,
            last_query: String::new(),
            search_terms: Vec::new(),
            search_deadline: None,
            conn,
        }
    }

    /// Handle a key event and return an optional message.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Message> {
        match self.mode {
            Mode::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => Some(Message::Back),
                _ => None,
            },
            Mode::Detail => self.handle_detail_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Option<Message> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match (key.code, ctrl) {
            (KeyCode::Esc, _) => {
                if self.input.value().is_empty() {
                    Some(Message::Quit)
                } else {
                    Some(Message::ClearInput)
                }
            }
            (KeyCode::Char('q'), false) => {
                // 'q' quits only if input is empty (otherwise it's a search char)
                if self.input.value().is_empty() {
                    Some(Message::Quit)
                } else {
                    self.input.handle_event(&crossterm::event::Event::Key(key));
                    Some(Message::SearchChanged)
                }
            }
            (KeyCode::Char('c'), true) => Some(Message::Quit),
            (KeyCode::Char('u'), true) => Some(Message::ClearInput),
            (KeyCode::Char('n'), true) => Some(Message::SelectNext),
            (KeyCode::Char('p'), true) => Some(Message::SelectPrev),
            (KeyCode::Char('d'), true) => Some(Message::ScrollHalfDown),
            (KeyCode::Char('b'), true) => Some(Message::ScrollHalfUp),
            (KeyCode::Up, _) => Some(Message::SelectPrev),
            (KeyCode::Down, _) => Some(Message::SelectNext),
            (KeyCode::Enter, _) => {
                if !self.results.is_empty() {
                    if shift {
                        Some(Message::ForkSession)
                    } else {
                        Some(Message::ResumeSession)
                    }
                } else {
                    None
                }
            }
            (KeyCode::Tab, _) => {
                if !self.results.is_empty() {
                    Some(Message::OpenDetail)
                } else {
                    None
                }
            }
            (KeyCode::Char('?'), false) if self.input.value().is_empty() => {
                Some(Message::ToggleHelp)
            }
            (KeyCode::Char('y'), false) if self.input.value().is_empty() => {
                Some(Message::CopySessionId)
            }
            (KeyCode::Char('r'), false) if self.input.value().is_empty() => {
                Some(Message::CopyResumeCmd)
            }
            _ => {
                // Forward to input widget
                self.input.handle_event(&crossterm::event::Event::Key(key));
                Some(Message::SearchChanged)
            }
        }
    }

    fn handle_detail_key(&self, key: KeyEvent) -> Option<Message> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (key.code, ctrl) {
            (KeyCode::Esc | KeyCode::Char('q'), _) => Some(Message::Back),
            (KeyCode::Char('c'), true) => Some(Message::Quit),
            (KeyCode::Char('/'), false) => Some(Message::FocusSearch),
            (KeyCode::Down | KeyCode::Char('j'), false) => Some(Message::DetailScrollDown),
            (KeyCode::Up | KeyCode::Char('k'), false) => Some(Message::DetailScrollUp),
            (KeyCode::Char('d'), true) => Some(Message::ScrollHalfDown),
            (KeyCode::Char('b'), true) => Some(Message::ScrollHalfUp),
            (KeyCode::Char('g'), false) => Some(Message::DetailTop),
            (KeyCode::Char('G'), false) => Some(Message::DetailBottom),
            (KeyCode::Char('n'), false) => Some(Message::NextMatch),
            (KeyCode::Char('N'), false) => Some(Message::PrevMatch),
            (KeyCode::Char('y'), false) => Some(Message::CopySessionId),
            (KeyCode::Char('r'), false) => Some(Message::CopyResumeCmd),
            (KeyCode::Char('?'), false) => Some(Message::ToggleHelp),
            _ => None,
        }
    }

    /// Update state in response to a message.
    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::Quit => {
                self.should_quit = true;
            }
            Message::Back => match self.mode {
                Mode::Help => {
                    self.mode = self.help_return_mode.clone();
                }
                Mode::Detail => {
                    self.mode = Mode::Normal;
                    self.detail_messages.clear();
                    self.detail_scroll = 0;
                }
                Mode::Normal => {}
            },
            Message::SearchChanged => {
                self.search_deadline =
                    Some(Instant::now() + std::time::Duration::from_millis(150));
            }
            Message::ClearInput => {
                self.search_deadline = None;
                self.input = Input::default();
                self.load_recent();
            }
            Message::SelectNext => {
                if !self.results.is_empty() {
                    self.selected = (self.selected + 1).min(self.results.len() - 1);
                    self.ensure_selected_visible();
                }
            }
            Message::SelectPrev => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_selected_visible();
            }
            Message::ScrollHalfDown => {
                if self.mode == Mode::Detail {
                    self.detail_scroll = self
                        .detail_scroll
                        .saturating_add(10)
                        .min(self.detail_max_scroll());
                } else {
                    let half = 10;
                    self.selected = (self.selected + half).min(if self.results.is_empty() {
                        0
                    } else {
                        self.results.len() - 1
                    });
                    self.ensure_selected_visible();
                }
            }
            Message::ScrollHalfUp => {
                if self.mode == Mode::Detail {
                    self.detail_scroll = self.detail_scroll.saturating_sub(10);
                } else {
                    self.selected = self.selected.saturating_sub(10);
                    self.ensure_selected_visible();
                }
            }
            Message::OpenDetail => {
                self.open_detail();
            }
            Message::ToggleHelp => {
                if self.mode == Mode::Help {
                    self.mode = self.help_return_mode.clone();
                } else {
                    self.help_return_mode = self.mode.clone();
                    self.mode = Mode::Help;
                }
            }
            Message::FocusSearch => {
                self.mode = Mode::Normal;
            }
            Message::CopySessionId => {
                if let Some(result) = self.results.get(self.selected) {
                    self.status_message = format!("Session ID: {}", result.session_id);
                }
            }
            Message::CopyResumeCmd => {
                if let Some(result) = self.results.get(self.selected) {
                    let app = SourceApp::parse(&result.source).unwrap_or(SourceApp::ClaudeCode);
                    let cmd = app.resume_cmd(&result.session_id);
                    self.status_message = format!("Resume: {cmd}");
                }
            }
            Message::DetailScrollDown => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add(1)
                    .min(self.detail_max_scroll());
            }
            Message::DetailScrollUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
            Message::DetailTop => {
                self.detail_scroll = 0;
            }
            Message::DetailBottom => {
                self.detail_scroll = self.detail_max_scroll();
            }
            Message::NextMatch => {
                if !self.detail_match_indices.is_empty() {
                    self.detail_current_match =
                        (self.detail_current_match + 1) % self.detail_match_indices.len();
                    self.scroll_to_current_match();
                }
            }
            Message::PrevMatch => {
                if !self.detail_match_indices.is_empty() {
                    if self.detail_current_match == 0 {
                        self.detail_current_match = self.detail_match_indices.len() - 1;
                    } else {
                        self.detail_current_match -= 1;
                    }
                    self.scroll_to_current_match();
                }
            }
            Message::ResumeSession => {
                if let Some(result) = self.results.get(self.selected) {
                    self.exit_action = Some(ExitAction::Resume {
                        session_id: result.session_id.clone(),
                        cwd: result.cwd.clone(),
                        source: result.source.clone(),
                    });
                    self.should_quit = true;
                }
            }
            Message::ForkSession => {
                if let Some(result) = self.results.get(self.selected) {
                    self.exit_action = Some(ExitAction::Fork {
                        session_id: result.session_id.clone(),
                        cwd: result.cwd.clone(),
                        source: result.source.clone(),
                    });
                    self.should_quit = true;
                }
            }
        }
    }

    fn load_recent(&mut self) {
        self.results = db::list_recent(&self.conn, 50, None).unwrap_or_default();
        self.status_message = format!("{} session(s)", self.results.len());
        self.selected = 0;
        self.scroll_offset = 0;
        self.last_query.clear();
        self.search_terms.clear();
    }

    fn perform_search(&mut self) {
        let query_str = self.input.value().to_string();
        if query_str.is_empty() {
            self.load_recent();
            return;
        }

        if query_str == self.last_query {
            return;
        }

        let parsed = search::parse_query(&query_str);

        // If only filters and no text, list recent with filters
        if parsed.text.is_empty() {
            let source = parsed.source_filter();
            match db::list_recent(&self.conn, 50, source) {
                Ok(rows) => {
                    self.search_terms.clear();
                    self.status_message = format!("{} session(s)", rows.len());
                    self.results = rows;
                    self.selected = 0;
                    self.scroll_offset = 0;
                }
                Err(_) => {
                    self.status_message = "Error".to_string();
                }
            }
            self.last_query = query_str;
            return;
        }

        let normalized = db::prefix_query(&db::normalize_fts_query(&parsed.text));
        let source = parsed.source_filter();
        match db::search(
            &self.conn,
            &normalized,
            parsed.file.as_deref(),
            parsed.branch.as_deref(),
            parsed.project.as_deref(),
            source,
            50,
        ) {
            Ok(rows) => {
                self.search_terms = search::query_terms(&parsed.text);
                self.status_message = format!("{} result(s)", rows.len());
                self.results = rows;
                self.selected = 0;
                self.scroll_offset = 0;
            }
            Err(_) => {
                self.status_message = "Invalid query".to_string();
                self.results.clear();
                self.selected = 0;
            }
        }
        self.last_query = query_str;
    }

    fn open_detail(&mut self) {
        let result = match self.results.get(self.selected) {
            Some(r) => r,
            None => return,
        };

        if let Some((app, path)) = search::session_jsonl_path(&result.session_id, &result.slug, &result.source) {
            match indexer::extract_messages_for(&path, &app) {
                Ok(msgs) => {
                    // Find match indices
                    self.detail_match_indices = msgs
                        .iter()
                        .filter(|m| search::message_matches(m, &self.search_terms))
                        .map(|m| m.index)
                        .collect();
                    self.detail_messages = msgs;
                    self.detail_scroll = 0;
                    self.detail_current_match = 0;
                    self.mode = Mode::Detail;

                    // Auto-scroll to first match
                    if !self.detail_match_indices.is_empty() {
                        self.scroll_to_current_match();
                    }
                }
                Err(e) => {
                    self.status_message = format!("Error loading session: {e}");
                }
            }
        } else {
            self.status_message = "Session file not found".to_string();
        }
    }

    fn detail_max_scroll(&self) -> usize {
        self.detail_messages.len().saturating_sub(1)
    }

    fn scroll_to_current_match(&mut self) {
        if let Some(&msg_idx) = self.detail_match_indices.get(self.detail_current_match) {
            // Find the position of this message in the messages vec
            if let Some(pos) = self.detail_messages.iter().position(|m| m.index == msg_idx) {
                self.detail_scroll = pos.saturating_sub(2);
            }
        }
    }

    fn ensure_selected_visible(&mut self) {
        // Keep selected item within a visible window.
        // The actual visible height depends on terminal size, but we use
        // a reasonable default and let the UI clamp as needed.
        let visible_height = 20;
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected - visible_height + 1;
        }
    }

    /// Fire pending debounced search if deadline has passed.
    pub fn tick(&mut self) {
        if let Some(deadline) = self.search_deadline {
            if Instant::now() >= deadline {
                self.search_deadline = None;
                self.perform_search();
            }
        }
    }

    /// Get the currently selected result (if any).
    pub fn selected_result(&self) -> Option<&SearchResult> {
        self.results.get(self.selected)
    }
}
