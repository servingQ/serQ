//! Queues: a pool-owning, stage-owning declaration whose *entries* hold the
//! admission, allocation and service of one request, called from the
//! deployment as `P[i].prefill (prompt);`.
//!
//! A queue is parse-time sugar (`docs/design/queue.md`). The parser records
//! each queue's pools (renamed `Q.p`), its stage (named `Q`) and its entries
//! with their bodies as written; `expand` then puts an entry's body in place
//! of every call, with the parameters substituted like a `where` binding,
//! the own pools and stage indexed by the call's member, the entry's `set`s
//! renamed `Q.x`, `mark x` written as `set Q.x = now`, and a `from S[k]`
//! bound to the pool `S`'s entry leases. The linker sees the kernel program
//! it saw before queues existed.

use crate::ast::{Arg, Expr, Ref, Stmt};

/// An entry a role asks for: `(verb, parameters, takes from)`.
pub type EntrySig = (&'static str, usize, bool);

/// The four roles of the serving vocabulary and the entries each asks for.
pub const ROLES: &[(&str, &[EntrySig])] = &[
    ("gateway", &[("route", 0, false)]),
    ("prefill", &[("prefill", 1, false)]),
    ("decode", &[("decode", 1, false), ("decode", 1, true)]),
    ("link", &[("transfer", 1, false)]),
];

/// The verb of a call and, when it is a link's `transfer … from S to P (m)`,
/// the load and release the linker writes around it.
pub fn role_of_verb(verb: &str, from: bool) -> Option<&'static str> {
    ROLES
        .iter()
        .find(|(_, entries)| entries.iter().any(|(v, _, f)| *v == verb && *f == from))
        .map(|(role, _)| *role)
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub verb: String,
    pub params: Vec<String>,
    /// `from NAME`: the name the body gives the source pool.
    pub from: Option<String>,
    pub body: Vec<Stmt>,
    /// Names the body `set`s or `choose`s: the entry's own.
    pub locals: Vec<String>,
    /// Session attributes the body reads that are neither parameters, locals
    /// nor context: legal only as the workload's `hidden` attributes, which
    /// `assemble` checks once the workload is known.
    pub reads: Vec<String>,
    /// Token position of the header, for errors.
    pub at: usize,
}

#[derive(Clone, Debug)]
pub struct QueueDecl {
    pub name: String,
    pub count: usize,
    pub roles: Vec<String>,
    /// Pools declared inside, by their bare names (`kv`, not `Q.kv`).
    pub pools: Vec<String>,
    pub has_stage: bool,
    pub entries: Vec<Entry>,
    /// The bare name of the pool an entry leases, when one does.
    pub leased: Option<String>,
    /// Names `mark`ed in some entry.
    pub marks: Vec<String>,
    pub at: usize,
}

impl QueueDecl {
    pub fn entry(&self, verb: &str, from: bool) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.verb == verb && e.from.is_some() == from)
    }
}

/// The error type of an expansion: a message and the token position it is
/// reported at.
pub struct ExpandError {
    pub at: usize,
    pub msg: String,
}

fn err<T>(at: usize, msg: impl Into<String>) -> Result<T, ExpandError> {
    Err(ExpandError {
        at,
        msg: msg.into(),
    })
}

/// What a call binds in the entry's body.
struct Ctx<'a> {
    q: &'a QueueDecl,
    /// The call's member index (`P[i]`), or none for a queue of one.
    index: Option<Expr>,
    params: Vec<(String, Expr)>,
    /// The `from` name and the source pool it stands for.
    from: Option<(String, Ref)>,
    at: usize,
}

