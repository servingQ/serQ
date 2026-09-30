# The seQ IR

The IR is the definition of a seQ program. Everything else is built around
it:

```
  program text (.seq)  ──parse + link──▶  IR (ir::Program, JSON)  ──▶  interpreter (interp)
  tools (Rust / JSON) ──────build/edit──▶                          ──▶  Lean model (generated)
                                                                   ──▶  checks, diffs, archives
```

The text syntax (`docs/language.md`) is one frontend. It exists because a
deployment is easier to read and write as text, not because the text is
the program. A tool that knows what it wants to run (a scenario from JSON,
a parameter sweep, a trace replay) builds or edits the IR as data instead
of generating text.

Source: `src/ir.rs`. Version: `IR_VERSION = 8` (2 added the sessions' turns;
3 renamed the `route` field to `session`; 4 replaced `CStep`'s two booleans
`exclusive_prefill` and `decode_first` by the one order `serve`;
5 added KV transfer and leases; 6 added renewal arrivals and finite open runs;
7 makes `Choose.key` a list of keys; 8 lets a `Run` hold several stages at
once, `also`, under the program's `share`).

## Why an IR first

- **One program, several consumers.** The interpreter runs the IR, the Lean
  model of serving-queue-theory is generated from the IR, and the vLLM
  oracle tests run the IR. Before the IR existed, the vLLM request program
  had three hand-kept copies: the Rust test built it as a string per
  scenario, the Lean generator held a hand-written Lean version, and
  `examples/multi-turn/vllm.seq` was a third variant. Now there is one file,
  `examples/oracle/vllm_request.seq`, compiled once per scenario into
  `tools/oracle/<name>.ir.json`, and both the Rust test and the Lean
  theorems read those files. The multi-turn cache scenario is the IR of
  `examples/replay/vllm_replay.seq` with its trace inlined
  (`tools/oracle/cache_trace.ir.json`), so its Lean program is generated
  too.
- **The workload instance is data.** Which sessions arrive with which
  attributes, and which turns each one replays, is part of the IR
  (`CArrival::Sessions`), not of the program text or a separate trace
  file. A scenario's requests are no longer encoded as nested conditionals
  on `serial`, and `seq-lang ir --inline-trace` turns a trace file into
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
`"-inf"`, and a reader of `Num` or `cap` takes a number or that string. `seq-lang ir FILE` prints it;
`seq-lang run/check FILE.json` reads it.

### `Program`

| Field | Meaning |
|---|---|
| `version` | `IR_VERSION`; a different version is rejected |
| `attrs` | attribute names; an attribute is referenced by its index (slot) |
| `observes` | observation names, by index |
| `pools` | `CPool`: `name`, `cap` (units), `block` (allocation granularity), `evict` (`Lru` or `By([key exprs])`), `preempt` (`None`, `Lifo`), `queue` (order key), `spill`, `admit_via` (stage whose scheduler admits waiting holders) |
| `stages` | `CStage`: `name`, `kind`: `Fifo(servers)`, `Ps(capacity expr)`, `Delay`, `Step(CStep)` with `budget`, `cost`, `chunk`, `serve` (how the iteration serves its residents, said once: an order, `By([key exprs])` (keys at the `Serve` moment, ties in admission order; no keys is admission order, `serve admission`; `decode first` is `By([decoding ? 0 : 1])`; a key may not draw), or the rule `ExclusivePrefill`, which is not an order and so cannot be combined with one), `memory` (pool index) |
| `arrival` | `Poisson(rate)`, `Renewal(gap expression)`, `Closed(n)`, `Batch(n)`, `Sessions([{attrs: [[slot, value], …]}])`, `None` |
| `trace`, `trace_ordered` | a trace corpus the workload draws turns from (path, resolved against the program's directory unless overridden) |
| `init`, `turn`, `session` | block indices: the workload's `init` and `turn` blocks and the session program |
| `blocks` | the statement blocks (an arena; bodies of holds, branches and loops refer to blocks by index) |
| `horizon`, `warmup`, `seed`, `arrivals` | the run; `arrivals` requires exactly N open arrivals and draining by `horizon` |
| `hidden` | attribute slots the scheduler may not read (`hidden o;`): legal at the `Session` moment only, below |
| `share` | `MaxMin` or `Bottleneck`: how the flows of runs over several stages divide the stages' capacity; present exactly when some `Run` has a non-empty `also`, omitted otherwise |
| `slot_cached`, `slot_serial`, … | slots of the built-in attributes (`cached`, `serial`, `turn_no`, `new`, `out`, `think`, `more`, `forced`, `computed`) |

`Sessions`: all the sessions arrive at time 0; each one runs `init`, then
its preset `attrs` overwrite what `init` set. A session may carry `turns`
(each a list of `[slot, value]`): its `turn` statements then read them in
order instead of the trace corpus, with the corpus's rule (`turn_no` counts
turns, the turn's values are set, `more` is 1 while another turn remains
and 0 after the last). `Program::with_sessions` builds sessions from
attribute names; `Program::inline_trace` (CLI `seq-lang ir
--inline-trace`) replaces an ordered trace by its sessions' turns, which
runs identically (`tests/ir.rs`).

### Statements (`CStmt`)

| Statement | Meaning |
|---|---|
| `Turn` | draw the next turn's attributes (workload `turn` block or trace) |
| `Set(slot, e)`, `Observe(k, e)` | assign an attribute, record an observation |
| `Hold {pools: [(pool, units, reserve?)], reuse?, body, cache?, lease?}` | acquire units of every pool (admission gate `reserve` if given), run `body`, release; `reuse` bounds the own cached prefix consumed, `cache` the units left cached; `lease: (pool, t)` keeps that pool's allocation past the scope, neither evictable nor a preemption victim, until the session's `Release` of it, `t` seconds, or its end (vLLM's `delay_free_blocks`) |
| `Grow(pool, e)`, `Drop(pool)` | grow the current hold, drop the own cached entry |
| `Release(pool)` | give the innermost enclosing hold's allocation on the pool back now, or end the session's lease of it, caching per the hold's `cache`; nothing held or leased there is a no-op. A KV transfer between instances is `Run` (the link), `Load` (the destination) and `Release` (the source's lease) |
| `Load(pool, e)` | the KV of `e` tokens arrived from outside the engine (a NIXL read): the innermost enclosing hold's computed position on the pool advances by `e`, within its allocation |
| `Run {stage, mode, work, growing?, also?}` | work at a stage; `mode` `Plain`, `Prefill`, `Decode` (step stages); `growing` the pool that grows with the tokens computed; `also` further stages the same job holds at once (a flow of `share`), omitted when empty |
| `Branch(e, then, else)`, `Loop(body)`, `Choose {var, count, key}` (`key` a list, compared in order), `End` | control; `End` ends the session |

### Expressions (`CExpr`)

`Num`, `Attr(slot)`, `Ctx(var)` (`Now`, `Size`, `Age`, `Last`, `Queued`,
`N`, `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`: each exists at
one *moment*, below, and `Now` at every one), `Sample(dist, args)`,
`Call(fun, args)` (arithmetic functions, pool and stage queries such as
`CachedIn(pool)`, `BudgetLeft(stage)`), `Unary`, `Binary`, `Cond`. Pool
and stage references are `CRef {base, count, index?}` (a family of
`count` pools from `base`, selected by `index`).

## Validation

`Program::validate` checks the version, that every block, attribute,
observation, pool and stage index exists, the run parameters, that
every context variable is read at the moment that supplies it, and the
flows: every stage array a `Run` holds with another (`also`) is a `ps` of
a constant capacity, every run on such a *shared* stage is `Plain` with no
`growing`, a run names each stage array once, and `share` is present
exactly when some `also` is non-empty (`Program::shared_stages`).
`Program::from_json`, `run_ir` and the linker (`compile_source`) call it,
so a text program meets the same check as IR from files and tools.

**Moments.** An expression is evaluated at one moment, fixed by
its position in the IR, and a context variable exists at one of them:

| Moment (`ir::Moment`) | Positions | Context variables |
|---|---|---|
| `Session` | statements of `init`, `turn`, `session`; a run's work; a hold's `cache` (read when the session releases); `Grow`, `Load`, `Branch`, `Choose` | `Now` |
| `Admit` | a hold's units, `reserve`, `reuse`; a pool's queue key (read when the scheduler admits or orders the session, not when it reaches the statement) | `Now` |
| `Evict` | eviction keys, a spill's `work` and `when` | `Size`, `Age`, `Last`, `Queued`, `Now` |
| `Ps` | a `ps` stage's capacity | `N`, `Now` |
| `Budget` | a step stage's `budget` and `chunk`, evaluated before the iteration from its residents | `Nres`, `Ndec`, `Kvb`, `Kvp`, `Now` |
| `Step` | a step stage's `cost`, evaluated after the iteration is scheduled | `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`, `Now` |
| `Serve` | a step stage's `serve by` keys, evaluated for one resident once the residents are known | `Decoding`, `Admission`, `Remaining`, `Nres`, `Ndec`, `Kvb`, `Kvp`, `Now` |

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
collection uses the same event number (`SeqExec.lean`'s `lru`: release
order).

## Stability

`IR_VERSION` identifies what a reader must understand, not the shape of the
file. `serving-queue-theory`'s `scripts/gen_seq_oracle.py` pins it and reads
the IR by field name, so a bump is a handshake between the two repositories,
priced as such. What a change to `src/ir.rs` does to the version:

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
- **Same shape, a stricter check: no bump.** An IR file that validated before
  and is rejected now was reading a context variable at a moment that never
  supplied it (Moments, above), or a new file lists in `hidden` an attribute
  the scheduler reads: the check got stricter, the format did not change.
- Committed IR files (`tools/oracle/*.ir.json`, incl. `cache_trace.ir.json`) are regenerated by
  `make oracle-ir` and checked by `make check`
  (`tests/vllm_oracle.rs::oracle_ir_files_are_current`).
- `tests/ir.rs`: every example program survives a JSON round trip exactly
  and runs to the same report from IR as from text.

A version is a release, and the lines above decide one thing: whether a
change to a *tagged* version opens the next number. While the version at
`IR_VERSION` has no tag (7 in `v0.1.0-rc5`, 6 in `v0.1.0-rc4`, 5 in `v0.1.0-rc1`;
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
does not advance computed KV. Version 8 is still untagged (the latest tag,
`v0.1.0-rc5`, carries 7), so the number remains 8 under the policy above;
the change belongs in its release note. JSON shape and the ordinary serving
order are unchanged. `ExclusivePrefill` is outside the Lean fragment;
this change neither extends it nor changes the oracle IR files.

## The Lean fragment

The Lean model (serving-queue-theory, `SeqExec.lean`) runs a fragment of
the IR on a step clock over natural numbers: pools with LRU eviction and
LIFO preemption, one step engine (stage 0) with unit iteration cost, delay
stages, explicit sessions with preset attributes and turns, and the
statements `Turn`, `Hold`, `Run`, `Set`, `Observe`, `Branch`, `Loop`, `End`
(not `Release`, `Load` or a hold with a `lease`: a program with a KV
transfer is outside the fragment until `SeqExec.lean` gives a hold's pool
its own release)
with expressions built from integer constants, attributes, `Now`,
`CachedIn`, `BudgetLeft`, `min`, `max`, `+`, `-` (truncated at 0), `*`,
`floor(a / b)`, comparisons and conditionals. A constant expression over
context variables with zero coefficients (the replay's cost at `a = b = 0`)
counts as a constant. Its generator translates an IR file into Lean and
fails on anything outside the fragment. The Lean `Branch` takes the first
block when the guard is non-zero; the interpreter admits only 0 or 1
(`docs/language.md`, Branching), so the two agree on every program that
runs.
