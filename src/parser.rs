//! Recursive-descent parser for seQ programs.
//!
//! ```text
//! program  := item*
//! item     := 'let' IDENT '=' expr ';'
//!           | 'pool' IDENT ('[' NUM ']')? '{' poolopt* '}'
//!           | 'stage' IDENT ('[' NUM ']')? ':' kind ';'
//!           | 'workload' '{' wlitem* '}'
//!           | 'session' block
//!           | 'run' '{' ('horizon' | 'warmup' | 'seed') expr ';' ... '}'
//! poolopt  := 'cap' expr ';' | 'block' expr ';'
//!           | 'evict' ('lru' | 'by' '(' expr (',' expr)* ')') ';'
//!           | 'preempt' ('lifo' | 'none') ';'
//!           | 'queue' ('fifo' | 'by' '(' expr ')') ';'
//!           | 'spill' IDENT 'via' IDENT '(' expr ')' 'when' '(' expr ')' ';'
//! kind     := 'fifo' ('(' expr ')')? | 'ps' '(' expr ')' | 'delay'
//!           | 'step' '{' stepopt* '}'
//! stepopt  := 'budget' expr ';' | 'cost' expr ';' | 'chunk' expr ';'
//!           | 'exclusive' 'prefill' ';' | 'memory' IDENT ';'
//! wlitem   := 'arrive' ('poisson' '(' expr ')' | 'closed' '(' expr ')' | 'none') ';'
//!           | 'trace' STRING ';' | 'init' block | 'turn' block
//! block    := '{' stmt* '}'
//! stmt     := 'turn' ';' | 'set' IDENT '=' expr ';' | 'observe' IDENT '=' expr ';'
//!           | 'hold' ref '(' expr ')' (',' ref '(' expr ')')* block ('cache' '(' expr ')')? ';'?
//!           | 'grow' ref '(' expr ')' ';' | 'drop' ref ';'
//!           | 'run' ref ('prefill' | 'decode')? '(' expr ')' ('growing' ref)? ';'
//!           | 'branch' ('with')? '(' expr ')' block ('else' block)?
//!           | 'loop' block | 'end' ';'
//!           | 'choose' IDENT 'in' expr 'by' '(' expr ')' ';'
//!           | serving
//! serving  := 'admit' ... 'keep' ...            -- as 'hold' ... 'cache' ...
//!           | role ('[' expr ']' | 'on' ref)? expr ('growing' ref)? ';'
//! role     := 'prefill' | 'transfer' | 'decode' | 'tool'
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

use std::fmt;

