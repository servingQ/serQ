//! Engines on devices (`docs/design/engine-device.md`): `device`, `engine …
//! on`, `pool … on`, an engine's `schedule` and `execute`. All of it is
//! parse-time sugar: an engine is a step stage, a pool on a device or an
//! engine is a pool, and the links between them (`memory`, `admit via`,
//! `waiting.count`) are written once every declaration is read.

use super::*;

/// `device NAME [N] { RESOURCE (PARAM) = expr; RESOURCE cap expr; }`
pub(super) struct DeviceDecl {
    pub name: String,
    pub count: usize,
    pub array: bool,
    /// Time resources: a demand to time, expanded as a `def` is, and only
    /// inside an engine's `execute`.
    pub times: Vec<Def>,
    /// Capacities, each declared as a pool with `pool NAME on DEVICE`.
    pub caps: Vec<Capacity>,
}

/// A capacity of a device or an engine, and where it is written.
pub(super) struct Capacity {
    pub name: String,
    pub cap: Expr,
    pub at: usize,
}

/// `engine NAME [N] on DEVICE { … }`, read as the step stage `stages[stage]`.
pub(super) struct EngineDecl {
    pub name: String,
    pub at: usize,
    pub device: String,
    pub stage: usize,
    pub caps: Vec<Capacity>,
}

/// What `pool NAME on …` names: the device or engine that owns the
/// capacity and where it is written.
pub(super) struct On {
    pub owner: String,
    pub at: usize,
    /// `on ENGINE.DEVICE`: the engine whose scheduler admits the pool's
    /// queue. `on DEVICE` admits a waiting hold as soon as it fits.
    pub admitted_by: Option<String>,
}

/// `pool NAME on OWNER { … }`: `prog.pools[pool]` is OWNER's capacity NAME.
pub(super) struct PoolOn {
    pub pool: usize,
    /// The capacity's name: the pool's, before a queue prefixes it.
    pub cap: String,
    pub on: On,
}

/// A statement of a schedule, before it is written as the kernel's.
enum SStmt {
    /// `advance running [only (p)] [ORDER] [each at most (e)];`
    Advance {
        only: Option<Expr>,
        order: Option<Serve>,
        cap: Option<Expr>,
    },
    /// `admit waiting [only (p)] [while (e)] [each at most (e)];`
    Admit {
        only: Option<Expr>,
        gate: Option<Expr>,
        cap: Option<Expr>,
    },
    /// `exclusive prefill [each at most (e)];`
    Exclusive {
        cap: Option<Expr>,
    },
    Branch(Expr, Vec<SStmt>, Vec<SStmt>),
    Set(String, Expr),
}

/// A schedule's `let`s, in order.
type Lets = Vec<(String, Expr)>;

/// The clauses that read the residents as they stand: as the iteration
/// starts (`tokens cap`, `each at most` and the `let`s it reads), at each
/// resident's turn (`only`, `by`) and in the schedule's statements.
const AS_THEY_STAND: &[&str] = &["tokens cap", "each at most", "only", "by", "schedule"];

/// Those, and `execute`, which reads the count of the residents too.
const RESIDENTS: &[&str] = &[
    "tokens cap",
    "each at most",
    "only",
    "by",
    "schedule",
    "execute",
];

/// An engine's values: the name it reads one by, the kernel's context
/// variable, and the clauses that read it. `running` is the residents as
/// they stand, `waiting` the queues, `batch` the iteration's batch: in a
/// schedule's statements (`schedule`), what it has formed so far; in
/// `execute`, what it formed. The kernel reads `decoders` and `kv_decode` as
/// the residents' before the batch is formed and as the batch's in `cost`
/// (#416), so the engine names them twice. Within a schedule, `only`, `by`
/// and `each at most` are read at their own moments, not the statements'.
const ENGINE_VALUES: [(&str, &str, &[&str]); 13] = [
    ("running.count", "residents", RESIDENTS),
    ("running.decoding", "decoders", AS_THEY_STAND),
    ("running.kv_decode", "kv_decode", AS_THEY_STAND),
    ("running.kv_prefill", "kv_prefill", AS_THEY_STAND),
    ("running.preempted", "preempted", &["schedule"]),
    ("waiting.count", WAITING_COUNT, RESIDENTS),
    ("waiting.admitted", "admitted", &["schedule"]),
    ("batch.tokens", "tokens", &["schedule", "execute"]),
    ("batch.prefilled", "prefilled", &["schedule", "execute"]),
    ("batch.decoding", "decoders", &["execute"]),
    ("batch.kv_decode", "kv_decode", &["execute"]),
    ("batch.kv_prefill", "kv_prefill", &["execute"]),
    ("batch.attention", "attention", &["execute"]),
];

/// The name an engine reads the kernel's context variable `var` by in
/// `clause` (`tokens cap`, `schedule`, `execute`), if it reads it there: a
/// view that draws a step stage in an engine's words names it so.
pub fn engine_name(var: &str, clause: &str) -> Option<&'static str> {
    ENGINE_VALUES
        .iter()
        .find(|(_, k, cs)| *k == var && cs.contains(&clause))
        .map(|(v, ..)| *v)
}

