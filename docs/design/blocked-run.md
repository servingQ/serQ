# A run that ends blocked

A run *ends blocked* when its report finds, at some pool, a session that no
event of the run would complete. Two witnesses say so, each per pool:
`outgrown`, a hold that asked its pool to grow past the cap, and
`deadlocked`, sessions waiting for room only sessions waiting like them
hold. `Report::blocked` is the one predicate; `serq run` prints the report
and exits 3 on it, pyserq reads it as `Report.blocked`, and the JSON carries
it as `blocked`. `stuck` keeps its count and loses the meaning "livelock":
it counts thrash as well (#480). No IR change: the report and the CLI only,
and `REPORT_VERSION` stays 1, the fields being added (`tests/report.rs`
holds the shape).

## Before

The only counter of a livelock was `stuck`, a session preempted again at a
position no further than its previous preemption
(`src/engine/interp.rs:4489-4495` at f8483cd):

```rust
        if let Some(&prev) = self.sessions[victim].preempt_pos.get(&pl) {
            if computed <= prev + EPS && !self.sessions[victim].stuck {
                self.sessions[victim].stuck = true;
                self.pools[pl].stuck += 1;
            }
        }
        self.sessions[victim].preempt_pos.insert(pl, computed);
```

#74 made `run` fail on `stuck > 0`; review broke its premise and #77
reverted it, and the test said why (`tests/cli.rs:1-4`):

```rust
//! The binary's exit status. A run whose report counts `stuck` sessions is a
//! result to read with care, not a failure: `stuck` counts a no-progress
//! re-preemption once per session and rises under ordinary overload too
//! (#64 holds the livelock criterion that would change this).
```

The same count came out of two opposite runs. The livelock of
`tests/pool_semantics.rs` (`livelock.sq`, a request of 100 + 101 tokens on
a 160-token pool):

```serq
fn main() {
  device gpu { kv cap 160; }
  engine llm on gpu {
    reqs cap 4;
    tokens cap 1000;
    scheduler {
      advance running;
      admit waiting while (running.preempted == 0) taking reqs (1), kv (100) reserve (100);
    }
    worker { forward (1); }
  }
  pool reqs on llm { }
  pool kv on gpu { block 16; evict lru; preempt lifo; }
  server { llm (100, 101); }
}
```

```text
$ serq run livelock.sq --workload w.sq --horizon 400     # w.sq: arrive batch(1); session { turn; end; }
kv    20.9     0.0  0.000    0.152     NaN     340         0         0      340      0    0      1
stuck: 1 session(s) preempted again at pool `kv` without passing the position of their previous preemption
$ echo $?
0
```

And #480's thrash, where every session goes on:

```text
$ serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --Lambda 0.5 --blocks 1000 --max_seqs 64 --chunk_cap 128 --horizon 20000 --warmup 2000 --seed 1
run: horizon 20000 end 20000 warmup 2000 seed 1 events 14786073 arrivals 9905 ended 8901 turns 80095 mean live 13.831
pool    used  cached  queue  holders    wait  admits  evict(n)   evict(u)  preempt  spill   rej  stuck
kv    6757.5  9221.2  0.000    1.073     NaN   94710     78111  538020240     7569      0  1279     31
stuck: 31 session(s) preempted again at pool `kv` without passing the position of their previous preemption
```

vLLM resets `num_computed_tokens` to 0 when it preempts
(`scheduler.py:1561`), so a re-prefill preempted again
lower down is ordinary under overload.

Two runs ended with sessions that could never complete and said nothing.
Holds taken in opposite orders (`dl.sq`):

```serq
fn main() {
  pool a { cap 1; }
  pool b { cap 1; }
  stage svc : delay;
  server (x) {
    if (x) {
      hold a (cost(a, 1)) { run svc (cost(svc, 1)); hold b (cost(b, 1)) { run svc (cost(svc, 1)); } }
    } else {
      hold b (cost(b, 1)) { run svc (cost(svc, 1)); hold a (cost(a, 1)) { run svc (cost(svc, 1)); } }
    }
  }
}
```

```text
$ serq run dl.sq --workload dlw.sq --horizon 100     # dlw.sq: arrive batch(2); init { set x = serial == 0 ? 1 : 0; } send (x);
run: horizon 100 end 100 warmup 0 seed 1 events 2 arrivals 2 ended 0 ...
a      1.0     0.0  0.990    1.000  0.0000       1         0         0        0      0    0      0
b      1.0     0.0  0.990    1.000  0.0000       1         0         0        0      0    0      0
$ echo $?
0
```

The wait-for fixpoint that finds such a cycle existed
(`waiting_for_each_other`, #372), but only a cycle through a send failed
the run. And `growing_stalled` (#440) restated "none will free room" for
growths without the lease expiry the fixpoint had. A session whose own
lease of 50 s frees the room it grows into (`lease.sq`):

```serq
fn main() {
  pool kv { cap 160; }
  stage d : delay;
  server {
    hold kv (cost(kv, 100)) { run d (cost(d, 1)); } lease kv (50);
    hold kv (cost(kv, 16)) { grow kv (cost(kv, 64)); run d (cost(d, 1)); }
  }
}
```

```text
$ serq run lease.sq --workload one.sq --horizon 40
grow: 1 hold(s) wait to grow in pool `kv` when the run ends, and every holder of `kv` is one of them, so none will free room (a hold around one on the same pool, or holders waiting on each other)
$ serq run lease.sq --workload one.sq --horizon 100
run: horizon 100 end 100 warmup 0 seed 1 events 3 arrivals 1 ended 1 ...
```

## After

```text
$ serq run livelock.sq --workload w.sq --horizon 400
kv    20.9     0.0  0.000    0.152     NaN     340         0         0      340      0    0      1
stuck: 1 session(s) preempted again at pool `kv` without passing the position of their previous preemption
outgrown: 1 session(s) live at the end asked to grow pool `kv` past its cap in a hold they keep open: no preemption makes room, and the hold never completes
livelock.sq: the run ended blocked: a session never completes at pool `kv` (the report says why in its `outgrown:` and `deadlock:` notes)
$ echo $?
3
$ serq run dl.sq --workload dlw.sq --horizon 100
deadlock: 1 session(s) wait for room in pool `a` that only sessions waiting like them hold, and no event will free it
deadlock: 1 session(s) wait for room in pool `b` that only sessions waiting like them hold, and no event will free it
dl.sq: the run ended blocked: a session never completes at pools `a`, `b` (the report says why in its `outgrown:` and `deadlock:` notes)
$ echo $?
3
$ serq run lease.sq --workload one.sq --horizon 40
grow: 1 hold(s) wait to grow in pool `kv` when the run ends, for room another holder may free
$ echo $?
0
```

#480's configuration still reports `stuck 31`, nothing blocked, exit 0.

## The criterion

### `outgrown`

`grow` records the pool when a hold's growth asks for more than the cap,
counting what the session's holds of the pool keep: the sum, over its hold
entries there, of the allocation less the cached prefix it reads through
another's allocation, plus the growth rounded to blocks. The record is
cleared where `preempt_pos` is, when a hold of that pool completes (its
scope's end, its request's end, its `release`), so it outlives the hold's
preemptions. The report counts the sessions live at the end with one.

Why no event completes such a hold: the cap is constant, and the session's
holds of the pool stay allocated while the hold is open (an enclosing hold
is not preempted by its inner one's growth, which preempts the innermost),
so `used + growth > cap` at every later try. Under `preempt none` the
growth waits for ever; under any form that takes it, the hold runs again
to the same growth (the livelock above). It is the negation of the
hypothesis of [IR v4](ir-v4.md) §7's theorem, `cap ≥ prompt + max_out`,
which vLLM checks once at start-up: it refuses a KV cache that cannot hold
one request of `max_model_len` (`kv_cache_utils.py:864-900`).
A lease's growth past the cap is already a run-time error.

### `deadlocked`

`waiting_for_each_other` is a greatest fixpoint: start with every session
that waits, in a queue, to grow, or for a send, and drop a session while
what it waits for can still come. What stays waits only for the others.

- A queued hold stays when one of its parts can never fit a pool *held
  still*. The part's least ask is its units rounded to blocks, less the
  cached prefix other live holds keep allocated along its chain, all of
  it whatever its `reuse`. A part whose units read the clock, a draw or
  the state (`ir::moves`, the rule `enqueue_hold` rejects by) is not
  judged: a later try may read it lower. One such part is enough: the hold
  is admitted only when every part fits.
- A growth stays when it does not fit (`fits_grown`, the test
  `retry_growers` makes) and its pool is held still.
- A load stays when every live leg it waits for stays (#372).

A pool is held still when it has holders, all of them in the set, and no
room comes without an event of theirs: no lease of the pool has a finite
expiry, no tick of `preempt leases by … every` is due while the pool is
over its cap, and no growth there could preempt one of them. The last is
read from who a growth would take now, `candidates` under a form that
preempts holds and `lessees` under `preempt leases by`. A resident of an
engine is a candidate there, and no session in the set is a resident; on a
pool that is no engine's memory, a holder in a scope is one, so a later
admission's growth may break a deadlock there (the test
`a_deadlock_a_later_growth_may_break_is_not_blocked`).

The report counts, per pool, the sessions of the set waiting for its room:
a growth at its pool, a queued hold at the pool of its first part that
stays short, which need not be the pool whose queue it is in (an engine's
request queues at its first pool, `reqs`, and may wait for `kv`). A load
among them still fails the run with #372's message, which names the
sessions and the fix (a finite `ttl`).

### `growing_stalled`

The field and its name stay. It is now read from the same set: one of the
pool's growths is in it. The rule it replaces, "every session with an
allocation in this pool waits to grow in it", was the fixpoint's growth
case written a second time without the lease expiry, which `lease.sq`
shows. `allocates_in`, which only it read, goes.

## What is not blocked

Each of these can still move, so none is in the verdict:

- `over_cap`: its ask reads state, and an ask over the cap at one try may
  fit at another (`tests/pool_semantics.rs`,
  `a_reserve_that_reads_the_state_waits_instead_of_being_rejected`, has
  one waiting over the cap that is admitted later).
- `idle_with_work`: an arrival or a timer may wake the engine.
- A growth waiting for room another holder may free (`growing_at_end`
  without `growing_stalled`).
- `rej`: the program's own rule ended the session.
- `stuck`: thrash, which completes.
- Sessions live at the end, or not yet admitted: an open run always ends
  with some, overload with more. Tutorial 6's program at 2.0 sessions/s
  ends its 20 000 s with 214 sessions live (`--warmup 0`: arrivals
  39 709, ended 39 495), and nothing blocked.

## Exit status

| Code | Meaning |
|---|---|
| 0 | done |
| 1 | no report: the program does not load, or the run fails (a run-time error, the send's hold-and-wait cycle, `--arrivals` not drained) |
| 2 | usage |
| 3 | the report is printed (and `--dump`, `--log` written), and the run ended blocked |

3 is what the review of #74 asked for: a code a script tells apart from a
program that does not load. `serq run` names the blocked pools on stderr.

## The counts

With the branch's binary, before choosing the exit status (#74's review
asked to count what it would fail):

| Runs | Blocked |
|---|---|
| `docs/development.md`'s sweep, `Lambda` 0.3, 0.6, 0.9 × seeds 1-3, `--horizon 2000` | none of 9 |
| `docs/tutorial/06-the-cliff.md`'s program, `Lambda` 1.5, 1.7, 1.8, 1.9, 2.0, seed 1, `--horizon 20000` | none of 5 |
| #480's configuration (`stuck 31`) | no |
| every pair of `examples/pairs.json`, `--horizon 100` and `--horizon 2000` | none of 42 × 2 |
| `vllm_replay.sq` at `--blocks` 100, 200, 400, 800, 1600 (`rej` 333 … 9) | none of 5 |
| `thunderagent.sq` with `closed_agents.sq`, `N` 4, 16, 48 × `tr` 0, 1 | none of 6 |
| `tests/report.rs`'s corpus test (every example and tutorial program, `--horizon 100`) | none |

`serving-queue-theory` is pinned at serQ v0.1.5 and does not see the new
exit status until it moves; its configurations of `vllm_replay.sq` are
the ones above.

## Self-critique

**`stuck > 0` fails the run (#74).** Reverted in #77: a re-prefill
preempted below its previous position is vLLM's behaviour
(`scheduler.py:1561`) and completes. `stuck` stays a measure of thrash.

**The same position k times.** The issue's first candidate. A position
read under recompute depends on block alignment and on cache hits, so k
is a tuning knob for a fact the growth past the cap states exactly; and
under `preempt none` the livelock never preempts, so no position is ever
read.

**A static check.** The linker would refuse `preempt lifo` with a session
whose most is above the cap. A workload's `prompt + o` is a distribution
(`o ~ exp(…)`) with no bound, and a bound a program writes (`vllm.sq`'s
`max_model_len`) has no construct the linker could read. The same goes for
the deadlock: `dl.sq` runs at cap 2, so the program is not wrong, its
instance is ([The push mode](push-mode.md) made the same call for a
send's cycle).

**"Live at the end and never admitted"** (the issue's comment). It catches
#145's wait at the head, but an open run ends with sessions that arrived
and wait for their first admission, more of them the more it is
overloaded: the verdict would fail a run for where its horizon fell.
`deadlocked` catches the wait at the head that no event ends, and #145's
`reserve` above the cap is a rejection (`rej`) since.

**`over_cap` and `idle_with_work` in the verdict.** Each can still move;
a verdict on them fails a run that completes with a longer horizon.

**A run-time error instead of exit 3.** It would drop the report, which is
what the reader needs to see why. Exit 1 keeps meaning "no report".

**Folding the send's cycle into `deadlocked`.** It stays an error with its
message, which names the sessions and the fix, and which per-pool counts
cannot; folding it would also move a run from exit 1 to exit 3.

**The queued ask as it queued.** #372's rule, which a first draft of this
change kept, judged a queued hold by the units it asked when it queued. A
hold whose units read the clock asks something else at the next try, and
a `queue … every` tick makes that try: `tests/pool_semantics.rs`'s
`a_hold_whose_ask_moves_is_not_deadlocked` was called deadlocked at
horizon 40 and ends both its sessions by 100. Only a part whose units do
not move is judged now; a deadlock whose every part moves is not
counted. The draft also held a pool still whatever its `preempt`, which
`a_deadlock_a_later_growth_may_break_is_not_blocked` shows wrong the same
way.

**Every short part held still.** #372's rule asked every pool the hold did
not fit to be held still. One suffices: the hold is admitted only when
every part fits.

**A `reserve` above the units.** Admission waits for the units or the
reservation above them, and the least ask reads the units only: a hold
short only by its `reserve` is judged by less than it asks, and a deadlock
there is missed (the run exits 0 with the sessions live, as before this
change). A reservation that does not move could be read, but only by
evaluating it, which `amount` does with the run's state to change (a
draw, an error), where the fixpoint only reads; the queued hold keeps its
units and not its reservation. A miss costs a verdict and never makes one
up, so this is left to a program that needs it.

**Counted at the queue's pool.** A first version counted a queued hold at
the pool whose queue it is in. An engine's request queues at its first
pool: behind a request that outgrew `kv` under `preempt none`, the note
said two sessions waited at `reqs` for room only waiting sessions hold,
and `serq run` named `reqs` blocked, with 3 of its 4 free
(`a_hold_short_of_a_later_pool_is_deadlocked_there`). The fixpoint now
says, for each session it keeps, the pool whose room it waits for.

**`outgrown` across re-executions.** The record outlives the hold's
preemption, which is what catches the livelock. A hold whose units or
control flow read state may ask less on a re-run and complete; while it is
running again at the end it is counted. No program here does this.

**A session's own lease is not counted in what it keeps.** A growth past
the cap beside the session's lease of the pool is not `outgrown`: the lease
may end. It is `deadlocked` if nothing else can move.

**A later admission.** A pool where a growth could take one of the waiting
holders is never held still, so a deadlock on a pool with `preempt by`
that no later admission will break is not counted (with no arrivals left,
`dl.sq` with `preempt lifo` on both pools ends unblocked). Telling the two
apart would need what arrives after the horizon.

**The zero-time spin.** A step that only preempts at zero cost loops at
one instant (#555, `a_zero_cost_preempting_step_does_not_hang`, ignored).
That run does not reach a report; it is not this verdict's.
