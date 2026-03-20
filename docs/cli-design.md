# trs CLI Design

## Design Rationale

### What changed from trs1

**Proper subcommand structure.** The Python version overloads the root command
with `--clean`, `--export`, `--import`, `--index`, `--reindex`, and `--no-index`
as flags. These are actions, not modifiers — they belong as subcommands. The new
design groups them logically:

- `trs search` — search (the primary operation)
- `trs index` — build/update the FTS index
- `trs ingest` — import NDJSON from stdin
- `trs db` — database management (`clean`, `export`, `import`)
- `trs schema` — show the ingest record schema
- `trs profiles` — list configured ingest profiles

**Implicit search preserved.** `trs <query>` remains shorthand for
`trs search <query>`. When invoked with no arguments in an interactive terminal,
trs launches a TUI for interactive search.

**Index decoupled from search.** In trs1, `--index` and `--reindex` are search
flags that trigger indexing before searching. Now `trs index` is its own command.
Search auto-indexes incrementally by default (controllable via `--no-index`).

**`trs db` subgroup.** Database operations (`clean`, `export`, `import`) are
grouped under `trs db` instead of being top-level flags.

**TUI mode.** Running `trs` with no arguments in an interactive terminal opens
an interactive search TUI. Pass `--no-tui` or pipe output to disable.

---

## Help Output

### `trs --help`

```
trs — full-text search over chat session transcripts

Usage: trs [OPTIONS] [QUERY]...
       trs <COMMAND>

  When called with a query, searches indexed sessions (shorthand for `trs search`).
  When called with no arguments in an interactive terminal, opens the TUI.

Commands:
  search    Search indexed sessions (default command)
  index     Build or update the search index
  ingest    Import sessions from NDJSON on stdin
  db        Manage the index database
  schema    Show the ingest record schema
  profiles  List configured ingest profiles

Options:
  -d, --db <PATH>    Index database path [env: TRS_DB]
                     [default: ~/.local/share/trs/index.db]
      --no-tui       Disable TUI even when interactive
      --color <WHEN>  Color output: auto, always, never [default: auto]
                     [env: TRS_COLOR]
  -h, --help         Print help
  -V, --version      Print version

  Any unrecognized arguments are treated as a search query.
  Use `trs search --help` for search-specific options.

Examples:
  trs "LaunchDarkly migration"       Search for a phrase
  trs                                Open interactive TUI
  trs index                          Build/update the index
  trs index --full                   Full reindex from scratch
  trs db clean                       Delete the index database
```

### `trs search --help`

```
Search indexed sessions

Usage: trs search [OPTIONS] [QUERY]...

  Positional arguments are joined into an FTS5 query. Supports:
    plain words:    trs search LaunchDarkly migration
    quoted phrase:  trs search '"exact phrase"'
    prefix:         trs search "migrat*"
    column scope:   trs search "first_message:dynamo"
    boolean:        trs search "rust AND wasm"

  An incremental index is run automatically before searching.
  Use --no-index to skip, or run `trs index` separately.

Arguments:
  [QUERY]...  FTS5 search query terms

Options:
  -f, --file <PATTERN>     Filter sessions by file path substring
  -b, --branch <PATTERN>   Filter sessions by git branch substring
  -p, --project <PATTERN>  Filter sessions by project/cwd substring
  -n, --limit <N>          Maximum number of results [default: 20]
  -A <N>                   Show N messages after each match [default: 1]
  -B <N>                   Show N messages before each match [default: 1]
  -C <N>                   Show N messages before and after (overrides -A, -B)
      --no-index           Skip auto-indexing, use existing index as-is
  -d, --db <PATH>          Index database path [env: TRS_DB]
      --color <WHEN>       Color output: auto, always, never [default: auto]
  -h, --help               Print help

Examples:
  trs search "LaunchDarkly migration"
  trs search DynamoDB -b saved-media
  trs search kitty -p dotfiles
  trs search "terraform" -f "*.tf" -n 5
  trs search "bug fix" -C 3
  trs search --no-index "quick query"
```

