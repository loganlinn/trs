use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use crate::search::DateFilter;
use crate::session::{SearchResult, Session};

/// Filter parameters shared by search and list queries.
#[derive(Debug, Default)]
pub struct SearchFilter<'a> {
    pub file_pat: Option<&'a str>,
    pub branch_pat: Option<&'a str>,
    pub project_pat: Option<&'a str>,
    pub source: Option<&'a str>,
    pub date: Option<&'a DateFilter>,
}

/// Build a SQL clause and parameter for a filter value.
///
/// - `value*` → prefix match: `column LIKE 'value%'`
/// - value containing `/` → exact match: `column = 'value'`
/// - plain name → substring match: `column LIKE '%value%'`
fn filter_clause(column: &str, value: &str) -> (String, String) {
    if let Some(prefix) = value.strip_suffix('*') {
        (format!("{column} LIKE ?"), format!("{prefix}%"))
    } else if value.contains('/') {
        (format!("{column} = ?"), value.to_string())
    } else {
        (format!("{column} LIKE ?"), format!("%{value}%"))
    }
}

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
    custom_title,
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
const CURRENT_VERSION: i64 = 5;

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
    (
        4,
        &[
            // Recreate FTS table with custom_title column for title-aware ranking
            "DROP TABLE IF EXISTS sessions_fts",
            "CREATE VIRTUAL TABLE sessions_fts USING fts5(
                session_id UNINDEXED,
                cwd,
                custom_title,
                git_branches,
                summary,
                first_message,
                files_touched,
                body,
                tokenize = 'porter unicode61'
            )",
            // Reset source_mtime to force full re-index (FTS schema changed)
            "UPDATE sessions SET source_mtime = 0",
        ],
    ),
    (
        5,
        &[
            // v4 pre-populated FTS from stale sessions data; clear and force re-index
            "DELETE FROM sessions_fts",
            "UPDATE sessions SET source_mtime = 0",
        ],
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
    let title_text = sess.custom_title.as_deref().unwrap_or("");
    conn.execute(
        "INSERT INTO sessions_fts
            (session_id, cwd, custom_title, git_branches, summary, first_message, files_touched, body)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        rusqlite::params![
            sess.session_id,
            sess.cwd,
            title_text,
            branches_json,
            sess.summary,
            sess.first_message,
            files_text,
            sess.body,
        ],
    )?;

    Ok(())
}

/// Map a database row from the `sessions` table to a `SearchResult`.
fn row_to_search_result(row: &rusqlite::Row) -> rusqlite::Result<SearchResult> {
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
        rank: row.get::<_, f64>("rank").unwrap_or(0.0),
    })
}

