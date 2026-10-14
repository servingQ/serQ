//! An outline of a program's text: where each declaration, relation and
//! entry call is written, as byte ranges into the source.
//!
//! For an editor that changes the text in place (serq-lar's figure, a
//! language server): it finds what to change here and leaves the meaning to
//! the compiler. The parser fills it as it reads, from the tokens it
//! consumes, only when asked (`serq::outline`); it adds no syntax and no IR,
//! and a program that parses has one whether it links or not.

use serde::Serialize;

use crate::frontend::lexer::{Tok, Token};

/// A half-open byte range `[start, end)` of the source, written `[start, end]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Range(pub usize, pub usize);

impl Range {
    pub fn start(self) -> usize {
        self.0
    }
    pub fn end(self) -> usize {
        self.1
    }
}

/// A braced declaration: all of it, and what is between its braces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Block {
    pub range: Range,
    pub body: Range,
}

/// `use "std/kvt";`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Use {
    pub path: String,
    pub range: Range,
}

/// A declaration known by its keyword and name: `let B = 8192;`, `stage
/// tool : delay;`, `engine vllm on gpu { … }`, `pool kv on gpu { … }`,
/// `device gpu { … }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decl {
    /// The keyword: `let`, `stage`, `engine`, `pool`, `device`.
    pub kind: String,
    pub name: String,
    pub range: Range,
}

/// What a queue declares, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    /// `device`, `engine`, `pool`, `nic`, `serve`, or `entry`.
    pub kind: String,
    /// A device's or a pool's name; an entry's verb.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// An entry's `from Q`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub range: Range,
    /// Between the braces, for a member that has them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Range>,
}

/// `queue NAME[N] : role, … { … }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Queue {
    pub name: String,
    pub roles: Vec<String>,
    /// The text between a family's brackets (`NP` in `queue P[NP]`).
    pub family: Option<String>,
    pub range: Range,
    pub body: Range,
    pub members: Vec<Member>,
}

/// `kvt.nixl_pull (SOURCE, READER, …);` or `kvt.nixl_push (…);`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Relation {
    /// `pull` or `push`.
    pub mode: String,
    pub source: String,
    pub reader: String,
    pub range: Range,
    /// The function's name, `nixl_pull` or `nixl_push`.
    pub function: Range,
}

/// A gateway's `route { … }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Route {
    pub gateway: String,
    pub range: Range,
    pub body: Range,
}

/// `Q[i].verb (args) [from S[k]];`, as written in the server, a gateway's
/// route or an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Call {
    pub queue: String,
    /// The text between the brackets (`j` in `D[j]`).
    pub index: Option<String>,
    pub verb: String,
    pub from: Option<String>,
    /// The statement, to its `;`.
    pub range: Range,
    /// Where it is written: `server`, `route` or `entry`.
    pub block: String,
    /// The queue whose route or entry it is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// The outline of a program's text (`serq::outline`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Outline {
    /// `fn main() { … }`.
    pub main: Option<Block>,
    pub uses: Vec<Use>,
    /// The declarations outside queues, in order: `let`, `stage`,
    /// `engine`, `pool`, `device`.
    pub decls: Vec<Decl>,
    pub queues: Vec<Queue>,
    pub relations: Vec<Relation>,
    pub routes: Vec<Route>,
    pub server: Option<Block>,
    pub workload: Option<Range>,
    pub calls: Vec<Call>,
}

impl Outline {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an outline serializes")
    }
}

/// The parser's recorder: the source, to turn a token's line and column
/// into a byte offset, and the outline so far.
pub(crate) struct Recorder {
    src: String,
    /// The byte offset of each line's start.
    lines: Vec<usize>,
    pub(crate) out: Outline,
    members: Vec<Member>,
}

fn word(t: &Token) -> Option<&str> {
    match &t.tok {
        Tok::Ident(s) => Some(s),
        _ => None,
    }
}

impl Recorder {
    pub(crate) fn new(src: &str) -> Recorder {
        let lines = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Recorder {
            src: src.to_string(),
            lines,
            out: Outline::default(),
            members: vec![],
        }
    }

    /// The byte offset of a token's first character; columns count
    /// characters.
    fn start(&self, t: &Token) -> usize {
        let Some(&line) = self.lines.get(t.line.saturating_sub(1)) else {
            return self.src.len();
        };
        self.src[line..]
            .char_indices()
            .nth(t.col.saturating_sub(1))
            .map_or(self.src.len(), |(b, _)| line + b)
    }

    fn end(&self, t: &Token) -> usize {
        self.start(t) + t.text.len()
    }

    /// Tokens `first..=last`.
    fn range(&self, toks: &[Token], first: usize, last: usize) -> Range {
        Range(self.start(&toks[first]), self.end(&toks[last]))
    }

