//! What each piece of a program is, for an editor or a page to colour.
//!
//! Four roles, four colours. A program's shape is `engine`/`workload`/`server`;
//! what a request *does* is `hold`, `run`, `observe`; the knobs are `cap`,
//! `evict`, `scheduler`; and what it *reads* is `cachedin`, `running.count`,
//! `now`. A reader should be able to tell those apart before reading a word,
//! so each is its own role, and `~` gets one too because that is where the
//! randomness enters. Arithmetic stays plain: it is how a program computes,
//! not what it means.
//!
//! The pieces are the lexer's tokens and trivia, so a word in a comment is
//! never coloured as code. What a program reads is the linker's and the IR's
//! tables (`CONTEXT_VARS`, `Fun`, `FOLDED`, `BUILTIN_ATTRS`, `AGGREGATES`,
//! `ENGINE_VALUES`); the keywords' roles are the three lists here, which
//! cover `parser::KEYWORDS`. The docs' Pygments lexer
//! (`docs/hooks/serq_lexer.py`) cannot read them and is held equal to them
//! by `tests/docs_lexer.rs`.
//!
//! `vocabulary` is the same words read the other way, for completing them:
//! each word the language gives, with the role it is coloured by.

use crate::frontend::lexer::{Tok, Token, lex_partial};
use crate::frontend::link::{AGGREGATES, BUILTIN_ATTRS, CONTEXT_VARS, FOLDED, INF, supplied};
use crate::frontend::parser::ENGINE_VALUES;
use crate::ir::{DistKind, Fun};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The blocks a program is made of: `engine`, `session`, `let`.
    Structure,
    /// What a session or a server does: `hold`, `run`, `observe`.
    Statement,
    /// The knobs: `cap`, `evict`, `scheduler`.
    Option,
    /// What a program reads: `now`, `cachedin(kv)`, `running.count`.
    Observable,
    /// A draw, `~exp`.
    Random,
    Number,
    String,
    Comment,
    Operator,
    Punctuation,
    /// A name the program gives, or arithmetic (`min`, `ln`).
    Name,
    Space,
    /// What follows a lexical error, which the lexer did not read: a string
    /// not closed yet.
    Unlexed,
}

impl Role {
    /// The role's name, for a CSS class or a theme's key.
    pub fn name(self) -> &'static str {
        match self {
            Role::Structure => "structure",
            Role::Statement => "statement",
            Role::Option => "option",
            Role::Observable => "observable",
            Role::Random => "random",
            Role::Number => "number",
            Role::String => "string",
            Role::Comment => "comment",
            Role::Operator => "operator",
            Role::Punctuation => "punctuation",
            Role::Name => "name",
            Role::Space => "space",
            Role::Unlexed => "unlexed",
        }
    }
}

/// The blocks a program is made of.
pub const STRUCTURE: [&str; 18] = [
    "fn",
    "let",
    "def",
    "use",
    "device",
    "engine",
    "connector",
    "pool",
    "stage",
    "workload",
    "session",
    "server",
    "queue",
    "gateway",
    "link",
    "route",
    "gauge",
    "claim",
];

/// Statements, in the session and server blocks.
pub const STATEMENTS: [&str; 41] = [
    "Size",
    "Cost",
    "turn",
    "request",
    "set",
    "observe",
    "hold",
    "admit",
    "self",
    "grow",
    "shrink",
    "drop",
    "release",
    "load",
    "lease",
    "from",
    "to",
    "if",
    "while",
    "loop",
    "fork",
    "join",
    "choose",
    "end",
    "run",
    "reserve",
    "reuse",
    "cache",
    "prefix",
    "at",
    "admission",
    "growing",
    "on",
    "in",
    "by",
    "else",
    "sum",
    // the serving vocabulary: sugar over hold and run
    "prefill",
    "transfer",
    "decode",
    "tool",
];