impl Ctx<'_> {
    fn own_ref(&self, r: &Ref) -> Ref {
        Ref {
            span: r.span,
            name: format!("{}.{}", self.q.name, r.name),
            index: self.index.clone().map(Box::new),
        }
    }

    fn reference(&self, r: &Ref) -> Result<Ref, ExpandError> {
        if let Some((from, src)) = &self.from
            && r.name == *from
        {
            if r.index.is_some() {
                return err(
                    self.at,
                    format!("`{}` is the source pool; it takes no index", from),
                );
            }
            return Ok(src.clone());
        }
        if self.q.pools.contains(&r.name) {
            if r.index.is_some() {
                return err(
                    self.at,
                    format!(
                        "`{}[…]` inside queue `{}`: an own pool is the member's; write `{}`",
                        r.name, self.q.name, r.name
                    ),
                );
            }
            return Ok(self.own_ref(r));
        }
        if r.name == self.q.name {
            if r.index.is_some() {
                return err(
                    self.at,
                    format!(
                        "`{}[…]` inside queue `{}`: the own stage is the member's; write `{}`",
                        r.name, self.q.name, r.name
                    ),
                );
            }
            return Ok(Ref {
                span: r.span,
                name: r.name.clone(),
                index: self.index.clone().map(Box::new),
            });
        }
        Ok(Ref {
            span: r.span,
            name: r.name.clone(),
            index: match &r.index {
                None => None,
                Some(i) => Some(Box::new(self.expr(i)?)),
            },
        })
    }

    fn var(&self, n: &str) -> Result<Expr, ExpandError> {
        if n == "self" {
            return match &self.index {
                Some(i) => Ok(i.clone()),
                None => err(
                    self.at,
                    format!(
                        "`self` inside queue `{}`, which is not a family",
                        self.q.name
                    ),
                ),
            };
        }
        if let Some((_, e)) = self.params.iter().find(|(p, _)| p == n) {
            return Ok(e.clone());
        }
        if self.q.locals().any(|l| l == n) {
            return Ok(Expr::Var(format!("{}.{n}", self.q.name)));
        }
        Ok(Expr::Var(n.to_string()))
    }

    fn expr(&self, e: &Expr) -> Result<Expr, ExpandError> {
        Ok(match e {
            Expr::Located(span, inner) => Expr::Located(*span, Box::new(self.expr(inner)?)),
            Expr::Num(x) => Expr::Num(*x),
            Expr::Var(n) => self.var(n)?,
            Expr::Sample(d, args) => Expr::Sample(
                d.clone(),
                args.iter()
                    .map(|a| self.expr(a))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Call(f, args) => Expr::Call(
                f.clone(),
                args.iter()
                    .map(|a| match a {
                        Arg::Expr(x) => Ok(Arg::Expr(self.expr(x)?)),
                        // a bare identifier argument names a parameter, a
                        // local, `self`, or a pool or stage
                        Arg::Ref(r) if r.index.is_none() => {
                            let is_name = r.name == "self"
                                || self.params.iter().any(|(p, _)| *p == r.name)
                                || self.q.locals().any(|l| l == r.name);
                            if is_name {
                                Ok(Arg::Expr(self.var(&r.name)?))
                            } else {
                                Ok(Arg::Ref(self.reference(r)?))
                            }
                        }
                        Arg::Ref(r) => Ok(Arg::Ref(self.reference(r)?)),
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Unary(op, a) => Expr::Unary(*op, Box::new(self.expr(a)?)),
            Expr::Binary(op, a, b) => {
                Expr::Binary(*op, Box::new(self.expr(a)?), Box::new(self.expr(b)?))
            }
            Expr::Cond(c, a, b) => Expr::Cond(
                Box::new(self.expr(c)?),
                Box::new(self.expr(a)?),
                Box::new(self.expr(b)?),
            ),
        })
    }

    /// An entry's `set` name as the session knows it: the queue's, unless
    /// the queue keeps the session's names (a gateway).
    fn local(&self, n: &str) -> String {
        if self.q.locals().any(|l| l == n) {
            format!("{}.{n}", self.q.name)
        } else {
            n.to_string()
        }
    }

    fn stmts(&self, stmts: &[Stmt]) -> Result<Vec<Stmt>, ExpandError> {
        stmts.iter().map(|s| self.stmt(s)).collect()
    }

    fn stmt(&self, s: &Stmt) -> Result<Stmt, ExpandError> {
        Ok(match s {
            Stmt::Turn | Stmt::End | Stmt::Request => s.clone(),
            Stmt::Set(n, e) => Stmt::Set(self.local(n), self.expr(e)?),
            Stmt::Observe(n, e) => Stmt::Observe(n.clone(), self.expr(e)?),
            Stmt::Mark(n) => Stmt::Set(format!("{}.{n}", self.q.name), Expr::Var("now".into())),
            Stmt::Hold {
                pools,
                reuse,
                body,
                cache,
                lease,
            } => Stmt::Hold {
                pools: pools
                    .iter()
                    .map(|(r, e, f)| {
                        Ok((
                            self.reference(r)?,
                            self.expr(e)?,
                            f.as_ref().map(|f| self.expr(f)).transpose()?,
                        ))
                    })
                    .collect::<Result<_, _>>()?,
                reuse: reuse.as_ref().map(|e| self.expr(e)).transpose()?,
                body: self.stmts(body)?,
                cache: cache.as_ref().map(|e| self.expr(e)).transpose()?,
                lease: match lease {
                    None => None,
                    Some((r, t)) => Some((self.reference(r)?, self.expr(t)?)),
                },
            },
            Stmt::Grow(r, e) => Stmt::Grow(self.reference(r)?, self.expr(e)?),
            Stmt::Drop(r) => Stmt::Drop(self.reference(r)?),
            Stmt::Release(r) => Stmt::Release(self.reference(r)?),
            Stmt::Load(r, e) => Stmt::Load(self.reference(r)?, self.expr(e)?),
            Stmt::Run {
                stage,
                mode,
                work,
                growing,
            } => Stmt::Run {
                stage: self.reference(stage)?,
                mode: *mode,
                work: self.expr(work)?,
                growing: growing.as_ref().map(|g| self.reference(g)).transpose()?,
            },
            Stmt::Branch(p, a, b) => Stmt::Branch(self.expr(p)?, self.stmts(a)?, self.stmts(b)?),
            Stmt::Loop(b) => Stmt::Loop(self.stmts(b)?),
            Stmt::Choose { var, count, key } => Stmt::Choose {
                var: self.local(var),
                count: self.expr(count)?,
                key: self.expr(key)?,
            },
            Stmt::Call {
                queue,
                verb,
                args,
                from,
                to,
            } => Stmt::Call {
                queue: self.reference(queue)?,
                verb: verb.clone(),
                args: args
                    .iter()
                    .map(|a| self.expr(a))
                    .collect::<Result<_, _>>()?,
                from: from.as_ref().map(|r| self.reference(r)).transpose()?,
                to: match to {
                    None => None,
                    Some((r, m)) => Some((self.reference(r)?, self.expr(m)?)),
                },
            },
        })
    }
}

impl QueueDecl {
    fn locals(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .flat_map(|e| e.locals.iter().map(String::as_str))
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

/// A gateway's `route` body as the program's server: its own pools and
/// marks named, nothing else renamed (the gateway reads and sets the
/// session's attributes).
pub fn gateway_body(q: &QueueDecl) -> Result<Vec<Stmt>, ExpandError> {
    let entry = q.entry("route", false).expect("a gateway has `route`");
    let ctx = Ctx {
        q,
        index: None,
        params: vec![],
        from: None,
        at: q.at,
    };
    ctx.stmts(&entry.body)
}

/// Replace every `Call` in `stmts`, at any depth, by the entry's body. An
/// entry may call another queue's entry (the decoder's `nic[self].transfer`),
/// so the replacement is expanded too; a cycle is cut at a depth no program
/// reaches.
pub fn expand(stmts: &mut Vec<Stmt>, queues: &[QueueDecl]) -> Result<(), ExpandError> {
    expand_at(stmts, queues, 0)
}

fn expand_at(stmts: &mut Vec<Stmt>, queues: &[QueueDecl], depth: usize) -> Result<(), ExpandError> {
    let mut out = Vec::with_capacity(stmts.len());
    for mut s in std::mem::take(stmts) {
        match &mut s {
            Stmt::Call {
                queue,
                verb,
                args,
                from,
                to,
            } => {
                let at = 0;
                let mut body = call(queue, verb, args, from.as_ref(), to.as_ref(), queues, at)?;
                if depth > 16 {
                    return err(
                        at,
                        format!(
                            "`{}.{verb}`: the entries call each other in a cycle",
                            queue.name
                        ),
                    );
                }
                expand_at(&mut body, queues, depth + 1)?;
                out.extend(body);
                continue;
            }
            Stmt::Hold { body, .. } | Stmt::Loop(body) => expand_at(body, queues, depth)?,
            Stmt::Branch(_, a, b) => {
                expand_at(a, queues, depth)?;
                expand_at(b, queues, depth)?;
            }
            _ => {}
        }
        out.push(s);
    }
    *stmts = out;
    Ok(())
}

/// The statements one call stands for.
fn call(
    queue: &Ref,
    verb: &str,
    args: &[Expr],
    from: Option<&Ref>,
    to: Option<&(Ref, Expr)>,
    queues: &[QueueDecl],
    at: usize,
) -> Result<Vec<Stmt>, ExpandError> {
    let q = queues
        .iter()
        .find(|q| q.name == queue.name)
        .ok_or_else(|| ExpandError {
            at,
            msg: format!(
                "`{}.{verb}`: no queue `{}` is declared",
                queue.name, queue.name
            ),
        })?;
    let at = q.at.max(at);
    let index = match (&queue.index, q.count) {
        (None, 1) => None,
        (Some(i), n) if n > 1 => Some((**i).clone()),
        (None, n) => {
            return err(
                at,
                format!(
                    "`{}.{verb}`: `{}` is a family of {n}; index it",
                    q.name, q.name
                ),
            );
        }
        (Some(_), _) => {
            return err(
                at,
                format!(
                    "`{}[…].{verb}`: `{}` is one queue, not a family",
                    q.name, q.name
                ),
            );
        }
    };
    // a link's `from`/`to` are the call's, not the entry's
    let is_transfer = role_of_verb(verb, false) == Some("link");
    let entry_from = from.is_some() && !is_transfer;
    let entry = match q.entry(verb, entry_from) {
        Some(e) => e,
        None => {
            let hint = match (q.entry(verb, !entry_from), from) {
                (Some(_), Some(_)) => format!(": its `{verb}` takes no `from`"),
                (Some(_), None) => format!(": its `{verb}` needs `from`"),
                _ => String::new(),
            };
            return err(
                at,
                format!(
                    "`{}.{verb}`: queue `{}` has no such entry{hint}",
                    q.name, q.name
                ),
            );
        }
    };
    if args.len() != entry.params.len() {
        return err(
            at,
            format!(
                "`{}.{verb}` takes {} argument(s), got {}",
                q.name,
                entry.params.len(),
                args.len()
            ),
        );
    }
    for (p, a) in entry.params.iter().zip(args) {
        if has_sample(a) {
            return err(
                at,
                format!(
                    "`{}.{verb}`: the argument for `{p}` draws a sample; a parameter is \
                     substituted, so a name used twice would draw twice",
                    q.name
                ),
            );
        }
    }
    // `from S[k]`: the pool `S`'s entry leases, for this request; or a pool
    // named outright (`from kvP[i]`, or an entry's `from` name already bound
    // to one), which the linker checks as it checks any `release`
    let src = match from {
        None => None,
        Some(r) if !queues.iter().any(|s| s.name == r.name) => Some(r.clone()),
        Some(r) => {
            let s = queues.iter().find(|s| s.name == r.name).expect("a queue");
            let leased = s.leased.as_ref().ok_or_else(|| ExpandError {
                at,
                msg: format!(
                    "`from {}`: no entry of `{}` leases a pool, so it has nothing to take",
                    r.name, r.name
                ),
            })?;
            match (&r.index, s.count) {
                (None, 1) | (Some(_), 2..) => {}
                (None, n) => {
                    return err(
                        at,
                        format!(
                            "`from {}`: `{}` is a family of {n}; index it",
                            r.name, r.name
                        ),
                    );
                }
                (Some(_), _) => {
                    return err(
                        at,
                        format!(
                            "`from {}[…]`: `{}` is one queue, not a family",
                            r.name, r.name
                        ),
                    );
                }
            }
            Some(Ref {
                span: r.span,
                name: format!("{}.{leased}", s.name),
                index: r.index.clone(),
            })
        }
    };
    if to.is_some() && !is_transfer {
        return err(
            at,
            format!("`to` belongs to a link's `transfer`, not to `{verb}`"),
        );
    }
    if is_transfer && (to.is_none() || from.is_none()) {
        return err(
            at,
            format!(
                "`{}.transfer (n) from S to P (m)`: a transfer names the pool it takes from \
                 and the pool it fills",
                q.name
            ),
        );
    }
    let ctx = Ctx {
        q,
        index,
        params: entry
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect(),
        from: entry.from.clone().zip(src.clone()),
        at,
    };
    let mut out = ctx.stmts(&entry.body)?;
    if let Some((dst, m)) = to {
        out.push(Stmt::Load(dst.clone(), m.clone()));
        out.push(Stmt::Release(src.expect("a transfer has a source")));
    }
    Ok(out)
}
