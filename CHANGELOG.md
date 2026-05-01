# Changelog

## [Unreleased]

## [0.5.1](https://github.com/loganlinn/trs/releases/tag/v0.5.1) - 2026-04-30

### Changed

- **Resume via `exec` instead of suspend/resume**: Selecting a session now `exec`s `claude --resume <id>` (or `codex`) directly, replacing the `trs` process. Eliminates the input lag that occurred when re-entering the TUI after the child exited.

### Added

- **`-c` / `--continue` flag**: Restores the prior TUI session — search query, pinned filters, and last-selected session id are persisted to `$XDG_STATE_HOME/trs/last.json` before exec, and reloaded on `trs --continue`. Skips re-indexing so the database matches the version that produced the saved results.

## [0.5.0](https://github.com/loganlinn/trs/compare/v0.1.0...v0.5.0) - 2026-04-08

### Added

- manual version bump for tui layout redesign (4792c88a65bd0a5b060b7231fc14c0abaf4de7ea)
- support session ID search

### Fixed

- fix failing test
- fix clippy findings
- fix gha workflows

### Added

- **UUID session lookup**: Paste a Claude session ID (UUID) into the TUI search box or `trs query <UUID>` to find a session by exact ID. Bypasses FTS and does a direct database lookup.
- **Configurable key bindings**: Key bindings are now loaded from `$XDG_CONFIG_HOME/trs/config.toml` under `[keys.normal]`, `[keys.detail]`, and `[keys.help]` sections. Supports single (`"ctrl-y"`) or multi-key (`["down", "ctrl-n"]`) bindings. All defaults are built in — config file is optional.

### Changed

- **TUI table layout with preview pane**: Search results now render as an aligned columnar table (Date, Project, Title, Msgs) with a split preview pane below showing matched snippets or the opening message. Replaces the free-form two-line list items. Dates use compact relative format (`today`, `3d`, `2w`, `Mar 2`). Preview pane hides on short terminals (< 16 rows).
- **TUI normal mode keybindings**: Bare alpha-key shortcuts (`r`, `y`, `q`) no longer intercept text input in the search box. Copy session ID moved to `Ctrl-Y`; quit via `Esc` or `Ctrl-C`. All characters now reach the input widget regardless of whether the search box is empty.
- **Removed copy resume command**: The `r` / copy resume command shortcut has been removed. Use `Ctrl-Y` to show the session ID instead.
- **Dynamic help overlay**: Help text and status bar hints are now generated from the active key binding config, staying in sync with user overrides.

## [0.4.2](https://github.com/loganlinn/trs/releases/tag/v0.4.2) - 2026-03-29

### Fixed

