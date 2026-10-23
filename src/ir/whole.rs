//! Token counts are whole numbers (#661, `docs/design/whole-tokens.md`).
//!
//! A number of tokens is a count: a request's input and output, what a hold
//! takes of a pool whose units are positions of a token sequence (a KV
//! pool), what it reuses and caches of it, a prefix's segments, a `load`,
//! the tokens a run on an engine computes, an engine's `tokens cap` and
//! `each at most`. The interpreter keeps them in `f64`, which is exact for
//! whole numbers: a cached prefix written in two releases then reads back as
//! the sum the workload wrote. With fractions it reads an ulp short, and a
//! program's `cached >= K` misses a prefix that is all there (#661), as a
//! prefill summed from its chunks did before (#479).
//!
//! So the check refuses a program whose token count may be fractional, by an
//! inference over the expression (`Whole::is`): what the program writes is
//! whole when every part is (an integer constant, `floor`, `ceil`, a count, a
//! sum or product of whole numbers, a comparison), and the program writes the
//! rounding it means, `floor(…)` or `ceil(…)`: which way to round is the
//! program's (design criterion 2). An attribute is whole when every
//! assignment to it is (the greatest fixpoint: `set K = K + n + o` keeps a
//! whole `K` whole). The trace's `new` and `out` are whole by the reader
//! (`trace::Corpus::from_csv`).
//!
//! What is not a token count is not checked: seconds, a stage's work, a
//! hold's `reserve` (an admission test, which SGLang computes with a real
//! `new_token_ratio`), and the units of a pool that holds no token sequence
//! (request slots, a router's weighted count: ThunderAgent's acting-token
//! weight).

use super::*;

/// The hint every refusal ends with: the rounding is the program's.
const ROUND: &str = "write the rounding it means: `floor(…)` or `ceil(…)` (#661)";

