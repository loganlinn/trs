use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config;
use crate::session::App;

fn long_version() -> &'static str {
    let pkg = env!("CARGO_PKG_VERSION");
    let git = env!("TRS_GIT_DESCRIBE");
    if git.is_empty() || git.strip_prefix('v') == Some(pkg) {
        pkg
    } else {
        Box::leak(format!("{pkg} ({git})").into_boxed_str())
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "trs",
    version = long_version(),
    about = "Full-text search over chat transcripts",
    long_about = "Full-text search over chat transcripts.\n\n\
        When called with no arguments in an interactive terminal, opens the TUI.\n\
        Use `trs query` (or `trs q`) to search from the command line.",
    after_help = "Examples:\n  \
        trs                                Open interactive TUI\n  \
        trs q \"LaunchDarkly migration\"     Search for a phrase\n  \
        trs q kitty -p dotfiles            Search with project filter\n  \
        trs index                          Build/update the index\n  \
        trs index --full                   Full reindex from scratch\n  \
        trs db clean                       Delete the index database"
)]
pub struct Cli {
    /// Index database path
    #[arg(short = 'd', long = "db", env = "TRS_DB", global = true)]
    pub db: Option<PathBuf>,

    /// Disable TUI even when interactive
    #[arg(long = "no-tui", global = true)]
    pub no_tui: bool,

    /// Color output: auto, always, never
    #[arg(
        long = "color",
        env = "TRS_COLOR",
        global = true,
        default_value = "auto"
    )]
    pub color: ColorChoice,

    /// Filter TUI sessions by git branch (current branch if omitted)
    #[arg(short = 'b', long = "branch", num_args = 0..=1, default_missing_value = "")]
    pub branch: Option<String>,

    /// Filter TUI sessions by project/cwd (cwd if omitted)
    #[arg(short = 'p', long = "project", num_args = 0..=1, default_missing_value = ".")]
    pub project: Option<String>,

    /// Shorthand for -p -b (current project + current branch)
    #[arg(short = '.', num_args = 0)]
    pub dot: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Resolve the database path (flag > env > default).
    pub fn db_path(&self) -> PathBuf {
        self.db.clone().unwrap_or_else(config::default_db_path)
    }

    /// Whether color output is enabled.
    pub fn use_color(&self) -> bool {
        // NO_COLOR takes precedence (per no-color.org)
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        match self.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => is_terminal::is_terminal(std::io::stdout()),
        }
    }
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search indexed sessions
    #[command(
        alias = "q",
        after_help = "Examples:\n  \
            trs query \"LaunchDarkly migration\"\n  \
            trs q DynamoDB -b saved-media\n  \
            trs q kitty -p dotfiles\n  \
            trs q \"terraform\" -f \"*.tf\" -n 5\n  \
            trs q \"bug fix\" -C 3\n  \
            trs q --no-index \"quick query\""
    )]
    Query(SearchArgs),

    /// Build or update the search index
    #[command(after_help = "Examples:\n  \
            trs index            Incremental update (fast)\n  \
            trs index --full     Full reindex (rebuilds everything)")]
    Index(IndexArgs),

    /// Import sessions from NDJSON on stdin
    #[command(after_help = "Examples:\n  \
            cat sessions.ndjson | trs ingest\n  \
            trs ingest -s slack < export.ndjson")]
    Ingest(IngestArgs),

    /// Manage the index database
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },

    /// Show the ingest record schema
    Schema(SchemaArgs),
}

#[derive(Parser, Debug)]
pub struct SearchArgs {
    /// FTS5 search query terms
    #[arg(trailing_var_arg = true)]
    pub query: Vec<String>,

    /// Filter sessions by file path substring
    #[arg(short = 'f', long = "file")]
    pub file_pat: Option<String>,

    /// Filter sessions by git branch substring
    #[arg(short = 'b', long = "branch")]
    pub branch_pat: Option<String>,

