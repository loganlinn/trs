# Rust TUI/CLI Ecosystem Research

Research for porting `trs` (transcript search) to Rust with ratatui-based TUI.

## Recommended Crates with Versions

### Core Dependencies

| Crate         | Version | Purpose          | Features/Notes                                           |
| ------------- | ------- | ---------------- | -------------------------------------------------------- |
| `ratatui`     | 0.30.0  | TUI framework    | `crossterm` backend (default)                            |
| `crossterm`   | 0.29.0  | Terminal backend | Re-exported via `ratatui::crossterm` since ratatui 0.28+ |
| `rusqlite`    | 0.38.0  | SQLite bindings  | Features: `bundled`, `fts5`                              |
| `clap`        | 4.6.0   | CLI arg parsing  | Feature: `derive`                                        |
| `serde`       | 1.0.228 | Serialization    | Feature: `derive`                                        |
| `serde_json`  | 1.0.149 | JSON parsing     | For JSONL session files                                  |
| `toml`        | 1.0.6   | TOML config      | For config file parsing                                  |
| `directories` | 6.0.0   | XDG paths        | `ProjectDirs` for config/data/cache                      |
| `anyhow`      | 1.0.102 | Error handling   | Application-level errors                                 |
| `thiserror`   | 2.0.18  | Error handling   | Library-level typed errors                               |
| `indicatif`   | 0.18.4  | Progress bars    | For indexing in non-TUI mode                             |
| `is-terminal` | 0.4.17  | TTY detection    | Decide TUI vs piped output                               |
| `regex`       | 1.12.3  | Regex support    | Query pattern matching                                   |
| `sha2`        | 0.10.9  | Content hashing  | File change detection for indexing                       |
| `chrono`      | 0.4.44  | Date/time        | Feature: `serde`                                         |

### TUI Input Widgets

| Crate              | Version | Notes                                                       |
| ------------------ | ------- | ----------------------------------------------------------- |
| `tui-input`        | 0.15.0  | Lightweight single-line input; good for search bar          |
| `ratatui-textarea` | 0.8.0   | Multi-line text editor widget (successor to `tui-textarea`) |

**Recommendation:** Use `tui-input` for the search bar -- it's simpler and purpose-built for single-line input. `ratatui-textarea` is overkill for a search field.

### Optional / Consider

| Crate            | Version | Purpose                                          |
| ---------------- | ------- | ------------------------------------------------ |
| `tokio`          | 1.50.0  | Async runtime (if needed for async event loop)   |
| `color-eyre`     | latest  | Enhanced error reporting with colored backtraces |
| `tui-scrollview` | latest  | Scrollable content areas for result preview      |

**Note on async:** Many ratatui apps use synchronous event loops with `crossterm::event::poll()` for simplicity. Async (tokio) is useful when you need background tasks (e.g., live search while typing). For trs, a sync event loop with periodic polling is likely sufficient since searches hit a local SQLite FTS5 index and return quickly.

## Project Structure Template

```
trs/
  Cargo.toml
  mise.toml
  src/
    main.rs           # Entry point: parse args, dispatch to subcommands
    cli.rs            # clap derive structs for CLI interface
    config.rs         # Config file loading (TOML), XDG paths
    db.rs             # SQLite/FTS5 schema, connection, queries
    indexer.rs        # JSONL parser, session indexing, content hashing
    search.rs         # Search query execution, result formatting
    output.rs         # Non-TUI output formatting (ripgrep-style)
    error.rs          # thiserror error types
    tui/
      mod.rs          # TUI module root: terminal init/restore, main loop
      app.rs          # App state, event->action dispatch, update logic
      event.rs        # Event enum (Key, Tick, Quit, Resize)
      action.rs       # Action enum (Search, SelectResult, Scroll, Quit, etc.)
      ui.rs           # Layout and rendering (draws widgets from app state)
      input.rs        # Search input widget wrapper
      results.rs      # Search results list widget
      preview.rs      # Result detail/preview widget
```

### Key Design Decisions

- **Flat module structure** for non-TUI code (db, indexer, search) -- keeps it simple
- **Nested `tui/` module** groups all interactive UI code
- **Separate `cli.rs`** keeps clap definitions clean and testable
- **`output.rs`** handles non-interactive (piped) output separately from TUI rendering

## Architectural Patterns

### 1. The Elm Architecture (TEA) -- Recommended

The ratatui documentation recommends TEA as the primary pattern. It maps well to trs:

```
Event -> Message -> Update(state) -> View(state) -> render
```

**Core loop:**

```rust
loop {
    terminal.draw(|f| app.view(f))?;

    if let Some(msg) = app.handle_event()? {
        if app.update(msg) == ShouldQuit::Yes {
            break;
        }
    }
}
```