### `trs index --help`

```
Build or update the search index

Usage: trs index [OPTIONS]

  Scans ~/.claude/projects/ for session JSONL files and indexes them
  into the FTS5 database. By default, only new or modified sessions
  are indexed (incremental). Stale sessions are pruned.

Options:
      --full          Full reindex: re-parse all sessions from scratch
  -d, --db <PATH>     Index database path [env: TRS_DB]
  -h, --help          Print help

Examples:
  trs index            Incremental update (fast)
  trs index --full     Full reindex (rebuilds everything)
```

### `trs ingest --help`

```
Import sessions from NDJSON on stdin

Usage: trs ingest [OPTIONS]

  Reads newline-delimited JSON from stdin, one session per line.
  Each line must be a JSON object with at minimum:
    session_id (str), source (str), body (str)

  Use `trs schema` to see all supported fields.
  Extra fields are stored in the metadata column.

  Records with a matching content_hash are skipped (deduplication).

Options:
  -P, --profile <NAME>    Apply a profile from profiles.toml
      --config <PATH>      Path to profiles TOML config
                           [default: ~/.config/trs/profiles.toml]
  -s, --source <SOURCE>   Only accept records matching this source value
  -d, --db <PATH>          Index database path [env: TRS_DB]
  -h, --help               Print help

Examples:
  cat sessions.ndjson | trs ingest
  my-export-tool | trs ingest --profile codex
  trs ingest -s slack < export.ndjson
```

### `trs db --help`

```
Manage the index database

Usage: trs db <COMMAND>

Commands:
  clean   Delete the index database
  export  Copy the index database to a file
  import  Replace the index database from a file

Options:
  -h, --help  Print help
```

### `trs db clean --help`

```
Delete the index database

Usage: trs db clean [OPTIONS]

  Removes the index database file. You will need to run `trs index`
  to rebuild it. Prompts for confirmation unless --force is given.

Options:
      --force        Skip confirmation prompt
  -d, --db <PATH>    Index database path [env: TRS_DB]
  -h, --help         Print help
```

### `trs db export --help`

```
Copy the index database to a file

Usage: trs db export [OPTIONS] <PATH>

Arguments:
  <PATH>  Destination path for the database copy

Options:
  -d, --db <PATH>    Index database path [env: TRS_DB]
  -h, --help         Print help

Examples:
  trs db export ~/backup.db
  trs db export /tmp/trs-snapshot.db
```

### `trs db import --help`

```
Replace the index database from a file

Usage: trs db import [OPTIONS] <PATH>

  Copies a database file into the index location, replacing any
  existing index. Prompts for confirmation if an index already exists.

Arguments:
  <PATH>  Source database file to import

Options:
      --force        Skip confirmation prompt
  -d, --db <PATH>    Index database path [env: TRS_DB]
  -h, --help         Print help

Examples:
  trs db import ~/backup.db
```

### `trs schema --help`

```
Show the ingest record schema

Usage: trs schema [OPTIONS]

  Prints the canonical IngestRecord schema used by `trs ingest`.

Options:
      --json    Emit raw JSON Schema instead of human-readable output
  -h, --help    Print help
```

### `trs profiles --help`

```
List configured ingest profiles

Usage: trs profiles [OPTIONS]

  Profiles define field mappings and defaults for `trs ingest --profile`.
  Config is read from ~/.config/trs/profiles.toml by default.

Options:
      --config <PATH>  Path to profiles TOML config
                       [default: ~/.config/trs/profiles.toml]
  -h, --help           Print help
```

---

## Environment Variables