/// Why `value` of `ENGINE_VALUES` is not read in `clause`.
fn not_read_in(value: &str, clause: &str) -> String {
    let (list, field) = value.split_once('.').expect("a list's value");
    if field == "preempted" || field == "admitted" {
        format!(
            "`{value}` is what the iteration's schedule did, read in its `branch`, `while` and \
             `set`, not in `{clause}`"
        )
    } else if list == "running" {
        format!(
            "`execute` times the batch: `{value}` counts the residents, in the batch or not, \
             and the batch's is `batch.{field}`"
        )
    } else if clause == "schedule" {
        format!(
            "`{value}` is the formed batch's, read in `execute`; a schedule's statements read \
             the batch so far as `batch.tokens` and `batch.prefilled`, and the residents' as \
             `running.…`"
        )
    } else {
        let so_far = if ["tokens", "prefilled"].contains(&field) {
            ", and as the batch so far in a schedule's `branch`, `while` and `set`"
        } else {
            ""
        };
        format!(
            "`{clause}` is read before the batch is formed, so it reads no `batch.…`: `{value}` \
             is read in `execute`{so_far}"
        )
    }
}

/// `waiting.count` until the pools the engine admits are known.
pub(super) const WAITING_COUNT: &str = "waiting.count";

impl Parser {
    /// `device` after the keyword.
    pub(super) fn device(&mut self, prog: &Program) -> PResult<()> {
        let at = self.pos;
        let name = self.ident()?;
        if KEYWORDS.contains(&name.as_str()) {
            return self.err_at(at, format!("`{name}` is a word of the language"));
        }
        // a queue is named whether or not it has a stage (a gateway has none)
        if self.devices.iter().any(|d| d.name == name)
            || prog.pools.iter().any(|p| p.name == name)
            || prog.stages.iter().any(|s| s.name == name)
            || self.queues.iter().any(|q| q.name == name)
        {
            return self.err_at(at, format!("`{name}` is declared twice"));
        }
        let array = self.array_count()?;
        self.device_body(name, array.unwrap_or(1), array.is_some())
    }

    /// `{ resource* }` of a device named `name` (a queue's is `Q.name`), a
    /// family of `count` when `array`.
    pub(super) fn device_body(&mut self, name: String, count: usize, array: bool) -> PResult<()> {
        self.expect(&Tok::LBrace)?;
        let mut d = DeviceDecl {
            name,
            count,
            array,
            times: vec![],
            caps: vec![],
        };
        while *self.peek() != Tok::RBrace {
            let r_at = self.pos;
            let r = self.ident()?;
            if KEYWORDS.contains(&r.as_str())
                || is_function(&r)
                || DISTRIBUTIONS.contains(&r.as_str())
                || self.defs.iter().any(|def| def.name == r)
            {
                return self.err_at(
                    r_at,
                    format!("`{r}` is already a word or a definition: name the resource otherwise"),
                );
            }
            if d.times.iter().any(|t| t.name == r) || d.caps.iter().any(|c| c.name == r) {
                return self.err_at(
                    r_at,
                    format!("`{r}` is a resource of {} twice", shown(&d.name)),
                );
            }
            if *self.peek() == Tok::LParen {
                d.times.push(self.time_resource(r_at, r)?);
            } else {
                if !self.eat_kw("cap") {
                    return self.err(format!(
                        "a device's resource is a time, `{r} (x) = expr;`, or a capacity, \
                         `{r} cap expr;`; found {}",
                        self.peek()
                    ));
                }
                let cap = self.expr()?;
                self.expect(&Tok::Semi)?;
                d.caps.push(Capacity {
                    name: r,
                    cap,
                    at: r_at,
                });
            }
        }
        self.expect(&Tok::RBrace)?;
        self.devices.push(d);
        Ok(())
    }

    /// `(PARAM, …) = expr ;` of a time resource named `name`: a definition
    /// an engine's `execute` expands.
    fn time_resource(&mut self, at: usize, name: String) -> PResult<Def> {
        self.expect(&Tok::LParen)?;
        let mut params: Vec<String> = vec![];
        while *self.peek() != Tok::RParen {
            let p_at = self.pos;
            let p = self.name()?;
            if KEYWORDS.contains(&p.as_str()) || is_function(&p) {
                return self.err_at(p_at, format!("`{p}` is a word of the language"));
            }
            if params.contains(&p) {
                return self.err_at(p_at, format!("`{p}` is a parameter twice"));
            }
            params.push(p);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::Assign)?;
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Tok::Eof => return self.err_at(at, format!("the resource `{name}` is not closed")),
                Tok::Semi if depth == 0 => break,
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
        let body = self.toks[start..self.pos].to_vec();
        self.expect(&Tok::Semi)?;
        if body.is_empty() {
            return self.err_at(at, format!("the resource `{name}` has no expression"));
        }
        let draws = body.iter().any(|t| t.tok == Tok::Tilde) || self.draws_through(&body);
        let (mut reads, calls) = self.reads_of(&body, true);
        reads.retain(|n| !params.contains(n));
        Ok(Def {
            line: self.toks[at].line,
            col: self.toks[at].col,
            file: self.toks[at].file,
            name,
            params,
            stmts: false,
            body,
            draws,
            assigns: vec![],
            entry_calls: vec![],
            reads,
            calls,
            turn: false,
        })
    }

    /// Whether `name` is a time resource of some device: read outside an
    /// engine's `execute`, where it is not defined, it is an error.
    pub(super) fn is_time_resource(&self, name: &str) -> bool {
        self.devices
            .iter()
            .any(|d| d.times.iter().any(|t| t.name == name))
    }

