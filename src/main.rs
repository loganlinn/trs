mod cli;
mod config;
mod db;
mod display;
mod error;
mod indexer;
mod output;
mod search;
mod session;
mod tui;

use std::io::{self, BufRead, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command, DbCommand};
use crate::session::{App, IngestRecord, INGEST_SCHEMA};
use crate::tui::PinnedFilters;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let log_dir = config::log_dir();
    if std::fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    let log_file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("trs.log"))
    {
        Ok(f) => f,
        Err(_) => return,
    };
    let filter = EnvFilter::try_from_env("TRS_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .try_init();
}

fn main() {
    init_tracing();
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e:#}");
            1
        }
    };
    process::exit(code);
}

fn run(cli: Cli) -> Result<i32> {
    let db_path = cli.db_path();
    let color = cli.use_color();

    match cli.command {
        Some(Command::Query(args)) => {
            let query_str = args.query.join(" ");
            if query_str.is_empty() {
                // No query text: launch TUI with filters pre-populated
                if !cli.no_tui && is_terminal::is_terminal(io::stderr()) {
                    let initial = args.to_tui_input();
                    if let Some(action) = tui::run(&initial, PinnedFilters::default())? {
                        exec_exit_action(action);
                    }
                    return Ok(0);
                }
                use clap::CommandFactory;
                Cli::command().print_help()?;
                return Ok(1);
            }

            let app_filter = args.app_filter();

            // Auto-index unless --no-index
            if !args.no_index {
                indexer::run_index(&db_path, false, app_filter.as_ref())?;
            }

            let query = db::normalize_fts_query(&query_str);
            let (ctx_before, ctx_after) = args.effective_context();
            let source_str = app_filter.map(|a| a.source_str().to_string());
            let project_pat = args.project_pat.as_deref().map(search::resolve_project_filter);
            let date_filter = args.date.as_deref().and_then(search::parse_date_filter);
            let filter = db::SearchFilter {
                file_pat: args.file_pat.as_deref(),
                branch_pat: args.branch_pat.as_deref(),
                project_pat: project_pat.as_deref(),
                source: source_str.as_deref(),
                date: date_filter.as_ref(),
            };
            let found = search::run_search(
                &query,
                &db_path,
                &filter,
                args.limit,
                ctx_before,
                ctx_after,
                color,
            )?;
            Ok(if found { 0 } else { 2 })
        }
        Some(Command::Index(args)) => {
            let app_filter = args.app_filter();
            indexer::run_index(&db_path, args.full, app_filter.as_ref())?;
            Ok(0)
        }
        Some(Command::Ingest(args)) => {
            run_ingest(&db_path, &args)?;
            Ok(0)
        }
        Some(Command::Db { command }) => {
            run_db_command(command, &db_path)?;
            Ok(0)
        }
        Some(Command::Schema(args)) => {
            run_schema(args.as_json)?;
            Ok(0)
        }
        None => {
            // No subcommand: launch TUI if interactive, otherwise show help
            if !cli.no_tui && is_terminal::is_terminal(io::stderr()) {
                let mut branch = cli.branch;
                let mut project = cli.project;
                if cli.dot {
                    branch.get_or_insert_with(String::new);
                    project.get_or_insert_with(|| ".".to_string());
                }
                let branch = match branch.as_deref() {
                    Some("") => current_git_branch(),
                    other => other.map(str::to_string),
                };
                let project = project.as_deref()
                    .map(search::resolve_project_filter);
                let pinned = PinnedFilters { branch, project };
                if let Some(action) = tui::run("", pinned)? {
                    exec_exit_action(action);
                }
                return Ok(0);
            }
            use clap::CommandFactory;
            Cli::command().print_help()?;
            Ok(1)
        }
    }
}

/// Replace the current process with the appropriate `--resume` command.
fn exec_exit_action(action: tui::ExitAction) -> ! {
    let (session_id, cwd, source, fork) = match action {
        tui::ExitAction::Resume {
            session_id,
            cwd,
            source,
        } => (session_id, cwd, source, false),
        tui::ExitAction::Fork {
            session_id,
            cwd,
            source,
        } => (session_id, cwd, source, true),
    };
    if !cwd.is_empty() {
        let dir = Path::new(&cwd);
        if dir.is_dir() {
            if let Err(e) = std::env::set_current_dir(dir) {
                eprintln!("Warning: failed to chdir to {cwd}: {e}");
            }
        } else {
            eprintln!("Session directory no longer exists: {cwd}");
            eprint!("Create empty directory and resume? [y/N] ");
            io::stdout().flush().ok();
            let mut answer = String::new();
            if io::stdin().read_line(&mut answer).is_err()
                || !answer.trim().eq_ignore_ascii_case("y")
            {
                eprintln!("Aborted.");
                process::exit(1);
            }
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("Failed to create directory {cwd}: {e}");
                process::exit(1);
            }
            if let Err(e) = std::env::set_current_dir(dir) {
                eprintln!("Failed to chdir to {cwd}: {e}");
                process::exit(1);
            }
        }
    }
    let app = App::parse(&source).unwrap_or(App::ClaudeCode);
    let bin = app.bin_name();
    let mut cmd = process::Command::new(bin);
    cmd.arg("--resume").arg(&session_id);
    if fork {
        cmd.arg("--fork-session");
    }
    let err = cmd.exec();
    eprintln!("Failed to exec {bin}: {err}");
    process::exit(1);
}

