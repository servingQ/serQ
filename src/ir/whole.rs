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
//! rounding it means, `ceil(~exp(500))` or `floor(…) + 1`: which way to round
//! is the program's (design criterion 2). An attribute is whole when every
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

/// Why a statement, or a declaration, is about to be refused: the place of
/// the token count and the expression.
struct Use {
    /// What the count is, as a message names it: "`llm`'s input".
    what: String,
    at: Option<(BlockId, usize)>,
}

/// The inference: which pools are KV pools, which attributes and registers
/// are whole, which engines' budgets are.
struct Whole {
    kv: Vec<bool>,
    /// `None`: whole. `Some(at)`: not, because of the assignment at `at`
    /// (a block and index), or `None` inside for a source that is not an
    /// assignment (the trace's `think`, a time the engine records, an
    /// explicit session's value).
    attr: Vec<Option<Option<(BlockId, usize)>>>,
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
    /// Whether `e` is a whole number whatever the run; `attrs`: read an
    /// attribute as its inference says (true) or as whole (false), which
    /// tells an expression fractional by itself from one fractional through
    /// an attribute.
    fn is(&self, e: &CExpr, attrs: bool) -> bool {
        let none: &[usize] = &[];
        self.first_part(e, attrs.then_some(none)).is_none()
    }

    /// The first part of `e` that makes it possibly fractional, outermost
    /// first: a leaf (a constant, a draw, an attribute, a context variable,
    /// a function) or an operator (`/`, `^`). `attrs`: the attributes read
    /// as whole whatever the inference says (`None`: every one).
    fn first_part<'e>(&self, e: &'e CExpr, attrs: Option<&[usize]>) -> Option<&'e CExpr> {
        let both = |a: &'e CExpr, b: &'e CExpr| {
            self.first_part(a, attrs)
                .or_else(|| self.first_part(b, attrs))
        };
        match e {
            CExpr::Num(x) => (!whole_number(*x)).then_some(e),
            CExpr::Attr(a) => {
                (attrs.is_some_and(|s| !s.contains(a)) && self.attr[*a].is_some()).then_some(e)
            }
            CExpr::Ctx(c) => (!c.whole()).then_some(e),
            CExpr::Reg(r) => (!self.reg[*r]).then_some(e),
            CExpr::Agg(a, _) => (*a != Agg::Count).then_some(e),
            CExpr::Cost(_, x) => self.first_part(x, attrs),
            CExpr::Unary(UnOp::Not, _) => None,
            CExpr::Unary(UnOp::Neg, x) => self.first_part(x, attrs),
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
                DistKind::Det => xs.first().and_then(|x| self.first_part(x, attrs)),
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
                let all =
                    |v: &[bool], r: &CRef| members(r).all(|i| v.get(i).copied() == Some(true));
                match f {
                    Fun::Floor
                    | Fun::Ceil
                    | Fun::Queue
                    | Fun::Busy
                    | Fun::Holders
                    | Fun::Queued => None,
                    Fun::Min | Fun::Max | Fun::Abs => exprs.find_map(|x| self.first_part(x, attrs)),
                    // a KV pool's units are whole; another pool's are the
                    // program's (a weighted count may be fractional)
                    Fun::Used | Fun::Free | Fun::CachedIn | Fun::Allocated => {
                        (!refs.all(|r| all(&self.kv, r))).then_some(e)
                    }
                    Fun::BudgetLeft => (!refs.all(|r| all(&self.budget, r))).then_some(e),
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
    /// of tokens the interpreter derives from whole ones. A time (`now`,
    /// `waited`, `age`, `last`) is not, nor the attention work `Σ n (K +
    /// n/2)`.
    pub fn whole(self) -> bool {
        !matches!(
            self,
            CtxVar::Now | CtxVar::Waited | CtxVar::Age | CtxVar::Last | CtxVar::Attn
        )
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
                "`{}` reads a pool whose units are not tokens, which may be fractional",
                f.name()
            ),
            _ => format!("`{}` may give a fraction", f.name()),
        },
        CExpr::Agg(a, _) => format!("`{}` may give a fraction", a.name()),
        _ => "it may be fractional".into(),
    }
}

