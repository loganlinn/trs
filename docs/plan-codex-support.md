---
date: 2026-03-20
git_revision: fd6a0d6
model: claude-opus-4-6
claude_version: claude-code
---

# Plan: Multi-application support (Codex + Claude Code)

## Problem

trs currently only indexes Claude Code sessions from `~/.claude/projects/`. OpenAI Codex stores sessions in a completely different layout (`~/.codex/sessions/YYYY/MM/DD/*.jsonl`) with a different JSONL schema. We need to support both — and make the architecture extensible for future apps.

## Key observations from research

### Codex session format
- **Location:** `~/.codex/sessions/YYYY/MM/DD/<name>.jsonl` (72 files found)
- **Archived:** `~/.codex/archived_sessions/*.jsonl` (3 files)
- **No project-based directory structure** — cwd is in the `session_meta` payload instead
- **JSONL line types:** `session_meta`, `event_msg`, `response_item`, `turn_context`
- **Roles:** `developer` (system prompt), `user`, `assistant`
- **session_meta payload:** `{id, timestamp, cwd, originator, cli_version, source, model_provider, git: {branch, ...}}`
- **response_item payload:** `{type, role, content: [{type: "input_text"|"output_text"|"function_call"|"function_call_output", ...}]}`
- **turn_context payload:** `{turn_id, cwd, model, summary, ...}`
- **Session ID format:** UUID, embedded in filename after `rollout-<datetime>-`
- **No slug/project concept** — flat date-based hierarchy

### Key differences from Claude Code format
| | Claude Code | Codex |
|---|---|---|
| Location | `~/.claude/projects/{slug}/{id}.jsonl` | `~/.codex/sessions/YYYY/MM/DD/*.jsonl` |
| Metadata | Top-level `cwd`, `gitBranch`, `timestamp` fields | Nested in `session_meta.payload` |
| Message types | `type: "user"/"assistant"/"summary"` | `type: "response_item"` with `payload.role` |
| Content | `message.content` (string or blocks) | `payload.content` (array of typed blocks) |
| Text block type | `{type: "text", text: "..."}` | `{type: "output_text", text: "..."}` |
| Tool use | `{type: "tool_use", name, input}` | `{type: "function_call", name, arguments}` |
| Tool result | `{type: "tool_result"}` | `{type: "function_call_output", output}` |
| User input | `{type: "input_text", text: "..."}` in content blocks or string | `{type: "input_text", input_text: "..."}` (note: field is `input_text` not `text`) |
| Session ID | filename stem | `session_meta.payload.id` (UUID) |
| Slug | parent directory name | none (derive from cwd) |
| Resume | `claude --resume <id>` | `codex --resume <id>` |

## Design

### New concept: `App` enum

Introduce an `App` enum that represents the source application. This replaces the free-form `source` string for built-in apps while keeping `source` in the DB for display/filtering.

```rust
pub enum App {
    ClaudeCode,
    Codex,
}
```

Each variant knows:
- Where to find sessions on disk (`sessions_dir()`)
- How to parse its JSONL format (`parse_session()`, `extract_messages()`)
- How to construct a resume command (`resume_cmd()`)
- Its canonical source string (`"claude-code"`, `"codex"`)

### Changes by file

#### 1. `src/session.rs` — Add `App` enum

- Add `App` enum with `ClaudeCode` and `Codex` variants
- Implement `App::source_str()`, `App::sessions_dir()`, `App::resume_cmd(session_id)`
- Add `App::from_source(s: &str) -> Option<App>` for DB round-tripping
- Add `App::all() -> &[App]` for iteration

#### 2. `src/config.rs` — Multi-app session discovery

- Replace `projects_dir()` with `App`-aware methods
- Add `codex_sessions_dir() -> PathBuf` (`~/.codex/sessions`)
- Add `codex_archived_dir() -> PathBuf` (`~/.codex/archived_sessions`)

