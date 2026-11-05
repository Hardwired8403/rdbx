//! Prepared statements support for `?` placeholders.

use rdbx_query::value::Value;

use crate::ast::{Expr, Statement};
use crate::error::{SqlError, SqlResult};

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

    pub fn bind(&mut self, idx: usize, value: Value) -> SqlResult<()> {
        if idx >= self.plan.arity {
            return Err(SqlError::Semantic(format!(
                "bind index {idx} out of range (arity {})",
                self.plan.arity
            )));
        }
        self.args[idx] = Some(value);
        Ok(())
    }

    pub fn execute(&mut self) -> SqlResult<Statement> {
        let final_stmt = self.substitute_placeholders(&self.plan.template.clone())?;
        Ok(final_stmt)
    }

    fn substitute_placeholders(&self, stmt: &Statement) -> SqlResult<Statement> {
        match stmt {
            Statement::Select {
                items,
                table,
                filter,
                order_by,
                limit,
                group_by,
                having,
            } => {
                let mut new_items = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        crate::ast::SelectItem::Wildcard => {
                            new_items.push(crate::ast::SelectItem::Wildcard)
                        }
                        crate::ast::SelectItem::Expr { expr, alias } => {
                            new_items.push(crate::ast::SelectItem::Expr {
                                expr: self.substitute_expr(expr)?,
                                alias: alias.clone(),
                            });
                        }
                    }
                }
                let new_filter = filter
                    .as_ref()
                    .map(|e| self.substitute_expr(e))
                    .transpose()?;
                let new_having = having
                    .as_ref()
                    .map(|e| self.substitute_expr(e))
                    .transpose()?;
                Ok(Statement::Select {
                    items: new_items,
                    table: table.clone(),
                    filter: new_filter,
                    order_by: order_by.clone(),
                    limit: *limit,
                    group_by: group_by.clone(),
                    having: new_having,
                })
            }
            Statement::Insert {
                table,
                columns,
                values,
            } => {
                let mut new_values = Vec::with_capacity(values.len());
                for row in values {
                    let mut new_row = Vec::with_capacity(row.len());
                    for expr in row {
                        new_row.push(self.substitute_expr(expr)?);
                    }
                    new_values.push(new_row);
                }
                Ok(Statement::Insert {
                    table: table.clone(),
                    columns: columns.clone(),
                    values: new_values,
                })
            }
            Statement::Update {
                table,
                assignments,
                filter,
            } => {
                let mut new_assignments = Vec::with_capacity(assignments.len());
                for (name, expr) in assignments {
                    new_assignments.push((name.clone(), self.substitute_expr(expr)?));
                }
                let new_filter = filter
                    .as_ref()
                    .map(|e| self.substitute_expr(e))
                    .transpose()?;
                Ok(Statement::Update {
                    table: table.clone(),
                    assignments: new_assignments,
                    filter: new_filter,
                })
            }
            Statement::Delete { table, filter } => {
                let new_filter = filter
                    .as_ref()
                    .map(|e| self.substitute_expr(e))
                    .transpose()?;
                Ok(Statement::Delete {
                    table: table.clone(),
                    filter: new_filter,
                })
            }
            Statement::CreateIndex {
                table,
                columns,
                name,
            } => Ok(Statement::CreateIndex {
                table: table.clone(),
                columns: columns.clone(),
                name: name.clone(),
            }),
            Statement::CreateTable { .. }
            | Statement::DropTable { .. }
            | Statement::DropIndex { .. }
            | Statement::Begin
            | Statement::Commit
            | Statement::Rollback => Ok(stmt.clone()),
        }
    }

    fn substitute_expr(&self, expr: &Expr) -> SqlResult<Expr> {
        Ok(match expr {
            Expr::Placeholder(idx) => match self.args.get(*idx as usize).and_then(|v| v.as_ref()) {
                Some(v) => match v {
                    Value::Null => Expr::Null,
                    Value::Bool(b) => Expr::Bool(*b),
                    Value::Int(i) => Expr::Int(*i),
                    Value::Float(f) => Expr::Float(*f),
                    Value::Bytes(b) => Expr::Str(String::from_utf8_lossy(b).into_owned()),
                },
                None => {
                    return Err(SqlError::Semantic(format!(
                        "missing bound value for parameter {idx}"
                    )));
                }
            },
            Expr::Binary(l, op, r) => Expr::Binary(
                Box::new(self.substitute_expr(l)?),
                *op,
                Box::new(self.substitute_expr(r)?),
            ),
            Expr::Unary(op, inner) => Expr::Unary(*op, Box::new(self.substitute_expr(inner)?)),
            Expr::Call(name, args) => {
                let mut out = Vec::with_capacity(args.len());
                for arg in args {
                    out.push(self.substitute_expr(arg)?);
                }
                Expr::Call(name.clone(), out)
            }
            Expr::Subquery(sub) => Expr::Subquery(Box::new(self.substitute_placeholders(sub)?)),
            Expr::InSubquery(lhs, sub) => Expr::InSubquery(
                Box::new(self.substitute_expr(lhs)?),
                Box::new(self.substitute_placeholders(sub)?),
            ),
            other => other.clone(),
        })
    }
}
