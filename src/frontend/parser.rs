//! Recursive-descent parser for seQ programs.
//!
//! ```text
//! program  := item*
//! item     := 'let' IDENT '=' expr ';'
//!           | 'pool' IDENT ('[' NUM ']')? '{' poolopt* '}'
//!           | 'stage' IDENT ('[' NUM ']')? ':' kind ';'
//!           | 'workload' '{' wlitem* '}'
//!           | 'session' block
//!           | 'server' block
//!           | 'run' '{' ('horizon' | 'warmup' | 'seed' | 'arrivals') expr ';' ... '}'
//! poolopt  := 'cap' expr ';' | 'block' expr ';'
//!           | 'evict' ('lru' | 'by' '(' expr (',' expr)* ')') ';'
//!           | 'preempt' ('lifo' | 'none') ';'
//!           | 'queue' ('fifo' | 'by' '(' expr ')') ';'
//!           | 'spill' IDENT 'via' IDENT '(' expr ')' 'when' '(' expr ')' ';'
//! kind     := 'fifo' ('(' expr ')')? | 'ps' '(' expr ')' | 'delay'
//!           | 'step' '{' stepopt* '}'
//! stepopt  := 'budget' expr ';' | 'cost' expr ';' | 'chunk' expr ';'
//!           | 'serve' ('admission' | 'decode' 'first' | 'exclusive' 'prefill'
//!                     | 'by' '(' expr (',' expr)* ')') ';'
//!           | 'memory' IDENT ';'
//! wlitem   := 'arrive' ('poisson' '(' expr ')' | 'renewal' '(' expr ')' | 'closed' '(' expr ')' | 'batch' '(' expr ')' | 'none') ';'
//!           | 'trace' STRING ('ordered')? ';' | 'init' block | 'turn' block
//!           | 'session' block                  -- the session's side, with 'request'
//!           | 'hidden' IDENT (',' IDENT)* ';'
//! block    := '{' stmt* '}'
//! stmt     := 'turn' ';' | 'request' ';' | 'set' IDENT '=' expr ';' | 'observe' IDENT '=' expr ';'
//!           | ('hold' | 'enter') ref '(' expr ')' (',' ref '(' expr ')')* block ('cache' '(' expr ')')? ';'?
//!           | 'admit' 'if' ref '(' expr ')' (',' ref '(' expr ')')* 'fit'
//!                 ('where' IDENT '=' expr (',' IDENT '=' expr)*)? block ('keep' '(' expr ')')? ';'?
//!           | 'grow' ref '(' expr ')' ';' | 'drop' ref ';'
//!           | 'run' ref ('prefill' | 'decode')? '(' expr ')' ('growing' ref)? ';'
//!           | 'branch' ('with')? '(' expr ')' block ('else' block)?
//!           | 'loop' block | 'end' ';'
//!           | 'choose' IDENT 'in' expr 'by' '(' expr ')' ';'
//!           | serving
//! serving  := 'enter' ... 'keep' ...            -- as 'hold' ... 'cache' ...
//!           | role ('[' expr ']' | 'on' ref)? expr ('growing' ref)? ';'
//!           | 'transfer' ('[' expr ']' | 'on' ref)? expr 'from' ref 'to' ref '(' expr ')' ';'
//! role     := 'prefill' | 'decode' | 'tool'
//! ref      := IDENT ('[' expr ']')?
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
//! the session's and are refused in a `server`; `admit if … fit where …`
//! is the server's spelling of `enter … at admission (…)` and is refused
//! outside one. `hold`, the kernel, is written anywhere.

use std::fmt;

use crate::frontend::ast::*;
use crate::frontend::lexer::{LexError, Tok, Token, lex};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

impl ParseError {
    pub fn render(&self, source: &str) -> String {
        Span {
            line: self.line,
            col: self.col,
            len: 1,
        }
        .render(source, &self.msg)
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            line: e.line,
            col: e.col,
            msg: e.msg,
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
}

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
    let toks = lex(src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        stages: vec![],
        definitions: vec![],
        side: Side::Session,
        wl_session: None,
        server: None,
    };
    p.program()
}

