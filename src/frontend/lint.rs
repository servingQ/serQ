//! Programs that link but almost certainly do not mean what they say.
//!
//! `link` rejects a program that makes no sense: an unknown pool, a negative
//! block, a warm-up past the horizon. These are the other kind - a program
//! that is well formed and whose author meant something else. The checks
//! below come from bugs this repository actually shipped, and none has a
//! legitimate instance in `examples/`, so they are errors rather than
//! warnings: a warning nobody acts on is worse than no check.

use crate::ir::{CArg, CExpr, CStmt, Fun, Program};

type Result = std::result::Result<(), String>;

/// Does this expression read live pool or stage state - something that
/// changes while a session waits?
fn reads_live_state(p: &Program, e: &CExpr, found: &mut Option<String>) -> bool {
    match e {
        CExpr::Call(f, args) => {
            let live = matches!(
                f,
                Fun::CachedIn
                    | Fun::Used
                    | Fun::Free
                    | Fun::Holders
                    | Fun::Queued
                    | Fun::BudgetLeft
                    | Fun::Queue
                    | Fun::Busy
                    | Fun::Work
                    | Fun::Price
                    | Fun::EstLambda
                    | Fun::EstRho
                    | Fun::EstWait
            );
            if live && found.is_none() {
                *found = Some(p.show_expr(e));
            }
            live || args.iter().any(|a| match a {
                CArg::Expr(x) => reads_live_state(p, x, found),
                CArg::Pool(r) | CArg::Stage(r) => r
                    .index
                    .as_ref()
                    .is_some_and(|i| reads_live_state(p, i, found)),
            })
        }
        CExpr::Num(_) | CExpr::Attr(_) | CExpr::Ctx(_) => false,
        CExpr::Sample(_, a) => a.iter().any(|x| reads_live_state(p, x, found)),
        CExpr::Unary(_, a) => reads_live_state(p, a, found),
        CExpr::Binary(_, a, b) => reads_live_state(p, a, found) || reads_live_state(p, b, found),
        CExpr::Cond(c, a, b) => {
            reads_live_state(p, c, found)
                || reads_live_state(p, a, found)
                || reads_live_state(p, b, found)
        }
    }
}

fn mentions_attr(e: &CExpr, slot: usize) -> bool {
    match e {
        CExpr::Attr(s) => *s == slot,
        CExpr::Num(_) | CExpr::Ctx(_) => false,
        CExpr::Sample(_, a) => a.iter().any(|x| mentions_attr(x, slot)),
        CExpr::Call(_, a) => a.iter().any(|x| match x {
            CArg::Expr(x) => mentions_attr(x, slot),
            CArg::Pool(r) | CArg::Stage(r) => {
                r.index.as_ref().is_some_and(|i| mentions_attr(i, slot))
            }
        }),
        CExpr::Unary(_, a) => mentions_attr(a, slot),
        CExpr::Binary(_, a, b) => mentions_attr(a, slot) || mentions_attr(b, slot),
        CExpr::Cond(c, a, b) => {
            mentions_attr(c, slot) || mentions_attr(a, slot) || mentions_attr(b, slot)
        }
    }
}

/// The first hold after `from` in `block` whose *header* reads `slot`, giving
/// the pool it holds. The body does not count: a `set` inside the body runs
/// after admission, which is the timing the author would have got anyway.
fn header_using(p: &Program, block: usize, from: usize, slot: usize) -> Option<String> {
    let stmts = p.blocks.get(block)?;
    for s in stmts.iter().skip(from) {
        match s {
            // reassigned: whatever it read before no longer reaches the hold
            CStmt::Set(w, _) if *w == slot => return None,
            CStmt::Hold {
                pools,
                reuse,
                cache,
                ..
            } => {
                // name the pool whose own expression reads it, not the
                // first of the hold: that is the one the reader has to look at
                for (r, u, res) in pools {
                    if mentions_attr(u, slot)
                        || res.as_ref().is_some_and(|e| mentions_attr(e, slot))
                    {
                        return Some(p.show_pool_ref(r));
                    }
                }
                let rest = reuse.iter().chain(cache.iter());
                if rest.into_iter().any(|e| mentions_attr(e, slot)) {
                    return Some(p.show_pool_ref(&pools[0].0));
                }
            }
            CStmt::Branch(_, t, e) => {
                if let Some(x) = header_using(p, *t, 0, slot).or(header_using(p, *e, 0, slot)) {
                    return Some(x);
                }
            }
            CStmt::Loop(b) => {
                if let Some(x) = header_using(p, *b, 0, slot) {
                    return Some(x);
                }
            }
            _ => {}
        }
    }
    None
}

/// A `set` that reads live state, used in a hold's header.
///
/// A hold's header is evaluated when the session is admitted; a `set` above
/// it runs when the session reaches that statement, which for a session that
/// then queues is *before* it waits. `examples/multi-turn/vllm.sq` shipped with exactly
/// this: a prefix-cache lookup bound before the queue, under a comment citing
/// the admission-time lookup. `at admission (name = e)` is the clause for it.
fn stale_header_read(p: &Program, block: usize, out: &mut Vec<String>) {
    let Some(stmts) = p.blocks.get(block) else {
        return;
    };
    for (i, s) in stmts.iter().enumerate() {
        match s {
            CStmt::Set(slot, e) => {
                let mut what = None;
                if reads_live_state(p, e, &mut what)
                    && let Some(pool) = header_using(p, block, i + 1, *slot)
                {
                    let name = p.attrs.get(*slot).map_or("?", String::as_str);
                    out.push(format!(
                        "`{name}` reads {} when the session reaches the `set`, and is used \
                         in the header of a hold on `{pool}`, which is read when the session \
                         is admitted. Write `at admission ({name} = …)` if the admission \
                         value is what you meant.",
                        what.unwrap_or_else(|| "live state".into())
                    ));
                }
            }
            CStmt::Hold { body, .. } => stale_header_read(p, *body, out),
            CStmt::Branch(_, t, e) => {
                stale_header_read(p, *t, out);
                stale_header_read(p, *e, out);
            }
            CStmt::Loop(b) => stale_header_read(p, *b, out),
            _ => {}
        }
    }
}

