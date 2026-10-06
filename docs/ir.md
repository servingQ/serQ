# The serQ IR

The IR is the definition of a serQ program. Everything else is built around
it:

```
  program text (.sq)  ──parse + link──▶  IR (ir::Program, JSON)  ──▶  interpreter (interp)
  tools (Rust / JSON) ──────build/edit──▶                          ──▶  Lean model (generated)
                                                                   ──▶  checks, diffs, archives
```

The [text syntax](language.md) compiles to this representation. Tools can
also build or edit it directly, for example to supply explicit sessions or
inline a trace with `serq ir --inline-trace`. Expressions are data rather
than host-language code, and every input passes `Program::validate`.

Source: `src/ir.rs`. Current version: `IR_VERSION = 11`.

## Format

JSON (`serde`): structs are objects with the field names below, enums are
externally tagged (`{"Num": 3.0}`, `{"Binary": ["Add", a, b]}`, unit
variants as strings, `"Lru"`). JSON has no infinity: an infinite constant
(`inf` in a program, a pool without `cap`) is the string `"inf"` or
`"-inf"`, and a reader of `Num` or `cap` takes a number or that string. `serq ir FILE` prints it;
`serq run/check FILE.json` reads it.

### `Program`

| Field | Meaning |
|---|---|
| `version` | `IR_VERSION`; a different version is rejected |
| `attrs` | attribute names; an attribute is referenced by its index (slot) |
| `observes` | observation names, by index |
| `pools` | `CPool` declarations, below |
| `stages` | `CStage` declarations, below |
| `arrival` | `Poisson(rate)`, `Renewal(gap expression)`, `Closed(n)`, `Batch(n)`, `Sessions([{attrs: [[slot, value], …]}])`, `None` |
| `trace`, `trace_ordered` | a trace corpus the workload draws turns from (path, resolved against the program's directory unless overridden) |
| `init`, `turn`, `session` | block indices: the workload's `init` and `turn` blocks and the session program |
| `blocks` | the statement blocks (an arena; bodies of holds, branches and loops refer to blocks by index) |
| `horizon`, `warmup`, `seed`, `arrivals` | the run; `arrivals` requires exactly N open arrivals and draining by `horizon` |
| `hidden` | attribute slots the scheduler may not read (`hidden o;`): legal at `Session` and `Given`, below |
| `share` | `MaxMin` or `Bottleneck`: how the flows of runs over several stages divide the stages' capacity; present exactly when some `Run` has a non-empty `also`, omitted otherwise |
| `gauges` | `[{name, expr}]`: functions of the state whose time average the report gives, each read at the `Gauge` moment after every instant; omitted when empty. They read and do not act, so a reader that ignores them runs the same program |
| `claims` | `[{name, given?, kind, expr}]`; read-only properties checked on the simulated path; omitted when empty |
| `registers` | `[{name, stage, init}]`; persistent stage registers, below; omitted when empty |
| `slot_cached`, `slot_serial`, … | slots of the built-in attributes (`cached`, `serial`, `turn_no`, `new`, `out`, `think`, `more`, `forced`, `computed`) |

`Sessions`: all the sessions arrive at time 0; each one runs `init`, then
its preset `attrs` overwrite what `init` set. A session may carry `turns`
(each a list of `[slot, value]`): its `turn` statements then read them in
order instead of the trace corpus, with the corpus's rule (`turn_no` counts
turns, the turn's values are set, `more` is 1 while another turn remains
and 0 after the last). `Program::with_sessions` builds sessions from
attribute names; `Program::inline_trace` (CLI `serq ir
--inline-trace`) replaces an ordered trace by its sessions' turns, which
runs identically (`tests/ir.rs`).

### Pools (`CPool`)

