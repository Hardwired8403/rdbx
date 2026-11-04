//! Recursive-descent parser.

use crate::ast::{BinOp, ColumnDef, Constraint, DataType, Expr, SelectItem, Statement, UnaryOp};
use crate::error::{SqlError, SqlResult};
use crate::lexer::{Lexer, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(sql: &str) -> SqlResult<Self> {
        let tokens = Lexer::new(sql).tokenize()?;
        Ok(Self { tokens, pos: 0 })
    }

    pub fn parse(sql: &str) -> SqlResult<Statement> {
        let mut p = Parser::new(sql)?;
        let stmt = p.parse_statement()?;
        p.expect_eof()?;
        Ok(stmt)
    }

    fn peek(&self) -> &TokenKind {
        const EOF: TokenKind = TokenKind::Eof;
        self.tokens.get(self.pos).map(|t| &t.kind).unwrap_or(&EOF)
    }

    fn advance(&mut self) -> TokenKind {
        let kind = self
            .tokens
            .get(self.pos)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        kind
    }

    fn pos(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|t| t.pos)
            .unwrap_or_else(|| self.tokens.last().map(|t| t.pos + 1).unwrap_or(0))
    }

    fn expect(&mut self, expected: &TokenKind) -> SqlResult<()> {
        if self.peek() == expected {
            self.pos += 1;
            Ok(())
        } else {
            Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("expected {expected:?}, found {:?}", self.peek()),
            })
        }
    }

    fn expect_ident(&mut self) -> SqlResult<String> {
        match self.peek().clone() {
            TokenKind::Ident(s) => {
                self.pos += 1;
                Ok(s)
            }
            other => Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("expected identifier, found {other:?}"),
            }),
        }
    }

    fn expect_eof(&mut self) -> SqlResult<()> {
        while self.peek() == &TokenKind::Semicolon {
            self.pos += 1;
        }
        if self.peek() == &TokenKind::Eof {
            Ok(())
        } else {
            Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("expected end of statement, found {:?}", self.peek()),
            })
        }
    }

    fn parse_statement(&mut self) -> SqlResult<Statement> {
        match self.peek() {
            TokenKind::Create => self.parse_create(),
            TokenKind::Drop => self.parse_drop(),
            TokenKind::Insert => self.parse_insert(),
            TokenKind::Select => self.parse_select(),
            TokenKind::Update => self.parse_update(),
            TokenKind::Delete => self.parse_delete(),
            TokenKind::Begin => {
                self.advance();
                Ok(Statement::Begin)
            }
            TokenKind::Commit => {
                self.advance();
                Ok(Statement::Commit)
            }
            TokenKind::Rollback => {
                self.advance();
                Ok(Statement::Rollback)
            }
            other => Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("unexpected token {other:?}"),
            }),
        }
    }

    fn parse_create(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Create)?;
        match self.peek() {
            TokenKind::Table => self.parse_create_table(),
            TokenKind::Index => self.parse_create_index(),
            other => Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("expected TABLE or INDEX, found {other:?}"),
            }),
        }
    }

    fn parse_create_table(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Table)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let mut columns = Vec::new();
        loop {
            columns.push(self.parse_column_def()?);
            if self.peek() == &TokenKind::Comma {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(&TokenKind::RParen)?;
        Ok(Statement::CreateTable { name, columns })
    }

    fn parse_column_def(&mut self) -> SqlResult<ColumnDef> {
        let name = self.expect_ident()?;
        let ty_name = self.expect_ident()?;
        let ty = DataType::parse(&ty_name).ok_or_else(|| SqlError::Parse {
            pos: self.pos(),
            message: format!("unknown type '{ty_name}'"),
        })?;
        let mut constraints = Vec::new();
        loop {
            match self.peek() {
                TokenKind::Primary => {
                    self.advance();
                    self.expect(&TokenKind::Key)?;
                    constraints.push(Constraint::PrimaryKey);
                }
                TokenKind::Unique => {
                    self.advance();
                    constraints.push(Constraint::Unique);
                }
                TokenKind::Not => {
                    self.advance();
                    self.expect(&TokenKind::Null)?;
                    constraints.push(Constraint::NotNull);
                }
                _ => break,
            }
        }
        Ok(ColumnDef {
            name,
            ty,
            constraints,
        })
    }

    fn parse_create_index(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Index)?;
        // Optional index name.
        let mut first = None;
        if let TokenKind::Ident(_) = self.peek()
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Ident(_))
            )
        {
            first = Some(self.expect_ident()?);
        }
        self.expect(&TokenKind::Ident("ON".to_string()))?;
        let table = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let mut columns = Vec::new();
        loop {
            columns.push(self.expect_ident()?);
            if self.peek() == &TokenKind::Comma {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(&TokenKind::RParen)?;
        Ok(Statement::CreateIndex {
            table,
            columns,
            name: first,
        })
    }

    fn parse_drop(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Drop)?;
        match self.peek() {
            TokenKind::Table => {
                self.advance();
                let name = self.expect_ident()?;
                Ok(Statement::DropTable { name })
            }
            TokenKind::Index => {
                self.advance();
                let name = self.expect_ident()?;
                Ok(Statement::DropIndex { name })
            }
            other => Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("expected TABLE or INDEX, found {other:?}"),
            }),
        }
    }

    fn parse_insert(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Insert)?;
        self.expect(&TokenKind::Into)?;
        let table = self.expect_ident()?;
        let mut columns = None;
        if self.peek() == &TokenKind::LParen {
            self.advance();
            let mut cols = Vec::new();
            loop {
                cols.push(self.expect_ident()?);
                if self.peek() == &TokenKind::Comma {
                    self.advance();
                    continue;
                }
                break;
            }
            self.expect(&TokenKind::RParen)?;
            columns = Some(cols);
        }
        self.expect(&TokenKind::Values)?;
        let mut values = Vec::new();
        loop {
            self.expect(&TokenKind::LParen)?;
            let mut row = Vec::new();
            loop {
                row.push(self.parse_expr()?);
                if self.peek() == &TokenKind::Comma {
                    self.advance();
                    continue;
                }
                break;
            }
            self.expect(&TokenKind::RParen)?;
            values.push(row);
            if self.peek() == &TokenKind::Comma {
                self.advance();
                continue;
            }
            break;
        }
        Ok(Statement::Insert {
            table,
            columns,
            values,
        })
    }

    fn parse_select(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Select)?;
        let mut items = Vec::new();
        loop {
            if self.peek() == &TokenKind::Star {
                self.advance();
                items.push(SelectItem::Wildcard);
            } else {
                let expr = self.parse_expr()?;
                let alias = if let TokenKind::Ident(s) = self.peek().clone() {
                    // Only treat as alias if not a reserved keyword (AS
                    // is not a keyword here to keep things simple).
                    self.advance();
                    Some(s)
                } else {
                    None
                };
                items.push(SelectItem::Expr { expr, alias });
            }
            if self.peek() == &TokenKind::Comma {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(&TokenKind::From)?;
        let table = self.expect_ident()?;
        let filter = if self.peek() == &TokenKind::Where {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let mut order_by = Vec::new();
        if self.peek() == &TokenKind::Order {
            self.advance();
            self.expect(&TokenKind::By)?;
            loop {
                let col = self.expect_ident()?;
                let asc = match self.peek() {
                    TokenKind::Asc => {
                        self.advance();
                        true
                    }
                    TokenKind::Desc => {
                        self.advance();
                        false
                    }
                    _ => true,
                };
                order_by.push((col, asc));
                if self.peek() == &TokenKind::Comma {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        let limit = if self.peek() == &TokenKind::Limit {
            self.advance();
            match self.advance() {
                TokenKind::Int(n) if n >= 0 => Some(n as usize),
                other => {
                    return Err(SqlError::Parse {
                        pos: self.pos(),
                        message: format!("expected non-negative integer, found {other:?}"),
                    });
                }
            }
        } else {
            None
        };
        Ok(Statement::Select {
            items,
            table,
            filter,
            order_by,
            limit,
        })
    }

    fn parse_update(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Update)?;
        let table = self.expect_ident()?;
        self.expect(&TokenKind::Set)?;
        let mut assignments = Vec::new();
        loop {
            let col = self.expect_ident()?;
            self.expect(&TokenKind::Eq)?;
            let expr = self.parse_expr()?;
            assignments.push((col, expr));
            if self.peek() == &TokenKind::Comma {
                self.advance();
                continue;
            }
            break;
        }
        let filter = if self.peek() == &TokenKind::Where {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Statement::Update {
            table,
            assignments,
            filter,
        })
    }

    fn parse_delete(&mut self) -> SqlResult<Statement> {
        self.expect(&TokenKind::Delete)?;
        self.expect(&TokenKind::From)?;
        let table = self.expect_ident()?;
        let filter = if self.peek() == &TokenKind::Where {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Statement::Delete { table, filter })
    }

    fn parse_expr(&mut self) -> SqlResult<Expr> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> SqlResult<Expr> {
        let mut lhs = self.parse_and()?;
        while self.peek() == &TokenKind::Or {
            self.advance();
            let rhs = self.parse_and()?;
            lhs = Expr::Binary(Box::new(lhs), BinOp::Or, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> SqlResult<Expr> {
        let mut lhs = self.parse_not()?;
        while self.peek() == &TokenKind::And {
            self.advance();
            let rhs = self.parse_not()?;
            lhs = Expr::Binary(Box::new(lhs), BinOp::And, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> SqlResult<Expr> {
        if self.peek() == &TokenKind::Not {
            self.advance();
            let inner = self.parse_not()?;
            return Ok(Expr::Unary(UnaryOp::Not, Box::new(inner)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> SqlResult<Expr> {
        let mut lhs = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                TokenKind::Eq => BinOp::Eq,
                TokenKind::Ne => BinOp::Ne,
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Le => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::Ge => BinOp::Ge,
                TokenKind::Like => BinOp::Like,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_additive()?;
            lhs = Expr::Binary(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> SqlResult<Expr> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_multiplicative()?;
            lhs = Expr::Binary(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> SqlResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_unary()?;
            lhs = Expr::Binary(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> SqlResult<Expr> {
        if self.peek() == &TokenKind::Minus {
            self.advance();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(UnaryOp::Neg, Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> SqlResult<Expr> {
        match self.advance() {
            TokenKind::Int(n) => Ok(Expr::Int(n)),
            TokenKind::Float(f) => Ok(Expr::Float(f)),
            TokenKind::Str(s) => Ok(Expr::Str(s)),
            TokenKind::True => Ok(Expr::Bool(true)),
            TokenKind::False => Ok(Expr::Bool(false)),
            TokenKind::Null => Ok(Expr::Null),
            TokenKind::Ident(name) => {
                if self.peek() == &TokenKind::LParen {
                    self.advance();
                    let mut args = Vec::new();
                    if self.peek() != &TokenKind::RParen {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.peek() == &TokenKind::Comma {
                                self.advance();
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Column(name))
                }
            }
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(inner)
            }
            other => Err(SqlError::Parse {
                pos: self.pos(),
                message: format!("unexpected token {other:?} in expression"),
            }),
        }
    }
}