/// Get the current git branch name, or None if not in a repo.
fn current_git_branch() -> Option<String> {
    process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8(o.stdout).ok()?;
            let s = s.trim();
            if s.is_empty() || s == "HEAD" { None } else { Some(s.to_string()) }
        })
}

// --- Ingest ---

fn run_ingest(db_path: &Path, args: &cli::IngestArgs) -> Result<()> {
    let conn = db::open_db(db_path, true)?;
    let existing = db::get_content_hashes(&conn)?;

    let mut indexed: usize = 0;
    let mut skipped: usize = 0;
    let mut errors: usize = 0;

    let stdin = io::stdin();
    for (lineno, line) in stdin.lock().lines().enumerate() {
        let raw = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Line {}: read error -- {e}", lineno + 1);
                errors += 1;
                continue;
            }
        };
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            continue;
        }

        let record: IngestRecord = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Line {}: validation error -- {e}", lineno + 1);
                errors += 1;
                continue;
            }
        };

        // Source filter
        if let Some(ref source_filter) = args.source {
            if record.source != *source_filter {
                skipped += 1;
                continue;
            }
        }

        // Content hash dedup
        if let Some(ref hash) = record.content_hash {
            if let Some(existing_hash) = existing.get(&record.session_id) {
                if existing_hash == hash {
                    skipped += 1;
                    continue;
                }
            }
        }

        let sess = record.to_session();
        db::upsert_session(&conn, &sess, 0.0)?;
        indexed += 1;
    }

    eprint!("Done. {indexed} indexed, {skipped} skipped");
    if errors > 0 {
        eprint!(", {errors} errors");
    }
    eprintln!(".");
    Ok(())
}

// --- DB management ---

fn run_db_command(cmd: DbCommand, db_path: &Path) -> Result<()> {
    match cmd {
        DbCommand::Clean { force } => {
            if !db_path.exists() {
                eprintln!("No index found.");
                return Ok(());
            }
            if !force {
                eprint!("Delete index at {}? [y/N] ", db_path.display());
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("y") {
                    eprintln!("Aborted.");
                    return Ok(());
                }
            }
            std::fs::remove_file(db_path)?;
            eprintln!("Deleted {}", db_path.display());
        }
        DbCommand::Export { path } => {
            if !db_path.exists() {
                eprintln!("No index to export.");
                return Ok(());
            }
            let dest = if path.extension().is_none() && path.is_dir() {
                path.join(db_path.file_name().unwrap_or_default())
            } else {
                path.clone()
            };
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(db_path, &dest)?;
            eprintln!("Exported {} -> {}", db_path.display(), dest.display());
        }
        DbCommand::Import { path, force } => {
            if !path.exists() {
                anyhow::bail!("Source not found: {}", path.display());
            }
            if db_path.exists() && !force {
                eprint!("Overwrite existing index at {}? [y/N] ", db_path.display());
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("y") {
                    eprintln!("Aborted.");
                    return Ok(());
                }
            }
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, db_path)?;
            eprintln!("Imported {} -> {}", path.display(), db_path.display());
        }
    }
    Ok(())
}

// --- Schema ---

fn run_schema(as_json: bool) -> Result<()> {
    if as_json {
        let schema = build_json_schema();
        println!("{}", serde_json::to_string_pretty(&schema)?);
        return Ok(());
    }

    println!("IngestRecord -- canonical trs ingest format\n");
    println!("Required fields:");
    for field in INGEST_SCHEMA.iter().filter(|f| f.required) {
        println!(
            "  {:<16} {:<8} {}",
            field.name, field.type_name, field.description
        );
    }
    println!();
    println!("Optional fields:");
    for field in INGEST_SCHEMA.iter().filter(|f| !f.required) {
        println!(
            "  {:<16} {:<8} {}",
            field.name, field.type_name, field.description
        );
    }
    println!();
    println!("Extra fields: stored verbatim in metadata column (source-specific data).");
    println!();
    println!("Example:");
    println!(
        r#"  {{"session_id":"abc","source":"codex","body":"full text","content_hash":"sha256..."}}"#
    );

    Ok(())
}

fn build_json_schema() -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for field in INGEST_SCHEMA {
        let type_val = match field.type_name {
            "integer" => "integer",
            "array" => "array",
            _ => "string",
        };
        let mut prop = serde_json::Map::new();
        prop.insert("type".into(), serde_json::json!(type_val));
        prop.insert("description".into(), serde_json::json!(field.description));
        if field.type_name == "array" {
            prop.insert("items".into(), serde_json::json!({"type": "string"}));
        }
        properties.insert(field.name.to_string(), serde_json::Value::Object(prop));
        if field.required {
            required.push(serde_json::json!(field.name));
        }
    }

    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "IngestRecord",
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": true
    })
}