/// `branch (0.8)` reads as a test and cannot be one: a guard is 0 or 1, and
/// a constant fraction was meant as a draw, which has its own spelling.
fn constant_probability_guard(p: &Program, block: usize, out: &mut Vec<String>) {
    let Some(stmts) = p.blocks.get(block) else {
        return;
    };
    for s in stmts {
        match s {
            CStmt::Branch(g, t, e) => {
                if let CExpr::Num(x) = g
                    && *x != 0.0
                    && *x != 1.0
                {
                    let g = crate::ir::show_num_exact(*x);
                    out.push(if *x > 0.0 && *x < 1.0 {
                        format!(
                            "`branch ({g})` is not a test: a guard is 0 or 1, and a constant \
                             strictly between them is a probability. Write `branch with ({g})`."
                        )
                    } else {
                        format!("`branch ({g})` is not a test: a guard is 0 or 1.")
                    });
                }
                constant_probability_guard(p, *t, out);
                constant_probability_guard(p, *e, out);
            }
            CStmt::Hold { body, .. } => constant_probability_guard(p, *body, out),
            CStmt::Loop(b) => constant_probability_guard(p, *b, out),
            _ => {}
        }
    }
}

/// Every expression a statement evaluates (a hold's header and clauses
/// included; its body is a block of its own).
fn exprs_of(s: &CStmt) -> Vec<&CExpr> {
    match s {
        CStmt::Set(_, e) | CStmt::Observe(_, e) | CStmt::Grow(_, e) | CStmt::Load(_, e) => {
            vec![e]
        }
        CStmt::Hold {
            pools,
            reuse,
            cache,
            lease,
            ..
        } => pools
            .iter()
            .flat_map(|(_, u, r)| std::iter::once(u).chain(r.iter()))
            .chain(reuse.iter())
            .chain(cache.iter())
            .chain(lease.iter().map(|(_, t)| t))
            .collect(),
        CStmt::Run { work, .. } => vec![work],
        CStmt::Branch(g, _, _) => vec![g],
        CStmt::Choose { count, key, .. } => std::iter::once(count).chain(key.iter()).collect(),
        CStmt::Turn | CStmt::Drop(_) | CStmt::Release(_) | CStmt::Loop(_) | CStmt::End => {
            vec![]
        }
    }
}

/// `cached` read in the body of a hold without a `cache` clause, and
/// `reuse` on one.
///
/// The clause is what makes a hold take part in the prefix cache: with it
/// the admission consumes the session's own prefix (`cached :=` what it
/// consumed) and the release keeps `min(ℓ, computed)`; without it the hold
/// is memory alone, leaves the prefix where it is, and sets `cached` to 0
/// (#230: an outer hold around the request's used to consume the prefix at
/// its admission, so the request found none). So `cached` in such a body is
/// always 0, and `reuse` bounds
/// nothing. A hold that consumed and kept nothing, which is what the body
/// used to see, is `cache (0)`.
fn cached_in_a_hold_without_cache(
    p: &Program,
    block: usize,
    hold: Option<(&str, bool)>,
    out: &mut Vec<String>,
) {
    let Some(stmts) = p.blocks.get(block) else {
        return;
    };
    for s in stmts {
        if let Some((pool, false)) = hold
            && exprs_of(s)
                .into_iter()
                .any(|e| mentions_attr(e, p.slot_cached))
        {
            out.push(format!(
                "`cached` is read in a hold on `{pool}` that has no `cache` clause: such a \
                 hold consumes nothing of the session's cached prefix, so `cached` is 0 \
                 there. Write `cache (0)` to consume the prefix and keep nothing, \
                 `cache (ℓ)` to keep ℓ of what was computed, or read `cached` above the \
                 hold, where the admission that set it is the enclosing hold's."
            ));
        }
        match s {
            CStmt::Hold {
                pools,
                reuse,
                body,
                cache,
                ..
            } => {
                let pool = p.show_pool_ref(&pools[0].0);
                if reuse.is_some() && cache.is_none() {
                    out.push(format!(
                        "`reuse` on a hold on `{pool}` that has no `cache` clause: such a hold \
                         consumes nothing of the session's cached prefix, so the bound has \
                         nothing to bound. Write `cache (0)` to consume the prefix and keep \
                         nothing, or `cache (ℓ)` to keep ℓ."
                    ));
                }
                cached_in_a_hold_without_cache(p, *body, Some((&pool, cache.is_some())), out);
            }
            CStmt::Branch(_, t, e) => {
                cached_in_a_hold_without_cache(p, *t, hold, out);
                cached_in_a_hold_without_cache(p, *e, hold, out);
            }
            CStmt::Loop(b) => cached_in_a_hold_without_cache(p, *b, hold, out),
            _ => {}
        }
    }
}

/// Every lint, over a linked program.
pub fn lint(p: &Program) -> Result {
    let mut out = vec![];
    for block in [p.session, p.init, p.turn] {
        stale_header_read(p, block, &mut out);
        constant_probability_guard(p, block, &mut out);
        cached_in_a_hold_without_cache(p, block, None, &mut out);
    }
    out.dedup();
    if out.is_empty() {
        Ok(())
    } else {
        Err(out.join("\n"))
    }
}