| Field | Meaning |
|---|---|
| `name`, `index?` | Resource name and optional array-member label. `index` is present even for a one-member array; it labels reports and does not affect execution. |
| `cap`, `block` | Capacity in units and optional allocation granularity. |
| `evict` | `Lru` or `By([keys])`, read at `Evict`. |
| `preempt` | `None` or `By {keys, tail}`. The least keys at `Victim` select the victim; ties select the last admitted. `tail` defaults to false (requeue at the head); true requeues as a newcomer. |
| `queue` | `null` for FIFO, otherwise nonempty pure selection keys, read at `Select`. |
| `spill` | Optional `CSpill {to, via, work, when}`: destination pool, transfer stage, work and predicate. |
| `admit_via` | Optional index of the step stage serving this pool's queue. |
| `reserve_held` | Outstanding reservations count against later admissions and other holds' growth. Omitted when false. |

`preempt lifo` compiles to `By {keys: [-admission], tail: false}`.
For allocation, caching, reservation lifetime and preemption behavior, see
[Pool](api/pool.md).

### Stages (`CStage`, `CStep`)

A `CStage` has `name`, optional `index` (the same report-label rule as a
pool), and `kind`: `Fifo(servers)`, `Ps(capacity)`, `Delay` or `Step(CStep)`.

| `CStep` field | Meaning |
|---|---|
| `budget`, `chunk` | Expressions read at `Budget`. |
| `cost` | Iteration duration, read at `Step`. |
| `granule?` | A positive constant, including infinity; prefill alignment. Omitted when absent. |
| `serve` | `By([keys])` in ascending order, ties by admission order, or `ExclusivePrefill`. Keys are pure expressions read at `Serve`. |
| `memory` | Optional pool index supplying resident KV totals and preemption candidates. |
| `iteration?` | A list of `CIter` statements, below. Omitted for the default serve-then-admit procedure. Cannot accompany `ExclusivePrefill`. |

`By([])` means admission order. `decode first` compiles to
`By([decoding ? 0 : 1])`. The [stage reference](api/stage.md) defines
chunking, exclusive prefill and the default iteration procedure.

| `CIter` variant | Reads at | Meaning |
|---|---|---|
| `Serve {only?, by?}` | `Serve` | Serve residents not yet served; absent `by` uses the stage's order. |
| `Admit {only?, gate?}` | `Serve` for `only`, `Plan` for `gate` | Admit and serve newcomers while budget, capacity and gate permit. An excluded newcomer remains resident but unserved. |
| `Branch(guard, then, else)` | `Plan` | Execute `then` for 1, `else` for 0. |
| `Set(register, expr)` | `Plan` | Update a register owned by this stage. |

Every path through an iteration body must reach `Serve` or `Admit`.
The frontend expands `serve only (p)` into
`[Serve {only: p}, Admit {only: p, gate: !preempted}]`.

### Registers and claims

