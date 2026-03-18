# trs

Local-first, full-text search over chat transcripts.

`trs` indexes [Claude Code](https://docs.anthropic.com/en/docs/claude-code) session JSONL files into a SQLite FTS5 database and lets you search them from a terminal UI or the command line. It also accepts transcripts from any source via NDJSON ingest with configurable field-mapping profiles.

## Install

```
cargo install --path .
```

## Usage

```
trs                                # open interactive TUI
trs q "LaunchDarkly migration"     # search for a phrase
trs q kitty -p dotfiles            # filter by project
trs q "terraform" -f "*.tf" -n 5   # filter by file, limit results
trs q "bug fix" -C 3               # show 3 messages of context
```

### Index

```
trs index          # incremental update
trs index --full   # full reindex from scratch
```

Sessions are discovered from `~/.claude/projects/` and indexed into `$XDG_DATA_HOME/trs/index.db`.

### Ingest

Pipe NDJSON records from any source:

```
cat sessions.ndjson | trs ingest
my-export-tool | trs ingest --profile slack
```

Required fields: `session_id`, `source`, `body`. Run `trs schema` for the full spec or `trs schema --json` for JSON Schema.

### Profiles

Map foreign record shapes to the ingest schema via `$XDG_CONFIG_HOME/trs/profiles.toml`:

```toml
[profiles.slack]
source = "slack"

[profiles.slack.fields]
"channel.name" = "slug"
"messages_text" = "body"

[profiles.slack.defaults]
message_count = 0
```

```
trs profiles       # list configured profiles
```

### Database management

```
trs db clean       # delete index
trs db export db.sqlite
trs db import db.sqlite
```

## Configuration

| Flag / Env | Description | Default |
|---|---|---|
| `-d` / `TRS_DB` | Database path | `$XDG_DATA_HOME/trs/index.db` |
| `--color` / `TRS_COLOR` | Color output (`auto`, `always`, `never`) | `auto` |
| `--no-tui` | Disable TUI even in interactive terminals | — |
| `NO_COLOR` | Disable color ([no-color.org](https://no-color.org)) | — |

## License

MIT
