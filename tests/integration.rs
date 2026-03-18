use std::path::PathBuf;

/// Get path to test fixtures.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn test_parse_sample_session() {
    // We test parse_session by using the public binary's module.
    // Since modules are private to main.rs, we test via the fixture directly.
    let fixture = fixtures_dir().join("sample-session.jsonl");
    assert!(fixture.exists(), "fixture file should exist");

    // Read and verify the JSONL is valid
    let content = std::fs::read_to_string(&fixture).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 5);

    // Verify each line is valid JSON
    for (i, line) in lines.iter().enumerate() {
        let val: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {}: {e}", i + 1));
        assert!(val.is_object());
    }
}

#[test]
fn test_cli_help_exits_zero() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .arg("--help")
        .output()
        .expect("failed to run trs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Full-text search over chat session transcripts"));
    assert!(stdout.contains("query"));
    assert!(stdout.contains("index"));
    assert!(stdout.contains("ingest"));
}

#[test]
fn test_cli_version() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .arg("--version")
        .output()
        .expect("failed to run trs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("trs"));
}

#[test]
fn test_schema_command() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .arg("schema")
        .output()
        .expect("failed to run trs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("session_id"));
    assert!(stdout.contains("Required fields"));
    assert!(stdout.contains("Optional fields"));
}

#[test]
fn test_schema_json_command() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .args(["schema", "--json"])
        .output()
        .expect("failed to run trs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(val["title"], "IngestRecord");
    assert!(val["properties"]["session_id"].is_object());
}

#[test]
fn test_index_with_temp_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .args(["index", "-d", db_path.to_str().unwrap()])
        .output()
        .expect("failed to run trs");

    // Should succeed (may index 0 sessions in test env)
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_db_clean_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("nonexistent.db");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .args(["db", "clean", "--force", "-d", db_path.to_str().unwrap()])
        .output()
        .expect("failed to run trs");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No index found"));
}

#[test]
fn test_query_no_index() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trs"))
        .args([
            "query",
            "--no-index",
            "-d",
            db_path.to_str().unwrap(),
            "hello",
        ])
        .output()
        .expect("failed to run trs");

    // Exit code 2 = no results (or index not found)
    assert!(!output.status.success());
}