/// Pool and stage options, workload forms, an engine's schedule and
/// execution, and a claim's forms. `advance`, `each` and `most` are read only
/// in the phrases `advance running` and `each at most`; `step` only to say an
/// engine is written instead. `queue`, `admit` and `while` are options too,
/// but a list above takes them first; they stay so that the list is the
/// docs lexer's word for word.
pub const OPTIONS: [&str; 64] = [
    "cap",
    "per_token",
    "save",
    "block",
    "evict",
    "lru",
    "preempt",
    "lifo",
    "leases",
    "suspend",
    "held",
    "requeue",
    "head",
    "tail",
    "none",
    "queue",
    "fifo",
    "admit",
    "while",
    "state",
    "taking",
    "via",
    "spill",
    "when",
    "ps",
    "delay",
    "cost",
    "granule",
    "serve",
    "latency",
    "nic",
    "exclusive",
    "first",
    "fit",
    "only",
    "arrive",
    "arrivals",
    "poisson",
    "renewal",
    "closed",
    "hidden",
    "send",
    "reply",
    "batch",
    "trace",
    "ordered",
    "init",
    "horizon",
    "warmup",
    "seed",
    "schedule",
    "scheduler",
    "advance",
    "each",
    "most",
    "execute",
    "worker",
    "forward",
    "step",
    "given",
    "every",
    "some",
    "iteration",
    "of",
];

/// A name the language supplies for a program to read: a context variable
/// or `inf` (the linker's `supplied`), a function that is not arithmetic, a
/// folded call, or a built-in attribute.
pub fn is_observable(word: &str) -> bool {
    supplied(word).is_some()
        || Fun::from_name(word).is_some_and(|f| !f.is_arithmetic())
        || FOLDED.contains(&word)
        || BUILTIN_ATTRS.contains(&word)
}

/// The role of a word standing alone; `call` when a `(` follows it, which
/// is what makes `count` a run's aggregate and not a name.
pub fn word_role(word: &str, call: bool) -> Role {
    if STRUCTURE.contains(&word) {
        Role::Structure
    } else if STATEMENTS.contains(&word) {
        Role::Statement
    } else if OPTIONS.contains(&word) {
        Role::Option
    } else if is_observable(word) || (call && AGGREGATES.iter().any(|(n, _)| *n == word)) {
        Role::Observable
    } else {
        Role::Name
    }
}

/// The program, cut into pieces with their roles. The pieces, concatenated,
/// are `src`; past a lexical error the rest is one `Unlexed` piece.
pub fn highlight(src: &str) -> Vec<(Role, String)> {
    let (toks, rest) = match lex_partial(src) {
        Ok(toks) => (toks, String::new()),
        Err(p) => (p.tokens, src.chars().skip(p.rest).collect()),
    };
    let mut out = Vec::new();
    for (t, role) in toks.iter().zip(roles(&toks)) {
        for tr in &t.leading {
            let r = if tr.is_comment() {
                Role::Comment
            } else {
                Role::Space
            };
            out.push((r, tr.text.clone()));
        }
        if t.tok != Tok::Eof {
            out.push((role, t.text.clone()));
        }
    }
    if !rest.is_empty() {
        out.push((Role::Unlexed, rest));
    }
    out
}

