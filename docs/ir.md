# The serQ IR

The IR is the definition of a serQ program. Everything else is built around
it:

```
  program text (.sq)  ──parse + link──▶  IR (ir::Program, JSON)  ──▶  interpreter (interp)
  tools (Rust / JSON) ──────build/edit──▶                          ──▶  Lean model (generated)
                                                                   ──▶  checks, diffs, archives
```

The text syntax (`docs/language.md`) is one frontend. It exists because a
deployment is easier to read and write as text, not because the text is
the program. A tool that knows what it wants to run (a scenario from JSON,
a parameter sweep, a trace replay) builds or edits the IR as data instead
of generating text.

Source: `src/ir.rs`. Version: `IR_VERSION = 11` (2 added the sessions' turns;
3 renamed the `route` field to `session`; 4 replaced `CStep`'s two booleans
`exclusive_prefill` and `decode_first` by the one order `serve`;
5 added KV transfer and leases; 6 added renewal arrivals and finite open runs;
7 makes `Choose.key` a list of keys; 8 lets a `Run` hold several stages at
once, `also`, under the program's `share`; 9 makes queue keys a list,
reevaluates them at selection and supplies `Waited`; 10 makes `Hold.cache`
the clause that admits a hold to the prefix cache; 11 gives every session
its own random streams, so a `seed` names a workload, not a sequence of
draws).

## Why an IR first

- **One program, several consumers.** The interpreter runs the IR, the Lean
  model (`lean/`) is generated from the IR, and the vLLM
  oracle tests run the IR. Before the IR existed, the vLLM request program
  had three hand-kept copies: the Rust test built it as a string per
  scenario, the Lean generator held a hand-written Lean version, and
  `examples/multi-turn/vllm.sq` was a third variant. Now there is one file,
  `examples/oracle/vllm_request.sq`, compiled once per scenario into
  `tools/oracle/<name>.ir.json`, and both the Rust test and the Lean
  theorems read those files. The multi-turn cache scenario is the IR of
  `examples/replay/vllm_replay.sq` with its trace inlined
  (`tools/oracle/cache_trace.ir.json`), so its Lean program is generated
  too.
- **The workload instance is data.** Which sessions arrive with which
  attributes, and which turns each one replays, is part of the IR
  (`CArrival::Sessions`), not of the program text or a separate trace
  file. A scenario's requests are no longer encoded as nested conditionals
  on `serial`, and `serq ir --inline-trace` turns a trace file into
  the sessions' turns.
