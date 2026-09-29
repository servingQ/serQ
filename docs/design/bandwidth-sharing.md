# A transfer uses both ends

A KV transfer between a prefill and a decode instance moves its bytes out of
the sender's NIC and into the receiver's, at once. A `run` sits at one stage,
so a program today can put a transfer on one of the two and not on both.
This document is the design for a run that holds several stages at once, and
for the policy that divides their capacity. RFC #118. Nothing here is
implemented yet: the Before runs, the After is a sketch.

## What the systems do

**NIXL pull mode issues every read at once.** The decoder's worker starts
the READ of a request as soon as the scheduler hands it the request and the
handshake with that prefiller is done (`nixl/pull_worker.py:51-83`); the
handshake is per remote engine and cached, so after the first request of a
pair there is none. The READ is posted asynchronously and its completion is
polled at a later step (`nixl/pull_worker.py:545-558`). There is no cap on
the reads in flight: `num_threads` (`nixl/base_worker.py:636-647`) sizes
UCX's progress threads, not a queue of transfers.

**The sharing is below vLLM.** How concurrent reads divide a NIC, a PCIe
switch or a fabric link is decided by UCX and the hardware (for RDMA,
congestion control such as DCQCN), not by anything in `ref/vllm`. A
program therefore has to state the sharing as a modelling choice; the
source cannot supply it.

**Both ends are shared.** Two decoders reading from one prefiller share the
prefiller's egress; two prefillers read by one decoder share its ingress.
Push mode (`NixlPushConnector`) moves the same bytes over the same two ends
in the other direction: which worker posts the operation differs, which
links carry it does not.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.seq` puts a read on the
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
$ seq-lang run egress.seq
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

## After (sketch)

**A run over several stages.** `CStmt::Run` gains one field:

```rust
Run { stage: CRef, mode: RunMode, work: CExpr, growing: Option<CRef>,
      also: Vec<CRef> }   // further stages the same job holds at once
```

`also` empty is today's run, byte for byte. With stages in `also`, the job
is one flow over `stage` and every stage in `also`: it holds all of them
from its start to its end, and its rate is set by the sharing policy from
the capacities of all of them. The surface is the serving form's `on` with
a tuple:

```
stage egress[2] : ps(BwP);     // a prefiller's NIC, tokens per second
stage ingress[2] : ps(BwD);    // a decoder's NIC
share maxmin;
...
        run setup (x0);
        transfer on (egress[i], ingress[j]) (prompt - c) from kvP[i] to kvD[j] (prompt - 1 - c);
```

which is `run (egress[i], ingress[j]) (prompt - c); load kvD[j] (…); release kvP[i];`.
The work is in the unit the capacities share (tokens here), so links of
different bandwidth can carry one flow; the fixed latency stays a `delay`
(#131), outside the sharing.

**The link rule.** Every stage of a run with a non-empty `also` is `ps(φ)`
with `φ` a constant, the run is plain (no `prefill`/`decode` mode, no
`growing`), and no stage appears twice. A `φ` that depends on `n` has no
meaning for a flow that `n` does not describe, so it is a link error, not a
choice.

**The policy is the program's.** `share P;` is one declaration for the
deployment, because it is a property of the coupled links together, not of
one stage. Two policies to start with:

| `share` | rate of flow `f` | work-conserving |
|---|---|---|
| `maxmin` (default) | max-min fair: raise every rate together, freeze the flows of a link when it fills, repeat (progressive filling) | yes |
| `min` | `min over f's stages s of φ_s / n_s`: its equal share at the tightest of its stages | no: a flow held back at one stage leaves its share unused at the others |

A single-stage flow gets `φ / n` under both, which is today's `ps`.

**The simulator.** Today's PS keeps one virtual time per stage and a finish
tag per job, which works because every job at a stage has the same rate
(`src/engine/interp.rs:2019-2056`). Flows over different stages have
different rates, so a multi-stage flow keeps its remaining work; at every
start and end of such a flow the rates of its connected component are
recomputed (water-filling, O(links × flows)) and the earliest end is
scheduled. A stage no multi-stage flow touches keeps the virtual-time code,
so every existing program runs bit for bit as before.

**The view.** A multi-stage run is one station per stage, drawn stacked in
one column and bracketed as one job: the arrows enter and leave the
bracket, not the stations.

## Expected values

Derived by hand, for the tests the implementation adds:

1. The Before, with the sender's NIC in the run: two flows of 1 on
   `egress (1)` shared, each also on its own `ingress (1)`, both start at
   1: the egress is the bottleneck at 1/2 each, so both end at 3.0 under
   `maxmin` and `min`.
2. Three flows, two links, all of work 1 from t = 0: `A` of capacity 1
   carries `f1`, `f2`; `B` of capacity 2 carries `f1`, `f3`.
   - `maxmin`: `A` fills first at 1/2 each for `f1`, `f2`; `f3` takes the
     rest of `B`, 3/2, and ends at 2/3; `f1`, `f2` stay at 1/2 and end at
     2.
   - `min`: `f1 = min(1/2, 2/2) = 1/2`, `f2 = 1/2`, `f3 = 2/2 = 1`; `f3`
     ends at 1, `f1` and `f2` at 2. The two policies differ in `f3` only,
     which is the capacity `min` leaves unused.
3. `also` empty on every example: the report and the oracle IR files are
   unchanged.

## Cost

- `IR_VERSION` 6 → 7. The field is new, but an old reader that ignores it
  runs a two-link flow on one link and is silently wrong, which is the case
  `docs/ir.md` §Stability says bumps. #140 also plans v7; whichever lands
  first bumps and the other rides along until the tag.
- `serving-queue-theory`: `scripts/gen_seq_oracle.py` pins the version and
  reads `Run` by field name. `SeqExec`'s fragment admits `step` and `delay`
  stages only, so no oracle program is affected; the generator raises
  `Fragment` on a non-empty `also` so that a future program cannot slip
  through.
- The linker rule above, the `share` declaration, the flow solver, the view,
  `docs/language.md` §3, `docs/ir.md`, the API reference.

## What is not verified

- **The policy.** `maxmin` is the textbook model of fair bandwidth sharing
  and plausible for RDMA under congestion control; nothing in this
  repository measures it. The A6000 testbed
  (`~/dev/trace/202609-a6000-pd-nixl/`) moved its KV inside one node over
  `cuda_ipc` and cannot tell NIC sharing policies apart: that needs two
  nodes, concurrent reads and the per-transfer times.
- **Push mode's concurrent legs** (the `book` reservation of
  [The KV transfer](pd-transfer.md)) are a separate change; with this one,
  push and pull put a transfer on the same two stages and differ in who
  posts it and one notification.

## Self-critique

- **Not a `network` stage kind.** A stage holding a family of links
  (`stage net : network { links egress[NP], ingress[ND]; share maxmin; }`)
  was the first sketch. It adds an address space (a link inside a stage)
  that `CRef`, the linker and the view would all learn; `also` reuses the
  stages a program already declares and adds one field.
- **Not per-stage policies.** A policy on each `ps` stage would let two
  stages of one flow disagree, and no rule of the sharing says what a flow
  over a `maxmin` and a `min` stage gets.
- **Not sugar.** No rewrite of the kernel states a job that holds two
  stages at once; the Before's two workarounds are the proof.
- **`proportional` fairness is left out** of the first cut: it needs an
  iterative solver, and without a measurement no program has a reason to
  pick it over `maxmin`.
- **Link latency is not a stage property here.** The fixed part of a
  transfer is the program's `delay`, as #131 made it; folding it into the
  link would put the wait back into the sharing.
