//! Parser tests.

use rdbx_sql::ast::{BinOp, DataType, Expr, SelectItem, Statement};
use rdbx_sql::parser::Parser;

#[test]
fn create_table_with_pk() {
    let stmt = Parser::parse(
        "CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL, email TEXT UNIQUE)",
    )
    .unwrap();
    match stmt {
        Statement::CreateTable { name, columns } => {
            assert_eq!(name, "users");
            assert_eq!(columns.len(), 3);
            assert_eq!(columns[0].ty, DataType::BigInt);
            assert!(
                columns[0]
                    .constraints
                    .iter()
                    .any(|c| matches!(c, rdbx_sql::ast::Constraint::PrimaryKey))
            );
            assert!(
                columns[1]
                    .constraints
                    .iter()
                    .any(|c| matches!(c, rdbx_sql::ast::Constraint::NotNull))
            );
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn insert_parse() {
    let stmt =
        Parser::parse("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").unwrap();
    match stmt {
        Statement::Insert {
            table,
            columns,
            values,
        } => {
            assert_eq!(table, "users");
            assert_eq!(columns.unwrap(), vec!["id", "name"]);
            assert_eq!(values.len(), 2);
            assert_eq!(values[0].len(), 2);
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn select_with_where_and_order() {
    let stmt = Parser::parse("SELECT id, name FROM users WHERE id > 10 ORDER BY name DESC LIMIT 5")
        .unwrap();
    match stmt {
        Statement::Select {
            items,
            table,
            filter,
            order_by,
            limit,
        } => {
            assert_eq!(items.len(), 2);
            assert_eq!(table, "users");
            assert!(filter.is_some());
            assert_eq!(order_by, vec![("name".into(), false)]);
            assert_eq!(limit, Some(5));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn select_wildcard() {
    let stmt = Parser::parse("SELECT * FROM t").unwrap();
    match stmt {
        Statement::Select { items, .. } => {
            assert_eq!(items, vec![SelectItem::Wildcard]);
        }
        _ => panic!("expected Select"),
    }
}

#[test]
fn expression_precedence() {
    let stmt = Parser::parse("SELECT 1 FROM t WHERE 1 + 2 * 3 = 7 AND x = 1").unwrap();
    // We don't deeply check the tree; we just want the parser to accept
    // the precedence without errors.
    match stmt {
        Statement::Select { filter, .. } => {
            assert!(matches!(filter, Some(Expr::Binary(_, BinOp::And, _))));
        }
        _ => panic!("expected Select"),
    }
}

#[test]
fn parse_errors_are_reported() {
    assert!(Parser::parse("SELEKT * FROM t").is_err());
    assert!(Parser::parse("CREATE TABLE").is_err());
    assert!(Parser::parse("INSERT INTO t VALUES (1,").is_err());
}

#[test]
fn transaction_statements() {
    assert_eq!(Parser::parse("BEGIN").unwrap(), Statement::Begin);
    assert_eq!(Parser::parse("COMMIT").unwrap(), Statement::Commit);
    assert_eq!(Parser::parse("ROLLBACK").unwrap(), Statement::Rollback);
}
