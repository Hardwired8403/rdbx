//! Planner tests: AST → LogicalPlan.

use rdbx_core::PageId;
use rdbx_query::LogicalPlan;
use rdbx_sql::catalog::{SqlCatalog, TableSchema};
use rdbx_sql::executor::compile;
use rdbx_sql::parser::Parser;

fn users_catalog() -> SqlCatalog {
    let defs = vec![
        rdbx_sql::ast::ColumnDef {
            name: "id".into(),
            ty: rdbx_sql::ast::DataType::BigInt,
            constraints: vec![rdbx_sql::ast::Constraint::PrimaryKey],
        },
        rdbx_sql::ast::ColumnDef {
            name: "name".into(),
            ty: rdbx_sql::ast::DataType::Text,
            constraints: vec![rdbx_sql::ast::Constraint::NotNull],
        },
    ];
    let mut cat = SqlCatalog::new();
    cat.insert(TableSchema::from_defs("users", &defs).unwrap());
    cat
}

fn root_of(_: &str) -> Option<PageId> {
    Some(PageId(2))
}

#[test]
fn select_star_plan() {
    let stmt = Parser::parse("SELECT * FROM users").unwrap();
    let plan = compile(&stmt, &users_catalog(), root_of).unwrap();
    assert!(matches!(plan, LogicalPlan::Scan { .. }));
}

#[test]
fn select_columns_with_filter() {
    let stmt = Parser::parse("SELECT name FROM users WHERE id = 1").unwrap();
    let plan = compile(&stmt, &users_catalog(), root_of).unwrap();
    match plan {
        LogicalPlan::Project { input, columns } => {
            assert_eq!(columns, vec![1]); // name is column 1
            assert!(matches!(*input, LogicalPlan::Filter { .. }));
        }
        other => panic!("expected Project, got {other:?}"),
    }
}

#[test]
fn insert_plan_encodes_row() {
    let stmt = Parser::parse("INSERT INTO users (id, name) VALUES (1, 'alice')").unwrap();
    let plan = compile(&stmt, &users_catalog(), root_of).unwrap();
    match plan {
        LogicalPlan::Insert { root, rows } => {
            assert_eq!(root, PageId(2));
            assert_eq!(rows.len(), 1);
            let (k, v) = &rows[0];
            assert!(!k.is_empty());
            assert!(!v.is_empty());
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn delete_plan_needs_pk_filter() {
    let stmt = Parser::parse("DELETE FROM users WHERE id = 5").unwrap();
    let plan = compile(&stmt, &users_catalog(), root_of).unwrap();
    assert!(matches!(plan, LogicalPlan::Delete { .. }));

    let no_filter = Parser::parse("DELETE FROM users").unwrap();
    assert!(compile(&no_filter, &users_catalog(), root_of).is_err());
}

#[test]
fn unknown_table_is_error() {
    let stmt = Parser::parse("SELECT * FROM ghosts").unwrap();
    assert!(compile(&stmt, &users_catalog(), root_of).is_err());
}

#[test]
fn unknown_column_is_error() {
    let stmt = Parser::parse("SELECT ghost FROM users").unwrap();
    assert!(compile(&stmt, &users_catalog(), root_of).is_err());
}