fn roles(toks: &[Token]) -> Vec<Role> {
    let word = |k: usize| match toks.get(k).map(|t| &t.tok) {
        Some(Tok::Ident(s)) => Some(s.as_str()),
        _ => None,
    };
    // nothing between this token and the one before it
    let tight = |k: usize| toks.get(k).is_some_and(|t| t.leading.is_empty());
    // only white space between them, as the words of one phrase
    let spaced = |k: usize| {
        toks.get(k)
            .is_some_and(|t| !t.leading.is_empty() && t.leading.iter().all(|x| !x.is_comment()))
    };
    let mut out = vec![Role::Name; toks.len()];
    let mut k = 0;
    while k < toks.len() {
        let (role, len) = match &toks[k].tok {
            Tok::Ident(w) => {
                let field = (tight(k + 1) && toks[k + 1].tok == Tok::Dot && tight(k + 2))
                    .then(|| word(k + 2))
                    .flatten();
                let next = word(k + 1).filter(|_| spaced(k + 1));
                if field.is_some_and(|f| {
                    ENGINE_VALUES
                        .iter()
                        .any(|(v, ..)| v.split_once('.') == Some((w.as_str(), f)))
                }) {
                    // an engine's value, `running.count`, read whole
                    (Role::Observable, 3)
                } else if matches!(
                    (w.as_str(), next),
                    ("advance", Some("running")) | ("admit", Some("waiting"))
                ) {
                    // a schedule's statements: `running` and `waiting` there
                    // name the engine's lists, not a value
                    (Role::Statement, 2)
                } else if w == "each"
                    && next == Some("at")
                    && word(k + 2) == Some("most")
                    && spaced(k + 2)
                {
                    (Role::Option, 3)
                } else if w == "tokens" && next == Some("cap") {
                    // `tokens cap B` is an engine's clause, not the context variable
                    (Role::Option, 1)
                } else {
                    let call = toks.get(k + 1).is_some_and(|t| {
                        t.tok == Tok::LParen && t.leading.iter().all(|x| !x.is_comment())
                    });
                    (word_role(w, call), 1)
                }
            }
            // a distribution's name is lower case, as the docs lexer reads it
            Tok::Tilde
                if tight(k + 1)
                    && word(k + 1).is_some_and(|w| {
                        w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                    }) =>
            {
                (Role::Random, 2)
            }
            Tok::Num(_) => (Role::Number, 1),
            Tok::Str(_) => (Role::String, 1),
            Tok::LBrace
            | Tok::RBrace
            | Tok::LParen
            | Tok::RParen
            | Tok::LBracket
            | Tok::RBracket
            | Tok::Comma
            | Tok::Semi
            | Tok::Dot => (Role::Punctuation, 1),
            Tok::Unread | Tok::Eof => (Role::Name, 1),
            _ => (Role::Operator, 1),
        };
        for r in &mut out[k..k + len] {
            *r = role;
        }
        k += len;
    }
    out
}

/// Every word the language gives a program, once, with the role it is
/// coloured by: the keywords, what a program reads (a context variable,
/// `inf`, an engine's value as `running.count`, a function, a folded call,
/// a built-in attribute, an aggregate), and a draw as `~exp`. Arithmetic
/// (`min`, `ln`) is a `Name`. For an editor to complete words from; it
/// reads the tables, so a word the language gains is offered with it.
pub fn vocabulary() -> Vec<(Role, String)> {
    let mut out: Vec<(Role, String)> = Vec::new();
    let mut add = |role: Role, w: &str| {
        if !out.iter().any(|(_, x)| x == w) {
            out.push((role, w.to_string()));
        }
    };
    for w in STRUCTURE {
        add(Role::Structure, w);
    }
    for w in STATEMENTS {
        add(Role::Statement, w);
    }
    for w in OPTIONS {
        add(Role::Option, w);
    }
    for (w, _) in CONTEXT_VARS {
        add(Role::Observable, w);
    }
    add(Role::Observable, INF);
    for (w, ..) in ENGINE_VALUES {
        add(Role::Observable, w);
    }
    for w in Fun::names() {
        add(word_role(w, true), w);
    }
    for w in FOLDED.into_iter().chain(BUILTIN_ATTRS) {
        add(Role::Observable, w);
    }
    for (w, _) in AGGREGATES {
        add(Role::Observable, w);
    }
    for d in DistKind::names() {
        add(Role::Random, &format!("~{d}"));
    }
    out
}

/// A parameter of a function a program may call: its name, as the
/// reference names it, and what it takes (`expr`, `stage` or `pool`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Param {
    pub name: &'static str,
    pub kind: &'static str,
}

/// A function of `ir::Fun`, as an editor offers it: for the keys a
/// `choose … by (…)` may read, and what each argument is. `blocksize` and
/// `cost`, which the linker folds, and the aggregates of a claim
/// (`total(o)`, …) are other tables'. The reference's index
/// (`docs/api/functions.md`) is a copy, held equal by a test.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Function {
    pub name: &'static str,
    pub params: Vec<Param>,
    /// What it reads: `arithmetic` (its arguments alone), `stage`, `pool`
    /// or `engine`.
    pub group: &'static str,
    /// What it returns, one sentence of Markdown (`code` in backticks).
    pub summary: &'static str,
}

