# A run's latency

The wait before a KV copy (a NIXL READ posted and polled before the bytes
move, `kvt.nixl_pull (P, D, latency = x0, …)`; a link's `serve ps(Bw)
latency x0`) was a delay stage of its own, `D.nic.latency`, run before the
transfer over the two NICs. The figure folded it into the transfer by its
name; the report and the run's log did not, and showed a NIC as two
stages, `D.nic` and `D.nic.latency`, with the same count. This document puts
the wait on the transfer's run, `Run.latency`: one stage, the NIC, whose
report row has the wait in its `wait` column. Issue #611; IR 14.

## Before

`serq run examples/pd-disaggregation/llmd_nixl_pull.sq --horizon 300`
(seed 1, serQ feba100, 2026-10-14), the stage table:

```
stage             number   util  done    thru    wait  service   iters
----------------  ------  -----  ----  ------  ------  -------  ------
P.nic[0]           0.026  0.025   736  2.4533  0.0000   0.0105       0
D.nic[0]           0.015  0.015   584  1.9467  0.0000   0.0078       0
D.nic.latency[0]   0.004  0.004   584  1.9467  0.0000   0.0020       0
```

`serq ir` listed the stage `D.nic.latency[2] : ps(present)`, and the
decoder's connector ran, for a request its entry took `from P[i]`:

```serq
run D.nic.latency[j] (x0); run P.nic[i], D.nic[j] (n); load D.kv[j] (m); release P.kv[i];
```

The figure wrote `D.nic latency (2e-3)` on the read between the boxes,
having found a delay stage named `<link>.latency` and removed it
(`fold_latencies`): "only the name says so". The log of the run told a
session was `run` and `done` at `D.nic.latency[0]`, 2 ms, before its run
at `P.nic[0]+D.nic[0]`; a viewer that places a session at the figure's
stations had nowhere to draw it for those 2 ms (serq-lar's `docs/upstream.md`).

Issue #193 chose the delay stage over an IR change ("parse-time sugar
keeps the IR; a `latency` in the IR costs a version and the Lean
generator"), and noted that the stage would stay in the IR and the figure.

## After

The same command, same seed:

```
stage     number   util  done    thru    wait  service   iters
--------  ------  -----  ----  ------  ------  -------  ------
P.nic[0]   0.031  0.025   736  2.4533  0.0020   0.0105       0
D.nic[0]   0.019  0.015   584  1.9467  0.0020   0.0078       0
```

Every other line of the report (the client's `ttft`, `tpot` and
`response`, the observations, the gauges, the engines' step table, the
pools) is byte-identical to the Before, and so is `vllm_nixl_push.sq`'s.
The kernel:

```serq
run P.nic[i], D.nic[j] (n, share = s, latency = x0); load D.kv[j] (m); release P.kv[i];
```

(`latency` is the run's field in the IR, and the clause beside `share` its
kernel spelling, which `examples/pd-disaggregation/pd_two_fabrics.sq`
writes on its RDMA read in place of its delay stage `post`; the relation's
`latency = x0` and the link's `serve … latency x0` lower to it.) The figure
writes `latency (2e-3)` on the
read, from the run itself. The log tells one `run` at `P.nic[0]+D.nic[0]`,
2 ms earlier than before, and one `done`.

## The design

**`Run.latency` is seconds the job waits at its stages before its work
starts.** From the run's start the job is present at every stage of the
run and takes none of their capacity: a shared stage's flows divide it
among the flows past their wait; a `ps` stage's virtual time, a `fifo`'s
queue and a delay's clock start when the wait ends. The stage report
counts the wait as the run's `wait` (its column: "from a run's arrival at
the stage to its start"), and the job in `present`; `util` is unchanged,
since the wait carries no capacity. A run on a step stage takes no
`latency`: an engine's wait is its pool's. `latency` is read at the run's
start, as `work` is, and it is seconds, not a resource cost.

| Surface | Kernel |
|---|---|
| `kvt.nixl_pull (P, D, latency = x, share = s);` and `transfer (n) from P to kv (m)` in `D[j]`, called `from P[i]` | `run P.nic[i], D.nic[j] (n, share = s, latency = x); load D.kv[j] (m); release P.kv[i];` |
| `kvt.nixl_push (P, D, latency = x, share = s);` | the same run, after the `join` |
| `queue L : link { serve ps(b) latency x; }` and `transfer on L[k], M[l] (n, share = s)` | `run L[k], M[l] (n, share = s, latency = x + y)`, `y` `M`'s if it has one, in the order named; a transfer that writes its own `latency = …` over such a link is refused |
| `run a, b (cost(a, b, w), share = s, latency = x)`, `transfer on a, b (w, share = s, latency = x) …` | the kernel's spelling, as written |

**Why the run, not the stage.** A link's `latency` reads like the stage's
attribute. It is not one: in a pull `D` waits before the copies it posts,
and a queue that reads from one relation and is the source of another
(`E` pulls from `D`) would, with the wait on `D.nic`, make `E`'s copies
wait `D`'s latency too. The wait belongs to the copy, so to its run.

**The IR version.** An old reader that ignores `latency` runs the copy
without the wait: a change of execution, so the tagged IR 13 becomes 14
(`docs/ir.md`, Stability). `scripts/gen_lean_oracle.py` reads 14 and
keeps a run with a `latency` outside the Lean fragment, beside a run with
`also`; no oracle or claims program has one. The two queue goldens that
compared the sugar with a flat program with the delay stage written out
(`tests/golden/queues/a_link_latency_is_a_wait_before_the_read`,
`a_pull_relation_is_the_flat_read`) are re-blessed, their flat programs
now spelling the clause; those programs have not linked since #539, so the
reference is the sugar's output, and its diff is the three lines above.
`tests/run_latency.rs` derives the wait's numbers by hand instead: two jobs
on one `ps(1)` with one waiting, the same on two shared stages, a `fifo`,
and a run of no work after its wait.

**The view.** `fold_latencies` and its node removal go: the walk writes
the note on the flow when the run has a `latency`. A delay stage before a
transfer stays a station, as it did, since nothing is folded by name any
more.

## What it earns

- The report's stage table has one row per NIC member, with the wait in
  the column that means it, and no row the program did not write.
- The log and a replay of it place a session at the NICs for the whole
  copy; the IR has no stage named by the kernel.
- The figure reads the wait from the run, not from a naming convention.
- The engine's `Start` event is the only new mechanism: a job present
  before it is served, which `fifo` already had.

## Self-critique

**The wait is present at both NICs.** Before, the job waited at the
poster's `D.nic.latency` alone; now it is present at `P.nic[i]` and
`D.nic[j]` during the wait, and each NIC row's `present` rises by its own
throughput times the wait (`P.nic[0]`: 736 × 0.002 / 300 = 0.0049;
`D.nic[0]`: 0.0039, the latency row's). A posted READ engages the source's NIC no
more than it engages its own; a `latency` that names which of the run's
stages the job waits at would say so, at the cost of a second field. The
report's `util` does not move either way, and the numbers of every
other table do not either, so the field is one until a program needs the
other reading.

**A single-stage run's latency is not drawn.** A `transfer on L` naming
one link with a `latency` has no flow, so no note; the station is the
link and the wait is in the IR and the report. The figure gains the note
on that station when a program needs it.

**One `latency` per poster stays.** The rule came from the stage's name,
one `P.nic.latency` per queue; the run carries the wait, but the wait's
constant is still named for the poster (`P.push.time`), so two pushes from
one queue with a latency each would clash there. The check is kept, with
its message no longer naming a stage, until a program wants the opposite.

**A link's `serve … latency` has no example.** `pd_two_fabrics.sq` writes
the clause on its transfer, and the relations lower to the field; the
link's spelling and the sum of several links' waits are reached by tests
only (`tests/queues.rs`, `tests/run_latency.rs`). It predates this change
(#193); whether it stays is a question for the next program that reads a
link.

**A copy of zero bytes still waits, and is now a run of the NIC.** Before,
the delay stage ran for a request whose KV was all cached and the zero
transfer returned at once, uncounted; now the run waits at the NICs and
completes when the wait ends, served for nothing, so it is in the NIC
rows' `done` and `thru` with a service of 0 and a wait of the latency
(the review's check: with half the copies empty, `done` doubles and the
mean `service` halves; the client's lines do not move). Neither example
has such a copy (`D.nic.latency`'s `done` equalled `D.nic`'s on every
seed). Whether vLLM posts a READ for no blocks is a question for
`ref/vllm`, not this change.

**Several links' latencies are one wait.** Before, `transfer on a, b`
with two latencies ran two delay stages in turn; now one run waits their
sum. The durations are the same up to the rounding of `t + x + y` against
`(t + x) + y` (a relative 4e-13 on a client mean in the review's check),
and the log shows one run, not three.
