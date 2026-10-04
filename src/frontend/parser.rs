//! Recursive-descent parser for serQ programs.
//!
//! ```text
//! program  := item*
//! item     := 'let' IDENT '=' expr ';'
//!           | 'pool' IDENT ('[' NUM ']')? '{' poolopt* '}'
//!           | 'stage' IDENT ('[' NUM ']')? ':' kind ';'
//!           | 'workload' '{' wlitem* '}'
//!           | 'session' block
//!           | 'server' block
//!           | 'queue' IDENT ('[' expr ']')? (':' IDENT (',' IDENT)*)? '{' qitem* '}'
//!           | 'run' '{' ('horizon' | 'warmup' | 'seed' | 'arrivals') expr ';' ... '}'
//!           | 'gauge' IDENT '=' expr ';'      -- a time average of the deployment's state
//! qitem    := 'pool' IDENT '{' poolopt* '}' | 'serve' kind
//!           | IDENT ('(' IDENT (',' IDENT)* ')')? ('from' IDENT)? block   -- an entry (crate::frontend::queue)
//! poolopt  := 'cap' expr ';' | 'block' expr ';'
//!           | 'evict' ('lru' | 'by' '(' expr (',' expr)* ')') ';'
//!           | 'preempt' ('lifo' | 'none') ';'
//!           | 'queue' ('fifo' | 'by' '(' expr (',' expr)* ')') ';'
//!           | 'spill' IDENT 'via' IDENT '(' expr ')' 'when' '(' expr ')' ';'
//! kind     := 'fifo' ('(' expr ')')? | 'ps' '(' expr ')' | 'delay'
//!           | 'step' '{' stepopt* '}'
//! stepopt  := 'budget' expr ';' | 'cost' expr ';' | 'chunk' expr ';'
//!           | 'serve' ('only' '(' expr ')')?
//!                     ('admission' | 'decode' 'first' | 'exclusive' 'prefill'
//!                     | 'by' '(' expr (',' expr)* ')') ';'   -- one of the two at least;
//!                                                          -- not `only` with `exclusive prefill`
//!           | 'memory' IDENT ';'
//! wlitem   := 'arrive' ('poisson' '(' expr ')' | 'renewal' '(' expr ')' | 'closed' '(' expr ')' | 'batch' '(' expr ')' | 'none') ';'
//!           | 'trace' STRING ('ordered')? ';' | 'init' block | 'turn' block
//!           | 'session' block                  -- the session's side, with 'request'
//!           | 'hidden' IDENT (',' IDENT)* ';'
//! block    := '{' stmt* '}'
//! stmt     := 'turn' ';' | 'request' IDENT? ';' | 'set' IDENT '=' expr ';' | 'observe' IDENT '=' expr ';'
//!           | 'hold' ref '(' expr ')' ('reserve' '(' expr ')')?
//!                 (',' ref '(' expr ')' ('reserve' '(' expr ')')?)* ('reuse' '(' expr ')')?
//!                 ('at' 'admission' '(' IDENT '=' expr (',' IDENT '=' expr)* ')')?
//!                 block ('cache' '(' expr ')')? ('lease' ref '(' expr ')')? ';'?
//!           | 'grow' ref '(' expr ')' ';' | 'drop' ref ';'
//!           | 'run' ref ('prefill' | 'decode')? '(' expr ')' ('growing' ref)? ';'
//!           | 'branch' ('with')? '(' expr ')' block ('else' block)?
//!           | 'loop' block | 'end' ';'
//!           | 'choose' IDENT 'in' expr 'by' '(' expr (',' expr)* ')' ';'
//!           | serving
//! serving  := role ('[' expr ']' | 'on' ref)? expr ('growing' ref)? ';'
//!           | 'transfer' ('[' expr ']' | 'on' ref)? expr 'from' ref 'to' ref '(' expr ')' ';'
//! role     := 'prefill' | 'decode' | 'tool'
//! ref      := IDENT ('[' expr ']')?
//! atom     := NUM | '(' expr ')' | IDENT | IDENT '(' arg (',' arg)* ')' | over
//! over     := ('max' | 'min' | 'sum') IDENT 'in' (NUM | IDENT | '(' expr ')') '(' expr ')'
//! ```
//!
//! The serving forms (`serving`) are sugar: they are rewritten to `hold`
//! and `run` here, so the AST, the IR and the interpreter know only the
//! kernel. A role finds its stage among the stages declared above the
//! statement: the stage of the role's name (`prefill`, `link` or
//! `transfer`, `decode`, `tool`), else, for `prefill` and `decode`, the
//! `step` engine; `on STAGE` names it explicitly. On a step engine the run
//! gets the role's mode (`run E prefill (S)`), elsewhere it is plain.
//!
//! The two sides. `workload { … session { … request; … } }` and
//! `server { … }` are one session written from its two sides: the client's
//! (arrivals, turns, thinking, whether to go on) and the server's (what
//! the deployment does with one request). The parser splices the server's
//! statements in place of every `request;`, so the AST holds one session
//! and the IR is the one the same program written as `session { … }`
//! compiles to. Each side has its words: `request`, `turn` and `end` are
//! the session's and are refused in a `server`. An admission is written
//! one way on both sides: `hold … at admission (…) { … } cache (…)`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::frontend::ast::*;
use crate::frontend::diagnostic::Source;
use crate::frontend::lexer::{LexError, Tok, Token, lex};
use crate::frontend::link::{BUILTIN_ATTRS, CONTEXT_VARS, FOLDED, FUNCTIONS};
use crate::frontend::queue::{self, QueueDecl};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
    /// The library the error is in, when it is not in the program.
    pub origin: Option<Source>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(lib) = &self.origin {
            write!(f, "{}:", lib.path)?;
        }
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

impl ParseError {
    pub fn render(&self, source: &str) -> String {
        // an error about the command line (`--def`) has no place in the text
        if self.line == 0 {
            return self.msg.clone();
        }
        let span = Span {
            line: self.line,
            col: self.col,
            len: 1,
            file: 0,
        };
        match &self.origin {
            Some(lib) => format!("{}:{}", lib.path, span.render(&lib.text, &self.msg)),
            None => span.render(source, &self.msg),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            line: e.line,
            col: e.col,
            msg: e.msg,
            origin: None,
        }
    }
}

type PResult<T> = Result<T, ParseError>;

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// The stages declared so far, (name, is a step engine): what the
    /// serving forms resolve their stage against.
    stages: Vec<(String, bool)>,
    definitions: Vec<(String, Span)>,
    /// Which side the statement being parsed is on.
    side: Side,
    /// The workload's `session` block and the `server` block, each with the
    /// position of its keyword, until `assemble` puts them together.
    wl_session: Option<(usize, Vec<Stmt>)>,
    server: Option<(usize, Vec<Stmt>)>,
    /// Where `share` was given, for the error that it is given twice.
    share_at: Option<usize>,
    /// Header bindings the body of their hold reads, with the position of
    /// the name: `set` at the top of the body, checked once the program's
    /// attributes are known (`check_body_bindings`).
    body_binds: Vec<(usize, String, Expr)>,
    /// The token position of each name `bindings` read, in order.
    bind_at: Vec<usize>,
    /// The `def`s so far, in order: a use expands to its body in place.
    defs: Vec<Def>,
    /// The token ranges uses were expanded into, innermost last, to say
    /// where an error inside one was used.
    expanded: Vec<Expanded>,
    /// Uses whose arguments read names a `turn;` or `request;` in the body
    /// might assign, which only the whole program says: checked at its end.
    deferred: Vec<Deferred>,
    /// The directory of the program's file, which a `use` reads next to;
    /// none for a program given as text.
    base: Option<PathBuf>,
    /// The libraries read so far (file `n` is `libs[n - 1]`), the
    /// directory of each, and each one's canonical path, read once.
    libs: Vec<Source>,
    lib_dirs: Vec<PathBuf>,
    /// Each library's directory as the program named it, for display.
    lib_shown_dirs: Vec<PathBuf>,
    read: Vec<PathBuf>,
    /// `--def name=expr`: the body an expression definition has instead of
    /// its own, and whether the program defined it.
    def_overrides: Vec<(String, String, bool)>,
    /// The queues declared so far; their entries are expanded in `assemble`.
    queues: Vec<QueueDecl>,
    /// `let` constants with a constant value, for a family's size.
    consts: Vec<(String, f64)>,
    /// Constants whose values can change through --set, including dependents.
    structural_overrides: Vec<String>,
    /// The queue whose entry is being parsed.
    in_queue: Option<usize>,
    /// The `from` name of the entry being parsed.
    entry_from: Option<String>,
    /// The program's `share` was given by a pull relation.
    relation_share: bool,
    /// Parsing the `serve` of a link queue, which may take a `latency`.
    latency_ok: bool,
    /// `Q.x` read as an expression, with the position: a mark or a pool of `Q`.
    dotted_reads: Vec<(usize, String)>,
    /// `Q[i].x` references, checked once the queues are known: `x` must be
    /// a pool of `Q`.
    indexed_dotted: Vec<(usize, String)>,
    /// Request sites and their explicit gateway, resolved after declarations.
    requests: Vec<(usize, Option<String>)>,
}

/// A use of a `def` whose body says `turn;` or `request;`, with the names
/// its arguments read.
struct Deferred {
    name: String,
    line: usize,
    col: usize,
    file: usize,
    reads: Vec<String>,
    turn: bool,
    /// Where its `request`s go.
    request: Vec<Sends>,
}

/// Where a `request` in a `def` body goes.
#[derive(Clone, Debug, PartialEq)]
enum Sends {
    /// `request;`: the anonymous server.
    Server,
    /// `request gw;`: the gateway `gw`.
    Gateway(String),
    /// `request p;` with `p` the definition's `k`-th parameter: the
    /// gateway its argument names.
    Param(usize),
    /// A gateway an argument names other than by its name: any of them.
    Any,
}

impl Sends {
    /// The statement, as an error names it.
    fn says(&self) -> String {
        match self {
            Sends::Server => "`request;`".into(),
            Sends::Gateway(g) => format!("`request {g};`"),
            Sends::Param(_) | Sends::Any => "`request`".into(),
        }
    }

    /// Where it goes at a use with these arguments, `params` being those
    /// of the definition the use is in (none at the top level).
    fn at(&self, args: &[Vec<Token>], params: &[String]) -> Sends {
        let Sends::Param(i) = self else {
            return self.clone();
        };
        match args.get(*i).map(Vec::as_slice) {
            Some([t]) => match &t.tok {
                Tok::Ident(g) => match params.iter().position(|p| p == g) {
                    Some(j) => Sends::Param(j),
                    None => Sends::Gateway(g.clone()),
                },
                _ => Sends::Any,
            },
            _ => Sends::Any,
        }
    }
}

/// The arguments of each use `name(a, …)` in these tokens, split at the
/// commas outside brackets.
fn uses_args(toks: &[Token], name: &str) -> Vec<Vec<Vec<Token>>> {
    let mut out = vec![];
    for (k, w) in toks.windows(2).enumerate() {
        if !(matches!(&w[0].tok, Tok::Ident(n) if n == name) && w[1].tok == Tok::LParen) {
            continue;
        }
        let mut args = vec![];
        let mut cur = vec![];
        let mut depth = 0usize;
        for t in &toks[k + 2..] {
            match t.tok {
                Tok::RParen if depth == 0 => break,
                Tok::Comma if depth == 0 => {
                    args.push(std::mem::take(&mut cur));
                    continue;
                }
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            cur.push(t.clone());
        }
        if !cur.is_empty() || !args.is_empty() {
            args.push(cur);
        }
        out.push(args);
    }
    out
}

/// What a request assigns, by where it goes.
#[derive(Default)]
struct Served {
    /// Whatever it goes to: `cached`, `computed`, the entries' locals and
    /// marks.
    shared: Vec<String>,
    /// The server's assignments.
    server: Vec<String>,
    /// Each gateway's `route`'s.
    gateways: Vec<(String, Vec<String>)>,
}

impl Served {
    /// Does a request sent `to` assign `n`?
    fn assigns(&self, to: &Sends, n: &str) -> bool {
        let n = n.to_string();
        self.shared.contains(&n)
            || match to {
                Sends::Server => self.server.contains(&n),
                Sends::Gateway(g) => self
                    .gateways
                    .iter()
                    .any(|(q, names)| q == g && names.contains(&n)),
                Sends::Param(_) | Sends::Any => {
                    self.server.contains(&n) || self.gateways.iter().any(|(_, ns)| ns.contains(&n))
                }
            }
    }
}

/// The tokens a use of a `def` became, and where it was used.
struct Expanded {
    start: usize,
    end: usize,
    name: String,
    line: usize,
    col: usize,
    file: usize,
}

/// `def name(x, y) = e;` or `def name(x, y) { statements }`: a name for
/// source a program would otherwise repeat. A use is replaced by the body's
/// tokens, each parameter by its argument's, and parsed where it stands, so
/// the AST, the IR and everything after know nothing of it.
#[derive(Clone)]
struct Def {
    name: String,
    params: Vec<String>,
    /// An expression (`= e;`) or statements (`{ … }`).
    stmts: bool,
    body: Vec<Token>,
    /// The body draws, itself or through a definition it uses.
    draws: bool,
    /// What the body assigns, itself or through a definition it uses: its
    /// `set`s, `choose`s and bindings, and `cached` and `computed` if it
    /// holds. An argument that reads one would read the body's value.
    assigns: Vec<String>,
    /// The names the body reads other than its parameters, and the
    /// functions of pool or stage state it calls, itself or through a
    /// definition it uses: what an argument that uses it reads.
    reads: Vec<String>,
    calls: Vec<String>,
    /// The body says `turn;`, itself or through a definition.
    turn: bool,
    /// Where the body's `request`s go, itself or through a definition.
    request: Vec<Sends>,
    /// Where the name is written.
    line: usize,
    col: usize,
    file: usize,
}

/// Tokens a program may expand to. Definitions use only earlier ones, so
/// an expansion ends; one can still double at every level.
const MAX_TOKENS: usize = 100_000;

/// The most members a family (`pool p[N]`, `queue D[N]`) may have.
const MAX_FAMILY: usize = 10_000;

/// The distributions of `~name(…)`, which a `def` may not be named.
const DISTRIBUTIONS: [&str; 6] = ["exp", "det", "uniform", "erlang", "h2", "bernoulli"];

/// Every word the grammar reads as a keyword somewhere. A `def` or a
/// parameter may not be one: a parameter is replaced token by token, and a
/// keyword in the body is a token of the same spelling. `tests/docs_lexer.rs`
/// keeps the list whole.
pub const KEYWORDS: [&str; 89] = [
    "admission",
    "admit",
    "arrivals",
    "arrive",
    "at",
    "batch",
    "bernoulli",
    "block",
    "bottleneck",
    "branch",
    "budget",
    "by",
    "cache",
    "cap",
    "choose",
    "chunk",
    "closed",
    "cost",
    "decode",
    "def",
    "delay",
    "drop",
    "else",
    "end",
    "evict",
    "exclusive",
    "fifo",
    "first",
    "fits",
    "from",
    "gauge",
    "grow",
    "growing",
    "hidden",
    "hold",
    "horizon",
    "in",
    "init",
    "latency",
    "lease",
    "let",
    "lifo",
    "link",
    "load",
    "loop",
    "lru",
    "mark",
    "maxmin",
    "memory",
    "nic",
    "none",
    "observe",
    "on",
    "only",
    "ordered",
    "poisson",
    "pool",
    "preempt",
    "prefill",
    "ps",
    "pull",
    "queue",
    "release",
    "renewal",
    "request",
    "reserve",
    "reuse",
    "run",
    "seed",
    "serve",
    "server",
    "session",
    "set",
    "share",
    "spill",
    "stage",
    "step",
    "sum",
    "to",
    "tool",
    "trace",
    "transfer",
    "turn",
    "use",
    "via",
    "warmup",
    "when",
    "with",
    "workload",
];

/// Where a statement sits: a top-level `session`, the `session` inside
/// `workload` (the only place `request` is a statement) or `server`.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Session,
    WorkloadSession,
    Server,
}

/// A serving form: a statement that desugars to `run` on the stage that
/// plays the role.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Prefill,
    Transfer,
    Decode,
    Tool,
}

impl Role {
    fn of(kw: &str) -> Option<Role> {
        match kw {
            "prefill" => Some(Role::Prefill),
            "transfer" => Some(Role::Transfer),
            "decode" => Some(Role::Decode),
            "tool" => Some(Role::Tool),
            _ => None,
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            Role::Prefill => "prefill",
            Role::Transfer => "transfer",
            Role::Decode => "decode",
            Role::Tool => "tool",
        }
    }

    /// The stage names that play the role by default.
    fn names(self) -> &'static [&'static str] {
        match self {
            Role::Prefill => &["prefill"],
            Role::Transfer => &["link", "transfer"],
            Role::Decode => &["decode"],
            Role::Tool => &["tool"],
        }
    }

    /// The run mode on a step engine, for the roles an engine plays.
    fn step_mode(self) -> Option<RunMode> {
        match self {
            Role::Prefill => Some(RunMode::Prefill),
            Role::Decode => Some(RunMode::Decode),
            Role::Transfer | Role::Tool => None,
        }
    }
}

pub fn parse(src: &str) -> PResult<Program> {
    parse_at(src, None)
}

/// Parse a program read from a file in `base`, next to which its `use`s
/// read their libraries.
pub fn parse_at(src: &str, base: Option<&Path>) -> PResult<Program> {
    parse_with(src, base, None, &[], &[])
}

/// `parse_at`, with the bodies `--def` gives expression definitions and
/// the constants `--set` overrides, which may not size a queue family.
pub fn parse_at_with(
    src: &str,
    base: Option<&Path>,
    defs: &[(String, String)],
    lets: &[String],
) -> PResult<Program> {
    parse_with(src, base, None, defs, lets)
}

/// Parse the program file `path`, whose text is `src`. The file counts as
/// read, so a library that `use`s it back is not read into it again.
pub fn parse_file(src: &str, path: &Path) -> PResult<Program> {
    parse_file_with(src, path, &[], &[])
}

/// `parse_file`, with `parse_at_with`'s overrides.
pub fn parse_file_with(
    src: &str,
    path: &Path,
    defs: &[(String, String)],
    lets: &[String],
) -> PResult<Program> {
    let root = path.canonicalize().ok();
    parse_with(src, path.parent(), root, defs, lets)
}

fn parse_with(
    src: &str,
    base: Option<&Path>,
    root: Option<PathBuf>,
    defs: &[(String, String)],
    lets: &[String],
) -> PResult<Program> {
    let toks = lex(src)?;
    let mut p = Parser::new(toks);
    p.base = base.map(Path::to_path_buf);
    p.read = root.into_iter().collect();
    p.def_overrides = defs
        .iter()
        .map(|(n, e)| (n.clone(), e.clone(), false))
        .collect();
    p.structural_overrides = lets.to_vec();
    let prog = p.program()?;
    let unknown: Vec<&str> = p
        .def_overrides
        .iter()
        .filter(|d| !d.2)
        .map(|d| d.0.as_str())
        .collect();
    if let Some(name) = unknown.first() {
        let mut known: Vec<&str> = p
            .defs
            .iter()
            .filter(|d| !d.stmts)
            .map(|d| d.name.as_str())
            .collect();
        known.sort();
        return Err(ParseError {
            line: 0,
            col: 0,
            msg: format!(
                "unknown --def `{name}`\nhelp: --def replaces the body of a declared `def NAME(...) = expr;`; \
                 the program declares: {}",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ),
            origin: None,
        });
    }
    Ok(prog)
}

/// Parse a standalone expression (used by `--set name=expr` on the CLI).
pub fn parse_expr(src: &str) -> PResult<Expr> {
    let toks = lex(src)?;
    let mut p = Parser::new(toks);
    let e = p.expr()?;
    p.expect(&Tok::Eof)?;
    Ok(e)
}

/// Replace every `Var(name)` of a header binding (`at admission (…)`,
/// `where …`) by its expression.
fn subst(e: &mut Expr, binds: &[(String, Expr)]) {
    match e {
        Expr::Located(_, inner) => subst(inner, binds),
        Expr::Var(n) => {
            if let Some((_, v)) = binds.iter().find(|(name, _)| name == n) {
                *e = v.clone();
            }
        }
        Expr::Num(_) => {}
        Expr::Sample(_, args) => args.iter_mut().for_each(|a| subst(a, binds)),
        Expr::Call(_, args) => args.iter_mut().for_each(|a| match a {
            Arg::Expr(x) => subst(x, binds),
            Arg::Ref(r) => {
                // a bare identifier argument is parsed as a reference (it may
                // name a pool or a stage); when it names a binding it is the
                // binding, else `min(known, …)` would read the attribute
                // `known` and not the header's `where known = …`
                if r.index.is_none()
                    && let Some((_, v)) = binds.iter().find(|(name, _)| *name == r.name)
                {
                    *a = Arg::Expr(v.clone());
                } else if let Some(i) = &mut r.index {
                    subst(i, binds);
                }
            }
        }),
        Expr::Unary(_, a) => subst(a, binds),
        Expr::Binary(_, a, b) => {
            subst(a, binds);
            subst(b, binds);
        }
        Expr::Cond(c, a, b) => {
            subst(c, binds);
            subst(a, binds);
            subst(b, binds);
        }
        Expr::Over(_, j, n, body) => {
            subst(n, binds);
            // the index is the body's own `j`
            let inner: Vec<_> = binds
                .iter()
                .filter(|(name, _)| name != j)
                .cloned()
                .collect();
            subst(body, &inner);
        }
    }
}

