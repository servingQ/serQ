# A transfer uses both ends

A KV transfer between a prefill and a decode instance moves its bytes out of
the sender's NIC and into the receiver's, at once. A `run` sits at one stage,
so a program today can put a transfer on one of the two and not on both.
This document is the design for a run that holds several stages at once, and
for the policy that divides their capacity. RFC #118, IR 8. The Before
runs on IR 7; the After runs, and `tests/shared_stages.rs` checks it
against the values derived by hand below.

## What the systems do

**NIXL pull mode issues every read at once.** The decoder's worker starts
the READ of a request as soon as the scheduler hands it the request and the
handshake with that prefiller is done (`nixl/pull_worker.py:51-83`); the
handshake is per remote engine and cached, so after the first request of a
pair there is none. The READ is posted asynchronously and its completion is
polled at a later step (`nixl/pull_worker.py:545-558`). The worker has no
cap on the reads in flight: `num_threads` (`nixl/base_worker.py:636-647`)
sizes UCX's progress threads, not a queue of transfers. The one bound is
the scheduler's: a read is admitted only when its blocks fit next to the
other reads' reservations (`scheduler.py:1200-1205`), which is the
program's `hold kvD`.

**The sharing is below vLLM.** How concurrent reads divide a NIC, a PCIe
switch or a fabric link is decided by UCX and the hardware (for RDMA,
congestion control such as DCQCN), not by anything in `ref/vllm`. A
program therefore has to state the sharing as a modelling choice; the
source cannot supply it.

**Both ends are shared.** Two decoders reading from one prefiller share the
prefiller's egress; two prefillers read by one decoder share its ingress.
Push mode moves the same bytes over the same two ends in the other
direction, a WRITE posted by the prefiller's worker
(`nixl/push_worker.py:714-722`): which worker posts the operation differs,
which links carry it does not.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.sq` puts a read on the
decoder's link alone:

```
stage link[2] : ps(1);     // a decoder's NIC: its reads share the bandwidth
...
        run setup (x0);
        transfer[j] ((prompt - c) / Bw) from kvP[i] to kvD[j] (prompt - 1 - c);
```

Two decoders pulling one prompt each from the same prefiller, each a
second's worth of bytes, both starting at t = 1:

```
pool kvP { cap 100; }
pool kvD[2] { cap 100; }
stage P : delay;
stage link[2] : ps(1);     // a decoder's NIC
stage D[2] : delay;
workload { arrive batch(2); }
session {
  set j = serial;
  hold kvP (10) { run P (1); } lease kvP (inf);
  hold kvD[j] (10) {
    transfer[j] (1) from kvP to kvD[j] (10);
    observe transferred = now;
    run D[j] (1);
  }
  end;
}
run { horizon 10; }
```
```
$ serq run egress.sq
transferred      2  2.0000    ±inf  0.000  2.0000
```

With the prefiller's NIC as fast as a decoder's, the two reads share it and
end at 3.0. The two ways the language offers today are both wrong:

- `run egress[i] (w); run ingress[j] (w);` passes the bytes through the
  two ends one after the other: twice the time, and a buffer between them
  that no NIC has, which is the store-and-forward link #115 took out of
  `transfer`.
- Scaling the work by the reads in flight at the sender, read when the
  transfer starts, misses every read that starts or ends while it runs.

## After

**A run over several stages.** `CStmt::Run` gains one field:

```rust
Run { stage: CRef, mode: RunMode, work: CExpr, growing: Option<CRef>,
      #[serde(default, skip_serializing_if = "Vec::is_empty")]
      also: Vec<CRef> }   // further stages the same job holds at once
```

`also` empty is today's run, and serialises as today's run. With stages in
`also`, the job is one *flow* over `stage` and every stage in `also`: it
holds all of them from its start to its end, and its rate is set by the
sharing policy from the capacities of all of them. The surface lists the
stages as a hold lists its pools, with commas:

```
stage egress[2] : ps(BwP);     // a prefiller's NIC, tokens per second
stage ingress[2] : ps(BwD);    // a decoder's NIC
share maxmin;
...
        run setup (x0);
        transfer on egress[i], ingress[j] (prompt - c) from kvP[i] to kvD[j] (prompt - 1 - c);