/// Look up a single session by exact session_id.
pub fn lookup_by_id(conn: &Connection, session_id: &str) -> Result<Option<SearchResult>> {
    let mut stmt = conn.prepare("SELECT * FROM sessions WHERE session_id = ?1")?;
    let mut rows = stmt.query_map([session_id], row_to_search_result)?;
    match rows.next() {
        Some(Ok(r)) => Ok(Some(r)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Execute an FTS5 search query with optional filters.
pub fn search(
    conn: &Connection,
    query: &str,
    filter: &SearchFilter,
    limit: i64,
) -> Result<Vec<SearchResult>> {
    let mut where_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(query.to_string())];

    if let Some(fp) = filter.file_pat {
        let (clause, param) = filter_clause("s.files_touched", fp);
        where_clauses.push(clause);
        params.push(Box::new(param));
    }
    if let Some(bp) = filter.branch_pat {
        let (clause, param) = filter_clause("s.git_branches", bp);
        where_clauses.push(clause);
        params.push(Box::new(param));
    }
    if let Some(pp) = filter.project_pat {
        let (clause, param) = filter_clause("s.cwd", pp);
        where_clauses.push(clause);
        params.push(Box::new(param));
    }
    if let Some(src) = filter.source {
        where_clauses.push("s.source = ?".to_string());
        params.push(Box::new(src.to_string()));
    }
    if let Some(df) = filter.date {
        where_clauses.push(format!("s.start_time {} ?", df.sql_op()));
        params.push(Box::new(df.sql_value()));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("AND {}", where_clauses.join(" AND "))
    };

    params.push(Box::new(limit));

    // Query 1: collect metadata-matching session IDs (fast, separate query).
    // If the column-filtered FTS syntax fails, fall back to empty set.
    let meta_query = format!(
        "{{cwd custom_title git_branches summary first_message files_touched}} : ({query})"
    );
    let meta_ids: std::collections::HashSet<String> = conn
        .prepare("SELECT session_id FROM sessions_fts WHERE sessions_fts MATCH ?1")
        .and_then(|mut stmt| {
            stmt.query_map([&meta_query], |row| row.get::<_, String>(0))?
                .collect::<Result<std::collections::HashSet<_>, _>>()
        })
        .unwrap_or_default();

    // Query 2: all matches with bm25 ranking + session data.
    // Two-tier ranking: metadata matches get -1e6 offset applied in Rust after fetch.
    // FTS columns: cwd(1), custom_title(2), git_branches(3), summary(4),
    //              first_message(5), files_touched(6), body(7)
    // bm25 weights: title 20x, cwd/summary 10x, branches/first_message 5x, files 3x, body 1x.
    let sql = format!(
        "SELECT s.*, bm25(sessions_fts, 10.0, 20.0, 5.0, 10.0, 5.0, 3.0, 1.0) AS rank
        FROM sessions_fts
        JOIN sessions s ON s.session_id = sessions_fts.session_id
        WHERE sessions_fts MATCH ?1
        {where_sql}
        ORDER BY rank
        LIMIT ?"
    );

    // Build param refs
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let mut results: Vec<SearchResult> = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    // Apply two-tier offset: metadata matches sort above body-only matches
    for r in &mut results {
        if meta_ids.contains(&r.session_id) {
            r.rank -= 1e6;
        }
    }
    results.sort_by(|a, b| a.rank.partial_cmp(&b.rank).unwrap());

    Ok(results)
}

/// List recent sessions ordered by start_time descending.
pub fn list_recent(
    conn: &Connection,
    limit: i64,
    filter: &SearchFilter,
) -> Result<Vec<SearchResult>> {
    let mut where_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(fp) = filter.file_pat {
        let (clause, param) = filter_clause("files_touched", fp);
        where_clauses.push(clause);
        params.push(Box::new(param));
    }
    if let Some(bp) = filter.branch_pat {
        let (clause, param) = filter_clause("git_branches", bp);
        where_clauses.push(clause);
        params.push(Box::new(param));
    }
    if let Some(pp) = filter.project_pat {
        let (clause, param) = filter_clause("cwd", pp);
        where_clauses.push(clause);
        params.push(Box::new(param));
    }
    if let Some(src) = filter.source {
        where_clauses.push("source = ?".to_string());
        params.push(Box::new(src.to_string()));
    }
    if let Some(df) = filter.date {
        where_clauses.push(format!("start_time {} ?", df.sql_op()));
        params.push(Box::new(df.sql_value()));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let sql = format!("SELECT * FROM sessions {where_sql} ORDER BY start_time DESC LIMIT ?");
    params.push(Box::new(limit));

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;
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

/// Append `*` to the last token for FTS5 prefix matching (for as-you-type search).
/// Only applies when the last token is >= 3 chars to avoid overly broad matches.
pub fn prefix_query(query: &str) -> String {
    let trimmed = query.trim_end();
    if trimmed.is_empty() || trimmed.ends_with('*') || trimmed.ends_with('"') {
        return query.to_string();
    }
    let last_token = trimmed.rsplit_once(' ').map(|(_, t)| t).unwrap_or(trimmed);
    if last_token.len() < 3 {
        return query.to_string();
    }
    format!("{trimmed}*")
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

        let results = search(&conn, "LaunchDarkly", &SearchFilter::default(), 10).unwrap();
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
    fn test_prefix_query() {
        assert_eq!(prefix_query("sess"), "sess*");
        assert_eq!(prefix_query("hello world"), "hello world*");
        assert_eq!(prefix_query("migrat*"), "migrat*");
        assert_eq!(prefix_query("\"exact phrase\""), "\"exact phrase\"");
        assert_eq!(prefix_query(""), "");
        // Short last tokens should NOT get prefix wildcard
        assert_eq!(prefix_query("a"), "a");
        assert_eq!(prefix_query("ab"), "ab");
        assert_eq!(prefix_query("hello a"), "hello a");
        assert_eq!(prefix_query("hello ab"), "hello ab");
        // 3+ chars should get wildcard
        assert_eq!(prefix_query("abc"), "abc*");
        assert_eq!(prefix_query("hello abc"), "hello abc*");
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
    fn test_lookup_by_id() {
        let conn = test_db();
        let sess = Session {
            session_id: "01020304-0506-0708-090a-0b0c0d0e0f10".into(),
            source: "claude-code".into(),
            cwd: "/home/user/project".into(),
            body: "some session content".into(),
            first_message: "hello".into(),
            ..Default::default()
        };
        upsert_session(&conn, &sess, 1.0).unwrap();

        let result = lookup_by_id(&conn, "01020304-0506-0708-090a-0b0c0d0e0f10").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().session_id, "01020304-0506-0708-090a-0b0c0d0e0f10");

        let result = lookup_by_id(&conn, "nonexistent-id").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_custom_title_searchable_and_ranked_higher() {
        let conn = test_db();
        // Session with "session" only in body
        let body_sess = Session {
            session_id: "body-1".into(),
            source: "claude-code".into(),
            body: "working on session management code".into(),
            ..Default::default()
        };
        // Session with "session" in custom_title
        let title_sess = Session {
            session_id: "title-1".into(),
            source: "claude-code".into(),
            custom_title: Some("session-names".into()),
            body: "some unrelated work".into(),
            ..Default::default()
        };
        upsert_session(&conn, &body_sess, 1.0).unwrap();
        upsert_session(&conn, &title_sess, 2.0).unwrap();

        let results = search(&conn, "session", &SearchFilter::default(), 10).unwrap();
        assert_eq!(results.len(), 2);
        // Title match should rank first (lower bm25 score = better)
        assert_eq!(results[0].session_id, "title-1");
        assert_eq!(results[1].session_id, "body-1");
    }

    #[test]
    fn test_prefix_search() {
        let conn = test_db();
        let sess = Session {
            session_id: "prefix-1".into(),
            source: "claude-code".into(),
            body: "implementing session management".into(),
            ..Default::default()
        };
        upsert_session(&conn, &sess, 1.0).unwrap();

        // Prefix query should match
        let results = search(&conn, "sess*", &SearchFilter::default(), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "prefix-1");

        // Partial without * should not match
        let results = search(&conn, "sess", &SearchFilter::default(), 10).unwrap();
        assert_eq!(results.len(), 0);
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
        let results = search(&conn, "rust", &SearchFilter { project_pat: Some("myapp"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 1);

        // Should not find with non-matching project filter
        let results = search(&conn, "rust", &SearchFilter { project_pat: Some("other"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_search_project_filter_exact_path() {
        let conn = test_db();
        let sess1 = Session {
            session_id: "proj-exact-1".into(),
            source: "claude-code".into(),
            cwd: "/home/user/gamma".into(),
            body: "working on gamma".into(),
            ..Default::default()
        };
        let sess2 = Session {
            session_id: "proj-exact-2".into(),
            source: "claude-code".into(),
            cwd: "/home/user/gamma/.worktrees/prettier".into(),
            body: "working on gamma worktree".into(),
            ..Default::default()
        };
        upsert_session(&conn, &sess1, 1.0).unwrap();
        upsert_session(&conn, &sess2, 1.0).unwrap();

        // Exact path: only the exact cwd match
        let results = search(&conn, "gamma", &SearchFilter { project_pat: Some("/home/user/gamma"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "proj-exact-1");

        // Wildcard path: matches children too
        let results = search(&conn, "gamma", &SearchFilter { project_pat: Some("/home/user/gamma*"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 2);

        // Plain name: substring match (both contain "gamma")
        let results = search(&conn, "gamma", &SearchFilter { project_pat: Some("gamma"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 2);
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
        let results = search(&conn, "feature", &SearchFilter::default(), 10).unwrap();
        assert_eq!(results.len(), 2);

        // Filter claude-code
        let results = search(&conn, "feature", &SearchFilter { source: Some("claude-code"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "claude-code");

        // Filter codex
        let results = search(&conn, "feature", &SearchFilter { source: Some("codex"), ..Default::default() }, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "codex");
    }
}
