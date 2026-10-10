//! The first target: vLLM v1's own scheduler, as a fixed-function
//! architecture (`docs/design/serving-specification-language.md`, §5).
//!
//! vLLM's scheduler runs one policy, which its configuration parameterises:
//! continuous batching on one step engine whose running requests are served
//! in admission order, a request-slot cap, a block pool with an LRU prefix
//! cache and LIFO preemption, FCFS admission. A program on that
//! architecture is synthesised to the configuration that makes vLLM run it,
//! and a program that states another policy is refused with the construct
//! vLLM cannot run. The stages' `cost` is the extern: vLLM runs the model,
//! so the cost model is not read. That the configured scheduler then
//! decides as the program does is what the oracle checks
//! (`tests/vllm_oracle.rs`, and `tests/target.rs` for the configuration).

use serde_json::{Value, json};

use crate::ir::{BinOp, CEvict, CExpr, CServe, CStageKind, CStmt, CtxVar, Preempt, Program};

fn number(p: &Program, e: &CExpr, what: &str) -> Result<f64, String> {
    match e {
        CExpr::Num(x) => Ok(*x),
        _ => Err(format!(
            "{what} is `{}`, which vLLM reads once from its configuration: it must be a constant",
            p.show_expr(e)
        )),
    }
}

/// The vLLM scheduler configuration that runs `p`, or the reason none does.
pub fn vllm(p: &Program) -> Result<Value, String> {
    let refuse = |m: String| Err(format!("not on vLLM's architecture: {m}"));
    // one engine; delay stages are the environment's or the served path's
    let steps: Vec<_> = p
        .stages
        .iter()
        .filter_map(|s| match &s.kind {
            CStageKind::Step(st) => Some((s, st)),
            _ => None,
        })
        .collect();
    let [(engine, step)] = steps.as_slice() else {
        return refuse(format!(
            "vLLM is one engine, and the program has {} engine(s)",
            steps.len()
        ));
    };
    if let Some(s) = p
        .stages
        .iter()
        .find(|s| matches!(s.kind, CStageKind::Fifo(_) | CStageKind::Ps(_)) && !s.kind.is_delay())
    {
        return refuse(format!(
            "stage `{}` is a queueing server; vLLM's only server is its engine, and a delay is all else it is given",
            s.name
        ));
    }
    // vLLM's running loop serves in admission order; keys it can observe are
    // supplied by the programmable scheduler (`tools/serq_vllm.py`)
    let serve_by = match &step.serve {
        CServe::By(keys) => {
            if let Some(k) = keys.iter().find(|k| !observable(k)) {
                return refuse(format!(
                    "`{}` advances running by `{}`; vLLM's scheduler observes `decoding` and `admission` of a \
                     running request, constants and arithmetic on them",
                    engine.name,
                    p.show_expr(k)
                ));
            }
            keys.clone()
        }
        _ => {
            return refuse(format!(
                "`{}` schedules `exclusive prefill`; vLLM mixes prefills and decodes in an \
                 iteration",
                engine.name
            ));
        }
    };
    // vLLM's `schedule()`: the running first, then the waiting while the
    // step has not preempted (scheduler.py:624-823, 868-1128), which is the
    // engine whose schedule lowers to no body, or a body that writes it out
    if let Some(body) = &step.iteration
        && !is_vllm_iteration(body)
    {
        return refuse(format!(
            "`{}` has its own schedule; vLLM's is `advance running; admit waiting while \
             (running.preempted == 0);`",
            engine.name
        ));
    }
    if p.blocks
        .iter()
        .flatten()
        .any(|s| matches!(s, CStmt::Fork(_)))
    {
        return refuse(
            "the request has legs (`fork`); vLLM's scheduler runs one engine, and the legs are \
             a router's"
                .to_string(),
        );
    }
    let Some(kv) = step.memory else {
        return refuse(format!(
            "`{}` has no memory; vLLM's engine has its KV cache",
            engine.name
        ));
    };
    if p.pools.len() != 2 {
        return refuse(format!(
            "vLLM has two pools, the KV blocks and the request slots, and the program has {}",
            p.pools.len()
        ));
    }
    let slots = 1 - kv;
    let (kvp, sp) = (&p.pools[kv], &p.pools[slots]);
    let Some(block) = kvp.block else {
        return refuse(format!(
            "pool `{}` has no block; vLLM allocates the KV cache in blocks",
            kvp.name
        ));
    };
    if !matches!(kvp.evict, CEvict::Lru) {
        return refuse(format!(
            "pool `{}` evicts by keys; vLLM's prefix cache is LRU",
            kvp.name
        ));
    }
    if kvp.preempt != Preempt::lifo() {
        return refuse(format!(
            "pool `{}` does not preempt as vLLM does: vLLM preempts the latest admitted and \
             re-queues it at the head (`preempt lifo`)",
            kvp.name
        ));
    }
    for q in [kvp, sp] {
        if q.reserve_held {
            return refuse(format!(
                "pool `{}` holds its reservations; vLLM tests the reservation at admission and \
                 keeps nothing",
                q.name
            ));
        }
        if q.queue.is_some() {
            return refuse(format!(
                "pool `{}` selects by keys; vLLM admits first come, first served",
                q.name
            ));
        }
        if q.spill.is_some() {
            return refuse(format!("pool `{}` spills; vLLM has no second tier", q.name));
        }
    }
    if sp.block.is_some() || !sp.cap.is_finite() {
        return refuse(format!(
            "pool `{}` is not a finite count of request slots",
            sp.name
        ));
    }
    let blocks = kvp.cap / block;
    if !(blocks.is_finite() && blocks.fract() == 0.0) {
        return refuse(format!(
            "pool `{}`'s cap is not a whole number of blocks",
            kvp.name
        ));
    }
    // vLLM's configuration counts tokens, requests and blocks in integers
    let count = |x: f64, what: &str| -> Result<u64, String> {
        if x >= 0.0 && x.fract() == 0.0 && x < u64::MAX as f64 {
            Ok(x as u64)
        } else {
            Err(format!(
                "not on vLLM's architecture: {what} {x} is not a whole number"
            ))
        }
    };
    let budget = count(
        number(p, &step.budget, "the `tokens cap`")?,
        "the `tokens cap`",
    )?;
    let chunk = count(vllm_chunk(p, &step.chunk, slots)?, "the `each at most`")?;
    let (seqs, block, blocks) = (
        count(sp.cap, &format!("pool `{}`'s cap", sp.name))?,
        count(block, &format!("pool `{}`'s block", kvp.name))?,
        count(blocks, &format!("pool `{}`'s blocks", kvp.name))?,
    );
    // a hold that caches its KV at release keeps a prefix the next turn reuses
    let prefix_caching = p.blocks.iter().flatten().any(|s| match s {
        CStmt::Hold { pools, cache, .. } => {
            cache.is_some() && pools.iter().any(|(r, _, _)| r.base == kv)
        }
        _ => false,
    });
    let mut out = json!({
        "target": "vllm",
        "engine": engine.name,
        "config": {
            "max_num_batched_tokens": budget,
            "max_num_seqs": seqs,
            "block_size": block,
            // vLLM counts the null block that serQ's pool leaves out
            "num_gpu_blocks": blocks + 1,
            "long_prefill_token_threshold": chunk,
            "enable_prefix_caching": prefix_caching,
        }
    });
    if !serve_by.is_empty() {
        out["config"]["scheduler_cls"] = json!("serq_vllm.SerqScheduler");
        out["serve_by"] = serde_json::to_value(&serve_by).expect("the IR serialises");
    }
    Ok(out)
}

