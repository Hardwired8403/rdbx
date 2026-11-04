//! Phase 7 integration: SQL through Database::execute.

use std::path::{Path, PathBuf};

use rdbx::{Database, DatabaseOptions, QueryResult, Value};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-p7-{tag}-{}-{}.rdbx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn wal_dir(path: &Path) -> PathBuf {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("rdbx");
    path.parent()
        .map(|parent| parent.join(format!("{file_name}.wal")))
        .unwrap_or_else(|| PathBuf::from(format!("{file_name}.wal")))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(wal_dir(path));
}

#[test]
fn create_insert_select_roundtrip() {
    let path = unique_path("basic");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();

    db.execute("CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
        .unwrap();
    db.execute("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')")
        .unwrap();

    match db.execute("SELECT * FROM users").unwrap() {
        QueryResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[0],
                vec![Value::Int(1), Value::Bytes(b"alice".to_vec())]
            );
        }
        other => panic!("expected Rows, got {other:?}"),
    }

    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn select_with_where() {
    let path = unique_path("where");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.execute("CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
        .unwrap();

    match db.execute("SELECT * FROM t WHERE id = 2").unwrap() {
        QueryResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int(2));
        }
        other => panic!("expected Rows, got {other:?}"),
    }

    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn delete_by_primary_key() {
    let path = unique_path("del");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.execute("CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
        .unwrap();
    db.execute("DELETE FROM t WHERE id = 2").unwrap();

    match db.execute("SELECT * FROM t").unwrap() {
        QueryResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 2);
        }
        other => panic!("expected Rows, got {other:?}"),
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn unknown_table_errors() {
    let path = unique_path("err");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    assert!(db.execute("SELECT * FROM ghosts").is_err());
    db.close().unwrap();
    cleanup(&path);
}