#### 3. `src/indexer.rs` — Codex parser + unified indexing

- **`glob_sessions()` → `glob_sessions_for(app: &App)`**: Each app has its own glob logic
  - `ClaudeCode`: existing `~/.claude/projects/**/*.jsonl` (skip subagents/tool-results)
  - `Codex`: `~/.codex/sessions/**/*.jsonl` + `~/.codex/archived_sessions/*.jsonl`
- **`glob_all_sessions() -> Vec<(App, PathBuf)>`**: Combines all apps
- **`parse_session(path, app)` → dispatch**: Route to `parse_claude_session()` or `parse_codex_session()`
- **New `parse_codex_session(path)`**: Parse Codex JSONL format
  - Extract `session_meta` for id, cwd, git branch, timestamps
  - Extract `response_item` with role=user for user messages
  - Extract `response_item` with role=assistant for assistant text (`output_text` blocks)
  - Extract `function_call` blocks for tool use (track tool names, file paths)
  - Extract `turn_context` for summary, model info
  - Derive slug from cwd (last path component of the project root)
- **New `extract_codex_messages(path)`**: For TUI detail view
- **`run_index()` changes**: Iterate over all apps, or filter by `--app` flag

#### 4. `src/cli.rs` — New `--app` filter flag

- Add `--app` / `-a` flag to `SearchArgs` and `IndexArgs`: `Option<String>` accepting `claude`, `codex`, or `all` (default)
  - Use short aliases: `claude`/`cc` for ClaudeCode, `codex`/`cx` for Codex
- The flag filters which apps to index and which results to show

#### 5. `src/db.rs` — Source-aware queries

- Add optional `source` filter to `search()` and `list_recent()`
- No schema changes needed — `source` column already exists

#### 6. `src/search.rs` — Source-aware JSONL lookup

- **`session_jsonl_path()`**: Extend to search Codex session dirs too, or use `App::from_source()` to dispatch
- **`run_search()`**: Pass through source filter

#### 7. `src/output.rs` — App-aware resume command

- `print_session_footer()`: Use `App::from_source()` to generate correct resume command (`codex --resume` vs `claude --resume`)

#### 8. `src/tui/app.rs` — App-aware resume/fork

- `exec_exit_action()`: Dispatch to correct CLI binary based on source

#### 9. Drop profiles system

Since you're not liking the current profiles direction, we'll remove the `Profiles` command and `config::FieldProfile`/`apply_profile`. The `ingest` command stays (for arbitrary NDJSON), but profiles are removed. This simplifies the codebase. Can re-add a better version later.

### What stays the same
- DB schema (no migration needed, `source` column already exists)
- Ingest command (still accepts NDJSON on stdin)
- FTS5 search mechanics
- TUI layout and keybindings

## File change summary

| File | Action |
|---|---|
| `src/session.rs` | Add `App` enum |
| `src/config.rs` | Add Codex paths, remove profile types |
| `src/indexer.rs` | Add `parse_codex_session`, `extract_codex_messages`, multi-app glob |
| `src/cli.rs` | Add `--app`/`-a` flag, remove `Profiles` command |
| `src/db.rs` | Add source filter to search/list_recent |
| `src/search.rs` | Multi-app JSONL path resolution |
| `src/output.rs` | App-aware resume command |
| `src/main.rs` | Remove profiles command handler, wire `--app` through |
| `src/tui/app.rs` | App-aware exit actions |
| `tests/integration.rs` | Add Codex parsing tests |
| `CHANGELOG.md` | Create with v0.2.0 entry |

## Test plan

- Unit tests for `parse_codex_session()` with fixture JSONL
- Unit tests for `extract_codex_messages()`
- Unit tests for `App` enum methods
- Unit tests for source filter in `db::search()`
- Integration test: index + search across both app types
- Integration test: `--app codex` filter
- Integration test: verify `trs index` discovers Codex sessions