use crate::ast::*;
use crate::lexer::{LexError, Tok, Token, lex};

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
    };
    let e = p.expr()?;
    p.expect(&Tok::Eof)?;
    Ok(e)
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

    fn ident(&mut self) -> PResult<String> {
        match self.advance() {
            Tok::Ident(s) => Ok(s),
            other => {
                self.pos -= 1;
                self.err(format!("expected identifier, found {other}"))
            }
        }
    }

    fn string(&mut self) -> PResult<String> {
        match self.advance() {
            Tok::Str(s) => Ok(s),
            other => {
                self.pos -= 1;
                self.err(format!("expected string, found {other}"))
            }
        }
    }

    fn program(&mut self) -> PResult<Program> {
        let mut prog = Program::default();
        while *self.peek() != Tok::Eof {
            if self.eat_kw("let") {
                let name = self.ident()?;
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
                prog.session = self.block()?;
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
                        other => return self.err(format!("unknown run option `{other}`")),
                    }
                }
                self.expect(&Tok::RBrace)?;
            } else {
                return self.err(format!("unexpected {} at top level", self.peek()));
            }
        }
        Ok(prog)
    }

    fn array_count(&mut self) -> PResult<usize> {
        if *self.peek() == Tok::LBracket {
            self.advance();
            let n = match self.advance() {
                Tok::Num(x) if x >= 1.0 && x.fract() == 0.0 => x as usize,
                other => {
                    self.pos -= 1;
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
        let name = self.ident()?;
        let count = self.array_count()?;
        self.expect(&Tok::LBrace)?;
        let mut d = PoolDecl {
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
                    d.admit_via = Some(self.ident()?);
                }
                "spill" => {
                    let to = self.ident()?;
                    self.expect_kw("via")?;
                    let via = self.ident()?;
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
                exclusive_prefill: false,
                decode_first: false,
                memory: None,
            };
            let mut has_cost = false;
            while *self.peek() != Tok::RBrace {
                let key = self.ident()?;
                match key.as_str() {
                    "budget" => s.budget = self.expr()?,
                    "cost" => {
                        s.cost = self.expr()?;
                        has_cost = true;
                    }
                    "chunk" => s.chunk = self.expr()?,
                    "exclusive" => {
                        self.expect_kw("prefill")?;
                        s.exclusive_prefill = true;
                    }
                    "decode" => {
                        self.expect_kw("first")?;
                        s.decode_first = true;
                    }
                    "memory" => s.memory = Some(self.ident()?),
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
        Ok(StageDecl { name, count, kind })
    }

    fn workload(&mut self) -> PResult<Workload> {
        self.expect(&Tok::LBrace)?;
        let mut w = Workload {
            arrive: Arrival::None,
            trace: None,
            trace_ordered: false,
            init: vec![],
            turn: vec![],
        };
        while *self.peek() != Tok::RBrace {
            if self.eat_kw("arrive") {
                w.arrive = if self.eat_kw("poisson") {
                    self.expect(&Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(&Tok::RParen)?;
                    Arrival::Poisson(e)
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
            v.push(self.stmt()?);
        }
        self.expect(&Tok::RBrace)?;
        Ok(v)
    }

    fn reference(&mut self) -> PResult<Ref> {
        let name = self.ident()?;
        let index = if *self.peek() == Tok::LBracket {
            self.advance();
            let e = self.expr()?;
            self.expect(&Tok::RBracket)?;
            Some(Box::new(e))
        } else {
            None
        };
        Ok(Ref { name, index })
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
                self.advance();
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Turn)
            }
            "end" => {
                self.advance();
                self.expect(&Tok::Semi)?;
                Ok(Stmt::End)
            }
            "set" => {
                self.advance();
                let name = self.ident()?;
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
            // `admit ... keep (l)` is `hold ... cache (l)`
            "hold" | "admit" => {
                self.advance();
                self.hold()
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
                // `branch (~bernoulli(p))`, which the interpreter already
                // treats identically to a bare `branch (p)`: the sample and
                // the guard draw from the same stream with the same
                // comparison, and the resulting 0/1 then short-circuits the
                // guard without a second draw. So the sugar is free and every
                // existing run is unmoved.
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
                let var = self.ident()?;
                self.expect_kw("in")?;
                let count = self.expr()?;
                self.expect_kw("by")?;
                let key = self.paren_expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Choose { var, count, key })
            }
            other => match Role::of(other) {
                Some(role) => self.serving(role),
                None => self.err(format!("unknown statement `{other}`")),
            },
        }
    }

    /// The rest of a `hold` (or `admit`) statement, after the keyword.
    fn hold(&mut self) -> PResult<Stmt> {
        let mut pools = vec![];
        loop {
            let r = self.reference()?;
            let e = self.paren_expr()?;
            let fits = if self.eat_kw("fits") {
                Some(self.paren_expr()?)
            } else {
                None
            };
            pools.push((r, e, fits));
            if *self.peek() == Tok::Comma {
                self.advance();
            } else {
                break;
            }
        }
        let reuse = if self.eat_kw("reuse") {
            Some(self.paren_expr()?)
        } else {
            None
        };
        let body = self.block()?;
        let cache = if self.eat_kw("cache") || self.eat_kw("keep") {
            Some(self.paren_expr()?)
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
        })
    }

    /// `prefill S;`, `transfer[j] X;`, `decode on E (D) growing kv;`, ...:
    /// a `run` on the stage that plays the role.
    fn serving(&mut self, role: Role) -> PResult<Stmt> {
        let at = self.pos;
        self.advance();
        let kw = role.keyword();
        let stage = if self.eat_kw("on") {
            let r = self.reference()?;
            if !self.stages.iter().any(|(n, _)| *n == r.name) {
                return self.err_at(
                    at,
                    format!(
                        "`{kw} on {}`: no stage `{}` is declared above",
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
            Ref { name, index }
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
        self.expect(&Tok::Semi)?;
        Ok(Stmt::Run {
            stage,
            mode,
            work,
            growing,
        })
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
                Ok(Expr::Sample(name, args))
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
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => {
                self.pos -= 1;
                self.err(format!("expected an expression, found {other}"))
            }
        }
    }

    /// A call argument. `name` followed by `[`, `,` or `)` and not
    /// otherwise an operator is parsed as a reference; the linker decides
    /// whether it names a pool, a stage or a variable.
    fn arg(&mut self) -> PResult<Arg> {
        if let Tok::Ident(name) = self.peek().clone() {
            match self.peek_at(1) {
                Tok::Comma | Tok::RParen => {
                    self.advance();
                    return Ok(Arg::Ref(Ref { name, index: None }));
                }
                Tok::LBracket => {
                    self.advance();
                    self.advance();
                    let e = self.expr()?;
                    self.expect(&Tok::RBracket)?;
                    return Ok(Arg::Ref(Ref {
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
                branch (0.9) { run tool (Z); turn; } else { end; }
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
        assert_eq!(parse(a).unwrap(), parse(b).unwrap());
    }

    #[test]
    fn serving_forms_desugar_to_the_kernel() {
        same(
            &format!(
                "{PD} session {{
                    admit kv (K) {{ prefill S; transfer X; }} keep (K);
                    admit kv (K) fits (F) reuse (R) {{ decode D; }}
                    branch (p) {{ tool Z; turn; }} else {{ end; }}
                }}"
            ),
            &format!(
                "{PD} session {{
                    hold kv (K) {{ run prefill (S); run link (X); }} cache (K);
                    hold kv (K) fits (F) reuse (R) {{ run decode (D); }}
                    branch (p) {{ run tool (Z); turn; }} else {{ end; }}
                }}"
            ),
        );
    }

    #[test]
    fn serving_forms_on_a_step_engine() {
        same(
            &format!(
                "{ENGINE} session {{
                    admit kv (c) {{ prefill (n) growing kv; decode (o - 1) growing kv; }} keep (c);
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
            "stage transfer : fifo; session { transfer X; }",
            "stage transfer : fifo; session { run transfer (X); }",
        );
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
}
