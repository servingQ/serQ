//! Engines on devices (`docs/design/engine-device.md`): `device`, `engine …
//! on`, `pool … on`, an engine's `scheduler` and `worker`. All of it is
//! parse-time sugar: an engine is a step stage, a pool on a device or an
//! engine is a pool, and the links between them (`memory`, `admit_via`,
//! `waiting.count`) are written once every declaration is read.

use super::*;

/// `device NAME [N] { RESOURCE (PARAM) = expr; RESOURCE cap expr; }`
pub(super) struct DeviceDecl {
    pub name: String,
    pub count: usize,
    pub array: bool,
    /// Time resources: a demand to time, expanded as a `def` is, and only
    /// inside an engine's `worker`.
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
    /// Whether it has a `connector`, whose records its caller may read.
    pub connector: bool,
    /// Whether it admits requests (`taking`), whose records its caller reads.
    pub request: bool,
    /// The device's pool its `taking` takes, its memory, by its bare name:
    /// a queue's entry that calls it leaves its KV there, which a `from`
    /// takes when a KV relation reads the queue (the request's send,
    /// `ir::CRequest::send`).
    pub kv: Option<String>,
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
const AS_THEY_STAND: &[&str] = &["tokens cap", "each at most", "only", "by", "scheduler"];

/// An engine's values: the name it reads one by, the kernel's context
/// variable, and the clauses that read it. `running` is the residents as
/// they stand, `waiting` the queues, `batch` the iteration's batch: in a
/// schedule's statements (`scheduler`), what it has formed so far; in
/// `worker`, what it formed. The kernel reads `decoders` and `kv_decode` as
/// the residents' before the batch is formed and as the batch's in `cost`
/// (#416), so the engine names them twice. Within a schedule, `only`, `by`
/// and `each at most` are read at their own moments, not the statements'.
pub const ENGINE_VALUES: [(&str, &str, &[&str]); 20] = [
    ("running.count", "residents", AS_THEY_STAND),
    ("running.decoding", "decoders", AS_THEY_STAND),
    ("running.kv_decode", "kv_decode", AS_THEY_STAND),
    ("running.kv_prefill", "kv_prefill", AS_THEY_STAND),
    ("running.preempted", "preempted", &["scheduler"]),
    ("waiting.count", WAITING_COUNT, AS_THEY_STAND),
    ("waiting.admitted", "admitted", &["scheduler"]),
    ("batch.tokens", "tokens", &["scheduler", "worker"]),
    ("batch.prefilled", "prefilled", &["scheduler", "worker"]),
    ("batch.decoding", "decoders", &["worker"]),
    ("batch.kv_decode", "kv_decode", &["worker"]),
    ("batch.kv_prefill", "kv_prefill", &["worker"]),
    ("batch.attention", "attention", &["worker"]),
    ("request.input", "request.input", &["taking", "cache"]),
    ("request.output", "request.output", &["taking", "cache"]),
    ("request.known", "request.known", &["taking", "cache"]),
    ("request.hit", "request.hit", &["taking"]),
    ("request.finished", "request.finished", &["cache"]),
    ("request.loaded", "request.loaded", &["taking"]),
    ("request.preempted", "request.preempted", &["taking"]),
];

/// The gate of vLLM's procedure, as the frontend recognises it and the
/// refusals that name it write it (#721): `advance running; admit waiting
/// while (running.preempted == 0);` is the procedure, and the same with
/// `exclusive prefill` first the `ExclusivePrefill` order.
pub const VLLM_GATE: &str = "running.preempted == 0";

/// Apply `f` to every pool or stage reference of an engine's admission.
fn qualify_request(r: &mut RequestSpec, f: &dyn Fn(&mut Ref)) {
    fn expr(e: &mut Expr, f: &dyn Fn(&mut Ref)) {
        match e {
            Expr::Located(_, x) | Expr::Unary(_, x) => expr(x, f),
            Expr::Binary(_, a, b) | Expr::Over(_, _, a, b) => {
                expr(a, f);
                expr(b, f);
            }
            Expr::Cond(c, a, b) => {
                expr(c, f);
                expr(a, f);
                expr(b, f);
            }
            Expr::Sample(_, xs) => xs.iter_mut().for_each(|x| expr(x, f)),
            Expr::Call(_, args) | Expr::Unread(args, _) => {
                for a in args.iter_mut() {
                    match a {
                        Arg::Ref(r) => f(r),
                        Arg::Expr(x) => expr(x, f),
                    }
                }
                if let Expr::Unread(_, body) = e {
                    expr(body, f);
                }
            }
            Expr::Num(_) | Expr::Var(_) => {}
        }
    }
    for (q, u, reserve) in &mut r.pools {
        f(q);
        expr(u, f);
        if let Some(x) = reserve {
            expr(x, f);
        }
    }
    for x in r.reuse.iter_mut().chain(&mut r.cache) {
        expr(x, f);
    }
    for (k, n) in r.prefix.iter_mut().flatten() {
        expr(k, f);
        expr(n, f);
    }
}

/// The name an engine reads the kernel's context variable `var` by in
/// `clause` (`tokens cap`, `scheduler`, `worker`), if it reads it there: a
/// view that draws a step stage in an engine's words names it so.
pub fn engine_name(var: &str, clause: &str) -> Option<&'static str> {
    ENGINE_VALUES
        .iter()
        .find(|(_, k, cs)| *k == var && cs.contains(&clause))
        .map(|(v, ..)| *v)
}