impl Program {
    /// The pools whose units are positions of a token sequence: one with a
    /// `block`, an engine's memory, every pool of a hold or an engine's
    /// request with `cache`, `reuse` or `prefix` (the prefix cache's), a
    /// `load`'s and a growing run's, and a pool a KV pool spills to.
    pub(crate) fn kv_pools(&self) -> Vec<bool> {
        let mut kv: Vec<bool> = self.pools.iter().map(|q| q.block.is_some()).collect();
        let mark = |kv: &mut Vec<bool>, r: &CRef| {
            for i in members(r) {
                if let Some(k) = kv.get_mut(i) {
                    *k = true;
                }
            }
        };
        for st in &self.stages {
            if let CStageKind::Step(sp) = &st.kind {
                if let Some(k) = sp.memory.and_then(|m| kv.get_mut(m)) {
                    *k = true;
                }
                if let Some(r) = &sp.request
                    && (r.cache.is_some() || r.reuse.is_some() || r.prefix.is_some())
                {
                    for (q, _, _) in &r.pools {
                        mark(&mut kv, q);
                    }
                }
            }
        }
        for st in self.blocks.iter().flatten() {
            match st {
                CStmt::Hold {
                    pools,
                    reuse,
                    cache,
                    prefix,
                    ..
                } if cache.is_some() || reuse.is_some() || prefix.is_some() => {
                    for (q, _, _) in pools {
                        mark(&mut kv, q);
                    }
                }
                CStmt::Load(r, _) => mark(&mut kv, r),
                CStmt::Run {
                    growing: Some(r), ..
                } => mark(&mut kv, r),
                _ => {}
            }
        }
        // what a KV pool spills is cached KV in the pool it spills to
        loop {
            let mut changed = false;
            for (i, q) in self.pools.iter().enumerate() {
                if let Some(s) = &q.spill
                    && kv[i]
                    && kv.get(s.to) == Some(&false)
                {
                    kv[s.to] = true;
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
            attr: vec![None; self.attrs.len()],
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
        let not = |slot: usize, w: &mut Whole| {
            if let Some(a) = w.attr.get_mut(slot) {
                *a = Some(None);
            }
        };
        not(self.slot_think, &mut w);
        for st in &self.stages {
            if let CStageKind::Step(sp) = &st.kind {
                if let Some(r) = &sp.request {
                    not(r.records.admitted, &mut w);
                    not(r.records.first, &mut w);
                }
                if let Some(c) = &sp.connector {
                    not(c.load_done, &mut w);
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
                        not(*a, &mut w);
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
                    let b = w.is(&sp.budget, true);
                    if w.budget[i] != b {
                        w.budget[i] = b;
                        changed = true;
                    }
                }
            }
            for (b, block) in self.blocks.iter().enumerate() {
                for (k, st) in block.iter().enumerate() {
                    if let CStmt::Set(a, e) = st
                        && w.attr[*a].is_none()
                        && !w.is(e, true)
                    {
                        w.attr[*a] = Some(Some((b, k)));
                        changed = true;
                    }
                }
            }
            for (r, e) in &sets {
                if w.reg[*r] && !w.is(e, true) {
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
        const ROUND: &str = "write the rounding it means: `ceil(…)`, or `floor(…) + 1` (#661)";
        // follow the attributes back to the first assignment fractional by
        // itself
        let mut seen = vec![];
        let mut cur = e;
        let mut via: Option<usize> = None;
        let mut at = u.at;
        loop {
            // an attribute already followed is read as whole: the next
            // part is why the assignment is fractional
            let none: &[usize] = &[];
            let Some(part) = w.first_part(cur, Some(&seen)) else {
                // (a cycle of attributes: the count itself is named)
                let part = w.first_part(e, Some(none)).expect("a fractional count");
                return Invalid {
                    message: format!(
                        "{} is a number of tokens, and `{}` may not be a whole number ({}); {ROUND}",
                        u.what,
                        self.show_expr_with(e, names),
                        reason(self, part)
                    ),
                    at: u.at,
                };
            };
            match part {
                CExpr::Attr(a) => {
                    seen.push(*a);
                    match w.attr[*a] {
                        Some(Some((b, k))) => {
                            let CStmt::Set(_, x) = &self.blocks[b][k] else {
                                unreachable!("an assignment")
                            };
                            via = Some(*a);
                            at = Some((b, k));
                            cur = x;
                        }
                        _ => {
                            let m = format!(
                                "{} is a number of tokens, and `{}` may not be a whole number: \
                                 `{}` is not ({}); {ROUND}",
                                u.what,
                                self.show_expr_with(e, names),
                                self.attrs[*a],
                                if *a == self.slot_think {
                                    "the trace's `think` is seconds"
                                } else {
                                    "a time, or an explicit session's fraction"
                                }
                            );
                            return Invalid {
                                message: m,
                                at: u.at,
                            };
                        }
                    }
                }
                part => {
                    let message = match via {
                        Some(a) => {
                            // the count, unless it is the attribute itself
                            let count = self.show_expr_with(e, names);
                            let use_ = if count == self.attrs[a] {
                                u.what.clone()
                            } else {
                                format!("{}: `{count}`", u.what)
                            };
                            format!(
                                "`{}` is a number of tokens ({use_}), and `{}` may not be a \
                                 whole number ({}); {ROUND}",
                                self.attrs[a],
                                self.show_expr(cur),
                                reason(self, part)
                            )
                        }
                        None => format!(
                            "{} is a number of tokens, and `{}` may not be a whole number ({}); \
                             write the rounding it means: `floor(…)` or `ceil(…)` (#661)",
                            u.what,
                            self.show_expr_with(e, names),
                            reason(self, part)
                        ),
                    };
                    return Invalid { message, at };
                }
            }
        }
    }

    /// Every token count is a whole number (#661): see the module.
    pub(super) fn validate_whole(&self) -> Result<(), Invalid> {
        let w = self.whole();
        let check = |e: Option<&CExpr>, u: Use, names: CtxNames| -> Result<(), Invalid> {
            match e {
                Some(e) if !w.is(e, true) => Err(self.not_whole(&w, e, &u, names)),
                _ => Ok(()),
            }
        };
        let plain: CtxNames = CtxVar::name;
        let budget: CtxNames =
            |v| crate::frontend::parser::engine_name(v.name(), "tokens cap").unwrap_or(v.name());
        let request: CtxNames =
            |v| crate::frontend::parser::engine_name(v.name(), "taking").unwrap_or(v.name());
        let pool = |q: &CRef| &self.pools[q.base].name;
        for (i, q) in self.pools.iter().enumerate() {
            if w.kv[i] && (!whole_number(q.cap) || q.block.is_some_and(|b| !whole_number(b))) {
                return Err(format!(
                    "pool `{}` holds tokens, and its cap ({}) and block ({}) are numbers of \
                     tokens: whole ones (#661)",
                    q.name,
                    q.cap,
                    q.block.map_or("none".into(), |b| b.to_string())
                )
                .into());
            }
        }
        for st in &self.stages {
            let CStageKind::Step(sp) = &st.kind else {
                continue;
            };
            let n = &st.name;
            let u = |what: &str| Use {
                what: format!("`{n}`'s {what}"),
                at: None,
            };
            check(Some(&sp.budget), u("`tokens cap`"), budget)?;
            check(Some(&sp.chunk), u("`each at most`"), budget)?;
            check(sp.granule.as_ref(), u("granule"), budget)?;
            if let Some(r) = &sp.request {
                for (q, units, _) in &r.pools {
                    if w.kv[q.base] {
                        check(Some(units), u(&format!("`taking {}`", pool(q))), request)?;
                    }
                }
                check(r.reuse.as_ref(), u("`reuse`"), request)?;
                check(r.cache.as_ref(), u("`cache`"), request)?;
                for (_, len) in r.prefix.iter().flatten() {
                    check(Some(len), u("`prefix` segment"), request)?;
                }
            }
            if let Some(c) = &sp.connector {
                check(
                    c.load.as_ref().map(|l| &l.0),
                    u("connector's `load`"),
                    request,
                )?;
                check(
                    c.save.as_ref().map(|s| &s.0),
                    u("connector's `save`"),
                    request,
                )?;
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
                let u = |what: String| Use {
                    what,
                    at: Some((b, k)),
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
                            if w.kv[q.base] {
                                check(Some(units), u(format!("`hold {}`", pool(q))), plain)?;
                            }
                        }
                        check(reuse.as_ref(), u("a hold's `reuse`".into()), plain)?;
                        check(cache.as_ref(), u("a hold's `cache`".into()), plain)?;
                        for (_, len) in prefix.iter().flatten() {
                            check(Some(len), u("a `prefix` segment".into()), plain)?;
                        }
                    }
                    CStmt::Grow(q, e) if w.kv[q.base] => {
                        check(Some(e), u(format!("`grow {}`", pool(q))), plain)?
                    }
                    CStmt::Shrink(q, e) if w.kv[q.base] => {
                        check(Some(e), u(format!("`shrink {}`", pool(q))), plain)?
                    }
                    CStmt::Load(q, e) => check(Some(e), u(format!("`load {}`", pool(q))), plain)?,
                    CStmt::Request {
                        stage,
                        input,
                        output,
                    } => {
                        let n = &self.stages[stage.base].name;
                        check(Some(input), u(format!("`{n}`'s input")), plain)?;
                        check(Some(output), u(format!("`{n}`'s output")), plain)?;
                    }
                    CStmt::Run {
                        stage,
                        work,
                        growing,
                        ..
                    } if on_engine(stage) || growing.is_some() => check(
                        Some(work),
                        u(format!("a run on `{}`", self.stages[stage.base].name)),
                        plain,
                    )?,
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

    /// Two attributes: 0 whole, 1 not.
    fn w() -> Whole {
        Whole {
            kv: vec![true, false],
            attr: vec![None, Some(None)],
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
            CExpr::Call(
                Fun::CachedIn,
                vec![CArg::Pool(CRef {
                    base: 0,
                    count: 1,
                    index: None,
                })],
            ),
        ] {
            assert!(w.is(&e, true), "{e:?}");
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
            CExpr::Call(
                Fun::Allocated,
                vec![CArg::Pool(CRef {
                    base: 1,
                    count: 1,
                    index: None,
                })],
            ),
        ] {
            assert!(!w.is(&e, true), "{e:?}");
        }
        // read as whole, an attribute does not make an expression fractional
        assert!(w.is(&attr(1), false));
    }
}