    /// Whether the next tokens begin `engine NAME on` or `engine NAME[`.
    pub(super) fn at_engine(&self) -> bool {
        self.is_kw("engine")
            && matches!(self.peek_at(1), Tok::Ident(_))
            && (matches!(self.peek_at(2), Tok::Ident(k) if k == "on")
                || *self.peek_at(2) == Tok::LBracket)
    }

    /// `engine NAME [N] on DEVICE { … }` after the keyword: a step stage.
    pub(super) fn engine(&mut self, prog: &mut Program) -> PResult<()> {
        let span = Some(self.span());
        let at = self.pos;
        let name = self.ident()?;
        if KEYWORDS.contains(&name.as_str()) {
            return self.err_at(at, format!("`{name}` is a word of the language"));
        }
        if prog.stages.iter().any(|s| s.name == name)
            || prog.pools.iter().any(|p| p.name == name)
            || self.devices.iter().any(|d| d.name == name)
        {
            return self.err_at(at, format!("`{name}` is declared twice"));
        }
        let array = self.array_count()?;
        self.expect_kw("on")?;
        let d_at = self.pos;
        let device = self.ident()?;
        let Some(dev) = self.devices.iter().position(|d| d.name == device) else {
            return self.err_at(
                d_at,
                format!(
                    "no device `{device}`: declare `device {device} {{ … }}` before the engine"
                ),
            );
        };
        let (dev_count, dev_array) = (self.devices[dev].count, self.devices[dev].array);
        match (array, dev_array) {
            (Some(n), true) if n == dev_count => {}
            (None, false) => {}
            _ => {
                return self.err_at(
                    at,
                    format!(
                        "an engine is one per device: `{device}` is {}, so write `engine {name}{}`",
                        if dev_array {
                            format!("a family of {dev_count}")
                        } else {
                            "one device".into()
                        },
                        if dev_array {
                            format!("[{dev_count}] on {device}")
                        } else {
                            format!(" on {device}")
                        }
                    ),
                );
            }
        }
        self.engine_body(prog, span, at, name, dev)
    }