**Why TEA over Component Architecture:** trs has a simple, single-screen UI (search bar + results list + preview). Component architecture adds indirection without benefit for this scale. TEA keeps state management explicit and centralized.

### 2. Event Handling

Use crossterm's synchronous polling model:

```rust
fn handle_event(&self) -> Result<Option<Message>> {
    if crossterm::event::poll(Duration::from_millis(100))? {
        match crossterm::event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                Ok(self.handle_key(key))
            }
            Event::Resize(w, h) => Ok(Some(Message::Resize(w, h))),
            _ => Ok(None),
        }
    } else {
        Ok(None) // tick
    }
}
```

### 3. App State

Centralized state struct:

```rust
struct App {
    mode: Mode,              // Normal, Search, Help
    query: Input,            // tui-input state
    results: Vec<SearchResult>,
    selected: usize,
    scroll_offset: usize,
    db: Database,
    should_quit: bool,
}

enum Mode {
    Normal,    // browsing results
    Search,    // typing in search bar
    Help,      // help overlay
}
```

### 4. Terminal Init/Restore with Panic Hook

Critical pattern -- always restore terminal on panic:

```rust
fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    // Restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = stdout().execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        original_hook(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout()))
        .context("failed to initialize terminal")
}
```

### 5. Dual-Mode Output

The app needs to work both interactively (TUI) and non-interactively (piped):

```rust
fn main() -> Result<()> {
    let args = Cli::parse();

    match args.command {
        Command::Search { query, interactive } => {
            let db = Database::open(&config)?;

            if interactive.unwrap_or_else(|| is_terminal::is_terminal(&stdout())) {
                tui::run(db, query)?;
            } else {
                // ripgrep-style line output
                let results = db.search(&query)?;
                output::print_results(&results, &mut stdout())?;
            }
        }
        Command::Index { .. } => { /* ... */ }
    }
    Ok(())
}
```

## Exemplary Projects

### 1. gitui (extrawurst/gitui)

**Architecture:** Multi-crate workspace separating async git operations (`asyncgit/`) from TUI (`src/`). Components are organized by feature (diff, commit, stash, etc.).

**Key patterns:**

- Async git operations run on background threads, results sent via channels
- Component-based UI with a `Component` trait
- Keybinding configuration loaded from files
- Theme system for customizable colors

**Relevance to trs:** The separation of data operations from UI is a good model. However, gitui's scale (many views, complex state) is much larger than trs needs.

**Link:** https://github.com/extrawurst/gitui

### 2. television (alexpasmantier/television)

**Architecture:** Single-crate with well-organized modules:

- `action.rs` / `event.rs` -- message types
- `app.rs` -- main application logic
- `draw.rs` / `render.rs` / `screen/` -- UI rendering
- `input.rs` / `keymap.rs` / `mouse.rs` -- input handling
- `channels/` -- data source abstraction
- `matcher/` -- fuzzy matching (uses nucleo)
- `previewer/` -- result preview rendering
- `config/` -- TOML configuration

**Key patterns:**

- Channel abstraction for pluggable data sources
- Separation of draw/render concerns
- TOML-based configuration with themes
- Uses tokio for async runtime

**Relevance to trs:** Very close in spirit -- a search-oriented TUI with input, results, and preview panes. The module organization (action/event/app/render) is a good template.

**Link:** https://github.com/alexpasmantier/television

### 3. serpl (yassinebridi/serpl)

**Architecture:** Search-and-replace TUI with multi-pane layout.

**Key patterns:**

- Multiple search modes (plain, regex, case-sensitive, whole-word)
- Four-pane layout: search input, replace input, results list, preview
- Tab-based focus switching between panes
- Customizable keybindings via config files

**Relevance to trs:** The search input + results list + preview pane layout maps directly to what trs needs. The mode-switching pattern (which pane has focus) is useful.

**Link:** https://github.com/yassinebridi/serpl

## Key Takeaways for trs Implementation

1. **Start with TEA pattern** -- simple, explicit, well-documented by ratatui
2. **Use `tui-input`** for the search bar, not a full textarea widget
3. **Centralized App state** with Mode enum for focus management
4. **Sync event loop** with crossterm polling -- no need for tokio
5. **Separate non-TUI output** into its own module for clean piped/scripted usage
6. **Panic hook** to always restore terminal state
7. **Module structure** modeled after television: action.rs, event.rs, app.rs, ui.rs
8. **rusqlite with bundled+fts5** features for zero-dependency SQLite with full-text search
9. **`directories` crate** for XDG-compliant config/data paths
10. **`indicatif`** for indexing progress in non-TUI mode
