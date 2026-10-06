//! Request sizes and resource costs. These checks apply equally to source
//! programs and directly constructed IR; conversions do not change the clock,
//! allocation lifetime, evaluation moment, or random stream.

use super::*;

impl CostTarget {
    pub fn joint(mut targets: Vec<Self>) -> Self {
        targets.sort();
        targets.dedup();
        if targets.len() == 1 {
            targets.remove(0)
        } else {
            Self::Joint(targets)
        }
    }
    fn any(&self, predicate: &impl Fn(&Self) -> bool) -> bool {
        match self {
            Self::Joint(ts) => ts.iter().any(|t| t.any(predicate)),
            _ => predicate(self),
        }
    }
    pub fn pool(r: &CRef) -> Self {
        Self::Pool {
            base: r.base,
            count: r.count,
        }
    }
    pub fn stage(r: &CRef) -> Self {
        Self::Stage {
            base: r.base,
            count: r.count,
        }
    }
}

impl Program {
    pub(super) fn validate_cost_target(&self, target: &CostTarget) -> Result<(), String> {
        let (base, count, len) = match target {
            CostTarget::Pool { base, count } => (*base, *count, self.pools.len()),
            CostTarget::Stage { base, count } => (*base, *count, self.stages.len()),
            CostTarget::Joint(ts) => {
                if ts.len() < 2
                    || *target != CostTarget::joint(ts.clone())
                    || ts.iter().any(|t| matches!(t, CostTarget::Joint(_)))
                {
                    return Err(
                        "joint cost resources must be distinct, sorted, and non-nested".into(),
                    );
                }
                return ts.iter().try_for_each(|t| self.validate_cost_target(t));
            }
        };
        if count == 0 || base.checked_add(count).is_none_or(|end| end > len) {
            return Err("cost resource is out of range".into());
        }
        Ok(())
    }
    pub(super) fn show_cost_target(&self, target: &CostTarget) -> String {
        match target {
            CostTarget::Pool { base, .. } => self
                .pools
                .get(*base)
                .map_or("?", |p| p.name.as_str())
                .into(),
            CostTarget::Stage { base, .. } => self
                .stages
                .get(*base)
                .map_or("?", |p| p.name.as_str())
                .into(),
            CostTarget::Joint(ts) => ts
                .iter()
                .map(|t| self.show_cost_target(t))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// Derive the source program's slot types. JSON supplies these explicitly;
    /// validation checks the declaration against every assignment and use.
    pub(crate) fn infer_types(&mut self) -> Result<(), String> {
        self.attr_types = vec![ValueType::Value; self.attrs.len()];
        for slot in [
            self.slot_serial,
            self.slot_turn,
            self.slot_new,
            self.slot_out,
            self.slot_think,
            self.slot_more,
            self.slot_forced,
        ] {
            self.attr_types[slot] = ValueType::Size;
        }
        for (b, block) in self.blocks.iter().enumerate() {
            for (k, st) in block.iter().enumerate() {
                if self.sides[b][k] == Side::Workload {
                    if let CStmt::Set(a, _) | CStmt::Choose { var: a, .. } = st {
                        self.attr_types[*a] = ValueType::Size;
                    }
                }
            }
        }
        // Costs can pass through aliases and definitions; their type does not
        // depend on the order in which the block arena was constructed.
        for _ in 0..=self.attrs.len() {
            let mut changed = false;
            for block in &self.blocks {
                for st in block {
                    if let CStmt::Set(a, e) = st {
                        if let Ok(Some(target)) = self.cost_type(e) {
                            let t = ValueType::Cost(target);
                            if self.attr_types[*a] != t {
                                if matches!(self.attr_types[*a], ValueType::Cost(_)) {
                                    return Err(format!(
                                        "`{}` is assigned costs of different resources",
                                        self.attrs[*a]
                                    ));
                                }
                                self.attr_types[*a] = t;
                                changed = true;
                            }
                        }
                    }
                }
            }
            if !changed {
                return Ok(());
            }
        }
        Err("cost type inference did not converge".into())
    }

    /// None is an ordinary quantity (a request size or bookkeeping value).
    /// Arithmetic must not erase a resource cost by copying or rescaling it.
    fn cost_type(&self, e: &CExpr) -> Result<Option<CostTarget>, String> {
        // Aggregates lower to expressions with thousands of terms. A recursive
        // type checker must not exhaust the stack on this supported input.
        let mut pending = vec![(e, false)];
        let mut types = std::collections::HashMap::new();
        while let Some((e, visited)) = pending.pop() {
            if visited {
                types.insert(e as *const CExpr, self.cost_type_node(e, &types)?);
                continue;
            }
            pending.push((e, true));
            let mut child = |e| pending.push((e, false));
            match e {
                CExpr::Cost(_, x) | CExpr::Unary(_, x) => child(x.as_ref()),
                CExpr::Binary(_, a, b) => {
                    child(a.as_ref());
                    child(b.as_ref());
                }
                CExpr::Cond(c, a, b) => {
                    child(c.as_ref());
                    child(a.as_ref());
                    child(b.as_ref());
                }
                CExpr::Sample(_, xs) => {
                    for x in xs {
                        child(x);
                    }
                }
                CExpr::Call(_, args) => {
                    for arg in args {
                        match arg {
                            CArg::Expr(x) => child(x),
                            CArg::Pool(r) | CArg::Stage(r) => {
                                if let Some(i) = &r.index {
                                    child(i.as_ref());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(types.remove(&(e as *const CExpr)).flatten())
    }

    fn cost_type_node(
        &self,
        e: &CExpr,
        types: &std::collections::HashMap<*const CExpr, Option<CostTarget>>,
    ) -> Result<Option<CostTarget>, String> {
        let child = |e: &CExpr| -> Result<Option<CostTarget>, String> {
            Ok(types[&(e as *const CExpr)].clone())
        };
        let scalar = |e: &CExpr| -> Result<(), String> {
            if child(e)?.is_some() {
                Err("a resource cost is not an ordinary quantity".into())
            } else {
                Ok(())
            }
        };
        Ok(match e {
            CExpr::Cost(target, e) => {
                scalar(e)?;
                Some(target.clone())
            }
            CExpr::Attr(a) => match self.attr_types.get(*a) {
                Some(ValueType::Cost(target)) => Some(target.clone()),
                Some(_) => None,
                None => return Err(format!("attribute {a} has no type")),
            },
            CExpr::Unary(UnOp::Neg, e) => child(e)?,
            CExpr::Unary(UnOp::Not, e) => {
                scalar(e)?;
                None
            }
            CExpr::Binary(op, a, b) => {
                let (a, b) = (child(a)?, child(b)?);
                match op {
                    BinOp::Add | BinOp::Sub if a == b => a,
                    BinOp::Mul if a.is_none() || b.is_none() => a.or(b),
                    BinOp::Div if b.is_none() => a,
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne if a == b => None,
                    _ if a.is_none() && b.is_none() => None,
                    _ => return Err("incompatible cost operands; convert the quantity with cost(resource, expression)".into()),
                }
            }
            CExpr::Cond(c, a, b) => {
                scalar(c)?;
                let (a, b) = (child(a)?, child(b)?);
                if a != b {
                    return Err("both branches must have the same cost type".into());
                }
                a
            }
            CExpr::Call(f, args) => {
                let mut types = vec![];
                for arg in args {
                    match arg {
                        CArg::Expr(e) => types.push(child(e)?),
                        CArg::Pool(r) | CArg::Stage(r) => {
                            if let Some(i) = &r.index {
                                scalar(i)?;
                            }
                        }
                    }
                }
                if matches!(f, Fun::Min | Fun::Max | Fun::Abs | Fun::Floor | Fun::Ceil)
                    && types.windows(2).all(|t| t[0] == t[1])
                {
                    types.into_iter().next().flatten()
                } else {
                    if types.iter().any(Option::is_some) {
                        return Err(format!(
                            "{} expects ordinary quantities, not resource costs",
                            f.name()
                        ));
                    }
                    None
                }
            }
            CExpr::Sample(_, xs) => {
                for x in xs {
                    scalar(x)?;
                }
                None
            }
            _ => None,
        })
    }

    pub(super) fn validate_types(&self) -> Result<(), Invalid> {
        if self.attr_types.len() != self.attrs.len() {
            return Err(
                "every attribute must have a size, value, or resource cost type"
                    .to_string()
                    .into(),
            );
        }
        if self.sides.len() != self.blocks.len()
            || self
                .sides
                .iter()
                .zip(&self.blocks)
                .any(|(s, b)| s.len() != b.len())
        {
            return Err("every IR statement must declare its workload/server side"
                .to_string()
                .into());
        }
        for slot in [
            self.slot_serial,
            self.slot_turn,
            self.slot_new,
            self.slot_out,
            self.slot_think,
            self.slot_more,
            self.slot_forced,
        ] {
            if self.attr_types[slot] != ValueType::Size {
                return Err(format!("built-in `{}` must have size type", self.attrs[slot]).into());
            }
        }
        for slot in [self.slot_cached, self.slot_computed] {
            if self.attr_types[slot] != ValueType::Value {
                return Err(format!("built-in `{}` must have value type", self.attrs[slot]).into());
            }
        }
        self.declaration_types()?;
        // A server's control-flow body cannot regain workload write authority.
        // Workload bodies may contain inlined requests, so the reverse is legal.
        fn server_body(p: &Program, b: usize) -> Result<(), Invalid> {
            for (k, st) in p.blocks[b].iter().enumerate() {
                if p.sides[b][k] != Side::Server {
                    return Err(Invalid::at(
                        "a server body cannot contain workload statements".into(),
                        b,
                        k,
                    ));
                }
                for child in children(st) {
                    server_body(p, child)?;
                }
            }
            Ok(())
        }
        for (b, block) in self.blocks.iter().enumerate() {
            for (k, st) in block.iter().enumerate() {
                if self.sides[b][k] == Side::Server {
                    for child in children(st) {
                        server_body(self, child)?;
                    }
                }
            }
        }
        for t in &self.attr_types {
            if let ValueType::Cost(t) = t {
                self.validate_cost_target(t)?;
            }
        }
        // A client may model its own tool or conversation-wide reservation.
        // It may not construct costs for resources allocated by the server.
        let mut server_pools = vec![false; self.pools.len()];
        let mut server_stages = vec![false; self.stages.len()];
        for (b, block) in self.blocks.iter().enumerate() {
            for (k, st) in block.iter().enumerate() {
                if self.sides[b][k] != Side::Server {
                    continue;
                }
                match st {
                    CStmt::Hold { pools, .. } => {
                        for (r, _, _) in pools {
                            server_pools[r.base..r.base + r.count].fill(true);
                        }
                    }
                    CStmt::Grow(r, _) | CStmt::Load(r, _) => {
                        server_pools[r.base..r.base + r.count].fill(true);
                    }
                    CStmt::Run {
                        stage,
                        also,
                        growing,
                        ..
                    } => {
                        for r in std::iter::once(stage).chain(also) {
                            server_stages[r.base..r.base + r.count].fill(true);
                        }
                        if let Some(r) = growing {
                            server_pools[r.base..r.base + r.count].fill(true);
                        }
                    }
                    _ => {}
                }
            }
        }
        for (b, block) in self.blocks.iter().enumerate() {
            for (k, st) in block.iter().enumerate() {
                let side = self.sides[b][k];
                let at = |e: String| Invalid::at(e, b, k);
                let expression = |e: &CExpr| -> Result<Option<CostTarget>, Invalid> {
                    let t = self.cost_type(e).map_err(at)?;
                    if (b == self.init || b == self.turn) && self.contains_cost(e) {
                        return Err(at("workload init and turn produce sizes, not costs".into()));
                    }
                    if side == Side::Workload {
                        let forbidden = e.find(&|e| {
                            let target = match e {
                                CExpr::Cost(t, _) => Some(t),
                                CExpr::Attr(a) => match &self.attr_types[*a] {
                                    ValueType::Cost(t) => Some(t),
                                    _ => None,
                                },
                                _ => None,
                            };
                            target.is_some_and(|t| {
                                t.any(&|t| match t {
                                    CostTarget::Pool { base, count } => {
                                        server_pools[*base..base + count].iter().any(|s| *s)
                                    }
                                    CostTarget::Stage { base, count } => {
                                        server_stages[*base..base + count].iter().any(|s| *s)
                                    }
                                    CostTarget::Joint(_) => unreachable!(),
                                })
                            })
                        });
                        if forbidden.is_some() {
                            return Err(at("the workload cannot create or read a server resource's cost; send a size and convert it in the server".into()));
                        }
                    }
                    Ok(t)
                };
                let amount = |e: &CExpr, target: CostTarget| -> Result<(), Invalid> {
                    if expression(e)? != Some(target) {
                        return Err(at("resource use requires its cost type; convert the quantity with cost(resource, expression)".into()));
                    }
                    Ok(())
                };
                for r in references(st) {
                    if let Some(i) = &r.index {
                        if expression(i)?.is_some() {
                            return Err(at("a resource index is not a resource cost".into()));
                        }
                    }
                }
                if (b == self.init || b == self.turn) && side != Side::Workload {
                    return Err(at("init and turn belong to the workload".into()));
                }
                match st {
                    CStmt::Set(a, e) => {
                        let t = expression(e)?;
                        if side == Side::Workload && self.attr_types[*a] == ValueType::Value {
                            return Err(at(format!(
                                "workload assignment `{}` must declare a size type",
                                self.attrs[*a]
                            )));
                        }
                        let expected = match &self.attr_types[*a] {
                            ValueType::Size if side == Side::Server => {
                                return Err(at(format!(
                                    "the server cannot assign request size `{}`; sizes are workload-owned and read-only in the server",
                                    self.attrs[*a]
                                )));
                            }
                            ValueType::Cost(t) => Some(t.clone()),
                            _ => None,
                        };
                        if t != expected {
                            return Err(at(format!(
                                "assignment changes the type of `{}`",
                                self.attrs[*a]
                            )));
                        }
                        if side == Side::Workload
                            && (b == self.init || b == self.turn)
                            && t.is_some()
                        {
                            return Err(at(
                                "workload init and turn produce sizes, not costs".into()
                            ));
                        }
                    }
                    CStmt::Choose { var, count, key } => {
                        if side == Side::Server && self.attr_types[*var] == ValueType::Size {
                            return Err(at(format!(
                                "the server cannot assign request size `{}`",
                                self.attrs[*var]
                            )));
                        }
                        if side == Side::Workload && self.attr_types[*var] != ValueType::Size {
                            return Err(at("workload choose must declare a size type".into()));
                        }
                        if matches!(self.attr_types[*var], ValueType::Cost(_)) {
                            return Err(at("choose produces an index, not a resource cost".into()));
                        }
                        if expression(count)?.is_some()
                            || key
                                .iter()
                                .any(|e| self.cost_type(e).is_ok_and(|t| t.is_some()))
                        {
                            return Err(at(
                                "choose requires ordinary quantities, not resource costs".into(),
                            ));
                        }
                        for e in key {
                            expression(e)?;
                        }
                    }
                    CStmt::Run {
                        stage, also, work, ..
                    } => amount(
                        work,
                        CostTarget::joint(
                            std::iter::once(stage)
                                .chain(also)
                                .map(CostTarget::stage)
                                .collect(),
                        ),
                    )?,
                    CStmt::Hold {
                        pools,
                        reuse,
                        cache,
                        lease,
                        ..
                    } => {
                        for (r, units, reserve) in pools {
                            amount(units, CostTarget::pool(r))?;
                            if let Some(e) = reserve {
                                amount(e, CostTarget::pool(r))?;
                            }
                        }
                        for e in reuse.iter().chain(cache) {
                            amount(
                                e,
                                CostTarget::joint(
                                    pools.iter().map(|(r, _, _)| CostTarget::pool(r)).collect(),
                                ),
                            )?;
                        }
                        if let Some((_, e)) = lease {
                            if expression(e)?.is_some() {
                                return Err(at(
                                    "a lease duration is seconds, not a resource cost".into()
                                ));
                            }
                        }
                    }
                    CStmt::Grow(r, e) | CStmt::Load(r, e) => amount(e, CostTarget::pool(r))?,
                    CStmt::Observe(_, e) => {
                        expression(e)?;
                    }
                    CStmt::Branch(e, ..) | CStmt::While(e, _) => {
                        if expression(e)?.is_some() {
                            return Err(at(
                                "a control-flow predicate is not a resource cost".into()
                            ));
                        }
                    }
                    CStmt::Turn | CStmt::End if side == Side::Server => {
                        return Err(at(
                            "turn and end belong to the workload, not the server".into()
                        ));
                    }
                    _ => {}
                }
            }
        }
        if let CArrival::Sessions(sessions) = &self.arrival {
            for s in sessions {
                for (a, _) in s.attrs.iter().chain(s.turns.iter().flatten()) {
                    if self.attr_types[*a] != ValueType::Size {
                        return Err(format!(
                            "explicit sessions may supply request sizes, not server value `{}`",
                            self.attrs[*a]
                        )
                        .into());
                    }
                }
            }
        }
        self.costs_are_initialized()?;
        Ok(())
    }

    fn contains_cost(&self, e: &CExpr) -> bool {
        e.any(&|e| {
            matches!(e, CExpr::Cost(..))
                || matches!(e, CExpr::Attr(a)
            if matches!(self.attr_types[*a], ValueType::Cost(_)))
        })
    }

    /// Resource declarations define scheduling and conversion formulas over
    /// quantities. A session's already-converted cost is not such a formula:
    /// these reads may happen before that session reaches its assignment.
    fn declaration_types(&self) -> Result<(), String> {
        let scalar = |e: &CExpr| -> Result<(), String> {
            self.cost_type(e)?;
            if self.contains_cost(e) {
                return Err("resource declarations, gauges and claims use ordinary quantities, not session resource costs".into());
            }
            Ok(())
        };
        fn iteration(
            body: &[CIter],
            check: &impl Fn(&CExpr) -> Result<(), String>,
        ) -> Result<(), String> {
            for st in body {
                match st {
                    CIter::Serve { only, by } => {
                        for e in only.iter().chain(by.iter().flatten()) {
                            check(e)?;
                        }
                    }
                    CIter::Admit { only, gate } => {
                        for e in only.iter().chain(gate) {
                            check(e)?;
                        }
                    }
                    CIter::Branch(e, a, b) => {
                        check(e)?;
                        iteration(a, check)?;
                        iteration(b, check)?;
                    }
                    CIter::Set(_, e) => check(e)?,
                }
            }
            Ok(())
        }
        if let CArrival::Renewal(e) = &self.arrival {
            scalar(e)?;
        }
        for p in &self.pools {
            let mut es: Vec<&CExpr> = p.queue.iter().flatten().collect();
            if let CEvict::By(keys) = &p.evict {
                es.extend(keys);
            }
            if let Preempt::By { keys, .. } = &p.preempt {
                es.extend(keys);
            }
            if let Some(spill) = &p.spill {
                es.extend([&spill.work, &spill.when]);
            }
            for e in es {
                scalar(e)?;
            }
        }
        for s in &self.stages {
            match &s.kind {
                CStageKind::Ps(e) => scalar(e)?,
                CStageKind::Step(st) => {
                    for e in [&st.budget, &st.cost, &st.chunk]
                        .into_iter()
                        .chain(st.granule.iter())
                    {
                        scalar(e)?;
                    }
                    if let CServe::By(keys) = &st.serve {
                        for e in keys {
                            scalar(e)?;
                        }
                    }
                    if let Some(body) = &st.iteration {
                        iteration(body, &scalar)?;
                    }
                }
                _ => {}
            }
        }
        for g in &self.gauges {
            scalar(&g.expr)?;
        }
        for c in &self.claims {
            for e in std::iter::once(&c.expr).chain(c.given.iter()) {
                scalar(e)?;
            }
        }
        Ok(())
    }

    fn costs_are_initialized(&self) -> Result<(), Invalid> {
        fn walk(p: &Program, b: usize, ready: &mut Vec<bool>) -> Result<(), Invalid> {
            for (k, st) in p.blocks[b].iter().enumerate() {
                let read = |e: &CExpr, ready: &[bool]| -> Result<(), Invalid> {
                    if let Some(CExpr::Attr(a)) = e.find(&|x| {
                        matches!(x, CExpr::Attr(a)
                        if matches!(p.attr_types[*a], ValueType::Cost(_)) && !ready[*a])
                    }) {
                        return Err(Invalid::at(
                            format!(
                                "cost `{}` is read before every path converts and assigns it",
                                p.attrs[*a]
                            ),
                            b,
                            k,
                        ));
                    }
                    Ok(())
                };
                for r in references(st) {
                    if let Some(i) = &r.index {
                        read(i, ready)?;
                    }
                }
                match st {
                    CStmt::Set(a, e) => {
                        read(e, ready)?;
                        ready[*a] = true;
                    }
                    CStmt::Observe(_, e) | CStmt::Grow(_, e) | CStmt::Load(_, e) => read(e, ready)?,
                    CStmt::Run { work, .. } => read(work, ready)?,
                    CStmt::Hold {
                        pools,
                        reuse,
                        body,
                        cache,
                        lease,
                    } => {
                        for (_, e, reserve) in pools {
                            read(e, ready)?;
                            if let Some(e) = reserve {
                                read(e, ready)?;
                            }
                        }
                        if let Some(e) = reuse {
                            read(e, ready)?;
                        }
                        // Release, rejection, end and preemption may evaluate
                        // the cache clause before the body reaches its last set.
                        if let Some(e) = cache {
                            read(e, ready)?;
                        }
                        walk(p, *body, ready)?;
                        if let Some((_, e)) = lease {
                            read(e, ready)?;
                        }
                    }
                    CStmt::Branch(e, a, c) => {
                        read(e, ready)?;
                        let mut other = ready.clone();
                        walk(p, *a, ready)?;
                        walk(p, *c, &mut other)?;
                        for (x, y) in ready.iter_mut().zip(other) {
                            *x &= y;
                        }
                    }
                    CStmt::While(e, body) => {
                        read(e, ready)?;
                        let mut inside = ready.clone();
                        walk(p, *body, &mut inside)?;
                    }
                    CStmt::Loop(body) => {
                        let mut inside = ready.clone();
                        walk(p, *body, &mut inside)?;
                    }
                    CStmt::Fork(body) => {
                        let mut leg = ready.clone();
                        walk(p, *body, &mut leg)?;
                    }
                    CStmt::Choose { count, key, .. } => {
                        read(count, ready)?;
                        for e in key {
                            read(e, ready)?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        let mut ready = vec![false; self.attrs.len()];
        walk(self, self.init, &mut ready)?;
        // Turn programs cannot construct costs, so need no dynamic frame.
        walk(self, self.turn, &mut ready)?;
        walk(self, self.session, &mut ready)
    }
}

fn children(st: &CStmt) -> Vec<BlockId> {
    match st {
        CStmt::Hold { body, .. }
        | CStmt::Loop(body)
        | CStmt::While(_, body)
        | CStmt::Fork(body) => vec![*body],
        CStmt::Branch(_, a, b) => vec![*a, *b],
        _ => vec![],
    }
}

fn references(st: &CStmt) -> Vec<&CRef> {
    match st {
        CStmt::Run {
            stage,
            also,
            growing,
            ..
        } => std::iter::once(stage).chain(also).chain(growing).collect(),
        CStmt::Hold { pools, lease, .. } => pools
            .iter()
            .map(|(r, _, _)| r)
            .chain(lease.iter().map(|(r, _)| r))
            .collect(),
        CStmt::Grow(r, _) | CStmt::Load(r, _) | CStmt::Drop(r) | CStmt::Release(r) => vec![r],
        _ => vec![],
    }
}
