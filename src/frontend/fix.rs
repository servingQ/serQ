//! A code's fix-it (`docs/reference/diagnostics.md`) as edits of the text,
//! for `serq::diagnose`: found from the refusal's place and the tokens
//! around it, never from the message's wording. Where the table names no
//! fix-it, or the text around the place is not the one the fix-it is for,
//! there is none.

use crate::frontend::diagnostic::{Code, Edit, Fix};
use crate::frontend::lexer::{Tok, Token, lex};
use crate::frontend::outline::Range;

/// The text as tokens, with where each starts and ends in bytes.
struct Text<'a> {
    src: &'a str,
    toks: Vec<Token>,
    /// The byte offset of each line's start.
    lines: Vec<usize>,
}

impl<'a> Text<'a> {
    fn new(src: &'a str) -> Option<Self> {
        let toks = lex(src).ok()?;
        let lines = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(k, _)| k + 1))
            .collect();
        Some(Text { src, toks, lines })
    }

    /// Where token `k` starts: its line's start, then its column in characters.
    fn start(&self, k: usize) -> usize {
        let t = &self.toks[k];
        let line = self
            .lines
            .get(t.line.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        self.src[line..]
            .char_indices()
            .nth(t.col.saturating_sub(1))
            .map_or(self.src.len(), |(b, _)| line + b)
    }

    fn end(&self, k: usize) -> usize {
        self.start(k) + self.toks[k].text.len()
    }

    /// The token at a place `L:C`.
    fn at(&self, line: usize, col: usize) -> Option<usize> {
        self.toks
            .iter()
            .position(|t| t.line == line && t.col == col && t.tok != Tok::Eof)
    }

    fn word(&self, k: usize) -> Option<&str> {
        match &self.toks.get(k)?.tok {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }
}

/// The token at a refusal's place, as a byte range.
pub fn place(src: &str, line: usize, col: usize) -> Option<Range> {
    let t = Text::new(src)?;
    let k = t.at(line, col)?;
    Some(Range(t.start(k), t.end(k)))
}

/// `code`'s fix-it for the refusal at `L:C`, if the table names one and the
/// text there is the one it is for.
pub fn fixes(code: Code, src: &str, line: usize, col: usize) -> Vec<Fix> {
    let fix = Text::new(src).and_then(|t| {
        let k = t.at(line, col)?;
        match code {
            Code::CopySwapped => swap(&t, k),
            Code::ConnectorMisnamed => rename(&t, k),
            _ => None,
        }
    });
    fix.into_iter().collect()
}

/// `E-copy-swapped`, at the relation: `kvt.nixl_pull (S, R, …)` written
/// `(R, S, …)`, or `kvt.l2_link (C, Q, …)` written `(Q, C, …)`, the two
/// queues' names exchanged.
fn swap(t: &Text, k: usize) -> Option<Fix> {
    let open = (k..t.toks.len()).find(|&j| t.toks[j].tok == Tok::LParen)?;
    let (a, b) = (open + 1, open + 3);
    if t.toks.get(open + 2)?.tok != Tok::Comma {
        return None;
    }
    let (first, second) = (t.word(a)?, t.word(b)?);
    use crate::frontend::parser::{KvtMode, kvt_relation};
    let link = open > 0
        && t.word(open - 1)
            .and_then(kvt_relation)
            .is_some_and(|r| r.mode == KvtMode::Link);
    let order = if link {
        "the L2 first, then the queue whose engine copies through it"
    } else {
        "the source first, then the reader"
    };
    Some(Fix {
        label: format!("swap `{first}` and `{second}`: {order}"),
        edits: vec![
            Edit {
                range: Range(t.start(a), t.end(a)),
                text: second.to_string(),
            },
            Edit {
                range: Range(t.start(b), t.end(b)),
                text: first.to_string(),
            },
        ],
    })
}

/// `E-connector-misnamed`, at the connector's name in queue `R`'s engine:
/// the name of the queue `R`'s relation reads from, `S` in
/// `kvt.nixl_pull (S, R, …)` or `kvt.nixl_push`.
fn rename(t: &Text, k: usize) -> Option<Fix> {
    // the queue the engine is in: the last `queue R` above
    let q = (0..k)
        .rev()
        .find(|&j| t.word(j) == Some("queue"))
        .and_then(|j| t.word(j + 1))?;
    // its relation, `nixl_pull (S, R` or `nixl_push (S, R`
    let source = (0..t.toks.len().saturating_sub(5)).find_map(|j| {
        let relation = matches!(t.word(j), Some("nixl_pull" | "nixl_push"))
            && t.toks[j + 1].tok == Tok::LParen
            && t.toks[j + 3].tok == Tok::Comma
            && t.word(j + 4) == Some(q);
        relation.then(|| t.word(j + 2)).flatten()
    })?;
    let wrong = t.word(k)?;
    Some(Fix {
        label: format!("rename `connector {wrong}` to `connector {source}`"),
        edits: vec![Edit {
            range: Range(t.start(k), t.end(k)),
            text: source.to_string(),
        }],
    })
}