/// Why a pool's units are positions of a token sequence (a KV pool): the
/// first thing the program says that makes it one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kv {
    /// It has a `block`.
    Block,
    /// It is the memory of the engine (a stage).
    Memory(usize),
    /// A hold at the statement takes it with this clause (`cache`, `reuse`,
    /// `prefix`).
    Clause(&'static str, BlockId, usize),
    /// The engine's admission (a stage) takes it with this clause.
    Taking(&'static str, usize),
    /// A `load` at the statement.
    Load(BlockId, usize),
    /// A growing run at the statement.
    Growing(BlockId, usize),
    /// The KV pool spills to it.
    Spill(usize),
}

impl Kv {
    /// The reason, as a message says it.
    fn said(self, p: &Program) -> String {
        match self {
            Kv::Block => "its `block`".into(),
            Kv::Memory(s) => format!("the memory of engine `{}`", p.stages[s].name),
            Kv::Clause(c, b, k) => format!("the `{c}` of the hold at {}", place(b, k)),
            Kv::Taking(c, s) => format!("engine `{}`'s `{c}`", p.stages[s].name),
            Kv::Load(b, k) => format!("the `load` at {}", place(b, k)),
            Kv::Growing(b, k) => format!("the growing run at {}", place(b, k)),
            Kv::Spill(q) => format!("pool `{}` spills to it", p.pools[q].name),
        }
    }
}

/// Whether an attribute is whole, and if not, why.
#[derive(Clone, Copy, Debug, PartialEq)]
enum AttrWhole {
    Whole,
    /// The assignment at this statement may make it fractional.
    SetAt(BlockId, usize),
    /// What gives it is not an assignment: the trace's `think`, a time an
    /// engine records, an explicit session's fraction.
    Given,
}

/// Where a token count is and what it is, for a message.
struct Use {
    /// What the count is: "what `hold kv` takes".
    what: String,
    at: Option<(BlockId, usize)>,
    decl: Option<Decl>,
    /// The KV pool whose units it counts, when that is why it is one.
    pool: Option<usize>,
}

/// The inference: which pools are KV pools, which attributes and registers
/// are whole, which engines' budgets are.
struct Whole {
    kv: Vec<Option<Kv>>,
    attr: Vec<AttrWhole>,
    reg: Vec<bool>,
    budget: Vec<bool>,
}

fn members(r: &CRef) -> std::ops::Range<usize> {
    r.base..r.base + r.count.max(1)
}

fn whole_number(x: f64) -> bool {
    x.is_infinite() || x.fract() == 0.0
}

impl Whole {
    /// Whether `e` is a whole number whatever the run.
    fn is(&self, e: &CExpr) -> bool {
        self.first_part(e, &[]).is_none()
    }

    /// The first part of `e` that makes it possibly fractional, outermost
    /// first: a leaf (a constant, a draw, an attribute, a context variable,
    /// a function) or an operator (`/`, `^`). The attributes of `seen` are
    /// read as whole: a message follows each once.
    fn first_part<'e>(&self, e: &'e CExpr, seen: &[usize]) -> Option<&'e CExpr> {
        let both = |a: &'e CExpr, b: &'e CExpr| {
            self.first_part(a, seen)
                .or_else(|| self.first_part(b, seen))
        };
        match e {
            CExpr::Num(x) => (!whole_number(*x)).then_some(e),
            CExpr::Attr(a) => (!seen.contains(a) && self.attr[*a] != AttrWhole::Whole).then_some(e),
            CExpr::Ctx(c) => (!c.whole()).then_some(e),
            CExpr::Reg(r) => (!self.reg[*r]).then_some(e),
            CExpr::Agg(a, _) => (*a != Agg::Count).then_some(e),
            CExpr::Cost(_, x) => self.first_part(x, seen),
            CExpr::Unary(UnOp::Not, _) => None,
            CExpr::Unary(UnOp::Neg, x) => self.first_part(x, seen),
            CExpr::Binary(op, a, b) => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul => both(a, b),
                BinOp::Div | BinOp::Pow => Some(e),
                BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::And
                | BinOp::Or => None,
            },
            CExpr::Cond(_, a, b) => both(a, b),
            CExpr::Sample(k, xs) => match k {
                DistKind::Bernoulli => None,
                DistKind::Det => xs.first().and_then(|x| self.first_part(x, seen)),
                DistKind::Exp | DistKind::Uniform | DistKind::Erlang | DistKind::H2 => Some(e),
            },
            CExpr::Call(f, args) => {
                let mut exprs = args.iter().filter_map(|a| match a {
                    CArg::Expr(x) => Some(x),
                    _ => None,
                });
                let mut refs = args.iter().filter_map(|a| match a {
                    CArg::Pool(r) | CArg::Stage(r) => Some(r),
                    _ => None,
                });
                match f {
                    Fun::Floor
                    | Fun::Ceil
                    | Fun::Queue
                    | Fun::Busy
                    | Fun::Holders
                    | Fun::Queued => None,
                    Fun::Min | Fun::Max | Fun::Abs => exprs.find_map(|x| self.first_part(x, seen)),
                    // a KV pool's units are whole; another pool's are the
                    // program's (a weighted count may be fractional)
                    Fun::Used | Fun::Free | Fun::CachedIn | Fun::Allocated => (!refs
                        .all(|r| members(r).all(|i| matches!(self.kv.get(i), Some(Some(_))))))
                    .then_some(e),
                    Fun::BudgetLeft => (!refs
                        .all(|r| members(r).all(|i| self.budget.get(i) == Some(&true))))
                    .then_some(e),
                    Fun::Work
                    | Fun::Sqrt
                    | Fun::Exp
                    | Fun::Ln
                    | Fun::Price
                    | Fun::EstLambda
                    | Fun::EstRho
                    | Fun::EstWait => Some(e),
                }
            }
        }
    }
}