| Variable      | Description                                        | Default                          |
|---------------|----------------------------------------------------|----------------------------------|
| `TRS_DB`      | Path to the index database                         | `~/.local/share/trs/index.db`   |
| `TRS_COLOR`   | Color mode: `auto`, `always`, `never`              | `auto`                           |
| `NO_COLOR`    | Disable color output (any non-empty value)         | _(unset)_                        |
| `XDG_DATA_HOME` | Base directory for index database                | `~/.local/share`                 |
| `XDG_CONFIG_HOME` | Base directory for profiles config             | `~/.config`                      |

`NO_COLOR` (per https://no-color.org/) takes precedence over `TRS_COLOR`.
`--color` flag takes precedence over both environment variables.

---

## Exit Codes

| Code | Meaning                                            |
|------|----------------------------------------------------|
| 0    | Success                                            |
| 1    | General error (bad arguments, runtime failure)     |
| 2    | No results found (search only)                     |
| 3    | Index not found (search without `--no-index`)      |
| 130  | Interrupted (Ctrl-C / SIGINT)                      |

Exit code 2 for "no results" enables scripting patterns like:

```sh
if trs search "migration" -n 1 >/dev/null 2>&1; then
  echo "found sessions about migration"
fi
```

---

## TUI Keyboard Shortcuts

The TUI opens when `trs` is invoked with no arguments in an interactive
terminal. It provides real-time search with a fuzzy-matching input.

| Key | Action |
|---|---|
| <kbd>Enter</kbd> | Resume session (`claude --resume`) |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | Fork session (`--fork-session`) |
| <kbd>Tab</kbd> | Open selected session detail |
| <kbd>Esc</kbd> / <kbd>q</kbd> | Quit / go back |
| <kbd>↑</kbd> / <kbd>Ctrl</kbd>+<kbd>P</kbd> | Previous result |
| <kbd>↓</kbd> / <kbd>Ctrl</kbd>+<kbd>N</kbd> | Next result |
| <kbd>Ctrl</kbd>+<kbd>U</kbd> | Clear search input |
| <kbd>Ctrl</kbd>+<kbd>D</kbd> | Scroll down half page |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> | Scroll up half page |
| <kbd>/</kbd> | Focus search input (from detail) |
| <kbd>y</kbd> | Show session ID |
| <kbd>r</kbd> | Show resume command |
| <kbd>?</kbd> | Show help overlay |

### Session Detail View

| Key | Action |
|---|---|
| <kbd>Esc</kbd> / <kbd>q</kbd> | Back to results list |
| <kbd>j</kbd> / <kbd>↓</kbd> | Scroll down |
| <kbd>k</kbd> / <kbd>↑</kbd> | Scroll up |
| <kbd>g</kbd> | Jump to top |
| <kbd>G</kbd> | Jump to bottom |
| <kbd>n</kbd> | Next match |
| <kbd>N</kbd> | Previous match |
| <kbd>y</kbd> | Show session ID |
| <kbd>r</kbd> | Show resume command |

---

## Command Routing Logic

The implicit-search behavior (`trs <query>` as shorthand for `trs search <query>`)
is implemented via clap's external subcommand mechanism or a custom dispatch:

```
trs <args>
  |
  +-- recognized subcommand? --> dispatch to subcommand
  |
  +-- no args + isatty(stdout)? --> launch TUI
  |
  +-- no args + !isatty(stdout)? --> print help to stderr, exit 1
  |
  +-- unrecognized args --> treat as `trs search <args>`
```

This is the same pattern used by `git` (e.g., `git diff` is shorthand).

---

## Global Options Inheritance

The `--db` and `--color` flags are defined on the root command and inherited
by all subcommands. Subcommands that need them redeclare them for discoverability
in `--help`, but they all resolve the same way:

1. Explicit `--db <path>` flag
2. `TRS_DB` environment variable
3. Default: `$XDG_DATA_HOME/trs/index.db`

Color resolution:

1. Explicit `--color <when>` flag
2. `NO_COLOR` env var (if set, disables color)
3. `TRS_COLOR` env var
4. Default: `auto` (color when stdout is a terminal)