A register's `stage` must have an iteration body and cannot be an array
member; its initial value is finite. `CExpr::Reg(index)` reads it only in
its stage's expressions and body, iteration claims, queue keys of pools
that stage admits, hold headers whose first pool is such a pool, gauges,
or `AtEnd` claims. Sets are undone when an attempt schedules, preempts and
admits nothing. See [registers](api/stage.md#registers).

A claim's `kind` is `EveryIteration(stage)`, `SomeIteration(stage)` or
`AtEnd`. Its expression is read at `Iteration` or `End`, respectively.
The optional `given` is checked at `Given` for every session; a zero value
puts the claim out of scope. Names must be distinct. Claims and gauges do
not affect execution, so an IR reader may ignore them without changing the
simulated path. See [claims](api/program.md#claim) for the distinction
between simulation checks and proofs.

### Statements (`CStmt`)

| Statement | Meaning |
|---|---|
| `Turn` | draw the next turn's attributes (workload `turn` block or trace) |
| `Set(slot, e)`, `Observe(k, e)` | assign an attribute, record an observation |
| `Hold {pools: [(pool, units, reserve?)], reuse?, body, cache?, lease?}` | acquire units of every pool (admission gate `reserve` if given), run `body`, release; with `cache` the admission consumes the own cached prefix, `reuse` bounds how much, and `cache` is the units left cached; without `cache` the hold leaves the session's cached prefix where it is; `lease: (pool, t)` keeps that pool's allocation past the scope, neither evictable nor a preemption victim, until the session's `Release` of it, `t` seconds, or its end (vLLM's `delay_free_blocks`) |
| `Grow(pool, e)`, `Drop(pool)` | grow the current hold, drop the own cached entry |
| `Release(pool)` | give the innermost enclosing hold's allocation on the pool back now, or end the session's lease of it, caching per the hold's `cache`; nothing held or leased there is a no-op. A KV transfer between instances is `Run` (the link), `Load` (the destination) and `Release` (the source's lease) |
| `Load(pool, e)` | the KV of `e` tokens arrived from outside the engine (a NIXL read): the innermost enclosing hold's computed position on the pool advances by `e`, within its allocation |
| `Run {stage, mode, work, growing?, also?}` | work at a stage; `mode` `Plain`, `Prefill`, `Decode` (step stages); `growing` the pool that grows with the tokens computed; `also` further stages the same job holds at once (a flow of `share`), omitted when empty |
| `Branch(e, then, else)`, `Loop(body)`, `Choose {var, count, key}` (`key` a list, compared in order), `End` | control; `End` ends the session |
| `Fork(body)`, `Join` | `Fork` runs `body` beside the session as a leg of the request: from now, with a copy of the attributes, its own holds and stream; the leases it leaves pass to the session when it ends. `Join` waits until every leg the session forked has ended |

### Expressions (`CExpr`)

`Num`, `Attr(slot)`, `Ctx(var)` (`Now`, `Waited`, `Size`, `Age`, `Last`, `Queued`,
`N`, `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Decoding`,
`Admission`, `Remaining`, `Position`, `Admitted`, `Preempted`, `Demand`,
`Served`, `Arrived`: availability is defined by the moments below), `Sample(dist, args)`,
`Reg(register)` (a stage register by index), `Call(fun, args)` (arithmetic functions, pool and stage queries such as
`CachedIn(pool)`, `BudgetLeft(stage)`), `Unary`, `Binary`, `Cond`, and
`Agg(agg, observation)`: `Total`, `Count`, `Largest`, `Smallest` (0 when
none) or `PrefixTotal` (`Σ_k Σ_{i≤k} v_i` over the values sorted
ascending) of every value the run observed under the observation, warm-up
included; read at the `End` moment only. Pool
and stage references are `CRef {base, count, index?}` (a family of
`count` pools from `base`, selected by `index`).

## Validation

`Program::from_json`, `run_ir` and the linker call `Program::validate`:
text and directly constructed IR meet the same rules. Validation covers:

- **Structure.** Version, names and indices, run parameters, function
  signatures and distribution arities. Blocks reachable from `init`,
  `turn` and `session` form a tree; `init` and `turn` contain only `Set`
  and `Observe`.
- **Evaluation.** Expressions read only what their moment supplies.
  Hold headers (`units`, `reserve`, `reuse`) cannot draw.
  `BudgetLeft` references only step stages, including every member of an
  indexed family.
- **Ownership.** `Grow`, `Load`, `Release` and a run's `growing` require an
  enclosing hold with the same `CRef` (base, count and index expression);
  `Release` may also end a lease. A hold leases only its own pool, names
  each reference once, and cannot change attributes its index reads.
  An index read again inside the hold cannot depend on state or time.
  Preemptible holds cannot use such indices or `cached`/`computed` in
  their pool indices.
- **Parallel legs.** A `Fork` body contains no `Turn`, `End`, `Fork` or
  `Join` and cannot act on its parent's holds. A fork cannot sit inside a
  hold whose pool may preempt. A program with either `Fork` or `Join`
  must contain the other.