    /// `{ … }` of the engine `name` on `devices[dev]`, with the device's
    /// family; a queue's engine is named after the queue.
    pub(super) fn engine_body(
        &mut self,
        prog: &mut Program,
        span: Option<Span>,
        at: usize,
        name: String,
        dev: usize,
    ) -> PResult<()> {
        let (dev_count, dev_array) = (self.devices[dev].count, self.devices[dev].array);
        let device = self.devices[dev].name.clone();
        if let Some(other) = self.engines.iter().find(|e| e.device == device) {
            return self.err_at(
                at,
                format!(
                    "`{}` already runs on `{device}`: one device runs one engine, which admits \
                     its pools",
                    other.name
                ),
            );
        }
        self.expect(&Tok::LBrace)?;
        let mut s = StepSpec {
            budget: Expr::Num(f64::INFINITY),
            cost: Expr::Num(0.0),
            chunk: Expr::Var("inf".into()),
            per_run: true,
            granule: None,
            serve: Serve::Admission,
            only: None,
            memory: None,
            iteration: None,
            state: vec![],
        };
        let mut caps: Vec<Capacity> = vec![];
        let (mut tokens, mut schedule, mut execute) = (false, false, false);
        while *self.peek() != Tok::RBrace {
            let k_at = self.pos;
            if self.eat_kw("tokens") {
                if tokens {
                    return self.err_at(k_at, "`tokens cap` twice: an iteration has one budget");
                }
                tokens = true;
                self.expect_kw("cap")?;
                let e_at = self.pos;
                let e = self.engine_expr("tokens cap", |p| p.expr())?;
                if self.const_value(&e).is_some_and(|v| v <= 0.0) {
                    return self.err_at(
                        e_at,
                        "`tokens cap` at or below 0: no iteration could compute a token",
                    );
                }
                s.budget = e;
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("granule") {
                s.granule = Some(self.expr()?);
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("state") {
                let n = self.ident()?;
                self.expect(&Tok::Assign)?;
                s.state.push((n, self.expr()?));
                self.expect(&Tok::Semi)?;
            } else if self.eat_kw("schedule") {
                if schedule {
                    return self.err_at(k_at, "`schedule` twice: an engine has one schedule");
                }
                schedule = true;
                self.schedule(&mut s)?;
            } else if self.eat_kw("execute") {
                if execute {
                    return self.err_at(k_at, "`execute` twice: a batch takes one time");
                }
                execute = true;
                self.expect(&Tok::LParen)?;
                // the device's time resources are definitions here, and only here
                let mark = self.defs.len();
                let times: Vec<Def> = self.devices[dev].times.clone();
                self.defs.extend(times);
                let e = self.engine_expr("execute", |p| p.expr());
                self.defs.truncate(mark);
                s.cost = e?;
                self.expect(&Tok::RParen)?;
                self.expect(&Tok::Semi)?;
            } else if let Some(now) = [
                ("budget", "`tokens cap B;`"),
                ("chunk", "`each at most (e)` on the schedule's statements"),
                ("cost", "`execute (T);`"),
                (
                    "memory",
                    "nothing: the engine's KV is the pool on its device",
                ),
                ("serve", "a statement of `schedule { … }`"),
                ("iteration", "`schedule { … }`"),
            ]
            .iter()
            .find(|(w, _)| self.is_kw(w))
            .map(|(_, now)| *now)
            {
                return self.err(format!(
                    "an engine writes this as {now} (docs/design/engine-device.md)"
                ));
            } else {
                let r = self.ident()?;
                if KEYWORDS.contains(&r.as_str()) {
                    return self.err_at(k_at, format!("`{r}` is a word of the language"));
                }
                if !self.eat_kw("cap") {
                    return self.err(format!(
                        "an engine holds capacities (`{r} cap expr;`), `tokens cap`, `granule`, \
                         `state`, `schedule` and `execute`; found {}",
                        self.peek()
                    ));
                }
                if caps.iter().any(|c| c.name == r) {
                    return self.err_at(k_at, format!("`{r}` is a capacity of `{name}` twice"));
                }
                let cap = self.expr()?;
                self.expect(&Tok::Semi)?;
                caps.push(Capacity {
                    name: r,
                    cap,
                    at: k_at,
                });
            }
        }
        self.expect(&Tok::RBrace)?;
        for (has, what) in [
            (
                tokens,
                "`tokens cap B;`: the tokens one iteration computes (`tokens cap inf;` for none)",
            ),
            (schedule, "`schedule { … }`: what each iteration does"),
            (execute, "`execute (T);`: how long a batch takes"),
        ] {
            if !has {
                return self.err_at(at, format!("engine `{name}` needs {what}"));
            }
        }
        self.stages.push(name.clone());
        self.engines.push(EngineDecl {
            name: name.clone(),
            at,
            device,
            stage: prog.stages.len(),
            caps,
        });
        prog.stages.push(StageDecl {
            span,
            name,
            count: dev_count,
            array: dev_array,
            kind: StageKind::Step(Box::new(s)),
        });
        Ok(())
    }

    /// `{ let …; stmt* }` of an engine's `schedule`, written into `s`.
    fn schedule(&mut self, s: &mut StepSpec) -> PResult<()> {
        let at = self.pos;
        self.expect(&Tok::LBrace)?;
        let (lets, mut body) = self.engine_expr("schedule", |p| p.schedule_items())?;
        self.expect(&Tok::RBrace)?;
        if body.is_empty() {
            return self.err_at(at, "a schedule needs a statement: what the iteration does");
        }
        // a `let` is read as the iteration starts, where the kernel reads its
        // per-run cap, and nowhere else; it may read the ones above it
        let mut lets = lets;
        for i in 1..lets.len() {
            let (above, rest) = lets.split_at_mut(i);
            rest[0].1.substitute(above);
        }
        if !lets.is_empty() {
            let mut stray = None;
            visit(&body, &mut |st| {
                for e in st.exprs() {
                    if stray.is_none() {
                        stray = lets
                            .iter()
                            .find(|(n, _)| reads_name(e, n))
                            .map(|(n, _)| n.clone());
                    }
                }
            });
            if let Some(n) = stray {
                return self.err_at(
                    at,
                    format!(
                        "`{n}` is a `let` of the schedule, read as the iteration starts: it is \
                         read only in `each at most`; name an expression read where it stands, \
                         as `only (p)` is, with `def {n}() {{ … }}`, read as `{n}()`"
                    ),
                );
            }
            visit_mut(&mut body, &mut |st| {
                if let Some(c) = st.cap_mut() {
                    c.substitute(&lets);
                }
            });
        }
        // the kernel has one per-run cap for the whole iteration
        let mut caps: Vec<Option<Expr>> = vec![];
        visit(&body, &mut |st| {
            if let Some(c) = st.cap() {
                caps.push(c.cloned());
            }
        });
        if let Some(first) = caps.first() {
            let same = caps.iter().all(|c| match (c, first) {
                (Some(a), Some(b)) => a.same_syntax(b),
                (None, None) => true,
                _ => false,
            });
            if !same {
                return self.err_at(
                    at,
                    "the schedule's `each at most` differ, or some statements have one and others \
                     not: an iteration has one cap per run",
                );
            }
            if let Some(c) = first {
                s.chunk = self.per_run_cap(at, c.clone())?;
            }
        }
        self.write_schedule(at, s, body)
    }

    /// The `let`s at the top of a schedule, then its statements.
    fn schedule_items(&mut self) -> PResult<(Lets, Vec<SStmt>)> {
        let mut lets = vec![];
        let mut body = vec![];
        while *self.peek() != Tok::RBrace {
            if self.is_kw("let") {
                if !body.is_empty() {
                    return self.err(
                        "a schedule's `let` stands before its statements: it is read as the \
                         iteration starts",
                    );
                }
                self.advance();
                let n_at = self.pos;
                let n = self.ident()?;
                if KEYWORDS.contains(&n.as_str()) || lets.iter().any(|(l, _)| *l == n) {
                    return self.err_at(n_at, format!("`{n}` cannot name a `let` here"));
                }
                self.expect(&Tok::Assign)?;
                let e = self.engine_expr("each at most", |p| p.expr())?;
                self.expect(&Tok::Semi)?;
                lets.push((n, e));
            } else {
                body.push(self.schedule_stmt()?);
            }
        }
        Ok((lets, body))
    }

    fn schedule_block(&mut self) -> PResult<Vec<SStmt>> {
        self.expect(&Tok::LBrace)?;
        let mut body = vec![];
        while *self.peek() != Tok::RBrace {
            if self.is_kw("let") {
                return self.err("a schedule's `let` stands at its top, not in a `branch`");
            }
            body.push(self.schedule_stmt()?);
        }
        self.expect(&Tok::RBrace)?;
        Ok(body)
    }

    /// `each at most (e)`, if it is next.
    fn each_at_most(&mut self) -> PResult<Option<Expr>> {
        if !self.eat_kw("each") {
            return Ok(None);
        }
        self.expect_kw("at")?;
        self.expect_kw("most")?;
        Ok(Some(self.engine_expr("each at most", |p| p.paren_expr())?))
    }

    fn schedule_stmt(&mut self) -> PResult<SStmt> {
        if self.eat_kw("advance") {
            if !self.eat_kw("running") {
                return self.err("`advance` takes `running`: `advance running;`");
            }
            let only = if self.eat_kw("only") {
                Some(self.engine_expr("only", |p| p.paren_expr())?)
            } else {
                None
            };
            let order = if self.eat_kw("admission") {
                Some(Serve::Admission)
            } else if self.eat_kw("decode") {
                self.expect_kw("first")?;
                Some(Serve::DecodeFirst)
            } else if self.eat_kw("by") {
                self.expect(&Tok::LParen)?;
                let mut keys = vec![self.engine_expr("by", |p| p.expr())?];
                while self.eat(&Tok::Comma) {
                    keys.push(self.engine_expr("by", |p| p.expr())?);
                }
                self.expect(&Tok::RParen)?;
                Some(Serve::By(keys))
            } else {
                None
            };
            let cap = self.each_at_most()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Advance { only, order, cap })
        } else if self.eat_kw("admit") {
            if !self.eat_kw("waiting") {
                return self.err("`admit` takes `waiting`: `admit waiting;`");
            }
            let only = if self.eat_kw("only") {
                Some(self.engine_expr("only", |p| p.paren_expr())?)
            } else {
                None
            };
            let gate = if self.eat_kw("while") {
                Some(self.paren_expr()?)
            } else {
                None
            };
            let cap = self.each_at_most()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Admit { only, gate, cap })
        } else if self.eat_kw("exclusive") {
            self.expect_kw("prefill")?;
            let cap = self.each_at_most()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Exclusive { cap })
        } else if self.eat_kw("branch") {
            let guard = self.paren_expr()?;
            let then = self.schedule_block()?;
            let other = if self.eat_kw("else") {
                self.schedule_block()?
            } else {
                vec![]
            };
            Ok(SStmt::Branch(guard, then, other))
        } else if self.eat_kw("set") {
            let n = self.ident()?;
            self.expect(&Tok::Assign)?;
            let e = self.expr()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Set(n, e))
        } else if self.is_kw("serve") {
            self.err("in a schedule, `serve` is `advance running`")
        } else {
            self.err(format!(
                "a schedule takes `advance running`, `admit waiting`, `exclusive prefill`, \
                 `branch` and `set`; found {}",
                self.peek()
            ))
        }
    }

    /// A per-run cap as written: `inf` (no cap) stands as an outcome, the
    /// whole cap or a branch of a `?:`, not inside arithmetic. The linker
    /// writes an outcome of `inf` as the kernel's 0, and refuses one at or
    /// below 0, once the `let`s have their values.
    fn per_run_cap(&self, at: usize, c: Expr) -> PResult<Expr> {
        // `inf` anywhere in `e`, a bare call argument (`min(inf, c)`) included
        fn reads_inf(e: &Expr) -> bool {
            e.any(&|x| match x {
                Expr::Var(n) => n == "inf",
                Expr::Call(_, args) => args
                    .iter()
                    .any(|a| matches!(a, Arg::Ref(r) if r.name == "inf" && r.index.is_none())),
                _ => false,
            })
        }
        fn inf_inside(e: &Expr) -> bool {
            match e {
                Expr::Located(_, a) => inf_inside(a),
                Expr::Var(n) if n == "inf" => false,
                Expr::Cond(k, a, b) => reads_inf(k) || inf_inside(a) || inf_inside(b),
                e => reads_inf(e),
            }
        }
        if inf_inside(&c) {
            return self.err_at(
                at,
                "`inf` inside an `each at most`'s arithmetic: no cap is an outcome of `inf` as \
                 a whole",
            );
        }
        Ok(c)
    }

    /// Write a schedule as the kernel's step: vLLM's procedure and the
    /// stage-level forms as no body, anything else as the body.
    fn write_schedule(&self, at: usize, s: &mut StepSpec, body: Vec<SStmt>) -> PResult<()> {
        if let [
            first,
            SStmt::Admit {
                only: a_only,
                gate: Some(g),
                ..
            },
        ] = &body[..]
            && not_preempted(g)
        {
            match first {
                SStmt::Advance { only, order, .. }
                    if match (only, a_only) {
                        (None, None) => true,
                        (Some(p), Some(q)) => p.same_syntax(q),
                        _ => false,
                    } =>
                {
                    s.serve = order.clone().unwrap_or(Serve::Admission);
                    s.only = only.clone();
                    return Ok(());
                }
                SStmt::Exclusive { .. } if a_only.is_none() => {
                    s.serve = Serve::ExclusivePrefill;
                    return Ok(());
                }
                _ => {}
            }
        }
        let mut exclusive = false;
        visit(&body, &mut |st| {
            exclusive |= matches!(st, SStmt::Exclusive { .. })
        });
        if exclusive {
            return self.err_at(
                at,
                "`exclusive prefill` takes back decodes already chosen, which a body cannot \
                 write: its schedule is `exclusive prefill; admit waiting while \
                 (running.preempted == 0);`",
            );
        }
        s.iteration = Some(kernel(body));
        Ok(())
    }

    /// `read` as `clause` of an engine, which reads `running.…`, `waiting.…`
    /// and `batch.…` as `ENGINE_VALUES` says, and no bare total; the clause
    /// outside it is restored after.
    fn engine_expr<T>(
        &mut self,
        clause: &'static str,
        read: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<T> {
        let outer = self.in_engine.replace(clause);
        let r = read(self);
        self.in_engine = outer;
        r
    }

    /// An engine's value after `running`, `waiting` or `batch` and its dot.
    pub(super) fn list_value(&mut self, at: usize, list: &str) -> PResult<Expr> {
        let Some(clause) = self.in_engine else {
            return self.err_at(
                at,
                format!(
                    "`{list}.…` is a value of an engine, read in its `tokens cap`, `execute` and \
                     `schedule`"
                ),
            );
        };
        let f_at = self.pos;
        let field = self.ident()?;
        let value = format!("{list}.{field}");
        let Some((_, var, clauses)) = ENGINE_VALUES.iter().find(|(v, ..)| *v == value) else {
            let has: Vec<&str> = ENGINE_VALUES
                .iter()
                .filter_map(|(v, ..)| v.strip_prefix(list)?.strip_prefix('.'))
                .collect();
            return self.err_at(
                f_at,
                format!("`{value}`: `{list}` has `{}`", has.join("`, `")),
            );
        };
        if !clauses.contains(&clause) {
            return self.err_at(at, not_read_in(&value, clause));
        }
        Ok(Expr::Var((*var).into()))
    }

    /// In an engine, the bare names its values replaced: the name for the
    /// clause, or why the clause reads none.
    pub(super) fn retired_in_engine(&self, at: usize, name: &str) -> PResult<()> {
        let Some(clause) = self.in_engine else {
            return Ok(());
        };
        let named: Vec<_> = ENGINE_VALUES
            .iter()
            .filter(|(_, k, _)| *k == name)
            .collect();
        let Some((value, _, _)) = named.first() else {
            return Ok(());
        };
        match named.iter().find(|(_, _, cs)| cs.contains(&clause)) {
            Some((now, ..)) => self.err_at(
                at,
                format!("in an engine, `{name}` is `{now}` here: one value, one name"),
            ),
            None => self.err_at(
                at,
                format!("`{name}` is `{value}`, and {}", not_read_in(value, clause)),
            ),
        }
    }

    /// Link the engines, their devices and the pools on them, once the
    /// program is read: a pool on an engine, or on its device as
    /// `on ENGINE.DEVICE`, is admitted by the engine; the pool on a device is
    /// its engine's memory; `waiting.count` counts the queues the engine
    /// admits.
    pub(super) fn link_engines(&mut self, prog: &mut Program) -> PResult<()> {
        for po in &self.pools_on {
            let name = prog.pools[po.pool].name.clone();
            if self.engines.iter().any(|e| e.name == po.on.owner) {
                prog.pools[po.pool].admit_via = Some(Ref {
                    span: None,
                    name: po.on.owner.clone(),
                    index: None,
                });
                continue;
            }
            let on: Vec<&EngineDecl> = self
                .engines
                .iter()
                .filter(|e| e.device == po.on.owner)
                .collect();
            let engine = match on[..] {
                [] => continue,
                [e] => e,
                _ => {
                    return self.err_at(
                        on[1].at,
                        format!(
                            "two engines on `{}`: who admits its pools would be ambiguous",
                            po.on.owner
                        ),
                    );
                }
            };
            let StageKind::Step(s) = &mut prog.stages[engine.stage].kind else {
                unreachable!("an engine is a step stage")
            };
            if let Some(m) = &s.memory {
                return self.err_at(
                    po.on.at,
                    format!(
                        "`{}` and `{name}` are both on `{}`: which is `{}`'s KV would be a \
                         choice the program did not make",
                        m.name, po.on.owner, engine.name
                    ),
                );
            }
            s.memory = Some(Ref {
                span: None,
                name: name.clone(),
                index: None,
            });
            prog.pools[po.pool].admit_via = po.on.admitted_by.as_ref().map(|e| Ref {
                span: None,
                name: e.clone(),
                index: None,
            });
        }
        for (owner, caps) in self
            .devices
            .iter()
            .map(|d| (&d.name, &d.caps))
            .chain(self.engines.iter().map(|e| (&e.name, &e.caps)))
        {
            for c in caps {
                let declared = self
                    .pools_on
                    .iter()
                    .any(|po| po.on.owner == *owner && po.cap == c.name);
                if !declared {
                    return self.err_at(
                        c.at,
                        format!(
                            "`{}` of {} is no pool: declare {}, or nothing can hold it",
                            c.name,
                            shown(owner),
                            pool_on_as_written(&c.name, owner)
                        ),
                    );
                }
            }
        }
        for e in &self.engines {
            let queues: Vec<Ref> = prog
                .pools
                .iter()
                .filter(|p| p.admit_via.as_ref().is_some_and(|r| r.name == e.name))
                .map(|p| Ref {
                    span: None,
                    name: p.name.clone(),
                    index: None,
                })
                .collect();
            let count = queues
                .into_iter()
                .map(|r| Expr::Call("queued".into(), vec![Arg::Ref(r)]))
                .reduce(|a, b| Expr::Binary(BinOp::Add, Box::new(a), Box::new(b)))
                .unwrap_or(Expr::Num(0.0));
            let family = prog.stages[e.stage].array;
            let StageKind::Step(s) = &mut prog.stages[e.stage].kind else {
                unreachable!("an engine is a step stage")
            };
            if family && step_reads(s, WAITING_COUNT) {
                return self.err_at(
                    e.at,
                    format!(
                        "`{}` is a family, and its `waiting.count` would read every member's \
                         queues, which no member's engine may: count a member's own with \
                         `queued(…)` where it has an index",
                        e.name
                    ),
                );
            }
            let binds = [(WAITING_COUNT.to_string(), count)];
            s.budget.substitute(&binds);
            s.cost.substitute(&binds);
            s.chunk.substitute(&binds);
            if let Serve::By(keys) = &mut s.serve {
                keys.iter_mut().for_each(|k| k.substitute(&binds));
            }
            if let Some(o) = &mut s.only {
                o.substitute(&binds);
            }
            if let Some(body) = &mut s.iteration {
                substitute_body(body, &binds);
            }
        }
        Ok(())
    }

    /// Record that `prog.pools[pool]` is `on.owner`'s capacity `cap`.
    pub(super) fn pool_on(&mut self, pool: usize, cap: String, on: On) -> PResult<()> {
        let po = PoolOn { pool, cap, on };
        if self
            .pools_on
            .iter()
            .any(|o| o.on.owner == po.on.owner && o.cap == po.cap)
        {
            return self.err_at(
                po.on.at,
                format!(
                    "`{}` of {} is declared as a pool twice",
                    po.cap,
                    shown(&po.on.owner)
                ),
            );
        }
        self.pools_on.push(po);
        Ok(())
    }

    /// `pool NAME on ENGINE.DEVICE`: ENGINE is declared above and runs on
    /// DEVICE, whose engine it is.
    pub(super) fn check_admitted_by(&self, at: usize, engine: &str, device: &str) -> PResult<()> {
        if !self.devices.iter().any(|d| d.name == device) {
            return self.err_at(
                at,
                format!(
                    "{} is not a device: `on ENGINE.DEVICE` is the device the engine runs on",
                    shown(device)
                ),
            );
        }
        match self.engines.iter().find(|e| e.name == engine) {
            Some(e) if e.device == device => Ok(()),
            Some(e) => self.err_at(
                at,
                format!(
                    "`{engine}` runs on {}, not {}: `on ENGINE.DEVICE` is the device the \
                     engine runs on",
                    shown(&e.device),
                    shown(device)
                ),
            ),
            None => self.err_at(
                at,
                format!(
                    "no engine `{engine}`: `on ENGINE.DEVICE` names the engine that admits the \
                     pool's queue, declared above the pool"
                ),
            ),
        }
    }

    /// `pool NAME on OWNER`: the capacity it is, as `(cap, count, array)`.
    pub(super) fn capacity_of(
        &self,
        at: usize,
        name: &str,
        owner: &str,
    ) -> PResult<(Expr, usize, bool)> {
        if let Some(d) = self.devices.iter().find(|d| d.name == owner) {
            if d.times.iter().any(|t| t.name == name) {
                return self.err_at(
                    at,
                    format!(
                        "`{name}` is a time resource of {}, not a capacity: no pool holds it",
                        shown(owner)
                    ),
                );
            }
            return match d.caps.iter().find(|c| c.name == name) {
                Some(c) => Ok((c.cap.clone(), d.count, d.array)),
                None => self.err_at(at, format!("{} has no capacity `{name}`", shown(owner))),
            };
        }
        if let Some(e) = self.engines.iter().find(|e| e.name == owner) {
            let (count, array) = self
                .devices
                .iter()
                .find(|d| d.name == e.device)
                .map_or((1, false), |d| (d.count, d.array));
            return match e.caps.iter().find(|c| c.name == name) {
                Some(c) => Ok((c.cap.clone(), count, array)),
                None => self.err_at(at, format!("`{owner}` has no capacity `{name}`")),
            };
        }
        self.err_at(
            at,
            format!("no device or engine `{owner}`: a pool is on one declared before it"),
        )
    }
}

