# Changelog

## [Unreleased]

## [0.4.0] - 2026-03-28

### Added

- **TUI pinned filters** (`-b`, `-p`, `-.`): Launch the TUI with persistent branch and/or project filters that stay active across searches.
  - `trs -b` — filter to current git branch.
  - `trs -b main` — filter to an explicit branch.
  - `trs -p` — filter to current working directory.
  - `trs -p dotfiles` — filter to a named project.
  - `trs -.` — shorthand for `-p -b` (current project + current branch).
- **Pinned filter display**: Active pinned filters are shown as a yellow `[branch:main]` badge in the search box title, making scope visible at a glance.

### Changed

- **`SearchFilter` struct**: Replaced positional filter arguments on `db::search()` and `db::list_recent()` with a `SearchFilter` struct, fixing `clippy::too_many_arguments`.
- **`is_some_and` idiom**: Replaced `map_or(false, ...)` with `is_some_and(...)` in date filter parsing.
- **Release task gates on lint + test**: `mise run release` now depends on `lint` and `test` tasks.

### Added

- **Search term highlighting in results list**: Matched terms are now highlighted (bold red) in the cwd/path, custom title, metadata, and preview fields of each search result, making it clear why a result matched.
- **Prefix matching in TUI**: As-you-type search now uses FTS5 prefix queries (e.g. typing "sess" matches "session"), so results appear incrementally while typing.
- **`custom_title` in FTS index**: Session names (from `--name` / `/rename`) are now full-text indexed and searchable.
- **Two-tier search ranking**: Sessions matching in metadata (title, cwd, summary, first_message, branches, files) always rank above body-only matches. Within each tier, BM25 with column weights (title 20x, cwd/summary 10x, branches/first_message 5x, files 3x, body 1x) determines order.
- **Help overlay from any screen**: The `?` help overlay now works from both Normal and Detail views, and correctly returns to the previous screen on dismiss.
- **Git describe in `--version`**: Build script embeds `git describe` output so `trs --version` shows the tag/commit (e.g. `0.2.0 (v0.2.0-5-gabcdef)`).
- **Debounced search input**: TUI search now waits 150ms after the last keystroke before querying, reducing unnecessary work while typing.
- **Date filter** (`--date`/`-D`, `date:`/`d:` in TUI): Filter sessions by date with comparison operators (`>`, `>=`, `=`, `<=`, `<`) and shorthands (`today`, `yesterday`, `7d`, `30d`). Partial dates like `2025-03` match all days in the month.
- **CLI-to-TUI filter parity**: CLI flags (`-p`, `-b`, `-f`, `-a`, `-D`) now seed the TUI search input via `to_tui_input()`. `list_recent` accepts all filter params, so filter-only queries (no search text) apply filters in both CLI and TUI.
- **Project filter wildcards**: Trailing `/*` or `*` on project filters enables prefix matching. Paths containing `/` use exact match; plain names use substring match.
- **`display` module**: Extracted shared display logic (project slug, role markers, date formatting, result grouping, snippet building) from `output` and `tui::ui` into `src/display.rs`.
- **Resume missing directory**: When resuming a session whose cwd no longer exists, prompt to create the directory instead of silently warning.

### Changed

- **Two-tier search uses separate queries**: Metadata-matching session IDs are collected in a lightweight first query; the main FTS query no longer uses a CTE with two MATCH clauses, improving compatibility and performance.
- **Prefix wildcard requires 3+ characters**: As-you-type prefix matching now only appends `*` when the last token is at least 3 characters, avoiding overly broad matches on short inputs.
- **Help keybinding**: Changed from `?` to `Ctrl-/` so help works regardless of input state. Added `Ctrl-j`/`Ctrl-k` as selection aliases.
- **Context defaults**: `-A`/`-B` default to 0 (was 1).
- **TUI result layout**: Title shown after metadata instead of inline with path; branches use compact `@branch` format; paths use `display::project_slug`.

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
- TUI resume/fork actions use the correct CLI binary based on session source.

## [0.1.2] - 2026-03-19

### Changed

- Version bump.

## [0.1.1] - 2026-03-18

### Added

- Show recent conversations on empty state in TUI.

## [0.1.0] - 2026-03-17

- Initial release.
