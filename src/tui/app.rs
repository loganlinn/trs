//! App state and update logic (TEA pattern).

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::widgets::TableState;
use rusqlite::Connection;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use crate::db;
use crate::display;
use crate::indexer;
use crate::keys::KeyBindings;
use crate::search;
use crate::session::{Message as SessionMessage, SearchResult};

/// Filters pinned via CLI flags — always applied, independent of search input.
#[derive(Debug, Clone, Default)]
pub struct PinnedFilters {
    pub branch: Option<String>,
    pub project: Option<String>,
}

impl PinnedFilters {
    pub fn is_empty(&self) -> bool {
        self.branch.is_none() && self.project.is_none()
    }

    /// Format pinned filters for display in the search box title.
    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref p) = self.project {
            parts.push(format!("project:{p}"));
        }
        if let Some(ref b) = self.branch {
            parts.push(format!("branch:{b}"));
        }
        parts.join(" ")
    }
}

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
    pub table_state: TableState,
    pub should_quit: bool,
    pub exit_action: Option<ExitAction>,
    pub status_message: String,

    // Preview pane state
    pub preview_snippets: Vec<display::MessageSnippet>,
    pub preview_scroll: usize,
    preview_session_id: String,

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

    /// CLI-pinned filters, always applied on top of search input.
    pub pinned: PinnedFilters,

    /// Configurable key bindings.
    pub keys: KeyBindings,

    // Database connection
    conn: Connection,
}

impl App {
    pub fn new(conn: Connection, initial_input: &str, pinned: PinnedFilters, keys: KeyBindings) -> Self {
        let initial_filter = pinned_to_filter(&pinned);
        let results = db::list_recent(&conn, 50, &initial_filter).unwrap_or_default();
        let status_message = format!("{} session(s)", results.len());
        let mut table_state = TableState::default();
        if !results.is_empty() {
            table_state.select(Some(0));
        }
        let mut app = Self {
            mode: Mode::Normal,
            input: Input::from(initial_input),
            results,
            table_state,
            should_quit: false,
            exit_action: None,
            status_message,
            preview_snippets: Vec::new(),
            preview_scroll: 0,
            preview_session_id: String::new(),
            detail_messages: Vec::new(),
            detail_scroll: 0,
            detail_match_indices: Vec::new(),
            detail_current_match: 0,
            help_return_mode: Mode::Normal,
            last_query: String::new(),
            search_terms: Vec::new(),
            search_deadline: None,
            pinned,
            keys,
            conn,
        };
        if !initial_input.is_empty() {
            app.perform_search();
        }
        app.load_preview();
        app
    }

    /// Index of the currently selected result (0 if nothing selected).
    pub fn selected_index(&self) -> usize {
        self.table_state.selected().unwrap_or(0)
    }