```

which is `run egress[i], ingress[j] (prompt - c); load kvD[j] (…); release kvP[i];`.
A parenthesised tuple is not used: `(k₁, k₂)` already means a lexicographic
key in `evict by` and `serve by` (and in #140's `choose by`), and a comma
list already means "all of these at once" in `hold a (u), b (v)`. The work
is in the unit the capacities share (tokens here), so links of different
bandwidth can carry one flow; the fixed latency stays a `delay` (#131),
outside the sharing. The indices of a stage array are evaluated once, when
the run starts, as a single-stage run's are.

**The link rule.** Every stage of a run with a non-empty `also` is `ps(φ)`
with `φ` a constant above 0 (a flow at rate 0 would wait for ever), and no stage array appears twice in one run: an index
is evaluated when the run starts, so `egress[i], egress[j]` could name one
stage twice, and the linker cannot tell. Every run on a shared stage (below),
single-stage runs included, is plain: no `prefill`/`decode` mode and no
`growing`, which a flow does not define (a `ps` stage allows neither
anyway). A `φ` that reads `present` has no meaning for a flow that
`present` does not describe, so it is a link error, not a choice.
`Program::validate` checks all of it, so IR from a tool meets the rule too. A program with a non-empty `also` anywhere must declare `share`.
Every stage in the list is checked as declared, as `on S`'s one stage is
today; the list is written only after `on` (and after `run`), since a form
without `on` finds its one stage by its role.

**The policy is the program's.** `share P;` is one declaration for the
deployment, because it is a property of the coupled links together, not of
one stage, and it has no default: the language supplies the mechanism and
the program names the rule. Two to start with:

| `share` | rate of flow `f` | work-conserving |
|---|---|---|
| `maxmin` | max-min fair: raise every rate together, freeze the flows of a stage when it fills, repeat (progressive filling) | yes |
| `bottleneck` | `min over f's stages s of φ_s / n_s`: its equal share at the tightest of its stages | no: a flow held back at one stage leaves its share unused at the others |

**Which stages are solved.** The partition is static, and its unit is a
declared stage, a whole array: an index is known only when a run starts. A
stage that appears in any run with a non-empty `also` (as its `stage` or in
its `also`) is a *shared* stage for the whole run: every job on it, a single-stage run
included, is a flow of the policy. Every other `ps` stage is served as
today, `φ(n) / n` by virtual time and finish tags
(`ps_reschedule`, `src/engine/interp.rs:2025-2062`), which is why every existing program
runs bit for bit as before. A single-stage flow on a shared stage is not
owed `φ / n`: under `maxmin` it takes what the multi-stage flows through
the same stage leave (`f3` in the second example below).

**The simulator.** Flows on shared stages keep their remaining work; at
every start and end of a flow every flow's rate is recomputed
(progressive filling for `maxmin`, a minimum per flow for `bottleneck`;
O(stages × flows)), the earliest end is scheduled, and an end scheduled
before is stale (`Interp::flows_reschedule`). A connected component at a
time would be cheaper on a large deployment; no program here is large
enough to ask for it.

**What a shared stage reports.** With `n_s` the flows holding `s` and `r_f`
their rates: `queue(s)`, `busy(s)` and the report's mean number are
`n_s`, as at any `ps` stage, where every job present is in service;
`work(s)` is the sum of their remaining work; utilisation is `Σ r_f / φ_s`,
the capacity carried, so `bottleneck` shows the share it leaves unused
where "not empty" would count the stage busy; `done` and the mean service
time count each flow once at every stage it held, its service being its
time from start to end, and so does the stage's price estimator.