/// A device or an engine as the program names it: a queue's device `Q.gpu`
/// is `gpu` in queue `Q`.
fn shown(owner: &str) -> String {
    match owner.split_once('.') {
        Some((q, local)) => format!("`{local}` of queue `{q}`"),
        None => format!("`{owner}`"),
    }
}

/// The `pool … on …` that declares capacity `cap` of `owner`, as written
/// where `owner` is declared.
fn pool_on_as_written(cap: &str, owner: &str) -> String {
    match owner.split_once('.') {
        Some((q, local)) => format!("`pool {cap} on {local} {{ … }}` in queue `{q}`"),
        None => format!("`pool {cap} on {owner} {{ … }}`"),
    }
}

/// `!running.preempted` or `running.preempted == 0`, as written.
fn not_preempted(e: &Expr) -> bool {
    let is_preempted = |e: &Expr| matches!(strip(e), Expr::Var(n) if n == "preempted");
    match strip(e) {
        Expr::Unary(UnOp::Not, a) => is_preempted(a),
        Expr::Binary(BinOp::Eq, a, b) => {
            (is_preempted(a) && matches!(strip(b), Expr::Num(z) if *z == 0.0))
                || (is_preempted(b) && matches!(strip(a), Expr::Num(z) if *z == 0.0))
        }
        _ => false,
    }
}