- Fixed two `resolve_project_filter` test assertions that hardcoded `/private/tmp` (macOS-specific) instead of dynamically resolving the canonical path, causing test failures on Linux. ([9c9dc70](https://github.com/loganlinn/trs/commit/9c9dc700738f4c0d4acc73351c36a01cc7a23de7))

## [0.4.0](https://github.com/loganlinn/trs/releases/tag/v0.4.0) - 2026-03-29

### Added

- **TUI pinned filters** (`-b`, `-p`, `-.`): Launch the TUI with persistent branch and/or project filters that stay active across searches. ([#release](https://github.com/loganlinn/trs/commit/8e2ea58453a3ba0f44918a02157c678916af4a8d))
  - `trs -b` — filter to current git branch; `trs -b main` — filter to a named branch.
  - `trs -p` — filter to current working directory; `trs -p dotfiles` — filter to a named project.
  - `trs -.` — shorthand for `-p -b` (current project + current branch).
- **Pinned filter display**: Active pinned filters are shown as a yellow `[branch:main]` badge in the search box title.
- **Search term highlighting in results list**: Matched terms are highlighted (bold red) in the project path, custom title, metadata, and first-message preview of each result.
- **Prefix matching in TUI**: As-you-type search uses FTS5 prefix queries (e.g. typing "sess" matches "session"); prefix wildcard requires 3+ characters to avoid overly broad matches.
- **`custom_title` in FTS index**: Session names set via `--name` or `/rename` are now full-text indexed and searchable.
- **Two-tier search ranking**: Sessions matching in metadata (title, cwd, summary, first_message, branches, files) always rank above body-only matches. Within each tier, BM25 with column weights (title 20x, cwd/summary 10x, branches/first_message 5x, files 3x, body 1x) determines order.
- **Help overlay from any screen**: `Ctrl-/` now opens the help overlay from both Normal and Detail views, and correctly returns to the previous screen on dismiss.
- **Git describe in `--version`**: `trs --version` now shows tag/commit info (e.g. `0.4.0 (v0.4.0-5-gabcdef)`) when built from a git checkout.
- **Debounced search input**: TUI search waits 150 ms after the last keystroke before querying, reducing unnecessary work while typing.
- **Date filter** (`--date`/`-D` on CLI; `date:`/`d:` in TUI): Filter sessions by date with comparison operators (`>`, `>=`, `=`, `<=`, `<`) and shorthands (`today`, `yesterday`, `7d`, `30d`). Partial dates like `2025-03` match all days in the month.
- **CLI-to-TUI filter parity**: CLI flags (`-p`, `-b`, `-f`, `-a`, `-D`) now seed the TUI search input via `to_tui_input()`, so filter-only invocations like `trs query -p myproject` drop into a pre-filtered TUI instead of showing help.
- **Project filter wildcards**: Trailing `/*` or `*` on project filters enables prefix matching (e.g. `/home/user/gamma*` matches all worktrees). Paths containing `/` use exact match; plain names use substring match.
- **Resume missing directory**: When resuming a session whose `cwd` no longer exists, `trs` now prompts to create the directory instead of silently warning.

### Changed

- **Help keybinding**: Changed from `?` to `Ctrl-/` so the overlay works regardless of whether the search input is focused. Added `Ctrl-j`/`Ctrl-k` as aliases for down/up selection.
- **Context defaults**: `-A`/`-B` now default to 0 (previously 1).
- **TUI result layout**: Custom title is shown after metadata instead of inline with the path; branches use compact `@branch` format; paths display the project slug via shared `display::project_slug`.
- **Two-tier search uses separate queries**: Metadata-matching session IDs are collected in a lightweight first query; the main FTS query no longer uses a CTE with two `MATCH` clauses, improving compatibility and performance.

## [0.3.1](https://github.com/loganlinn/trs/releases/tag/v0.3.1) - 2026-03-29

### Changed

- Internal refactor: replaced positional filter arguments on `db::search()` and `db::list_recent()` with a `SearchFilter` struct, resolving a `clippy::too_many_arguments` warning. No user-facing behaviour change. ([7cf5a4c](https://github.com/loganlinn/trs/commit/7cf5a4c99c749ac68b5b48e17b2e1c46e6878e4c))

## [0.2.0] - 2026-03-20

### Added

- **Codex support**: Index and search OpenAI Codex sessions alongside Claude Code sessions.
  - Discovers sessions from `~/.codex/sessions/` and `~/.codex/archived_sessions/`.
  - Parses Codex JSONL format (session_meta, response_item, turn_context, function_call).
  - Resume Codex sessions with `codex --resume <id>` from search results and TUI.
- **`App` enum** (`ClaudeCode`, `Codex`): Type-safe representation of source applications.
- **`--app` / `-a` flag** on `index` and `query` commands to filter by source app.
  - Accepts: `claude` / `cc`, `codex` / `cx`. Omit for all apps.
- Source filter on `db::search()` and `db::list_recent()`.
- Tests for Codex session parsing, message extraction, source filtering, and CLI flags.

### Removed

- **Profiles system**: Removed `profiles` command, `--profile` / `-P` flag on ingest,
  `FieldProfile`, `apply_profile`, and `profiles.toml` config loading.
  The `ingest` command still accepts NDJSON on stdin.
- `toml` dependency.

### Changed

- `trs index` now indexes all supported apps by default (Claude Code + Codex).
- `parse_session()` and `extract_messages()` now dispatch by `App` type.
- `session_jsonl_path()` searches both Claude Code and Codex directories.
- TUI resume/fork actions use the correct CLI binary based on session source.## [0.1.2] - 2026-03-19

### Changed

- Version bump.## [0.1.1] - 2026-03-18

### Added

- Show recent conversations on empty state in TUI.## [0.1.0] - 2026-03-17

- Initial release.