impl CtxVar {
    /// Whether the context variable is a whole number: a count, or a number
    /// of tokens the interpreter derives from whole ones. A time is not, nor
    /// the attention work `Σ n (K + n/2)`. Every variable is listed, so a new
    /// one is classified where it is added.
    fn whole(self) -> bool {
        match self {
            CtxVar::Now | CtxVar::Waited | CtxVar::Age | CtxVar::Last | CtxVar::Attn => false,
            CtxVar::Size
            | CtxVar::Queued
            | CtxVar::N
            | CtxVar::Ntok
            | CtxVar::Ndec
            | CtxVar::Npre
            | CtxVar::Nres
            | CtxVar::Kvb
            | CtxVar::Kvp
            | CtxVar::Decoding
            | CtxVar::Admission
            | CtxVar::Remaining
            | CtxVar::Position
            | CtxVar::Admitted
            | CtxVar::Preempted
            | CtxVar::Demand
            | CtxVar::Served
            | CtxVar::Arrived
            | CtxVar::Input
            | CtxVar::Output
            | CtxVar::Known
            | CtxVar::Hit
            | CtxVar::Finished
            | CtxVar::Loaded
            | CtxVar::Requeued => true,
        }
    }
}

/// Why `part` makes an expression possibly fractional, for a message.
fn reason(p: &Program, part: &CExpr) -> String {
    match part {
        CExpr::Num(x) => format!("`{x}` is a fraction"),
        CExpr::Binary(BinOp::Div, ..) => "`/` divides".into(),
        CExpr::Binary(BinOp::Pow, ..) => "`^` may give a fraction".into(),
        CExpr::Sample(k, _) => format!("`~{}` draws a real number", k.name()),
        CExpr::Ctx(c) => format!("`{}` is not a count", c.name()),
        CExpr::Attr(a) => format!("`{}` may not be whole", p.attrs[*a]),
        CExpr::Reg(r) => format!("`{}` may not be whole", p.registers[*r].name),
        CExpr::Call(f, _) => match f {
            Fun::Used | Fun::Free | Fun::CachedIn | Fun::Allocated => format!(
                "`{}` reads a pool that holds no tokens, whose units may be fractions",
                f.name()
            ),
            Fun::BudgetLeft => "`budget_left` reads a `tokens cap` that may not be whole".into(),
            _ => format!("`{}` may give a fraction", f.name()),
        },
        CExpr::Agg(a, _) => format!("`{}` may give a fraction", a.name()),
        _ => "it may be fractional".into(),
    }
}

impl Program {
    /// Which pools are KV pools, and why: one with a `block`, an engine's
    /// memory, every pool of a hold or an engine's admission with `cache`,
    /// `reuse` or `prefix` (the prefix cache's), a `load`'s and a growing
    /// run's, and a pool a KV pool spills to.
    fn kv_pools(&self) -> Vec<Option<Kv>> {
        let mut kv: Vec<Option<Kv>> = self
            .pools
            .iter()
            .map(|q| q.block.map(|_| Kv::Block))
            .collect();
        let mark = |kv: &mut Vec<Option<Kv>>, r: &CRef, why: Kv| {
            for i in members(r) {
                if let Some(k @ None) = kv.get_mut(i) {
                    *k = Some(why);
                }
            }
        };
        // the clause that makes a hold's pools the prefix cache's
        let clause = |reuse: bool, cache: bool, prefix: bool| {
            [("cache", cache), ("reuse", reuse), ("prefix", prefix)]
                .into_iter()
                .find(|(_, x)| *x)
                .map(|(c, _)| c)
        };
        for (s, st) in self.stages.iter().enumerate() {
            if let CStageKind::Step(sp) = &st.kind {
                if let Some(k @ None) = sp.memory.and_then(|m| kv.get_mut(m)) {
                    *k = Some(Kv::Memory(s));
                }
                if let Some(r) = &sp.request
                    && let Some(c) =
                        clause(r.reuse.is_some(), r.cache.is_some(), r.prefix.is_some())
                {
                    for (q, _, _) in &r.pools {
                        mark(&mut kv, q, Kv::Taking(c, s));
                    }
                }
            }
        }
        for (b, block) in self.blocks.iter().enumerate() {
            for (k, st) in block.iter().enumerate() {
                match st {
                    CStmt::Hold {
                        pools,
                        reuse,
                        cache,
                        prefix,
                        ..
                    } => {
                        if let Some(c) = clause(reuse.is_some(), cache.is_some(), prefix.is_some())
                        {
                            for (q, _, _) in pools {
                                mark(&mut kv, q, Kv::Clause(c, b, k));
                            }
                        }
                    }
                    CStmt::Load(r, _) => mark(&mut kv, r, Kv::Load(b, k)),
                    CStmt::Run {
                        growing: Some(r), ..
                    } => mark(&mut kv, r, Kv::Growing(b, k)),
                    _ => {}
                }
            }
        }
        // what a KV pool spills is cached KV in the pool it spills to
        loop {
            let mut changed = false;
            for (i, q) in self.pools.iter().enumerate() {
                if let Some(s) = &q.spill
                    && kv[i].is_some()
                    && kv.get(s.to) == Some(&None)
                {
                    kv[s.to] = Some(Kv::Spill(i));
                    changed = true;
                }
            }
            if !changed {
                return kv;
            }
        }
    }

