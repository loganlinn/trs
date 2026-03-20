mod cli;
mod config;
mod db;
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
use crate::config::FieldProfile;
use crate::session::{IngestRecord, INGEST_SCHEMA};

fn main() {
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
                // No query: if interactive, launch TUI; otherwise show help
                if !cli.no_tui && is_terminal::is_terminal(io::stdout()) {
                    if let Some(action) = tui::run()? {
                        exec_exit_action(action);
                    }
                    return Ok(0);
                }
                use clap::CommandFactory;
                Cli::command().print_help()?;
                return Ok(1);
            }

            // Auto-index unless --no-index
            if !args.no_index {
                indexer::run_index(&db_path, false)?;
            }

            let query = db::normalize_fts_query(&query_str);
            let (ctx_before, ctx_after) = args.effective_context();
            let found = search::run_search(
                &query,
                &db_path,
                args.file_pat.as_deref(),
                args.branch_pat.as_deref(),
                args.project_pat.as_deref(),
                args.limit,
                ctx_before,
                ctx_after,
                color,
            )?;
            Ok(if found { 0 } else { 2 })
        }
        Some(Command::Index(args)) => {
            indexer::run_index(&db_path, args.full)?;
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
        Some(Command::Profiles(args)) => {
            let cfg_path = args
                .config_path
                .unwrap_or_else(config::default_profiles_path);
            run_profiles(&cfg_path)?;
            Ok(0)
        }
        None => {
            // No subcommand: launch TUI if interactive, otherwise show help
            if !cli.no_tui && is_terminal::is_terminal(io::stdout()) {
                if let Some(action) = tui::run()? {
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

/// Replace the current process with `claude --resume <id>` (or --fork-session).
fn exec_exit_action(action: tui::ExitAction) -> ! {
    let (session_id, fork) = match action {
        tui::ExitAction::Resume(id) => (id, false),
        tui::ExitAction::Fork(id) => (id, true),
    };
    let mut cmd = process::Command::new("claude");
    cmd.arg("--resume").arg(&session_id);
    if fork {
        cmd.arg("--fork-session");
    }
    let err = cmd.exec();
    eprintln!("Failed to exec claude: {err}");
    process::exit(1);
}

// --- Ingest ---

fn run_ingest(db_path: &Path, args: &cli::IngestArgs) -> Result<()> {
    let resolved_profile: Option<FieldProfile> = if let Some(ref profile_name) = args.profile {
        let cfg_path = args
            .config_path
            .clone()
            .unwrap_or_else(config::default_profiles_path);
        let cfg = config::load_profiles(&cfg_path)?;
        let profile = cfg
            .profiles
            .get(profile_name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Profile {:?} not found in {}",
                    profile_name,
                    cfg_path.display()
                )
            })?
            .clone();
        Some(profile)
    } else {
        None
    };

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

        let record: IngestRecord = if let Some(ref profile) = resolved_profile {
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(serde_json::Value::Object(map)) => {
                    let mapped = config::apply_profile(&map, profile);
                    match serde_json::from_value(serde_json::Value::Object(mapped)) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("Line {}: validation error -- {e}", lineno + 1);
                            errors += 1;
                            continue;
                        }
                    }
                }
                Ok(_) => {
                    eprintln!("Line {}: expected JSON object", lineno + 1);
                    errors += 1;
                    continue;
                }
                Err(e) => {
                    eprintln!("Line {}: parse error -- {e}", lineno + 1);
                    errors += 1;
                    continue;
                }
            }
        } else {
            match serde_json::from_str(&raw) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Line {}: validation error -- {e}", lineno + 1);
                    errors += 1;
                    continue;
                }
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

// --- Profiles ---

fn run_profiles(cfg_path: &Path) -> Result<()> {
    let cfg = config::load_profiles(cfg_path)?;
    if cfg.profiles.is_empty() {
        eprintln!("No profiles found in {}", cfg_path.display());
        return Ok(());
    }
    eprintln!(
        "{} profile(s) in {}\n",
        cfg.profiles.len(),
        cfg_path.display()
    );
    for (name, prof) in &cfg.profiles {
        let src = prof
            .source
            .as_deref()
            .map(|s| format!("source={s}"))
            .unwrap_or_else(|| "--".into());
        let field_count = prof.fields.len();
        let default_count = prof.defaults.len();
        let mut parts = vec![format!("{name:<16}"), format!("{src:<20}")];
        if field_count > 0 {
            let s = if field_count != 1 { "s" } else { "" };
            parts.push(format!("{field_count} field mapping{s}"));
        }
        if default_count > 0 {
            let s = if default_count != 1 { "s" } else { "" };
            parts.push(format!("{default_count} default{s}"));
        }
        println!("  {}", parts.join("  "));
    }
    Ok(())
}
