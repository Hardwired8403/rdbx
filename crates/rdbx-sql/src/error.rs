//! SQL error type.

use std::fmt;

/// All errors produced by the SQL front-end.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlError {
    /// Lexer encountered an unexpected character.
    Lex { pos: usize, ch: char },
    /// Parser failed to produce a statement.
    Parse { pos: usize, message: String },
    /// Semantic error (unknown table, column, type mismatch, etc.).
    Semantic(String),
    /// Feature recognised by the parser but not implemented.
    Unimplemented(String),
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlError::Lex { pos, ch } => write!(f, "lex error at {pos}: unexpected {ch:?}"),
            SqlError::Parse { pos, message } => {
                write!(f, "parse error at {pos}: {message}")
            }
            SqlError::Semantic(m) => write!(f, "semantic error: {m}"),
            SqlError::Unimplemented(m) => write!(f, "unimplemented: {m}"),
        }
    }
}

impl std::error::Error for SqlError {}

pub type SqlResult<T> = Result<T, SqlError>;