    fn whole(&self) -> Whole {
        let mut w = Whole {
            kv: self.kv_pools(),
            attr: vec![AttrWhole::Whole; self.attrs.len()],
            reg: self
                .registers
                .iter()
                .map(|r| whole_number(r.init))
                .collect(),
            budget: vec![false; self.stages.len()],
        };
        // what is whole by construction: a `choose`'s index, the serial, the
        // turn, the trace's `new` and `out` (its reader refuses fractions),
        // `cached` and `computed` and what an engine records of tokens; what
        // is not: the trace's `think` (seconds), the times an engine records
        let mut sets: Vec<(usize, &CExpr)> = vec![];
        let given = |slot: usize, w: &mut Whole| {
            if let Some(a) = w.attr.get_mut(slot) {
                *a = AttrWhole::Given;
            }
        };
        given(self.slot_think, &mut w);
        for st in &self.stages {
            if let CStageKind::Step(sp) = &st.kind {
                if let Some(r) = &sp.request {
                    given(r.records.admitted, &mut w);
                    given(r.records.first, &mut w);
                }
                if let Some(c) = &sp.connector {
                    given(c.load_done, &mut w);
                }
                fn iter_sets<'a>(body: &'a [CIter], out: &mut Vec<(usize, &'a CExpr)>) {
                    for it in body {
                        match it {
                            CIter::Set(r, e) => out.push((*r, e)),
                            CIter::Branch(_, a, b) => {
                                iter_sets(a, out);
                                iter_sets(b, out);
                            }
                            CIter::Serve { .. } | CIter::Admit { .. } => {}
                        }
                    }
                }
                iter_sets(sp.iteration.as_deref().unwrap_or(&[]), &mut sets);
            }
        }
        if let CArrival::Sessions(ss) = &self.arrival {
            for s in ss {
                for (a, x) in s.attrs.iter().chain(s.turns.iter().flatten()) {
                    if !whole_number(*x) {
                        given(*a, &mut w);
                    }
                }
            }
        }
        // the greatest fixpoint: an attribute is whole until an assignment
        // says it may not be; each pass refutes one at least, or stops
        loop {
            let mut changed = false;
            for (i, st) in self.stages.iter().enumerate() {
                if let CStageKind::Step(sp) = &st.kind {
                    let b = w.is(&sp.budget);
                    if w.budget[i] != b {
                        w.budget[i] = b;
                        changed = true;
                    }
                }
            }
            for (b, block) in self.blocks.iter().enumerate() {
                for (k, st) in block.iter().enumerate() {
                    if let CStmt::Set(a, e) = st
                        && w.attr[*a] == AttrWhole::Whole
                        && !w.is(e)
                    {
                        w.attr[*a] = AttrWhole::SetAt(b, k);
                        changed = true;
                    }
                }
            }
            for (r, e) in &sets {
                if w.reg[*r] && !w.is(e) {
                    w.reg[*r] = false;
                    changed = true;
                }
            }
            if !changed {
                return w;
            }
        }
    }