/// vLLM's `long_prefill_token_threshold` as a program states it. vLLM
/// applies the cap only while more than one request is running or waiting
/// (scheduler.py:606-616), so a constant cap is not vLLM's: an engine writes
/// `each at most (threshold)` with `let threshold = running.count +
/// waiting.count > 1 ? c : inf;` (`long_prefill(c)`, lib/vllm.sq), or no
/// `each at most` for no cap. In the IR that is the chunk
/// `residents + queued(reqs) > 1 ? c : 0`, `reqs` the request-slot pool, or 0.
fn vllm_chunk(p: &Program, e: &CExpr, slots: usize) -> Result<f64, String> {
    use crate::ir::{CArg, Fun};
    let refuse = || {
        Err(format!(
            "not on vLLM's architecture: the `each at most` is `{}`; vLLM caps a prefill only while \
             another request is running or waiting (scheduler.py:606-616)\nhelp: in an \
             engine's schedule, `let threshold = running.count + waiting.count > 1 ? c : inf;` \
             and `each at most (threshold)`, or no `each at most` for no cap",
            p.show_expr(e),
        ))
    };
    match e {
        CExpr::Num(x) if *x == 0.0 => return Ok(0.0),
        CExpr::Cond(test, then, other) if **other == CExpr::Num(0.0) => {
            if let (CExpr::Num(c), CExpr::Binary(BinOp::Gt, lhs, one)) = (&**then, &**test)
                && **one == CExpr::Num(1.0)
                && let CExpr::Binary(BinOp::Add, res, q) = &**lhs
                && **res == CExpr::Ctx(CtxVar::Nres)
                && let CExpr::Call(Fun::Queued, args) = &**q
                && let [CArg::Pool(r)] = args.as_slice()
                && r.base == slots
                && r.count == 1
                && r.index.is_none()
            {
                return Ok(*c);
            }
        }
        _ => {}
    }
    refuse()
}

/// A serve key `tools/serq_vllm.py` can evaluate on a running request:
/// `decoding`, `admission`, numbers, and arithmetic, comparison and
/// conditionals on them.
fn observable(e: &CExpr) -> bool {
    use crate::ir::{BinOp, CtxVar};
    match e {
        CExpr::Num(_) => true,
        CExpr::Ctx(v) => matches!(v, CtxVar::Decoding | CtxVar::Admission),
        CExpr::Unary(_, a) | CExpr::Cost(_, a) => observable(a),
        CExpr::Binary(op, a, b) => *op != BinOp::Pow && observable(a) && observable(b),
        CExpr::Cond(c, a, b) => observable(c) && observable(a) && observable(b),
        CExpr::Attr(_) | CExpr::Sample(..) | CExpr::Call(..) | CExpr::Agg(..) | CExpr::Reg(_) => {
            false
        }
    }
}

/// The body `[Serve, Admit while !preempted]`, vLLM's `schedule()` written
/// out: the engine's own order and no `only`. The frontend lowers that
/// schedule to no body; an IR may still carry it as one.
fn is_vllm_iteration(body: &[crate::ir::CIter]) -> bool {
    use crate::ir::{CIter, UnOp};
    let not_preempted = CExpr::Unary(UnOp::Not, Box::new(CExpr::Ctx(CtxVar::Preempted)));
    matches!(
        body,
        [
            CIter::Serve { only: None, by: None },
            CIter::Admit { only: None, gate: Some(g) },
        ] if *g == not_preempted
    )
}