fn strip(e: &Expr) -> &Expr {
    match e {
        Expr::Located(_, a) => strip(a),
        e => e,
    }
}

fn reads_name(e: &Expr, n: &str) -> bool {
    e.any(&|x| matches!(x, Expr::Var(v) if v == n))
}

/// Each statement of a schedule, a `branch`'s included, before its own.
fn visit(body: &[SStmt], f: &mut impl FnMut(&SStmt)) {
    for st in body {
        f(st);
        if let SStmt::Branch(_, a, b) = st {
            visit(a, f);
            visit(b, f);
        }
    }
}

fn visit_mut(body: &mut [SStmt], f: &mut impl FnMut(&mut SStmt)) {
    for st in body {
        f(st);
        if let SStmt::Branch(_, a, b) = st {
            visit_mut(a, f);
            visit_mut(b, f);
        }
    }
}

impl SStmt {
    /// The per-run cap of a statement that gives tokens: `Some(None)` for
    /// one written without it, `None` for a statement that gives none.
    fn cap(&self) -> Option<Option<&Expr>> {
        match self {
            SStmt::Advance { cap, .. } | SStmt::Admit { cap, .. } | SStmt::Exclusive { cap } => {
                Some(cap.as_ref())
            }
            SStmt::Branch(..) | SStmt::Set(..) => None,
        }
    }

