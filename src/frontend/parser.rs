//! Recursive-descent parser for serQ programs.
//!
//! ```text
//! program  := (let | def | use)* 'fn' 'main' '(' ')' '{' item* '}'
//! item     := 'let' IDENT '=' expr ';'
//!           | 'pool' IDENT ('[' NUM ']')? '{' poolopt* '}'
//!           | 'stage' IDENT ('[' NUM ']')? ':' kind ';'
//!           | 'workload' '{' wlitem* '}'
//!           | 'server' block
//!           | 'queue' IDENT ('[' expr ']')? (':' IDENT (',' IDENT)*)? '{' qitem* '}'
//!           | 'gauge' IDENT '=' expr ';'      -- a time average of the deployment's state
//!           | 'claim' IDENT ('given' '(' expr ')')? ':'
//!                 ('every' | 'some') 'iteration' 'of' ref '(' expr ')' ';'
//!           | 'claim' IDENT ('given' '(' expr ')')? ':' 'at' 'end' '(' expr ')' ';'
//! qitem    := 'pool' IDENT '{' poolopt* '}' | 'serve' kind
//!           | IDENT ('(' IDENT (',' IDENT)* ')')? ('from' IDENT)? block   -- an entry (crate::frontend::queue)
//! poolopt  := 'cap' expr ';' | 'block' expr ';'
//!           | 'evict' ('lru' | 'by' '(' expr (',' expr)* ')') ';'
//!           | 'preempt' ('lifo' | 'none' | 'by' '(' expr (',' expr)* ')' ('requeue' ('head' | 'tail'))?) ';'
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
//!           | 'session' block                  -- optional sequence of completed turns
//!           | 'hidden' IDENT (',' IDENT)* ';'
//! block    := '{' stmt* '}'
//! stmt     := 'turn' ';' | 'set' IDENT '=' expr ';' | 'observe' IDENT '=' expr ';'
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
//! serving  := 'tool' ('[' expr ']' | 'on' ref)? expr ';'
//!           | 'transfer' ('[' expr ']' | 'on' ref)? expr 'from' ref 'to' ref '(' expr ')' ';'
//! ref      := IDENT ('[' expr ']')?
//! atom     := NUM | '(' expr ')' | IDENT | IDENT '(' arg (',' arg)* ')' | over
//! over     := ('max' | 'min' | 'sum') IDENT 'in' (NUM | IDENT | '(' expr ')') '(' expr ')'
//! ```
//!
//! The serving forms (`serving`) are sugar: they are rewritten to `hold`
//! and `run` here, so the AST, the IR and the interpreter know only the
//! kernel. A role finds its stage among the stages declared above the
//! statement: the stage of the role's name (`link` or `transfer`, `tool`);
//! `on STAGE` names it explicitly. The run is plain. Prefill and decode are
//! not roles: they are the mode of a `run` on a step engine.
//!
//! A workload's optional session describes complete turns and their continuation.
//! Each source `turn;` becomes an attribute draw followed by the one server's
//! statements. Omitting the session means one turn. Queue routing belongs to
//! the server; lifecycle (`turn`, early `end`) belongs to the session.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::frontend::ast::*;
use crate::frontend::diagnostic::Source;
use crate::frontend::lexer::{LexError, Tok, Token, lex};
use crate::frontend::link::{AGGREGATES, BUILTIN_ATTRS, CONTEXT_VARS, FOLDED, FUNCTIONS};
use crate::frontend::queue::{self, QueueDecl};

mod device;

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
        // an error about an override (a `def`'s body) has no place in the text
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
    args_imported: bool,
    supplied_inputs: Vec<String>,
    toks: Vec<Token>,
    pos: usize,
    /// The stages declared so far: what the serving forms resolve their
    /// stage against.
    stages: Vec<String>,
    definitions: Vec<(String, Span)>,
    /// Composite costs lower to named scalar fields in declaration order.
    cost_records: std::collections::BTreeMap<String, Vec<String>>,
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
    /// Uses whose arguments read names a `turn;` in the body
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
    /// The `def` overrides: the body an expression definition has instead of
    /// its own, and whether the program defined it.
    def_overrides: Vec<(String, String, bool)>,
    /// The queues declared so far; their entries are expanded in `assemble`.
    queues: Vec<QueueDecl>,
    /// `let` constants with a constant value, for a family's size.
    consts: Vec<(String, f64)>,
    /// Constants whose values a `let` override can change, including dependents.
    structural_overrides: Vec<String>,
    /// The queue whose entry is being parsed.
    in_queue: Option<usize>,
    /// The `from` name of the entry being parsed.
    entry_from: Option<String>,
    entry_gateway: bool,
    /// The verb of the relation that gave the program's `share`.
    relation_share: Option<&'static str>,
    /// The queues whose relation gives the wait before each copy they post.
    posters: Vec<String>,
    /// Parsing the `serve` of a link queue, which may take a `latency`.
    latency_ok: bool,
    /// `Q.x` read as an expression, with the position: a mark or a pool of `Q`.
    dotted_reads: Vec<(usize, String, Option<String>)>,
    /// `Q[i].x` references, checked once the queues are known: `x` must be
    /// a pool of `Q`.
    indexed_dotted: Vec<(usize, String)>,
    /// `device`s, `engine … on` them and `pool … on` either, linked once
    /// the program is read (`device::link_engines`).
    devices: Vec<device::DeviceDecl>,
    engines: Vec<device::EngineDecl>,
    pools_on: Vec<device::PoolOn>,
    /// Parsing an engine's `schedule`, where `running.…` and `waiting.…`
    /// are read.
    in_schedule: bool,
    /// The queue whose body is being read: its `device gpu` is `Q.gpu`.
    device_scope: Option<String>,
}

/// A use of a `def` whose body says `turn;`, with the names
/// its arguments read.
struct Deferred {
    name: String,
    line: usize,
    col: usize,
    file: usize,
    reads: Vec<String>,
    turn: bool,
}

/// The attributes one completed turn may change in the serving path.
#[derive(Default)]
struct Served {
    shared: Vec<String>,
    server: Vec<String>,
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

/// `def name(x, y) { e }` or `def name(x, y) { statements }`: a name for
/// source a program would otherwise repeat. A use is replaced by the body's
/// tokens, each parameter by its argument's, and parsed where it stands, so
/// the AST, the IR and everything after know nothing of it.
#[derive(Clone)]
struct Def {
    name: String,
    params: Vec<String>,
    /// A single expression or statements, both enclosed in braces.
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
pub const KEYWORDS: [&str; 113] = [
    "Cost",
    "Size",
    "admission",
    "admit",
    "advance",
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
    "claim",
    "closed",
    "cost",
    "decode",
    "def",
    "delay",
    "device",
    "drop",
    "else",
    "end",
    "engine",
    "every",
    "evict",
    "exclusive",
    "execute",
    "fifo",
    "first",
    "fits",
    "fn",
    "fork",
    "from",
    "gauge",
    "given",
    "granule",
    "grow",
    "growing",
    "head",
    "held",
    "hidden",
    "hold",
    "horizon",
    "in",
    "init",
    "iteration",
    "join",
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
    "of",
    "on",
    "only",
    "ordered",
    "poisson",
    "pool",
    "preempt",
    "prefill",
    "ps",
    "pull",
    "push",
    "queue",
    "release",
    "renewal",
    "request",
    "requeue",
    "reserve",
    "reuse",
    "run",
    "schedule",
    "seed",
    "serve",
    "server",
    "session",
    "set",
    "share",
    "some",
    "spill",
    "stage",
    "state",
    "step",
    "sum",
    "tail",
    "to",
    "tool",
    "trace",
    "transfer",
    "turn",
    "use",
    "via",
    "warmup",
    "when",
    "while",
    "with",
    "workload",
];

/// Where a statement sits: declarations, the workload session, or a server.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Session,
    WorkloadSession,
    Server,
}

/// A serving form: a statement that desugars to `run` on the stage that
/// plays the role. Prefill and decode are not among them: on a step engine
/// they are the run's mode, written `run E prefill (…)`.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Transfer,
    Tool,
}

impl Role {
    fn of(kw: &str) -> Option<Role> {
        match kw {
            "transfer" => Some(Role::Transfer),
            "tool" => Some(Role::Tool),
            _ => None,
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            Role::Transfer => "transfer",
            Role::Tool => "tool",
        }
    }