    /// The refusal of a token count `e` that may be fractional: at the
    /// assignment that made it so when it is an attribute's (where the
    /// rounding goes), else at the count itself.
    fn not_whole(&self, w: &Whole, e: &CExpr, u: &Use, names: CtxNames) -> Invalid {
        // why the count is one, when a pool's being a KV pool says so
        let holds = u.pool.and_then(|q| {
            let why = w.kv[q]?;
            Some(format!(
                "`{}` holds tokens: {}",
                self.pools[q].name,
                why.said(self)
            ))
        });
        let refuse = |message: String, at: Option<(BlockId, usize)>| Invalid {
            message,
            at,
            decl: if at.is_some() { None } else { u.decl },
        };
        let count = self.show_expr_with(e, names);
        // the count itself is named: fractional by itself, or through
        // attributes whose cause is not an assignment
        let direct = |why: String| {
            let holds = holds.as_ref().map_or(String::new(), |h| format!(" ({h})"));
            refuse(
                format!(
                    "{} is a number of tokens{holds}, and `{count}` may not be a whole number \
                     ({why}); {ROUND}",
                    u.what
                ),
                u.at,
            )
        };
        // follow the attributes back to the first assignment fractional by
        // itself; one already followed is read as whole, so the next part is
        // why its assignment is fractional
        let mut seen = vec![];
        let mut cur = e;
        let mut via: Option<(usize, BlockId, usize)> = None;
        loop {
            let Some(part) = w.first_part(cur, &seen) else {
                // (a cycle of attributes: the count is named)
                let part = w.first_part(e, &[]).expect("a fractional count");
                return direct(reason(self, part));
            };
            match part {
                CExpr::Attr(a) => {
                    seen.push(*a);
                    match w.attr[*a] {
                        AttrWhole::SetAt(b, k) => {
                            let CStmt::Set(_, x) = &self.blocks[b][k] else {
                                unreachable!("an assignment")
                            };
                            via = Some((*a, b, k));
                            cur = x;
                        }
                        AttrWhole::Given | AttrWhole::Whole => {
                            let why = if *a == self.slot_think {
                                format!("`{}` is the trace's seconds", self.attrs[*a])
                            } else {
                                format!(
                                    "`{}` is a time, or an explicit session's fraction",
                                    self.attrs[*a]
                                )
                            };
                            return direct(why);
                        }
                    }
                }
                part => {
                    let Some((a, b, k)) = via else {
                        return direct(reason(self, part));
                    };
                    // the count, unless it is the attribute itself, and why
                    // it is one
                    let mut use_ = u.what.clone();
                    if count != self.attrs[a] {
                        use_ = format!("{use_}: `{count}`");
                    }
                    if let Some(h) = &holds {
                        use_ = format!("{use_}; {h}");
                    }
                    return refuse(
                        format!(
                            "`{}` is a number of tokens ({use_}), and `{}` may not be a whole \
                             number ({}); {ROUND}",
                            self.attrs[a],
                            self.show_expr(cur),
                            reason(self, part)
                        ),
                        Some((b, k)),
                    );
                }
            }
        }
    }