/// A context variable as an engine's `tokens cap` reads it: the residents'
/// totals by their list (`running.decoding`), the rest by the kernel's name.
/// A printer of a step stage's budget (`Program::show_expr_with`) takes it.
pub(crate) fn tokens_cap_name(v: crate::ir::CtxVar) -> &'static str {
    engine_name(v.name(), "tokens cap").unwrap_or(v.name())
}

/// Why `value` of `ENGINE_VALUES` is not read in `clause`.
fn not_read_in(value: &str, clause: &str) -> String {
    let (list, field) = value.split_once('.').expect("a list's value");
    if list == "request" {
        if field == "hit" {
            "`request.hit` is read at the request's admission, in `taking` and a connector's \
             `load`"
                .into()
        } else if field == "loaded" {
            "`request.loaded` is what the connector's `load` read at the request's admission, \
             read in `taking`"
                .into()
        } else if field == "finished" {
            "`request.finished` is read when the request gives its pools back, in `cache`".into()
        } else {
            format!(
                "`{value}` is the request's, read at its admission (`taking`, `reuse`, \
                 `prefix`) and in its `cache`, not in `{clause}`"
            )
        }
    } else if clause == "taking" || clause == "cache" {
        format!(
            "`{clause}` is read for one request, which reads `request.…`, the session's \
             attributes and the engine's pools, not `{value}`"
        )
    } else if field == "preempted" || field == "admitted" {
        format!(
            "`{value}` is what the iteration's schedule did, read in its `if`, `while` and \
             `set`, not in `{clause}`"
        )
    } else if clause == "worker" {
        let batch = if list == "running" && field != "count" {
            format!(", and the batch's is `batch.{field}`")
        } else {
            String::new()
        };
        format!(
            "the worker reads what the scheduler scheduled, `batch.…`: `{value}` counts the \
             {}, in the batch or not{batch}",
            if list == "running" {
                "residents"
            } else {
                "queues"
            }
        )
    } else if clause == "scheduler" {
        format!(
            "`{value}` is the formed batch's, read in `worker`; a schedule's statements read \
             the batch so far as `batch.tokens` and `batch.prefilled`, and the residents' as \
             `running.…`"
        )
    } else {
        let so_far = if ["tokens", "prefilled"].contains(&field) {
            ", and as the batch so far in a schedule's `if`, `while` and `set`"
        } else {
            ""
        };
        format!(
            "`{clause}` is read before the batch is formed, so it reads no `batch.…`: `{value}` \
             is read in `worker`{so_far}"
        )
    }
}

/// `waiting.count` until the pools the engine admits are known.
pub(super) const WAITING_COUNT: &str = "waiting.count";