/// Parse a standalone expression (used by `--set name=expr` on the CLI).
pub fn parse_expr(src: &str) -> PResult<Expr> {
    let toks = lex(src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        stages: vec![],
        definitions: vec![],
        side: Side::Session,
        wl_session: None,
        server: None,
    };
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

impl Parser {
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

    fn err_at<T>(&self, pos: usize, msg: impl Into<String>) -> PResult<T> {
        let t = &self.toks[pos];
        Err(ParseError {
            line: t.line,
            col: t.col,
            msg: msg.into(),
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
        }
    }

    fn definition(&mut self) -> PResult<String> {
        let span = self.span();
        let name = self.ident()?;
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
            if self.eat_kw("let") {
                let name = self.definition()?;
                self.expect(&Tok::Assign)?;
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                prog.lets.push((name, e));
            } else if self.eat_kw("pool") {
                prog.pools.push(self.pool()?);
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
        }
        self.assemble(&mut prog)?;
        prog.definitions = std::mem::take(&mut self.definitions);
        Ok(prog)
    }

    /// Put the two sides together: the server's statements in place of
    /// every `request;` of the workload's session, which becomes the
    /// program's session. A side without the other is an error, and so is a
    /// third session at top level.
    fn assemble(&mut self, prog: &mut Program) -> PResult<()> {
        match (self.wl_session.take(), self.server.take()) {
            (None, None) => Ok(()),
            (Some((at, _)), None) => self.err_at(
                at,
                "a `session` inside `workload` is written against a `server` block; \
                 without one, write `session` at top level",
            ),
            (None, Some((at, _))) => self.err_at(
                at,
                "`server` needs a `session` inside `workload` that says `request;`",
            ),
            (Some((s_at, mut session)), Some((v_at, server))) => {
                if !prog.session.is_empty() {
                    return self.err_at(
                        s_at,
                        "a program has one session: inside `workload` (with a `server`) \
                         or at top level, not both",
                    );
                }
                if splice(&mut session, &server) == 0 {
                    return self.err_at(
                        v_at,
                        "`server` is never requested: the workload's session has no `request;`",
                    );
                }
                prog.session = session;
                Ok(())
            }
        }
    }

    fn array_count(&mut self) -> PResult<usize> {
        if *self.peek() == Tok::LBracket {
            self.advance();
            let at = self.pos;
            let n = match self.advance() {
                Tok::Num(x) if x >= 1.0 && x.fract() == 0.0 => x as usize,
                other => {
                    self.pos = at;
                    return self.err(format!(
                        "array size must be a positive integer, found {other}"
                    ));
                }
            };
            self.expect(&Tok::RBracket)?;
            Ok(n)
        } else {
            Ok(1)
        }
    }

    fn pool(&mut self) -> PResult<PoolDecl> {
        let span = Some(self.span());
        let name = self.ident()?;
        let count = self.array_count()?;
        self.expect(&Tok::LBrace)?;
        let mut d = PoolDecl {
            span,
            name,
            count,
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
                        let e = self.expr()?;
                        self.expect(&Tok::RParen)?;
                        QueueOrder::By(e)
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
        let count = self.array_count()?;
        self.expect(&Tok::Colon)?;
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
                                "`serve` takes `admission`, `decode first`, `exclusive prefill` or `by (keys)`, found {}",
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
            StageKind::Step(s)
        } else {
            return self.err(format!("unknown stage kind {}", self.peek()));
        };
        // `step { ... }` needs no semicolon
        if *self.peek() == Tok::Semi || !matches!(kind, StageKind::Step(_)) {
            self.expect(&Tok::Semi)?;
        }
        Ok(StageDecl {
            span,
            name,
            count,
            kind,
        })
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
        if let Tok::Ident(s) = self.peek()
            && let Some(role) = Role::of(s)
        {
            out.extend(self.serving(role)?);
            return Ok(());
        }
        out.push(self.stmt()?);
        Ok(())
    }

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
        Ok(Ref { span, name, index })
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
                        return self
                            .err("`request` inside `server`: a server does not request itself");
                    }
                    Side::Session => {
                        return self.err(
                            "`request` is a statement of the `session` inside `workload`, \
                             next to a `server` block",
                        );
                    }
                }
                self.advance();
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Request)
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
            // `enter ... keep (l)` is `hold ... cache (l)`
            "hold" | "enter" => {
                if kw == "enter" && self.side == Side::Server {
                    return self.err(
                        "`enter` is the session's word; in a `server` block write \
                         `admit if … fit`",
                    );
                }
                self.advance();
                self.hold(false)
            }
            // `admit if P (u), … fit where x = e { body } keep (l)` is the
            // server's spelling of `enter P (u), … at admission (x = e) { body }
            // keep (l)`: in a server the header is the admission, so the
            // clause does not have to say when.
            "admit" => {
                if self.side != Side::Server {
                    return self.err(
                        "`admit` is the server's word: the scheduler admits, the session \
                         enters. In a `session` block write `enter`, in a `server` block \
                         `admit if … fit` (the pool option `admit via` is unchanged)",
                    );
                }
                self.advance();
                if !self.eat_kw("if") {
                    return self.err(format!(
                        "expected `if` after `admit` (`admit if reqs (1), kv (u) fit …`), found {}",
                        self.peek()
                    ));
                }
                self.hold(true)
            }
            "grow" => {
                self.advance();
                let r = self.reference()?;
                let e = self.paren_expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Grow(r, e))
            }
            "drop" => {
                self.advance();
                let r = self.reference()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Drop(r))
            }
            "release" => {
                self.advance();
                let r = self.reference()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Release(r))
            }
            "load" => {
                self.advance();
                let r = self.reference()?;
                let e = self.paren_expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Load(r, e))
            }
            "run" => {
                self.advance();
                let stage = self.reference()?;
                let mode = if self.eat_kw("prefill") {
                    RunMode::Prefill
                } else if self.eat_kw("decode") {
                    RunMode::Decode
                } else {
                    RunMode::Plain
                };
                let work = self.paren_expr()?;
                let growing = if self.eat_kw("growing") {
                    Some(self.reference()?)
                } else {
                    None
                };
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing,
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
                let key = self.paren_expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Choose { var, count, key })
            }
            // the serving forms (`prefill`, `transfer`, `decode`, `tool`) are
            // parsed by `stmt_into`, since one of them stands for several
            // statements
            other => self.err(format!("unknown statement `{other}`")),
        }
    }

    /// The rest of a `hold` or `enter` statement after the keyword, or of
    /// `admit if` (`admit`) after the `if`.
    fn hold(&mut self, admit: bool) -> PResult<Stmt> {
        let mut pools = vec![];
        loop {
            let r = self.reference()?;
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
        if admit && !self.eat_kw("fit") {
            return self.err(format!(
                "expected `fit` after the pools of `admit if`, found {}",
                self.peek()
            ));
        }
        let mut reuse = if self.eat_kw("reuse") {
            Some(self.paren_expr()?)
        } else {
            None
        };
        // `at admission (hit = e, ...)` (`where hit = e, ...` in a server)
        // names values the header is written in terms of. Everything in a
        // hold's header is evaluated when the session is admitted; a `set`
        // above the hold is not, and looks the same. The bindings are
        // substituted into the header's expressions here, so the AST, the
        // IR and the interpreter never see them.
        let binds = if admit {
            if self.is_kw("at") {
                return self
                    .err("in `admit if … fit` the header is the admission: write `where hit = …`");
            }
            if self.eat_kw("where") {
                self.bindings("where")?
            } else {
                vec![]
            }
        } else {
            if self.is_kw("where") {
                return self.err(
                    "`where` is the server's clause; in `enter` and `hold` write \
                     `at admission (hit = …)`",
                );
            }
            self.at_admission()?
        };
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
        let body = self.block()?;
        let mut cache = if self.eat_kw("cache") || self.eat_kw("keep") {
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
            let r = self.reference()?;
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

    /// `name = e, name = f, …`: the bindings of an `at admission (…)` or a
    /// `where` clause, after its keyword. `clause` names it in errors.
    fn bindings(&mut self, clause: &str) -> PResult<Vec<(String, Expr)>> {
        let mut binds: Vec<(String, Expr)> = vec![];
        loop {
            let name = self.ident()?;
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
        Ok(binds)
    }

    /// `prefill S;`, `transfer[j] X from P to Q (n);`, `decode on E (D)
    /// growing kv;`, ...: a `run` on the stage that plays the role.
    fn serving(&mut self, role: Role) -> PResult<Vec<Stmt>> {
        let at = self.pos;
        let span = Some(self.span());
        self.advance();
        let kw = role.keyword();
        let stage = if self.eat_kw("on") {
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
            r
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
            Some(self.reference()?)
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
            let from = self.reference()?;
            self.expect_kw("to")?;
            let to = self.reference()?;
            let units = self.paren_expr()?;
            self.expect(&Tok::Semi)?;
            return Ok(vec![
                Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing: None,
                },
                Stmt::Load(to, units),
                Stmt::Release(from),
            ]);
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
                    "`transfer` without `from P to Q (n)`: a KV transfer leaves the hold on P and enters the hold on Q\n\
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
        }])
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
        let at = self.pos;
        let span = self.span();
        match self.advance() {
            Tok::Num(x) => Ok(Expr::Num(x)),
            Tok::LParen => {
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
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
                } else {
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
                Tok::LBracket => {
                    self.advance();
                    self.advance();
                    let e = self.expr()?;
                    self.expect(&Tok::RBracket)?;
                    return Ok(Arg::Ref(Ref {
                        span,
                        name,
                        index: Some(Box::new(e)),
                    }));
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
            stage decode : ps(min(n, 8));
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
        stage decode : ps(n);
        stage tool : delay;
    "#;

    const ENGINE: &str = r#"
        pool kv { cap 100; }
        stage engine : step { cost 1; }
        stage tool : delay;
    "#;

    fn same(a: &str, b: &str) {
        assert_eq!(
            without_locations(parse(a).unwrap()),
            without_locations(parse(b).unwrap())
        );
    }

    #[test]
    fn serving_forms_desugar_to_the_kernel() {
        same(
            &format!(
                "{PD} session {{
                    enter kv (K) {{ prefill S; }} keep (K) lease kv (inf);
                    enter kvD (K) {{ transfer X from kv to kvD (K); }}
                    enter kv (K) reserve (F) reuse (R) {{ decode D; }}
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
                    enter kv (c) {{ prefill (n) growing kv; decode (o - 1) growing kv; }} keep (c);
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
    fn a_transfer_says_where_the_kv_goes() {
        // without `from P to Q` it would be a link that stores and forwards,
        // which is the kernel's `run`, not a transfer
        let e = parse(&format!(
            "{PD} session {{ enter kv (K) {{ transfer X; }} }}"
        ))
        .unwrap_err();
        assert!(
            e.msg.contains("`transfer` without `from P to Q (n)`"),
            "{e}"
        );
        assert!(e.msg.contains("`run link (w);`"), "{e}");
        let e = parse("stage link[2] : ps(1); session { transfer[0] X; }").unwrap_err();
        assert!(e.msg.contains("`run link[…] (w);`"), "{e}");
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
    /// the session block that has the server in place of the request, and
    /// `admit if … fit where …` to the `enter … at admission (…)` it spells.
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
                  admit if reqs (1), kv (min(prompt, hit + budget_left(engine))) fit
                        where hit = min(cachedin(kv), prompt - 1) {{
                    prefill (prompt - cached) growing kv;
                    decode (o - 1) growing kv;
                  }} keep (prompt + o);
                }}"
            ),
            &format!(
                "{DEPLOYMENT} workload {{ {CLIENT} }}
                session {{
                  turn;
                  loop {{
                    set prompt = K + n;
                    enter reqs (1), kv (min(prompt, hit + budget_left(engine)))
                          at admission (hit = min(cachedin(kv), prompt - 1)) {{
                      prefill (prompt - cached) growing kv;
                      decode (o - 1) growing kv;
                    }} keep (prompt + o);
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
            &format!("pool kv {{ cap 1; }} {WL} server {{ enter kv (1) {{ }} }}"),
            "`enter` is the session's word",
        );
        // the server's words in a session
        refused(
            "pool kv { cap 1; } session { admit if kv (1) fit { } }",
            "`admit` is the server's word",
        );
        refused(
            "pool kv { cap 1; } session { request; }",
            "`session` inside `workload`",
        );
        refused(
            "pool kv { cap 1; } session { enter kv (1) where x = 1 { } }",
            "`where` is the server's clause",
        );
        // and the server's form is one form
        refused(
            &format!(
                "pool kv {{ cap 1; }} {WL} server {{ admit if kv (1) fit at admission (x = 1) {{ }} }}"
            ),
            "the header is the admission",
        );
        refused(
            &format!("pool kv {{ cap 1; }} {WL} server {{ admit kv (1) {{ }} }}"),
            "expected `if` after `admit`",
        );
        refused(
            &format!("pool kv {{ cap 1; }} {WL} server {{ admit if kv (1) {{ }} }}"),
            "expected `fit`",
        );
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