/// Does this expression draw?
fn has_sample(e: &Expr) -> bool {
    match e {
        Expr::Located(_, inner) => has_sample(inner),
        Expr::Sample(..) => true,
        Expr::Num(_) | Expr::Var(_) => false,
        Expr::Call(_, args) => args.iter().any(|a| match a {
            Arg::Expr(x) => has_sample(x),
            Arg::Ref(r) => r.index.as_ref().is_some_and(|i| has_sample(i)),
        }),
        Expr::Unary(_, a) => has_sample(a),
        Expr::Binary(_, a, b) => has_sample(a) || has_sample(b),
        Expr::Cond(c, a, b) => has_sample(c) || has_sample(a) || has_sample(b),
        Expr::Over(_, _, n, e) => has_sample(n) || has_sample(e),
    }
}

/// The names `set` or `choose` assigns anywhere in `stmts`.
pub(crate) fn assigned_in(stmts: &[Stmt], out: &mut Vec<String>) {
    for s in stmts {
        match s {
            Stmt::Set(n, _) | Stmt::Choose { var: n, .. } => out.push(n.clone()),
            Stmt::Hold { body, .. } | Stmt::Loop(body) => assigned_in(body, out),
            Stmt::Branch(_, a, b) => {
                assigned_in(a, out);
                assigned_in(b, out);
            }
            _ => {}
        }
    }
}

/// An argument of the statements `def` reads the clock or live state,
/// which the body would read where it reads the parameter.
fn live_message(def: &str, p: &str, what: &str) -> String {
    let of = if p.is_empty() {
        String::from("an argument")
    } else {
        format!("the argument for `{p}`")
    };
    format!(
        "{of} of `{def}` reads `{what}`, which changes while statements run: the body would \
         read it where it reads the parameter, not here\nhelp: `set` the value first \
         (`set t = {what};`) and pass the name"
    )
}

/// An argument of `def` reads `n`, which the body assigns before it reads
/// the parameter `p`.
fn capture_message(def: &str, p: &str, n: &str) -> String {
    format!(
        "the argument for `{p}` reads `{n}`, which `{def}` assigns: it would read the \
         body's `{n}`, not this one\nhelp: `set` the value under another name first and \
         pass that; a key over a `choose` of the body is written where the `choose` is"
    )
}

/// The names a body's tokens assign: `set n =`, `choose n`, a binding
/// `n =` (not an observation's name, which is no attribute), and `cached`
/// and `computed` if it holds, which its admission sets.
fn assigned_tokens(b: &[Token]) -> Vec<String> {
    let mut out: Vec<String> = (0..b.len())
        .filter_map(|k| {
            let Tok::Ident(n) = &b[k].tok else {
                return None;
            };
            let prev = k.checked_sub(1).map(|j| &b[j].tok);
            let chosen = prev == Some(&Tok::Ident("choose".into()));
            let assigned = b.get(k + 1).is_some_and(|t| t.tok == Tok::Assign)
                && prev != Some(&Tok::Ident("observe".into()));
            (chosen || assigned).then(|| n.clone())
        })
        .collect();
    let holds = b
        .iter()
        .any(|t| matches!(&t.tok, Tok::Ident(k) if k == "hold"));
    if holds {
        out.extend(["cached".to_string(), "computed".to_string()]);
    }
    out
}

/// The functions of a value alone; the others read pool or stage state.
const PURE: [&str; 9] = [
    "min", "max", "abs", "floor", "ceil", "sqrt", "exp", "ln", "pow",
];

/// The words #136 took out of the language, with what a program writes
/// instead: one spelling for one admission.
fn retired(word: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|(w, _)| *w == word)
        .map(|(_, now)| *now)
}

const RETIRED: [(&str, &str); 5] = [
    (
        "enter",
        "`enter` is now `hold`: `hold P (u) … at admission (x = e) { … } cache (ℓ);`",
    ),
    (
        "admit",
        "`admit if … fit where x = e { … } keep (ℓ)` is now `hold … at admission (x = e) \
         { … } cache (ℓ)` (the pool option `admit via` is unchanged)",
    ),
    ("keep", "`keep` is now `cache`"),
    ("where", "`where x = e` is now `at admission (x = e)`"),
    (
        "fit",
        "`fit` is gone: the pools of a `hold` are the condition",
    ),
];

/// Do these tokens say the statement `w;`?
fn says(b: &[Token], w: &str) -> bool {
    b.windows(2)
        .any(|x| x[0].tok == Tok::Ident(w.into()) && x[1].tok == Tok::Semi)
}

/// Where the `request;` and `request gw;` these tokens say go, `params`
/// being the definition's.
fn sends_of(b: &[Token], params: &[String]) -> Vec<Sends> {
    let mut out = vec![];
    for (k, t) in b.iter().enumerate() {
        if t.tok != Tok::Ident("request".into()) {
            continue;
        }
        match (b.get(k + 1).map(|t| &t.tok), b.get(k + 2).map(|t| &t.tok)) {
            (Some(Tok::Semi), _) => out.push(Sends::Server),
            (Some(Tok::Ident(g)), Some(Tok::Semi)) => {
                out.push(match params.iter().position(|p| p == g) {
                    Some(i) => Sends::Param(i),
                    None => Sends::Gateway(g.clone()),
                })
            }
            _ => {}
        }
    }
    out
}

/// A function of the language: one the linker resolves or one it folds.
fn is_function(name: &str) -> bool {
    FUNCTIONS.contains(&name) || FOLDED.contains(&name)
}

/// Do these tokens use the definition `name`, `name(`?
fn uses(toks: &[Token], name: &str) -> bool {
    toks.windows(2)
        .any(|w| matches!(&w[0].tok, Tok::Ident(n) if n == name) && w[1].tok == Tok::LParen)
}

/// Is this argument a reference as written, `kv` or `kvD[j]`?
fn is_reference(a: &[Token]) -> bool {
    match a {
        [t] => matches!(t.tok, Tok::Ident(_)),
        [t, open, .., close] => {
            matches!(t.tok, Tok::Ident(_))
                && open.tok == Tok::LBracket
                && close.tok == Tok::RBracket
                && {
                    // the bracket that opens is the one that closes
                    let mut depth = 0i32;
                    a[1..].iter().enumerate().all(|(k, t)| {
                        match t.tok {
                            Tok::LBracket => depth += 1,
                            Tok::RBracket => depth -= 1,
                            _ => {}
                        }
                        depth > 0 || k == a.len() - 2
                    })
                }
        }
        _ => false,
    }
}

fn is_context_var(v: &str) -> bool {
    CONTEXT_VARS.iter().any(|(name, _)| *name == v)
}

/// Does this expression read the name `n` (an attribute, a constant, or a
/// binding)?
fn expr_reads(e: &Expr, n: &str) -> bool {
    match e {
        Expr::Located(_, inner) => expr_reads(inner, n),
        Expr::Var(v) => v == n,
        Expr::Num(_) => false,
        Expr::Sample(_, args) => args.iter().any(|a| expr_reads(a, n)),
        Expr::Call(_, args) => args.iter().any(|a| match a {
            Arg::Expr(x) => expr_reads(x, n),
            Arg::Ref(r) => ref_reads(r, n) || (r.index.is_none() && r.name == n),
        }),
        Expr::Unary(_, a) => expr_reads(a, n),
        Expr::Binary(_, a, b) => expr_reads(a, n) || expr_reads(b, n),
        Expr::Cond(c, a, b) => expr_reads(c, n) || expr_reads(a, n) || expr_reads(b, n),
        Expr::Over(_, j, m, e) => expr_reads(m, n) || (j != n && expr_reads(e, n)),
    }
}

fn ref_reads(r: &Ref, n: &str) -> bool {
    r.index.as_ref().is_some_and(|i| expr_reads(i, n))
}

/// Does this statement read the name `n` anywhere, its blocks included?
fn stmt_reads(s: &Stmt, n: &str) -> bool {
    let block = |b: &[Stmt]| b.iter().any(|s| stmt_reads(s, n));
    match s {
        Stmt::Turn | Stmt::Request | Stmt::End => false,
        Stmt::Set(_, e) | Stmt::Observe(_, e) => expr_reads(e, n),
        Stmt::Hold { body, .. } => {
            // a nested hold that binds `n` itself gives its body its own `n`
            header_reads(s, n) || (!binds_in_body(body, n) && block(body))
        }
        Stmt::Grow(r, e) | Stmt::Load(r, e) => ref_reads(r, n) || expr_reads(e, n),
        Stmt::Drop(r) | Stmt::Release(r) => ref_reads(r, n),
        Stmt::Run {
            stage,
            work,
            growing,
            also,
            ..
        } => {
            ref_reads(stage, n)
                || also.iter().any(|r| ref_reads(r, n))
                || expr_reads(work, n)
                || growing.as_ref().is_some_and(|g| ref_reads(g, n))
        }
        Stmt::Branch(c, a, b) => expr_reads(c, n) || block(a) || block(b),
        Stmt::Loop(b) => block(b),
        Stmt::Choose { count, key, .. } => {
            expr_reads(count, n) || key.iter().any(|k| expr_reads(k, n))
        }
        Stmt::Call {
            queue,
            args,
            from,
            to,
            ..
        } => {
            ref_reads(queue, n)
                || args.iter().any(|a| expr_reads(a, n))
                || from.as_ref().is_some_and(|r| ref_reads(r, n))
                || to
                    .as_ref()
                    .is_some_and(|(r, e)| ref_reads(r, n) || expr_reads(e, n))
        }
        Stmt::Mark(_) => false,
    }
}

/// Does the header of this hold (its units, `reserve`, `reuse`, `cache`,
/// `lease`) read `n`?
fn header_reads(s: &Stmt, n: &str) -> bool {
    let Stmt::Hold {
        pools,
        reuse,
        cache,
        lease,
        ..
    } = s
    else {
        return false;
    };
    pools.iter().any(|(r, u, f)| {
        ref_reads(r, n) || expr_reads(u, n) || f.as_ref().is_some_and(|f| expr_reads(f, n))
    }) || reuse.as_ref().is_some_and(|e| expr_reads(e, n))
        || cache.as_ref().is_some_and(|e| expr_reads(e, n))
        || lease
            .as_ref()
            .is_some_and(|(r, t)| ref_reads(r, n) || expr_reads(t, n))
}

/// Does this hold body begin with the `set` of a binding `n`? The parser
/// puts a binding the body reads there, and nothing else sets its name.
fn binds_in_body(body: &[Stmt], n: &str) -> bool {
    body.iter()
        .take_while(|s| matches!(s, Stmt::Set(..)))
        .any(|s| matches!(s, Stmt::Set(v, _) if v == n))
}