    /// Handle a key event and return an optional message.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Message> {
        match self.mode {
            Mode::Help => {
                if self.keys.help.close.matches(&key) {
                    Some(Message::Back)
                } else {
                    None
                }
            }
            Mode::Detail => self.handle_detail_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    /// Handle a mouse event and return an optional message.
    pub fn handle_mouse(&self, mouse: MouseEvent) -> Option<Message> {
        match mouse.kind {
            MouseEventKind::ScrollDown => match self.mode {
                Mode::Normal => Some(Message::SelectNext),
                Mode::Detail => Some(Message::DetailScrollDown),
                Mode::Help => None,
            },
            MouseEventKind::ScrollUp => match self.mode {
                Mode::Normal => Some(Message::SelectPrev),
                Mode::Detail => Some(Message::DetailScrollUp),
                Mode::Help => None,
            },
            _ => None,
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Option<Message> {
        let keys = &self.keys.normal;

        // Esc: clear input if any, else quit (universal convention, not configurable)
        if key.code == KeyCode::Esc {
            return if self.input.value().is_empty() {
                Some(Message::Quit)
            } else {
                Some(Message::ClearInput)
            };
        }

        if keys.quit.matches(&key) {
            return Some(Message::Quit);
        }
        if keys.clear_input.matches(&key) {
            return Some(Message::ClearInput);
        }
        if keys.select_next.matches(&key) {
            return Some(Message::SelectNext);
        }
        if keys.select_prev.matches(&key) {
            return Some(Message::SelectPrev);
        }
        if keys.scroll_half_down.matches(&key) {
            return Some(Message::ScrollHalfDown);
        }
        if keys.scroll_half_up.matches(&key) {
            return Some(Message::ScrollHalfUp);
        }
        if keys.resume_session.matches(&key) {
            return if !self.results.is_empty() {
                Some(Message::ResumeSession)
            } else {
                None
            };
        }
        if keys.fork_session.matches(&key) {
            return if !self.results.is_empty() {
                Some(Message::ForkSession)
            } else {
                None
            };
        }
        if keys.open_detail.matches(&key) {
            return if !self.results.is_empty() {
                Some(Message::OpenDetail)
            } else {
                None
            };
        }
        if keys.toggle_help.matches(&key) {
            return Some(Message::ToggleHelp);
        }
        if keys.copy_session_id.matches(&key) {
            return Some(Message::CopySessionId);
        }

        // Forward everything else to input widget
        self.input
            .handle_event(&crossterm::event::Event::Key(key));
        Some(Message::SearchChanged)
    }

    fn handle_detail_key(&self, key: KeyEvent) -> Option<Message> {
        let keys = &self.keys.detail;

        if keys.back.matches(&key) {
            Some(Message::Back)
        } else if keys.quit.matches(&key) {
            Some(Message::Quit)
        } else if keys.focus_search.matches(&key) {
            Some(Message::FocusSearch)
        } else if keys.scroll_down.matches(&key) {
            Some(Message::DetailScrollDown)
        } else if keys.scroll_up.matches(&key) {
            Some(Message::DetailScrollUp)
        } else if keys.scroll_half_down.matches(&key) {
            Some(Message::ScrollHalfDown)
        } else if keys.scroll_half_up.matches(&key) {
            Some(Message::ScrollHalfUp)
        } else if keys.top.matches(&key) {
            Some(Message::DetailTop)
        } else if keys.bottom.matches(&key) {
            Some(Message::DetailBottom)
        } else if keys.next_match.matches(&key) {
            Some(Message::NextMatch)
        } else if keys.prev_match.matches(&key) {
            Some(Message::PrevMatch)
        } else if keys.copy_session_id.matches(&key) {
            Some(Message::CopySessionId)
        } else if keys.toggle_help.matches(&key) {
            Some(Message::ToggleHelp)
        } else {
            None
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
                    let i = self.selected_index();
                    self.table_state
                        .select(Some((i + 1).min(self.results.len() - 1)));
                    self.load_preview();
                }
            }
            Message::SelectPrev => {
                let i = self.selected_index();
                self.table_state.select(Some(i.saturating_sub(1)));
                self.load_preview();
            }
            Message::ScrollHalfDown => {
                if self.mode == Mode::Detail {
                    self.detail_scroll = self
                        .detail_scroll
                        .saturating_add(10)
                        .min(self.detail_max_scroll());
                } else {
                    let half = 10;
                    let max = if self.results.is_empty() {
                        0
                    } else {
                        self.results.len() - 1
                    };
                    self.table_state
                        .select(Some((self.selected_index() + half).min(max)));
                    self.load_preview();
                }
            }
            Message::ScrollHalfUp => {
                if self.mode == Mode::Detail {
                    self.detail_scroll = self.detail_scroll.saturating_sub(10);
                } else {
                    self.table_state
                        .select(Some(self.selected_index().saturating_sub(10)));
                    self.load_preview();
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
                if let Some(result) = self.results.get(self.selected_index()) {
                    self.status_message = format!("Session ID: {}", result.session_id);
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
                if let Some(result) = self.results.get(self.selected_index()) {
                    self.exit_action = Some(ExitAction::Resume {
                        session_id: result.session_id.clone(),
                        cwd: result.cwd.clone(),
                        source: result.source.clone(),
                    });
                    self.should_quit = true;
                }
            }
            Message::ForkSession => {
                if let Some(result) = self.results.get(self.selected_index()) {
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
        let filter = pinned_to_filter(&self.pinned);
        self.results = db::list_recent(&self.conn, 50, &filter).unwrap_or_default();
        self.status_message = format!("{} session(s)", self.results.len());
        self.table_state.select(if self.results.is_empty() {
            None
        } else {
            Some(0)
        });
        self.last_query.clear();
        self.search_terms.clear();
        self.invalidate_preview();
        self.load_preview();
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

        // Merge: inline filters override pinned filters
        let branch_pat = parsed.branch.as_deref().or(self.pinned.branch.as_deref());
        let project_pat = parsed.project.as_deref().or(self.pinned.project.as_deref());

        // Direct session ID lookup when input is a UUID
        if search::is_uuid(&parsed.text) {
            match db::lookup_by_id(&self.conn, parsed.text.trim()) {
                Ok(Some(result)) => {
                    self.search_terms.clear();
                    self.status_message = "1 result (ID match)".to_string();
                    self.results = vec![result];
                    self.table_state.select(Some(0));
                }
                Ok(None) => {
                    self.search_terms.clear();
                    self.status_message = "No session with that ID".to_string();
                    self.results.clear();
                    self.table_state.select(None);
                }
                Err(_) => {
                    self.status_message = "Lookup error".to_string();
                }
            }
            self.last_query = query_str;
            self.invalidate_preview();
            self.load_preview();
            return;
        }

        // If only filters and no text, list recent with filters
        if parsed.text.is_empty() {
            let filter = db::SearchFilter {
                file_pat: parsed.file.as_deref(),
                branch_pat,
                project_pat,
                source: parsed.source_filter(),
                date: parsed.date.as_ref(),
            };
            match db::list_recent(&self.conn, 50, &filter) {
                Ok(rows) => {
                    self.search_terms.clear();
                    self.status_message = format!("{} session(s)", rows.len());
                    self.results = rows;
                    self.table_state.select(if self.results.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                }
                Err(_) => {
                    self.status_message = "Error".to_string();
                }
            }
            self.last_query = query_str;
            self.invalidate_preview();
            self.load_preview();
            return;
        }

        let normalized = db::prefix_query(&db::normalize_fts_query(&parsed.text));
        let filter = db::SearchFilter {
            file_pat: parsed.file.as_deref(),
            branch_pat,
            project_pat,
            source: parsed.source_filter(),
            date: parsed.date.as_ref(),
        };
        match db::search(&self.conn, &normalized, &filter, 50) {
            Ok(rows) => {
                self.search_terms = search::query_terms(&parsed.text);
                self.status_message = format!("{} result(s)", rows.len());
                self.results = rows;
                self.table_state.select(if self.results.is_empty() {
                    None
                } else {
                    Some(0)
                });
            }
            Err(_) => {
                self.status_message = "Invalid query".to_string();
                self.results.clear();
                self.table_state.select(None);
            }
        }
        self.last_query = query_str;
        self.invalidate_preview();
        self.load_preview();
    }

    fn invalidate_preview(&mut self) {
        self.preview_session_id.clear();
        self.preview_snippets.clear();
        self.preview_scroll = 0;
    }

    /// Load preview snippets for the currently selected result.
    fn load_preview(&mut self) {
        let result = match self.results.get(self.selected_index()) {
            Some(r) => r,
            None => {
                self.preview_snippets.clear();
                self.preview_scroll = 0;
                return;
            }
        };

        // Skip if already loaded for this session
        if result.session_id == self.preview_session_id {
            return;
        }
        self.preview_session_id = result.session_id.clone();
        self.preview_scroll = 0;

        // Only load snippets from JSONL when we have search terms
        if self.search_terms.is_empty() {
            self.preview_snippets.clear();
            return;
        }

        if let Some((app, path)) =
            search::session_jsonl_path(&result.session_id, &result.slug, &result.source)
        {
            if let Ok(msgs) = indexer::extract_messages_for(&path, &app) {
                let rd = display::prepare_result(result, &msgs, &self.search_terms, 0, 0);
                self.preview_snippets = rd.snippets;
                return;
            }
        }
        self.preview_snippets.clear();
    }

    fn open_detail(&mut self) {
        let result = match self.results.get(self.selected_index()) {
            Some(r) => r,
            None => return,
        };

        if let Some((app, path)) =
            search::session_jsonl_path(&result.session_id, &result.slug, &result.source)
        {
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

    /// Get the currently selected result (if any).
    pub fn selected_result(&self) -> Option<&SearchResult> {
        self.results.get(self.selected_index())
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
}

/// Build a `SearchFilter` from pinned filters (borrows from the struct).
fn pinned_to_filter(pinned: &PinnedFilters) -> db::SearchFilter<'_> {
    db::SearchFilter {
        branch_pat: pinned.branch.as_deref(),
        project_pat: pinned.project.as_deref(),
        ..Default::default()
    }
}