**The view.** A multi-stage run is one station per stage, next to each
other in the station row and bracketed as one job (`BoxStyle::Flow`, drawn
as a rail so the other figures' bytes do not move): the session comes in at
the first station and leaves from the last, and no arrow runs between
them. The flow's stations are put side by side in the row, in the run's
order, where the first of them would stand (`adjacent_flows`), so the
bracket takes in no other station; an arrow the new order turns leftwards
is drawn as a return. Stacking them in one column would say "at once"
better, and needs edge routing the one-row layout does not have.

## Expected values

Derived by hand, and checked by `tests/shared_stages.rs`:

1. The Before, with the sender's NIC in the run: two flows of 1 on
   `egress (1)` shared, each also on its own `ingress (1)`, both start at
   1: the egress is the bottleneck at 1/2 each, so both end at 3.0 under
   `maxmin` and `bottleneck`.
2. Three flows, two stages, all of work 1 from t = 0: `A` of capacity 1
   carries `f1`, `f2`; `B` of capacity 2 carries `f1`, `f3` (`f3` a
   single-stage run on the shared stage `B`).
   - `maxmin`: `A` fills first at 1/2 each for `f1`, `f2`; `f3` takes the
     rest of `B`, 3/2, and ends at 2/3; `f1`, `f2` stay at 1/2 and end at
     2.
   - `bottleneck`: `f1 = min(1/2, 2/2) = 1/2`, `f2 = 1/2`, `f3 = 2/2 = 1`;
     `f3` ends at 1, `f1` and `f2` at 2. The two policies differ in `f3`
     only, which is the capacity `bottleneck` leaves unused. `B`'s
     utilisation until t = 2/3 is 2/2 under `maxmin` and 3/4 under
     `bottleneck`.
3. `also` empty on every example: the reports are unchanged. The oracle IR
   files change in their `"version"` only.

## Cost

- `IR_VERSION` 7 → 8 (7, `choose` over a tuple of keys, is tagged in
  `v0.1.0-rc5`). The fields are new, but an old reader that ignores them
  runs a two-stage flow on one stage and is silently wrong, which is the
  case `docs/ir.md` §Stability says bumps. Every `tools/oracle/*.ir.json`
  is regenerated and differs in its `"version"` line only.
- `serving-queue-theory`: `scripts/gen_serq_oracle.py` pins the version and
  reads `Run` by field name, so it would ignore `also` without complaint.
  `SerqExec`'s fragment admits `step` and `delay` stages only (a `ps` stage
  is already refused), so no oracle program is affected. The companion
  change pins 8 and raises `Fragment` on a non-empty `also` as well, so
  that the refusal does not rest on the stage kind alone; it lands when
  `serving-queue-theory` moves to the tag that carries IR 8.
- The language grows by three keywords (`share`, `maxmin`, `bottleneck`;
  79 → 82 in `tools/metrics.json`) and one IR field on `Run` and one on
  `Program`; no `CStmt` or `CExpr` variant is added.
- The rule above, the `share` declaration, the flow solver, the report's
  definitions, the view, `docs/language.md` §2 and §3, `docs/ir.md`, the
  API reference, the cheatsheet and the docs lexer.

## Relations

- **#72, the queue RFC.** Its `link` role (`transfer (n) from Q to POOL (m)`
  in the entry of one queue) runs in its own queue only; with this change
  the role's service is a flow over its own NIC and the other side's, which
  the role would name as it names the pool the KV arrives in.
- **Push mode's concurrent legs** (the `book` reservation of
  [The KV transfer](pd-transfer.md)) are a separate change; with this one,
  push and pull put a transfer on the same two stages and differ in who
  posts it and one notification.

## What is not verified

- **The policy.** `maxmin` is the textbook model of fair bandwidth sharing
  and plausible for RDMA under congestion control; nothing in this
  repository measures it. The A6000 testbed
  (`~/dev/trace/202609-a6000-pd-nixl/`) moved its KV inside one node over
  `cuda_ipc` and cannot tell sharing policies apart: that needs two nodes,
  concurrent reads and the per-transfer times.

## Self-critique

- **Not a `network` stage kind.** A stage holding a family of links
  (`stage net : network { links egress[NP], ingress[ND]; share maxmin; }`)
  was the first sketch. It adds an address space (a link inside a stage)
  that `CRef`, the linker and the view would all learn; `also` reuses the
  stages a program already declares and adds one field.
- **Not `stage: Vec<CRef>`.** Making the run's stage a list gives no stage
  a special place, at the same version bump, but renames the field the
  generator reads (`v["stage"]`) and makes every single-stage run a list of
  one. `also` keeps today's runs as they are, in the IR and in the
  generator.
- **Not per-stage policies, and one policy per deployment.** A policy on
  each `ps` stage would let two stages of one flow disagree, and no rule of
  the sharing says what a flow over a `maxmin` and a `bottleneck` stage
  gets. The price is that one deployment cannot say NVLink shares one way
  and RDMA another; a policy per connected component of shared stages
  would, and is the extension if a program needs it.
- **No default policy.** `maxmin` as a default would be a rule the language
  makes for every program that does not say otherwise (criterion 2).
- **Only `ps` stages.** A `delay` stage has no capacity to share, and a
  flow through one gains nothing from it. A `fifo` stage serves one job at
  a time, so a flow waiting in its queue while holding another stage is a
  hold-and-wait the flow semantics does not define.
- **Not sugar.** No rewrite of the kernel states a job that holds two
  stages at once; the Before's two workarounds are the proof.
- **`proportional` fairness is left out** of the first cut: it needs an
  iterative solver, and without a measurement no program has a reason to
  pick it over `maxmin`.
- **Link latency is not a stage property here.** The fixed part of a
  transfer is the program's `delay`, as #131 made it; folding it into the
  link would put the wait back into the sharing.