- **Progress and amounts.** Every loop path reaches a `Run` with positive
  or nonconstant work, a hold whose body does so, or `End`.
  Constant work and resource amounts cannot be negative or NaN. A
  constant hold demand, rounded to blocks, must fit at least one pool its
  reference can name. See [Every instant settles](language.md#every-instant-settles)
  for runtime checks of progress that static validation cannot establish.
- **Arrivals.** Constant renewal gaps are positive; Poisson rates are
  positive and finite. `Closed` and `Batch` populations lie between 1 and
  `MAX_SESSIONS`.
- **Stages and flows.** A run uses `Prefill` or `Decode` exactly on step
  stages, and `growing` only there. Every stage family named by a
  multi-stage run is a `Ps` with constant capacity; all runs on such a
  shared stage are `Plain`, without `growing`. A run names each family
  once. `share` is present exactly when some run has a nonempty `also`.

`validate_located` identifies statement errors by block and statement
index; the text frontend maps these to source locations. Declaration and
flow checks may have no statement location. `src/ir.rs` is the authoritative
set of validation rules, including restrictions on scheduling expressions
and registers described above.

### Moments

An expression is evaluated at one moment, fixed by
its position in the IR. Context variables may be read only at the listed
moments. Additional checks forbid `Now` in `Gauge`, `Given`, `only`
predicates and iteration-body guards, admission gates and assignments:

| Moment (`ir::Moment`) | Positions | Context variables |
|---|---|---|
| `Session` | statements of `init`, `turn`, `session`; a run's work; a hold's `cache` (read when the session releases); `Grow`, `Load`, `Branch`, `Choose` | `Now` |
| `Admit` | a hold's units, `reserve`, `reuse`, admission bindings | `Now` |
| `Select` | a pool's queue keys, reevaluated for each waiting hold before every admission attempt | `Waited`, `Now` |
| `Evict` | eviction keys, a spill's `work` and `when` | `Size`, `Age`, `Last`, `Queued`, `Now` |
| `Ps` | a `ps` stage's capacity | `N`, `Now` |
| `Budget` | a step stage's `budget` and `chunk`, evaluated before the iteration from its residents (a `BudgetLeft` is rejected: it is planned from a budget) | `Nres`, `Ndec`, `Kvb`, `Kvp`, `Now` |
| `Step` | a step stage's `cost`, evaluated after the iteration is scheduled | `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Now` |
| `Serve` | a step stage's `serve by` keys and `only`, evaluated for one resident at its turn, the totals as the residents stand then (a session the iteration admitted included) | `Decoding`, `Admission`, `Remaining`, `Nres`, `Ndec`, `Kvb`, `Kvp`, `Now` (`only` not `Now`) |
| `Victim` | a pool's preemption keys, evaluated for each candidate when growth cannot fit | `Decoding`, `Admission`, `Position`, `Now` |
| `Plan` | iteration-body branch guards, admission gates and register assignments | `Nres`, `Ndec`, `Kvb`, `Kvp`, `Ntok`, `Npre`, `Admitted`, `Preempted` |
| `Gauge` | a gauge, evaluated on the state an instant ends with and held until the next, with no session (an `Attr`, a `Sample`, `CachedIn`, `Now`, `Work` or `BudgetLeft` is rejected, and an index is a `Num` in range, so reading it cannot fail the run) | none |
| `Given` | a claim's `given`, evaluated for each session once its `init` has run (and its preset attributes are set), on its attributes (`hidden` ones included: a claim is no scheduler) and the constants; no `Sample`, no `Call` but arithmetic | none (not `Now`) |
| `Iteration` | a claim over the iterations of a step stage, evaluated when an iteration starts, after its batch is scheduled (where the cost is read); no `Attr`, no `Sample`, of the calls only arithmetic, `Queue`, `Busy`, `Used`, `Free`, `Holders`, `Queued`, an index a `Num` in range | `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Demand`, `Served`, `Arrived`, `Now` |
| `End` | a claim `at end`, evaluated once when the run ends, when no session is live; no `Attr`, no `Sample`, no `Call` but arithmetic; the only moment that reads `Agg` | `Now` |

`Demand` is the tokens the stage's residents could take in the iteration
if the budget were unlimited: `min(1, remaining)` for a decode, the
remaining work (capped by a positive `chunk`) for a prefill, summed over the
residents after the batch is scheduled (those admitted by it and those
`only` excludes included). `Served` is the tokens the stage scheduled in its
earlier iterations, over the whole run.

The index of a pool or stage reference (`CRef.index`) is evaluated with the
expression around it, so at that expression's moment: `evict by (size +
used(kv[size]))` is legal. The table is what the interpreter fills into its
context at each position (`interp.rs`: `Ctx`), not a policy: `tokens` in a
budget would read 0 because the tokens are not scheduled yet, so the budget
may not read it.

An attribute listed in `hidden` may be read at `Session` or by a claim's
`Given` condition. Scheduler reads are rejected. This is a validation
restriction: a valid program runs the same with or without the declaration.
The IR does not distinguish server statements from workload statements.
The additional [server hidden-attribute rule](api/workload.md#hidden) is
checked by the text linker.

## Ties

Tie-breaking rules are defined in [language semantics](language.md#3-semantics).
They are part of the IR's meaning, not configurable fields. The Lean model
uses the same ordering information, such as release order for LRU eviction.

## Stability

`IR_VERSION` identifies program meaning, not just JSON shape. The
interpreter rejects a different version. Regenerate older JSON from its
source program and check the applicable
[release notes](https://github.com/servingQ/serQ/releases) for behavior
changes.

For a **tagged** IR version:

| Change | Version bump? |
|---|---|
| Remove, rename or retype a field or variant | Yes |
| Change the meaning of an existing field or construct | Yes, even if the JSON still parses |
| Add a field an old reader can ignore without changing execution | No |
| Add a field an old reader would need to preserve execution | Yes, regardless of defaults or serde attributes |
| Strengthen validation of an invalid program, or reject a run that cannot progress | No |
| Change draw streams or their seeding, changing the run identified by a seed | Yes |

While a version is untagged, changes are recorded for the coming release
without another bump. Consumers of an untagged version must pin a commit.

The Lean generators pin the version and read fields by name. A version
change updates the generators and affected generated files together;
`make lean` checks their agreement. Committed oracle IR is regenerated
with `make oracle-ir` and checked by `make check`. `tests/ir.rs` checks
exact JSON round trips and agreement between execution from text and IR.

## The Lean fragment

The Lean model (`lean/Serq/Exec.lean`, [docs/lean.md](lean.md)) runs a fragment of
the IR over natural numbers, event by event as the interpreter does: pools
with LRU eviction and LIFO preemption, one step engine (stage 0) whose
iteration cost uses supported terms in `tokens`, `prefilled`, `decoders`,
`kv_decode` and `attention`, with natural coefficients and a constant term
of at least 1 (`cost 1` is the step clock; attention coefficients must be even), delay stages, explicit sessions with preset attributes and turns, and the
statements `Turn`, `Hold`, `Run`, `Set`, `Observe`, `Branch`, `Loop`, `End`
(not `Release`, `Load`, `Fork`, `Join` or a hold with a `lease`)
with expressions built from integer constants, attributes, `Now`,
`CachedIn`, `BudgetLeft`, `min`, `max`, `+`, `-` (truncated at 0), `*`,
`floor(a / b)`, `ceil(a / k)` with a constant `k`, comparisons, `!`, `&&`,
`||` and conditionals; an expression of constants alone is folded first,
as the interpreter computes it. In a cost, a context
variable with a zero coefficient (the replay's cost at `a = b = 0`) is
dropped. Its generator translates an IR file into Lean and
fails on anything outside the fragment. The Lean `Branch` takes the first
block when the guard is non-zero; the interpreter admits only 0 or 1
(`docs/language.md`, Branching), so the two agree on every program that
runs.

The claims generator additionally accepts the body generated by `serve only`,
one queue key on a pool that no run grows, and supported tiled costs. See
[the Lean guide](lean.md#what-is-not-covered-yet) for these restrictions.
Arbitrary iteration bodies, registers, `reserve_held`, `granule` and
preemption policies other than LIFO are not translated.
