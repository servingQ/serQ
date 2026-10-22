# Integer ticks

The interpreter's clock was an `f64`, and "the same instant" was whatever
the last bit of a sum said: runs of 0.2 then 0.1 ended after runs of 0.25
then 0.05, a hold that queued at 0.1 had waited 0.19999999999999998 at 0.3,
and an iteration of 1371 tokens at 2·10⁻⁵ s ended at 0.027420000000000003
(#66, [IR v4](ir-v4.md) §4). This document makes the clock a whole number
of *ticks*, 10⁻⁹ of its unit: every duration the run schedules is rounded
to the nearest tick, and the clock adds ticks. No IR field changes shape;
the meaning does, so `IR_VERSION` goes from 15 (tagged in v0.1.7-alpha.2)
to 16.

## Before

Each program below runs with `arrive batch(n); session { turn; }` (`n` the
sessions it names), as `serq run P --workload W --horizon 10 --log L.json`,
the binary of f8483cd. The log's `wait` and `admit` events are shown.

**Two sums of one time.** Session 0 reaches a gate of one unit after runs
of 0.2 then 0.1, session 1 after 0.25 then 0.05:

```serq
fn main() {
  stage d : delay;
  pool gate { cap 1; }
  server {
    if (serial == 0) { run d (cost(d, 0.2)); run d (cost(d, 0.1)); } else { run d (cost(d, 0.25)); run d (cost(d, 0.05)); }
    hold gate (cost(gate, 1)) { observe who = serial; observe at = now; run d (cost(d, 1)); }
  }
}
```

```json
{"t": 0.3, "ev": "wait", "s": 1, "pool": 0}
{"t": 0.3, "ev": "admit", "s": 1, "pool": 0}
{"t": 0.30000000000000004, "ev": "wait", "s": 0, "pool": 0}
{"t": 1.3, "ev": "admit", "s": 0, "pool": 0}
```

**`waited`.** Session 0 holds the gate from 0 to 0.25 + 0.05; session 1
queues at 0.1, session 2 at 0.15; a hold that has waited 0.2 goes first,
else the newest serial (Ascend's aging, `examples/vendors/ascend.sq`, writes
`elapsed >= max_wait`):

```serq
  pool gate { cap 1; queue by (waited >= 0.2 ? 0 : 1, -serial); }
  server {
    if (serial == 1) { run d (cost(d, 0.1)); }
    if (serial == 2) { run d (cost(d, 0.15)); }
    hold gate (cost(gate, 1)) {
      observe who = serial;
      observe at = now;
      if (serial == 0) { run d (cost(d, 0.25)); run d (cost(d, 0.05)); } else { run d (cost(d, 1)); }
    }
  }
```

```json
{"t": 0.3, "ev": "admit", "s": 2, "pool": 0}
{"t": 1.3, "ev": "admit", "s": 1, "pool": 0}
```

At 0.3 session 1 had waited `0.3 - 0.1`, 0.19999999999999998.

**`age`.** Session 0 releases 10 cached units at 0.1, session 1 releases 5
at 0.2; at 0.3 session 2 needs 15 of 25, and one entry goes, an old one
(`age >= 0.2`) first, then the smaller:

```serq
  pool kv { cap 25; evict by (age >= 0.2 ? 0 : 1, size); }
  stage d : delay;
  server {
    if (serial == 0) { hold kv (cost(kv, 10)) { run d (cost(d, 0.1)); } cache (cost(kv, 10)); }
    if (serial == 1) { run d (cost(d, 0.15)); hold kv (cost(kv, 5)) { run d (cost(d, 0.05)); } cache (cost(kv, 5)); }
    if (serial == 2) { run d (cost(d, 0.3)); hold kv (cost(kv, 15)) { run d (cost(d, 0.1)); } }
    run d (cost(d, 1 + serial));
    if (serial == 0) { hold kv (cost(kv, 10)) { observe kept0 = cached; } cache (cost(kv, 10)); }
    if (serial == 1) { hold kv (cost(kv, 5)) { observe kept1 = cached; } cache (cost(kv, 5)); }
  }
```

`kept0` 10, `kept1` 0: session 0's entry was 0.19999999999999998 old, both
were young, and session 1's went.

**A tick at a release.** `queue fifo every (0.1)`; session 0, let in at the
tick at 0.1, releases after 1.1 then 0.1:

```serq
  pool gate { cap 1; queue fifo every (0.1); }
  server {
    hold gate (cost(gate, 1)) {
      observe who = serial;
      observe at = now;
      if (serial == 0) { run d (cost(d, 1.1)); run d (cost(d, 0.1)); }
    }
  }
```

```json
{"t": 0.1, "ev": "admit", "s": 0, "pool": 0}
{"t": 1.4000000000000001, "ev": "admit", "s": 1, "pool": 0}
```

The release came at 1.3000000000000003, after the tick at `13 * 0.1`, 1.3.

**A copy that is free.** `examples/pd-disaggregation/pd_batching.sq`'s
baseline reads the KV at `Bw` 10¹⁵ tokens/s and observes `transfer =
D.load_done - D.admitted`. At seed 1, horizon 20: mean 2.0257e-12, cv2
0.094, p99 2.9949e-12, the noise of `f64` sums for a copy the program
says is instantaneous.

**The log.** `docs/reference/log.md`'s example, `llmd_nixl_pull.sq` under
`chat_reply.sq`: `{"t": 0, "ev": "iter", "stage": 0, "dur":
0.027420000000000003, "tokens": 1371, …}`, and every event of the
iteration's end at `"t": 0.027420000000000003`.

## After

The same commands with this change:

| Program | Before | After |
|---|---|---|
| two sums | session 1 at 0.3, session 0 waits from 0.30000000000000004, admitted at 1.3 | both reach the gate at 0.3; session 0's run was scheduled first: session 0 at 0.3, session 1 at 1.3 |
| `waited` | session 2 at 0.3, session 1 at 1.3 | session 1 (waited 0.2) at 0.3, session 2 at 1.3 |
| `age` | `kept0` 10, `kept1` 0 | `kept0` 0, `kept1` 5 |
| a tick at a release | session 1 at 1.4000000000000001 | session 1 at 1.3: the release and the tick are one instant, and the pools' ticks act after its other events |
| `run d (4e-10); observe a = now; run d (6e-10); observe b = now;` | `a` 4e-10, `b` 1e-9 | `a` 0, `b` 1e-9 |
| pd_batching `transfer` | mean 2.0257e-12, cv2 0.094 | every sample 0 |
| the log | `"dur": 0.027420000000000003` | `"dur": 0.02742`, the end at `"t": 0.02742` |

Each is a test: `tests/integer_clock.rs` and, for the tick,
`tests/queue_ticks.rs` (`a_tick_at_a_release_admits_at_that_instant`).

## The rule

The clock is a `u64` of ticks; one tick is `1 / TICKS_PER_UNIT` of the
clock's unit. The resolution and the rounding are one table in
`src/ir.rs`, which `Program::validate` and the interpreter read:

```rust
pub const TICKS_PER_UNIT: f64 = 1e9;
pub fn to_ticks(x: f64) -> u64 { (x * TICKS_PER_UNIT).round_ties_even() as u64 }
pub fn from_ticks(t: u64) -> f64 { t as f64 / TICKS_PER_UNIT }
```

- **Each duration is rounded where it is scheduled.** `Interp::after(dt,
  ev)` schedules `ev` at `clock + to_ticks(dt)`: a run's work at rate 1, a
  `latency`, an iteration's `cost`, an arrival's gap, a lease's time, the
  next finish of a `ps` stage or a flow. A pool's ticks are at `k *
  to_ticks(dt)`, in integers. `horizon` and `warmup` are rounded the same
  way. The event queue orders by the tick, then by the existing rank (a
  pool's tick after the instant's other events) and sequence number.
- **The nearest tick, a half to the even one.** A duration under half a
  tick takes none: it ends at the instant it began, after the events
  already scheduled there.
- **What a program reads.** `now`, `E.admitted`, `E.first`, `E.load_done`,
  the observations and the log's `t` are `from_ticks(clock)`, the nearest
  `f64`. `waited`, `age` and `last` are computed from ticks: a waiting
  hold's `queued_at` and a cache node's `last` are ticks, and `waited` is
  `from_ticks(clock - queued_at)`. An iteration's `dur` in the log and the
  step-duration statistic are the rounded duration.

Nothing else moves. The settle rule ("an iteration starts only once every
event of its instant has been handled") compares ticks; so does the run's
count of the times a session became ready at one instant.

## Why the nearest

A ceiling or a floor of `x * 1e9` depends on the last bit of `x` again:
`1371 * 2e-5` is 0.027420000000000003, whose ceiling is 27 420 001 ticks,
and the nearest is 27 420 000. Rounding each duration, not the absolute
time `now + d`, is what makes `0.2 + 0.1` and `0.25 + 0.05` one instant:
`to_ticks(0.2) + to_ticks(0.1)` and `to_ticks(0.25) + to_ticks(0.05)` are
both 300 000 000, while `0.2 + 0.1` and `0.25 + 0.05` differ in `f64`.
Rounding half to even is the `f64` rule and takes no side; a half-tick
duration is a value no program writes on purpose.

## Why the IR keeps its units

The issue's sketch put ticks in the IR: `horizon: u64`, `Program.tick_ns`,
`Instant { tick, sub }`. Most of the IR's times are expressions only the
run can evaluate: a run's work becomes time at its stage's rate, a step's
`cost` is read after the batch is scheduled, a gap may be a draw, a lease's
time is read at the release. Only the run can round them. A few constant
fields in ticks (`every`, `queue_every`, `ttl`, `horizon`, `warmup`) beside
those would give one dimension two units in one program (criterion 0). The
IR keeps every time as a number in the clock's unit, and the rounding is
one rule, applied once, where the interpreter schedules.

The unit stays the program's: a program whose costs are seconds runs in
seconds, and `examples/oracle/vllm_request.sq` runs on the step clock
(`forward (1)`). A tick is 10⁻⁹ of either. `tick_ns` would make the unit
a field; nothing reads it but the rounding, and one constant is the
rounding.

`sub`, the `n` of a superdense instant, was not added. The event queue
already orders an instant's events by the rank that puts the pools' ticks
last and then by the sequence number they were scheduled in (#52,
`docs/language.md` Ties): that order is the `n`. A counter beside it would
record the same order twice (one list, one place).

## The Lean fragment

The fragment's clock is ℕ (`lean/Serq/Exec.lean`: "time is a natural
number"), its step clock `cost 1` one unit. A unit is 10⁹ ticks: every
fragment time is a natural number of units, hence a whole number of ticks,
the embedding ℕ → 10⁹ℕ is exact, and the interpreter's instants on a
fragment program are Lean's. The generators pin 16 and read a 15 file the
same way (`scripts/gen_lean_oracle.py`, `SUPPORTED_IR_VERSIONS`); the
generated `lean/Serq/Oracle.lean`, `Regress.lean` and `Claims.lean` carry no
version and do not change, nor do the interpreter's answers to the Lean
regressions (`scripts/lean_regress.py`).

Making one tick the fragment's unit, as the issue's After said, would need
ticks in the IR (above) or the generator scaling every cost and horizon by
10⁹, which the `decide +kernel` theorems would pay for.

## What a fixed resolution cannot run

A clock with a resolution refuses what would round to no time where no
time is a loop. Each check is stricter, and bumps nothing on its own; each
message reads the resolution from `TICKS_PER_UNIT`.

| Program | Before | After |
|---|---|---|
| `preempt leases by (serial) every (1e-10)` | ran | does not link: "pool `prog`: `preempt leases by` `every (0.0000000001)` is under half a tick of the clock (1e-9 of its unit), which counts whole ticks: it would round to no time, and what it times would end, or come again, at the instant it began" |
| `queue fifo every (1e-10)` | ran | does not link, the same message |
| a send's `ttl = 1e-10` | ran | does not link: "a send's `ttl` 0.0000000001 is under half a tick …" |
| `arrive renewal(1e-10)` | did not end | does not link: "`arrive renewal(0.0000000001)`: an interarrival time is at least one tick of the clock (1e-9 of its unit), which counts whole ticks; under half of one, every arrival would come at one instant and the run would never pass it" |
| `--horizon 1e-10` | ran | does not run: "run: warmup 0 and horizon 0.0000000001 round to one tick of the clock (1e-9 of its unit), so the measurement would last no time: need 0 <= warmup < horizon, at least one tick apart" |
| `--horizon 1e11` | ran | an error when the run starts: "run: horizon 100000000000 is past the clock's last tick, 18446744073.709553 of its unit (2^64 - 1 ticks of 1e-9)" |
| `worker { forward (1e-10); }` | ran | an error at the iteration: "engine `llm`: its `worker` is 0.0000000001 for an iteration of 100 tokens, under half a tick of the clock (1e-9 of its unit); an iteration that schedules tokens lasts a positive time, at least one tick" |
| `arrive renewal(~uniform(1e-11, 1e-10))` | did not end | an error at the 1 000 001st arrival at one instant: "`arrive renewal(~uniform(1e-11, 1e-10))`: 1000001 arrivals at the instant t = 0: its gaps are under half a tick of the clock (1e-9 of its unit), which counts whole ticks, so they take no time" |
| `loop { run d (cost(d, 1e-10)); }` | did not end | the instant does not settle, as for a run of zero work; the message names "a `run` of zero work or of under half a tick of the clock" |

The horizon's limit is checked when the run starts, not in `validate`:
`serq check` and `draw` give a program without a horizon a placeholder of
`f64::MAX` (`lib::inspected`). The arrivals' bound is the number of
sessions a `batch` may hold (`ir::MAX_SESSIONS`), what one instant is
known to carry.

A static check of a `run`'s constant work under half a tick was not added:
work becomes time at the stage's rate (`ps(φ)`, a shared stage's
capacity), so a constant under a tick is neither always nor only a zero
duration. The run catches the loop.

## What it moves

Every example under each of its workloads (`examples/pairs.json`), `serq
run P --workload W --horizon H --warmup H/10 --seed 1`, with the binaries
of f8483cd and this change:

- at `H` 200, 41 of 42 reports are the same line for line;
  `pd_batching.sq`'s `transfer` row reads cv2 NaN where it read 0.088 (its
  samples are all 0);
- at `H` 2000, 40 of 42 are the same: `pd_batching.sq` as at 200, and two
  fewer events and two fewer iterations of `D[0]` (8 872 382 → 8 872 380),
  its other columns the same; `fastertransformer.sq` under `periodic.sq`,
  the p99 of its `running` status 1975.1407 → 1975.1408.

The oracles (`tools/oracle`), the claims (`tools/claims`), the golden queue
reports, the Lean regressions' answers and every other test are unchanged;
their IR files move only their `version`.

## Why it happened

The assumption that was wrong: that two sums of the same durations are the
same instant. `f64` addition is not associative, so `0.2 + 0.1` and `0.25
+ 0.05` are two instants, and `0.3 - 0.1` is not `0.2`. The rule "these
are one instant" was written in five places, each on `f64`: the event
queue's order (`Entry::cmp`, `total_cmp` on the time), the settle rule's
`pending_now` (`e.time <= self.now`), the pools' tick schedule, which
patched it (`if k as f64 * dt <= self.now { k += 1 }`), the differences
behind `waited` and `age`, and the count of a session's readies at one
instant (keyed by `now`). One integer clock carries all five; the tick
schedule's patch is gone.

The same class was fixed for token counts in #661 ([Whole token
counts](whole-tokens.md)): the trie's sum of two releases read an ulp short
of the count the workload wrote. That fix made the counts integers; this
one makes the instants integers. What is left on `f64` is arithmetic on
times that no instant depends on: the statistics (a stage's service, a
response time, a status's stay) and a program's own differences (below).

## Should the language have refused it

No. The five programs above are correct: each says what it means, and the
interpreter answered by the last bit of a sum. No static check can tell
which sums a run will make coincide. The new checks cover only what a
fixed resolution cannot run, each with its test and its reason in the
message.

## Self-critique

- **Ticks in the IR's fields** (the issue's sketch). Rejected: the IR's
  times are mostly expressions only the run evaluates; a few constants in
  ticks beside them would be two units for one dimension.
- **Ceiling or floor.** Rejected: both depend on the last bit of the
  product they round (27 420 001 ticks for 1371 × 2·10⁻⁵).
- **A minimum of one tick** for every positive duration. Rejected: a second
  rule beside the nearest, and it would give pd_batching's free copy a
  nanosecond the program did not write. The `f64` clock already absorbed a
  duration under half an ulp of `now`, a threshold that moved with `now`;
  the tick fixes it at 0.5·10⁻⁹.
- **A tolerance on time comparisons** (`EPS`). Rejected: a second rounding
  hidden in each comparison, and it does not reach a comparison the program
  writes, as the trie's `EPS` did not reach `cached >= K` (#661).
- **A configurable resolution** (`tick_ns`). Not done: nothing needs it
  yet. A program that needs a finer time writes its times in a smaller
  unit, which brings the clock's last tick nearer in that unit (2⁶⁴ − 1
  ticks: about 584 years in seconds, 213 days in milliseconds).
- **Rational time.** Rejected: exact, but every cost the program computes
  (`batch.tokens * 2e-5`) is already an `f64`, and the rationals would only
  carry its rounding further.
- **A static check of sub-tick `run` work.** Rejected: the stage's rate
  makes it unsound (above).
- **A remaining gap.** A program's own difference, `now - E.admitted`, is
  arithmetic on the two `f64`s `from_ticks` gives, and may be off in the
  last bit where `waited` is not. Making times integral in `CExpr` would be
  a type for every consumer of the IR; `waited` is the context variable
  that says "the time since" without the subtraction. The statistics are
  differences of instants too, and pd_batching's `transfer` is 0 because
  its two instants are one tick, not because the subtraction is exact in
  general.
