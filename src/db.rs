use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use crate::session::{SearchResult, Session};

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    session_id    TEXT PRIMARY KEY,
    source        TEXT NOT NULL DEFAULT 'claude-code',
    cwd           TEXT,
    slug          TEXT,
    git_branches  TEXT,
    start_time    TEXT,
    end_time      TEXT,
    files_touched TEXT,
    tools_used    TEXT,
    message_count INTEGER DEFAULT 0,
    first_message TEXT,
    summary       TEXT,
    indexed_at    TEXT NOT NULL,
    source_mtime  REAL NOT NULL,
    content_hash  TEXT,
    metadata      TEXT
);

CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
    session_id   UNINDEXED,
    cwd,
    git_branches,
    summary,
    first_message,
    files_touched,
    body,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);
";

#[allow(dead_code)]
const CURRENT_VERSION: i64 = 3;

const MIGRATIONS: &[(i64, &[&str])] = &[
    (
        2,
        &[
            "ALTER TABLE sessions ADD COLUMN content_hash TEXT",
            "ALTER TABLE sessions ADD COLUMN metadata TEXT",
        ],
    ),
    (
        3,
        &["ALTER TABLE sessions ADD COLUMN custom_title TEXT"],
    ),
];

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY)",
        [],
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;

    for &(version, stmts) in MIGRATIONS {
        if version <= current {
            continue;
        }
        for stmt in stmts {
            // Ignore errors (e.g. duplicate column)
            let _ = conn.execute(stmt, []);
        }
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
            [version],
        )?;
    }
    Ok(())
}

/// Open (and optionally initialize) the database.
pub fn open_db(path: &Path, init: bool) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating db directory {}", parent.display()))?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("opening database {}", path.display()))?;
    if init {
        conn.execute_batch(SCHEMA)
            .context("initializing database schema")?;
    }
    run_migrations(&conn)?;
    Ok(conn)
}

/// Insert or update a session in both the sessions table and FTS index.
pub fn upsert_session(conn: &Connection, sess: &Session, mtime: f64) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let branches_json = serde_json::to_string(&sess.git_branches)?;
    let files_json = serde_json::to_string(&sess.files_touched)?;
    let tools_json = serde_json::to_string(&sess.tools_used)?;
    let metadata_json = if sess.metadata.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&sess.metadata)?)
    };

    conn.execute(
        "INSERT INTO sessions
            (session_id, source, cwd, slug, git_branches, start_time, end_time,
             files_touched, tools_used, message_count, first_message, summary,
             indexed_at, source_mtime, content_hash, custom_title, metadata)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
        ON CONFLICT(session_id) DO UPDATE SET
            source=excluded.source, cwd=excluded.cwd, slug=excluded.slug,
            git_branches=excluded.git_branches, start_time=excluded.start_time,
            end_time=excluded.end_time, files_touched=excluded.files_touched,
            tools_used=excluded.tools_used, message_count=excluded.message_count,
            first_message=excluded.first_message, summary=excluded.summary,
            indexed_at=excluded.indexed_at, source_mtime=excluded.source_mtime,
            content_hash=excluded.content_hash, custom_title=excluded.custom_title,
            metadata=excluded.metadata",
        rusqlite::params![
            sess.session_id,
            sess.source,
            sess.cwd,
            sess.slug,
            branches_json,
            sess.start_time,
            sess.end_time,
            files_json,
            tools_json,
            sess.message_count,
            sess.first_message,
            sess.summary,
            now,
            mtime,
            sess.content_hash,
            sess.custom_title,
            metadata_json,
        ],
    )?;

    conn.execute(
        "DELETE FROM sessions_fts WHERE session_id = ?1",
        [&sess.session_id],
    )?;

    let files_text = sess.files_touched.join(" ");
    conn.execute(
        "INSERT INTO sessions_fts
            (session_id, cwd, git_branches, summary, first_message, files_touched, body)
        VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![
            sess.session_id,
            sess.cwd,
            branches_json,
            sess.summary,
            sess.first_message,
            files_text,
            sess.body,
        ],
    )?;

    Ok(())
}