/// The first of `bound` that `stmts` read outside the body of a hold that
/// binds it; `scope` are the ones bound around `stmts`.
fn stray_read(stmts: &[Stmt], bound: &[String], scope: &[String]) -> Option<String> {
    let outside = |s: &Stmt, reads: &dyn Fn(&Stmt, &str) -> bool| {
        bound
            .iter()
            .find(|n| !scope.contains(n) && reads(s, n))
            .cloned()
    };
    for s in stmts {
        let found = match s {
            Stmt::Hold { body, .. } => outside(s, &header_reads).or_else(|| {
                let mut inner = scope.to_vec();
                inner.extend(bound.iter().filter(|n| binds_in_body(body, n)).cloned());
                stray_read(body, bound, &inner)
            }),
            Stmt::Branch(c, a, b) => outside(s, &|_, n| expr_reads(c, n))
                .or_else(|| stray_read(a, bound, scope))
                .or_else(|| stray_read(b, bound, scope)),
            Stmt::Loop(b) => stray_read(b, bound, scope),
            // the binding's own `set`, at the top of its hold's body
            Stmt::Set(v, _) if bound.contains(v) && scope.contains(v) => None,
            _ => outside(s, &stmt_reads),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Every expression of the declarations: pools, stages, the arrivals and
/// the run options, which a hold's binding is never in scope of.
fn decl_exprs(prog: &Program) -> Vec<&Expr> {
    let mut out: Vec<&Expr> = vec![];
    for p in &prog.pools {
        out.push(&p.cap);
        out.extend(&p.block);
        if let EvictOrder::By(keys) = &p.evict {
            out.extend(keys);
        }
        if let QueueOrder::By(keys) = &p.queue {
            out.extend(keys);
        }
        if let Some(sp) = &p.spill {
            out.extend([&sp.work, &sp.when]);
            out.extend(
                [&sp.to, &sp.via]
                    .into_iter()
                    .filter_map(|r| r.index.as_deref()),
            );
        }
        out.extend(p.admit_via.iter().filter_map(|r| r.index.as_deref()));
    }
    for st in &prog.stages {
        match &st.kind {
            StageKind::Fifo(e) | StageKind::Ps(e) => out.push(e),
            StageKind::Delay => {}
            StageKind::Step(sp) => {
                out.extend([&sp.budget, &sp.cost, &sp.chunk]);
                out.extend(sp.memory.iter().filter_map(|r| r.index.as_deref()));
                out.extend(&sp.only);
                if let Serve::By(keys) = &sp.serve {
                    out.extend(keys);
                }
            }
        }
    }
    if let Some(w) = &prog.workload {
        match &w.arrive {
            Arrival::Poisson(e) | Arrival::Renewal(e) | Arrival::Closed(e) | Arrival::Batch(e) => {
                out.push(e)
            }
            Arrival::None => {}
        }
    }
    let r = &prog.run;
    out.extend(
        [&r.horizon, &r.warmup, &r.seed, &r.arrivals]
            .into_iter()
            .flatten(),
    );
    out
}

/// The first thing `e` reads whose value at a hold's admission is not its
/// value in the hold's body: an observable of pool or stage state, a
/// context variable, or `cached`, which the admission sets. `attrs` are
/// the program's attributes and `lets` its constants, which shadow a
/// context variable of the same name as the linker resolves them.
fn live_read(e: &Expr, attrs: &[String], lets: &[String]) -> Option<String> {
    let var = |v: &str| {
        let shadowed = attrs.iter().chain(lets).any(|a| a == v);
        (v == "cached" || (!shadowed && is_context_var(v))).then(|| v.to_string())
    };
    match e {
        Expr::Located(_, inner) => live_read(inner, attrs, lets),
        Expr::Num(_) => None,
        Expr::Var(v) => var(v),
        Expr::Sample(_, args) => args.iter().find_map(|a| live_read(a, attrs, lets)),
        Expr::Call(f, args) => {
            if !PURE.contains(&f.as_str()) {
                return Some(format!("{f}(…)"));
            }
            args.iter().find_map(|a| match a {
                Arg::Expr(x) => live_read(x, attrs, lets),
                Arg::Ref(r) if r.index.is_none() => var(&r.name),
                Arg::Ref(_) => None,
            })
        }
        Expr::Unary(_, a) => live_read(a, attrs, lets),
        Expr::Binary(_, a, b) => live_read(a, attrs, lets).or_else(|| live_read(b, attrs, lets)),
        Expr::Cond(c, a, b) => live_read(c, attrs, lets)
            .or_else(|| live_read(a, attrs, lets))
            .or_else(|| live_read(b, attrs, lets)),
        Expr::Over(_, _, n, e) => live_read(n, attrs, lets).or_else(|| live_read(e, attrs, lets)),
    }
}

/// The distinct requests `stmts` makes, at any depth: `request;`, or a
/// gateway's `route` (`request gw;`), each once.
fn request_sites(stmts: &[Stmt], out: &mut Vec<Stmt>) {
    for s in stmts {
        match s {
            Stmt::Call { verb, .. } if verb != "route" => {}
            Stmt::Request | Stmt::Call { .. } => {
                let same = |o: &Stmt| match (o, s) {
                    (Stmt::Request, Stmt::Request) => true,
                    (Stmt::Call { queue: a, .. }, Stmt::Call { queue: b, .. }) => a.same_target(b),
                    _ => false,
                };
                if !out.iter().any(same) {
                    out.push(s.clone());
                }
            }
            Stmt::Hold { body, .. } | Stmt::Loop(body) => request_sites(body, out),
            Stmt::Branch(_, a, b) => {
                request_sites(a, out);
                request_sites(b, out);
            }
            _ => {}
        }
    }
}

/// Replace every `request;` in `stmts`, at any depth, by the server's
/// statements. Returns how many were replaced.
fn splice(stmts: &mut Vec<Stmt>, server: &[Stmt]) -> usize {
    let mut n = 0;
    let mut out = Vec::with_capacity(stmts.len());
    for mut s in std::mem::take(stmts) {
        match &mut s {
            Stmt::Request => {
                n += 1;
                out.extend(server.iter().cloned());
                continue;
            }
            Stmt::Hold { body, .. } | Stmt::Loop(body) => n += splice(body, server),
            Stmt::Branch(_, a, b) => {
                n += splice(a, server);
                n += splice(b, server);
            }
            _ => {}
        }
        out.push(s);
    }
    *stmts = out;
    n
}

/// The names an expression reads: variables and references.
fn names(e: &Expr, vars: &mut Vec<String>, refs: &mut Vec<Ref>) {
    let mut indexed = vec![];
    names_in(e, vars, &mut indexed, refs);
    vars.extend(indexed);
}

/// `names`, with the names read inside a reference's index (`cachedin(P[i].kv)`)
/// apart in `indexed`.
fn names_in(e: &Expr, vars: &mut Vec<String>, indexed: &mut Vec<String>, refs: &mut Vec<Ref>) {
    match e {
        Expr::Located(_, inner) => names_in(inner, vars, indexed, refs),
        Expr::Num(_) => {}
        Expr::Var(n) => vars.push(n.clone()),
        Expr::Sample(_, args) => args.iter().for_each(|a| names_in(a, vars, indexed, refs)),
        Expr::Call(_, args) => args.iter().for_each(|a| match a {
            Arg::Expr(x) => names_in(x, vars, indexed, refs),
            Arg::Ref(r) => {
                if let Some(i) = &r.index {
                    let mut inner = vec![];
                    names_in(i, &mut inner, indexed, refs);
                    indexed.extend(inner);
                }
                refs.push(r.clone());
            }
        }),
        Expr::Unary(_, a) => names_in(a, vars, indexed, refs),
        Expr::Binary(_, a, b) => {
            names_in(a, vars, indexed, refs);
            names_in(b, vars, indexed, refs);
        }
        Expr::Cond(c, a, b) => {
            names_in(c, vars, indexed, refs);
            names_in(a, vars, indexed, refs);
            names_in(b, vars, indexed, refs);
        }
        // the index is the aggregate's own, not a name the expression reads
        Expr::Over(_, j, n, body) => {
            names_in(n, vars, indexed, refs);
            let (mut v, mut i, mut r) = (vec![], vec![], vec![]);
            names_in(body, &mut v, &mut i, &mut r);
            vars.extend(v.into_iter().filter(|x| x != j));
            indexed.extend(i.into_iter().filter(|x| x != j));
            // a bare `j` argument (`max(j, 1)`) parses as a reference
            refs.extend(r.into_iter().filter(|x| x.index.is_some() || x.name != *j));
        }
    }
}

/// The context an expression may read anywhere: the pool, stage and
/// step-iteration variables, and the built-in session attributes.
const CONTEXT: &[&str] = &[
    "waited",
    "size",
    "age",
    "last",
    "waiting",
    "present",
    "tokens",
    "decoders",
    "prefilled",
    "residents",
    "kv_decode",
    "kv_prefill",
    "attention",
    "decoding",
    "admission",
    "remaining",
    // what the scheduler keeps of a request: its cache hit and the tokens
    // it has computed. The session's own (`out`, `think`, `more`, `forced`,
    // `new`, `turn_no`, `serial`) an entry reads only if `hidden`.
    "cached",
    "computed",
];

/// An entry's body: what it `set`s or `choose`s, what it `mark`s, and the
/// pools its holds lease.
fn collect_entry(
    stmts: &[Stmt],
    locals: &mut Vec<String>,
    marks: &mut Vec<String>,
    leased: &mut Vec<String>,
) {
    for s in stmts {
        match s {
            Stmt::Set(n, _) | Stmt::Choose { var: n, .. } => {
                if !locals.contains(n) {
                    locals.push(n.clone());
                }
            }
            Stmt::Mark(n) => {
                if !marks.contains(n) {
                    marks.push(n.clone());
                }
            }
            Stmt::Hold { body, lease, .. } => {
                if let Some((r, _)) = lease
                    && !leased.contains(&r.name)
                {
                    leased.push(r.name.clone());
                }
                collect_entry(body, locals, marks, leased);
            }
            Stmt::Loop(body) => collect_entry(body, locals, marks, leased),
            Stmt::Branch(_, a, b) => {
                collect_entry(a, locals, marks, leased);
                collect_entry(b, locals, marks, leased);
            }
            _ => {}
        }
    }
}

/// The expressions of an entry's body, the holds' headers apart from the
/// rest: the header is the admission and sees less. The index of every
/// reference the body names (`run nic[k]`, `hold kv`, `release src`) is in
/// `indices`: read as the body reads, and the only place a `from` name may
/// stand as a number.
fn split_reads<'a>(
    stmts: &'a [Stmt],
    headers: &mut Vec<&'a Expr>,
    bodies: &mut Vec<&'a Expr>,
    indices: &mut Vec<&'a Expr>,
) {
    let index = |r: &'a Ref, indices: &mut Vec<&'a Expr>| {
        if let Some(i) = &r.index {
            indices.push(i);
        }
    };
    for s in stmts {
        match s {
            Stmt::Turn | Stmt::End | Stmt::Request | Stmt::Mark(_) => {}
            Stmt::Set(_, e) | Stmt::Observe(_, e) => bodies.push(e),
            Stmt::Grow(r, e) | Stmt::Load(r, e) => {
                index(r, indices);
                bodies.push(e);
            }
            Stmt::Drop(r) | Stmt::Release(r) => index(r, indices),
            Stmt::Hold {
                pools,
                reuse,
                body,
                cache,
                lease,
            } => {
                for (r, e, f) in pools {
                    index(r, indices);
                    headers.push(e);
                    if let Some(f) = f {
                        headers.push(f);
                    }
                }
                if let Some(r) = reuse {
                    headers.push(r);
                }
                if let Some(c) = cache {
                    bodies.push(c);
                }
                if let Some((r, t)) = lease {
                    index(r, indices);
                    bodies.push(t);
                }
                split_reads(body, headers, bodies, indices);
            }
            Stmt::Run {
                stage,
                work,
                growing,
                also,
                ..
            } => {
                index(stage, indices);
                also.iter().for_each(|r| index(r, indices));
                if let Some(g) = growing {
                    index(g, indices);
                }
                bodies.push(work);
            }
            Stmt::Branch(p, a, b) => {
                bodies.push(p);
                split_reads(a, headers, bodies, indices);
                split_reads(b, headers, bodies, indices);
            }
            Stmt::Loop(b) => split_reads(b, headers, bodies, indices),
            Stmt::Choose { count, key, .. } => {
                bodies.push(count);
                bodies.extend(key);
            }
            Stmt::Call {
                queue,
                args,
                from,
                to,
                ..
            } => {
                index(queue, indices);
                bodies.extend(args.iter());
                if let Some(r) = from {
                    index(r, indices);
                }
                if let Some((r, m)) = to {
                    index(r, indices);
                    bodies.push(m);
                }
            }
        }
    }
}

/// Whether every way through `stmts` leaves a lease: a `from` takes the KV
/// the entry leased, and a way that leases nothing would leave none to take.
fn always_leases(stmts: &[Stmt]) -> bool {
    for s in stmts {
        match s {
            Stmt::Hold { lease: Some(_), .. } => return true,
            Stmt::Hold { body, .. } if always_leases(body) => return true,
            Stmt::Branch(_, a, b) if always_leases(a) && always_leases(b) => return true,
            Stmt::End => return false,
            _ => {}
        }
    }
    false
}

/// The pools an entry's statements name: its holds', leases', and those of
/// `grow`, `drop`, `release`, `load`, `growing` and a call's `to`.
fn pools_named<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Ref>) {
    for s in stmts {
        match s {
            Stmt::Grow(r, _) | Stmt::Load(r, _) | Stmt::Drop(r) | Stmt::Release(r) => out.push(r),
            Stmt::Hold {
                pools, body, lease, ..
            } => {
                out.extend(pools.iter().map(|(r, _, _)| r));
                if let Some((r, _)) = lease {
                    out.push(r);
                }
                pools_named(body, out);
            }
            Stmt::Run {
                growing: Some(g), ..
            } => out.push(g),
            Stmt::Branch(_, a, b) => {
                pools_named(a, out);
                pools_named(b, out);
            }
            Stmt::Loop(b) => pools_named(b, out),
            Stmt::Call {
                to: Some((r, _)), ..
            } => out.push(r),
            _ => {}
        }
    }
}

impl Parser {
    fn new(toks: Vec<Token>) -> Parser {
        Parser {
            toks,
            pos: 0,
            stages: vec![],
            definitions: vec![],
            side: Side::Session,
            wl_session: None,
            server: None,
            share_at: None,
            body_binds: vec![],
            bind_at: vec![],
            defs: vec![],
            expanded: vec![],
            deferred: vec![],
            base: None,
            libs: vec![],
            lib_dirs: vec![],
            lib_shown_dirs: vec![],
            read: vec![],
            def_overrides: vec![],
            queues: vec![],
            consts: vec![],
            structural_overrides: vec![],
            in_queue: None,
            entry_from: None,
            relation_share: false,
            latency_ok: false,
            dotted_reads: vec![],
            indexed_dotted: vec![],
            requests: vec![],
        }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, k: usize) -> &Tok {
        let i = (self.pos + k).min(self.toks.len() - 1);
        &self.toks[i].tok
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        self.err_at(self.pos, msg)
    }

    /// The library of file `file`, for an error in it.
    fn origin(&self, file: usize) -> Option<Source> {
        file.checked_sub(1).and_then(|i| self.libs.get(i)).cloned()
    }

    /// `path:` for a place in a library, nothing in the program.
    fn place(&self, file: usize, line: usize, col: usize) -> String {
        match self.origin(file) {
            Some(lib) => format!("{}:{line}:{col}", lib.path),
            None => format!("{line}:{col}"),
        }
    }

    fn err_at<T>(&self, pos: usize, msg: impl Into<String>) -> PResult<T> {
        let t = &self.toks[pos];
        let mut msg = msg.into();
        for e in self.expanded.iter().rev() {
            if e.start <= pos && pos < e.end {
                msg.push_str(&format!(
                    "\nnote: in `{}`, used at {}",
                    e.name,
                    self.place(e.file, e.line, e.col)
                ));
            }
        }
        Err(ParseError {
            line: t.line,
            col: t.col,
            msg,
            origin: self.origin(t.file),
        })
    }

    fn advance(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, t: &Tok) -> PResult<()> {
        if self.peek() == t {
            self.advance();
            Ok(())
        } else {
            self.err(format!("expected {t}, found {}", self.peek()))
        }
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> PResult<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            self.err(format!("expected `{kw}`, found {}", self.peek()))
        }
    }

    fn span(&self) -> Span {
        let t = &self.toks[self.pos];
        Span {
            line: t.line,
            col: t.col,
            len: t.text.chars().count().max(1),
            file: t.file,
        }
    }

    fn definition(&mut self) -> PResult<String> {
        let span = self.span();
        let name = self.name()?;
        self.definitions.push((name.clone(), span));
        Ok(name)
    }

    fn bare_reference(&mut self) -> PResult<Ref> {
        let span = Some(self.span());
        Ok(Ref {
            span,
            name: self.ident()?,
            index: None,
        })
    }

    fn ident(&mut self) -> PResult<String> {
        let at = self.pos;
        match self.advance() {
            Tok::Ident(s) => Ok(s),
            other => {
                self.pos = at;
                self.err(format!("expected identifier, found {other}"))
            }
        }
    }

    /// An identifier a program names something with: not a word #136
    /// retired, so that one name never means the old statement in one place
    /// and the program's thing in another.
    fn name(&mut self) -> PResult<String> {
        if let Tok::Ident(w) = self.peek()
            && let Some(now) = retired(w)
        {
            return self.err(format!("`{w}` is a retired word and names nothing: {now}"));
        }
        self.ident()
    }

    fn string(&mut self) -> PResult<String> {
        let at = self.pos;
        match self.advance() {
            Tok::Str(s) => Ok(s),
            other => {
                self.pos = at;
                self.err(format!("expected string, found {other}"))
            }
        }
    }

    fn program(&mut self) -> PResult<Program> {
        let mut prog = Program::default();
        while *self.peek() != Tok::Eof {
            if self.toks[self.pos].file != 0 && !self.is_kw("def") && !self.is_kw("use") {
                return self.err(format!(
                    "a library holds definitions: found {} where `def` or `use` goes",
                    self.peek()
                ));
            }
            let (item, item_file) = (self.pos, self.toks[self.pos].file);
            if self.is_kw("use") {
                self.use_library()?;
                continue;
            }
            if self.eat_kw("let") {
                let name = self.definition()?;
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                let mut vars = vec![];
                names(&e, &mut vars, &mut Vec::new());
                if vars.iter().any(|v| self.structural_overrides.contains(v))
                    && !self.structural_overrides.contains(&name)
                {
                    self.structural_overrides.push(name.clone());
                }
                if let Some(v) = self.const_value(&e) {
                    self.consts.push((name.clone(), v));
                }
                prog.lets.push((name, e));
            } else if self.eat_kw("def") {
                self.def()?;
            } else if self.eat_kw("gauge") {
                let name = self.ident()?;
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                prog.gauges.push((name, e));
            } else if self.eat_kw("pool") {
                prog.pools.push(self.pool()?);
            } else if self.is_kw("queue") {
                let at = self.pos;
                self.advance();
                self.queue(&mut prog, at)?;
            } else if self.eat_kw("stage") {
                let d = self.stage()?;
                self.stages
                    .push((d.name.clone(), matches!(d.kind, StageKind::Step(_))));
                prog.stages.push(d);
            } else if self.eat_kw("workload") {
                if prog.workload.is_some() {
                    return self.err("duplicate workload");
                }
                prog.workload = Some(self.workload()?);
            } else if self.eat_kw("session") {
                if !prog.session.is_empty() {
                    return self.err("duplicate session");
                }
                self.side = Side::Session;
                prog.session = self.block()?;
            } else if self.is_kw("server") {
                let at = self.pos;
                self.advance();
                if self.server.is_some() {
                    return self.err_at(at, "duplicate server");
                }
                self.side = Side::Server;
                let body = self.block()?;
                self.side = Side::Session;
                self.server = Some((at, body));
            } else if matches!(self.peek(), Tok::Ident(_))
                && *self.peek_at(1) == Tok::Ident("pull".into())
            {
                self.pull_relation(&mut prog)?;
            } else if self.is_kw("share") {
                let at = self.pos;
                self.advance();
                if let Some(first) = self.share_at {
                    let line = self.toks[first].line;
                    let what = if self.relation_share {
                        "the pull relation on line"
                    } else {
                        "the first is on line"
                    };
                    return self.err_at(at, format!("`share` is given twice: {what} {line}"));
                }
                self.share_at = Some(at);
                prog.share = Some(if self.eat_kw("maxmin") {
                    crate::ir::Share::MaxMin
                } else if self.eat_kw("bottleneck") {
                    crate::ir::Share::Bottleneck
                } else {
                    return self.err(format!(
                        "`share` takes `maxmin` or `bottleneck`, found {}",
                        self.peek()
                    ));
                });
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("run") {
                self.expect(&Tok::LBrace)?;
                while *self.peek() != Tok::RBrace {
                    let key = self.ident()?;
                    let e = self.expr()?;
                    self.expect(&Tok::Semi)?;
                    match key.as_str() {
                        "horizon" => prog.run.horizon = Some(e),
                        "warmup" => prog.run.warmup = Some(e),
                        "seed" => prog.run.seed = Some(e),
                        "arrivals" => prog.run.arrivals = Some(e),
                        other => return self.err(format!("unknown run option `{other}`")),
                    }
                }
                self.expect(&Tok::RBrace)?;
            } else {
                return self.err(format!("unexpected {} at top level", self.peek()));
            }
            // a library's definition ends in the library: the program does not
            // finish it, nor it the program's
            let last = self.toks[self.pos.saturating_sub(1)].file;
            if last != item_file {
                return self.err_at(
                    item,
                    "this item does not end in the file it starts in: a library's \
                     definitions are whole",
                );
            }
        }
        // a request runs the server or a named gateway's `route`, whose
        // admissions set `cached` and `computed`, and the entries it calls
        // set and mark (`D.first_token`)
        let mut shared: Vec<String> = vec!["cached".into(), "computed".into()];
        let mut served = Served::default();
        if let Some((_, server)) = &self.server {
            assigned_in(server, &mut served.server);
        }
        for q in &self.queues {
            for e in &q.entries {
                if e.verb == "route" {
                    let mut names = vec![];
                    assigned_in(&e.body, &mut names);
                    served.gateways.push((q.name.clone(), names));
                } else {
                    shared.extend(e.locals.iter().map(|l| format!("{}.{l}", q.name)));
                }
            }
            shared.extend(q.marks.iter().map(|m| format!("{}.{m}", q.name)));
        }
        served.shared = shared;
        self.assemble(&mut prog)?;
        self.check_body_bindings(&prog)?;
        self.check_def_names(&prog)?;
        self.check_deferred(&prog, &served)?;
        prog.definitions = std::mem::take(&mut self.definitions);
        prog.libs = self.libs.clone();
        Ok(prog)
    }

    /// Resolve named gateway requests, then expand entries and splice the
    /// anonymous server at bare `request;` sites. The workload's session
    /// becomes the program's session; a second top-level session is an error.
    fn assemble(&mut self, prog: &mut Program) -> PResult<()> {
        for (at, target) in &self.requests {
            match target {
                Some(name) => {
                    let Some(q) = self.queues.iter().find(|q| q.name == *name) else {
                        return self.err_at(
                            *at,
                            format!("`request {name}`: no queue `{name}` is declared"),
                        );
                    };
                    if !q.roles.iter().any(|role| role == "gateway") {
                        return self.err_at(
                            *at,
                            format!("`request {name}`: queue `{name}` does not play `gateway`"),
                        );
                    }
                }
                None if self.server.is_none() => {
                    return self.err_at(
                        *at,
                        "`request;` needs a `server` block; name a gateway with `request NAME;`",
                    );
                }
                None => {}
            }
        }
        if self.wl_session.is_some() && !prog.session.is_empty() {
            return self
                .err("a program has one session: inside `workload` or at top level, not both");
        }
        // what one request runs, before either side is expanded: the one
        // thing every `request` of the workload's session names
        let mut request = vec![];
        if let Some((_, s)) = &self.wl_session {
            request_sites(s, &mut request);
            if request.len() > 1 {
                request.clear();
            }
        }
        // Queues first: every entry call in place, so the sides are plain
        // statements when they are put together.
        if !self.queues.is_empty() {
            let hidden: Vec<String> = prog
                .workload
                .as_ref()
                .map(|w| w.hidden.clone())
                .unwrap_or_default();
            for q in &self.queues {
                for e in &q.entries {
                    if let Some(n) = e.reads.iter().find(|n| !hidden.contains(n)) {
                        return self.err_at(
                            e.at,
                            format!(
                                "`{}.{}` reads `{n}`, a session attribute set outside the queue: an \
                                 entry sees its parameters, the queue's pools, the context and the \
                                 request's `hidden` attributes; pass `{n}` as a parameter or `mark` \
                                 a moment for the caller to read",
                                q.name, e.verb
                            ),
                        );
                    }
                }
            }
            for (at, name) in std::mem::take(&mut self.indexed_dotted) {
                let (qn, field) = name.split_once('.').expect("a dotted name");
                if let Some(q) = self.queues.iter().find(|q| q.name == qn)
                    && !q.pools.iter().any(|p| p == field)
                {
                    return self.err_at(
                        at,
                        format!(
                            "`{qn}[…].{field}` reads the request's attribute, which is no member's: \
                             write `{qn}.{field}`"
                        ),
                    );
                }
            }
            for (at, name) in std::mem::take(&mut self.dotted_reads) {
                let (qn, field) = name.split_once('.').expect("a dotted name");
                let ok = self.queues.iter().any(|q| {
                    q.name == qn
                        && (q.marks.iter().any(|m| m == field)
                            || q.pools.iter().any(|p| p == field)
                            || q.entries
                                .iter()
                                .any(|e| e.locals.iter().any(|l| l == field)))
                });
                if !ok {
                    return self.err_at(
                        at,
                        format!("`{name}`: no entry of `{qn}` marks or sets `{field}`, and `{qn}` has no pool `{field}`"),
                    );
                }
            }
            let queues = std::mem::take(&mut self.queues);
            let libs = self.libs.clone();
            let expand = |stmts: &mut Vec<Stmt>| -> PResult<()> {
                queue::expand(stmts, &queues).map_err(|e| ParseError {
                    line: e.at.line,
                    col: e.at.col,
                    msg: e.msg,
                    origin: e.at.file.checked_sub(1).and_then(|i| libs.get(i)).cloned(),
                })
            };
            if let Some((_, s)) = &mut self.wl_session {
                expand(s)?;
            }
            if let Some((_, s)) = &mut self.server {
                expand(s)?;
            }
            expand(&mut prog.session)?;
            expand(&mut request)?;
        }
        match (self.wl_session.take(), self.server.take()) {
            (None, None) => Ok(()),
            (Some((_, session)), None)
                if self.requests.iter().any(|(_, target)| target.is_some()) =>
            {
                prog.session = session;
                prog.request = request;
                Ok(())
            }
            (Some((at, _)), None) => self.err_at(
                at,
                "a `session` inside `workload` is written against a `server` block; \
                 name a gateway with `request NAME;`, or write `session` at top level",
            ),
            (None, Some((at, _))) => self.err_at(
                at,
                "`server` needs a `session` inside `workload` that says `request;`",
            ),
            (Some((_, mut session)), Some((v_at, server))) => {
                if splice(&mut session, &server) == 0 {
                    return self.err_at(
                        v_at,
                        "`server` is never requested: the workload's session has no `request;`",
                    );
                }
                splice(&mut request, &server);
                prog.session = session;
                prog.request = request;
                Ok(())
            }
        }
    }

    /// A binding its hold's body reads was set at the top of the body. That
    /// is its admission value only if it reads attributes and constants, and
    /// the `set` is the binding's alone only if nothing else has its name.
    fn check_body_bindings(&self, prog: &Program) -> PResult<()> {
        if self.body_binds.is_empty() {
            return Ok(());
        }
        let lets: Vec<String> = prog.lets.iter().map(|(n, _)| n.clone()).collect();
        // what the program's own `set` and `choose` assign (the bindings'
        // `set`s are not recorded as definitions)
        let assigned: Vec<String> = self
            .definitions
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| !lets.contains(n))
            .collect();
        for (at, name, e) in &self.body_binds {
            let clash = if BUILTIN_ATTRS.contains(&name.as_str()) {
                Some("an attribute the scheduler sets")
            } else if is_context_var(name) || name == "inf" {
                Some("a name the language supplies")
            } else if prog.pools.iter().any(|d| d.name == *name) {
                Some("a pool")
            } else if prog.stages.iter().any(|d| d.name == *name) {
                Some("a stage")
            } else if lets.contains(name) {
                Some("a `let` constant")
            } else if assigned.contains(name) {
                Some("an attribute the program sets")
            } else {
                None
            };
            if let Some(what) = clash {
                return self.err_at(
                    *at,
                    format!(
                        "the body reads `{name}`, which is also {what}: the body would not \
                         say which it means\nhelp: rename the binding"
                    ),
                );
            }
            if expr_reads(e, name) {
                return self.err_at(*at, format!("the binding `{name}` reads itself"));
            }
            if let Some(what) = live_read(e, &assigned, &lets) {
                let help = if what == "cached" || what.starts_with("cachedin") {
                    "in the body read `cached`, the units the admission consumed"
                } else {
                    "name what the body needs with a `set` in the body"
                };
                return self.err_at(
                    *at,
                    format!(
                        "the body reads `{name}`, and `{name}` reads `{what}`: the body \
                         sees a binding that reads only attributes and constants\n\
                         help: {help}"
                    ),
                );
            }
        }
        // the binding is the body's: a read anywhere else would get the value
        // the last such hold left
        let mut bound: Vec<String> = self.body_binds.iter().map(|(_, n, _)| n.clone()).collect();
        bound.sort();
        bound.dedup();
        let mut stray = stray_read(&prog.session, &bound, &[]);
        if let Some(w) = &prog.workload {
            stray = stray
                .or_else(|| stray_read(&w.init, &bound, &[]))
                .or_else(|| stray_read(&w.turn, &bound, &[]));
        }
        stray = stray.or_else(|| {
            decl_exprs(prog)
                .into_iter()
                .find_map(|e| bound.iter().find(|n| expr_reads(e, n)).cloned())
        });
        if let Some(name) = stray {
            let (at, _, _) = self.body_binds.iter().find(|(_, n, _)| *n == name).unwrap();
            return self.err_at(
                *at,
                format!(
                    "`{name}` is read outside the body of the hold that binds it\n\
                     help: a header's binding is the header's and its body's; \
                     name a value the rest of the program reads with `set`"
                ),
            );
        }
        Ok(())
    }

    /// A use whose body says `turn;` or `request;` may not pass an argument
    /// that reads what the workload's `turn` or the server assigns.
    fn check_deferred(&self, prog: &Program, served: &Served) -> PResult<()> {
        // a turn draws the workload's `turn` block, or a trace's attributes
        let mut turned: Vec<String> = BUILTIN_ATTRS.iter().map(|a| a.to_string()).collect();
        if let Some(w) = &prog.workload {
            assigned_in(&w.turn, &mut turned);
        }
        let lets: Vec<&str> = prog.lets.iter().map(|(n, _)| n.as_str()).collect();
        let attrs: Vec<&str> = self
            .definitions
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !lets.contains(n))
            .collect();
        for u in &self.deferred {
            if let Some(v) = u.reads.iter().find(|v| {
                is_context_var(v) && !attrs.contains(&v.as_str()) && !lets.contains(&v.as_str())
            }) {
                return Err(ParseError {
                    line: u.line,
                    col: u.col,
                    msg: live_message(&u.name, "", v),
                    origin: self.origin(u.file),
                });
            }
            let found = u.reads.iter().find_map(|n| {
                if u.turn && turned.contains(n) {
                    Some((n, "`turn;`".to_string()))
                } else {
                    let s = u.request.iter().find(|s| served.assigns(s, n))?;
                    Some((n, s.says()))
                }
            });
            if let Some((n, by)) = found {
                return Err(ParseError {
                    line: u.line,
                    col: u.col,
                    msg: format!(
                        "an argument of `{}` reads `{n}`, which its {by} assigns: it would \
                         read the new `{n}`, not this one\nhelp: `set` the value under \
                         another name first and pass that",
                        u.name
                    ),
                    origin: self.origin(u.file),
                });
            }
        }
        Ok(())
    }

    /// `use "path";`: the library's definitions, read from `path` next to
    /// the file the `use` is in, in place of the `use`. A library read once
    /// is not read again.
    fn use_library(&mut self) -> PResult<()> {
        let at = self.pos;
        self.advance();
        let path = self.string()?;
        self.expect(&Tok::Semi)?;
        let file = self.toks[at].file;
        let dir = match file.checked_sub(1) {
            Some(i) => Some(self.lib_dirs[i].clone()),
            None => self.base.clone(),
        };
        let Some(dir) = dir else {
            return self.err_at(
                at,
                format!(
                    "`use \"{path}\"` reads a file next to the program, and this program was \
                     given as text"
                ),
            );
        };
        let full = match file.checked_sub(1) {
            Some(i) => self.lib_shown_dirs[i].join(&path),
            None => dir.join(&path),
        };
        let canonical = full
            .canonicalize()
            .or_else(|e| self.err_at(at, format!("cannot read `{path}`: {e}")))?;
        let mut toks = vec![];
        if !self.read.contains(&canonical) {
            let text = std::fs::read_to_string(&canonical)
                .or_else(|e| self.err_at(at, format!("cannot read `{path}`: {e}")))?;
            let shown = full.display().to_string();
            self.lib_shown_dirs
                .push(full.parent().map(Path::to_path_buf).unwrap_or_default());
            let lib = Source { path: shown, text };
            toks = lex(&lib.text).map_err(|e| ParseError {
                line: e.line,
                col: e.col,
                msg: e.msg,
                origin: Some(lib.clone()),
            })?;
            toks.pop(); // its end of file
            let id = self.libs.len() + 1;
            for t in &mut toks {
                t.file = id;
            }
            self.libs.push(lib);
            self.lib_dirs.push(
                canonical
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
            );
            self.read.push(canonical);
        }
        if self.toks.len() + toks.len() > MAX_TOKENS {
            return self.err_at(at, "the libraries are more than a program can hold");
        }
        self.toks.splice(at..self.pos, toks);
        self.pos = at;
        Ok(())
    }

    /// A `def` named like a declaration would be one name for two things.
    fn check_def_names(&self, prog: &Program) -> PResult<()> {
        for d in &self.defs {
            let what = if prog.pools.iter().any(|x| x.name == d.name) {
                "a pool"
            } else if prog.stages.iter().any(|x| x.name == d.name) {
                "a stage"
            } else if prog.lets.iter().any(|(n, _)| *n == d.name) {
                "a `let` constant"
            } else {
                continue;
            };
            return Err(ParseError {
                line: d.line,
                col: d.col,
                msg: format!("`{}` is defined, and is also {what}", d.name),
                origin: self.origin(d.file),
            });
        }
        Ok(())
    }

    /// `def name(x, …) = e;` or `def name(x, …) { … }`, after `def`.
    fn def(&mut self) -> PResult<()> {
        let at = self.pos;
        let name = self.ident()?;
        if KEYWORDS.contains(&name.as_str())
            || is_function(&name)
            || DISTRIBUTIONS.contains(&name.as_str())
        {
            return self.err_at(at, format!("`{name}` is a word of the language"));
        }
        if let Some(now) = retired(&name) {
            return self.err_at(
                at,
                format!("`{name}` is a retired word and names nothing: {now}"),
            );
        }
        if self.defs.iter().any(|d| d.name == name) {
            return self.err_at(at, format!("`{name}` is defined twice"));
        }
        // a definition uses only the ones before it, so none can reach
        // itself: an earlier one that uses this name used something undefined
        if let Some(d) = self.defs.iter().find(|d| uses(&d.body, &name)) {
            return self.err_at(
                at,
                format!(
                    "`{}` uses `{name}`, which is defined after it: a definition uses the \
                     ones before it",
                    d.name
                ),
            );
        }
        self.expect(&Tok::LParen)?;
        let mut params: Vec<String> = vec![];
        while *self.peek() != Tok::RParen {
            let p_at = self.pos;
            let p = self.name()?;
            if KEYWORDS.contains(&p.as_str()) || is_function(&p) {
                return self.err_at(
                    p_at,
                    format!("`{p}` is a word of the language: name the parameter otherwise"),
                );
            }
            if params.contains(&p) {
                return self.err_at(p_at, format!("`{p}` is a parameter twice"));
            }
            params.push(p);
            if *self.peek() == Tok::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        let stmts = match self.peek() {
            Tok::Assign => false,
            Tok::LBrace => true,
            other => {
                return self.err(format!(
                    "expected `= expression;` or `{{ statements }}` after `def {name}(…)`, found {other}"
                ));
            }
        };
        self.advance();
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Tok::Eof => return self.err_at(at, format!("`def {name}` is not closed")),
                Tok::RBrace if depth == 0 && stmts => break,
                Tok::Semi if depth == 0 && !stmts => break,
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    if depth == 0 {
                        return self.err(format!("unmatched {}", self.peek()));
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.advance();
        }
        let mut body = self.toks[start..self.pos].to_vec();
        self.advance();
        if let Some(i) = self.def_overrides.iter().position(|d| d.0 == name) {
            if stmts {
                return self.err_at(
                    at,
                    format!(
                        "--def {name}: `def {name}` is statements; --def replaces the body of an \
                         expression definition"
                    ),
                );
            }
            self.def_overrides[i].2 = true;
            let text = self.def_overrides[i].1.clone();
            let place = &self.toks[at];
            let (line, col, file) = (place.line, place.col, place.file);
            body = lex(&text)
                .map_err(|e| ParseError {
                    line,
                    col,
                    msg: format!("--def {name}: {}", e.msg),
                    origin: None,
                })?
                .into_iter()
                .filter(|t| t.tok != Tok::Eof)
                .map(|t| Token {
                    line,
                    col,
                    file,
                    ..t
                })
                .collect();
        }
        if body.is_empty() && !stmts {
            return self.err_at(at, format!("`def {name}` has no expression"));
        }
        if uses(&body, &name) {
            return self.err_at(at, format!("`{name}` uses itself"));
        }
        // a parameter where the body names what it assigns would put an
        // argument there, which is not a name
        for (k, w) in body.windows(2).enumerate() {
            let named = match (&w[0].tok, &w[1].tok) {
                (Tok::Ident(kw), Tok::Ident(n)) if kw == "choose" => Some(n),
                (Tok::Ident(n), Tok::Assign) => Some(n),
                // `sum k in …`: the aggregate binds `k`
                (Tok::Ident(kw), Tok::Ident(n))
                    if Agg::from_name(kw).is_some()
                        && matches!(body.get(k + 2).map(|t| &t.tok), Some(Tok::Ident(i)) if i == "in") =>
                {
                    Some(n)
                }
                _ => None,
            };
            if let Some(n) = named
                && params.contains(n)
            {
                return self.err_at(
                    at,
                    format!(
                        "`{n}` is a parameter, and the body names with it what it assigns \
                         or binds"
                    ),
                );
            }
        }
        let draws = body.iter().any(|t| t.tok == Tok::Tilde) || self.draws_through(&body);
        let used: Vec<&Def> = self.defs.iter().filter(|d| uses(&body, &d.name)).collect();
        let mut assigns = assigned_tokens(&body);
        let mut turn = says(&body, "turn");
        let mut request = sends_of(&body, &params);
        let (mut reads, mut calls) = self.reads_of(&body, !stmts);
        reads.retain(|n| !params.contains(n));
        for d in &used {
            assigns.extend(d.assigns.iter().cloned());
            turn |= d.turn;
            // a parameter of `d` is whatever this body passes it
            for args in uses_args(&body, &d.name) {
                request.extend(d.request.iter().map(|s| s.at(&args, &params)));
            }
        }
        assigns.sort();
        assigns.dedup();
        request.dedup();
        calls.sort();
        calls.dedup();
        self.defs.push(Def {
            line: self.toks[at].line,
            col: self.toks[at].col,
            file: self.toks[at].file,
            name,
            params,
            stmts,
            body,
            draws,
            assigns,
            reads,
            calls,
            turn,
            request,
        });
        Ok(())
    }

    /// The names `toks` read and the functions of live state they call,
    /// joined over the definitions they use.
    /// `expr`: the tokens are an expression (an argument, an expression
    /// definition's body), where a word that is also a keyword (`cap`,
    /// `latency`) can only be a name read; in statements it may be the
    /// keyword.
    fn reads_of(&self, toks: &[Token], expr: bool) -> (Vec<String>, Vec<String>) {
        let mut reads = vec![];
        let mut calls = vec![];
        // `max j in n (e)`: the word, `j`, `in` and the `j`s of `(e)` are the
        // aggregate's own, not names read
        let mut own = vec![false; toks.len()];
        for k in 0..toks.len() {
            let ident = |i: usize| match toks.get(i).map(|t| &t.tok) {
                Some(Tok::Ident(n)) => Some(n.as_str()),
                _ => None,
            };
            let (Some(w), Some(j), Some("in")) = (ident(k), ident(k + 1), ident(k + 2)) else {
                continue;
            };
            if Agg::from_name(w).is_none() {
                continue;
            }
            own[k] = true;
            own[k + 1] = true;
            own[k + 2] = true;
            // the count (a token, or a parenthesised expression), then the
            // parenthesised body
            let mut i = k + 3;
            if toks.get(i).map(|t| &t.tok) == Some(&Tok::LParen) {
                let mut depth = 0;
                while let Some(t) = toks.get(i) {
                    match t.tok {
                        Tok::LParen => depth += 1,
                        Tok::RParen => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            } else {
                i += 1;
            }
            if toks.get(i).map(|t| &t.tok) != Some(&Tok::LParen) {
                continue;
            }
            let mut depth = 0;
            while let Some(t) = toks.get(i) {
                match &t.tok {
                    Tok::LParen => depth += 1,
                    Tok::RParen => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Tok::Ident(n) if n == j => own[i] = true,
                    _ => {}
                }
                i += 1;
            }
        }
        for (k, t) in toks.iter().enumerate() {
            let Tok::Ident(n) = &t.tok else { continue };
            if own[k] {
                continue;
            }
            let called = toks.get(k + 1).is_some_and(|t| t.tok == Tok::LParen);
            if called && FUNCTIONS.contains(&n.as_str()) {
                if !PURE.contains(&n.as_str()) {
                    calls.push(n.clone());
                }
            } else if !called && (expr || !KEYWORDS.contains(&n.as_str())) {
                reads.push(n.clone());
            }
        }
        for d in self.defs.iter().filter(|d| uses(toks, &d.name)) {
            reads.extend(d.reads.iter().cloned());
            calls.extend(d.calls.iter().cloned());
        }
        reads.sort();
        reads.dedup();
        (reads, calls)
    }

    /// Does one of the definitions `toks` uses draw?
    fn draws_through(&self, toks: &[Token]) -> bool {
        self.defs.iter().any(|d| d.draws && uses(toks, &d.name))
    }

    /// The `def` the next tokens use, `name(`.
    fn use_of_def(&self) -> Option<usize> {
        let Tok::Ident(n) = self.peek() else {
            return None;
        };
        if *self.peek_at(1) != Tok::LParen {
            return None;
        }
        self.defs.iter().position(|d| d.name == *n)
    }

    /// Replace the use of `defs[i]` at `pos` (`name(a, …)`, and its `;` for
    /// statements) by the body, each parameter by its argument: as written
    /// when it is a reference (`kv`, `kvD[j]`), which may stand for a pool or
    /// a stage, and in parentheses otherwise, so that `f(a + b)` is not
    /// `a + b * 2` inside.
    fn expand(&mut self, i: usize) -> PResult<()> {
        let at = self.pos;
        let (use_line, use_col, use_file) =
            (self.toks[at].line, self.toks[at].col, self.toks[at].file);
        self.advance();
        self.advance();
        let mut args: Vec<Vec<Token>> = vec![];
        let mut cur = vec![];
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Tok::Eof => return self.err_at(at, "the use of a definition is not closed"),
                Tok::RParen if depth == 0 => break,
                Tok::Comma if depth == 0 => {
                    args.push(std::mem::take(&mut cur));
                    self.advance();
                    continue;
                }
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    if depth == 0 {
                        return self.err(format!("unmatched {}", self.peek()));
                    }
                    depth -= 1;
                }
                _ => {}
            }
            cur.push(self.toks[self.pos].clone());
            self.advance();
        }
        if !cur.is_empty() || !args.is_empty() {
            args.push(cur);
        }
        self.advance();
        let d = self.defs[i].clone();
        if args.len() != d.params.len() {
            return self.err_at(
                at,
                format!(
                    "`{}` takes {} argument(s), got {}",
                    d.name,
                    d.params.len(),
                    args.len()
                ),
            );
        }
        // the names the body assigns: an argument that reads one would read
        // the body's value, not the one at the use
        for (p, a) in d.params.iter().zip(&args) {
            if a.is_empty() {
                return self.err_at(at, format!("`{}`: the argument for `{p}` is empty", d.name));
            }
            let (reads, _) = self.reads_of(a, true);
            if let Some(n) = d.assigns.iter().find(|n| reads.contains(n)) {
                return self.err_at(at, capture_message(&d.name, p, n));
            }
            let uses = d
                .body
                .iter()
                .filter(|t| t.tok == Tok::Ident(p.clone()))
                .count();
            if uses > 1 && (a.iter().any(|t| t.tok == Tok::Tilde) || self.draws_through(a)) {
                return self.err_at(
                    at,
                    format!(
                        "the argument for `{p}` draws, and `{}` reads `{p}` {uses} times: \
                         it would draw {uses} times\nhelp: `set` the draw first and pass the name",
                        d.name
                    ),
                );
            }
        }
        if d.stmts {
            for (p, a) in d.params.iter().zip(&args) {
                let (_, calls) = self.reads_of(a, true);
                if let Some(f) = calls.first() {
                    return self.err_at(at, live_message(&d.name, p, &format!("{f}(…)")));
                }
            }
        }
        let turn = d.turn;
        // a parameter's gateway is the argument's name
        let request: Vec<Sends> = d.request.iter().map(|s| s.at(&args, &[])).collect();
        if d.stmts {
            let mut reads: Vec<String> =
                args.iter().flat_map(|a| self.reads_of(a, true).0).collect();
            reads.sort();
            reads.dedup();
            self.deferred.push(Deferred {
                name: d.name.clone(),
                line: use_line,
                col: use_col,
                file: use_file,
                reads,
                turn,
                request,
            });
        }
        let mut end = self.pos;
        if d.stmts {
            if *self.peek() != Tok::Semi {
                return self.err(format!(
                    "expected `;` after `{}(…)`, found {}",
                    d.name,
                    self.peek()
                ));
            }
            end += 1;
        }
        let paren = |tok: Tok, text: &str, like: &Token| Token {
            tok,
            line: like.line,
            col: like.col,
            file: like.file,
            text: text.into(),
            leading: vec![],
        };
        let mut out = vec![];
        if !d.stmts {
            out.push(paren(Tok::LParen, "(", &self.toks[at]));
        }
        for t in &d.body {
            let Tok::Ident(n) = &t.tok else {
                out.push(t.clone());
                continue;
            };
            let Some(k) = d.params.iter().position(|p| p == n) else {
                out.push(t.clone());
                continue;
            };
            let a = &args[k];
            if is_reference(a) {
                out.extend(a.iter().cloned());
            } else {
                out.push(paren(Tok::LParen, "(", &a[0]));
                out.extend(a.iter().cloned());
                out.push(paren(Tok::RParen, ")", &a[a.len() - 1]));
            }
        }
        if !d.stmts {
            out.push(paren(Tok::RParen, ")", &self.toks[end - 1]));
        }
        if self.toks.len() - (end - at) + out.len() > MAX_TOKENS {
            return self.err_at(at, "the definitions expand to more than a program can hold");
        }
        // the expansions this one is inside grow by what it adds
        let grown = out.len() as isize - (end - at) as isize;
        for e in self.expanded.iter_mut().rev() {
            if e.end <= at {
                continue;
            }
            if e.start <= at {
                e.end = (e.end as isize + grown) as usize;
            }
        }
        self.expanded.push(Expanded {
            start: at,
            end: at + out.len(),
            name: d.name.clone(),
            line: use_line,
            col: use_col,
            file: use_file,
        });
        self.toks.splice(at..end, out);
        self.pos = at;
        Ok(())
    }

    /// `[N]` after a name: a family's size, a positive integer or a `let`
    /// constant that is one (`queue D[ND]`); `None` without brackets.
    fn array_count(&mut self) -> PResult<Option<usize>> {
        if *self.peek() == Tok::LBracket {
            self.advance();
            let at = self.pos;
            let e = self.expr()?;
            let mut vars = vec![];
            names(&e, &mut vars, &mut Vec::new());
            if let Some(name) = vars
                .iter()
                .find(|name| self.structural_overrides.contains(*name))
            {
                return self.err_at(
                    at,
                    format!("--set {name} affects an array size resolved during parsing"),
                );
            }
            let n = match self.const_value(&e) {
                Some(x) if x > MAX_FAMILY as f64 && x.fract() == 0.0 => {
                    return self.err_at(
                        at,
                        format!(
                            "array size {x} is more than a family holds ({MAX_FAMILY}): every member \
                             is a pool or stage of its own"
                        ),
                    );
                }
                Some(x) if x >= 1.0 && x.fract() == 0.0 => x as usize,
                Some(x) => {
                    return self.err_at(
                        at,
                        format!("array size must be a positive integer, found {x}"),
                    );
                }
                None => {
                    return self.err_at(
                        at,
                        "array size must be a positive integer or a `let` constant that is one",
                    );
                }
            };
            self.expect(&Tok::RBracket)?;
            Ok(Some(n))
        } else {
            Ok(None)
        }
    }

    /// The value of a constant expression over numbers and `let` constants,
    /// if it is one.
    fn const_value(&self, e: &Expr) -> Option<f64> {
        Some(match e {
            Expr::Located(_, inner) => self.const_value(inner)?,
            Expr::Num(x) => *x,
            Expr::Var(n) if n == "inf" => f64::INFINITY,
            Expr::Var(n) => self.consts.iter().rev().find(|(c, _)| c == n)?.1,
            Expr::Unary(UnOp::Neg, a) => -self.const_value(a)?,
            Expr::Unary(UnOp::Not, a) => {
                if self.const_value(a)? != 0.0 {
                    0.0
                } else {
                    1.0
                }
            }
            Expr::Binary(op, a, b) => {
                crate::frontend::link::binop(*op, self.const_value(a)?, self.const_value(b)?)
            }
            Expr::Cond(c, a, b) => {
                if self.const_value(c)? != 0.0 {
                    self.const_value(a)?
                } else {
                    self.const_value(b)?
                }
            }
            // the linker writes an aggregate out; the parser does not fold one
            Expr::Over(..) => return None,
            // the linker's constant functions, so that a `let` the linker
            // folds the parser folds too (`let N = min(2, 3); queue D[N]`)
            Expr::Call(f, args) => {
                let xs = args
                    .iter()
                    .map(|a| match a {
                        Arg::Expr(e) => self.const_value(e),
                        Arg::Ref(r) if r.index.is_none() => {
                            self.const_value(&Expr::Var(r.name.clone()))
                        }
                        Arg::Ref(_) => None,
                    })
                    .collect::<Option<Vec<f64>>>()?;
                crate::frontend::link::const_call(f, &xs)?
            }
            Expr::Sample(..) => return None,
        })
    }

    fn pool(&mut self) -> PResult<PoolDecl> {
        let span = Some(self.span());
        let name = self.ident()?;
        let array = self.array_count()?;
        self.expect(&Tok::LBrace)?;
        let mut d = PoolDecl {
            span,
            name,
            count: array.unwrap_or(1),
            array: array.is_some(),
            cap: Expr::Num(f64::INFINITY),
            block: None,
            evict: EvictOrder::Lru,
            preempt: Preempt::None,
            queue: QueueOrder::Fifo,
            spill: None,
            admit_via: None,
        };
        while *self.peek() != Tok::RBrace {
            let key = self.ident()?;
            match key.as_str() {
                "cap" => d.cap = self.expr()?,
                "block" => d.block = Some(self.expr()?),
                "evict" => {
                    if self.eat_kw("lru") {
                        d.evict = EvictOrder::Lru;
                    } else {
                        self.expect_kw("by")?;
                        self.expect(&Tok::LParen)?;
                        let mut keys = vec![self.expr()?];
                        while *self.peek() == Tok::Comma {
                            self.advance();
                            keys.push(self.expr()?);
                        }
                        self.expect(&Tok::RParen)?;
                        d.evict = EvictOrder::By(keys);
                    }
                }
                "preempt" => {
                    d.preempt = if self.eat_kw("lifo") {
                        Preempt::Lifo
                    } else {
                        self.expect_kw("none")?;
                        Preempt::None
                    }
                }
                "queue" => {
                    d.queue = if self.eat_kw("fifo") {
                        QueueOrder::Fifo
                    } else {
                        self.expect_kw("by")?;
                        self.expect(&Tok::LParen)?;
                        let mut keys = vec![self.expr()?];
                        while *self.peek() == Tok::Comma {
                            self.advance();
                            keys.push(self.expr()?);
                        }
                        self.expect(&Tok::RParen)?;
                        QueueOrder::By(keys)
                    }
                }
                "admit" => {
                    self.expect_kw("via")?;
                    d.admit_via = Some(self.bare_reference()?);
                }
                "spill" => {
                    let to = self.bare_reference()?;
                    self.expect_kw("via")?;
                    let via = self.bare_reference()?;
                    self.expect(&Tok::LParen)?;
                    let work = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    self.expect_kw("when")?;
                    self.expect(&Tok::LParen)?;
                    let when = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    d.spill = Some(Spill {
                        to,
                        via,
                        work,
                        when,
                    });
                }
                other => return self.err(format!("unknown pool option `{other}`")),
            }
            self.expect(&Tok::Semi)?;
        }
        self.expect(&Tok::RBrace)?;
        Ok(d)
    }

    fn stage(&mut self) -> PResult<StageDecl> {
        let span = Some(self.span());
        let name = self.ident()?;
        let array = self.array_count()?;
        self.expect(&Tok::Colon)?;
        let kind = self.stage_kind()?;
        Ok(StageDecl {
            span,
            name,
            count: array.unwrap_or(1),
            array: array.is_some(),
            kind,
        })
    }

    /// A stage's kind, `fifo`, `ps (phi)`, `delay` or `step { … }`, with its
    /// closing semicolon (none after `step { … }`).
    fn stage_kind(&mut self) -> PResult<StageKind> {
        let kind = if self.eat_kw("fifo") {
            if *self.peek() == Tok::LParen {
                self.advance();
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                StageKind::Fifo(e)
            } else {
                StageKind::Fifo(Expr::Num(1.0))
            }
        } else if self.eat_kw("ps") {
            self.expect(&Tok::LParen)?;
            let e = self.expr()?;
            self.expect(&Tok::RParen)?;
            StageKind::Ps(e)
        } else if self.eat_kw("delay") {
            StageKind::Delay
        } else if self.eat_kw("step") {
            self.expect(&Tok::LBrace)?;
            let mut s = StepSpec {
                budget: Expr::Num(f64::INFINITY),
                cost: Expr::Num(0.0),
                chunk: Expr::Num(0.0),
                serve: Serve::Admission,
                only: None,
                memory: None,
            };
            let mut has_cost = false;
            let mut has_serve = false;
            while *self.peek() != Tok::RBrace {
                let key = self.ident()?;
                match key.as_str() {
                    "budget" => s.budget = self.expr()?,
                    "cost" => {
                        s.cost = self.expr()?;
                        has_cost = true;
                    }
                    "chunk" => s.chunk = self.expr()?,
                    "serve" => {
                        if has_serve {
                            return self.err(
                                "`serve` twice: a step stage serves its residents in one way",
                            );
                        }
                        has_serve = true;
                        if self.eat_kw("only") {
                            self.expect(&Tok::LParen)?;
                            s.only = Some(self.expr()?);
                            self.expect(&Tok::RParen)?;
                            if *self.peek() == Tok::Semi {
                                self.expect(&Tok::Semi)?;
                                continue;
                            }
                            if self.is_kw("exclusive") {
                                return self.err(
                                    "`serve only (…) exclusive prefill`: the exclusive rule admits a waiting prefill in place of the decodes it displaces, and what `only` would do to either is a third rule",
                                );
                            }
                        }
                        s.serve = if self.eat_kw("admission") {
                            Serve::Admission
                        } else if self.eat_kw("decode") {
                            self.expect_kw("first")?;
                            Serve::DecodeFirst
                        } else if self.eat_kw("exclusive") {
                            self.expect_kw("prefill")?;
                            Serve::ExclusivePrefill
                        } else if self.eat_kw("by") {
                            self.expect(&Tok::LParen)?;
                            let mut keys = vec![self.expr()?];
                            while *self.peek() == Tok::Comma {
                                self.expect(&Tok::Comma)?;
                                keys.push(self.expr()?);
                            }
                            self.expect(&Tok::RParen)?;
                            Serve::By(keys)
                        } else {
                            return self.err(format!(
                                "`serve` takes `admission`, `decode first`, `by (keys)` or `exclusive prefill`, the first three after an optional `only (expr)`; found {}",
                                self.peek()
                            ));
                        };
                    }
                    "exclusive" => {
                        return self.err(
                            "`exclusive prefill;` is now `serve exclusive prefill;`: a step stage serves its residents in one way",
                        );
                    }
                    "decode" => {
                        return self.err(
                            "`decode first;` is now `serve decode first;`: a step stage serves its residents in one way",
                        );
                    }
                    "memory" => s.memory = Some(self.bare_reference()?),
                    other => return self.err(format!("unknown step option `{other}`")),
                }
                self.expect(&Tok::Semi)?;
            }
            self.expect(&Tok::RBrace)?;
            if !has_cost {
                return self.err("a step stage needs `cost`");
            }
            StageKind::Step(Box::new(s))
        } else {
            return self.err(format!("unknown stage kind {}", self.peek()));
        };
        if self.is_kw("latency") {
            if self.latency_ok {
                return Ok(kind);
            }
            return self.err(
                "`latency` belongs to the `serve` of a queue that plays `link`: a link's fixed wait",
            );
        }
        // `step { ... }` needs no semicolon
        if *self.peek() == Tok::Semi || !matches!(kind, StageKind::Step(_)) {
            self.expect(&Tok::Semi)?;
        }
        Ok(kind)
    }

    // ------------------------------------------------------------- queues

    /// `queue NAME [N] [: ROLE, …] { pool …; serve kind; VERB (params) [from NAME] block; … }`
    /// after the keyword. The pools go to the program as `NAME.pool`, the
    /// stage as `NAME`, and the entries wait for their calls (`assemble`).
    /// A gateway's `route` body is selected explicitly by `request NAME;`.
    fn queue(&mut self, prog: &mut Program, at: usize) -> PResult<()> {
        let span = Some(self.span());
        let name = self.ident()?;
        if Role::of(&name).is_some() || queue::ROLES.iter().any(|(r, _)| *r == name) {
            return self.err_at(
                at + 1,
                format!("`{name}` is a serving word; a queue needs another name"),
            );
        }
        if self.queues.iter().any(|q| q.name == name) {
            return self.err_at(at + 1, format!("duplicate queue `{name}`"));
        }
        // `Q[n]` is a family whatever `n`, called by index, and its pools and
        // stages are arrays; `Q` is one queue
        let array = self.array_count()?;
        let family = array.is_some();
        let count = array.unwrap_or(1);
        let mut roles = vec![];
        if *self.peek() == Tok::Colon {
            self.advance();
            loop {
                let r_at = self.pos;
                let r = self.ident()?;
                if !queue::ROLES.iter().any(|(name, _)| *name == r) {
                    return self.err_at(
                        r_at,
                        format!(
                            "unknown role `{r}`: a queue plays gateway, prefill, decode or link"
                        ),
                    );
                }
                if roles.contains(&r) {
                    return self.err_at(r_at, format!("role `{r}` twice"));
                }
                roles.push(r);
                if *self.peek() == Tok::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        let is_gateway = roles.iter().any(|r| r == "gateway");
        if is_gateway && family {
            return self.err_at(
                at,
                "a gateway is one queue, not a family: the requests enter it",
            );
        }
        self.queues.push(QueueDecl {
            name: name.clone(),
            count,
            family,
            roles: roles.clone(),
            pools: vec![],
            has_stage: false,
            entries: vec![],
            leased: None,
            marks: vec![],
            latency: None,
            nic: false,
            pulls: None,
            at,
        });
        let qi = self.queues.len() - 1;
        self.expect(&Tok::LBrace)?;
        while *self.peek() != Tok::RBrace {
            // pools, then the stage, then the entries: each reads what is above it
            let item_at = self.pos;
            if self.is_kw("pool")
                && (self.queues[qi].has_stage || !self.queues[qi].entries.is_empty())
            {
                return self.err_at(
                    item_at,
                    format!(
                        "queue `{name}` declares its pools first, above its `serve` and entries"
                    ),
                );
            }
            if self.is_kw("serve") && !self.queues[qi].entries.is_empty() {
                return self.err_at(
                    item_at,
                    format!("queue `{name}` declares its `serve` above its entries"),
                );
            }
            if self.eat_kw("pool") {
                let p_at = self.pos;
                let mut d = self.pool()?;
                if d.count != 1 {
                    return self.err_at(
                        p_at,
                        format!(
                            "`pool {}[…]` in queue `{name}`: a queue's pool is the member's, one per \
                             member of `{name}`; write `pool {}`",
                            d.name, d.name
                        ),
                    );
                }
                if self.queues[qi].pools.contains(&d.name) {
                    return self.err(format!("duplicate pool `{}` in queue `{name}`", d.name));
                }
                self.queues[qi].pools.push(d.name.clone());
                d.name = format!("{name}.{}", d.name);
                d.count = count;
                d.array = family;
                prog.pools.push(d);
            } else if self.is_kw("serve") {
                let s_at = self.pos;
                self.advance();
                if self.queues[qi].has_stage {
                    return self
                        .err_at(s_at, format!("`serve` twice: queue `{name}` is one stage"));
                }
                self.latency_ok = roles.iter().any(|r| r == "link");
                let kind = self.stage_kind();
                self.latency_ok = false;
                let mut kind = kind?;
                if self.eat_kw("latency") {
                    let l_at = self.pos;
                    let e = self.expr()?;
                    let Some(v) = self.const_value(&e) else {
                        return self.err_at(
                            l_at,
                            "`latency` is a number or a constant over `let`s: a link's fixed wait, \
                             the same for every transfer",
                        );
                    };
                    self.expect(&Tok::Semi)?;
                    // a constant of its own, which the transfer's wait reads: an
                    // entry the transfer is written in substitutes its parameters
                    // and locals by name, and no name of one has a dot. The
                    // expression stays the program's, so `--set` reaches it.
                    let lname = format!("{name}.latency.time");
                    self.consts.push((lname.clone(), v));
                    prog.lets.push((lname.clone(), e));
                    self.queues[qi].latency = Some(Expr::Var(lname));
                }
                if let StageKind::Step(s) = &mut kind
                    && let Some(m) = &mut s.memory
                    && self.queues[qi].pools.contains(&m.name)
                {
                    m.name = format!("{name}.{}", m.name);
                }
                self.queues[qi].has_stage = true;
                self.stages
                    .push((name.clone(), matches!(kind, StageKind::Step(_))));
                prog.stages.push(StageDecl {
                    span,
                    name: name.clone(),
                    count,
                    array: family,
                    kind,
                });
            } else if self.is_kw("nic") {
                let n_at = self.pos;
                self.advance();
                if self.queues[qi].nic {
                    return self.err_at(n_at, format!("`nic` twice: queue `{name}` has one NIC"));
                }
                if !self.queues[qi].entries.is_empty() {
                    return self.err_at(
                        n_at,
                        format!("queue `{name}` declares its `nic` above its entries"),
                    );
                }
                let kind = self.stage_kind()?;
                self.queues[qi].nic = true;
                let nname = format!("{name}.nic");
                self.stages.push((nname.clone(), false));
                prog.stages.push(StageDecl {
                    span,
                    name: nname,
                    count,
                    array: family,
                    kind,
                });
            } else if let Tok::Ident(verb) = self.peek().clone() {
                self.entry(qi, verb)?;
            } else {
                return self.err(format!(
                    "expected `pool`, `serve` or an entry in queue `{name}`, found {}",
                    self.peek()
                ));
            }
        }
        self.expect(&Tok::RBrace)?;
        // a link's latency: a delay stage of its own, which a transfer over
        // the link runs first
        if self.queues[qi].latency.is_some() {
            if self.queues[qi].entry("transfer", false).is_some() {
                return self.err_at(
                    at,
                    format!(
                        "queue `{name}` has a `transfer` entry and a `latency`: a called link's \
                         time is its entry's; write the latency in the entry body"
                    ),
                );
            }
            let lname = format!("{name}.latency");
            self.stages.push((lname.clone(), false));
            prog.stages.push(StageDecl {
                span,
                name: lname,
                count,
                array: family,
                kind: StageKind::Delay,
            });
        }
        // every role's entries are there, and nothing else
        let q = &self.queues[qi];
        for r in &roles {
            let (_, entries) = queue::ROLES.iter().find(|(n, _)| n == r).expect("a role");
            // a role's first entry is required; the others (`decode … from`) may be left out,
            // and a link's too: its `serve` is its cost (`transfer on L[k] (n) …`)
            let (verb, _, from) = entries[0];
            if *r == "link" && q.entry(verb, from).is_none() {
                if !q.has_stage {
                    return self.err_at(
                        at,
                        format!(
                            "queue `{name}` plays `link` and has neither a `serve` nor a \
                             `transfer` entry: nothing says what a transfer over it costs"
                        ),
                    );
                }
            } else if q.entry(verb, from).is_none() {
                return self.err_at(
                    at,
                    format!("queue `{name}` plays `{r}` and has no `{verb}` entry"),
                );
            }
        }
        for e in &q.entries {
            let role = queue::role_of_verb(&e.verb, e.from.is_some());
            match role {
                Some(r) if roles.iter().any(|x| x == r) => {}
                Some(r) => {
                    return self.err_at(
                        e.at,
                        format!(
                            "`{}` is an entry of a `{r}` queue; declare `queue {name} : {r}`",
                            e.verb
                        ),
                    );
                }
                None => {
                    let hint = if e.from.is_some() {
                        format!(": `{}` takes no `from`", e.verb)
                    } else {
                        String::new()
                    };
                    return self.err_at(
                        e.at,
                        format!("`{}` is not an entry of any role{hint}", e.verb),
                    );
                }
            }
        }
        Ok(())
    }

    /// `VERB (params) [from NAME] block` inside a queue: parsed on the
    /// server's side, with the queue's own stage the one the serving forms
    /// name.
    fn entry(&mut self, qi: usize, verb: String) -> PResult<()> {
        let at = self.pos;
        self.advance();
        let mut params = vec![];
        // `route { … }`: an entry without parameters needs no `()`
        if *self.peek() == Tok::LParen {
            self.advance();
            if *self.peek() != Tok::RParen {
                loop {
                    let p = self.ident()?;
                    if params.contains(&p) {
                        return self.err(format!("parameter `{p}` twice"));
                    }
                    params.push(p);
                    if *self.peek() == Tok::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect(&Tok::RParen)?;
        }
        let from = if self.eat_kw("from") {
            Some(self.ident()?)
        } else {
            None
        };
        let qname = self.queues[qi].name.clone();
        if self.queues[qi].entry(&verb, from.is_some()).is_some() {
            return self.err_at(at, format!("queue `{qname}` has `{verb}` twice"));
        }
        // the role's signature: `prefill (prompt)`, not one of the program's
        if let Some(&(_, n, _)) = queue::ROLES
            .iter()
            .flat_map(|(_, sigs)| sigs.iter())
            .find(|(v, _, f)| *v == verb && *f == from.is_some())
            && params.len() != n
        {
            return self.err_at(
                at,
                format!(
                    "`{qname}.{verb}` takes {n} parameter(s), as the role says (`{verb}{}`); \
                     it declares {}",
                    if n == 0 {
                        String::new()
                    } else {
                        " (…)".into()
                    },
                    params.len()
                ),
            );
        }
        let outer_side = self.side;
        let outer_stages = std::mem::take(&mut self.stages);
        // the body's serving forms find the queue's own step engine; the
        // stages that are not one (a link's, a delay) it may name
        let own = self.queues[qi].has_stage.then(|| {
            outer_stages
                .iter()
                .rev()
                .find(|(n, _)| *n == qname)
                .cloned()
                .expect("the queue's stage")
        });
        self.stages = outer_stages
            .iter()
            .filter(|(n, step)| !step && own.as_ref().is_none_or(|o| o.0 != *n))
            .cloned()
            .chain(own.clone())
            .collect();
        self.side = Side::Server;
        self.in_queue = Some(qi);
        self.entry_from = from.clone();
        let body = self.block();
        self.entry_from = None;
        self.in_queue = None;
        self.side = outer_side;
        self.stages = outer_stages;
        let body = body?;
        // what the body sets is the entry's; what it marks the caller reads;
        // what it leases a `from` takes
        let mut locals = vec![];
        let mut marks = vec![];
        let mut leased = vec![];
        collect_entry(&body, &mut locals, &mut marks, &mut leased);
        if let Some(p) = locals.iter().find(|l| params.contains(l)) {
            return self.err_at(at, format!("`{qname}.{verb}` sets its parameter `{p}`"));
        }
        if let Some(l) = locals
            .iter()
            .find(|l| self.consts.iter().any(|(c, _)| c == *l))
        {
            return self.err_at(
                at,
                format!(
                    "`{qname}.{verb}` sets `{l}`, a `let` constant: the header would read the \
                     constant and the body the attribute\nhelp: name the attribute otherwise"
                ),
            );
        }
        let q = &self.queues[qi];
        for l in &leased {
            if !q.pools.contains(l) {
                return self.err_at(
                    at,
                    format!("`{qname}.{verb}` leases `{l}`, which is not a pool of `{qname}`"),
                );
            }
            if let Some(other) = &q.leased
                && other != l
            {
                return self.err_at(
                    at,
                    format!(
                        "`{qname}` leases `{other}` and `{l}`: a `from {qname}` needs one pool to take"
                    ),
                );
            }
        }
        // the header sees the parameters and the queue's own; the body also
        // the context and the request's hidden attributes
        let mut reads = vec![];
        // the gateway's `route` is the deployment's and reads as a server
        // does; any other entry of a queue that is also a gateway is an entry
        let is_gateway = verb == "route" && q.roles.iter().any(|r| r == "gateway");
        let allowed_var = |n: &str, header: bool| -> bool {
            params.iter().any(|p| p == n)
                || n == "self"
                || n == "inf"
                || self.consts.iter().any(|(c, _)| c == n)
                || CONTEXT.contains(&n)
                || (!header && (n == "now" || locals.contains(&n.to_string())))
        };
        let own_ref = |r: &Ref| -> bool {
            q.pools.contains(&r.name)
                || r.name == qname
                || from.as_deref() == Some(r.name.as_str())
                || params.contains(&r.name)
        };
        if !is_gateway {
            let mut headers = vec![];
            let mut bodies = vec![];
            let mut indices = vec![];
            split_reads(&body, &mut headers, &mut bodies, &mut indices);
            // an own pool, or the `from` name, is all an entry's statements hold
            let mut named = vec![];
            pools_named(&body, &mut named);
            if let Some(r) = named
                .iter()
                .find(|r| !q.pools.contains(&r.name) && from.as_deref() != Some(r.name.as_str()))
            {
                return self.err_at(
                    at,
                    format!(
                        "`{qname}.{verb}` holds `{}`, which is not a pool of `{qname}`: an entry \
                         allocates its queue's own pools{}",
                        r.name,
                        match &from {
                            Some(f) => format!(" and `{f}`, the pool its `from` names"),
                            None => String::new(),
                        }
                    ),
                );
            }
            for (e, header, in_index) in headers
                .iter()
                .map(|e| (e, true, false))
                .chain(bodies.iter().map(|e| (e, false, false)))
                .chain(indices.iter().map(|e| (e, false, true)))
            {
                let mut vars = vec![];
                let mut indexed = vec![];
                let mut refs = vec![];
                names_in(e, &mut vars, &mut indexed, &mut refs);
                let tagged = vars
                    .into_iter()
                    .map(|v| (v, in_index))
                    .chain(indexed.into_iter().map(|v| (v, true)));
                for (v, in_index) in tagged {
                    // `from src`: the source pool, and in an index its member
                    if from.as_deref() == Some(v.as_str()) {
                        if in_index {
                            continue;
                        }
                        return self.err_at(
                            at,
                            format!(
                                "`{qname}.{verb}` reads `{v}` as a number: `{v}` is the source pool, \
                                 and only in an index the source member (`L[{v}]`)"
                            ),
                        );
                    }
                    if allowed_var(&v, header) {
                        continue;
                    }
                    if v.contains('.') {
                        return self.err_at(
                            at,
                            format!(
                                "`{qname}.{verb}` reads `{v}`, another queue's: an entry sees its own; \
                                 the gateway reads across queues and passes what an entry needs"
                            ),
                        );
                    }
                    if header {
                        return self.err_at(
                            at,
                            format!(
                                "`{qname}.{verb}`: the header reads `{v}`; an entry's header sees its \
                                 parameters and the queue's pools and stage. Pass `{v}` as a parameter",
                            ),
                        );
                    }
                    if !reads.contains(&v) {
                        reads.push(v);
                    }
                }
                for r in refs {
                    // a bare identifier argument may be a variable; a dotted
                    // one is another queue's pool
                    if r.index.is_none()
                        && !own_ref(&r)
                        && !r.name.contains('.')
                        && allowed_var(&r.name, header)
                    {
                        continue;
                    }
                    if r.index.is_none() && !own_ref(&r) && !r.name.contains('.') && !header {
                        if !reads.contains(&r.name) {
                            reads.push(r.name.clone());
                        }
                        continue;
                    }
                    if !own_ref(&r) {
                        return self.err_at(
                            at,
                            format!(
                                "`{qname}.{verb}` reads `{}`, which is not a pool or stage of `{qname}`: \
                                 a queue sees its own; the deployment reads across queues",
                                r.name
                            ),
                        );
                    }
                }
            }
        }
        let q = &mut self.queues[qi];
        if let Some(l) = leased.first() {
            q.leased = Some(l.clone());
        }
        for m in marks {
            if !q.marks.contains(&m) {
                q.marks.push(m);
            }
        }
        let leases = always_leases(&body);
        q.entries.push(queue::Entry {
            verb,
            params,
            from,
            leases,
            body,
            locals: if is_gateway { vec![] } else { locals },
            reads,
            at,
        });
        Ok(())
    }

    fn workload(&mut self) -> PResult<Workload> {
        self.expect(&Tok::LBrace)?;
        let mut w = Workload {
            arrive: Arrival::None,
            trace: None,
            trace_ordered: false,
            init: vec![],
            turn: vec![],
            hidden: vec![],
        };
        while *self.peek() != Tok::RBrace {
            if self.eat_kw("hidden") {
                w.hidden.push(self.ident()?);
                while *self.peek() == Tok::Comma {
                    self.expect(&Tok::Comma)?;
                    w.hidden.push(self.ident()?);
                }
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("arrive") {
                w.arrive = if self.eat_kw("poisson") {
                    self.expect(&Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    Arrival::Poisson(e)
                } else if self.eat_kw("renewal") {
                    self.expect(&Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    Arrival::Renewal(e)
                } else if self.eat_kw("closed") {
                    self.expect(&Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    Arrival::Closed(e)
                } else if self.eat_kw("batch") {
                    self.expect(&Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    Arrival::Batch(e)
                } else {
                    self.expect_kw("none")?;
                    Arrival::None
                };
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("trace") {
                w.trace = Some(self.string()?);
                w.trace_ordered = self.eat_kw("ordered");
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("init") {
                w.init = self.block()?;
            } else if self.eat_kw("turn") {
                w.turn = self.block()?;
            } else if self.is_kw("session") {
                let at = self.pos;
                self.advance();
                if self.wl_session.is_some() {
                    return self.err_at(at, "duplicate session in workload");
                }
                self.side = Side::WorkloadSession;
                let body = self.block()?;
                self.side = Side::Session;
                self.wl_session = Some((at, body));
            } else {
                return self.err(format!("unexpected {} in workload", self.peek()));
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(w)
    }

    fn block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect(&Tok::LBrace)?;
        let mut v = vec![];
        while *self.peek() != Tok::RBrace {
            self.stmt_into(&mut v)?;
        }
        self.expect(&Tok::RBrace)?;
        Ok(v)
    }

    /// One statement into `out`. A serving form is parsed here because
    /// `transfer … from P to Q (n)` stands for three kernel statements.
    fn stmt_into(&mut self, out: &mut Vec<Stmt>) -> PResult<()> {
        if let Some(i) = self.use_of_def() {
            if !self.defs[i].stmts {
                return self.err(format!(
                    "`{}` is an expression (`def {0}(…) = …;`), not statements",
                    self.defs[i].name
                ));
            }
            return self.expand(i);
        }
        if let Tok::Ident(s) = self.peek()
            && let Some(role) = Role::of(s)
        {
            out.extend(self.serving(role)?);
            return Ok(());
        }
        out.push(self.stmt()?);
        Ok(())
    }

    /// `name`, `name[i]`, `Q.p` or `Q[i].p`: a pool or stage, the last two a
    /// queue's, read from outside it.
    fn reference(&mut self) -> PResult<Ref> {
        let span = Some(self.span());
        let name = self.ident()?;
        let index = if *self.peek() == Tok::LBracket {
            self.advance();
            let e = self.expr()?;
            self.expect(&Tok::RBracket)?;
            Some(Box::new(e))
        } else {
            None
        };
        let name = if *self.peek() == Tok::Dot {
            self.advance();
            let full = format!("{name}.{}", self.ident()?);
            if index.is_some() {
                // `D[j].kv` is a member's pool; `D[j].first_token` would be a
                // member's mark, and a mark is the request's
                self.indexed_dotted.push((self.pos - 1, full.clone()));
            }
            full
        } else {
            name
        };
        Ok(Ref { span, name, index })
    }

    /// A pool a statement acts on (hold, grow, drop, release, load, lease): a
    /// queue's pool is its entries' alone.
    fn own_pool(&mut self, what: &str) -> PResult<Ref> {
        let at = self.pos;
        let r = self.reference()?;
        let here = self.in_queue.map(|qi| self.queues[qi].name.as_str());
        if let Some((q, p)) = r.name.split_once('.')
            && here != Some(q)
        {
            return self.err_at(
                at,
                format!(
                    "`{what} {}`: `{p}` is a pool of queue `{q}`, and only `{q}`'s entries hold it; \
                     call one (`{q}.verb (…)`)",
                    r.name
                ),
            );
        }
        Ok(r)
    }

    /// `Q[i].verb (args) [from S[k]] [to P (m)];`
    fn call(&mut self) -> PResult<Stmt> {
        let at = self.pos;
        let span = Some(self.span());
        let name = self.ident()?;
        let index = if *self.peek() == Tok::LBracket {
            self.advance();
            let e = self.expr()?;
            self.expect(&Tok::RBracket)?;
            Some(Box::new(e))
        } else {
            None
        };
        if *self.peek() != Tok::Dot {
            return self.err_at(
                at,
                format!("`{name}[…]` as a statement: a queue's entry is `{name}[i].verb (…)`"),
            );
        }
        self.advance();
        let verb = self.ident()?;
        self.expect(&Tok::LParen)?;
        let mut args = vec![];
        if *self.peek() != Tok::RParen {
            args.push(self.expr()?);
            while *self.peek() == Tok::Comma {
                self.advance();
                args.push(self.expr()?);
            }
        }
        self.expect(&Tok::RParen)?;
        let from = if self.eat_kw("from") {
            let f_at = self.pos;
            let r = self.reference()?;
            if let Some((q, _)) = r.name.split_once('.') {
                // the lease check is the queue's: whichever entry of `q` ran
                return self.err_at(
                    f_at,
                    format!(
                        "`from {}`: a call takes from a queue, `from {q}[…]`",
                        r.name
                    ),
                );
            }
            Some(r)
        } else {
            None
        };
        let to = if self.eat_kw("to") {
            let r = self.reference()?;
            let m = self.paren_expr()?;
            Some((r, m))
        } else {
            None
        };
        self.expect(&Tok::Semi)?;
        Ok(Stmt::Call {
            queue: Ref { span, name, index },
            verb,
            args,
            from,
            to,
        })
    }

    /// At an identifier followed by `[` or `.`: does the reference (`p[i]`,
    /// `Q.p`, `Q[i].p`) end the call argument?
    fn ref_ends_arg(&self) -> bool {
        let mut k = 1;
        if *self.peek_at(k) == Tok::LBracket {
            let mut depth = 0;
            loop {
                match self.peek_at(k) {
                    Tok::LBracket => depth += 1,
                    Tok::RBracket => {
                        depth -= 1;
                        if depth == 0 {
                            k += 1;
                            break;
                        }
                    }
                    Tok::Eof => return false,
                    _ => {}
                }
                k += 1;
            }
        }
        if *self.peek_at(k) == Tok::Dot {
            k += 2;
        }
        matches!(self.peek_at(k), Tok::Comma | Tok::RParen)
    }

    /// At `[`: is the matching `]` followed by `.`? (`Q[i].x` in an expression, refused)
    fn in_expr_index_dot(&self) -> bool {
        let mut depth = 0;
        let mut k = 0;
        loop {
            match self.peek_at(k) {
                Tok::LBracket => depth += 1,
                Tok::RBracket => {
                    depth -= 1;
                    if depth == 0 {
                        return *self.peek_at(k + 1) == Tok::Dot;
                    }
                }
                Tok::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    fn paren_expr(&mut self) -> PResult<Expr> {
        self.expect(&Tok::LParen)?;
        let e = self.expr()?;
        self.expect(&Tok::RParen)?;
        Ok(e)
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let kw = match self.peek() {
            Tok::Ident(s) => s.clone(),
            other => return self.err(format!("expected a statement, found {other}")),
        };
        match kw.as_str() {
            "turn" => {
                if self.side == Side::Server {
                    return self.err(
                        "`turn` is the session's: the next turn is the workload's decision, \
                         not the server's",
                    );
                }
                self.advance();
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Turn)
            }
            "end" => {
                if self.side == Side::Server {
                    return self.err(
                        "`end` is the session's: a server is done with a request when its \
                         block is, and whether the session goes on is the workload's",
                    );
                }
                self.advance();
                self.expect(&Tok::Semi)?;
                Ok(Stmt::End)
            }
            "request" => {
                match self.side {
                    Side::WorkloadSession => {}
                    Side::Server => {
                        return self.err("`request` inside a server or queue entry: a server does not request itself");
                    }
                    Side::Session => {
                        return self
                            .err("`request` is a statement of the `session` inside `workload`");
                    }
                }
                let at = self.pos;
                self.advance();
                if *self.peek() == Tok::Semi {
                    self.advance();
                    self.requests.push((at, None));
                    Ok(Stmt::Request)
                } else {
                    let target_at = self.pos;
                    let queue = self.bare_reference()?;
                    self.expect(&Tok::Semi)?;
                    self.requests.push((target_at, Some(queue.name.clone())));
                    Ok(Stmt::Call {
                        queue,
                        verb: "route".into(),
                        args: vec![],
                        from: None,
                        to: None,
                    })
                }
            }
            "set" => {
                self.advance();
                let name = self.definition()?;
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Set(name, e))
            }
            "observe" => {
                self.advance();
                let name = self.ident()?;
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Observe(name, e))
            }
            "hold" => {
                self.advance();
                self.hold()
            }
            "grow" => {
                self.advance();
                let r = self.own_pool("grow")?;
                let e = self.paren_expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Grow(r, e))
            }
            "drop" => {
                self.advance();
                let r = self.own_pool("drop")?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Drop(r))
            }
            "release" => {
                self.advance();
                let r = self.own_pool("release")?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Release(r))
            }
            "load" => {
                self.advance();
                let r = self.own_pool("load")?;
                let e = self.paren_expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Load(r, e))
            }
            "run" => {
                self.advance();
                // `run (X);` inside a queue: its own stage
                let stage = if *self.peek() == Tok::LParen {
                    match self.in_queue {
                        Some(qi) if self.queues[qi].has_stage => Ref {
                            span: Some(self.span()),
                            name: self.queues[qi].name.clone(),
                            index: None,
                        },
                        Some(qi) => {
                            return self.err(format!(
                                "`run (…)` in queue `{}`, which has no `serve`",
                                self.queues[qi].name
                            ));
                        }
                        None => return self.err("`run` names a stage: `run STAGE (X)`"),
                    }
                } else {
                    self.reference()?
                };
                let also = self.more_stages()?;
                let mode = if self.eat_kw("prefill") {
                    RunMode::Prefill
                } else if self.eat_kw("decode") {
                    RunMode::Decode
                } else {
                    RunMode::Plain
                };
                let work = self.paren_expr()?;
                let growing = if self.eat_kw("growing") {
                    Some(self.own_pool("growing")?)
                } else {
                    None
                };
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing,
                    also,
                })
            }
            "branch" => {
                self.advance();
                // `branch with (p)` is a draw, and says so. It rewrites to
                // `branch (~bernoulli(p))`: the sample is a 0 or a 1 by the
                // time the guard sees it, and a bare `branch (p)` with a
                // fractional `p` is an error, not a draw. So the sugar is
                // free and the only way to write the draw.
                let draw = self.eat_kw("with");
                let e = self.paren_expr()?;
                let guard = if draw {
                    Expr::Sample("bernoulli".into(), vec![e])
                } else {
                    e
                };
                let then = self.block()?;
                let els = if self.eat_kw("else") {
                    self.block()?
                } else {
                    vec![]
                };
                Ok(Stmt::Branch(guard, then, els))
            }
            "loop" => {
                self.advance();
                Ok(Stmt::Loop(self.block()?))
            }
            "choose" => {
                self.advance();
                let var = self.definition()?;
                self.expect_kw("in")?;
                let count = self.expr()?;
                self.expect_kw("by")?;
                self.expect(&Tok::LParen)?;
                let mut key = vec![self.expr()?];
                while *self.peek() == Tok::Comma {
                    self.advance();
                    key.push(self.expr()?);
                }
                self.expect(&Tok::RParen)?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Choose { var, count, key })
            }
            "mark" => {
                if self.in_queue.is_none() {
                    return self.err(
                        "`mark` is a queue entry's statement: it names a moment the caller reads",
                    );
                }
                self.advance();
                let name = self.ident()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Mark(name))
            }
            // `Q.verb (…);`, `Q[i].verb (…) from S[k] to p (m);`: a queue's entry
            _ if matches!(self.peek_at(1), Tok::Dot | Tok::LBracket) => self.call(),
            // the serving forms (`prefill`, `transfer`, `decode`, `tool`) are
            // parsed by `stmt_into`, since one of them stands for several
            // statements
            other => match retired(other) {
                Some(now) => self.err(now),
                None => self.err(format!("unknown statement `{other}`")),
            },
        }
    }

    /// The rest of a `hold` statement after the keyword.
    fn hold(&mut self) -> PResult<Stmt> {
        let mut pools = vec![];
        loop {
            let r = self.own_pool("hold")?;
            let e = self.paren_expr()?;
            // `fits` was this clause's name until it was renamed for saying
            // what the pool observes rather than what the session requires.
            if self.is_kw("fits") {
                return self.err("`fits` is now `reserve`");
            }
            let reserve = if self.eat_kw("reserve") {
                Some(self.paren_expr()?)
            } else {
                None
            };
            pools.push((r, e, reserve));
            if *self.peek() == Tok::Comma {
                self.advance();
            } else {
                break;
            }
        }
        let mut reuse = if self.eat_kw("reuse") {
            Some(self.paren_expr()?)
        } else {
            None
        };
        // `at admission (hit = e, ...)` names values the header is written in
        // terms of. Everything in a hold's header is evaluated when the
        // session is admitted; a `set` above the hold is not, and looks the
        // same. The bindings are substituted into the header's expressions
        // here, so the interpreter never sees them.
        self.bind_at.clear();
        if let Tok::Ident(w) = self.peek()
            && let Some(now) = retired(w)
        {
            return self.err(now);
        }
        let binds = self.at_admission()?;
        if !binds.is_empty() {
            for (_, e, reserve) in &mut pools {
                subst(e, &binds);
                if let Some(f) = reserve {
                    subst(f, &binds);
                }
            }
            if let Some(r) = &mut reuse {
                subst(r, &binds);
            }
        }
        let bind_at = std::mem::take(&mut self.bind_at);
        let mut body = self.block()?;
        // A binding the body reads is set at the top of the body, to the
        // value it had at the admission: the body runs at the admission's
        // instant, so an expression of attributes and constants reads the
        // same there. One that reads live state does not, and is refused
        // once the program's attributes are known.
        let mut sets = vec![];
        for ((name, e), &at) in binds.iter().zip(&bind_at) {
            if body.iter().any(|s| stmt_reads(s, name)) {
                sets.push(Stmt::Set(name.clone(), e.clone()));
                self.body_binds.push((at, name.clone(), e.clone()));
            }
        }
        if !sets.is_empty() {
            sets.append(&mut body);
            body = sets;
        }
        if let Tok::Ident(w) = self.peek()
            && w == "keep"
        {
            return self.err(retired("keep").unwrap());
        }
        let mut cache = if self.eat_kw("cache") {
            Some(self.paren_expr()?)
        } else {
            None
        };
        if let Some(c) = &mut cache {
            subst(c, &binds);
        }
        // `lease P (t)`: the allocation on `P` outlives the scope, for the
        // session's transfer to take, for at most `t` seconds
        let lease = if self.eat_kw("lease") {
            let r = self.own_pool("lease")?;
            let mut t = self.paren_expr()?;
            subst(&mut t, &binds);
            Some((r, t))
        } else {
            None
        };
        if *self.peek() == Tok::Semi {
            self.advance();
        }
        Ok(Stmt::Hold {
            pools,
            reuse,
            body,
            cache,
            lease,
        })
    }

    /// `at admission (hit = e, need = f)`: names for a hold's header.
    ///
    /// A later binding sees the earlier ones, so a header can be written in
    /// steps. Samples are rejected: the bindings are substituted, and a name
    /// used twice would draw twice.
    fn at_admission(&mut self) -> PResult<Vec<(String, Expr)>> {
        if !self.eat_kw("at") {
            return Ok(vec![]);
        }
        if !self.eat_kw("admission") {
            return self.err("expected `admission` after `at`");
        }
        self.expect(&Tok::LParen)?;
        let binds = self.bindings("at admission")?;
        self.expect(&Tok::RParen)?;
        Ok(binds)
    }

    /// `name = e, name = f, …`: the bindings of an `at admission (…)`, after
    /// its keyword. `clause` names it in errors.
    fn bindings(&mut self, clause: &str) -> PResult<Vec<(String, Expr)>> {
        let mut binds: Vec<(String, Expr)> = vec![];
        loop {
            self.bind_at.push(self.pos);
            let name = self.name()?;
            if binds.iter().any(|(n, _)| *n == name) {
                return self.err(format!("`{name}` is bound twice in one `{clause}`"));
            }
            self.expect(&Tok::Assign)?;
            let mut e = self.expr()?;
            if has_sample(&e) {
                return self.err(format!(
                    "`{name}` draws a sample: a `{clause}` binding is substituted, \
                     so a name used twice would draw twice"
                ));
            }
            subst(&mut e, &binds);
            binds.push((name, e));
            if *self.peek() == Tok::Comma {
                self.advance();
            } else {
                break;
            }
        }
        // the earlier bindings are substituted, so a name of the clause still
        // read is a later one's, which the binding cannot see
        let at = self.bind_at.len() - binds.len();
        for (i, (name, e)) in binds.iter().enumerate() {
            if let Some((later, _)) = binds[i + 1..].iter().find(|(n, _)| expr_reads(e, n)) {
                return self.err_at(
                    self.bind_at[at + i],
                    format!(
                        "`{name}` reads `{later}`, which is bound after it: a binding \
                         sees the ones before it"
                    ),
                );
            }
        }
        Ok(binds)
    }

    /// `D pull P latency x share maxmin;`: the KV the entries of `D` take
    /// `from P` is read by `D`, over `P`'s NIC and `D`'s at once, after `D`
    /// waits `x`; concurrent reads divide the two NICs by the policy, which
    /// is the program's `share`.
    fn pull_relation(&mut self, prog: &mut Program) -> PResult<()> {
        let at = self.pos;
        let span = Some(self.span());
        let puller = self.ident()?;
        self.advance(); // `pull`
        let s_at = self.pos;
        let source = self.ident()?;
        for (q, q_at) in [(&puller, at), (&source, s_at)] {
            let Some(d) = self.queues.iter().find(|d| d.name == *q) else {
                return self.err_at(
                    q_at,
                    format!("`{puller} pull {source}`: no queue `{q}` is declared above"),
                );
            };
            if !d.nic {
                return self.err_at(
                    q_at,
                    format!(
                        "`{puller} pull {source}`: queue `{q}` has no `nic`; a read runs over \
                         the source's NIC and the reader's"
                    ),
                );
            }
        }
        if puller == source {
            return self.err_at(
                s_at,
                format!("`{puller} pull {source}`: a queue reads from another"),
            );
        }
        let qi = self
            .queues
            .iter()
            .position(|d| d.name == puller)
            .expect("checked above");
        if let Some((other, _)) = &self.queues[qi].pulls {
            return self.err_at(
                at,
                format!("`{puller}` pulls from `{other}` already: one relation per reader"),
            );
        }
        let latency = if self.eat_kw("latency") {
            let l_at = self.pos;
            let e = self.expr()?;
            let Some(v) = self.const_value(&e) else {
                return self.err_at(
                    l_at,
                    "`latency` is a number or a constant over `let`s: the reader's fixed wait \
                     before each read",
                );
            };
            // a constant of its own (see the link's `latency`), and a delay
            // stage of the reader's, one per member
            let lname = format!("{puller}.pull.time");
            self.consts.push((lname.clone(), v));
            prog.lets.push((lname.clone(), e));
            let count = self.queues[qi].count;
            let sname = format!("{puller}.nic.latency");
            self.stages.push((sname.clone(), false));
            prog.stages.push(StageDecl {
                span,
                name: sname,
                count,
                array: self.queues[qi].family,
                kind: StageKind::Delay,
            });
            Some(Expr::Var(lname))
        } else {
            None
        };
        // the policy is the relation's: written here, not left to a default
        if !self.eat_kw("share") {
            return self.err(format!(
                "`{puller} pull {source}` names how concurrent reads divide the NICs: \
                 `share maxmin` or `share bottleneck`, found {}",
                self.peek()
            ));
        }
        let policy = if self.eat_kw("maxmin") {
            crate::ir::Share::MaxMin
        } else if self.eat_kw("bottleneck") {
            crate::ir::Share::Bottleneck
        } else {
            return self.err(format!(
                "`share` takes `maxmin` or `bottleneck`, found {}",
                self.peek()
            ));
        };
        self.expect(&Tok::Semi)?;
        match prog.share {
            Some(p) if p != policy => {
                return self.err_at(
                    at,
                    "the program divides shared stages by one policy: every relation and \
                     `share` names the same",
                );
            }
            Some(_) if !self.relation_share => {
                return self.err_at(
                    at,
                    "`share` is declared on its own and on the relation: the relation names it",
                );
            }
            _ => {}
        }
        prog.share = Some(policy);
        self.relation_share = true;
        self.share_at.get_or_insert(at);
        self.queues[qi].pulls = Some((source, latency));
        Ok(())
    }

    /// `prefill S;`, `transfer[j] X from P to Q (n);`, `decode on E (D)
    /// growing kv;`, ...: a `run` on the stage that plays the role.
    fn serving(&mut self, role: Role) -> PResult<Vec<Stmt>> {
        let at = self.pos;
        let span = Some(self.span());
        self.advance();
        let kw = role.keyword();
        let mut also = vec![];
        // a read the queue's relation says: over the source's NIC and its own
        let pulled = role == Role::Transfer
            && *self.peek() == Tok::LParen
            && self.entry_from.is_some()
            && self.in_queue.is_some_and(|qi| self.queues[qi].nic);
        let stage = if pulled {
            Ref {
                span,
                name: queue::PULL.into(),
                index: None,
            }
        } else if self.eat_kw("on") {
            // `on a, b`: the stages the job holds at once, each declared
            let first = self.declared_stage(kw)?;
            while *self.peek() == Tok::Comma {
                self.advance();
                also.push(self.declared_stage(kw)?);
            }
            first
        } else {
            let index = if *self.peek() == Tok::LBracket {
                self.advance();
                let e = self.expr()?;
                self.expect(&Tok::RBracket)?;
                Some(Box::new(e))
            } else {
                None
            };
            let name = self.role_stage(role, at)?;
            Ref { span, name, index }
        };
        let is_step = self
            .stages
            .iter()
            .any(|(n, step)| *n == stage.name && *step);
        let mode = match role.step_mode() {
            Some(m) if is_step => m,
            _ => RunMode::Plain,
        };
        let work = self.expr()?;
        let growing = if self.eat_kw("growing") {
            Some(self.own_pool("growing")?)
        } else {
            None
        };
        // `transfer (w) from P to Q (n);`: the KV of `n` tokens moves from
        // the session's hold on `P` to its hold on `Q` over the link. Sugar
        // for `run link (w); load Q (n); release P;` - the link takes the
        // time, the tokens count as computed at `Q`, and `P` is free.
        if role == Role::Transfer && self.eat_kw("from") {
            if growing.is_some() {
                return self.err_at(
                    at,
                    "`transfer … from P to Q`: a transfer does not grow a pool",
                );
            }
            let f_at = self.pos;
            let from = self.own_pool("transfer … from")?;
            // the read takes the source's lease: the entry's `from` name
            if pulled
                && (from.index.is_some() || self.entry_from.as_deref() != Some(from.name.as_str()))
            {
                return self.err_at(
                    f_at,
                    format!(
                        "`transfer … from {}`: a read takes from the entry's source, `from {}`",
                        from.name,
                        self.entry_from.as_deref().unwrap_or("?")
                    ),
                );
            }
            self.expect_kw("to")?;
            let to = self.own_pool("transfer … to")?;
            let units = self.paren_expr()?;
            self.expect(&Tok::Semi)?;
            // each link with a latency is waited at first, in the order named
            let mut out: Vec<Stmt> = std::iter::once(&stage)
                .chain(also.iter())
                .filter_map(|r| {
                    let q = self.queues.iter().find(|q| q.name == r.name)?;
                    Some(Stmt::Run {
                        stage: Ref {
                            span: r.span,
                            name: format!("{}.latency", q.name),
                            index: r.index.clone(),
                        },
                        mode: RunMode::Plain,
                        work: q.latency.clone()?,
                        growing: None,
                        also: vec![],
                    })
                })
                .collect();
            out.extend([
                Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing: None,
                    also,
                },
                Stmt::Load(to, units),
                Stmt::Release(from),
            ]);
            return Ok(out);
        }
        // A KV transfer leaves one hold and enters another; a link that
        // only takes time is the kernel's `run`, and says so.
        if role == Role::Transfer {
            let at_stage = if stage.index.is_some() {
                format!("{}[…]", stage.name)
            } else {
                stage.name.clone()
            };
            return self.err_at(
                at,
                format!(
                    "`transfer` without `from P to Q (n)`: a KV transfer leaves the lease (or hold) on P and enters the hold on Q\n\
                     help: write `transfer (w) from P to Q (n);`, or `run {at_stage} (w);` for a link that only takes time"
                ),
            );
        }
        self.expect(&Tok::Semi)?;
        Ok(vec![Stmt::Run {
            stage,
            mode,
            work,
            growing,
            also,
        }])
    }

    /// A stage named after `on`, which must be declared above.
    fn declared_stage(&mut self, kw: &str) -> PResult<Ref> {
        let ref_at = self.pos;
        let r = self.reference()?;
        if !self.stages.iter().any(|(n, _)| *n == r.name) {
            let help = crate::frontend::diagnostic::suggestion(
                &r.name,
                self.stages.iter().map(|(name, _)| name.as_str()),
            )
            .map(|name| format!("did you mean stage `{name}`?"))
            .unwrap_or_else(|| "declare the stage above this statement".into());
            return self.err_at(
                ref_at,
                format!(
                    "`{kw} on {}`: no stage `{}` is declared above\nhelp: {help}",
                    r.name, r.name
                ),
            );
        }
        Ok(r)
    }

    /// `run a, b (w)`: the stages after the first, if any.
    fn more_stages(&mut self) -> PResult<Vec<Ref>> {
        let mut also = vec![];
        while *self.peek() == Tok::Comma {
            self.advance();
            also.push(self.reference()?);
        }
        Ok(also)
    }

    /// The stage a role names when none is given: the stage of the role's
    /// name, else (for `prefill` and `decode`) the step engine; exactly one.
    fn role_stage(&self, role: Role, at: usize) -> PResult<String> {
        let kw = role.keyword();
        let mut found: Vec<&str> = self
            .stages
            .iter()
            .filter(|(n, _)| role.names().contains(&n.as_str()))
            .map(|(n, _)| n.as_str())
            .collect();
        if found.is_empty() && role.step_mode().is_some() {
            found = self
                .stages
                .iter()
                .filter(|(_, step)| *step)
                .map(|(n, _)| n.as_str())
                .collect();
        }
        match found.len() {
            1 => Ok(found[0].to_string()),
            0 => {
                let names = role
                    .names()
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                let engine = if role.step_mode().is_some() {
                    ", declare a `step` engine"
                } else {
                    ""
                };
                self.err_at(
                    at,
                    format!(
                        "no stage declared above plays `{kw}`: name a stage {names}{engine}, or write `{kw} on STAGE (...)`"
                    ),
                )
            }
            _ => self.err_at(
                at,
                format!(
                    "several stages play `{kw}` ({}): write `{kw} on STAGE (...)`",
                    found.join(", ")
                ),
            ),
        }
    }

    // ---------------------------------------------------------- expressions

    fn expr(&mut self) -> PResult<Expr> {
        let c = self.or()?;
        if *self.peek() == Tok::Question {
            self.advance();
            let a = self.expr()?;
            self.expect(&Tok::Colon)?;
            let b = self.expr()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn or(&mut self) -> PResult<Expr> {
        let mut l = self.and()?;
        while *self.peek() == Tok::OrOr {
            self.advance();
            let r = self.and()?;
            l = Expr::Binary(BinOp::Or, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn and(&mut self) -> PResult<Expr> {
        let mut l = self.cmp()?;
        while *self.peek() == Tok::AndAnd {
            self.advance();
            let r = self.cmp()?;
            l = Expr::Binary(BinOp::And, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn cmp(&mut self) -> PResult<Expr> {
        let l = self.add()?;
        let op = match self.peek() {
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            Tok::EqEq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            _ => return Ok(l),
        };
        self.advance();
        let r = self.add()?;
        Ok(Expr::Binary(op, Box::new(l), Box::new(r)))
    }

    fn add(&mut self) -> PResult<Expr> {
        let mut l = self.mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(l),
            };
            self.advance();
            let r = self.mul()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
    }

    fn mul(&mut self) -> PResult<Expr> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => return Ok(l),
            };
            self.advance();
            let r = self.unary()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
    }

    fn unary(&mut self) -> PResult<Expr> {
        match self.peek() {
            Tok::Minus => {
                self.advance();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)))
            }
            Tok::Not => {
                self.advance();
                Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)))
            }
            Tok::Tilde => {
                self.advance();
                let span = self.span();
                let name = self.ident()?;
                self.expect(&Tok::LParen)?;
                let mut args = vec![];
                if *self.peek() != Tok::RParen {
                    args.push(self.expr()?);
                    while *self.peek() == Tok::Comma {
                        self.advance();
                        args.push(self.expr()?);
                    }
                }
                self.expect(&Tok::RParen)?;
                Ok(Expr::Located(span, Box::new(Expr::Sample(name, args))))
            }
            _ => self.pow(),
        }
    }

    fn pow(&mut self) -> PResult<Expr> {
        let base = self.atom()?;
        if *self.peek() == Tok::Caret {
            self.advance();
            let e = self.unary()?;
            return Ok(Expr::Binary(BinOp::Pow, Box::new(base), Box::new(e)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> PResult<Expr> {
        if let Some(i) = self.use_of_def() {
            if self.defs[i].stmts {
                return self.err(format!(
                    "`{}` is statements (`def {0}(…) {{ … }}`), not an expression",
                    self.defs[i].name
                ));
            }
            self.expand(i)?;
        }
        let at = self.pos;
        let span = self.span();
        match self.advance() {
            Tok::Num(x) => Ok(Expr::Num(x)),
            Tok::LParen => {
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Tok::Ident(name)
                if Agg::from_name(&name).is_some()
                    && matches!(self.peek(), Tok::Ident(_))
                    && matches!(self.peek_at(1), Tok::Ident(k) if k == "in") =>
            {
                let agg = Agg::from_name(&name).expect("guarded");
                let var = self.ident()?;
                self.advance(); // in
                // the count is a number, a name or a parenthesised expression
                // (what a `def` argument becomes): `ND (` would read as a call
                let cspan = self.span();
                let count = match self.advance() {
                    Tok::Num(x) => Expr::Num(x),
                    Tok::Ident(n) => Expr::Located(cspan, Box::new(Expr::Var(n))),
                    Tok::LParen => {
                        let e = self.expr()?;
                        self.expect(&Tok::RParen)?;
                        e
                    }
                    other => {
                        return self.err(format!(
                            "`{name} {var} in` takes a number, a constant's name or `( expr )`, found {other}"
                        ));
                    }
                };
                self.expect(&Tok::LParen)?;
                let body = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(Expr::Located(
                    span,
                    Box::new(Expr::Over(agg, var, Box::new(count), Box::new(body))),
                ))
            }
            Tok::Ident(name) => {
                if *self.peek() == Tok::LParen {
                    self.advance();
                    let mut args = vec![];
                    if *self.peek() != Tok::RParen {
                        args.push(self.arg()?);
                        while *self.peek() == Tok::Comma {
                            self.advance();
                            args.push(self.arg()?);
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    Ok(Expr::Located(span, Box::new(Expr::Call(name, args))))
                } else if *self.peek() == Tok::Dot
                    || (*self.peek() == Tok::LBracket && self.in_expr_index_dot())
                {
                    // `Q.x`: a moment an entry of `Q` marked, or what it set;
                    // the attribute is the request's, so no member index
                    let at = self.pos - 1;
                    if *self.peek() == Tok::LBracket {
                        return self.err_at(
                            at,
                            "`Q[…].x` reads the request's attribute, which is no member's: write `Q.x`",
                        );
                    }
                    self.expect(&Tok::Dot)?;
                    let field = self.ident()?;
                    let name = format!("{name}.{field}");
                    self.dotted_reads.push((at, name.clone()));
                    Ok(Expr::Located(span, Box::new(Expr::Var(name))))
                } else {
                    if name == "self" && self.in_queue.is_none() {
                        self.pos -= 1;
                        return self.err("`self` is a queue entry's word: the member's own index");
                    }
                    Ok(Expr::Located(span, Box::new(Expr::Var(name))))
                }
            }
            other => {
                self.pos = at;
                self.err(format!("expected an expression, found {other}"))
            }
        }
    }

    /// A call argument. `name` followed by `[`, `,` or `)` and not
    /// otherwise an operator is parsed as a reference; the linker decides
    /// whether it names a pool, a stage or a variable.
    fn arg(&mut self) -> PResult<Arg> {
        let span = Some(self.span());
        if let Tok::Ident(name) = self.peek().clone() {
            match self.peek_at(1) {
                Tok::Comma | Tok::RParen => {
                    self.advance();
                    return Ok(Arg::Ref(Ref {
                        span,
                        name,
                        index: None,
                    }));
                }
                // `kv[j]`, `D[j].kv`, `D.kv`: a reference, dotted when a queue's,
                // when nothing but `,` or `)` follows it
                Tok::LBracket | Tok::Dot if self.ref_ends_arg() => {
                    return Ok(Arg::Ref(self.reference()?));
                }
                _ => {}
            }
        }
        Ok(Arg::Expr(self.expr()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_program() {
        let src = r#"
            let a = 2e-5;
            pool kv { cap 3e5; evict lru; preempt lifo; }
            stage prefill : fifo;
            stage decode : ps(min(present, 8));
            stage tool : delay;
            workload {
              arrive poisson(0.3);
              init { set K = 0; set n = ~uniform(1e4, 3e4); }
              turn { set K = K + n + o; set n = ~exp(1000); }
            }
            session {
              turn;
              loop {
                hold kv (K + n + o) {
                  run prefill (a * (K + n - cached));
                  observe ttft = now - t0;
                  run decode (o * 2e-4);
                } cache (K + n + o);
                branch with (0.9) { run tool (Z); turn; } else { end; }
              }
            }
            run { horizon 1000; warmup 100; seed 1; }
        "#;
        let p = parse(src).unwrap();
        assert_eq!(p.pools.len(), 1);
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.lets[0].0, "a");
        assert!(matches!(p.session[1], Stmt::Loop(_)));
    }

    #[test]
    fn precedence_and_refs() {
        let e = parse_expr("1 + 2 * 3 ^ 2 < 20 && work(prefill[j]) > 0").unwrap();
        assert!(matches!(e, Expr::Binary(BinOp::And, _, _)));
        let e = parse_expr("-x ? a : b").unwrap();
        assert!(matches!(e, Expr::Cond(..)));
    }

    const PD: &str = r#"
        pool kv { cap 100; }
        pool kvD { cap 100; }
        stage prefill : fifo;
        stage link : ps(1);
        stage decode : ps(present);
        stage tool : delay;
    "#;

    const ENGINE: &str = r#"
        pool kv { cap 100; }
        stage engine : step { cost 1; }
        stage tool : delay;
    "#;

    /// The same program runs: the same session. What one request runs, kept
    /// for the deployment view, is not compared: one spelling splits the
    /// session into a workload and a server, and the other does not.
    fn same(a: &str, b: &str) {
        let run = |s: &str| Program {
            request: vec![],
            ..without_locations(parse(s).unwrap())
        };
        assert_eq!(run(a), run(b));
    }

    #[test]
    fn serving_forms_desugar_to_the_kernel() {
        same(
            &format!(
                "{PD} session {{
                    hold kv (K) {{ prefill S; }} cache (K) lease kv (inf);
                    hold kvD (K) {{ transfer X from kv to kvD (K); }}
                    hold kv (K) reserve (F) reuse (R) {{ decode D; }}
                    branch with (p) {{ tool Z; turn; }} else {{ end; }}
                }}"
            ),
            &format!(
                "{PD} session {{
                    hold kv (K) {{ run prefill (S); }} cache (K) lease kv (inf);
                    hold kvD (K) {{ run link (X); load kvD (K); release kv; }}
                    hold kv (K) reserve (F) reuse (R) {{ run decode (D); }}
                    branch with (p) {{ run tool (Z); turn; }} else {{ end; }}
                }}"
            ),
        );
    }

    #[test]
    fn serving_forms_on_a_step_engine() {
        same(
            &format!(
                "{ENGINE} session {{
                    hold kv (c) {{ prefill (n) growing kv; decode (o - 1) growing kv; }} cache (c);
                    tool (~exp(Z));
                }}"
            ),
            &format!(
                "{ENGINE} session {{
                    hold kv (c) {{ run engine prefill (n) growing kv; run engine decode (o - 1) growing kv; }} cache (c);
                    run tool (~exp(Z));
                }}"
            ),
        );
    }

    #[test]
    fn serving_forms_name_their_stage_explicitly() {
        // the role's stage array, indexed
        same(
            "stage prefill[2] : fifo; session { choose j in 2 by (work(prefill[j])); prefill[j] S; }",
            "stage prefill[2] : fifo; session { choose j in 2 by (work(prefill[j])); run prefill[j] (S); }",
        );
        // any stage, with the mode a step engine needs
        same(
            &format!(
                "{ENGINE} stage rep[2] : fifo; session {{ prefill on rep[1] S; decode on engine (D); }}"
            ),
            &format!(
                "{ENGINE} stage rep[2] : fifo; session {{ run rep[1] (S); run engine decode (D); }}"
            ),
        );
        // a stage named `transfer` plays transfer
        same(
            "pool a { cap 1; } pool b { cap 1; } stage transfer : fifo;
             session { hold b (1) { hold a (1) { transfer X from a to b (1); } } }",
            "pool a { cap 1; } pool b { cap 1; } stage transfer : fifo;
             session { hold b (1) { hold a (1) { run transfer (X); load b (1); release a; } } }",
        );
    }

    #[test]
    fn a_binding_the_body_reads_is_set_at_the_top_of_the_body() {
        // the body sees `known` at its admission value, which for attributes
        // and constants is the value it has at the top of the body
        same(
            &format!(
                "{ENGINE} session {{
                    hold kv (known) at admission (known = computed < p ? p : computed + 1) {{
                        prefill (known - cached) growing kv;
                    }}
                    end;
                }}"
            ),
            &format!(
                "{ENGINE} session {{
                    hold kv (computed < p ? p : computed + 1) {{
                        set known = computed < p ? p : computed + 1;
                        run engine prefill (known - cached) growing kv;
                    }}
                    end;
                }}"
            ),
        );
        // a binding the body does not read stays in the header
        same(
            &format!("{ENGINE} session {{ hold kv (h) at admission (h = 1) {{ }} }}"),
            &format!("{ENGINE} session {{ hold kv (1) {{ }} }}"),
        );
    }

    #[test]
    fn the_body_does_not_see_a_binding_of_live_state() {
        // at the top of the body the admission has consumed the prefix, so
        // `cachedin(kv)` there is not what the header read
        for (binding, help) in [
            ("hit = min(cachedin(kv), p)", "read `cached`"),
            ("hit = cached", "read `cached`"),
            ("hit = now", "with a `set` in the body"),
        ] {
            let e = parse(&format!(
                "{ENGINE} session {{ hold kv (hit) at admission ({binding}) {{ observe h = hit; }} }}"
            ))
            .unwrap_err();
            assert!(
                e.msg.contains("the body reads `hit`, and `hit` reads"),
                "{binding}: {}",
                e.msg
            );
            assert!(e.msg.contains(help), "{}", e.msg);
        }
        // `n` is a context variable unless the program assigns it
        let src = format!(
            "{ENGINE} session {{ set n = 3; hold kv (m) at admission (m = n + 1) {{ observe x = m; }} }}"
        );
        parse(&src).unwrap();
    }

    #[test]
    fn a_binding_the_body_reads_has_a_name_of_its_own() {
        // its `set` makes the name an attribute: one the scheduler, a `let`,
        // the context or the program already has would be two things
        for (pre, binding, what) in [
            ("", "cached = 3", "an attribute the scheduler sets"),
            ("", "computed = 50", "an attribute the scheduler sets"),
            ("", "size = 5", "a name the language supplies"),
            ("", "now = 1", "a name the language supplies"),
            ("", "inf = 3", "a name the language supplies"),
            ("", "kv = 3", "a pool"),
            ("", "engine = 3", "a stage"),
            ("let bs = 4;", "bs = 2", "a `let` constant"),
            ("", "k = 7", "an attribute the program sets"),
        ] {
            let assign = if what.ends_with("program sets") {
                "set k = 5;"
            } else {
                ""
            };
            let name = binding.split(' ').next().unwrap();
            let e = parse(&format!(
                "{pre} {ENGINE} session {{ {assign} hold kv (1) at admission ({binding}) {{ observe a = {name}; }} }}"
            ))
            .unwrap_err();
            assert!(e.msg.contains(what), "{binding}: {}", e.msg);
        }
        let e = parse(&format!(
            "{ENGINE} session {{ hold kv (1) at admission (k = k + 1) {{ observe a = k; }} }}"
        ))
        .unwrap_err();
        assert!(e.msg.contains("reads itself"), "{}", e.msg);
        let e = parse(&format!(
            "{ENGINE} session {{ hold kv (1) at admission (j = k, k = 5) {{ observe a = j; }} }}"
        ))
        .unwrap_err();
        assert!(e.msg.contains("bound after it"), "{}", e.msg);
        // an attribute the body sets itself is not the binding
        let e = parse(&format!(
            "{ENGINE} session {{ hold kv (hit) at admission (hit = min(cachedin(kv), 1)) {{ set hit = cached; observe h = hit; }} }}"
        ))
        .unwrap_err();
        assert!(e.msg.contains("an attribute the program sets"), "{}", e.msg);
    }

    #[test]
    fn a_binding_is_read_only_in_its_hold() {
        for after in [
            "observe b = k;",
            "hold kv (k) { }",
            "branch (k > 1) { } else { }",
        ] {
            let e = parse(&format!(
                "{ENGINE} session {{ hold kv (1) at admission (k = 2) {{ observe a = k; }} {after} }}"
            ))
            .unwrap_err();
            assert!(e.msg.contains("outside the body"), "{after}: {}", e.msg);
        }
        let e = parse(
            "pool kv { cap 100; queue by (k); } stage engine : step { cost 1; }
             session { hold kv (1) at admission (k = 2) { observe a = k; } }",
        )
        .unwrap_err();
        assert!(e.msg.contains("outside the body"), "{}", e.msg);
        // two holds may bind one name, and a nested hold may bind it again
        // over a live outer binding its body does not read
        parse(&format!(
            "{ENGINE} session {{
                hold kv (1) at admission (k = 2) {{ observe a = k; }}
                hold kv (1) at admission (k = 3) {{ observe b = k; }}
                hold kv (h) at admission (h = cachedin(kv)) {{
                    hold kv (1) at admission (h = 1) {{ observe c = h; }}
                }}
            }}"
        ))
        .unwrap();
    }

    #[test]
    fn a_def_is_its_body_where_it_is_used() {
        // an expression: the argument in parentheses, the body too
        same(
            &format!(
                "{ENGINE} def full(x) = floor((x - 1) / bs) * bs;
                 session {{ set h = full(a + b) * 2; set g = min(full(k), 3); }}"
            ),
            &format!(
                "{ENGINE} session {{
                    set h = (floor(((a + b) - 1) / bs) * bs) * 2;
                    set g = min(floor((k - 1) / bs) * bs, 3);
                 }}"
            ),
        );
        // statements, with references for pools and stages; the use is
        // parsed where it stands, so a serving form finds its stage there
        same(
            "pool kv[2] { cap 100; } stage E[2] : step { cost 1; }
             def put(p, s, n) { hold p (n) { prefill on s (n) growing p; } cache (n); }
             session { choose j in 2 by (used(kv[j])); put(kv[j], E[j], k + 1); end; }",
            "pool kv[2] { cap 100; } stage E[2] : step { cost 1; }
             session {
                choose j in 2 by (used(kv[j]));
                hold kv[j] ((k + 1)) { run E[j] prefill ((k + 1)) growing kv[j]; } cache ((k + 1));
                end;
             }",
        );
        // the side is the use's: a `hold` in a server
        same(
            &format!(
                "{ENGINE} def take(n) {{ hold kv (n) {{ prefill (n) growing kv; }} }}
                 workload {{ session {{ request; end; }} }}
                 server {{ take(4); }}"
            ),
            &format!(
                "{ENGINE} workload {{ session {{ request; end; }} }}
                 server {{ hold kv (4) {{ prefill (4) growing kv; }} }}"
            ),
        );
    }

    #[test]
    fn a_def_says_what_goes_wrong() {
        let err = |src: &str| parse(&format!("{ENGINE} {src}")).unwrap_err().msg;
        assert!(err("def f(x) = x; session { f(1); }").contains("is an expression"));
        assert!(err("def f(x) { end; } session { set a = f(1); }").contains("is statements"));
        assert!(
            err("def f(x) = x; session { set a = f(1, 2); }")
                .contains("takes 1 argument(s), got 2")
        );
        assert!(
            err("def f(x) = x + x; session { set a = f(~exp(1)); }").contains("would draw 2 times")
        );
        assert!(err("def f(x) = f(x); session { }").contains("uses itself"));
        assert!(err("def min(x) = x; session { }").contains("a word of the language"));
        assert!(err("def uniform(x) = x; session { }").contains("a word of the language"));
        assert!(err("def f(on) = on; session { }").contains("a word of the language"));
        assert!(err("def f(min) = min(min, 1); session { }").contains("a word of the language"));
        // a definition uses only the ones before it: no recursion
        assert!(
            err("def g(x) = f(x); def f(x) = g(x); session { set a = g(1); }")
                .contains("`g` uses `f`, which is defined after it")
        );
        assert!(
            err("def g(x) { f(x); } def f(x) { g(x); } session { g(1); }")
                .contains("defined after it")
        );
        // a stray closer
        assert!(err("def f(x) = x; session { set a = f(1]); }").contains("unmatched"));
        // a definition that draws draws when it is an argument
        assert!(
            err("def d() = ~exp(1); def twice(x) = x + x; session { set a = twice(d()); }")
                .contains("would draw 2 times")
        );
        // an argument the body would capture
        assert!(
            err("def f(x) { set s = 10; observe o = x; } session { f(s + 1); }")
                .contains("which `f` assigns")
        );
        // an observation's name is not captured
        parse(&format!(
            "{ENGINE} def f(x) {{ observe s = 10; observe o = x; }} session {{ f(s + 1); }}"
        ))
        .unwrap();
        assert!(err("def f(p) { set p = 1; } session { f(2); }").contains("is a parameter"));
        assert!(
            err(
                "def f(h) { hold kv (h) at admission (h = 3) { observe a = h; } } session { f(2); }"
            )
            .contains("is a parameter")
        );
        // a parameter may not be an aggregate's index, and a count may be one
        assert!(
            err("def total(k) = sum k in 2 (k); session { set x = total(7); }")
                .contains("is a parameter")
        );
        parse("def total(n) = sum k in n (k); session { set x = total(2); }").unwrap();
        // a parenthesised count: the body's `k` is still the aggregate's
        parse(
            "def total() = sum k in (1 + 1) (k); def next(x) { turn; observe p = x; } \
               workload { turn { set k = 1; } } session { next(total()); end; }",
        )
        .unwrap();
        // an aggregate's index is its own, not a name the argument reads
        parse(
            "def total() = sum i in 2 (i); def next(x) { turn; observe p = x; } \
               workload { turn { set i = 1; } } session { next(total()); end; }",
        )
        .unwrap();
        // what a turn, a request or an admission assigns is captured too
        assert!(
            err("def next(x) { turn; observe p = x; } workload { turn { set n = 1; } } session { next(n); end; }")
                .contains("which its `turn;` assigns")
        );
        assert!(
            err("def go(x) { request; observe b = x; } workload { init { set t0 = 0; } session { go(t0); end; } } server { set t0 = now; }")
                .contains("which its `request;` assigns")
        );
        assert!(
            err("def take(x) { hold kv (4) { observe got = x; } } session { take(cached); }")
                .contains("which `take` assigns")
        );
        // the clock and live state are read where the body reads them
        assert!(
            err("stage svc : fifo; def timed(t) { run svc (1); observe took = now - t; } session { timed(now); }")
                .contains("reads `now`, which changes")
        );
        assert!(
            err("def f(q) { observe b = q; } session { f(used(kv)); }").contains("reads `used(…)`")
        );
        // an expression's argument is read where the expression is
        parse(&format!(
            "{ENGINE} def g(x) = x + 1; session {{ set a = g(now); }}"
        ))
        .unwrap();
        // `n` is an attribute when the program sets it
        parse(&format!(
            "{ENGINE} def f(x) {{ observe b = x; }} session {{ set n = 1; f(n); }}"
        ))
        .unwrap();
        // and through an expression the argument uses
        assert!(
            err("stage svc : fifo; def clock() = now; def timed(t) { run svc (1); observe took = now - t; } session { timed(clock()); }")
                .contains("reads `now`")
        );
        assert!(
            err("def occ(p) = used(p); def f(q) { observe b = q; } session { f(occ(kv)); }")
                .contains("reads `used(…)`")
        );
        assert!(
            err("def plus(x) = s + x; def f(v) { set s = 10; observe o = v; } session { f(plus(1)); }")
                .contains("which `f` assigns")
        );
        // and through a definition the body uses
        assert!(
            err("def reset() { set s = 10; } def f(x) { reset(); observe o = x; } session { f(s + 1); }")
                .contains("which `f` assigns")
        );
        assert!(
            err("def adv() { turn; } def next(x) { adv(); observe p = x; } workload { turn { set n = 1; } } session { next(n); end; }")
                .contains("which its `turn;` assigns")
        );
        assert!(
            err("def ask() { request; } def go(x) { ask(); observe b = x; } workload { session { go(cached); end; } } server { }")
                .contains("which its `request;` assigns")
        );
        // a name that is a declaration's
        assert!(err("def kv(x) = x; session { }").contains("also a pool"));
        assert!(err("def engine(x) = x; session { }").contains("also a stage"));
        // the name of a statement body's attribute is not a use
        parse(&format!(
            "{ENGINE} def c(x) {{ set c = x; }} session {{ c(1); }}"
        ))
        .unwrap();
        // an error in the body says where the definition was used
        let e =
            err("def take(n) { turn; } workload { session { request; end; } } server { take(4); }");
        assert!(e.contains("note: in `take`, used at"), "{e}");
        assert!(err("def f(x) = x; def f(y) = y; session { }").contains("defined twice"));
        // an argument used once may draw
        parse(&format!(
            "{ENGINE} def f(x) = x + 1; session {{ set a = f(~exp(1)); }}"
        ))
        .unwrap();
        // a def used before it is defined is a call of an unknown function,
        // which the linker reports
        parse(&format!(
            "{ENGINE} session {{ set a = f(1); }} def f(x) = x;"
        ))
        .unwrap();
    }

    #[test]
    fn a_transfer_says_where_the_kv_goes() {
        // without `from P to Q` it would be a link that stores and forwards,
        // which is the kernel's `run`, not a transfer
        let e = parse(&format!("{PD} session {{ hold kv (K) {{ transfer X; }} }}")).unwrap_err();
        assert!(
            e.msg.contains("`transfer` without `from P to Q (n)`"),
            "{e}"
        );
        assert!(e.msg.contains("`run link (w);`"), "{e}");
        let e = parse("stage link[2] : ps(1); session { transfer[0] X; }").unwrap_err();
        assert!(e.msg.contains("`run link[…] (w);`"), "{e}");
        let e = parse(
            "pool kv { cap 1; } stage nic : ps(1); session { transfer on nic X growing kv; }",
        )
        .unwrap_err();
        assert!(
            e.msg.contains("`transfer` without `from P to Q (n)`"),
            "{e}"
        );
        assert!(e.msg.contains("`run nic (w);`"), "{e}");
    }

    #[test]
    fn serving_forms_need_exactly_one_stage() {
        let e = parse("stage svc : fifo; session { prefill S; }").unwrap_err();
        assert!(
            e.msg.contains("no stage declared above plays `prefill`"),
            "{e}"
        );
        assert_eq!((e.line, e.col), (1, 29));
        let e =
            parse("stage a : step { cost 1; } stage b : step { cost 1; } session { decode D; }")
                .unwrap_err();
        assert!(e.msg.contains("several stages play `decode` (a, b)"), "{e}");
        let e = parse("stage engine : step { cost 1; } session { transfer X; }").unwrap_err();
        assert!(
            e.msg.contains("no stage declared above plays `transfer`"),
            "{e}"
        );
        let e = parse("stage tool : delay; session { tool on other Z; }").unwrap_err();
        assert!(e.msg.contains("no stage `other` is declared above"), "{e}");
    }

    const DEPLOYMENT: &str = r#"
        pool kv { cap 1000; block 16; evict lru; }
        pool reqs { cap 4; }
        stage engine : step { budget 64; cost 1; memory kv; }
        stage tool : delay;
    "#;

    const CLIENT: &str =
        "arrive poisson(1); init { set K = 0; } turn { set n = 10; set o = 5; set more = 1; }";

    /// `workload { session { … request; … } }` and `server { … }` parse to
    /// the session block that has the server in place of the request.
    #[test]
    fn the_two_sides_are_one_session() {
        same(
            &format!(
                "{DEPLOYMENT} workload {{ {CLIENT}
                    session {{
                      turn;
                      loop {{
                        request;
                        set K = prompt + o;
                        branch (more) {{ tool 3; turn; }} else {{ end; }}
                      }}
                    }}
                }}
                server {{
                  set prompt = K + n;
                  hold reqs (1), kv (min(prompt, hit + budget_left(engine)))
                        at admission (hit = min(cachedin(kv), prompt - 1)) {{
                    prefill (prompt - cached) growing kv;
                    decode (o - 1) growing kv;
                  }} cache (prompt + o);
                }}"
            ),
            &format!(
                "{DEPLOYMENT} workload {{ {CLIENT} }}
                session {{
                  turn;
                  loop {{
                    set prompt = K + n;
                    hold reqs (1), kv (min(prompt, hit + budget_left(engine)))
                          at admission (hit = min(cachedin(kv), prompt - 1)) {{
                      prefill (prompt - cached) growing kv;
                      decode (o - 1) growing kv;
                    }} cache (prompt + o);
                    set K = prompt + o;
                    branch (more) {{ tool 3; turn; }} else {{ end; }}
                  }}
                }}"
            ),
        );
    }

    #[test]
    fn request_is_spliced_at_any_depth_and_as_often_as_written() {
        same(
            "stage s : fifo; workload { session { branch (x) { request; } else { loop { request; end; } } } }
             server { run s (1); }",
            "stage s : fifo; workload { } session { branch (x) { run s (1); } else { loop { run s (1); end; } } }",
        );
        // the kernel is written on either side
        same(
            "pool kv { cap 1; } workload { session { request; end; } } server { hold kv (1) { } }",
            "pool kv { cap 1; } workload { } session { hold kv (1) { } end; }",
        );
        // the order of the blocks does not matter
        same(
            "stage s : fifo; server { run s (1); } workload { session { request; end; } }",
            "stage s : fifo; workload { } session { run s (1); end; }",
        );
    }

    fn refused(src: &str, needle: &str) {
        let e = parse(src).unwrap_err();
        assert!(e.msg.contains(needle), "{src}\n  {e}");
    }

    #[test]
    fn each_side_keeps_its_words() {
        const WL: &str = "workload { session { request; } }";
        // the session's words in a server
        refused(
            &format!("stage s : fifo; {WL} server {{ turn; }}"),
            "`turn` is the session's",
        );
        refused(
            &format!("stage s : fifo; {WL} server {{ end; }}"),
            "`end` is the session's",
        );
        refused(
            &format!("stage s : fifo; {WL} server {{ request; }}"),
            "does not request itself",
        );
        refused(
            "pool kv { cap 1; } session { request; }",
            "`session` inside `workload`",
        );
    }

    #[test]
    fn one_admission_is_written_one_way() {
        // the words #136 took out say what a program writes instead
        const WL: &str = "workload { session { request; } }";
        for (src, now) in [
            ("session { enter kv (1) { } }", "`enter` is now `hold`"),
            (
                &*format!("{WL} server {{ admit if kv (1) fit {{ }} }}"),
                "is now `hold … at admission",
            ),
            (
                "session { hold kv (1) { } keep (1); }",
                "`keep` is now `cache`",
            ),
            (
                "session { hold kv (1) where x = 1 { } }",
                "`where x = e` is now `at admission (x = e)`",
            ),
            ("session { hold kv (1) fit { } }", "`fit` is gone"),
        ] {
            refused(&format!("pool kv {{ cap 1; }} {src}"), now);
        }
        // and none of them names anything, so a name never means two things
        for src in [
            "def keep(n) { observe k = n; } session { end; }",
            "def f(where) = where; session { end; }",
            "session { set fit = 1; end; }",
            "session { hold kv (1) at admission (enter = 1) { observe e = enter; } end; }",
        ] {
            refused(&format!("pool kv {{ cap 1; }} {src}"), "is a retired word");
        }
        // `hold` is written on either side, with its bindings
        parse(&format!(
            "pool kv {{ cap 1; }} {WL} server {{ hold kv (x) at admission (x = 1) {{ }} cache (1); }}"
        ))
        .unwrap();
    }

    #[test]
    fn a_side_needs_the_other() {
        refused(
            "stage s : fifo; workload { session { run s (1); } }",
            "written against a `server` block",
        );
        refused(
            "stage s : fifo; server { run s (1); }",
            "`server` needs a `session` inside `workload`",
        );
        refused(
            "stage s : fifo; workload { session { run s (1); } } server { run s (1); }",
            "never requested",
        );
        refused(
            "stage s : fifo; workload { session { request; } } server { run s (1); } session { run s (1); }",
            "one session",
        );
        refused(
            "stage s : fifo; session { run s (1); } workload { session { request; } } server { run s (1); }",
            "one session",
        );
        refused(
            "stage s : fifo; server { run s (1); } server { run s (1); }",
            "duplicate server",
        );
    }
}
