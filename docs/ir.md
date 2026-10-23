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

The source model and externally supplied execution settings resolve to one
complete `Program`; its JSON records the experiment as well as the deployment.

Source: `src/ir.rs`. Current version: `IR_VERSION = 16`.

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
| `attr_types` | mandatory type per slot: `Size`, `Value`, or `Cost(CostTarget)` |
| `sides` | mandatory `Workload`/`Server` per statement, aligned with `blocks`, including nested bodies |
| `observes` | observation names, by index |
| `pools` | `CPool` declarations, below |
| `stages` | `CStage` declarations, below |
| `arrival` | `Poisson(rate)`, `Renewal(gap expression)`, `Closed(n)`, `Batch(n)`, `Sessions([{attrs: [[slot, value], …]}])`, `None` |
| `trace`, `trace_ordered` | a trace corpus the workload draws turns from (path, resolved against the program's directory unless overridden) |
| `init`, `turn`, `session` | block indices: the workload's `init` and `turn` blocks and the session program |
| `blocks` | the statement blocks (an arena; bodies of holds, branches and loops refer to blocks by index) |
| `horizon`, `warmup`, `seed`, `arrivals` | the run; `arrivals` requires exactly N open arrivals and draining by `horizon` |
| `hidden` | attribute slots the scheduler may not read (a `hidden` parameter, `server (hidden output)`): legal at `Session` and `Given`, below |
| `gauges` | `[{name, expr}]`: functions of the state whose time average the report gives, each read at the `Gauge` moment after every instant; omitted when empty. They read and do not act, so a reader that ignores them runs the same program |
| `claims` | `[{name, given?, kind, expr}]`; read-only properties checked on the simulated path; omitted when empty |
| `registers` | `[{name, stage, init}]`; persistent stage registers, below; omitted when empty |
| `routes` | `[{gateway, block, from, to}]`: a gateway's `route`, expanded where the server called it (`gw.route();`), is the statements `blocks[block][from..to]`, the blocks nested in them included. They run as any of the server's statements; the deployment view draws them as the gateway. Omitted when empty. A reader that ignores them runs the same program. `validate` refuses a range the block does not have |
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
| `preempt` | `None`, `By {keys, tail}` or `Leases {keys, every?, suspend?}`. The least keys at `Victim` select the victim; ties select the last admitted. Under `By`, the victim is a hold, which runs again; `tail` defaults to false (requeue at the head); true requeues as a newcomer. Under `Leases`, the candidates are the sessions that lease the pool and hold it in no scope, and the victim's lease ends while its session goes on. With `every: dt` (seconds) the pool acts at every multiple of `dt` instead: a lease's growth is let through past the cap, and at a tick leases end in the keys' order while the pool is over its cap; `every` does not link with `reserve_held`; with `suspend: [keys]` (which needs `every`) a lease taken, at a tick or by a hold's growth, is set aside, its units back, a prefix it read let go and `Allocated` 0, and a later tick, before it takes any, gives set-aside leases back in the order of the `suspend` keys (read at `Victim`), each that fits; `Release` or the session's end drops one. |
| `queue` | `null` for FIFO, otherwise nonempty pure selection keys, read at `Select`. |
| `queue_every` | Optional seconds: the pool admits only at every multiple, the holds that waited when the tick began in the queue's order, each admitted one run until it blocks before the next is tried, one that does not fit passed over; a hold queued during a tick waits for the next. At an instant the pools' ticks follow its other events, in pool order, a pool's `queue_every` tick before its `Leases` `every` tick. Not on a pool with `admit_via`. |
| `spill` | Optional `CSpill {to, via, work, when}`: destination pool, transfer stage, work and predicate. |
| `admit_via` | Optional index of the step stage serving this pool's queue; a stage of another kind is refused, since nothing would admit the queue. |
| `reserve_held` | Outstanding reservations count against later admissions and other holds' growth. Omitted when false. |

`preempt lifo` compiles to `By {keys: [-admission], tail: false}`.
For allocation, caching, reservation lifetime and preemption behavior, see
[Pool](api/pool.md).

### Stages (`CStage`, `CStep`)

A `CStage` has `name`, optional `index` (the same report-label rule as a
pool), and `kind`: `Fifo(servers)`, `Ps(capacity)` or `Step(CStep)`. A
delay stage is `Ps(Ctx(N))`, `ps(present)` (`CStageKind::delay`).

| `CStep` field | Meaning |
|---|---|
| `budget`, `chunk` | Expressions read at `Budget`. |
| `cost` | Iteration duration, read at `Step`. |
| `granule?` | A positive constant, including infinity; prefill alignment. Omitted when absent. |
| `serve` | `By([keys])` in ascending order, ties by admission order, or `ExclusivePrefill`. Keys are pure expressions read at `Serve`. |
| `memory` | Optional pool index supplying resident KV totals and preemption candidates. |
| `iteration?` | A list of `CIter` statements, below. Omitted for the default serve-then-admit procedure. Cannot accompany `ExclusivePrefill`. |
| `request?` | `CRequest`: how the engine admits and releases a request (`CStmt::Request`), below. Omitted when it takes nothing and caches nothing. |
| `connector?` | `CConnector`: its KV connector, to an L2 or from its queue's KV source, below. Omitted when it has none. |

A `CRequest` is a hold's header read for a request: `pools: [(pool,
units, reserve?)]`, `reuse?`, `prefix?` (read at `Request`), `cache?:
(pool, ℓ)` and `send?`. Its `cache` names the engine's `memory`, a pool it
takes, where the request's position is: no other pool consumes or keeps a
prefix (16, #366). `send: {ttl?}` marks the engine a KV
relation's source: each request registers a send of its allocation of the
engine's `memory` (a pool it takes) when it ends, which keeps it, as a
hold's `lease` keeps one, until a reader's load ends the send, `ttl`
seconds (a constant above 0; absent, never) pass before a reader's copy
begins, or the session ends; a load whose send has ended before its copy
began loads nothing, and the request computes those tokens; `cache` is read when the request ends
and applied when the send does (the P/D prefiller's blocks, kept for the
decoder's read: vLLM's `delay_free_blocks`). An engine whose requests are
sent and that no connector's `source` reads is refused, since nothing
would read the send. The request waits in its first pool's queue. Its
`records` are the slots the engine writes: the observations `reused` and
`prefill` for each admission, and the attributes `admitted`, `first`,
`last_reused` and `last_prefill` (`E.reused`, `E.prefill`) the caller reads
after the call. A family engine's members each have their own `CStep`,
whose `CRequest` names the member's pools. A `CConnector` has `load?` and
`save?`, each a `CTransfer` of one shape: `amount`, read at `Request`
(after `Hit`) or at `Release` and stored in the attribute `loaded` or
`saved`; the copy, a run on `stage` (and `also`, held at once: a KV
relation's reader's NIC, or the engine's own NIC under `kvt.l2_link`) of
`work` after `latency?`, with `share?` and `poster?` as a `Run`'s; `into` and
`units`, where the KV lands (a `Load`); `room?: {held, grow, cache}`, a
save's room in an L2's pool, held first, grown by what the L2's entry
lacks, cached whole; and `source?`, a KV relation's read: the source's
memory, its member the attribute `from?` (which the request's entry sets;
-1: no source, nothing loaded), whose registered send the load waits for
and ends. The engine runs a transfer as its own action while the
request's status is `waiting_for_kv` (a load) or `finished` (a save);
`Program::kernel` writes the statements it is (`Run; Load`, or `Hold {
Grow; Run; Load }`) for the interpreter and the figure, which the IR does
not hold. One status per request (`docs/design/request-state.md`). `record` is
the observation of what each admission loaded, `load_done` the attribute
of when its latest load ended. After a load the interpreter computes the
request's last token again if every one has KV, and reads the `CRequest`
again: the pools it took none of and takes now, it waits for, as a part of
its hold. The blocks are roots of the block tree, not bodies of a
statement; a load's acts on the request's hold on `memory`, a save's
inside every pool the request takes.

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
An engine whose `advance running` and `admit waiting` share `only (p)`
lowers to `[Serve {only: p}, Admit {only: p, gate: !preempted}]`.

### Registers and claims

A register's `stage` must have an iteration body and cannot be an array
member; its initial value is finite. `CExpr::Reg(index)` reads it only in
its stage's expressions and body, iteration claims, queue keys of pools
that stage admits, hold headers whose first pool is such a pool, gauges,
or `AtEnd` claims. Sets are undone when an attempt schedules, preempts and
admits nothing. See [registers](api/engine.md#registers).

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
| `Turn` | draw the next turn's attributes (workload `turn` block or trace); the source `turn;` also expands the server statements immediately after this node |
| `Set(slot, e)`, `Observe(k, e)` | assign an attribute, record an observation |
| `Hold {pools: [(pool, units, reserve?)], reuse?, body, cache?: (pool, ℓ), prefix?, lease?}` | acquire units of every pool (admission gate `reserve` if given), run `body`, release; `cache` names one of `pools` (16, #366), on which alone the admission reads the cached prefix of the hold's chain and keeps it allocated while the hold lasts, `reuse` bounds how much, and `ℓ` is the units of it left cached along the chain, at most what that pool computed (its allocation, or the position a growing run or a `Load` reached on it); the other pools, and every pool of a hold without `cache`, leave the cache where it is; `prefix: [(key, length)]` is the chain, keyed segments shared with every hold whose leading keys are equal (a key is equal when it is the same `Attr` slot, or a structurally equal expression, with the same value, -0 = 0; a segment of length 0 is left out), absent `[(serial, ∞)]`, the session's own; a prefix several holds read is allocated once; with `prefix` the read is at most the allocation, without it the session's entry is consumed whole; what the session's entry held past a `reuse` bound stays cached, dead; on a pool without `block` the chain is the session's own; `lease: (pool, t)` keeps that pool's allocation past the scope, neither evictable nor a preemption victim except under the pool's `Leases`, until the session's `Release` of it, `t` seconds, or its end (vLLM's `delay_free_blocks`) |
| `Grow(pool, e)`, `Shrink(pool, e)`, `Drop(pool)` | grow, or give `e` back from, the current hold, or outside every hold of the pool the session's lease of it (a hold keeps what it computed); drop the session's own cached entry |
| `Release(pool)` | give the innermost enclosing hold's allocation on the pool back now, or end the session's lease of it, caching per the hold's `cache`; nothing held or leased there is a no-op. A KV transfer between instances is `Run` (the link), `Load` (the destination) and `Release` (the source's lease) |
| `Load(pool, e)` | the KV of `e` tokens arrived from outside the engine (a NIXL read): the innermost enclosing hold's computed position on the pool advances by `e`, within its allocation |
| `Run {stage, mode, work, growing?, also?, share?, latency?, poster?}` | work at a stage; `mode` `Plain`, `Prefill`, `Decode` (step stages); `growing` the pool that grows with the tokens computed, whose innermost hold's position, when the run ends, is its position at the run's start plus `work`; `also` further stages the same job holds at once, a flow, omitted when empty; `share` `MaxMin` or `Bottleneck`, how the flows of its fabric divide the stages' capacity, present exactly when `also` is not empty; `latency` seconds the job waits at its stages before its work starts, present there and taking no capacity (posting a NIXL READ and the step that polls its completion), read at the run's start like `work`, not on a step stage, omitted when none: the stage report counts it as the run's wait, and a run of zero work with one completes when it ends; `poster` which of its stages (`stage` 0, `also[k]` `k + 1`) posts the copy, set by a KV relation (`kvt.nixl_pull`: the reader's NIC; `kvt.nixl_push`: the source's), omitted when no side is named (an L2 connector's copy over `kvt.l2_link`, whose loads and saves go opposite ways): it says who starts the copy, for the figure, and the run is the same whichever it is |
| `While(guard, body)` | test `guard` before each pass (exactly 0 or 1); run `body` for 1, continue after the loop for 0; reevaluate after each completed body |
| `Branch(e, then, else)`, `Loop(body)`, `Choose {var, count, key}` (`key` a list, compared in order), `End` | control; `End` ends the session |
| `Fork(body)` | runs `body` beside the session as a leg of the turn: from now, with a copy of the attributes, its own holds and stream, the session's cached prefix; the send it leaves passes to the session when it ends. The body calls an engine whose requests are sent once on every path and leases nothing; the session waits for the leg by its reader's load of that send |
| `Request {stage, input, output}` | a request to step stage `stage`, `input` tokens of prompt and `output` to generate, both read at the call: the engine keeps its state (the tokens it has, its KV position on `memory`, what it has generated), admits it by its `CRequest` (none: as it is called), runs its connector's load, computes what of its tokens has no KV, samples a token when they all have it, decodes the rest, runs its connector's save, and gives its pools back; a preempted request keeps its tokens, `known = max(known, computed + 1)`, and waits again; the session goes on when the request has generated `output` |

### Types and statement authority

`Size` slots are workload-owned and read-only to server statements. `Value`
slots are server bookkeeping; workload assignment targets must be `Size`
or a permitted client `Cost`. Built-in trace/client slots are `Size`;
`cached` and `computed` are `Value`. Explicit sessions may preset only sizes.
Server control-flow bodies retain server authority. Inlined server statements
inside a workload block retain `Server` in `sides`; there is no new runtime
request frame or attribute reset. A request is the run of `Server`
statements after a `Turn`, up to the next `Workload` statement: where the
interpreter's client turn ends, and what the deployment view draws.

`CostTarget` is `Pool {base, count}`, `Stage {base, count}`, or
`Joint([targets])`. A joint target has at least two distinct, sorted,
non-nested targets. A cost's resource span must match its consumer: a run's
stage(s), a hold's individual pool, the pool its `cache` names for its
`cache` and `reuse`, or all held pools for a `reuse` without `cache`. `Grow` and `Load` require their pool cost. A lease duration
remains an ordinary scalar in seconds. No raw number or size implicitly
becomes a cost in these statements.

`CExpr::Cost(target, expression)` explicitly interprets an ordinary quantity
as the resource cost; the expression must not already have a cost type.
Evaluation returns the inner value at the same moment and with the same
random stream. Numeric execution is unchanged. Aliases and allowed scalar
arithmetic preserve costs; incompatible cost operands are invalid.
See [attribute types](api/attributes.md#sizes-values-and-costs).

`init` and `turn` neither construct nor read costs. A workload session cannot
construct or read costs of pools acquired, grown or loaded, or stages run by server
statements. It can use client resources. Resource declarations, gauges and
claims use ordinary quantities rather than session costs. Every cost
attribute read must follow an assignment on all paths; `cache` cost
attributes must be ready at hold entry, including for early release paths.

### Expressions (`CExpr`)

`Cost(target, expression)`, `Num`, `Attr(slot)`, `Ctx(var)` (`Now`, `Waited`, `Size`, `Age`, `Last`, `Queued`,
`N`, `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Decoding`,
`Admission`, `Remaining`, `Position`, `Admitted`, `Preempted`, `Demand`,
`Served`, `Arrived`, and a request's `Input`, `Output`, `Known`, `Hit`,
`Finished`, `Loaded`, `Requeued`: availability is defined by the moments below), `Sample(dist, args)`,
`Reg(register)` (a stage register by index), `Call(fun, args)` (arithmetic functions, pool and stage queries such as
`CachedIn(pool)`, `Allocated(pool)`, `BudgetLeft(stage)`), `Unary`, `Binary`, `Cond`, and
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
- **Types.** The mandatory type and side tables obey the contract above;
  conversions cannot hide an invalid inner expression or evaluation moment.
- **Evaluation.** Expressions read only what their moment supplies.
  Hold headers (`units`, `reserve`, `reuse`) cannot draw. A `prefix`
  segment's key and length read no state, no clock, no draw, and not
  `cached` or `computed`: they are read while the hold waits and when it
  is admitted, and the trie they index must not move under them. A hold
  with `prefix` has a segment, only its last one endless, takes at least
  one pool with `block`, and none that spills
  or is spilled to. No statement sets `serial`, the session's number:
  a session's cache entry is what hangs under the key of its serial.
  `BudgetLeft` references only step stages, including every member of an
  indexed family.
- **Ownership.** `Grow`, `Shrink`, `Load`, `Release` and a run's `growing` require an
  enclosing hold with the same `CRef` (base, count and index expression);
  `Grow`, `Shrink` and `Release` may instead act on a lease of it. A hold leases only its own pool, names
  each reference once, and cannot change attributes its index reads.
  An index read again inside the hold cannot depend on state or time.
  Preemptible holds cannot use such indices or `cached`/`computed` in
  their pool indices.
- **Parallel legs.** A `Fork` body contains no `Turn`, `End`, `Fork` or
  hold with a `lease`, and cannot act on its parent's holds. A fork cannot
  sit inside a hold whose pool may preempt. Its body calls, once on every
  path, an engine whose requests are sent (`CRequest.send`), and every
  path of the session after the fork reaches a `Request` whose connector's
  load reads that source before the session ends.
- **Progress and amounts.** Every `Loop` or `While` body path reaches a `Run` with positive
  or nonconstant work, a hold whose body does so, or `End`.
  Constant work and resource amounts cannot be negative or NaN. A
  constant hold demand, rounded to blocks, must fit at least one pool its
  reference can name. See [Every instant settles](language.md#every-instant-settles)
  for runtime checks of progress that static validation cannot establish.
- **Token counts.** A token count is a whole number (#661,
  [Whole token counts](design/whole-tokens.md)): a `Request`'s `input`
  and `output`; a `Load`; a `Run`'s `work` on a step stage or with
  `growing`; a step stage's `budget`, `chunk` and `granule`; a hold's or
  a `CRequest`'s `reuse`, `cache` and `prefix` lengths, its units on a KV
  pool, and a `Grow` or `Shrink` of one; a `CConnector`'s `load` and
  `save`; a KV pool's `cap` (`inf` too) and `block`. A *KV pool* is one
  whose units are positions of a token sequence: it has a `block`, is a
  step stage's `memory`, is the pool a hold's or a `CRequest`'s `cache`
  names, is taken by one with `reuse` or `prefix` and no `cache`, is a
  `Load`'s or a `growing` run's, or is spilled to by one. Every such expression must be provably whole: an integer
  constant or `inf`, `Floor`, `Ceil`, `Queue`, `Busy`, `Holders`,
  `Queued`, a comparison or a logical operator; `Add`, `Sub`, `Mul`,
  `Neg`, `Min`, `Max`, `Abs` and both branches of a `Cond` of whole
  numbers; `Bernoulli`, and `Det` of a whole number; `Used`, `Free`,
  `CachedIn`, `Allocated` of a KV pool; `BudgetLeft` of a stage whose
  budget is whole; every context variable but `Now`, `Waited`, `Age`,
  `Last` and `Attn`; an attribute every `Set` of which, and every
  explicit session's value of which, is whole (the greatest fixpoint),
  and the built-in ones but `think` and the times an engine records; a
  register whose `init` and every `Set` are whole. `Div`, `Pow`, the
  continuous draws and the other functions are not. A hold's `reserve`,
  a non-KV pool's units, seconds and stage work are not token counts.
  The trace's `new` and `out` columns are whole numbers when it is read.
  The error names the count, why a KV pool is one (the first of the
  reasons above, with its statement's place), and, for an attribute, the
  assignment that made it fractional, which is where the program writes
  its rounding. A message quotes a statement's place as `place(b, k)`,
  which `Invalid::placed` writes as the text's `line:col` (the linker) or
  as the block and statement; an error about an engine's clause or a
  pool's cap carries the declaration (`Invalid::decl`).
- **Arrivals.** Constant renewal gaps are positive; Poisson rates are
  positive and finite. `Closed` and `Batch` populations lie between 1 and
  `MAX_SESSIONS`.
- **Stages and flows.** A run uses `Prefill` or `Decode` exactly on step
  stages, and `growing` only there. A pool's spill `via` is not a step
  stage: its copy is a `Plain` job no session owns, and a step stage's
  iteration serves a request's prefill or decode. Every stage family
  named by a multi-stage run is a `Ps` with constant capacity; all runs on such a
  shared stage are `Plain`, without `growing`. A run names each family
  once, and has a `share` exactly when its `also` is nonempty. The stages
  runs join, however indirectly, are one fabric, and the runs of a fabric
  name one `share` (`Program::stage_shares`). Every stage is the `stage`
  or one of the `also` of some `Run`, the stage of some `Request`, or a
  pool's spill `via` (`Program::unreached_stage`): a stage nothing runs
  is reached by no request. It is checked last, so that a program wrong
  in another way is told that first.

`validate_located` identifies statement errors by block and statement
index; the text frontend maps these to source locations, and a stage
nothing runs to its declaration. Other declaration and flow checks may
have no location. `src/ir.rs` is the authoritative
set of validation rules, including restrictions on scheduling expressions
and registers described above.

### Moments

An expression is evaluated at one moment, fixed by
its position in the IR. Context variables may be read only at the listed
moments. Additional checks forbid `Now` in `Gauge`, `Given`, `only`
predicates and iteration-body guards, admission gates and assignments:

| Moment (`ir::Moment`) | Positions | Context variables |
|---|---|---|
| `Session` | statements of `init`, `turn`, `session`; a run's work; a hold's `cache` (read when the session releases); `Grow`, `Load`, `Branch`, `While`, `Choose` | `Now` |
| `Admit` | a hold's units, `reserve`, `reuse`, admission bindings | `Now` |
| `Select` | a pool's queue keys, reevaluated for each waiting hold before every admission attempt (under `queue_every`, before each at a tick) | `Waited`, `Now` |
| `Evict` | eviction keys, a spill's `work` and `when` | `Size`, `Age`, `Last`, `Queued`, `Now` |
| `Ps` | a `ps` stage's capacity, read for the stage's jobs (no `Attr`: no one session is read) | `N`, `Now` |
| `Budget` | a step stage's `budget` and `chunk`, evaluated before the iteration from its residents (no `Attr`; a `BudgetLeft` is rejected: it is planned from a budget) | `Nres`, `Ndec`, `Kvb`, `Kvp`, `Now` |
| `Step` | a step stage's `cost`, evaluated after the iteration is scheduled (no `Attr`) | `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Now` |
| `Serve` | a step stage's `serve by` keys and `only`, evaluated for one resident at its turn, the totals as the residents stand then (a session the iteration admitted included) | `Decoding`, `Admission`, `Remaining`, `Nres`, `Ndec`, `Kvb`, `Kvp`, `Now` (`only` not `Now`) |
| `Victim` | a pool's preemption keys, evaluated for each candidate when growth cannot fit, and at a pool's tick over its lessees and the leases it set aside | `Decoding`, `Admission`, `Position`, `Now` |
| `Plan` | iteration-body branch guards, admission gates and register assignments (no `Attr`) | `Nres`, `Ndec`, `Kvb`, `Kvp`, `Ntok`, `Npre`, `Admitted`, `Preempted` |
| `Gauge` | a gauge, evaluated on the state an instant ends with and held until the next, with no session (an `Attr`, a `Sample`, `CachedIn`, `Allocated`, `Now`, `Work` or `BudgetLeft` is rejected, and an index is a `Num` in range, so reading it cannot fail the run) | none |
| `Given` | a claim's `given`, evaluated for each session once its `init` has run (and its preset attributes are set), on its attributes (`hidden` ones included: a claim is no scheduler) and the constants; no `Sample`, no `Call` but arithmetic | none (not `Now`) |
| `Iteration` | a claim over the iterations of a step stage, evaluated when an iteration starts, after its batch is scheduled (where the cost is read); no `Attr`, no `Sample`, of the calls only arithmetic, `Queue`, `Busy`, `Used`, `Free`, `Holders`, `Queued`, an index a `Num` in range | `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Demand`, `Served`, `Arrived`, `Now` |
| `End` | a claim `at end`, evaluated once when the run ends, when no session is live; no `Attr`, no `Sample`, no `Call` but arithmetic; the only moment that reads `Agg` | `Now` |
| `Request` | a `CRequest`'s units, `reserve`, `reuse` and `prefix`, and a `CConnector`'s load, read for the request the scheduler admits; no draw | `Input`, `Output`, `Known`, `Hit`, `Loaded`, `Requeued`, `Now` |
| `Release` | a `CRequest`'s `cache` and a `CConnector`'s save, read when the request gives its pools back (`Finished` 0 at a preemption, 1 at its end) | `Input`, `Output`, `Known`, `Finished`, `Now` |

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
statements `Turn`, `Hold`, `Run`, `Set`, `Observe`, `Branch`, `Loop`, `While`, `End`
(not `Release`, `Load`, `Fork`, a hold with a `lease` or an engine whose requests are sent)
with expressions built from integer constants, attributes, `Now`,
`CachedIn`, `BudgetLeft`, `min`, `max`, `+`, `-` (truncated at 0), `*`,
`floor(a / b)`, `ceil(a / k)` with a constant `k`, comparisons, `!`, `&&`,
`||` and conditionals; an expression of constants alone is folded first,
as the interpreter computes it. In a cost, a context
variable with a zero coefficient (the replay's cost at `a = b = 0`) is
dropped. Its generator translates an IR file into Lean and
fails on anything outside the fragment. Lean uses a nonzero test for
`Branch` and `While`; Rust reports an error for a guard other than 0 or 1.
The `While` translator therefore requires a guaranteed 0/1 range:
boolean literals, comparisons, logical expressions, conditionals with boolean
outcomes, or read-only trace `more` with boolean session and turn presets.
Other guards are outside the fragment, including mutable attribute guards.
A `While` guard and the transitive assignments of attributes it reads may
not contain subtraction. A 0/1 result alone does not ensure that a comparison
agrees when natural subtraction truncates a negative intermediate value.
Every division in the guard or those assignments must have a divisor that
folds to a positive natural constant. This excludes dynamic divisors, even
ones that stay positive: Lean division by zero returns zero, unlike Rust's
infinity or NaN. These checks inspect expressions before constant folding.
This preserves the prefix-cache oracle without translating an invalid guard
into a successful Lean execution. For `Branch`, correspondence remains
restricted to executions with valid 0/1 guards.

The claims generator additionally accepts the body an engine's shared `only` lowers to,
one queue key on a pool that no run grows, and supported tiled costs. A
claim read at the end aggregates with `Total`, `Count` or `PrefixTotal`:
`Largest` and `Smallest` are outside the fragment. See
[the Lean guide](lean.md#what-is-not-covered-yet) for these restrictions.
Arbitrary iteration bodies, registers, `reserve_held`, `granule` and
preemption policies other than LIFO are not translated.

### Source entry points and inputs

The `.sq` frontend requires `fn main()` for an executable source program,
the deployment, and a workload file given beside it (`--workload`), which
the tool pairs with it into one program ([paired files](language.md#paired-files)).
`std/args` resolves declared inputs before constructing this IR. Neither the
entry-point wrapper nor the input library adds a runtime node or field;
plain constants remain folded, and an IR program never reads host arguments.
This source-language change does not change `IR_VERSION` or IR meaning.

### Explicit source declarations

The `.sq` frontend accepts explicit `Size` and `Cost` declarations.
A composite Cost is lowered to separate scalar attributes, one per named
resource, and existing `CExpr::Cost` conversions. Source declarations are
checked against the inferred slot types and erased; they add no IR node
or execution scope. Serialized programs retain the same mandatory
`attr_types` and `sides` validation contract.