    /// Filter sessions by project/cwd substring
    #[arg(short = 'p', long = "project")]
    pub project_pat: Option<String>,

    /// Maximum number of results
    #[arg(short = 'n', long = "limit", default_value = "20")]
    pub limit: i64,

    /// Show N messages after each match
    #[arg(short = 'A', default_value = "0")]
    pub context_after: usize,

    /// Show N messages before each match
    #[arg(short = 'B', default_value = "0")]
    pub context_before: usize,

    /// Show N messages before and after (overrides -A, -B)
    #[arg(short = 'C')]
    pub context_both: Option<usize>,

    /// Skip auto-indexing, use existing index as-is
    #[arg(long = "no-index")]
    pub no_index: bool,

    /// Filter by source app: claude (cc), codex (cx). Default: all.
    #[arg(short = 'a', long = "app")]
    pub app: Option<String>,

    /// Filter by date: supports operators (>2025-01-01, >=, =, <=, <)
    /// and shorthands (today, yesterday, 7d, 30d)
    #[arg(short = 'D', long = "date")]
    pub date: Option<String>,
}

impl SearchArgs {
    pub fn effective_context(&self) -> (usize, usize) {
        if let Some(c) = self.context_both {
            (c, c)
        } else {
            (self.context_before, self.context_after)
        }
    }

    pub fn app_filter(&self) -> Option<App> {
        self.app.as_deref().and_then(App::parse)
    }

    /// Build a TUI search input string from CLI flags.
    ///
    /// Resolves paths (e.g. `-p .` becomes the absolute cwd) but displays
    /// the user's home directory as `~` for brevity.
    pub fn to_tui_input(&self) -> String {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let abbreviate = |s: &str| -> String {
            if let Some(ref h) = home {
                if let Some(rest) = s.strip_prefix(h.to_str().unwrap_or("")) {
                    return format!("~{rest}");
                }
            }
            s.to_string()
        };

        let mut parts = Vec::new();
        if let Some(ref p) = self.project_pat {
            let resolved = crate::search::resolve_project_filter(p);
            parts.push(format!("project:{}", abbreviate(&resolved)));
        }
        if let Some(ref b) = self.branch_pat {
            parts.push(format!("branch:{b}"));
        }
        if let Some(ref f) = self.file_pat {
            parts.push(format!("file:{f}"));
        }
        if let Some(ref a) = self.app {
            parts.push(format!("app:{a}"));
        }
        if let Some(ref d) = self.date {
            parts.push(format!("date:{d}"));
        }
        if !self.query.is_empty() {
            parts.push(self.query.join(" "));
        }
        let mut s = parts.join(" ");
        if !s.is_empty() {
            s.push(' ');
        }
        s
    }
}

#[derive(Parser, Debug)]
pub struct IndexArgs {
    /// Full reindex: re-parse all sessions from scratch
    #[arg(long = "full")]
    pub full: bool,

    /// Only index a specific app: claude (cc), codex (cx). Default: all.
    #[arg(short = 'a', long = "app")]
    pub app: Option<String>,
}

impl IndexArgs {
    pub fn app_filter(&self) -> Option<App> {
        self.app.as_deref().and_then(App::parse)
    }
}