    fn cap_mut(&mut self) -> Option<&mut Expr> {
        match self {
            SStmt::Advance { cap, .. } | SStmt::Admit { cap, .. } | SStmt::Exclusive { cap } => {
                cap.as_mut()
            }
            SStmt::Branch(..) | SStmt::Set(..) => None,
        }
    }

    /// The statement's own expressions other than its cap.
    fn exprs(&self) -> Vec<&Expr> {
        match self {
            SStmt::Advance { only, order, .. } => {
                let mut v: Vec<&Expr> = only.iter().collect();
                if let Some(Serve::By(keys)) = order {
                    v.extend(keys);
                }
                v
            }
            SStmt::Admit { only, gate, .. } => only.iter().chain(gate.iter()).collect(),
            SStmt::Exclusive { .. } => vec![],
            SStmt::Branch(g, ..) => vec![g],
            SStmt::Set(_, e) => vec![e],
        }
    }
}

/// A schedule's statements as the kernel's iteration body.
fn kernel(body: Vec<SStmt>) -> Vec<IterStmt> {
    body.into_iter()
        .map(|st| match st {
            SStmt::Advance { only, order, .. } => IterStmt::Serve { only, order },
            SStmt::Admit { only, gate, .. } => IterStmt::Admit { only, gate },
            SStmt::Exclusive { .. } => unreachable!("refused before the body is written"),
            SStmt::Branch(g, a, b) => IterStmt::Branch(g, kernel(a), kernel(b)),
            SStmt::Set(n, e) => IterStmt::Set(n, e),
        })
        .collect()
}