    /// Between the first `{` of `first..=last` and `last`, a `}`.
    fn body(&self, toks: &[Token], first: usize, last: usize) -> Option<Range> {
        let open = (first..last).find(|&k| toks[k].tok == Tok::LBrace)?;
        (toks[last].tok == Tok::RBrace)
            .then(|| Range(self.end(&toks[open]), self.start(&toks[last])))
    }

    fn name_at(toks: &[Token], k: usize) -> String {
        toks.get(k).and_then(word).unwrap_or_default().to_string()
    }

    /// A declaration of `main` or above it, tokens `at..=last`: whatever
    /// its keyword says it is. A queue records itself (`queue`).
    pub(crate) fn item(&mut self, toks: &[Token], at: usize, last: usize) {
        let range = self.range(toks, at, last);
        let key = word(&toks[at]).unwrap_or_default();
        match key {
            "use" => {
                if let Tok::Str(path) = &toks[at + 1].tok {
                    self.out.uses.push(Use {
                        path: path.clone(),
                        range,
                    });
                }
            }
            "let" | "stage" | "engine" | "pool" | "device" => self.out.decls.push(Decl {
                kind: key.into(),
                name: Self::name_at(toks, at + 1),
                range,
            }),
            "workload" => self.out.workload = Some(range),
            "server" => {
                if let Some(body) = self.body(toks, at, last) {
                    self.out.server = Some(Block { range, body });
                }
            }
            "kvt" => {
                let function = Self::name_at(toks, at + 2);
                let mode = function
                    .strip_prefix("nixl_")
                    .unwrap_or(&function)
                    .to_string();
                self.out.relations.push(Relation {
                    mode,
                    source: Self::name_at(toks, at + 4),
                    reader: Self::name_at(toks, at + 6),
                    range,
                    function: self.range(toks, at + 2, at + 2),
                });
            }
            _ => {}
        }
    }

    /// `fn main() {` at `at`, its `{` at `open`, its `}` at `close`.
    pub(crate) fn main(&mut self, toks: &[Token], at: usize, open: usize, close: usize) {
        self.out.main = Some(Block {
            range: self.range(toks, at, close),
            body: Range(self.end(&toks[open]), self.start(&toks[close])),
        });
    }

    /// A member of the queue being read, tokens `at..=last`.
    pub(crate) fn member(&mut self, toks: &[Token], at: usize, last: usize) {
        let key = word(&toks[at]).unwrap_or_default().to_string();
        let range = self.range(toks, at, last);
        let body = self.body(toks, at, last);
        let m = match key.as_str() {
            "device" | "pool" => Member {
                kind: key,
                name: Some(Self::name_at(toks, at + 1)),
                from: None,
                range,
                body,
            },
            "engine" | "nic" | "serve" => Member {
                kind: key,
                name: None,
                from: None,
                range,
                body,
            },
            _ => {
                // an entry: its verb, and `from Q` in its header
                let header = (at..=last)
                    .find(|&k| toks[k].tok == Tok::LBrace)
                    .unwrap_or(last);
                let from = (at..header)
                    .find(|&k| word(&toks[k]) == Some("from"))
                    .map(|k| Self::name_at(toks, k + 1));
                Member {
                    kind: "entry".into(),
                    name: Some(key),
                    from,
                    range,
                    body,
                }
            }
        };
        self.members.push(m);
    }

    /// The queue just read: `queue` at `at`, its family's brackets at
    /// `brackets`, its braces at `open` and `close`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue(
        &mut self,
        toks: &[Token],
        at: usize,
        name: &str,
        roles: &[String],
        brackets: Option<(usize, usize)>,
        open: usize,
        close: usize,
    ) {
        let members = std::mem::take(&mut self.members);
        if roles.iter().any(|r| r == "gateway") {
            for m in members.iter().filter(|m| m.kind == "entry") {
                if let Some(body) = m.body {
                    self.out.routes.push(Route {
                        gateway: name.into(),
                        range: m.range,
                        body,
                    });
                }
            }
        }
        let family = brackets.map(|(l, r)| {
            self.src[self.end(&toks[l])..self.start(&toks[r])]
                .trim()
                .to_string()
        });
        self.out.queues.push(Queue {
            name: name.into(),
            roles: roles.to_vec(),
            family,
            range: self.range(toks, at, close),
            body: Range(self.end(&toks[open]), self.start(&toks[close])),
            members,
        });
    }

    /// An entry call, tokens `at..=last`; `index` its brackets, if any.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call(
        &mut self,
        toks: &[Token],
        at: usize,
        last: usize,
        index: Option<(usize, usize)>,
        verb: &str,
        from: Option<&str>,
        block: &str,
        owner: Option<&str>,
    ) {
        let index = index.map(|(l, r)| {
            self.src[self.end(&toks[l])..self.start(&toks[r])]
                .trim()
                .to_string()
        });
        self.out.calls.push(Call {
            queue: Self::name_at(toks, at),
            index,
            verb: verb.into(),
            from: from.map(str::to_string),
            range: self.range(toks, at, last),
            block: block.into(),
            owner: owner.map(str::to_string),
        });
    }
}