    /// The stage names that play the role by default.
    fn names(self) -> &'static [&'static str] {
        match self {
            Role::Transfer => &["link", "transfer"],
            Role::Tool => &["tool"],
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

/// `parse_at`, with the bodies `def` overrides give expression definitions
/// and the constants `let` overrides replace, which may not size a queue family.
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
    p.supplied_inputs = lets.to_vec();
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
                "unknown `def` override `{name}`\nhelp: an override replaces the body of a declared `def NAME(...) {{ expr }}`; \
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

/// Parse an instance file (`--instance`): the values of a program's
/// constants, as `let` bindings, and its run options, as a `run` block.
pub fn parse_instance(src: &str) -> PResult<(Vec<(String, Expr)>, RunOpts)> {
    let toks = lex(src)?;
    Parser::new(toks).instance()
}

/// Parse a standalone expression (an override's: `--set name=expr`, `sets=`).
pub fn parse_expr(src: &str) -> PResult<Expr> {
    let toks = lex(src)?;
    let mut p = Parser::new(toks);
    let e = p.expr()?;
    p.expect(&Tok::Eof)?;
    Ok(e)
}

/// The names `set` or `choose` assigns anywhere in `stmts`.
pub(crate) fn assigned_in(stmts: &[Stmt], out: &mut Vec<String>) {
    for s in stmts {
        match s {
            Stmt::Set(n, _) | Stmt::Choose { var: n, .. } => out.push(n.clone()),
            Stmt::Hold { body, .. }
            | Stmt::Loop(body)
            | Stmt::While(_, body)
            | Stmt::Fork(body) => assigned_in(body, out),
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

/// A function of the language: one the linker resolves or one it folds.
fn is_function(name: &str) -> bool {
    FUNCTIONS.contains(&name)
        || FOLDED.contains(&name)
        || AGGREGATES.iter().any(|(n, _)| *n == name)
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
        Stmt::Declare(..) | Stmt::Side(_) | Stmt::Turn | Stmt::Request | Stmt::End | Stmt::Join => {
            false
        }
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
        Stmt::While(e, b) => expr_reads(e, n) || block(b),
        Stmt::Loop(b) | Stmt::Fork(b) => block(b),
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
            Stmt::While(e, b) => {
                outside(s, &|_, n| expr_reads(e, n)).or_else(|| stray_read(b, bound, scope))
            }
            Stmt::Loop(b) | Stmt::Fork(b) => stray_read(b, bound, scope),
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

/// Find turn sites, wrapped in their session-wide holds, for the deployment view.
/// The internal `Request` marker is inserted after each source turn and is
/// replaced by the server before the linker sees the AST.
fn request_sites(stmts: &[Stmt], holds: &[&Stmt], out: &mut Vec<Stmt>) {
    // All turns use the same server. Two sites have the same serving path
    // inside holds of the same pools; the units held do not change what the
    // figure draws, so `hold live (1)` and `hold live (2)` are one.
    fn same(a: &Stmt, b: &Stmt) -> bool {
        match (a, b) {
            (Stmt::Request, Stmt::Request) => true,
            (
                Stmt::Hold {
                    pools: p, body: x, ..
                },
                Stmt::Hold {
                    pools: q, body: y, ..
                },
            ) => {
                p.len() == q.len()
                    && p.iter().zip(q).all(|(a, b)| a.0.same_target(&b.0))
                    && same(&x[0], &y[0])
            }
            _ => false,
        }
    }
    for s in stmts {
        match s {
            Stmt::Request => {
                let mut site = s.clone();
                for &h in holds.iter().rev() {
                    let mut h = h.clone();
                    let Stmt::Hold { body, .. } = &mut h else {
                        unreachable!("only holds are pushed")
                    };
                    *body = vec![site];
                    site = h;
                }
                if !out.iter().any(|o| same(o, &site)) {
                    out.push(site);
                }
            }
            Stmt::Hold { body, .. } => {
                let mut inner = holds.to_vec();
                inner.push(s);
                request_sites(body, &inner, out);
            }
            Stmt::Loop(body) | Stmt::While(_, body) | Stmt::Fork(body) => {
                request_sites(body, holds, out)
            }
            Stmt::Branch(_, a, b) => {
                request_sites(a, holds, out);
                request_sites(b, holds, out);
            }
            _ => {}
        }
    }
}

/// Replace every internal request marker, at any depth, by the server's
/// statements. Returns how many were replaced.
fn splice(stmts: &mut Vec<Stmt>, server: &[Stmt]) -> usize {
    let mut n = 0;
    let mut out = Vec::with_capacity(stmts.len());
    for mut s in std::mem::take(stmts) {
        match &mut s {
            Stmt::Request => {
                n += 1;
                out.push(Stmt::Side(crate::ir::Side::Server));
                out.extend(server.iter().cloned());
                out.push(Stmt::Side(crate::ir::Side::Workload));
                continue;
            }
            Stmt::Hold { body, .. }
            | Stmt::Loop(body)
            | Stmt::While(_, body)
            | Stmt::Fork(body) => n += splice(body, server),
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
pub(crate) fn names(e: &Expr, vars: &mut Vec<String>, refs: &mut Vec<Ref>) {
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
        Expr::Call(f, args) if f == "cost" => {
            if let Some(arg) = args.last() {
                match arg {
                    Arg::Expr(e) => names_in(e, vars, indexed, refs),
                    Arg::Ref(r) => vars.push(r.name.clone()),
                }
            }
        }
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
            Stmt::Loop(body) | Stmt::While(_, body) | Stmt::Fork(body) => {
                collect_entry(body, locals, marks, leased)
            }
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
            Stmt::Declare(..)
            | Stmt::Side(_)
            | Stmt::Turn
            | Stmt::End
            | Stmt::Request
            | Stmt::Mark(_)
            | Stmt::Join => {}
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
            Stmt::While(e, b) => {
                bodies.push(e);
                split_reads(b, headers, bodies, indices);
            }
            Stmt::Loop(b) | Stmt::Fork(b) => split_reads(b, headers, bodies, indices),
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
            Stmt::Loop(b) | Stmt::While(_, b) | Stmt::Fork(b) => pools_named(b, out),
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
            args_imported: false,
            supplied_inputs: vec![],
            toks,
            pos: 0,
            stages: vec![],
            definitions: vec![],
            cost_records: Default::default(),
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
            entry_gateway: false,
            relation_share: None,
            posters: vec![],
            latency_ok: false,
            dotted_reads: vec![],
            indexed_dotted: vec![],
            devices: vec![],
            engines: vec![],
            pools_on: vec![],
            in_schedule: false,
            device_scope: None,
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

    fn eat(&mut self, token: &Tok) -> bool {
        if self.peek() == token {
            self.advance();
            true
        } else {
            false
        }
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

    /// `claim NAME [given (e)] : …;`, after `claim`: a proposition about
    /// every path, which reads and does not act.
    fn claim(&mut self) -> PResult<ClaimDecl> {
        let span = Some(self.span());
        let name = self.name()?;
        let given = if self.eat_kw("given") {
            self.expect(&Tok::LParen)?;
            let e = self.expr()?;
            self.expect(&Tok::RParen)?;
            Some(e)
        } else {
            None
        };
        self.expect(&Tok::Colon)?;
        let over = if self.is_kw("every") || self.is_kw("some") {
            let every = self.eat_kw("every");
            if !every {
                self.expect_kw("some")?;
            }
            self.expect_kw("iteration")?;
            self.expect_kw("of")?;
            let r = self.reference()?;
            if every {
                ClaimOver::Every(r)
            } else {
                ClaimOver::Some(r)
            }
        } else if self.eat_kw("at") {
            self.expect_kw("end")?;
            ClaimOver::End
        } else {
            return self.err(format!(
                "a claim is `every iteration of STAGE (…)`, `some iteration of STAGE (…)` \
                 or `at end (…)`, found {}",
                self.peek()
            ));
        };
        self.expect(&Tok::LParen)?;
        let expr = self.expr()?;
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::Semi)?;
        Ok(ClaimDecl {
            span,
            name,
            given,
            over,
            expr,
        })
    }

    /// `{ horizon e; warmup e; seed e; arrivals e; }`, after `run`.
    fn run_block(&mut self, run: &mut RunOpts) -> PResult<()> {
        self.expect(&Tok::LBrace)?;
        while *self.peek() != Tok::RBrace {
            let key = self.ident()?;
            let e = self.expr()?;
            self.expect(&Tok::Semi)?;
            match key.as_str() {
                "horizon" => run.horizon = Some(e),
                "warmup" => run.warmup = Some(e),
                "seed" => run.seed = Some(e),
                "arrivals" => run.arrivals = Some(e),
                other => return self.err(format!("unknown run option `{other}`")),
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(())
    }

    /// An instance: `let` bindings of the program's inputs and at most
    /// one `run` block, nothing that adds to the program's structure.
    fn instance(&mut self) -> PResult<(Vec<(String, Expr)>, RunOpts)> {
        let (mut lets, mut run, mut ran) =
            (Vec::<(String, Expr)>::new(), RunOpts::default(), false);
        while *self.peek() != Tok::Eof {
            if self.eat_kw("let") {
                let at = self.pos;
                let name = self.ident()?;
                if lets.iter().any(|(n, _)| *n == name) {
                    return self.err_at(at, format!("`{name}` is bound twice in this instance"));
                }
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                lets.push((name, e));
            } else if self.is_kw("run") && !ran {
                self.pos += 1;
                self.run_block(&mut run)?;
                ran = true;
            } else {
                return self.err(format!(
                    "an instance binds values: found {} where `let` or `run` goes\n\
                     help: an instance gives the program's declared inputs their values \
                     (`let NAME = expr;`) and the run its options (`run {{ … }}`, once); \
                     pools, stages, the workload and definitions belong to the program",
                    self.peek()
                ));
            }
        }
        Ok((lets, run))
    }

    fn program(&mut self) -> PResult<Program> {
        let mut prog = Program::default();
        let mut in_main = false;
        while *self.peek() != Tok::Eof {
            if self.toks[self.pos].file == 0 {
                if self.eat_kw("fn") {
                    if in_main || prog.has_main {
                        return self.err("a program has exactly one `fn main()`; nested or duplicate entry points are not allowed");
                    }
                    let name = self.ident()?;
                    if name != "main" {
                        return self
                            .err("the entry point is `fn main()`; reusable definitions use `def`");
                    }
                    self.expect(&Tok::LParen)?;
                    self.expect(&Tok::RParen)?;
                    self.expect(&Tok::LBrace)?;
                    prog.has_main = true;
                    in_main = true;
                    continue;
                }
                if in_main && *self.peek() == Tok::RBrace {
                    self.advance();
                    in_main = false;
                    continue;
                }
                if !in_main && !self.is_kw("use") && !self.is_kw("def") && !self.is_kw("let") {
                    return self.err("executable declarations belong inside `fn main() { … }`; only `use`, `def` and constants belong outside it");
                }
                if prog.has_main && !in_main {
                    return self.err("declarations belong before `fn main()`; its local names are not visible after it");
                }
            }
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
                let mut supplied_input = false;
                let e = if matches!(self.peek(), Tok::Ident(n) if n == "args")
                    && *self.peek_at(1) == Tok::Dot
                {
                    if !in_main || !self.args_imported {
                        return self.err("`args.number` needs `use \"std/args\";` and a `let` inside `fn main()`");
                    }
                    self.advance();
                    self.advance();
                    let function = self.ident()?;
                    if function != "number" {
                        return self.err(format!("std/args has no function `{function}`; use `args.number(\"name\", default)`"));
                    }
                    self.expect(&Tok::LParen)?;
                    let key = self.string()?;
                    crate::frontend::args::check_name(&key).or_else(|e| self.err(e))?;
                    if prog.inputs.iter().any(|(n, _)| *n == key) {
                        return self.err(format!(
                            "argument `{key}` is declared twice; read it once and reuse its binding"
                        ));
                    }
                    self.expect(&Tok::Comma)?;
                    let default = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    supplied_input = self.supplied_inputs.contains(&key);
                    prog.inputs.push((key, prog.lets.len()));
                    default
                } else {
                    self.expr()?
                };
                self.expect(&Tok::Semi)?;
                let mut vars = vec![];
                names(&e, &mut vars, &mut Vec::new());
                let varies =
                    supplied_input || vars.iter().any(|v| self.structural_overrides.contains(v));
                self.structural_overrides.retain(|n| n != &name);
                if varies {
                    self.structural_overrides.push(name.clone());
                }
                if let Some(v) = self.const_value(&e) {
                    self.consts.push((name.clone(), v));
                }
                prog.lets.push((name, e));
            } else if self.eat_kw("def") {
                self.def()?;
            } else if self.eat_kw("claim") {
                prog.claims.push(self.claim()?);
            } else if self.eat_kw("gauge") {
                let name = self.ident()?;
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                prog.gauges.push((name, e));
            } else if self.eat_kw("pool") {
                let (d, on) = self.pool(true)?;
                if let Some((owner, at)) = on {
                    self.pool_on(device::PoolOn {
                        pool: prog.pools.len(),
                        owner,
                        cap: d.name.clone(),
                        at,
                    })?;
                }
                prog.pools.push(d);
            } else if self.eat_kw("device") {
                self.device(&prog)?;
            } else if self.at_engine() {
                self.advance();
                self.engine(&mut prog)?;
            } else if self.is_kw("queue") {
                let at = self.pos;
                self.advance();
                self.queue(&mut prog, at)?;
            } else if self.eat_kw("stage") {
                let d = self.stage()?;
                self.stages.push(d.name.clone());
                prog.stages.push(d);
            } else if self.eat_kw("workload") {
                if prog.workload.is_some() {
                    return self.err("duplicate workload");
                }
                prog.workload = Some(self.workload()?);
            } else if self.is_kw("session") {
                return self.err("`session` belongs inside `workload`; put request handling in `server` and describe its turns with `turn;`");
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
                && (*self.peek_at(1) == Tok::Ident("pull".into())
                    || *self.peek_at(1) == Tok::Ident("push".into()))
            {
                self.copy_relation(&mut prog)?;
            } else if self.is_kw("share") {
                let at = self.pos;
                self.advance();
                if let Some(first) = self.share_at {
                    let line = self.toks[first].line;
                    let what = match self.relation_share {
                        Some(verb) => format!("the {verb} relation on line"),
                        None => "the first is on line".into(),
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
                return self.err("execution settings do not belong in a model\nhelp: supply --horizon T (and --warmup, --seed, --arrivals) or --instance FILE; `run STAGE (work);` belongs in a session or server");
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
        if in_main {
            return self.err("unclosed `fn main()`; expected `}`");
        }
        if self.args_imported
            && (prog.lets.iter().any(|(n, _)| n == "args")
                || prog.pools.iter().any(|p| p.name == "args")
                || prog.stages.iter().any(|s| s.name == "args")
                || self.queues.iter().any(|q| q.name == "args")
                || self.defs.iter().any(|d| d.name == "args"))
        {
            return self.err(
                "`args` names the imported std/args module; give the declaration another name",
            );
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
                if e.verb != "route" {
                    shared.extend(e.locals.iter().map(|l| format!("{}.{l}", q.name)));
                }
            }
            shared.extend(q.marks.iter().map(|m| format!("{}.{m}", q.name)));
        }
        served.shared = shared;
        self.assemble(&mut prog, &mut served)?;
        self.link_engines(&mut prog)?;
        self.check_body_bindings(&prog)?;
        self.check_def_names(&prog)?;
        self.check_deferred(&prog, &served)?;
        prog.definitions = std::mem::take(&mut self.definitions);
        prog.cost_records = std::mem::take(&mut self.cost_records).into_iter().collect();
        prog.libs = self.libs.clone();
        Ok(prog)
    }

    /// Expand queue entries, then insert the server after each turn draw.
    fn assemble(&mut self, prog: &mut Program, served: &mut Served) -> PResult<()> {
        if self.wl_session.is_none() && self.server.is_some() {
            if prog.workload.is_none() {
                return self.err("`server` needs a `workload`: its default session is one turn");
            }
            self.wl_session = Some((self.pos, vec![Stmt::Turn, Stmt::Request]));
        }
        // what one request runs, before either side is expanded: the one
        // thing every `request` of the workload's session names
        let mut request = vec![];
        if let Some((_, s)) = &self.wl_session {
            request_sites(s, &[], &mut request);
            if request.len() > 1 {
                request.clear();
            }
        }
        // Queues first: every entry call in place, so the sides are plain
        // statements when they are put together.
        {
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
            for (at, name, scope) in std::mem::take(&mut self.dotted_reads) {
                let (qn, field) = name.split_once('.').expect("a dotted name");
                let record = scope.map_or_else(|| qn.to_string(), |q| format!("{q}.{qn}"));
                let ok = self
                    .cost_records
                    .get(&record)
                    .is_some_and(|fields| fields.iter().any(|f| f == field))
                    || self.queues.iter().any(|q| {
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
            expand(&mut request)?;
        }
        if let Some((_, server)) = &self.server {
            assigned_in(server, &mut served.server);
        }
        match (self.wl_session.take(), self.server.take()) {
            (None, None) if prog.workload.is_some() => self.err("a workload needs a `server` block to handle its turns"),
            (None, None) => Ok(()),
            (Some((at, _)), None) => self.err_at(
                at,
                "a `session` inside `workload` is written against a `server` block; add `server { … }`",
            ),
            (None, Some((at, _))) => self.err_at(
                at,
                "`server` needs a `workload`",
            ),
            (Some((_, mut session)), Some((v_at, server))) => {
                if splice(&mut session, &server) == 0 {
                    return self.err_at(
                        v_at,
                        "the workload's session has no `turn;`: each turn uses the server",
                    );
                }
                splice(&mut request, &server);
                if !matches!(session.last(), Some(Stmt::End)) {
                    session.push(Stmt::End);
                }
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

    /// A use whose body says `turn;` may not pass an argument
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
                (u.turn
                    && (turned.contains(n)
                        || served.server.contains(n)
                        || served.shared.contains(n)))
                .then_some((n, "`turn;`"))
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
        if path == "std/args" {
            self.args_imported = true;
            return Ok(());
        }
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

    /// `def name(x, …) { e }` or `def name(x, …) { … }`, after `def`.
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
        if self.is_time_resource(&name) {
            return self.err_at(
                at,
                format!("`{name}` is a device's time resource: name the definition otherwise"),
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
        if *self.peek() == Tok::Assign {
            return self.err(
                "expression definitions use `def name(…) { expression }` without a semicolon",
            );
        }
        self.expect(&Tok::LBrace)?;
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Tok::Eof => return self.err_at(at, format!("`def {name}` is not closed")),
                Tok::RBrace if depth == 0 => break,
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
        // Expressions contain no statement terminators or blocks. Empty
        // definitions remain statement definitions, as do nested blocks.
        let stmts = body.is_empty()
            || body
                .iter()
                .any(|t| matches!(t.tok, Tok::Semi | Tok::LBrace));
        if stmts
            && body
                .last()
                .is_some_and(|t| !matches!(t.tok, Tok::Semi | Tok::RBrace))
        {
            return self.err_at(at, "a definition contains either one expression without a semicolon or statements; a statement body cannot end with a result expression");
        }

        if let Some(i) = self.def_overrides.iter().position(|d| d.0 == name) {
            if stmts {
                return self.err_at(
                    at,
                    format!(
                        "the `def` override `{name}`: `def {name}` is statements; an override replaces the body of an \
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
                    msg: format!("the `def` override `{name}`: {}", e.msg),
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
        let (mut reads, mut calls) = self.reads_of(&body, !stmts);
        reads.retain(|n| !params.contains(n));
        for d in &used {
            assigns.extend(d.assigns.iter().cloned());
            turn |= d.turn;
        }
        assigns.sort();
        assigns.dedup();
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
                    format!(
                        "the `let` override `{name}` affects an array size resolved during parsing"
                    ),
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
        self.fold(e, &std::cell::Cell::new(0))
    }

    /// `const_value` with the terms its aggregates have written out so far:
    /// nested ones multiply, and like the linker's (`over_terms`) the total
    /// is bounded, or a nest of three would run for minutes. Past the bound
    /// this is not a constant here, and the linker says why.
    fn fold(&self, e: &Expr, terms: &std::cell::Cell<usize>) -> Option<f64> {
        Some(match e {
            Expr::Located(_, inner) => self.fold(inner, terms)?,
            Expr::Num(x) => *x,
            Expr::Var(n) if n == "inf" => f64::INFINITY,
            Expr::Var(n) => self.consts.iter().rev().find(|(c, _)| c == n)?.1,
            Expr::Unary(UnOp::Neg, a) => -self.fold(a, terms)?,
            Expr::Unary(UnOp::Not, a) => {
                if self.fold(a, terms)? != 0.0 {
                    0.0
                } else {
                    1.0
                }
            }
            Expr::Binary(op, a, b) => {
                crate::frontend::link::binop(*op, self.fold(a, terms)?, self.fold(b, terms)?)
            }
            Expr::Cond(c, a, b) => {
                if self.fold(c, terms)? != 0.0 {
                    self.fold(a, terms)?
                } else {
                    self.fold(b, terms)?
                }
            }
            // folded as the linker writes it out, so that a `let` it folds
            // sizes an array here too (#274)
            Expr::Over(agg, j, n, body) => {
                let count = self.fold(n, terms)?;
                let total = terms.get() as f64 + count;
                if !(count >= 1.0
                    && count.fract() == 0.0
                    && total <= crate::frontend::link::MAX_OVER as f64)
                {
                    return None;
                }
                terms.set(total as usize);
                let mut acc: Option<f64> = None;
                for k in 0..count as usize {
                    let mut term = (**body).clone();
                    crate::frontend::link::bind_index(&mut term, j, k as f64);
                    let x = self.fold(&term, terms)?;
                    acc = Some(match (acc, agg) {
                        (None, _) => x,
                        (Some(a), Agg::Sum) => a + x,
                        (Some(a), Agg::Max) => a.max(x),
                        (Some(a), Agg::Min) => a.min(x),
                    });
                }
                acc?
            }
            // the linker's constant functions, so that a `let` the linker
            // folds the parser folds too (`let N = min(2, 3); queue D[N]`)
            Expr::Call(f, args) => {
                let xs = args
                    .iter()
                    .map(|a| match a {
                        Arg::Expr(e) => self.fold(e, terms),
                        Arg::Ref(r) if r.index.is_none() => {
                            self.fold(&Expr::Var(r.name.clone()), terms)
                        }
                        Arg::Ref(_) => None,
                    })
                    .collect::<Option<Vec<f64>>>()?;
                crate::frontend::link::const_call(f, &xs)?
            }
            Expr::Sample(..) => return None,
        })
    }

    /// `pool NAME [N] { … }`, or `pool NAME on OWNER { … }` where `on` is
    /// allowed: OWNER's capacity NAME, with the owner and where it is named.
    fn pool(&mut self, on_ok: bool) -> PResult<(PoolDecl, Option<(String, usize)>)> {
        let span = Some(self.span());
        let at = self.pos;
        let name = self.ident()?;
        let array = self.array_count()?;
        let mut on = None;
        let mut count = array.unwrap_or(1);
        let mut is_array = array.is_some();
        let mut cap = Expr::Num(f64::INFINITY);
        if self.is_kw("on") {
            if !on_ok {
                return self.err("a queue's pool is its own: `pool NAME on …` is a deployment's");
            }
            if array.is_some() {
                return self.err_at(
                    at,
                    "a pool on a device or an engine is a family as its owner is: write no `[N]`",
                );
            }
            self.advance();
            let o_at = self.pos;
            let mut owner = self.ident()?;
            if let Some(q) = &self.device_scope {
                // a queue's pool is the member's: on the queue's own device,
                // or on the queue's engine, which is named after the queue
                let scoped = format!("{q}.{owner}");
                if self.devices.iter().any(|d| d.name == scoped) {
                    owner = scoped;
                } else if owner != *q {
                    return self.err_at(
                        o_at,
                        format!(
                            "a queue's pool is on its own device or its engine: `{owner}` is \
                             not a device or the engine of queue `{q}`; write `on DEVICE` for a \
                             `device` above, or `on {q}`"
                        ),
                    );
                }
            } else if self.queues.iter().any(|q| q.name == owner)
                && self.engines.iter().any(|e| e.name == owner)
            {
                return self.err_at(
                    o_at,
                    format!(
                        "`{owner}` is a queue's engine, which admits the queue's own pools: \
                         declare `pool {name} on {owner}` in queue `{owner}`"
                    ),
                );
            }
            let (c, n, a) = self.capacity_of(at, &name, &owner)?;
            (cap, count, is_array) = (c, n, a);
            on = Some((owner, o_at));
        }
        self.expect(&Tok::LBrace)?;
        let mut d = PoolDecl {
            span,
            name,
            count,
            array: is_array,
            cap,
            block: None,
            evict: EvictOrder::Lru,
            preempt: PreemptOrder::None,
            queue: QueueOrder::Fifo,
            spill: None,
            admit_via: None,
            reserve_held: false,
        };
        while *self.peek() != Tok::RBrace {
            let key = self.ident()?;
            if on.is_some() && (key == "cap" || key == "admit") {
                return self.err_at(
                    self.pos - 1,
                    format!(
                        "a pool on a device or an engine takes `{key}` from it: its capacity \
                         and who admits it are already said"
                    ),
                );
            }
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
                        // the latest admitted, back at the head: vLLM's
                        // `running[-1]` and `prepend_request`
                        let admission = Expr::Var("admission".into());
                        PreemptOrder::By {
                            keys: vec![Expr::Unary(UnOp::Neg, Box::new(admission))],
                            tail: false,
                        }
                    } else if self.eat_kw("by") {
                        self.expect(&Tok::LParen)?;
                        let mut keys = vec![self.expr()?];
                        while *self.peek() == Tok::Comma {
                            self.advance();
                            keys.push(self.expr()?);
                        }
                        self.expect(&Tok::RParen)?;
                        let tail = if self.eat_kw("requeue") {
                            if self.eat_kw("tail") {
                                true
                            } else {
                                self.expect_kw("head")?;
                                false
                            }
                        } else {
                            false
                        };
                        PreemptOrder::By { keys, tail }
                    } else {
                        self.expect_kw("none")?;
                        PreemptOrder::None
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
                "reserve" => {
                    self.expect_kw("held")?;
                    d.reserve_held = true;
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
        Ok((d, on))
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

    /// `{ stmt* }` of a step stage's `iteration`: `serve`, `admit` and
    /// `branch`, and nothing else (no loop: an iteration ends).
    fn iteration_body(&mut self) -> PResult<Vec<IterStmt>> {
        self.expect(&Tok::LBrace)?;
        let mut body = vec![];
        while *self.peek() != Tok::RBrace {
            body.push(self.iteration_stmt()?);
        }
        self.expect(&Tok::RBrace)?;
        Ok(body)
    }

    fn iteration_stmt(&mut self) -> PResult<IterStmt> {
        if self.eat_kw("serve") {
            let mut only = None;
            if self.eat_kw("only") {
                self.expect(&Tok::LParen)?;
                only = Some(self.expr()?);
                self.expect(&Tok::RParen)?;
            }
            let order = if self.eat_kw("admission") {
                Some(Serve::Admission)
            } else if self.eat_kw("decode") {
                self.expect_kw("first")?;
                Some(Serve::DecodeFirst)
            } else if self.eat_kw("by") {
                self.expect(&Tok::LParen)?;
                let mut keys = vec![self.expr()?];
                while *self.peek() == Tok::Comma {
                    self.expect(&Tok::Comma)?;
                    keys.push(self.expr()?);
                }
                self.expect(&Tok::RParen)?;
                Some(Serve::By(keys))
            } else if self.is_kw("exclusive") {
                return self.err(
                    "`exclusive prefill` is a stage's rule (one prefill, the whole budget, \
                     displacing the decodes already chosen), and a body cannot take back a serve: \
                     write the rule on the stage without a body, or a body without the rule",
                );
            } else {
                None
            };
            self.expect(&Tok::Semi)?;
            Ok(IterStmt::Serve { only, order })
        } else if self.eat_kw("admit") {
            let mut only = None;
            if self.eat_kw("only") {
                self.expect(&Tok::LParen)?;
                only = Some(self.expr()?);
                self.expect(&Tok::RParen)?;
            }
            let mut gate = None;
            if self.eat_kw("while") {
                self.expect(&Tok::LParen)?;
                gate = Some(self.expr()?);
                self.expect(&Tok::RParen)?;
            }
            self.expect(&Tok::Semi)?;
            Ok(IterStmt::Admit { only, gate })
        } else if self.eat_kw("branch") {
            self.expect(&Tok::LParen)?;
            let guard = self.expr()?;
            self.expect(&Tok::RParen)?;
            let then = self.iteration_body()?;
            let other = if self.eat_kw("else") {
                self.iteration_body()?
            } else {
                vec![]
            };
            Ok(IterStmt::Branch(guard, then, other))
        } else if self.eat_kw("set") {
            let name = self.ident()?;
            self.expect(&Tok::Assign)?;
            let e = self.expr()?;
            self.expect(&Tok::Semi)?;
            Ok(IterStmt::Set(name, e))
        } else {
            self.err(format!(
                "an iteration takes `serve`, `admit`, `branch` and `set`; found {}",
                self.peek()
            ))
        }
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
                per_run: false,
                granule: None,
                serve: Serve::Admission,
                only: None,
                memory: None,
                iteration: None,
                state: vec![],
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
                    "granule" => s.granule = Some(self.expr()?),
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
                    "state" => {
                        let name = self.ident()?;
                        self.expect(&Tok::Assign)?;
                        s.state.push((name, self.expr()?));
                    }
                    "iteration" => {
                        if s.iteration.is_some() {
                            return self.err("`iteration` twice: a step stage has one iteration");
                        }
                        s.iteration = Some(self.iteration_body()?);
                        // a block, like `step { … }`: no semicolon
                        continue;
                    }
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
    /// A gateway's `route` body is called from the server with `NAME.route();`.
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
        // a device or an engine declared after the queue is refused by its
        // own check, so one declared before is refused here: the order
        // changes nothing
        if self.devices.iter().any(|d| d.name == name) || prog.stages.iter().any(|s| s.name == name)
        {
            return self.err_at(at + 1, format!("`{name}` is declared twice"));
        }
        if name == "running" || name == "waiting" {
            return self.err_at(
                at + 1,
                format!(
                    "`{name}.…` is a value of an engine's schedule; a queue needs another name"
                ),
            );
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
            takes: None,
            at,
        });
        let qi = self.queues.len() - 1;
        self.expect(&Tok::LBrace)?;
        while *self.peek() != Tok::RBrace {
            // pools and devices, then the stage, then the entries: each reads
            // what is above it; a pool on a device or the engine may follow
            // the engine
            let item_at = self.pos;
            let pool_on =
                self.is_kw("pool") && matches!(self.peek_at(2), Tok::Ident(k) if k == "on");
            if self.is_kw("pool")
                && (!self.queues[qi].entries.is_empty() || (self.queues[qi].has_stage && !pool_on))
            {
                return self.err_at(
                    item_at,
                    format!("queue `{name}` declares its pools first, above its stage and entries"),
                );
            }
            if self.is_kw("serve") && !self.queues[qi].entries.is_empty() {
                return self.err_at(
                    item_at,
                    format!("queue `{name}` declares its `serve` above its entries"),
                );
            }
            if self.is_kw("device") {
                self.advance();
                if self.queues[qi].has_stage || !self.queues[qi].entries.is_empty() {
                    return self.err_at(
                        item_at,
                        format!("queue `{name}` declares its device above its engine and entries"),
                    );
                }
                let d_at = self.pos;
                let dev = self.ident()?;
                if *self.peek() == Tok::LBracket {
                    return self.err(format!(
                        "a queue's device is the member's, one per member of `{name}`: write `device {dev}`"
                    ));
                }
                if KEYWORDS.contains(&dev.as_str()) {
                    return self.err_at(d_at, format!("`{dev}` is a word of the language"));
                }
                if dev == name {
                    return self.err_at(
                        d_at,
                        format!(
                            "`{dev}` names queue `{name}`'s engine, so `on {dev}` would mean \
                             two things: a queue's device needs another name"
                        ),
                    );
                }
                let scoped = format!("{name}.{dev}");
                if self.devices.iter().any(|d| d.name == scoped)
                    || self.queues[qi].pools.contains(&dev)
                {
                    return self
                        .err_at(d_at, format!("`{dev}` is declared twice in queue `{name}`"));
                }
                self.device_body(scoped, count, family)?;
            } else if self.is_kw("engine") && matches!(self.peek_at(1), Tok::Ident(k) if k == "on")
            {
                let e_at = self.pos;
                self.advance();
                self.advance();
                if self.queues[qi].has_stage {
                    return self.err_at(
                        e_at,
                        format!("queue `{name}` has one stage: its engine or its `serve`"),
                    );
                }
                if KEYWORDS.contains(&name.as_str()) {
                    return self.err_at(
                        at + 1,
                        format!(
                            "`{name}` is a word of the language, and a queue's engine is named \
                             after the queue: name the queue for what it models"
                        ),
                    );
                }
                let d_at = self.pos;
                let dev = self.ident()?;
                let scoped = format!("{name}.{dev}");
                let Some(di) = self.devices.iter().position(|d| d.name == scoped) else {
                    return self.err_at(
                        d_at,
                        format!("no device `{dev}` in queue `{name}`: declare `device {dev} {{ … }}` above the engine"),
                    );
                };
                self.engine_body(prog, span, e_at, name.clone(), di)?;
                self.queues[qi].has_stage = true;
            } else if self.eat_kw("pool") {
                let p_at = self.pos;
                self.device_scope = Some(name.clone());
                let parsed = self.pool(true);
                self.device_scope = None;
                let (mut d, on) = parsed?;
                if self
                    .devices
                    .iter()
                    .any(|dv| dv.name == format!("{name}.{}", d.name))
                {
                    return self.err_at(
                        p_at,
                        format!("`{}` is declared twice in queue `{name}`", d.name),
                    );
                }
                if let Some((owner, o_at)) = on {
                    if self.queues[qi].pools.contains(&d.name) {
                        return self.err(format!("duplicate pool `{}` in queue `{name}`", d.name));
                    }
                    self.queues[qi].pools.push(d.name.clone());
                    self.pool_on(device::PoolOn {
                        pool: prog.pools.len(),
                        owner,
                        cap: d.name.clone(),
                        at: o_at,
                    })?;
                    d.name = format!("{name}.{}", d.name);
                    prog.pools.push(d);
                    continue;
                }
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
                self.stages.push(name.clone());
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
                self.stages.push(nname.clone());
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
                    "expected `pool`, `device`, `engine on`, `serve` or an entry in queue \
                     `{name}`, found {}",
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
            self.stages.push(lname.clone());
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
        self.side = Side::Server;
        self.in_queue = Some(qi);
        self.entry_from = from.clone();
        self.entry_gateway =
            verb == "route" && self.queues[qi].roles.iter().any(|r| r == "gateway");
        let body = self.block();
        self.entry_from = None;
        self.entry_gateway = false;
        self.in_queue = None;
        self.side = outer_side;
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
                    if header && locals.contains(&v) {
                        return self.err_at(at, format!(
                            "`{qname}.{verb}`: the admission header reads local `{v}`; it sees parameters and the queue's resources, not values calculated by its body. Write the resource conversion in the header or pass a quantity as a parameter"
                        ));
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
                    "`{}` is an expression (`def {0}(…) {{ … }}`), not statements",
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
        if let Tok::Ident(s) = self.peek()
            && (s == "prefill" || s == "decode")
        {
            let s = s.clone();
            return self.err(format!(
                "`{s}` is the mode of a `run` on a step engine, not a statement\n\
                 help: write `run E {s} (cost(E, T));` on the step engine `E`, or `run S (cost(S, W));` on another stage"
            ));
        }
        if self.is_kw("Size") || self.is_kw("Cost") {
            return self.typed_declaration(out);
        }
        let stmt = self.stmt()?;
        let turn = matches!(stmt, Stmt::Turn);
        out.push(stmt);
        if turn {
            out.push(Stmt::Request);
        }
        Ok(())
    }

    fn cost_scope(&self) -> Option<String> {
        self.in_queue
            .filter(|_| !self.entry_gateway)
            .map(|qi| self.queues[qi].name.clone())
    }

    /// Typed declarations are source sugar; each field retains a resource cost.
    fn typed_declaration(&mut self, out: &mut Vec<Stmt>) -> PResult<()> {
        let is_size = self.eat_kw("Size");
        if !is_size {
            self.expect_kw("Cost")?;
        }
        if is_size && self.side == Side::Server {
            return self
                .err("a server cannot declare a request Size; calculate a separate value or Cost");
        }
        let name = self.definition()?;
        self.expect(&Tok::Assign)?;
        let kind = if is_size {
            DeclaredType::Size
        } else {
            DeclaredType::Cost
        };
        if !is_size && self.eat(&Tok::LBrace) {
            let mut fields = vec![];
            loop {
                if *self.peek() == Tok::RBrace {
                    break;
                }
                let r = self.reference()?;
                if r.index.is_some() {
                    return self.err("a Cost field names a resource family without a member index");
                }
                if fields.contains(&r.name) {
                    return self.err(format!("Cost `{name}` repeats resource `{}`", r.name));
                }
                self.expect(&Tok::Colon)?;
                let value = self.expr()?;
                let field = format!("{name}.{}", r.name);
                fields.push(r.name.clone());
                out.push(Stmt::Declare(field.clone(), kind));
                out.push(Stmt::Set(field, Expr::cost(&[r], value)));
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RBrace)?;
            self.expect(&Tok::Semi)?;
            if fields.is_empty() {
                return self.err("a Cost record must name at least one resource");
            }
            let record = self
                .cost_scope()
                .map_or_else(|| name.clone(), |q| format!("{q}.{name}"));
            if let Some(previous) = self.cost_records.get(&record) {
                if previous != &fields {
                    return self.err(format!(
                        "Cost `{name}` is redeclared with different resource fields or order"
                    ));
                }
            } else {
                self.cost_records.insert(record, fields);
            }
        } else {
            let value = self.expr()?;
            self.expect(&Tok::Semi)?;
            out.push(Stmt::Declare(name.clone(), kind));
            out.push(Stmt::Set(name, value));
        }
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
        if self.side == Side::WorkloadSession {
            return self
                .err("queue entries belong in `server`; a session describes turns with `turn;`");
        }
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
            "request" => self.err("`request` is replaced by `turn;`: a turn draws its attributes and waits for the server; put gateway routing in `server { gw.route(); }`"),
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
            "while" => {
                self.advance();
                let guard = self.paren_expr()?;
                Ok(Stmt::While(guard, self.block()?))
            }
            "loop" => {
                self.advance();
                Ok(Stmt::Loop(self.block()?))
            }
            // `fork { … }`: the block runs beside the session, a leg of the
            // same request; `join;` waits for every leg forked so far
            "fork" => {
                self.advance();
                Ok(Stmt::Fork(self.block()?))
            }
            "join" => {
                self.advance();
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Join)
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
                e.substitute(&binds);
                if let Some(f) = reserve {
                    f.substitute(&binds);
                }
            }
            if let Some(r) = &mut reuse {
                r.substitute(&binds);
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
            c.substitute(&binds);
        }
        // `lease P (t)`: the allocation on `P` outlives the scope, for the
        // session's transfer to take, for at most `t` seconds
        let lease = if self.eat_kw("lease") {
            let r = self.own_pool("lease")?;
            let mut t = self.paren_expr()?;
            t.substitute(&binds);
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
            if e.draws() {
                return self.err(format!(
                    "`{name}` draws a sample: a `{clause}` binding is substituted, \
                     so a name used twice would draw twice"
                ));
            }
            e.substitute(&binds);
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
    /// is the program's `share`. `P push D latency x share maxmin;`: the
    /// same copy over the same two NICs, written by `P` (NIXL's push mode),
    /// so the wait `x` before each write is `P`'s.
    fn copy_relation(&mut self, prog: &mut Program) -> PResult<()> {
        let at = self.pos;
        let span = Some(self.span());
        let first = self.ident()?;
        let push = *self.peek() == Tok::Ident("push".into());
        let verb = if push { "push" } else { "pull" };
        self.advance(); // `pull` or `push`
        let s_at = self.pos;
        let second = self.ident()?;
        let (reader, source) = if push {
            (second.clone(), first.clone())
        } else {
            (first.clone(), second.clone())
        };
        let written = format!("`{first} {verb} {second}`");
        let copies = if push { "writes" } else { "reads" };
        for (q, q_at) in [(&first, at), (&second, s_at)] {
            let Some(d) = self.queues.iter().find(|d| d.name == *q) else {
                return self.err_at(q_at, format!("{written}: no queue `{q}` is declared above"));
            };
            if !d.nic {
                return self.err_at(
                    q_at,
                    format!(
                        "{written}: queue `{q}` has no `nic`; a copy runs over \
                         the source's NIC and the reader's"
                    ),
                );
            }
        }
        if reader == source {
            return self.err_at(s_at, format!("{written}: a queue copies the KV to another"));
        }
        let qi = self
            .queues
            .iter()
            .position(|d| d.name == reader)
            .expect("checked above");
        if let Some(r) = &self.queues[qi].takes {
            return self.err_at(
                at,
                format!(
                    "`{reader}` takes the KV from `{}` already: one relation per reader",
                    r.source
                ),
            );
        }
        // the side that posts the copy waits before each one
        let poster = if push { &source } else { &reader };
        let pi = self
            .queues
            .iter()
            .position(|d| d.name == *poster)
            .expect("checked above");
        let latency = if self.eat_kw("latency") {
            let l_at = self.pos;
            let e = self.expr()?;
            let Some(v) = self.const_value(&e) else {
                return self.err_at(
                    l_at,
                    "`latency` is a number or a constant over `let`s: the fixed wait of the \
                     side that posts each copy",
                );
            };
            // a constant of its own (see the link's `latency`), and a delay
            // stage of the side that posts, one per member
            let lname = format!("{poster}.{verb}.time");
            let sname = format!("{poster}.nic.latency");
            // the wait is the poster's, one delay stage per poster
            if self.posters.contains(poster) {
                return self.err_at(
                    l_at,
                    format!(
                        "{written}: `{poster}` waits before the copies of another relation \
                         already, at `{sname}`: one `latency` per poster"
                    ),
                );
            }
            self.posters.push(poster.clone());
            self.consts.push((lname.clone(), v));
            prog.lets.push((lname.clone(), e));
            let count = self.queues[pi].count;
            self.stages.push(sname.clone());
            prog.stages.push(StageDecl {
                span,
                name: sname,
                count,
                array: self.queues[pi].family,
                kind: StageKind::Delay,
            });
            Some(Expr::Var(lname))
        } else {
            None
        };
        // the policy is the relation's: written here, not left to a default
        if !self.eat_kw("share") {
            return self.err(format!(
                "{written} names how concurrent {copies} divide the NICs: \
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
            Some(_) if self.relation_share.is_none() => {
                return self.err_at(
                    at,
                    "`share` is declared on its own and on the relation: the relation names it",
                );
            }
            _ => {}
        }
        prog.share = Some(policy);
        self.relation_share = Some(verb);
        self.share_at.get_or_insert(at);
        self.queues[qi].takes = Some(queue::Takes {
            source,
            latency,
            push,
        });
        Ok(())
    }

    /// `tool Z;`, `transfer[j] X from P to Q (n);`, `tool on S (Z);`: a
    /// `run` on the stage that plays the role.
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
        let raw_work = self.expr()?;
        let resources: Vec<_> = std::iter::once(&stage)
            .chain(also.iter())
            .cloned()
            .collect();
        let work = if pulled {
            raw_work
        } else {
            Expr::cost(&resources, raw_work)
        };
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
                        work: Expr::cost(
                            &[Ref {
                                span: r.span,
                                name: format!("{}.latency", q.name),
                                index: None,
                            }],
                            q.latency.clone()?,
                        ),
                        growing: None,
                        also: vec![],
                    })
                })
                .collect();
            out.extend([
                Stmt::Run {
                    stage,
                    mode: RunMode::Plain,
                    work,
                    growing: None,
                    also,
                },
                Stmt::Load(to.clone(), Expr::cost(&[to], units)),
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
                     help: write `transfer (w) from P to Q (n);`, or `run {at_stage} (cost({}, w));` for a link that only takes time", stage.name
                ),
            );
        }
        self.expect(&Tok::Semi)?;
        Ok(vec![Stmt::Run {
            stage,
            mode: RunMode::Plain,
            work,
            growing,
            also,
        }])
    }

    /// A stage named after `on`, which must be declared above.
    fn declared_stage(&mut self, kw: &str) -> PResult<Ref> {
        let ref_at = self.pos;
        let r = self.reference()?;
        if !self.stages.contains(&r.name) {
            let help = crate::frontend::diagnostic::suggestion(
                &r.name,
                self.stages.iter().map(String::as_str),
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
    /// name; exactly one.
    fn role_stage(&self, role: Role, at: usize) -> PResult<String> {
        let kw = role.keyword();
        let found: Vec<&str> = self
            .stages
            .iter()
            .filter(|n| role.names().contains(&n.as_str()))
            .map(String::as_str)
            .collect();
        match found.len() {
            1 => Ok(found[0].to_string()),
            0 => {
                let names = role
                    .names()
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                self.err_at(
                    at,
                    format!(
                        "no stage declared above plays `{kw}`: name a stage {names}, or write `{kw} on STAGE (...)`"
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
                    if self.is_time_resource(&name) {
                        return self.err_at(
                            at,
                            format!(
                                "`{name}` is a device's time resource, read in an engine's `execute`"
                            ),
                        );
                    }
                    if name == "cost" {
                        let resources = args.len().saturating_sub(1);
                        for arg in &mut args[..resources] {
                            if let Arg::Ref(r) = arg {
                                if r.index.as_deref().is_some_and(Expr::draws) {
                                    return self.err_at(at, "a cost names a resource family; its type annotation cannot draw a member index");
                                }
                            }
                        }
                        if let Some(last @ Arg::Ref(_)) = args.last_mut() {
                            let Arg::Ref(r) = last else { unreachable!() };
                            if r.index.is_none() {
                                *last = Arg::Expr(Expr::Located(
                                    r.span.unwrap_or(span),
                                    Box::new(Expr::Var(r.name.clone())),
                                ));
                            }
                        }
                    }
                    Ok(Expr::Located(span, Box::new(Expr::Call(name, args))))
                } else if (name == "running" || name == "waiting") && *self.peek() == Tok::Dot {
                    let at = self.pos - 1;
                    self.advance();
                    let e = self.list_value(at, &name)?;
                    Ok(Expr::Located(span, Box::new(e)))
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
                    let mut name = format!("{name}.{}", self.ident()?);
                    while self.eat(&Tok::Dot) {
                        name.push('.');
                        name.push_str(&self.ident()?);
                    }
                    self.dotted_reads
                        .push((at, name.clone(), self.cost_scope()));
                    Ok(Expr::Located(span, Box::new(Expr::Var(name))))
                } else {
                    if name == "self" && self.in_queue.is_none() {
                        self.pos -= 1;
                        return self.err("`self` is a queue entry's word: the member's own index");
                    }
                    self.retired_in_schedule(self.pos - 1, &name)?;
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

    /// Existing semantic fixtures describe a main body; complete example files
    /// already contain their entry point. The entrypoint tests use the public API
    /// directly, so this builder cannot make an implicit program pass those checks.
    pub fn main_source(body: &str) -> String {
        if body.contains("fn main()") {
            body.to_string()
        } else {
            format!("fn main() {{ {body}\n}}")
        }
    }

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

          session {

            loop { turn;
              branch with (0.9) { run tool (cost(tool, Z));  } else { end; }
            }

          }
        }
        server {
          hold kv (cost(kv, K + n + o)) {
            run prefill (cost(prefill, a * (K + n - cached)));
            observe ttft = now - t0;
            run decode (cost(decode, o * 2e-4));
          } cache (cost(kv, K + n + o));
        }

"#;
        let p = parse(&main_source(src)).unwrap();
        assert_eq!(p.pools.len(), 1);
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.lets[0].0, "a");
        assert!(matches!(p.session[0], Stmt::Loop(_)));
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

    /// Compare executable expansion, erasing authority markers only in this
    /// parser test. `size_cost` and IR tests check the retained authority;
    /// moving a size assignment across it is no longer a valid equivalence.
    fn same(a: &str, b: &str) {
        fn executable(stmts: &mut Vec<Stmt>) {
            stmts.retain(|s| !matches!(s, Stmt::Side(_)));
            for s in stmts {
                match s {
                    Stmt::Hold { body, .. }
                    | Stmt::Loop(body)
                    | Stmt::While(_, body)
                    | Stmt::Fork(body) => executable(body),
                    Stmt::Branch(_, a, b) => {
                        executable(a);
                        executable(b);
                    }
                    _ => {}
                }
            }
        }
        let run = |s: &str| {
            let mut p = without_locations(parse(&main_source(s)).unwrap());
            p.request.clear();
            executable(&mut p.session);
            p
        };
        assert_eq!(run(a), run(b));
    }

    #[test]
    fn serving_forms_desugar_to_the_kernel() {
        same(
            &format!(
                "{PD} workload {{ session {{ turn;
            branch with (p) {{ tool Z;  }} else {{ end; }}

        }} }}
        server {{
          hold kvD (cost(kvD, K)) {{ transfer X from kv to kvD (K); }}
        }}"
            ),
            &format!(
                "{PD} workload {{ session {{ turn;
            branch with (p) {{ run tool (cost(tool, Z));  }} else {{ end; }}

        }} }}
        server {{
          hold kvD (cost(kvD, K)) {{ run link (cost(link, X)); load kvD (cost(kvD, K)); release kv; }}
        }}"
            ),
        );
    }

    #[test]
    fn prefill_and_decode_are_the_mode_of_a_run() {
        // a word that named the stage and the mode at once: the run says both
        for (form, kw) in [
            ("prefill (n) growing kv;", "prefill"),
            ("decode on engine (o);", "decode"),
        ] {
            let e = parse(&main_source(&format!(
                "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, 1)) {{ {form} }}\n}}"
            )))
            .unwrap_err();
            assert!(
                e.msg.contains(&format!("`{kw}` is the mode of a `run`")),
                "{e}"
            );
            assert!(
                e.msg.contains(&format!("`run E {kw} (cost(E, T));`")),
                "{e}"
            );
            assert_eq!((e.line, e.col), (7, 34), "{e}");
        }
    }

    #[test]
    fn serving_forms_name_their_stage_explicitly() {
        // the role's stage array, indexed
        same(
            "stage tool[2] : fifo; workload { session { turn; \n} }\nserver { choose j in 2 by (work(tool[j])); tool[j] S;\n}",
            "stage tool[2] : fifo; workload { session { turn; \n} }\nserver { choose j in 2 by (work(tool[j])); run tool[j] (cost(tool, S));\n}",
        );
        // any stage
        same(
            "stage rep[2] : fifo; workload { session { turn; \n} }\nserver { tool on rep[1] S;\n}",
            "stage rep[2] : fifo; workload { session { turn; \n} }\nserver { run rep[1] (cost(rep, S));\n}",
        );
        // a stage named `transfer` plays transfer
        same(
            "pool a { cap 1; } pool b { cap 1; } stage transfer : fifo;
        workload { session { turn;
        } }
        server { hold b (cost(b, 1)) { hold a (cost(a, 1)) { transfer X from a to b (1); } }
        }",
            "pool a { cap 1; } pool b { cap 1; } stage transfer : fifo;
        workload { session { turn;
        } }
        server { hold b (cost(b, 1)) { hold a (cost(a, 1)) { run transfer (cost(transfer, X)); load b (cost(b, 1)); release a; } }
        }",
        );
    }

    #[test]
    fn a_binding_the_body_reads_is_set_at_the_top_of_the_body() {
        // the body sees `known` at its admission value, which for attributes
        // and constants is the value it has at the top of the body
        same(
            &format!(
                "{ENGINE} workload {{ session {{ turn;
            end;

        }} }}
        server {{
          hold kv (cost(kv, known)) at admission (known = computed < p ? p : computed + 1) {{
            run engine prefill (cost(engine, known - cached)) growing kv;
          }}
        }}"
            ),
            &format!(
                "{ENGINE} workload {{ session {{ turn;
            end;

        }} }}
        server {{
          hold kv (cost(kv, computed < p ? p : computed + 1)) {{
            set known = computed < p ? p : computed + 1;
            run engine prefill (cost(engine, known - cached)) growing kv;
          }}
        }}"
            ),
        );
        // a binding the body does not read stays in the header
        same(
            &format!(
                "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, h)) at admission (h = 1) {{ }}\n}}"
            ),
            &format!(
                "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, 1)) {{ }}\n}}"
            ),
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
            let e = parse(&main_source(&format!(
                "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, hit)) at admission ({binding}) {{ observe h = hit; }}\n}}"
            )
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
            "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ set n = 3; hold kv (cost(kv, m)) at admission (m = n + 1) {{ observe x = m; }}\n}}"
        );
        parse(&main_source(&src)).unwrap();
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
            let e = parse(&main_source(&format!(
                "{pre} {ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ {assign} hold kv (cost(kv, 1)) at admission ({binding}) {{ observe a = {name}; }}\n}}"
            )
            ))
            .unwrap_err();
            assert!(e.msg.contains(what), "{binding}: {}", e.msg);
        }
        let e = parse(&main_source(&format!(
            "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, 1)) at admission (k = k + 1) {{ observe a = k; }}\n}}"
        )
        ))
        .unwrap_err();
        assert!(e.msg.contains("reads itself"), "{}", e.msg);
        let e = parse(&main_source(&format!(
            "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, 1)) at admission (j = k, k = 5) {{ observe a = j; }}\n}}"
        )
        ))
        .unwrap_err();
        assert!(e.msg.contains("bound after it"), "{}", e.msg);
        // an attribute the body sets itself is not the binding
        let e = parse(&main_source(&format!(
            "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, hit)) at admission (hit = min(cachedin(kv), 1)) {{ set hit = cached; observe h = hit; }}\n}}"
        )
        ))
        .unwrap_err();
        assert!(e.msg.contains("an attribute the program sets"), "{}", e.msg);
    }

    #[test]
    fn a_binding_is_read_only_in_its_hold() {
        for after in [
            "observe b = k;",
            "hold kv (cost(kv, k)) { }",
            "branch (k > 1) { } else { }",
        ] {
            let e = parse(&main_source(&format!(
                "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, 1)) at admission (k = 2) {{ observe a = k; }} {after}\n}}"
            )
            ))
            .unwrap_err();
            assert!(e.msg.contains("outside the body"), "{after}: {}", e.msg);
        }
        let e = parse(&main_source(
            "pool kv { cap 100; queue by (k); } stage engine : step { cost 1; }
        workload { session { turn;
        } }
        server { hold kv (cost(kv, 1)) at admission (k = 2) { observe a = k; }
        }",
        ))
        .unwrap_err();
        assert!(e.msg.contains("outside the body"), "{}", e.msg);
        // two holds may bind one name, and a nested hold may bind it again
        // over a live outer binding its body does not read
        parse(&main_source(&format!(
            "{ENGINE} workload {{ session {{ turn;

        }} }}
        server {{
          hold kv (cost(kv, 1)) at admission (k = 2) {{ observe a = k; }}
          hold kv (cost(kv, 1)) at admission (k = 3) {{ observe b = k; }}
          hold kv (cost(kv, h)) at admission (h = cachedin(kv)) {{
            hold kv (cost(kv, 1)) at admission (h = 1) {{ observe c = h; }}
          }}
        }}"
        )))
        .unwrap();
    }

    #[test]
    fn a_def_is_its_body_where_it_is_used() {
        // an expression: the argument in parentheses, the body too
        same(
            &format!(
                "{ENGINE} def full(x) {{ floor((x - 1) / bs) * bs }}
        workload {{ session {{ turn;
        }} }}
        server {{ set h = full(a + b) * 2; set g = min(full(k), 3);
        }}"
            ),
            &format!(
                "{ENGINE} workload {{ session {{ turn;

        }} }}
        server {{
          set h = (floor(((a + b) - 1) / bs) * bs) * 2;
          set g = min(floor((k - 1) / bs) * bs, 3);
        }}"
            ),
        );
        // a serving form in a def finds its stage where the def is used
        same(
            "stage T[2] : fifo; def put(s, n) { tool on s (n); }
        workload { session { turn; end;
        } }
        server { choose j in 2 by (work(T[j])); put(T[j], 3);
        }",
            "stage T[2] : fifo;
        workload { session { turn;
            end;

        } }
        server {
          choose j in 2 by (work(T[j]));
          run T[j] (cost(T, 3));
        }",
        );
        // statements, with references for pools and stages
        same(
            "pool kv[2] { cap 100; } stage E[2] : step { cost 1; }
        def put(p, s, n) { hold p (cost(p, n)) { run s prefill (cost(s, n)) growing p; } cache (cost(p, n)); }
        workload { session { turn; end;
        } }
        server { choose j in 2 by (used(kv[j])); put(kv[j], E[j], k + 1);
        }",
            "pool kv[2] { cap 100; } stage E[2] : step { cost 1; }
        workload { session { turn;
            end;

        } }
        server {
          choose j in 2 by (used(kv[j]));
          hold kv[j] (cost(kv[j], (k + 1))) { run E[j] prefill (cost(E[j], (k + 1))) growing kv[j]; } cache (cost(kv[j], (k + 1)));
        }",
        );
        // the side is the use's: a `hold` in a server
        same(
            &format!(
                "{ENGINE} def take(n) {{ hold kv (cost(kv, n)) {{ run engine prefill (cost(engine, n)) growing kv; }} }}
        workload {{ session {{ turn; end; }} }}
        server {{ take(4); }}"
            ),
            &format!(
                "{ENGINE} workload {{ session {{ turn; end; }} }}
        server {{ hold kv (cost(kv, 4)) {{ run engine prefill (cost(engine, 4)) growing kv; }} }}"
            ),
        );
    }

    #[test]
    fn a_def_says_what_goes_wrong() {
        let err = |src: &str| {
            parse(&main_source(&format!("{ENGINE} {src}")))
                .unwrap_err()
                .msg
        };
        assert!(
            err("def f(x) { x } workload { session { turn; \n} }\nserver { f(1);\n}")
                .contains("is an expression")
        );
        assert!(
            err("def f(x) { end; } workload { session { turn; \n} }\nserver { set a = f(1);\n}")
                .contains("is statements")
        );
        assert!(
            err("def f(x) { x } workload { session { turn; \n} }\nserver { set a = f(1, 2);\n}")
                .contains("takes 1 argument(s), got 2")
        );
        assert!(
            err("def f(x) { x + x } workload { session { turn; \n} }\nserver { set a = f(~exp(1));\n}").contains("would draw 2 times")
        );
        assert!(
            err("def f(x) { f(x) } workload { session { turn; \n} }\nserver {\n}")
                .contains("uses itself")
        );
        assert!(
            err("def min(x) { x } workload { session { turn; \n} }\nserver {\n}")
                .contains("a word of the language")
        );
        assert!(
            err("def uniform(x) { x } workload { session { turn; \n} }\nserver {\n}")
                .contains("a word of the language")
        );
        assert!(
            err("def f(on) { on } workload { session { turn; \n} }\nserver {\n}")
                .contains("a word of the language")
        );
        assert!(
            err("def f(min) { min(min, 1) } workload { session { turn; \n} }\nserver {\n}")
                .contains("a word of the language")
        );
        // a definition uses only the ones before it: no recursion
        assert!(
            err("def g(x) { f(x) } def f(x) { g(x) } workload { session { turn; \n} }\nserver { set a = g(1);\n}")
                .contains("`g` uses `f`, which is defined after it")
        );
        assert!(
            err("def g(x) { f(x); } def f(x) { g(x); } workload { session { turn; \n} }\nserver { g(1);\n}")
                .contains("defined after it")
        );
        // a stray closer
        assert!(
            err("def f(x) { x } workload { session { turn; \n} }\nserver { set a = f(1]);\n}")
                .contains("unmatched")
        );
        // a definition that draws draws when it is an argument
        assert!(
            err("def d() { ~exp(1) } def twice(x) { x + x } workload { session { turn; \n} }\nserver { set a = twice(d());\n}")
                .contains("would draw 2 times")
        );
        // an argument the body would capture
        assert!(
            err("def f(x) { set s = 10; observe o = x; } workload { session { turn; \n} }\nserver { f(s + 1);\n}")
                .contains("which `f` assigns")
        );
        // an observation's name is not captured
        parse(&main_source(&format!(
            "{ENGINE} def f(x) {{ observe s = 10; observe o = x; }} workload {{ session {{ turn; \n}} }}\nserver {{ f(s + 1);\n}}"
        )
        ))
        .unwrap();
        assert!(
            err("def f(p) { set p = 1; } workload { session { turn; \n} }\nserver { f(2);\n}")
                .contains("is a parameter")
        );
        assert!(
            err(
                "def f(h) { hold kv (cost(kv, h)) at admission (h = 3) { observe a = h; } } workload { session { turn; \n} }\nserver { f(2);\n}"
            )
            .contains("is a parameter")
        );
        // a parameter may not be an aggregate's index, and a count may be one
        assert!(
            err("def tally(k) { sum k in 2 (k) } workload { session { turn; \n} }\nserver { set x = tally(7);\n}")
                .contains("is a parameter")
        );
        parse(&main_source("def tally(n) { sum k in n (k) } workload { session { turn; \n} }\nserver { set x = tally(2);\n}",
        )).unwrap();
        // a parenthesised count: the body's `k` is still the aggregate's
        parse(&main_source(
            "def tally() { sum k in (1 + 1) (k) } def next(x) { turn; observe p = x; } workload { turn { set k = 1; }
          session { next(tally()); turn; end; }
        } server {}",
        )
        )
        .unwrap();
        // an aggregate's index is its own, not a name the argument reads
        parse(&main_source(
            "def tally() { sum i in 2 (i) } def next(x) { turn; observe p = x; } workload { turn { set i = 1; }
          session { next(tally()); turn; end; }
        } server {}",
        )
        )
        .unwrap();
        // what a turn, a request or an admission assigns is captured too
        assert!(
            err("def next(x) { turn; observe p = x; } workload { turn { set n = 1; } \n  session { next(n); turn; end; } } server {}")
                .contains("which its `turn;` assigns")
        );
        assert!(
            err("def go(x) { turn; observe b = x; } workload { init { set t0 = 0; } session { go(t0); end; } } server { set t0 = now; }")
                .contains("which its `turn;` assigns")
        );
        assert!(
            err("def take(x) { hold kv (cost(kv, 4)) { observe got = x; } } workload { session { turn; \n} }\nserver { take(cached);\n}")
                .contains("which `take` assigns")
        );
        // the clock and live state are read where the body reads them
        assert!(
            err("stage svc : fifo; def timed(t) { run svc (cost(svc, 1)); observe took = now - t; } workload { session { turn; \n} }\nserver { timed(now);\n}")
                .contains("reads `now`, which changes")
        );
        assert!(
            err("def f(q) { observe b = q; } workload { session { turn; \n} }\nserver { f(used(kv));\n}").contains("reads `used(…)`")
        );
        // an expression's argument is read where the expression is
        parse(&main_source(&format!(
            "{ENGINE} def g(x) {{ x + 1 }} workload {{ session {{ turn; \n}} }}\nserver {{ set a = g(now);\n}}"
        )
        ))
        .unwrap();
        // `n` is an attribute when the program sets it
        parse(&main_source(&format!(
            "{ENGINE} def f(x) {{ observe b = x; }} workload {{ session {{ turn; \n}} }}\nserver {{ set n = 1; f(n);\n}}"
        )
        ))
        .unwrap();
        // and through an expression the argument uses
        assert!(
            err("stage svc : fifo; def clock() { now } def timed(t) { run svc (cost(svc, 1)); observe took = now - t; } workload { session { turn; \n} }\nserver { timed(clock());\n}")
                .contains("reads `now`")
        );
        assert!(
            err("def occ(p) { used(p) } def f(q) { observe b = q; } workload { session { turn; \n} }\nserver { f(occ(kv));\n}")
                .contains("reads `used(…)`")
        );
        assert!(
            err("def plus(x) { s + x } def f(v) { set s = 10; observe o = v; } workload { session { turn; \n} }\nserver { f(plus(1));\n}")
                .contains("which `f` assigns")
        );
        // and through a definition the body uses
        assert!(
            err("def reset() { set s = 10; } def f(x) { reset(); observe o = x; } workload { session { turn; \n} }\nserver { f(s + 1);\n}")
                .contains("which `f` assigns")
        );
        assert!(
            err("def adv() { turn; } def next(x) { adv(); observe p = x; } workload { turn { set n = 1; } \n  session { next(n); turn; end; } } server {}")
                .contains("which its `turn;` assigns")
        );
        assert!(
            err("def ask() { turn; } def go(x) { ask(); observe b = x; } workload { session { go(cached); end; } } server { }")
                .contains("which its `turn;` assigns")
        );
        // a name that is a declaration's
        assert!(
            err("def kv(x) { x } workload { session { turn; \n} }\nserver {\n}")
                .contains("also a pool")
        );
        assert!(
            err("stage svc : fifo; def svc(x) { x } workload { session { turn; \n} }\nserver {\n}")
                .contains("also a stage")
        );
        // `engine` is the keyword of an engine's declaration
        assert!(
            err("def engine(x) { x } workload { session { turn; \n} }\nserver {\n}")
                .contains("a word of the language")
        );
        // the name of a statement body's attribute is not a use
        parse(&main_source(&format!(
            "{ENGINE} def c(x) {{ set c = x; }} workload {{ session {{ turn; \n}} }}\nserver {{ c(1);\n}}"
        )
        ))
        .unwrap();
        // an error in the body says where the definition was used
        let e =
            err("def take(n) { turn; } workload { session { turn; end; } } server { take(4); }");
        assert!(e.contains("note: in `take`, used at"), "{e}");
        assert!(
            err("def f(x) { x } def f(y) { y } workload { session { turn; \n} }\nserver {\n}")
                .contains("defined twice")
        );
        // an argument used once may draw
        parse(&main_source(&format!(
            "{ENGINE} def f(x) {{ x + 1 }} workload {{ session {{ turn; \n}} }}\nserver {{ set a = f(~exp(1));\n}}"
        )
        ))
        .unwrap();
        // a def used before it is defined is a call of an unknown function,
        // which the linker reports
        parse(&main_source(&format!(
            "{ENGINE} workload {{ session {{ turn; \n}} }}\nserver {{ set a = f(1);\n}} def f(x) {{ x }}"
        )
        ))
        .unwrap();
    }

    #[test]
    fn a_transfer_says_where_the_kv_goes() {
        // without `from P to Q` it would be a link that stores and forwards,
        // which is the kernel's `run`, not a transfer
        let e = parse(&main_source(&format!(
            "{PD} workload {{ session {{ turn; \n}} }}\nserver {{ hold kv (cost(kv, K)) {{ transfer X; }}\n}}"
        )))
        .unwrap_err();
        assert!(
            e.msg.contains("`transfer` without `from P to Q (n)`"),
            "{e}"
        );
        assert!(e.msg.contains("`run link (cost(link, w));`"), "{e}");
        let e = parse(&main_source(
            "stage link[2] : ps(1); workload { session { turn; \n} }\nserver { transfer[0] X;\n}",
        ))
        .unwrap_err();
        assert!(e.msg.contains("`run link[…] (cost(link, w));`"), "{e}");
        let e = parse(&main_source(
            "pool kv { cap 1; } stage nic : ps(1); workload { session { turn; \n} }\nserver { transfer on nic X growing kv;\n}",
        )
        )
        .unwrap_err();
        assert!(
            e.msg.contains("`transfer` without `from P to Q (n)`"),
            "{e}"
        );
        assert!(e.msg.contains("`run nic (cost(nic, w));`"), "{e}");
    }

    #[test]
    fn serving_forms_need_exactly_one_stage() {
        let e = parse(&main_source(
            "stage svc : fifo; workload { session { turn; \n} }\nserver { tool S;\n}",
        ))
        .unwrap_err();
        assert!(
            e.msg.contains("no stage declared above plays `tool`"),
            "{e}"
        );
        assert_eq!((e.line, e.col), (3, 10));
        let e =
            parse(&main_source("pool a { cap 1; } pool b { cap 1; } stage link : fifo; stage transfer : fifo; workload { session { turn; \n} }\nserver { transfer X from a to b (1);\n}",
        ))
                .unwrap_err();
        assert!(
            e.msg
                .contains("several stages play `transfer` (link, transfer)"),
            "{e}"
        );
        let e = parse(&main_source("stage engine : step { cost 1; } workload { session { turn; \n} }\nserver { transfer X;\n}",
        )).unwrap_err();
        assert!(
            e.msg.contains("no stage declared above plays `transfer`"),
            "{e}"
        );
        let e = parse(&main_source(
            "stage tool : delay; workload { session { turn; \n} }\nserver { tool on other Z;\n}",
        ))
        .unwrap_err();
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

    /// `workload { session { … turn; … } }` and `server { … }` parse to
    /// the session block that has the server in place of the request.
    #[test]
    fn the_two_sides_are_one_session() {
        same(
            &format!(
                "{DEPLOYMENT} workload {{ {CLIENT}
          session {{

            loop {{
              turn;
              set K = prompt + o;
              branch (more) {{ tool 3;  }} else {{ end; }}
            }}
          }}
        }}
        server {{
          set prompt = K + n;
          hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, hit + budget_left(engine))))
          at admission (hit = min(cachedin(kv), prompt - 1)) {{
            run engine prefill (cost(engine, prompt - cached)) growing kv;
            run engine decode (cost(engine, o - 1)) growing kv;
          }} cache (cost(reqs, kv, prompt + o));
        }}"
            ),
            &format!(
                "{DEPLOYMENT} workload {{ {CLIENT}
          session {{

            loop {{ turn;
              branch (more) {{ tool 3;  }} else {{ end; }}
            }}

          }}
        }}
        server {{
          set prompt = K + n;
          hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, hit + budget_left(engine))))
          at admission (hit = min(cachedin(kv), prompt - 1)) {{
            run engine prefill (cost(engine, prompt - cached)) growing kv;
            run engine decode (cost(engine, o - 1)) growing kv;
          }} cache (cost(reqs, kv, prompt + o));
          set K = prompt + o;
        }}"
            ),
        );
    }

    #[test]
    fn request_is_spliced_at_any_depth_and_as_often_as_written() {
        same(
            "stage s : fifo; workload { session { branch (x) { turn; } else { loop { turn; end; } } } }
        server { run s (cost(s, 1)); }",
            "stage s : fifo; workload { \n  session { branch (x) { turn; } else { loop { turn; end; } } \n  }\n} server { run s (cost(s, 1));\n}",
        );
        // the kernel is written on either side
        same(
            "pool kv { cap 1; } workload { session { turn; end; } } server { hold kv (cost(kv, 1)) { } }",
            "pool kv { cap 1; } workload { \n  session { turn; end; \n  }\n} server { hold kv (cost(kv, 1)) { }\n}",
        );
        // the order of the blocks does not matter
        same(
            "stage s : fifo; server { run s (cost(s, 1)); } workload { session { turn; end; } }",
            "stage s : fifo; workload { \n  session { turn; end; \n  }\n} server { run s (cost(s, 1));\n}",
        );
    }

    fn refused(src: &str, needle: &str) {
        let e = parse(&main_source(src)).unwrap_err();
        assert!(e.msg.contains(needle), "{src}\n  {e}");
    }

    #[test]
    fn a_session_belongs_only_inside_workload() {
        for source in [
            "session {}",
            "session { end; }",
            "workload { arrive batch(1); } session { end; }",
            "session {} workload { session { turn; end; } } server {}",
            "workload { session { turn; end; } } server {} session {}",
        ] {
            refused(source, "`session` belongs inside `workload`");
            refused(source, "describe its turns with `turn;`");
        }
    }

    #[test]
    fn each_side_keeps_its_words() {
        const WL: &str = "workload { session { turn; } }";
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
            "`request` is replaced",
        );
        refused(
            "pool kv { cap 1; } session { turn; }",
            "`session` belongs inside `workload`",
        );
    }

    #[test]
    fn one_admission_is_written_one_way() {
        // the words #136 took out say what a program writes instead
        const WL: &str = "workload { session { turn; } }";
        for (src, now) in [
            (
                "workload { session { turn; \n} }\nserver { enter kv (1) { }\n}",
                "`enter` is now `hold`",
            ),
            (
                &*format!("{WL} server {{ admit if kv (1) fit {{ }} }}"),
                "is now `hold … at admission",
            ),
            (
                "workload { session { turn; \n} }\nserver { hold kv (cost(kv, 1)) { } keep (1);\n}",
                "`keep` is now `cache`",
            ),
            (
                "workload { session { turn; \n} }\nserver { hold kv (cost(kv, 1)) where x = 1 { }\n}",
                "`where x = e` is now `at admission (x = e)`",
            ),
            (
                "workload { session { turn; \n} }\nserver { hold kv (cost(kv, 1)) fit { }\n}",
                "`fit` is gone",
            ),
        ] {
            refused(&format!("pool kv {{ cap 1; }} {src}"), now);
        }
        // and none of them names anything, so a name never means two things
        for src in [
            "def keep(n) { observe k = n; } workload { session { turn; end; \n} }\nserver {\n}",
            "def f(where) { where } workload { session { turn; end; \n} }\nserver {\n}",
            "workload { session { turn; end; \n} }\nserver { set fit = 1;\n}",
            "workload { session { turn; end; \n} }\nserver { hold kv (cost(kv, 1)) at admission (enter = 1) { observe e = enter; }\n}",
        ] {
            refused(&format!("pool kv {{ cap 1; }} {src}"), "is a retired word");
        }
        // `hold` is written on either side, with its bindings
        parse(&main_source(&format!(
            "pool kv {{ cap 1; }} {WL} server {{ hold kv (cost(kv, x)) at admission (x = 1) {{ }} cache (cost(kv, 1)); }}"
        )
        ))
        .unwrap();
    }

    #[test]
    fn a_side_needs_the_other() {
        refused(
            "stage s : fifo; workload { session { run s (cost(s, 1)); } }",
            "written against a `server` block",
        );
        refused(
            "stage s : fifo; server { run s (cost(s, 1)); }",
            "`server` needs a `workload`",
        );
        refused(
            "stage s : fifo; workload { session { run s (cost(s, 1)); } } server { run s (cost(s, 1)); }",
            "session has no `turn;`",
        );
        refused(
            "stage s : fifo; workload { session { turn; } } server { run s (cost(s, 1)); } session { run s (cost(s, 1)); }",
            "`session` belongs inside `workload`",
        );
        refused(
            "stage s : fifo; session { run s (cost(s, 1)); } workload { session { turn; } } server { run s (cost(s, 1)); }",
            "`session` belongs inside `workload`",
        );
        refused(
            "stage s : fifo; server { run s (cost(s, 1)); } server { run s (cost(s, 1)); }",
            "duplicate server",
        );
    }
}