/// A role a queue plays (`queue Q : prefill`), as an editor offers it: its
/// entry's verb, how many parameters the entry takes, and whether it is
/// written `from` the queue whose KV it reads (#645 B5, B8). The
/// language's table is `queue::ROLES`; the one in `docs/language.md` is a
/// copy, held equal by a test.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct QueueRole {
    pub name: &'static str,
    /// The entry's verb, `prefill`, or null for a role with none (`l2`).
    pub verb: Option<&'static str>,
    pub params: usize,
    /// Its entry is written `… from Q`, the queue whose KV it reads.
    pub from: bool,
    /// The queue may have no entry, its `serve` the cost (`link`).
    pub entry_optional: bool,
}

/// Every role of the serving vocabulary, in the table's order.
pub fn queue_roles() -> Vec<QueueRole> {
    crate::frontend::queue::ROLES
        .iter()
        .map(|r| QueueRole {
            name: r.name,
            verb: (!r.verb.is_empty()).then_some(r.verb),
            params: r.params,
            from: r.from,
            entry_optional: r.entry_optional,
        })
        .collect()
}

/// Every function of `ir::Fun`, in the table's order.
pub fn functions() -> Vec<Function> {
    Fun::all()
        .map(|f| Function {
            name: f.name(),
            params: f
                .params()
                .iter()
                .zip(f.signature())
                .map(|(&name, &kind)| Param {
                    name,
                    kind: kind.name(),
                })
                .collect(),
            group: f.group().name(),
            summary: summary(f),
        })
        .collect()
}

