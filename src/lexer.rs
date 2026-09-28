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

/// What sits between two tokens: whitespace and comments, in source order.
///
/// The lexer used to drop both, which meant anything built on the token
/// stream - a formatter, a rename that can tell a word in code from the same
/// word in a comment - would have thrown away a third of `programs/vllm.seq`,
/// including the upstream citations `make check` verifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriviaKind {
    Whitespace,
    /// `// ...`
    Line,
    /// `/* ... */`
    Block,
}

#[derive(Clone, Debug)]
pub struct Trivia {
    pub kind: TriviaKind,
    /// The source text, verbatim.
    pub text: String,
    pub line: usize,
    pub col: usize,
}

impl Trivia {
    pub fn is_comment(&self) -> bool {
        self.kind != TriviaKind::Whitespace
    }
    /// Blank lines in a whitespace run, which is what a paragraph break is.
    pub fn blank_lines(&self) -> usize {
        if self.kind == TriviaKind::Whitespace {
            self.text.matches('\n').count().saturating_sub(1)
        } else {
            0
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
    /// The token's own source text. A number keeps its spelling: `1e5` is not
    /// `100000`, and a formatter must not decide otherwise.
    pub text: String,
    /// Everything between the previous token and this one.
    pub leading: Vec<Trivia>,
}

/// The tokens and their trivia, concatenated, are the source again.
pub fn unlex(toks: &[Token]) -> String {
    let mut out = String::new();
    for t in toks {
        for tr in &t.leading {
            out.push_str(&tr.text);
        }
        out.push_str(&t.text);
    }
    out
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

#[allow(clippy::too_many_arguments)]
fn emit(
    out: &mut Vec<Token>,
    pending: &mut Vec<Trivia>,
    tok: Tok,
    line: usize,
    col: usize,
    chars: &[char],
    start: usize,
    end: usize,
) {
    out.push(Token {
        tok,
        line,
        col,
        text: chars[start..end].iter().collect(),
        leading: std::mem::take(pending),
    });
}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut pending: Vec<Trivia> = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let n = chars.len();
    while i < n {
        let c = chars[i];
        // whitespace, kept as one run so that blank lines survive
        if c.is_whitespace() {
            let (start, sline, scol) = (i, line, col);
            while i < n && chars[i].is_whitespace() {
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
                i += 1;
            }
            pending.push(Trivia {
                kind: TriviaKind::Whitespace,
                text: chars[start..i].iter().collect(),
                line: sline,
                col: scol,
            });
            continue;
        }
        // comments: // ... and /* ... */
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            let (start, scol) = (i, col);
            while i < n && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            pending.push(Trivia {
                kind: TriviaKind::Line,
                text: chars[start..i].iter().collect(),
                line,
                col: scol,
            });
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            let (start, sline, scol) = (i, line, col);
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
            i = (i + 2).min(n); // an unterminated block comment runs to the end
            col += 2;
            pending.push(Trivia {
                kind: TriviaKind::Block,
                text: chars[start..i].iter().collect(),
                line: sline,
                col: scol,
            });
            continue;
        }
        let (tline, tcol) = (line, col);
        let tok_start = i;

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
            emit(
                &mut out,
                &mut pending,
                Tok::Num(v),
                tline,
                tcol,
                &chars,
                tok_start,
                i,
            );
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
            emit(
                &mut out,
                &mut pending,
                Tok::Ident(s),
                tline,
                tcol,
                &chars,
                tok_start,
                i,
            );
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
            emit(
                &mut out,
                &mut pending,
                Tok::Str(s),
                tline,
                tcol,
                &chars,
                tok_start,
                i,
            );
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
        emit(
            &mut out,
            &mut pending,
            tok,
            tline,
            tcol,
            &chars,
            tok_start,
            i,
        );
    }
    out.push(Token {
        tok: Tok::Eof,
        line,
        col,
        text: String::new(),
        leading: std::mem::take(&mut pending),
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

    /// The property a formatter stands on: nothing between two tokens is
    /// dropped, so the stream is the source again.
    #[test]
    fn tokens_and_trivia_reconstruct_the_source() {
        for src in [
            "pool kv { cap 3e5; } // c\n x <= ~exp(1.5)",
            "// leading\n\nlet a = 1; /* mid */ let b = \"two words\";\n",
            "  ",
            "",
            "let a = 1; /* unterminated",
            "\u{fffd}dent",
        ] {
            let Ok(toks) = lex(src) else { continue };
            assert_eq!(unlex(&toks), src, "round trip of {src:?}");
        }
    }

    #[test]
    fn a_comment_is_leading_trivia_of_the_token_after_it() {
        let t = lex("let a = 1;\n// why\nlet b = 2;").unwrap();
        let b = t
            .iter()
            .position(|t| t.tok == Tok::Ident("b".into()))
            .unwrap();
        // the comment hangs off `let`, the token that starts b's statement
        let lt = &t[b - 1];
        assert_eq!(lt.tok, Tok::Ident("let".into()));
        let c: Vec<&Trivia> = lt.leading.iter().filter(|x| x.is_comment()).collect();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "// why");
        assert_eq!(c[0].line, 2);
        assert_eq!(c[0].kind, TriviaKind::Line);
    }

    #[test]
    fn a_blank_line_survives_as_trivia() {
        let t = lex("let a = 1;\n\n\nlet b = 2;").unwrap();
        let gap: usize = t
            .iter()
            .flat_map(|t| &t.leading)
            .map(|x| x.blank_lines())
            .max()
            .unwrap();
        assert_eq!(gap, 2);
    }

    /// A number keeps its spelling. `3e5` is not `300000`.
    #[test]
    fn a_token_carries_its_own_spelling() {
        let t = lex("cap 3e5; block 1_024;").unwrap();
        let texts: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
        assert!(texts.contains(&"3e5"), "{texts:?}");
        assert!(texts.contains(&"1_024"), "{texts:?}");
    }
}
