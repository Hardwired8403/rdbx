//! Hand-written lexer.

use crate::error::{SqlError, SqlResult};

/// One lexical token.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub pos: usize,
}

/// Token kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords (case-insensitive)
    Create,
    Table,
    Index,
    Drop,
    Insert,
    Into,
    Values,
    Select,
    From,
    Where,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    Update,
    Set,
    Delete,
    Begin,
    Commit,
    Rollback,
    Primary,
    Key,
    Unique,
    Not,
    Null,
    And,
    Or,
    Like,
    In,
    Group,
    Having,
    As,
    Join,
    Inner,
    Left,
    On,
    True,
    False,
    Count,
    Sum,
    Min,
    Max,
    Avg,
    Question,

    // Literals & identifiers
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),

    // Punctuation & operators
    LParen,
    RParen,
    Comma,
    Semicolon,
    Star,
    Plus,
    Minus,
    Slash,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    Eof,
}

/// Lexer.
pub struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    pub fn tokenize(mut self) -> SqlResult<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if eof {
                return Ok(tokens);
            }
        }
    }

    fn next_token(&mut self) -> SqlResult<Token> {
        self.skip_whitespace_and_comments();
        let pos = self.pos;
        let Some(&b) = self.bytes.get(self.pos) else {
            return Ok(Token {
                kind: TokenKind::Eof,
                pos,
            });
        };
        let kind = match b {
            b'(' => self.single(TokenKind::LParen),
            b')' => self.single(TokenKind::RParen),
            b',' => self.single(TokenKind::Comma),
            b';' => self.single(TokenKind::Semicolon),
            b'*' => self.single(TokenKind::Star),
            b'+' => self.single(TokenKind::Plus),
            b'-' => self.single(TokenKind::Minus),
            b'/' => self.single(TokenKind::Slash),
            b'?' => self.single(TokenKind::Question),
            b'=' => self.single(TokenKind::Eq),
            b'<' => {
                self.pos += 1;
                if self.bytes.get(self.pos) == Some(&b'=') {
                    self.pos += 1;
                    TokenKind::Le
                } else if self.bytes.get(self.pos) == Some(&b'>') {
                    self.pos += 1;
                    TokenKind::Ne
                } else {
                    TokenKind::Lt
                }
            }
            b'>' => {
                self.pos += 1;
                if self.bytes.get(self.pos) == Some(&b'=') {
                    self.pos += 1;
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            b'!' => {
                self.pos += 1;
                if self.bytes.get(self.pos) == Some(&b'=') {
                    self.pos += 1;
                    TokenKind::Ne
                } else {
                    return Err(SqlError::Lex { pos, ch: '!' });
                }
            }
            b'\'' => self.lex_string(pos)?,
            b'0'..=b'9' => self.lex_number(pos)?,
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident_or_keyword(),
            other => {
                return Err(SqlError::Lex {
                    pos,
                    ch: other as char,
                });
            }
        };
        Ok(Token { kind, pos })
    }

    fn single(&mut self, kind: TokenKind) -> TokenKind {
        self.pos += 1;
        kind
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.bytes.get(self.pos) {
                Some(&b) if b.is_ascii_whitespace() => self.pos += 1,
                Some(b'-') if self.bytes.get(self.pos + 1) == Some(&b'-') => {
                    self.pos += 2;
                    while let Some(&b) = self.bytes.get(self.pos) {
                        self.pos += 1;
                        if b == b'\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn lex_string(&mut self, start: usize) -> SqlResult<TokenKind> {
        self.pos += 1;
        let content_start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b'\'' {
                let s = std::str::from_utf8(&self.bytes[content_start..self.pos])
                    .map_err(|_| SqlError::Lex {
                        pos: start,
                        ch: '\'',
                    })?
                    .to_string();
                self.pos += 1;
                return Ok(TokenKind::Str(s));
            }
            self.pos += 1;
        }
        Err(SqlError::Lex {
            pos: start,
            ch: '\'',
        })
    }

    fn lex_number(&mut self, start: usize) -> SqlResult<TokenKind> {
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.bytes.get(self.pos) == Some(&b'.') {
            is_float = true;
            self.pos += 1;
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
        }
        let text =
            std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| SqlError::Lex {
                pos: start,
                ch: '?',
            })?;
        if is_float {
            text.parse::<f64>()
                .map(TokenKind::Float)
                .map_err(|_| SqlError::Lex {
                    pos: start,
                    ch: '.',
                })
        } else {
            text.parse::<i64>()
                .map(TokenKind::Int)
                .map_err(|_| SqlError::Lex {
                    pos: start,
                    ch: '?',
                })
        }
    }

    fn lex_ident_or_keyword(&mut self) -> TokenKind {
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        match text.to_ascii_uppercase().as_str() {
            "CREATE" => TokenKind::Create,
            "TABLE" => TokenKind::Table,
            "INDEX" => TokenKind::Index,
            "DROP" => TokenKind::Drop,
            "INSERT" => TokenKind::Insert,
            "INTO" => TokenKind::Into,
            "VALUES" => TokenKind::Values,
            "SELECT" => TokenKind::Select,
            "FROM" => TokenKind::From,
            "WHERE" => TokenKind::Where,
            "ORDER" => TokenKind::Order,
            "BY" => TokenKind::By,
            "ASC" => TokenKind::Asc,
            "DESC" => TokenKind::Desc,
            "LIMIT" => TokenKind::Limit,
            "UPDATE" => TokenKind::Update,
            "SET" => TokenKind::Set,
            "DELETE" => TokenKind::Delete,
            "BEGIN" => TokenKind::Begin,
            "COMMIT" => TokenKind::Commit,
            "ROLLBACK" => TokenKind::Rollback,
            "PRIMARY" => TokenKind::Primary,
            "KEY" => TokenKind::Key,
            "UNIQUE" => TokenKind::Unique,
            "NOT" => TokenKind::Not,
            "NULL" => TokenKind::Null,
            "AND" => TokenKind::And,
            "OR" => TokenKind::Or,
            "LIKE" => TokenKind::Like,
            "IN" => TokenKind::In,
            "GROUP" => TokenKind::Group,
            "HAVING" => TokenKind::Having,
            "AS" => TokenKind::As,
            "JOIN" => TokenKind::Join,
            "INNER" => TokenKind::Inner,
            "LEFT" => TokenKind::Left,
            "ON" => TokenKind::On,
            "TRUE" => TokenKind::True,
            "FALSE" => TokenKind::False,
            "COUNT" => TokenKind::Count,
            "SUM" => TokenKind::Sum,
            "MIN" => TokenKind::Min,
            "MAX" => TokenKind::Max,
            "AVG" => TokenKind::Avg,
            _ => TokenKind::Ident(text.to_string()),
        }
    }
}