/// Execute an FTS5 search query with optional filters.
pub fn search(
    conn: &Connection,
    query: &str,
    file_pat: Option<&str>,
    branch_pat: Option<&str>,
    project_pat: Option<&str>,
    source_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<SearchResult>> {
    let mut where_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(query.to_string())];

    if let Some(fp) = file_pat {
        where_clauses.push("s.files_touched LIKE ?".to_string());
        params.push(Box::new(format!("%{fp}%")));
    }
    if let Some(bp) = branch_pat {
        where_clauses.push("s.git_branches LIKE ?".to_string());
        params.push(Box::new(format!("%{bp}%")));
    }
    if let Some(pp) = project_pat {
        where_clauses.push("s.cwd LIKE ?".to_string());
        params.push(Box::new(format!("%{pp}%")));
    }
    if let Some(src) = source_filter {
        where_clauses.push("s.source = ?".to_string());
        params.push(Box::new(src.to_string()));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    params.push(Box::new(limit));

    let sql = format!(
        "WITH matches AS (
            SELECT session_id, rank
            FROM sessions_fts
            WHERE sessions_fts MATCH ?1
        )
        SELECT s.*, m.rank
        FROM matches m
        JOIN sessions s USING (session_id)
        {where_sql}
        ORDER BY m.rank
        LIMIT ?"
    );

    // Build param refs
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                session_id: row.get("session_id")?,
                source: row.get::<_, String>("source").unwrap_or_default(),
                cwd: row.get::<_, String>("cwd").unwrap_or_default(),
                slug: row.get::<_, String>("slug").unwrap_or_default(),
                git_branches: row.get::<_, String>("git_branches").unwrap_or_default(),
                start_time: row.get::<_, String>("start_time").unwrap_or_default(),
                end_time: row.get::<_, String>("end_time").unwrap_or_default(),
                files_touched: row.get::<_, String>("files_touched").unwrap_or_default(),
                tools_used: row.get::<_, String>("tools_used").unwrap_or_default(),
                message_count: row.get::<_, i64>("message_count").unwrap_or_default(),
                first_message: row.get::<_, String>("first_message").unwrap_or_default(),
                summary: row.get::<_, String>("summary").unwrap_or_default(),
                content_hash: row
                    .get::<_, Option<String>>("content_hash")
                    .unwrap_or_default(),
                custom_title: row
                    .get::<_, Option<String>>("custom_title")
                    .unwrap_or_default(),
                metadata: row.get::<_, Option<String>>("metadata").unwrap_or_default(),
                rank: row.get::<_, f64>("rank").unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// List recent sessions ordered by start_time descending.
pub fn list_recent(conn: &Connection, limit: i64, source_filter: Option<&str>) -> Result<Vec<SearchResult>> {
    let sql = if source_filter.is_some() {
        "SELECT * FROM sessions WHERE source = ?2 ORDER BY start_time DESC LIMIT ?1"
    } else {
        "SELECT * FROM sessions ORDER BY start_time DESC LIMIT ?1"
    };
    let mut stmt = conn.prepare(sql)?;
    let param_fn = |row: &rusqlite::Row| -> rusqlite::Result<SearchResult> {
        Ok(SearchResult {
            session_id: row.get("session_id")?,
            source: row.get::<_, String>("source").unwrap_or_default(),
            cwd: row.get::<_, String>("cwd").unwrap_or_default(),
            slug: row.get::<_, String>("slug").unwrap_or_default(),
            git_branches: row.get::<_, String>("git_branches").unwrap_or_default(),
            start_time: row.get::<_, String>("start_time").unwrap_or_default(),
            end_time: row.get::<_, String>("end_time").unwrap_or_default(),
            files_touched: row.get::<_, String>("files_touched").unwrap_or_default(),
            tools_used: row.get::<_, String>("tools_used").unwrap_or_default(),
            message_count: row.get::<_, i64>("message_count").unwrap_or_default(),
            first_message: row.get::<_, String>("first_message").unwrap_or_default(),
            summary: row.get::<_, String>("summary").unwrap_or_default(),
            content_hash: row
                .get::<_, Option<String>>("content_hash")
                .unwrap_or_default(),
            custom_title: row
                .get::<_, Option<String>>("custom_title")
                .unwrap_or_default(),
            metadata: row.get::<_, Option<String>>("metadata").unwrap_or_default(),
            rank: 0.0,
        })
    };
    let rows = if let Some(src) = source_filter {
        stmt.query_map(rusqlite::params![limit, src], param_fn)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([limit], param_fn)?
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

/// Get stored session_id -> source_mtime for incremental indexing.
pub fn get_stored_mtimes(conn: &Connection) -> Result<std::collections::HashMap<String, f64>> {
    let mut stmt = conn.prepare("SELECT session_id, source_mtime FROM sessions")?;
    let map = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let mtime: f64 = row.get(1)?;
            Ok((id, mtime))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(map)
}

/// Get session IDs that are file-backed (mtime > 0).
pub fn get_file_backed_ids(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT session_id FROM sessions WHERE source_mtime > 0.0")?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}

/// Get existing content hashes for dedup during ingest.
pub fn get_content_hashes(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn
        .prepare("SELECT session_id, content_hash FROM sessions WHERE content_hash IS NOT NULL")?;
    let map = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let hash: String = row.get(1)?;
            Ok((id, hash))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(map)
}

/// Delete sessions by IDs (for pruning dead sessions).
pub fn delete_sessions(conn: &Connection, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders: String = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");

    let params: Vec<&dyn rusqlite::types::ToSql> = ids
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();

    conn.execute(
        &format!("DELETE FROM sessions WHERE session_id IN ({placeholders})"),
        params.as_slice(),
    )?;
    conn.execute(
        &format!("DELETE FROM sessions_fts WHERE session_id IN ({placeholders})"),
        params.as_slice(),
    )?;
    Ok(())
}

/// Quote hyphenated tokens so FTS5 doesn't interpret `-` as NOT.
pub fn normalize_fts_query(query: &str) -> String {
    if query.contains('"') {
        return query.to_string();
    }
    query
        .split_whitespace()
        .map(|token| {
            if !token.starts_with('-') && token.contains('-') {
                format!("\"{token}\"")
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn test_schema_creation() {
        let conn = test_db();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_upsert_and_search() {
        let conn = test_db();
        let sess = Session {
            session_id: "test-123".into(),
            source: "claude-code".into(),
            cwd: "/home/user/project".into(),
            slug: "my-project".into(),
            body: "implementing a LaunchDarkly migration script".into(),
            first_message: "help me migrate".into(),
            message_count: 3,
            ..Default::default()
        };
        upsert_session(&conn, &sess, 1234.0).unwrap();

        let results = search(&conn, "LaunchDarkly", None, None, None, None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "test-123");
    }

    #[test]
    fn test_upsert_idempotent() {
        let conn = test_db();
        let sess = Session {
            session_id: "dup-1".into(),
            source: "test".into(),
            body: "some text".into(),
            ..Default::default()
        };
        upsert_session(&conn, &sess, 1.0).unwrap();
        upsert_session(&conn, &sess, 2.0).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // FTS should also have exactly one row
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts_count, 1);
    }

    #[test]
    fn test_normalize_fts_query() {
        assert_eq!(normalize_fts_query("hello world"), "hello world");
        assert_eq!(normalize_fts_query("vue-router"), "\"vue-router\"");
        assert_eq!(normalize_fts_query("-excluded"), "-excluded");
        assert_eq!(
            normalize_fts_query("already \"quoted\""),
            "already \"quoted\""
        );
    }

    #[test]
    fn test_delete_sessions() {
        let conn = test_db();
        let sess = Session {
            session_id: "del-1".into(),
            source: "test".into(),
            body: "delete me".into(),
            ..Default::default()
        };
        upsert_session(&conn, &sess, 1.0).unwrap();
        delete_sessions(&conn, &["del-1".to_string()]).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_search_with_filters() {
        let conn = test_db();
        let sess = Session {
            session_id: "filter-1".into(),
            source: "claude-code".into(),
            cwd: "/home/user/myapp".into(),
            git_branches: vec!["feature-branch".into()],
            files_touched: vec!["src/main.rs".into()],
            body: "working on the rust port".into(),
            ..Default::default()
        };
        upsert_session(&conn, &sess, 1.0).unwrap();

        // Should find with matching project filter
        let results = search(&conn, "rust", None, None, Some("myapp"), None, 10).unwrap();
        assert_eq!(results.len(), 1);

        // Should not find with non-matching project filter
        let results = search(&conn, "rust", None, None, Some("other"), None, 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_search_source_filter() {
        let conn = test_db();
        let claude_sess = Session {
            session_id: "claude-1".into(),
            source: "claude-code".into(),
            body: "working on feature X".into(),
            ..Default::default()
        };
        let codex_sess = Session {
            session_id: "codex-1".into(),
            source: "codex".into(),
            body: "working on feature X".into(),
            ..Default::default()
        };
        upsert_session(&conn, &claude_sess, 1.0).unwrap();
        upsert_session(&conn, &codex_sess, 1.0).unwrap();

        // No filter: both
        let results = search(&conn, "feature", None, None, None, None, 10).unwrap();
        assert_eq!(results.len(), 2);

        // Filter claude-code
        let results = search(&conn, "feature", None, None, None, Some("claude-code"), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "claude-code");

        // Filter codex
        let results = search(&conn, "feature", None, None, None, Some("codex"), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "codex");
    }
}