impl Parser {
    /// `device` after the keyword.
    pub(super) fn device(&mut self) -> PResult<()> {
        let at = self.pos;
        let name = self.ident()?;
        if KEYWORDS.contains(&name.as_str()) {
            return self.err_at(
                at,
                Code::ReservedWord.message(format_args!("`{name}` is a word of the language")),
            );
        }
        self.declare(at, &name, Kind::Device)?;
        if self.devices.iter().any(|d| d.name == name) {
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
            if is_reserved(&r) || self.defs.iter().any(|def| def.name == r) {
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
    /// an engine's `worker` expands.
    fn time_resource(&mut self, at: usize, name: String) -> PResult<Def> {
        self.expect(&Tok::LParen)?;
        let mut params: Vec<String> = vec![];
        while *self.peek() != Tok::RParen {
            let p_at = self.pos;
            let p = self.name()?;
            if is_reserved(&p) {
                return self.err_at(
                    p_at,
                    Code::ReservedWord.message(format_args!("`{p}` is a word of the language")),
                );
            }
            self.own_name(p_at, &p, "a parameter")?;
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
    /// engine's `worker`, where it is not defined, it is an error.
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
            return self.err_at(
                at,
                Code::ReservedWord.message(format_args!("`{name}` is a word of the language")),
            );
        }
        // a queue's engine is named after the queue: that one is refused as
        // a queue, in `declare_in`
        if self
            .declared
            .iter()
            .any(|d| d.name == name && d.kind == Kind::Engine)
        {
            return self.err_at(at, format!("`{name}` is declared twice"));
        }
        self.declare(at, &name, Kind::Engine)?;
        let array = self.array_count()?;
        // an engine runs through its request, and a family has none: a
        // family of engines is a queue family, whose entries run them
        if let Some(n) = array {
            return self.err_at(
                at,
                format!(
                    "engine family `{name}[{n}]`: a family of engines is a queue family, \
                     `queue {name}[{n}] : ROLE {{ device …; engine on DEVICE {{ … }} … }}`, whose \
                     entries the server calls as `{name}[j].VERB (…);` (#539)"
                ),
            );
        }
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
        if dev_array {
            return self.err_at(
                at,
                format!(
                    "an engine is one per device, and `{device}` is a family of {dev_count}: \
                     engines on a family of devices are a queue family, `queue {name}[{dev_count}] \
                     : ROLE {{ device …; engine on DEVICE {{ … }} … }}` (#539)"
                ),
            );
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
            granule: None,
            serve: Serve::Admission,
            memory: None,
            schedule: Schedule::Procedure(None),
            state: vec![],
            request: None,
            connector: None,
        };
        let mut caps: Vec<Capacity> = vec![];
        let (mut tokens, mut schedule, mut execute) = (false, false, false);
        self.takings.clear();
        while *self.peek() != Tok::RBrace {
            let k_at = self.pos;
            if self.is_kw("request") {
                return self.err(
                    "an engine keeps a request's state, as vLLM's scheduler does: no `request \
                     (…) { … }` says what it does with one. What its admission takes is \
                     `taking … [reuse (e)] [prefix (…)] [cache (e)]` on `admit waiting`, and the \
                     server calls `E (input, output);` (docs/design/scheduler-worker.md)",
                );
            } else if self.eat_kw("tokens") {
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
            } else if self.eat_kw("scheduler") {
                if schedule {
                    return self.err_at(k_at, "`scheduler` twice: an engine has one scheduler");
                }
                schedule = true;
                self.schedule(&name, &mut s)?;
            } else if self.eat_kw("connector") {
                if s.connector.is_some() {
                    return self.err_at(k_at, "`connector` twice: an engine has one KV connector");
                }
                s.connector = Some(self.connector()?);
            } else if self.eat_kw("worker") {
                if execute {
                    return self.err_at(k_at, "`worker` twice: an engine has one worker");
                }
                execute = true;
                s.cost = self.worker(dev)?;
            } else if let Some(now) = [
                ("budget", "`tokens cap B;`"),
                ("chunk", "`each at most (e)` on the schedule's statements"),
                ("cost", "`worker { forward (T); }`"),
                ("execute", "`worker { forward (T); }`"),
                (
                    "memory",
                    "nothing: the engine's KV is the pool on its device",
                ),
                ("serve", "a statement of `scheduler { … }`"),
                ("iteration", "`scheduler { … }`"),
                ("schedule", "`scheduler { … }`"),
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
                    return self.err_at(
                        k_at,
                        Code::ReservedWord.message(format_args!("`{r}` is a word of the language")),
                    );
                }
                // an engine's capacity is one pool: a request's `taking`
                // names one, and a queue's entry names its own without an
                // index (#539)
                if let Some(n) = self.array_count()? {
                    let message = match device.split_once('.') {
                        None => format!(
                            "`{r}[{n}]` in engine `{name}`: an engine admits through its \
                             request's `taking`, which names one `{r}`; write `{r} cap c;`, or \
                             a queue family `queue Q[{n}]` for {n} engines"
                        ),
                        Some((q, _)) => format!(
                            "`{r}[{n}]` in queue `{q}`'s engine: an entry of the queue names \
                             its own pool without an index, so no entry could pick a member; \
                             a queue's engine admits one `{r}`, written `{r} cap c;`"
                        ),
                    };
                    return self.err_at(k_at, message);
                }
                if !self.eat_kw("cap") {
                    return self.err(format!(
                        "an engine holds capacities (`{r} cap expr;`), `tokens cap`, `granule`, \
                         `state`, `connector`, `scheduler` and `worker`; found {}",
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
            (schedule, "`scheduler { … }`: what each iteration does"),
            (execute, "`worker { forward (T); }`: how long a batch takes"),
        ] {
            if !has {
                return self.err_at(at, format!("engine `{name}` needs {what}"));
            }
        }
        // what its request may take: its own capacities and its device's
        let takeable: Vec<String> = caps
            .iter()
            .chain(&self.devices[dev].caps)
            .map(|c| c.name.clone())
            .collect();
        // a queue's engine admits by its entries, which hold its pools
        let in_queue = device.contains('.');
        let mut taking = self.engine_request(&name, &takeable)?;
        let kv = taking
            .pools
            .iter()
            .find(|(r, _, _)| self.devices[dev].caps.iter().any(|c| c.name == r.name))
            .map(|(r, _, _)| r.name.clone());
        if !in_queue {
            s.request = Some(taking);
        } else if !taking.pools.is_empty() {
            // a queue's engine takes its queue's pools, `E.kv`; a member of
            // a family its own (the linker writes the member)
            let q = device.split('.').next().unwrap_or_default().to_string();
            qualify_request(&mut taking, &|r: &mut Ref| {
                if takeable.contains(&r.name) {
                    r.name = format!("{q}.{}", r.name);
                }
            });
            s.request = Some(taking);
        }
        self.stages.push(name.clone());
        self.engines.push(EngineDecl {
            name: name.clone(),
            at,
            device,
            stage: prog.stages.len(),
            caps,
            connector: s.connector.is_some(),
            request: s.request.is_some() || !in_queue,
            kv,
        });
        prog.stages.push(StageDecl {
            span,
            name: name.clone(),
            count: dev_count,
            array: dev_array,
            kind: StageKind::Step(Box::new(s)),
        });
        Ok(())
    }

    /// `C { per_token (r); [load (n);] [save (n);] }` of an engine's
    /// `connector`: the L2 queue `C` it loads a request's KV from at the
    /// request's admission (`n` read as `taking` is, `request.hit` included)
    /// and saves it to at its end (`n` read as `cache` is), each token's copy
    /// taking `r` at `C`'s bandwidth (`ir::CConnector`).
    fn connector(&mut self) -> PResult<ConnectorSpec> {
        let c_at = self.pos;
        let span = Some(self.span());
        let c = self.ident()?;
        // an L2 declared above, or the source of the engine's queue's KV
        // relation, which may be declared below (`relations`)
        let l2 = self.cache_of(&c).filter(|(name, _)| *name == c);
        if l2.is_none() && !self.queues.iter().any(|q| q.name == c) {
            return self.err_at(
                c_at,
                format!(
                    "`connector {c}`: `{c}` is not a queue that plays `l2` declared above, nor \
                     a queue a queue's engine reads KV from"
                ),
            );
        }
        let r = |name: String| Ref {
            span,
            name,
            index: None,
        };
        self.expect(&Tok::LBrace)?;
        let (mut per_token, mut load, mut save) = (None, None, None);
        while *self.peek() != Tok::RBrace {
            let k_at = self.pos;
            let (slot, clause) = if self.eat_kw("per_token") {
                (&mut per_token, "per_token")
            } else if self.eat_kw("load") {
                (&mut load, "taking")
            } else if self.eat_kw("save") {
                (&mut save, "cache")
            } else {
                return self.err(format!(
                    "a connector takes `per_token (r);`, `load (n);` and `save (n);`; found {}",
                    self.peek()
                ));
            };
            if slot.is_some() {
                return self.err_at(k_at, "a connector's clause twice");
            }
            self.expect(&Tok::LParen)?;
            let e_at = self.pos;
            let e = self.engine_expr(clause, |p| p.expr())?;
            // a save's copy runs in the L2's hold: `cached` and `computed`
            // there are the L2 entry's, not the request's
            if clause == "per_token"
                && let Some(w) = SCHEDULER_ATTRS.into_iter().find(|w| expr_reads(&e, w))
            {
                return self.err_at(
                    e_at,
                    format!(
                        "`per_token` is read as a copy starts, where `{w}` would be the L2 \
                         entry's, not the request's"
                    ),
                );
            }
            *slot = Some(e);
            self.expect(&Tok::RParen)?;
            self.expect(&Tok::Semi)?;
        }
        self.expect(&Tok::RBrace)?;
        let kind = match l2 {
            Some((stage, pool)) => {
                let Some(per_token) = per_token else {
                    return self.err_at(
                        c_at,
                        "a connector needs `per_token (r);`: the time a token's copy takes",
                    );
                };
                if load.is_none() && save.is_none() {
                    return self.err_at(c_at, "a connector needs `load (n);`, `save (n);` or both");
                }
                ConnectorKind::L2 {
                    stage: r(stage),
                    pool: r(pool),
                    per_token,
                }
            }
            // a read over the NICs the relation declares, whose time is
            // theirs, of what the source leased: there is nothing to save
            None => {
                if per_token.is_some() || save.is_some() || load.is_none() {
                    return self.err_at(
                        c_at,
                        format!(
                            "`connector {c}` reads the KV queue `{c}` leased, over the NICs \
                             `kvt.nixl_pull` or `kvt.nixl_push` declares: it takes `load (n);` \
                             alone"
                        ),
                    );
                }
                ConnectorKind::Relation(None)
            }
        };
        Ok(ConnectorSpec {
            queue: c,
            kind,
            load,
            save,
        })
    }

    /// `{ forward (T); }` of an engine on `devices[dev]`: what the worker does
    /// with the batch the scheduler formed, here its forward's time `T`.
    fn worker(&mut self, dev: usize) -> PResult<Expr> {
        let at = self.pos;
        self.expect(&Tok::LBrace)?;
        let mut forward = None;
        while *self.peek() != Tok::RBrace {
            let k_at = self.pos;
            if !self.eat_kw("forward") {
                return self.err(format!(
                    "a worker takes `forward (T);`, the batch's time; found {}",
                    self.peek()
                ));
            }
            if forward.is_some() {
                return self.err_at(k_at, "`forward` twice: a batch takes one forward");
            }
            self.expect(&Tok::LParen)?;
            // the device's time resources are definitions here, and only here
            let mark = self.defs.len();
            let times: Vec<Def> = self.devices[dev].times.clone();
            self.defs.extend(times);
            let e = self.engine_expr("worker", |p| p.expr());
            self.defs.truncate(mark);
            forward = Some(e?);
            self.expect(&Tok::RParen)?;
            self.expect(&Tok::Semi)?;
        }
        self.expect(&Tok::RBrace)?;
        match forward {
            Some(e) => Ok(e),
            None => self.err_at(at, "a worker needs `forward (T);`: how long a batch takes"),
        }
    }

    /// `{ let …; stmt* }` of engine `engine`'s `scheduler`, written into `s`.
    fn schedule(&mut self, engine: &str, s: &mut StepSpec) -> PResult<()> {
        let at = self.pos;
        self.expect(&Tok::LBrace)?;
        let (lets, mut body) = self.engine_expr("scheduler", |p| p.schedule_items(engine))?;
        self.expect(&Tok::RBrace)?;
        if body.is_empty() {
            return self.err_at(at, "a scheduler needs a statement: what the iteration does");
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

    /// The `let`s at the top of a schedule, then its statements. A `let`
    /// is read only in the schedule's `each at most`, which reads the
    /// program's `def`s and `let`s beside it: one may not share its name
    /// with a `def` (`Declaration::meets`).
    fn schedule_items(&mut self, engine: &str) -> PResult<(Lets, Vec<SStmt>)> {
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
                self.declare_in(Scope::Schedule(engine.into()), n_at, &n, Kind::Constant)?;
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
                return self.err("a schedule's `let` stands at its top, not in an `if`");
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
            self.taking_src()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Admit { only, gate, cap })
        } else if self.eat_kw("exclusive") {
            self.expect_kw("prefill")?;
            let cap = self.each_at_most()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Exclusive { cap })
        } else if self.eat_kw("if") {
            self.schedule_if()
        } else if self.eat_kw("set") {
            let n = self.ident()?;
            self.expect(&Tok::Assign)?;
            let e = self.expr()?;
            self.expect(&Tok::Semi)?;
            Ok(SStmt::Set(n, e))
        } else if self.is_kw("serve") {
            self.err("in a schedule, `serve` is `advance running`")
        } else if let Tok::Ident(w) = self.peek()
            && let Some(now) = super::retired(w)
        {
            self.err(now)
        } else {
            self.err(format!(
                "a schedule takes `advance running`, `admit waiting`, `exclusive prefill`, \
                 `if` and `set`; found {}",
                self.peek()
            ))
        }
    }

    /// A schedule's `if (e) { … } [else { … } | else if …]`, after the `if`.
    fn schedule_if(&mut self) -> PResult<SStmt> {
        let guard = self.paren_expr()?;
        let then = self.schedule_block()?;
        let other = if !self.eat_kw("else") {
            vec![]
        } else if self.eat_kw("if") {
            vec![self.schedule_if()?]
        } else {
            self.schedule_block()?
        };
        Ok(SStmt::Branch(guard, then, other))
    }

    /// A per-run cap as written: `inf` (no cap) stands as an outcome, the
    /// whole cap or a branch of a `?:`, not inside arithmetic. The linker
    /// writes an outcome of `inf` as the kernel's 0, and refuses one at or
    /// below 0, once the `let`s have their values.
    fn per_run_cap(&self, at: usize, c: Expr) -> PResult<Expr> {
        // `inf` anywhere in `e`, a bare call argument (`min(inf, c)`) included
        fn inf(n: &str) -> bool {
            supplied(n) == Some(Supplied::Inf)
        }
        fn reads_inf(e: &Expr) -> bool {
            e.any(&|x| match x {
                Expr::Var(n) => inf(n),
                Expr::Call(_, args) => args
                    .iter()
                    .any(|a| matches!(a, Arg::Ref(r) if inf(&r.name) && r.index.is_none())),
                _ => false,
            })
        }
        fn inf_inside(e: &Expr) -> bool {
            match e {
                Expr::Located(_, a) => inf_inside(a),
                Expr::Var(n) if inf(n) => false,
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
    /// stage-level forms as no body, anything else as the body. The
    /// procedure is recognised by its gate's spelling, `VLLM_GATE` once each
    /// `def` is written out (`vllm_gate`), not by what a gate computes
    /// (#721).
    fn write_schedule(&self, at: usize, s: &mut StepSpec, body: Vec<SStmt>) -> PResult<()> {
        if let [
            first,
            SStmt::Admit {
                only: a_only,
                gate: Some(g),
                ..
            },
        ] = &body[..]
            && vllm_gate(g)
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
                    s.schedule = Schedule::Procedure(only.clone());
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
            // the rule's own form with another gate: the gate is what differs
            let gated = matches!(
                &body[..],
                [
                    SStmt::Exclusive { .. },
                    SStmt::Admit {
                        only: None,
                        gate: Some(_),
                        ..
                    },
                ]
            );
            return self.err_at(
                at,
                format!(
                    "`exclusive prefill` takes back decodes already chosen, which a body cannot \
                     write: its schedule is `exclusive prefill; admit waiting while \
                     ({VLLM_GATE});`{}",
                    if gated {
                        format!(
                            ", its gate spelt `{VLLM_GATE}`: a gate written otherwise makes \
                             the schedule a body"
                        )
                    } else {
                        String::new()
                    }
                ),
            );
        }
        s.schedule = Schedule::Body(kernel(body));
        Ok(())
    }

    /// `read` as `clause` of an engine, which reads `running.…`, `waiting.…`
    /// and `batch.…` as `ENGINE_VALUES` says, and no bare total; the clause
    /// outside it is restored after.
    pub(super) fn engine_expr<T>(
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
                    "`{list}.…` is a value of an engine, read in its `tokens cap`, `worker` and \
                     `scheduler`"
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
    /// clause, or why the clause reads none. A name renamed since (#139),
    /// `ntok` for `tokens`, is answered with the engine's name for it. A
    /// name the program declares (a `let`, an attribute, a `def`) is the
    /// program's, as the linker reads it before a context variable.
    pub(super) fn retired_in_engine(&self, at: usize, written: &str) -> PResult<()> {
        let Some(clause) = self.in_engine else {
            return Ok(());
        };
        if self.definitions.iter().any(|(d, _)| d == written)
            || self.defs.iter().any(|d| d.name == written)
        {
            return Ok(());
        }
        let name = match RENAMED.iter().find(|(old, _)| *old == written) {
            Some((_, new)) => *new,
            None => written,
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
                format!("in an engine, `{written}` is `{now}` here: one value, one name"),
            ),
            None => self.err_at(
                at,
                format!(
                    "`{written}` is `{value}`, and {}",
                    not_read_in(value, clause)
                ),
            ),
        }
    }

    /// The engine whose scheduler admits the queue of the pool `po`: the
    /// engine it is on, or the one its `on ENGINE.DEVICE` names (which
    /// `check_admitted_by` has seen runs there); none for a pool `on
    /// DEVICE`, the engine's KV too, where a hold is admitted as soon as it
    /// fits. The one rule: `link_engines` sets `admit_via` by it,
    /// `check_engine_runs` reads it, and the outline says it.
    pub(super) fn admitted_by(&self, po: &PoolOn) -> Option<String> {
        if self.engines.iter().any(|e| e.name == po.on.owner) {
            Some(po.on.owner.clone())
        } else {
            po.on.admitted_by.clone()
        }
    }

    /// The outline's devices and engines, once the program is read: each
    /// one's capacities and the pool on each, and an engine's device's
    /// memory, the first `pool … on` it, as `link_engines` takes it (which
    /// refuses a second). As written: two pools on an engine's device are
    /// both listed, the first `pool … on` the memory.
    pub(super) fn record_places(&mut self, prog: &Program) {
        let Some(r) = self.outline.as_ref() else {
            return;
        };
        let caps = |owner: &str, caps: &[Capacity]| -> Vec<crate::frontend::outline::Capacity> {
            let on = |c: &Capacity| {
                self.pools_on
                    .iter()
                    .find(|po| po.on.owner == owner && po.cap == c.name)
            };
            caps.iter()
                .map(|c| crate::frontend::outline::Capacity {
                    name: c.name.clone(),
                    range: r.token(&self.toks, c.at),
                    pool: on(c).map(|po| prog.pools[po.pool].name.clone()),
                    // as `link_engines` sets it, from the one rule
                    admitted_by: on(c).and_then(|po| self.admitted_by(po)),
                })
                .collect()
        };
        let devices: Vec<_> = self
            .devices
            .iter()
            .map(|d| {
                let caps = caps(&d.name, &d.caps);
                let engine = self.engines.iter().find(|e| e.device == d.name);
                crate::frontend::outline::Device {
                    name: d.name.clone(),
                    engine: engine.map(|e| e.name.clone()),
                    memory: engine.and(
                        self.pools_on
                            .iter()
                            .find(|po| po.on.owner == d.name)
                            .map(|po| prog.pools[po.pool].name.clone()),
                    ),
                    caps,
                }
            })
            .collect();
        let engines: Vec<_> = self
            .engines
            .iter()
            .map(|e| crate::frontend::outline::Engine {
                name: e.name.clone(),
                device: e.device.clone(),
                caps: caps(&e.name, &e.caps),
            })
            .collect();
        // each queue's free places for a pool, as `pool … on` spells them
        // inside it (#645 B6)
        let spots: Vec<(String, Vec<crate::frontend::outline::Spot>)> = self
            .queues
            .iter()
            .map(|q| (q.name.clone(), self.pool_spots(&q.name)))
            .collect();
        let r = self.outline.as_mut().expect("checked above");
        r.out.devices = devices;
        r.out.engines = engines;
        for (name, pools) in spots {
            if let Some(q) = r.out.queues.iter_mut().find(|q| q.name == name) {
                q.accepts.pools = pools;
            }
        }
    }

    /// The capacities of queue `q`'s engine and devices no pool is on yet,
    /// each as `pool NAME on …` may spell it in `q`, and who would admit it
    /// (`admitted_by`'s rule). A device its engine runs on holds one pool
    /// (`E-device-two-pools`): once it has one, it offers none.
    fn pool_spots(&self, q: &str) -> Vec<crate::frontend::outline::Spot> {
        use crate::frontend::outline::Spot;
        let mut out = vec![];
        if let Some(e) = self.engines.iter().find(|e| e.name == q) {
            for c in e.caps.iter().filter(|c| !self.has_pool(q, &c.name)) {
                out.push(Spot {
                    on: q.to_string(),
                    cap: c.name.clone(),
                    admitted_by: Some(q.to_string()),
                });
            }
        }
        let prefix = format!("{q}.");
        for d in &self.devices {
            let Some(bare) = d.name.strip_prefix(&prefix) else {
                continue;
            };
            let engine = self.engines.iter().find(|e| e.device == d.name);
            if engine.is_some() && d.caps.iter().any(|c| self.has_pool(&d.name, &c.name)) {
                continue;
            }
            for c in d.caps.iter().filter(|c| !self.has_pool(&d.name, &c.name)) {
                // `on gpu`: no engine admits it
                out.push(Spot {
                    on: bare.to_string(),
                    cap: c.name.clone(),
                    admitted_by: None,
                });
                // `on Q.gpu`, the engine that runs there: its scheduler does
                if let Some(e) = engine {
                    out.push(Spot {
                        on: format!("{}.{bare}", e.name),
                        cap: c.name.clone(),
                        admitted_by: Some(e.name.clone()),
                    });
                }
            }
        }
        out
    }

    /// Link the engines, their devices and the pools on them, once the
    /// program is read: a pool on an engine, or on its device as
    /// `on ENGINE.DEVICE`, is admitted by the engine; the pool on a device is
    /// its engine's memory; `waiting.count` counts the queues the engine
    /// admits, each member of a family.
    pub(super) fn link_engines(&mut self, prog: &mut Program) -> PResult<()> {
        let via = |e: Option<String>| {
            e.map(|name| Ref {
                span: None,
                name,
                index: None,
            })
        };
        for po in &self.pools_on {
            let name = prog.pools[po.pool].name.clone();
            if self.engines.iter().any(|e| e.name == po.on.owner) {
                prog.pools[po.pool].admit_via = via(self.admitted_by(po));
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
                    // which of the two is the KV is the program's to say:
                    // the help names both and the one way the other goes
                    format!(
                        "{}\nhelp: the device an engine runs on holds one pool, its KV: keep \
                         `{}` or `{name}` there, and declare the other on no device, above the \
                         engine (`pool NAME {{ cap …; }}`)",
                        Code::DeviceTwoPools.message(format_args!(
                            "`{}` and `{name}` are both on `{}`: which is `{}`'s KV would be a \
                             choice the program did not make",
                            m.name, po.on.owner, engine.name
                        )),
                        m.name
                    ),
                );
            }
            s.memory = Some(Ref {
                span: None,
                name: name.clone(),
                index: None,
            });
            prog.pools[po.pool].admit_via = via(self.admitted_by(po));
        }
        for (owner, caps) in self
            .devices
            .iter()
            .map(|d| (&d.name, &d.caps))
            .chain(self.engines.iter().map(|e| (&e.name, &e.caps)))
        {
            for c in caps {
                let declared = self.has_pool(owner, &c.name);
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
            // a family's members are queues each, so each is counted
            let queues: Vec<Ref> = prog
                .pools
                .iter()
                .filter(|p| p.admit_via.as_ref().is_some_and(|r| r.name == e.name))
                .flat_map(|p| {
                    let members: Vec<Option<usize>> = if p.array {
                        (0..p.count).map(Some).collect()
                    } else {
                        vec![None]
                    };
                    members.into_iter().map(|i| Ref {
                        span: None,
                        name: p.name.clone(),
                        index: i.map(|i| Box::new(Expr::Num(i as f64))),
                    })
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
            match &mut s.schedule {
                Schedule::Procedure(o) => o.iter_mut().for_each(|o| o.substitute(&binds)),
                Schedule::Body(body) => substitute_body(body, &binds),
            }
        }
        Ok(())
    }

    /// Record that `prog.pools[pool]` is `on.owner`'s capacity `cap`.
    pub(super) fn pool_on(&mut self, pool: usize, cap: String, on: On) -> PResult<()> {
        let po = PoolOn { pool, cap, on };
        if self.has_pool(&po.on.owner, &po.cap) {
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

    /// Whether a pool is declared on `owner`'s capacity `cap` (a pool read
    /// so far: while the program is read, the pools above).
    fn has_pool(&self, owner: &str, cap: &str) -> bool {
        self.pools_on
            .iter()
            .any(|po| po.on.owner == owner && po.cap == cap)
    }

    /// The capacities of `owner` no pool above is on yet: the ones a pool
    /// of another name may have meant (`kv2` is no misspelt `kv` when a
    /// pool is on `kv` already). A pool declared below is not seen.
    fn free_caps<'a>(
        &'a self,
        owner: &'a str,
        caps: &'a [Capacity],
    ) -> impl Iterator<Item = &'a str> + 'a {
        caps.iter()
            .filter(move |c| !self.has_pool(owner, &c.name))
            .map(|c| c.name.as_str())
    }

    /// `pool NAME on OWNER`: the capacity it is, as `(cap, count, array)`;
    /// `at` is where NAME is written, `o_at` where OWNER is.
    pub(super) fn capacity_of(
        &self,
        at: usize,
        o_at: usize,
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
                // the device an engine runs on holds one pool, its KV: one
                // capacity more would be the two pools `link_engines`
                // refuses, so the way out is E-device-two-pools'
                None if !d.caps.is_empty() && self.engines.iter().any(|e| e.device == d.name) => {
                    self.err_at(
                        at,
                        format!(
                            "{}\nhelp: the device an engine runs on holds one pool, its KV: \
                             declare `{name}` on no device, above the engine (`pool {name} \
                             {{ cap …; }}`)",
                            Code::PoolNoCapacity
                                .message(format_args!("{} has no capacity `{name}`", shown(owner)))
                        ),
                    )
                }
                None => self.err_at(
                    at,
                    no_capacity(&shown(owner), name, self.free_caps(owner, &d.caps)),
                ),
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
                None => self.err_at(
                    at,
                    no_capacity(&format!("`{owner}`"), name, self.free_caps(owner, &e.caps)),
                ),
            };
        }
        let help = crate::frontend::diagnostic::suggestion(
            owner,
            self.devices
                .iter()
                .map(|d| d.name.as_str())
                .chain(self.engines.iter().map(|e| e.name.as_str()))
                .filter(|n| !n.contains('.')),
        )
        .map(|n| format!("did you mean `{n}`?"))
        .unwrap_or_else(|| "declare the device or the engine above the pool".into());
        self.err_at(
            o_at,
            format!(
                "no device or engine `{owner}`: a pool is on one declared before it\nhelp: {help}"
            ),
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

/// The refusal of `pool name on owner` when `owner` (as `shown`) declares
/// no capacity `name`: a pool is a capacity of its device or engine. How
/// much it holds is the program's, so the help names the line, not its
/// amount; a capacity of a nearby name is likely the one meant.
fn no_capacity<'a>(owner: &str, name: &str, caps: impl Iterator<Item = &'a str>) -> String {
    let near = crate::frontend::diagnostic::suggestion(name, caps)
        .map(|c| format!("\nhelp: did you mean `{c}`?"))
        .unwrap_or_default();
    format!(
        "{}\nhelp: a pool on {owner} is one of its capacities: declare `{name} cap …;` in it{near}",
        crate::frontend::diagnostic::Code::PoolNoCapacity
            .message(format_args!("{owner} has no capacity `{name}`"))
    )
}

/// The `pool … on …` that declares capacity `cap` of `owner`, as written
/// where `owner` is declared.
fn pool_on_as_written(cap: &str, owner: &str) -> String {
    match owner.split_once('.') {
        Some((q, local)) => format!("`pool {cap} on {local} {{ … }}` in queue `{q}`"),
        None => format!("`pool {cap} on {owner} {{ … }}`"),
    }
}

/// The gate `VLLM_GATE`, once each `def` is written out: vLLM's procedure
/// has one spelling (#721). A gate that computes the same written otherwise
/// (`!running.preempted`, `0 == running.preempted`, `running.preempted <
/// 1`) makes the schedule a body of the program's own, which runs alike.
fn vllm_gate(e: &Expr) -> bool {
    matches!(
        strip(e),
        Expr::Binary(BinOp::Eq, a, b)
            if matches!(strip(a), Expr::Var(n) if n == crate::ir::CtxVar::Preempted.name())
                && matches!(strip(b), Expr::Num(z) if *z == 0.0)
    )
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

/// Each statement of a schedule, an `if`'s included, before its own.
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
        || match &s.schedule {
            Schedule::Procedure(o) => o.iter().any(|e| reads_name(e, n)),
            Schedule::Body(b) => body_reads(b, n),
        }
}