#[derive(Parser, Debug)]
pub struct IngestArgs {
    /// Only accept records matching this source value
    #[arg(short = 's', long = "source")]
    pub source: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum DbCommand {
    /// Delete the index database
    Clean {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
    /// Copy the index database to a file
    Export {
        /// Destination path for the database copy
        path: PathBuf,
    },
    /// Replace the index database from a file
    Import {
        /// Source database file to import
        path: PathBuf,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

#[derive(Parser, Debug)]
pub struct SchemaArgs {
    /// Emit raw JSON Schema instead of human-readable output
    #[arg(long = "json")]
    pub as_json: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_parses() {
        // Verify the clap derive generates valid CLI
        Cli::command().debug_assert();
    }

    #[test]
    fn test_query_args() {
        let cli = Cli::parse_from(["trs", "query", "hello", "world"]);
        match cli.command {
            Some(Command::Query(args)) => {
                assert_eq!(args.query, vec!["hello", "world"]);
            }
            _ => panic!("expected Query command"),
        }
    }

    #[test]
    fn test_query_alias() {
        let cli = Cli::parse_from(["trs", "q", "hello", "world"]);
        match cli.command {
            Some(Command::Query(args)) => {
                assert_eq!(args.query, vec!["hello", "world"]);
            }
            _ => panic!("expected Query command via alias"),
        }
    }

    #[test]
    fn test_no_args_is_none() {
        let cli = Cli::parse_from(["trs"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_index_command() {
        let cli = Cli::parse_from(["trs", "index", "--full"]);
        match cli.command {
            Some(Command::Index(args)) => assert!(args.full),
            _ => panic!("expected Index command"),
        }
    }

    #[test]
    fn test_db_clean() {
        let cli = Cli::parse_from(["trs", "db", "clean", "--force"]);
        match cli.command {
            Some(Command::Db {
                command: DbCommand::Clean { force },
            }) => assert!(force),
            _ => panic!("expected Db Clean"),
        }
    }

    #[test]
    fn test_to_tui_input_project() {
        let args = SearchArgs {
            query: vec![],
            file_pat: None,
            branch_pat: None,
            project_pat: Some(".".into()),
            limit: 20,
            context_after: 0,
            context_before: 0,
            context_both: None,
            no_index: false,
            app: None,
            date: None,
        };
        let input = args.to_tui_input();
        assert!(input.starts_with("project:"));
        assert!(input.ends_with(' '));
        // `.` should resolve; shouldn't start with "project:."
        assert!(!input.starts_with("project:."));
    }

    #[test]
    fn test_to_tui_input_multiple_filters() {
        let args = SearchArgs {
            query: vec!["search".into(), "terms".into()],
            file_pat: Some("*.rs".into()),
            branch_pat: Some("main".into()),
            project_pat: None,
            limit: 20,
            context_after: 0,
            context_before: 0,
            context_both: None,
            no_index: false,
            app: Some("claude".into()),
            date: None,
        };
        let input = args.to_tui_input();
        assert!(input.contains("branch:main"));
        assert!(input.contains("file:*.rs"));
        assert!(input.contains("app:claude"));
        assert!(input.contains("search terms"));
        assert!(input.ends_with(' '));
    }

    #[test]
    fn test_to_tui_input_empty() {
        let args = SearchArgs {
            query: vec![],
            file_pat: None,
            branch_pat: None,
            project_pat: None,
            limit: 20,
            context_after: 0,
            context_before: 0,
            context_both: None,
            no_index: false,
            app: None,
            date: None,
        };
        assert_eq!(args.to_tui_input(), "");
    }

    #[test]
    fn test_context_override() {
        let args = SearchArgs {
            query: vec![],
            file_pat: None,
            branch_pat: None,
            project_pat: None,
            limit: 20,
            context_after: 0,
            context_before: 0,
            context_both: Some(3),
            no_index: false,
            app: None,
            date: None,
        };
        assert_eq!(args.effective_context(), (3, 3));
    }

    #[test]
    fn test_app_filter() {
        let cli = Cli::parse_from(["trs", "q", "--app", "codex", "hello"]);
        match cli.command {
            Some(Command::Query(args)) => {
                assert_eq!(args.app_filter(), Some(App::Codex));
            }
            _ => panic!("expected Query command"),
        }
    }

    #[test]
    fn test_index_app_filter() {
        let cli = Cli::parse_from(["trs", "index", "--app", "claude"]);
        match cli.command {
            Some(Command::Index(args)) => {
                assert_eq!(args.app_filter(), Some(App::ClaudeCode));
            }
            _ => panic!("expected Index command"),
        }
    }
}
