use std::path::{Path, PathBuf};

use rdbx::{Database, DatabaseOptions, QueryResult, Value};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-p75-{tag}-{}-{}.rdbx",
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
fn schema_persists_across_reopen() {
    let path = unique_path("schema");
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        db.execute("CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
            .unwrap();
        db.execute("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')")
            .unwrap();
        db.close().unwrap();
    }

    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    match db.execute("SELECT * FROM users WHERE id = 2").unwrap() {
        QueryResult::Rows { rows, .. } => {
            assert_eq!(
                rows,
                vec![vec![Value::Int(2), Value::Bytes(b"bob".to_vec())]]
            );
        }
        other => panic!("expected Rows, got {other:?}"),
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn select_count_star() {
    let path = unique_path("count_star");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.execute("CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
        .unwrap();

    match db.execute("SELECT COUNT(*) FROM t").unwrap() {
        QueryResult::Rows { columns, rows } => {
            assert_eq!(columns, vec!["count".to_string()]);
            assert_eq!(rows, vec![vec![Value::Int(3)]]);
        }
        other => panic!("expected Rows, got {other:?}"),
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn select_count_with_filter() {
    let path = unique_path("count_filter");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.execute("CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
        .unwrap();

    match db.execute("SELECT COUNT(*) FROM t WHERE id > 1").unwrap() {
        QueryResult::Rows { columns, rows } => {
            assert_eq!(columns, vec!["count".to_string()]);
            assert_eq!(rows, vec![vec![Value::Int(2)]]);
        }
        other => panic!("expected Rows, got {other:?}"),
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn update_preserves_other_columns() {
    let path = unique_path("update_rmw");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.execute("CREATE TABLE t (id BIGINT PRIMARY KEY, a INT, b TEXT)")
        .unwrap();
    db.execute("INSERT INTO t (id, a, b) VALUES (1, 10, 'x'), (2, 20, 'y')")
        .unwrap();

    match db.execute("UPDATE t SET a = 99 WHERE id = 2").unwrap() {
        QueryResult::Affected(n) => assert_eq!(n, 1),
        other => panic!("expected Affected, got {other:?}"),
    }

    match db.execute("SELECT * FROM t WHERE id = 2").unwrap() {
        QueryResult::Rows { rows, .. } => {
            assert_eq!(
                rows[0],
                vec![Value::Int(2), Value::Int(99), Value::Bytes(b"y".to_vec())]
            );
        }
        other => panic!("expected Rows, got {other:?}"),
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn update_rejects_pk() {
    let path = unique_path("update_pk");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.execute("CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO t (id, name) VALUES (1, 'a')")
        .unwrap();

    let err = db.execute("UPDATE t SET id = 2 WHERE id = 1").unwrap_err();
    assert!(err.to_string().contains("PRIMARY KEY") || err.to_string().contains("not supported"));
    db.close().unwrap();
    cleanup(&path);
}
