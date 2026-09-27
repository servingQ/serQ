//! Tokens of the seQ surface syntax.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    // punctuation
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Dot,
    Assign,
    Tilde,
    Question,
    // operators
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,
    AndAnd,
    OrOr,
    Not,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "`{s}`"),
            Tok::Num(x) => write!(f, "{x}"),
            Tok::Str(s) => write!(f, "\"{s}\""),
            other => write!(f, "{other:?}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct LexError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let n = chars.len();
    while i < n {
        let c = chars[i];
        // whitespace
        if c == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        // comments: // ... and /* ... */
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            col += 2;
            while i < n && !(chars[i] == '*' && i + 1 < n && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
                i += 1;
            }
            i += 2;
            col += 2;
            continue;
        }
        let (tline, tcol) = (line, col);
        let push = |out: &mut Vec<Token>, tok: Tok| {
            out.push(Token {
                tok,
                line: tline,
                col: tcol,
            })
        };
        // numbers
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && chars[i + 1].is_ascii_digit()) {
            let start = i;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                let save = i;
                i += 1;
                if i < n && (chars[i] == '+' || chars[i] == '-') {
                    i += 1;
                }
                if i < n && chars[i].is_ascii_digit() {
                    while i < n && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
            let v: f64 = text.parse().map_err(|_| LexError {
                line,
                col,
                msg: format!("bad number `{text}`"),
            })?;
            col += i - start;
            push(&mut out, Tok::Num(v));
            continue;
        }
        // identifiers
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            col += i - start;
            push(&mut out, Tok::Ident(s));
            continue;
        }
        // strings
        if c == '"' {
            let mut s = String::new();
            i += 1;
            col += 1;
            while i < n && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
                col += 1;
            }
            if i >= n {
                return Err(LexError {
                    line,
                    col,
                    msg: "unterminated string".into(),
                });
            }
            i += 1;
            col += 1;
            push(&mut out, Tok::Str(s));
            continue;
        }
        let two = if i + 1 < n {
            Some((chars[i], chars[i + 1]))
        } else {
            None
        };
        let (tok, len) = match two {
            Some(('<', '=')) => (Tok::Le, 2),
            Some(('>', '=')) => (Tok::Ge, 2),
            Some(('=', '=')) => (Tok::EqEq, 2),
            Some(('!', '=')) => (Tok::Ne, 2),
            Some(('&', '&')) => (Tok::AndAnd, 2),
            Some(('|', '|')) => (Tok::OrOr, 2),
            _ => (
                match c {
                    '{' => Tok::LBrace,
                    '}' => Tok::RBrace,
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    '[' => Tok::LBracket,
                    ']' => Tok::RBracket,
                    ',' => Tok::Comma,
                    ';' => Tok::Semi,
                    ':' => Tok::Colon,
                    '.' => Tok::Dot,
                    '=' => Tok::Assign,
                    '~' => Tok::Tilde,
                    '?' => Tok::Question,
                    '+' => Tok::Plus,
                    '-' => Tok::Minus,
                    '*' => Tok::Star,
                    '/' => Tok::Slash,
                    '^' => Tok::Caret,
                    '<' => Tok::Lt,
                    '>' => Tok::Gt,
                    '!' => Tok::Not,
                    other => {
                        return Err(LexError {
                            line,
                            col,
                            msg: format!("unexpected character `{other}`"),
                        });
                    }
                },
                1,
            ),
        };
        i += len;
        col += len;
        push(&mut out, tok);
    }
    out.push(Token {
        tok: Tok::Eof,
        line,
        col,
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_numbers_idents_and_ops() {
        let t = lex("pool kv { cap 3e5; } // c\n x <= ~exp(1.5)").unwrap();
        let toks: Vec<Tok> = t.into_iter().map(|t| t.tok).collect();
        assert_eq!(toks[0], Tok::Ident("pool".into()));
        assert_eq!(toks[3], Tok::Ident("cap".into()));
        assert_eq!(toks[4], Tok::Num(3e5));
        assert!(toks.contains(&Tok::Le));
        assert!(toks.contains(&Tok::Tilde));
        assert_eq!(*toks.last().unwrap(), Tok::Eof);
    }
}
