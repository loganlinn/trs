# Changelog

## [Unreleased]

### Added

- **Search term highlighting in results list**: Matched terms are now highlighted (bold red) in the cwd/path, custom title, metadata, and preview fields of each search result, making it clear why a result matched.

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