    /// Every token count is a whole number (#661): see the module.
    pub(super) fn validate_whole(&self) -> Result<(), Invalid> {
        let w = self.whole();
        let check = |e: Option<&CExpr>, u: Use, names: CtxNames| -> Result<(), Invalid> {
            match e {
                Some(e) if !w.is(e) => Err(self.not_whole(&w, e, &u, names)),
                _ => Ok(()),
            }
        };
        let plain: CtxNames = CtxVar::name;
        let budget: CtxNames = crate::frontend::parser::tokens_cap_name;
        let request: CtxNames =
            |v| crate::frontend::parser::engine_name(v.name(), "taking").unwrap_or(v.name());
        let name = |q: &CRef| &self.pools[q.base].name;
        for (i, q) in self.pools.iter().enumerate() {
            let Some(why) = w.kv[i] else {
                continue;
            };
            let bad = |what: &str, x: f64| {
                (!whole_number(x)).then(|| Invalid {
                    message: format!(
                        "pool `{}` holds tokens ({}), and its {what} {x} is not a whole number \
                         of them; {ROUND}",
                        q.name,
                        why.said(self)
                    ),
                    at: None,
                    decl: Some(Decl::Pool(i)),
                })
            };
            if let Some(e) = bad("cap", q.cap).or_else(|| q.block.and_then(|b| bad("block", b))) {
                return Err(e);
            }
        }
        for (s, st) in self.stages.iter().enumerate() {
            let CStageKind::Step(sp) = &st.kind else {
                continue;
            };
            let n = &st.name;
            let u = |what: String, pool: Option<usize>| Use {
                what,
                at: None,
                decl: Some(Decl::Stage(s)),
                pool,
            };
            check(
                Some(&sp.budget),
                u(format!("engine `{n}`'s `tokens cap`"), None),
                budget,
            )?;
            check(
                Some(&sp.chunk),
                u(format!("engine `{n}`'s `each at most`"), None),
                budget,
            )?;
            check(
                sp.granule.as_ref(),
                u(format!("engine `{n}`'s granule"), None),
                budget,
            )?;
            if let Some(r) = &sp.request {
                for (q, units, _) in &r.pools {
                    if w.kv[q.base].is_some() {
                        let what = format!("what engine `{n}` takes of `{}`", name(q));
                        check(Some(units), u(what, Some(q.base)), request)?;
                    }
                }
                check(
                    r.reuse.as_ref(),
                    u(format!("engine `{n}`'s `reuse`"), None),
                    request,
                )?;
                check(
                    r.cache.as_ref(),
                    u(format!("engine `{n}`'s `cache`"), None),
                    request,
                )?;
                for (_, len) in r.prefix.iter().flatten() {
                    let what = format!("a segment of engine `{n}`'s `prefix`");
                    check(Some(len), u(what, None), request)?;
                }
            }
            if let Some(c) = &sp.connector {
                let what = |x: &str| u(format!("engine `{n}`'s connector's `{x}`"), None);
                for l in &c.loads {
                    // of several, the one through its stage
                    let what = match c.loads.len() {
                        1 => what("load"),
                        _ => u(
                            format!(
                                "engine `{n}`'s connector's `load` at `{}`",
                                self.stages[l.stage.base].name
                            ),
                            None,
                        ),
                    };
                    check(Some(&l.amount), what, request)?;
                }
                check(c.save.as_ref().map(|s| &s.amount), what("save"), request)?;
            }
        }
        let on_engine = |r: &CRef| {
            members(r).any(|i| {
                matches!(
                    self.stages.get(i).map(|s| &s.kind),
                    Some(CStageKind::Step(_))
                )
            })
        };
        for (b, block) in self.blocks.iter().enumerate() {
            for (k, st) in block.iter().enumerate() {
                let u = |what: String, pool: Option<usize>| Use {
                    what,
                    at: Some((b, k)),
                    decl: None,
                    pool,
                };
                match st {
                    CStmt::Hold {
                        pools,
                        reuse,
                        cache,
                        prefix,
                        ..
                    } => {
                        for (q, units, _) in pools {
                            if w.kv[q.base].is_some() {
                                let what = format!("what `hold {}` takes", name(q));
                                check(Some(units), u(what, Some(q.base)), plain)?;
                            }
                        }
                        check(reuse.as_ref(), u("a hold's `reuse`".into(), None), plain)?;
                        check(cache.as_ref(), u("a hold's `cache`".into(), None), plain)?;
                        for (_, len) in prefix.iter().flatten() {
                            check(Some(len), u("a `prefix` segment".into(), None), plain)?;
                        }
                    }
                    CStmt::Grow(q, e) | CStmt::Shrink(q, e) if w.kv[q.base].is_some() => {
                        let verb = if matches!(st, CStmt::Grow(..)) {
                            "grow"
                        } else {
                            "shrink"
                        };
                        let what = format!("what `{verb} {}` moves", name(q));
                        check(Some(e), u(what, Some(q.base)), plain)?
                    }
                    CStmt::Load(q, e) => {
                        let what = format!("what `load {}` loads", name(q));
                        check(Some(e), u(what, None), plain)?
                    }
                    CStmt::Request {
                        stage,
                        input,
                        output,
                    } => {
                        let n = &self.stages[stage.base].name;
                        check(Some(input), u(format!("`{n}`'s input"), None), plain)?;
                        check(Some(output), u(format!("`{n}`'s output"), None), plain)?;
                    }
                    CStmt::Run {
                        stage,
                        work,
                        growing,
                        ..
                    } if on_engine(stage) || growing.is_some() => {
                        let what =
                            format!("what a run on `{}` computes", self.stages[stage.base].name);
                        check(Some(work), u(what, None), plain)?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(a: usize) -> CExpr {
        CExpr::Attr(a)
    }
    fn num(x: f64) -> CExpr {
        CExpr::Num(x)
    }
    fn bin(op: BinOp, a: CExpr, b: CExpr) -> CExpr {
        CExpr::Binary(op, Box::new(a), Box::new(b))
    }
    fn call(f: Fun, xs: Vec<CExpr>) -> CExpr {
        CExpr::Call(f, xs.into_iter().map(CArg::Expr).collect())
    }
    fn pool(base: usize) -> CArg {
        CArg::Pool(CRef {
            base,
            count: 1,
            index: None,
        })
    }

    /// Two attributes, 0 whole and 1 not; pool 0 a KV pool, 1 not.
    fn w() -> Whole {
        Whole {
            kv: vec![Some(Kv::Block), None],
            attr: vec![AttrWhole::Whole, AttrWhole::Given],
            reg: vec![],
            budget: vec![],
        }
    }

    #[test]
    fn whole_parts_make_whole_numbers() {
        let w = w();
        for e in [
            num(3.0),
            num(f64::INFINITY),
            bin(BinOp::Add, attr(0), bin(BinOp::Mul, num(2.0), attr(0))),
            bin(BinOp::Sub, attr(0), num(1.0)),
            call(Fun::Ceil, vec![bin(BinOp::Div, attr(1), num(3.0))]),
            // `floor(x, step)` is `floor(x / step) * step`
            bin(
                BinOp::Mul,
                call(Fun::Floor, vec![bin(BinOp::Div, attr(1), num(16.0))]),
                num(16.0),
            ),
            call(Fun::Min, vec![attr(0), num(7.0)]),
            bin(BinOp::Gt, attr(1), num(0.5)),
            CExpr::Cond(Box::new(attr(1)), Box::new(attr(0)), Box::new(num(1.0))),
            CExpr::Sample(DistKind::Bernoulli, vec![num(0.3)]),
            CExpr::Sample(DistKind::Det, vec![num(4.0)]),
            CExpr::Ctx(CtxVar::Known),
            CExpr::Cost(CostTarget::Pool { base: 0, count: 1 }, Box::new(attr(0))),
            CExpr::Call(Fun::CachedIn, vec![pool(0)]),
        ] {
            assert!(w.is(&e), "{e:?}");
        }
    }

    #[test]
    fn a_fraction_a_division_a_draw_a_time_may_not_be_whole() {
        let w = w();
        for e in [
            num(0.5),
            attr(1),
            bin(BinOp::Div, attr(0), num(2.0)),
            bin(BinOp::Pow, attr(0), num(2.0)),
            bin(BinOp::Mul, CExpr::Ctx(CtxVar::Now), num(0.0)),
            CExpr::Sample(DistKind::Exp, vec![num(500.0)]),
            CExpr::Sample(DistKind::Uniform, vec![num(1.0), num(3.0)]),
            CExpr::Sample(DistKind::Det, vec![num(0.5)]),
            CExpr::Cond(Box::new(attr(0)), Box::new(attr(0)), Box::new(num(0.5))),
            call(Fun::Max, vec![attr(0), attr(1)]),
            call(Fun::Sqrt, vec![num(4.0)]),
            CExpr::Ctx(CtxVar::Attn),
            // a pool that holds no tokens counts in the program's units
            CExpr::Call(Fun::Allocated, vec![pool(1)]),
        ] {
            assert!(!w.is(&e), "{e:?}");
        }
        // an attribute followed already is read as whole
        assert!(w.first_part(&attr(1), &[1]).is_none());
    }
}
