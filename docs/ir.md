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

Source: `src/ir.rs`. Version: `IR_VERSION = 3` (2 added the sessions' turns;
3 renamed the `route` field to `session`).

## Why an IR first

- **One program, several consumers.** The interpreter runs the IR, the Lean
  model of serving-queue-theory is generated from the IR, and the vLLM
  oracle tests run the IR. Before the IR existed, the vLLM request program
  had three hand-kept copies: the Rust test built it as a string per
  scenario, the Lean generator held a hand-written Lean version, and
  `programs/vllm.seq` was a third variant. Now there is one file,
  `programs/vllm_request.seq`, compiled once per scenario into
  `tools/oracle/<name>.ir.json`, and both the Rust test and the Lean
  theorems read those files. The multi-turn cache scenario is the IR of
  `programs/vllm_replay.seq` with its trace inlined
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
variants as strings, `"Lru"`). `seq-lang ir FILE` prints it;
`seq-lang run/check FILE.json` reads it.

### `Program`

| Field | Meaning |
|---|---|
| `version` | `IR_VERSION`; a different version is rejected |
| `attrs` | attribute names; an attribute is referenced by its index (slot) |
| `observes` | observation names, by index |
| `pools` | `CPool`: `name`, `cap` (units), `block` (allocation granularity), `evict` (`Lru` or `By([key exprs])`), `preempt` (`None`, `Lifo`), `queue` (order key), `spill`, `admit_via` (stage whose scheduler admits waiting holders) |
| `stages` | `CStage`: `name`, `kind`: `Fifo(servers)`, `Ps(capacity expr)`, `Delay`, `Step(CStep)` with `budget`, `cost`, `chunk`, `exclusive_prefill`, `decode_first`, `memory` (pool index) |
| `arrival` | `Poisson(rate)`, `Closed(n)`, `Batch(n)`, `Sessions([{attrs: [[slot, value], …]}])`, `None` |
| `trace`, `trace_ordered` | a trace corpus the workload draws turns from (path, resolved against the program's directory unless overridden) |
| `init`, `turn`, `session` | block indices: the workload's `init` and `turn` blocks and the session program |
| `blocks` | the statement blocks (an arena; bodies of holds, branches and loops refer to blocks by index) |
| `horizon`, `warmup`, `seed` | the run |
| `slot_cached`, `slot_serial`, … | slots of the built-in attributes (`cached`, `serial`, `turn_no`, `new`, `out`, `think`, `more`, `forced`) |

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
| `Hold {pools: [(pool, units, fits?)], reuse?, body, cache?}` | acquire units of every pool (admission gate `fits` if given), run `body`, release; `reuse` bounds the own cached prefix consumed, `cache` the units left cached |
| `Grow(pool, e)`, `Drop(pool)` | grow the current hold, drop the own cached entry |
| `Run {stage, mode, work, growing?}` | work at a stage; `mode` `Plain`, `Prefill`, `Decode` (step stages); `growing` the pool that grows with the tokens computed |
| `Branch(e, then, else)`, `Loop(body)`, `Choose {var, count, key}`, `End` | control; `End` ends the session |

### Expressions (`CExpr`)

`Num`, `Attr(slot)`, `Ctx(var)` (`Now`, `Size`, `Age`, `Last`, `Queued`,
`N`, `Ntok`, `Ndec`, `Npre`, `Nres`, `Kvb`, `Kvp`, `Attn`: meaningful only
where the semantics supplies them), `Sample(dist, args)`,
`Call(fun, args)` (arithmetic functions, pool and stage queries such as
`CachedIn(pool)`, `BudgetLeft(stage)`), `Unary`, `Binary`, `Cond`. Pool
and stage references are `CRef {base, count, index?}` (a family of
`count` pools from `base`, selected by `index`).

## Validation

`Program::validate` checks the version, that every block, attribute,
observation, pool and stage index exists, and the run parameters.
`Program::from_json` and `run_ir` call it. The text frontend always
produces valid IR; the check guards IR from files and tools.

## Stability

- Any change to the types in `src/ir.rs` bumps `IR_VERSION`. A field rename
  counts: `route` became `session` in 3, and every reader has to move with it.
  `serving-queue-theory`'s `scripts/gen_seq_oracle.py` pins `IR_VERSION` and
  reads `ir["route"]`, so it needs the matching change.
- Committed IR files (`tools/oracle/*.ir.json`, incl. `cache_trace.ir.json`) are regenerated by
  `make oracle-ir` and checked by `make check`
  (`tests/vllm_oracle.rs::oracle_ir_files_are_current`).
- `tests/ir.rs`: every example program survives a JSON round trip exactly
  and runs to the same report from IR as from text.

## The Lean fragment

The Lean model (serving-queue-theory, `SeqExec.lean`) runs a fragment of
the IR on a step clock over natural numbers: pools with LRU eviction and
LIFO preemption, one step engine (stage 0) with unit iteration cost, delay
stages, explicit sessions with preset attributes and turns, and the
statements `Turn`, `Hold`, `Run`, `Set`, `Observe`, `Branch`, `Loop`, `End`
with expressions built from integer constants, attributes, `Now`,
`CachedIn`, `BudgetLeft`, `min`, `max`, `+`, `-` (truncated at 0), `*`,
`floor(a / b)`, comparisons and conditionals. A constant expression over
context variables with zero coefficients (the replay's cost at `a = b = 0`)
counts as a constant. Its generator translates an IR file into Lean and
fails on anything outside the fragment.
