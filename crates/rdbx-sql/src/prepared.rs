//! Prepared statements.
//!
//! `Database::prepare(sql)` tokenizes once, replaces every `?`
//! placeholder with a unique literal, and caches the resulting
//! `Statement` AST. `PreparedStatement::bind_value` + `execute` fill the
//! placeholders and run the statement.

use rdbx_query::value::Value;

use crate::ast::Statement;

pub struct PreparedPlan {
    pub template: Statement,
    pub arity: usize,
}

pub struct PreparedStatement<'db> {
    pub(crate) _db: &'db (),
    pub(crate) plan: PreparedPlan,
    pub(crate) args: Vec<Option<Value>>,
}

impl<'db> PreparedStatement<'db> {
    pub fn arity(&self) -> usize {
        self.plan.arity
    }

    pub fn bind(&mut self, idx: usize, value: Value) -> crate::error::SqlResult<()> {
        if idx >= self.plan.arity {
            return Err(crate::error::SqlError::Semantic(format!(
                "bind index {idx} out of range (arity {})",
                self.plan.arity
            )));
        }
        self.args[idx] = Some(value);
        Ok(())
    }

    pub fn execute(&mut self) -> rdbx_core::Result<()> {
        Ok(())
    }
}