- **Closed and checkable.** The IR has no closures and no host-language
  code: every expression is a tree over a fixed set of operators, context
  variables and functions. That is what makes a formal semantics possible,
  and `Program::validate` can check any IR, however it was produced.

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
| `pools` | `CPool`: `name`, `index` (the member's index in an array declared `pool kv[N]`, a one-member array's too, and a queue family's, `queue D[1]` included; omitted for a single pool; a report label the run does not read), `cap` (units), `block` (allocation granularity), `evict` (`Lru` or `By([key exprs])`), `preempt` (`None`, `Lifo`), `queue` (`null` for FIFO, otherwise a nonempty list of pure selection keys), `spill`, `admit_via` (stage whose scheduler admits waiting holders) |
| `stages` | `CStage`: `name`, `index` (as for `CPool`, `stage E[N]`), `kind`: `Fifo(servers)`, `Ps(capacity expr)`, `Delay`, `Step(CStep)` with `budget`, `cost`, `chunk`, `serve` (how the iteration serves its residents, said once: an order, `By([key exprs])` (keys at the `Serve` moment, ties in admission order; no keys is admission order, `serve admission`; `decode first` is `By([decoding ? 0 : 1])`; a key may not draw), or the rule `ExclusivePrefill`, which is not an order and so cannot be combined with one), `only` (absent, or a `Serve`-moment predicate that may not draw or read `Now` or `work`: the residents it reads as 0 are not served that iteration, and `serve` orders the rest; not with `ExclusivePrefill`), `memory` (pool index) |
| `arrival` | `Poisson(rate)`, `Renewal(gap expression)`, `Closed(n)`, `Batch(n)`, `Sessions([{attrs: [[slot, value], …]}])`, `None` |
| `trace`, `trace_ordered` | a trace corpus the workload draws turns from (path, resolved against the program's directory unless overridden) |
| `init`, `turn`, `session` | block indices: the workload's `init` and `turn` blocks and the session program |
| `blocks` | the statement blocks (an arena; bodies of holds, branches and loops refer to blocks by index) |
| `horizon`, `warmup`, `seed`, `arrivals` | the run; `arrivals` requires exactly N open arrivals and draining by `horizon` |
| `hidden` | attribute slots the scheduler may not read (`hidden o;`): legal at the `Session` moment only, below |
| `share` | `MaxMin` or `Bottleneck`: how the flows of runs over several stages divide the stages' capacity; present exactly when some `Run` has a non-empty `also`, omitted otherwise |
| `gauges` | `[{name, expr}]`: functions of the state whose time average the report gives, each read at the `Gauge` moment after every instant; omitted when empty. They read and do not act, so a reader that ignores them runs the same program |
| `claims` | `[{name, given?, kind, expr}]`: propositions about every path, which the interpreter checks on the path it runs and the report states; omitted when empty. `kind` is `EveryIteration(stage)` or `SomeIteration(stage)` (a step stage's index; `expr` read at the `Iteration` moment) or `AtEnd` (`expr` read at the `End` moment); `given`, omitted when absent, is read at the `Given` moment for every session, and one that reads 0 puts the claim out of the run's scope. Claim names are distinct. They read and do not act, so a reader that ignores them runs the same program |
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

### Statements (`CStmt`)

| Statement | Meaning |
|---|---|
| `Turn` | draw the next turn's attributes (workload `turn` block or trace) |
| `Set(slot, e)`, `Observe(k, e)` | assign an attribute, record an observation |
| `Hold {pools: [(pool, units, reserve?)], reuse?, body, cache?, lease?}` | acquire units of every pool (admission gate `reserve` if given), run `body`, release; with `cache` the admission consumes the own cached prefix, `reuse` bounds how much, and `cache` is the units left cached; without `cache` the hold leaves the session's cached prefix where it is (10); `lease: (pool, t)` keeps that pool's allocation past the scope, neither evictable nor a preemption victim, until the session's `Release` of it, `t` seconds, or its end (vLLM's `delay_free_blocks`) |
| `Grow(pool, e)`, `Drop(pool)` | grow the current hold, drop the own cached entry |
| `Release(pool)` | give the innermost enclosing hold's allocation on the pool back now, or end the session's lease of it, caching per the hold's `cache`; nothing held or leased there is a no-op. A KV transfer between instances is `Run` (the link), `Load` (the destination) and `Release` (the source's lease) |
| `Load(pool, e)` | the KV of `e` tokens arrived from outside the engine (a NIXL read): the innermost enclosing hold's computed position on the pool advances by `e`, within its allocation |
| `Run {stage, mode, work, growing?, also?}` | work at a stage; `mode` `Plain`, `Prefill`, `Decode` (step stages); `growing` the pool that grows with the tokens computed; `also` further stages the same job holds at once (a flow of `share`), omitted when empty |
| `Branch(e, then, else)`, `Loop(body)`, `Choose {var, count, key}` (`key` a list, compared in order), `End` | control; `End` ends the session |

### Expressions (`CExpr`)

`Num`, `Attr(slot)`, `Ctx(var)` (`Now`, `Waited`, `Size`, `Age`, `Last`, `Queued`,
`N`, `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Decoding`,
`Admission`, `Remaining`, `Demand`, `Served`, `Arrived`: each exists at
one *moment*, below, and `Now` at every one), `Sample(dist, args)`,
`Call(fun, args)` (arithmetic functions, pool and stage queries such as
`CachedIn(pool)`, `BudgetLeft(stage)`), `Unary`, `Binary`, `Cond`, and
`Agg(agg, observation)`: `Total`, `Count`, `Largest`, `Smallest` (0 when
none) or `PrefixTotal` (`Σ_k Σ_{i≤k} v_i` over the values sorted
ascending) of every value the run observed under the observation, warm-up
included; read at the `End` moment only. Pool
and stage references are `CRef {base, count, index?}` (a family of
`count` pools from `base`, selected by `index`).

## Validation

`Program::validate` checks the version, that every block, attribute,
observation, pool and stage index exists, the run parameters, that every
call has its function's arguments (`Fun::signature`) and every draw its
distribution's (`DistKind::arity`), that `init` and `turn` only `Set`
and `Observe`, that a `Run`'s mode is `Prefill` or `Decode` exactly on a
step stage and its `growing` only there, that `Grow`, `Load`, `Release`
and `growing` stand inside a `Hold` of an equal `CRef` (base, count and
index expression; `Release` also where a hold leases it), and a hold leases
one of its own pools, that a hold's body changes no attribute its index
reads and an index read again inside reads no state or clock, that a hold a
pool may preempt reads no such index nor `cached`/`computed` (it is admitted
anew), that the blocks reached from `init`, `turn` and `session` form a
tree, that
every context variable is read at the moment that supplies it, that every
`Loop` lets time pass on every path through its body (a `Run` of
non-constant or positive work, a `Hold` whose body does, or `End`;
`docs/language.md` §3, Every instant settles), that a `Hold`'s units,
`reserve` and `reuse` do not draw (they are re-read at every admission
attempt; the rule of a queue key), that a `Hold` takes each pool reference
once, that a `Hold` whose units or `reserve`
are a constant, rounded to blocks, fits some pool its reference may name
(one that fits none is rejected whenever it is reached), that a `BudgetLeft` names step stages
only (every member of the array it references), that a constant renewal
gap is a positive time (the linker folds one) and a poisson rate a positive, finite
number, that a `Closed` or `Batch` workload starts 1 to `MAX_SESSIONS` sessions, that a constant amount (the work of a `Run`, the units of a `Hold`, a
`Grow` or a `Load`) is not negative or NaN, and the
flows: every stage array a `Run` holds with another (`also`) is a `ps` of
a constant capacity, every run on such a *shared* stage is `Plain` with no
`growing`, a run names each stage array once, and `share` is present
exactly when some `also` is non-empty (`Program::shared_stages`).
`Program::from_json`, `run_ir` and the linker (`compile_source`) call it,
so a text program meets the same check as IR from files and tools. An
error from the checks of a block's statements says which statement
(`Program::validate_located`: block and index), and the text frontend shows
it at that statement's line; the flow and declaration checks have no
statement to name.

**Moments.** An expression is evaluated at one moment, fixed by
its position in the IR, and a context variable exists at one of them:

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
| `Gauge` | a gauge, evaluated on the state an instant ends with and held until the next, with no session (an `Attr`, a `Sample`, `CachedIn`, `Now`, `Work` or `BudgetLeft` is rejected, and an index is a `Num` in range, so reading it cannot fail the run) | none |
| `Given` | a claim's `given`, evaluated for each session once its `init` has run (and its preset attributes are set), on its attributes (`hidden` ones included: a claim is no scheduler) and the constants; no `Sample`, no `Call` but arithmetic | none (not `Now`) |
| `Iteration` | a claim over the iterations of a step stage, evaluated when an iteration starts, after its batch is scheduled (where the cost is read); no `Attr`, no `Sample`, of the calls only arithmetic, `Queue`, `Busy`, `Used`, `Free`, `Holders`, `Queued`, an index a `Num` in range | `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Demand`, `Served`, `Now` |
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

An attribute listed in `hidden` may be read at `Session` only; at every
other moment it is what the scheduler would be peeking at. The field is a
check, not a semantics: a program that validates runs the same with or
without it, so the Lean generator does not read it.

Before this check the variable read as 0 anywhere else and the program ran
(`age` in a session statement, `tokens` in a queue key); the doc comment said
"meaningful only where the semantics supplies them", which is what an
undefined behaviour is. The check is stricter validation of IR whose types
did not change, so it is not a version bump; an IR file that used to pass
and now fails was reading a value the semantics never supplied.

## Ties

The IR carries no field for what breaks a tie: every ordered collection
has one rule, stated in `docs/language.md` §3 (Ties), so a `tie` field
(the IR v4 RFC's `COrder`) would carry no information. A Lean model of a
collection uses the same event number (`lean/Serq/Exec.lean`'s `lru`:
release order).

## Stability

`IR_VERSION` identifies what a reader must understand, not the shape of the
file. `scripts/gen_lean_oracle.py` pins it and reads the IR by field name,
so a bump moves the generator and `lean/Serq/Oracle.lean` in the same change
(`make lean`), and every other reader of the IR with it. What a change to `src/ir.rs` does to the version:

- **Removed, renamed or retyped field or variant: bump.** An old reader
  fails loudly. `route` became `session` in 3, and the generator's
  `ir["route"]` moved with it; under 4, `serve` went from the variant
  `Admission` to `By([])` (#50) and the generator's `Fragment` check caught
  it.
- **Old files still parse, different meaning: bump, and say so in the
  release note.** An old reader parses the file and disagrees with the new
  one about what it means; the version is the only warning it gets. The
  signal in a diff is a change to `docs/language.md` §3 or `src/engine/interp.rs`
  that alters what an existing statement or field does while `src/ir.rs`
  keeps the field or node. The interpreter setting `computed` on a
  preemption while the Lean fragment does not (`docs/language.md` §3,
  `computed`) is the standing example of the gap this line closes.
- **Added field: no bump when an old reader still reads every file right.**
  The test is not whether the field has a default but whether it carries
  meaning: a check (`hidden`), a label or a legend a reader may ignore does
  not bump; a field that changes what the program does bumps as a change of
  meaning, whatever its serde attributes — `turns` in 2 (the sessions a
  program runs) had a default and was omitted when empty, and bumped. A
  field without a default also makes a new reader reject old files
  (`missing field`), so the committed IR files are regenerated in the same
  change; the generator reads by name and ignores what it does not know.
  `gauges` does not bump: it changes what the report says, not what the
  program does, and a reader that drops it (the Lean generator) runs the
  same sessions to the same end, since a gauge reads no draw, plans no
  iteration and names its pools and stages by number. `claims` does not
  bump for the same reason: a claim reads no draw, plans no iteration, names
  its pools and stages by number and acts on nothing, so a reader that drops
  it runs the same sessions to the same end. It adds the variants
  `CExpr::Agg`, `CtxVar::Demand` and `CtxVar::Served`, which only a claim
  reads; IR 11 has no tag, so they go in the coming tag's message.
- **Same shape, a stricter check: no bump.** An IR file that validated before
  and is rejected now was reading a context variable at a moment that never
  supplied it (Moments, above), or a new file lists in `hidden` an attribute
  the scheduler reads, or has a `Loop` that can pass without letting time
  pass: the check got stricter, the format did not change. The same goes
  for a run-time error the interpreter did not raise before (an instant
  that does not settle, an iteration with tokens at cost 0): the programs
  it now refuses ran forever or stopped the clock.
- **The same `seed`, a different run: bump.** Which stream a draw reads
  and how the streams are seeded decides what a `seed` names. Under 10 it
  named one sequence of draws that every session consumed in execution
  order; under 11 it names a workload, one stream per (session, turn), so
  the same IR file with the same `seed` prints different numbers (and two
  machines under one seed see the same marks). An old reader parses the
  file and disagrees with the new one about what the run is: the second
  line above, and the version is the warning.
- Committed IR files (`tools/oracle/*.ir.json`, incl. `cache_trace.ir.json`) are regenerated by
  `make oracle-ir` and checked by `make check`
  (`tests/vllm_oracle.rs::oracle_ir_files_are_current`).
- `tests/ir.rs`: every example program survives a JSON round trip exactly
  and runs to the same report from IR as from text.

A version is a release, and the lines above decide one thing: whether a
change to a *tagged* version opens the next number. While the version at
`IR_VERSION` has no tag (11 has none yet; 10 in `v0.1.2` and `v0.1.1`, 9 in `v0.1.0` and `v0.1.0-rc7`, 8 in `v0.1.0-rc6`, 7 in `v0.1.0-rc5`, 6 in `v0.1.0-rc4`,
5 in `v0.1.0-rc1`;
`v0.1.0-rc0` is 3), no line bumps; the
change is listed in the coming tag's message, which is the release note,
and the handshake happens once, at the tag. A reader on an untagged version
reads a commit, not a version: under 4, `serve` moved twice and the
generator followed twice (#45, #50), while `slot_computed` and `hidden`
joined without a reader noticing.

Version 5 carries `Release`, `Load` and `Hold.lease` for KV transfer and
allocations that outlive their hold scope. The oracle programs use none of
these mechanisms; their regenerated IR has the new version and a `null`
`lease`. The generator pins 5 in the matching `serving-queue-theory` change.

Version 6 adds `Renewal(CExpr)` and `Program.arrivals`. A renewal gap may
be constant or sampled, uses only constants and distributions, and the
first arrival follows one gap. `Poisson(rate)` retains its initial arrival
at zero. Finite runs must generate the requested arrivals and drain by the
horizon; completion before or at warmup is an error. Both additions are
outside the Lean fragment (which accepts explicit sessions); the matching
generator pins 6 and rejects an arrival limit instead of ignoring it.

Version 7 makes `Choose.key` a list, compared in order, ties to the smallest
index (#140): a router that prefers a cache-warm pod and then the least
loaded one says so, where version 6 packed the two into one number
(`warm * 1e9 + load`), which is wrong once the load exceeds the multiplier.
One key is a list of one. `Choose` is outside the Lean fragment, so the
matching generator only moves its pin to 7.

Version 8 also strengthens `CServe::ExclusivePrefill` from resident-only
isolation to a whole-batch constraint: a prefill runs alone, including a
waiting prefill that displaces tentative resident decodes. Cancelled work
does not advance computed KV. This landed while 8 was untagged, so the
number stayed 8 under the policy above, and it is in `v0.1.0-rc6`'s release
note; `v0.1.0-rc6` carries 8, so the next change to the IR opens 9. JSON shape and the ordinary serving
order are unchanged. `ExclusivePrefill` is outside the Lean fragment;
this change neither extends it nor changes the oracle IR files.

Version 9 replaces cached enqueue keys with selection-time evaluation and
changes `CPool.queue` from `Option<CExpr>` to `Option<Vec<CExpr>>`. `None`
remains FIFO; a nonempty list is compared lexicographically with enqueue-order
ties. `CtxVar::Waited` exists at the new `Select` moment. Keys may not draw,
and scheduler-hidden attributes remain rejected. Existing time-dependent
queue policies change meaning; static one-key policies keep their order.
Resumed holds keep prepend priority and the selected non-fitting request
still blocks the queue. The current interpreter rejects older IR versions;
regenerate JSON from source. `v0.1.0-rc6` carried 8, so this
meaning/shape change opened 9, and it is in `v0.1.0-rc7`'s release note;
`v0.1.0-rc7` and `v0.1.0` carry 9, so the next change to the IR opens 10.

The companion `serving-queue-theory/scripts/gen_serq_oracle.py` reads version 9
for its FIFO fragment and rejects non-FIFO queues. It also retains support
for its pinned version 7 and version 8 FIFO programs. No waiting-selection
proof is claimed. Oracle JSON files move to version 9; their schedules and
generated Lean programs remain unchanged.

Version 10 makes `Hold.cache` the clause that admits a hold to the prefix
cache (#230): a hold without it consumes nothing of the session's own
entry and sets `cached` to 0, where 9 consumed the entry at every admission,
so a hold admitted inside another on the same pool found none. A hold written
around the request's on the same pool (a reservation given back before the
request is admitted) thus no longer costs the request its hit. A hold
without `cache` on a pool where the session has an entry no longer drops
it, whether or not its body reads `cached`; `cache (0)` is the 9 meaning.
The shape is unchanged; the meaning of an absent `cache` is, on a tagged
version, so this opened 10, which `v0.1.1` carries; the next change to the
IR opens 11. No oracle program holds a pool with entries
without `cache`, so the oracle schedules and the trace are unchanged; the
files carry the new version. The Lean fragment's `admit`
(`SerqExec.lean`) consumes unconditionally and moves with the generator's
pin in the matching `serving-queue-theory` change. The linker rejects a
`cached` read, or a `reuse`, in a hold without `cache` (a stricter check,
which catches the body that would read a different number, not every
program whose numbers move).

Version 11 gives every session its own random streams
(`docs/language.md` §3, Workload; `docs/design/stochastic-model.md`,
Proposition 1): a session's `init` and `turn` blocks read a stream seeded
from (seed, serial, turn), its statements one seeded from (seed, serial),
and a draw the machine makes (a `cost`, a `budget`, an eviction key) the
interpreter's. Under 10 every session's `init`, `turn` and statements
consumed two shared sequences in execution order, so a change to the
machine that reordered the sessions' draws changed the marks they drew
(on `examples/multi-turn/vllm.sq` at seed 1, 967 of 4 733 (session, turn)
pairs between two budgets). Under 11 a (session, turn) draws the same
marks under every machine run on one seed: a `seed` names a workload. The
shape is unchanged; the meaning of `seed` is, on a tagged version, so this
opened 11; the next change to the IR opens 12. Every number a seeded run
prints moves; the tests compare against closed forms, confidence
intervals and traces and were unchanged, and the oracle programs draw
nothing, so their schedules and the trace are unchanged and the files carry
the new version. The generator's pin moves to 11, and the companion
`serving-queue-theory` moves its pin and re-records its serQ-derived
seeded numbers when it takes the release. 11 also carries three stricter
checks that would not have opened a version on their own: the linker
refuses a `Loop` that can pass without letting time pass, and the run time
refuses an instant that does not settle and an iteration with tokens at
cost 0 (`docs/language.md` §3, Every instant settles).

11 also carries `CStep.only` (#261, `serve only (p)`), added while 11 is
untagged: a program without it serialises as before and runs as before, and
one with it changes what the iteration serves, so on a tagged version it
would have opened a number; it goes in the coming tag's message instead. An
older reader that ignored the field would run every resident and print a
different schedule, which is why the line in the release note says so. The
Lean generator raises `Fragment` on a stage with `only`; no oracle program
has one, so the oracle IR files and the Lean fragment are unchanged.
With it, a serve key reads the totals (`Nres`, `Ndec`, `Kvb`, `Kvp`) as the
residents stand when it is read, where it read them as they stood before
the iteration: a session admitted in the iteration is counted. That is a
change of meaning under the same shape, listed in the same message; no
committed program has a key that reads a total, and none of their numbers
moved.
11 also carries the preempted hold's cache (#326): a hold released by a
preemption caches its position (`computed`), not its allocation, where it
cached its allocation when no `growing` run or `load` had advanced it. Same
shape, another meaning, in the same message; the oracle scenarios, the
trace and the regressions never preempt such a hold, and none of their
numbers moved.

## The Lean fragment

The Lean model (`lean/Serq/Exec.lean`, [docs/lean.md](lean.md)) runs a fragment of
the IR over natural numbers, event by event as the interpreter does: pools
with LRU eviction and LIFO preemption, one step engine (stage 0) whose
iteration cost is affine in `tokens`, `prefilled` and `decoders` with
natural coefficients and a constant term of at least 1 (`cost 1` is the
step clock), delay stages, explicit sessions with preset attributes and turns, and the
statements `Turn`, `Hold`, `Run`, `Set`, `Observe`, `Branch`, `Loop`, `End`
(not `Release`, `Load` or a hold with a `lease`: a program with a KV
transfer is outside the fragment until `Exec` gives a hold's pool
its own release)
with expressions built from integer constants, attributes, `Now`,
`CachedIn`, `BudgetLeft`, `min`, `max`, `+`, `-` (truncated at 0), `*`,
`floor(a / b)`, comparisons and conditionals. In a cost, a context
variable with a zero coefficient (the replay's cost at `a = b = 0`) is
dropped. Its generator translates an IR file into Lean and
fails on anything outside the fragment. The Lean `Branch` takes the first
block when the guard is non-zero; the interpreter admits only 0 or 1
(`docs/language.md`, Branching), so the two agree on every program that
runs.