fn substitute_body(body: &mut [IterStmt], binds: &[(String, Expr)]) {
    for st in body {
        match st {
            IterStmt::Serve { only, order } => {
                only.iter_mut().for_each(|e| e.substitute(binds));
                if let Some(Serve::By(keys)) = order {
                    keys.iter_mut().for_each(|e| e.substitute(binds));
                }
            }
            IterStmt::Admit { only, gate } => {
                only.iter_mut()
                    .chain(gate.iter_mut())
                    .for_each(|e| e.substitute(binds));
            }
            IterStmt::Branch(g, a, b) => {
                g.substitute(binds);
                substitute_body(a, binds);
                substitute_body(b, binds);
            }
            IterStmt::Set(_, e) => e.substitute(binds),
        }
    }
}

/// Whether a step's expressions read the name `n`.
fn step_reads(s: &StepSpec, n: &str) -> bool {
    fn body_reads(b: &[IterStmt], n: &str) -> bool {
        b.iter().any(|st| match st {
            IterStmt::Serve { only, order } => {
                only.iter().any(|e| reads_name(e, n))
                    || matches!(order, Some(Serve::By(keys)) if keys.iter().any(|e| reads_name(e, n)))
            }
            IterStmt::Admit { only, gate } => only.iter().chain(gate).any(|e| reads_name(e, n)),
            IterStmt::Branch(g, a, b) => reads_name(g, n) || body_reads(a, n) || body_reads(b, n),
            IterStmt::Set(_, e) => reads_name(e, n),
        })
    }
    reads_name(&s.budget, n)
        || reads_name(&s.cost, n)
        || reads_name(&s.chunk, n)
        || matches!(&s.serve, Serve::By(keys) if keys.iter().any(|e| reads_name(e, n)))
        || s.only.iter().any(|e| reads_name(e, n))
        || s.iteration.as_deref().is_some_and(|b| body_reads(b, n))
}