/// What a function returns: the reference's `Returns`, its first sentence.
/// A match, so a new `Fun` does not compile without one.
fn summary(f: Fun) -> &'static str {
    match f {
        Fun::Min => "The smaller argument.",
        Fun::Max => "The larger argument.",
        Fun::Abs => "The absolute value of `x`.",
        Fun::Floor => "The greatest integer less than or equal to `x`, represented as a number.",
        Fun::Ceil => "The least integer greater than or equal to `x`, represented as a number.",
        Fun::Sqrt => "The square root of `x`.",
        Fun::Exp => "The exponential `eˣ`.",
        Fun::Ln => "The natural logarithm of `x`.",
        Fun::Queue => "Jobs present at `s`, including waiting and in-service jobs.",
        Fun::Busy => {
            "Jobs in service: active servers for `fifo`, scheduled jobs in the current \
             iteration for `step`."
        }
        Fun::Work => "Unfinished work at `s`, in the stage's work unit.",
        Fun::Used => "Units currently allocated in `p`.",
        Fun::Free => "`cap - used`. Cached units are reclaimable and are not subtracted.",
        Fun::CachedIn => {
            "The cached units in `p` of this session's chain there (its waiting hold's if that \
             hold asks for `p`, else its innermost hold of `p`'s, else the chain it or one of \
             its legs last cached along in `p`, else its own), in whole blocks; 0 when \
             evaluated without a session."
        }
        Fun::Allocated => {
            "This session's own allocated units in `p`, in its holds and its leases; 0 when \
             evaluated without a session."
        }
        Fun::Holders => "Sessions holding units in `p`.",
        Fun::Queued => "Sessions waiting in `p`'s admission queue.",
        Fun::Price => "An online estimate of the added cost of a cache miss, in clock units.",
        Fun::BudgetLeft => {
            "Tokens available to admissions after planning service for the running requests."
        }
        Fun::EstLambda => "Measured arrival rate at `s`, per clock unit.",
        Fun::EstRho => "Measured utilization at `s`, capped below 1.",
        Fun::EstWait => {
            "Measured mean wait at `s` from a run's arrival to its start, a run's `latency` \
             included, in clock units."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles_of(src: &str) -> Vec<(Role, String)> {
        highlight(src)
            .into_iter()
            .filter(|(r, _)| *r != Role::Space)
            .collect()
    }

    fn role(src: &str, piece: &str) -> Role {
        roles_of(src)
            .into_iter()
            .find(|(_, t)| t == piece)
            .unwrap_or_else(|| panic!("no {piece:?} in {src:?}"))
            .0
    }

    #[test]
    fn the_pieces_are_the_source() {
        for src in [
            "pool kv { cap 3e5; } // c\n x <= ~exp(1.5)",
            "let a = \"open",
            "/* open",
            "let a = 1; @ let b = 2;",
            "x = 1.2.3;",
            "let a = 1;\nlet s = \"two\nlines",
            "",
        ] {
            let joined: String = highlight(src).into_iter().map(|(_, t)| t).collect();
            assert_eq!(joined, src);
        }
        // each error site leaves the rest from the item it could not read
        for (src, rest) in [
            ("let a = \"open", "\"open"),
            ("a /* open", "/* open"),
            ("x = 1.2.3;", "1.2.3;"),
            ("a @ b", "@ b"),
        ] {
            assert_eq!(
                highlight(src).last().unwrap(),
                &(Role::Unlexed, rest.into())
            );
        }
    }

    #[test]
    fn four_roles() {
        let src = "engine E { cap 4; } session { hold kv (now); }";
        assert_eq!(role(src, "engine"), Role::Structure);
        assert_eq!(role(src, "hold"), Role::Statement);
        assert_eq!(role(src, "cap"), Role::Option);
        assert_eq!(role(src, "now"), Role::Observable);
        assert_eq!(role(src, "E"), Role::Name);
    }

    #[test]
    fn a_word_in_a_comment_is_a_comment() {
        let src = "// hold now\nlet x = 1;";
        assert_eq!(role(src, "// hold now"), Role::Comment);
        assert_eq!(role(src, "let"), Role::Structure);
    }

    #[test]
    fn phrases_are_read_whole() {
        let src = "x = running.count + ~exp(1) + min(1, 2) + count(o) + count;";
        let r = roles_of(src);
        let at = r.iter().position(|(_, t)| t == "running").unwrap();
        assert!(r[at..at + 3].iter().all(|(r, _)| *r == Role::Observable));
        assert_eq!(role(src, "~"), Role::Random);
        assert_eq!(role(src, "exp"), Role::Random);
        assert_eq!(role(src, "min"), Role::Name);
        let counts: Vec<Role> = r
            .iter()
            .filter(|(_, t)| t == "count")
            .map(|p| p.0)
            .collect();
        // `running.count`'s field, the aggregate called, and a name
        assert_eq!(counts, [Role::Observable, Role::Observable, Role::Name]);
        assert_eq!(role("advance running;", "running"), Role::Statement);
        assert_eq!(role("each at most (2)", "most"), Role::Option);
        assert_eq!(role("tokens cap B", "tokens"), Role::Option);
        assert_eq!(role("x = tokens;", "tokens"), Role::Observable);
    }

    #[test]
    fn what_follows_an_error_is_unlexed() {
        let r = roles_of("let a = 1; let s = \"open");
        assert_eq!(r[0].0, Role::Structure);
        assert_eq!(r.last().unwrap(), &(Role::Unlexed, "\"open".to_string()));
    }

    /// Each word is offered with the role it is coloured by, written as a
    /// call (which an aggregate needs to be one); every keyword that is
    /// coloured is offered, and a distribution as a draw. A keyword left
    /// plain (`budget`, kept to say what replaced it) is not.
    #[test]
    fn the_vocabulary_is_coloured_as_offered() {
        let v = vocabulary();
        let offered = |w: &str| v.iter().any(|(_, x)| x == w);
        for (role, w) in &v {
            assert_eq!(highlight(&format!("{w}(x)"))[0].0, *role, "{w}");
        }
        for k in crate::frontend::parser::KEYWORDS {
            if highlight(&format!("{k}(x)"))[0].0 != Role::Name {
                assert!(offered(k), "{k} is coloured, not offered");
            }
        }
        for d in DistKind::names() {
            assert!(offered(&format!("~{d}")), "~{d}");
        }
        assert!(!offered("budget"));
        assert!(v.iter().any(|(r, w)| *r == Role::Name && w == "min"));
    }

    /// Every example reads whole, and is its pieces again.
    #[test]
    fn every_example_reads_whole() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "sq") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples"),
            &mut files,
        );
        assert!(files.len() > 20, "found {} examples", files.len());
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            let pieces = highlight(&src);
            let joined: String = pieces.iter().map(|(_, t)| t.as_str()).collect();
            assert_eq!(joined, src, "{}", f.display());
            assert!(
                pieces.iter().all(|(r, _)| *r != Role::Unlexed),
                "{} does not lex",
                f.display()
            );
        }
    }
}
