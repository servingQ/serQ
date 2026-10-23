# A claim in its engine's words

A claim over an engine's iterations, `claim … every iteration of E (…)`
or `… some iteration of E (…)`, reads the engine's values by the names the
engine's own clauses read them by: the batch as `batch.tokens`,
`batch.prefilled`, `batch.decoding`, `batch.kv_decode`,
`batch.kv_prefill` and `batch.attention`, as the worker reads it, and the
residents' number as `running.count`. The kernel's bare names (`tokens`,
`decoders`, `residents`, …) no longer link there, and anywhere else they
are answered with the engine's names. The change is parse-time
sugar and a stricter check: every engine name lowers to the context
variable the bare name gave, so the IR, `IR_VERSION`, the Lean generators
and `lean/` do not move (#437).

## The case

A claim read the kernel's names, and the engine's body read the same
values by the engine's names, so one program called one value two ways.
Worse, the engine's table (`ENGINE_VALUES`, `src/frontend/parser/device.rs`)
maps the kernel's `decoders` twice: to `running.decoding`, the residents
that decode, read in `tokens cap` and the scheduler, and to
`batch.decoding`, the decodes in the batch, read in `worker`. A reader of
`decoders == residents` in a claim could not tell from the program which
one it was (criteria 0 and 1), and this site did not: `docs/use-cases/bari.md`
read the claim's `decoders` as `running.decoding` (removed here), where it
was `batch.decoding`. The review of #422 found it; the issue
could only refuse the bare names once every step stage was an engine, which
#421 made true.

## Where a claim is read

The interpreter checks an iteration claim with the worker's cost, in the
cost's own context: `start_iteration` builds it once the batch `assign` is
formed (`src/engine/interp.rs:5589-5626`), summing `ntok`, `ndec`, `npre`,
`kvb`, `kvp` and `attn` over the batch and taking `nres` as
`self.residents(st).len()`, then passes it to `check_iteration`
(`:5638`), which adds `demand`, `served` and `arrived` and evaluates each
claim (`:5976-6018`). So in a claim:

| Engine name | Kernel name (IR) | What it is there |
|---|---|---|
| `batch.tokens` | `tokens` (`Ntok`) | the tokens of this batch |
| `batch.prefilled` | `prefilled` (`Npre`) | its prefill tokens |
| `batch.decoding` | `decoders` (`Ndec`) | its decodes |
| `batch.kv_decode`, `batch.kv_prefill` | `kv_decode`, `kv_prefill` (`Kvb`, `Kvp`) | the memory its decodes, its prefills hold |
| `batch.attention` | `attention` (`Attn`) | its attention work |
| `running.count` | `residents` (`Nres`) | the residents once the batch is formed, in it or not: the ones it admitted counted, the ones it preempted not |

`demand`, `served` and `arrived` are the claim's own and have no engine
name, so they stay as they are; so does `now`.

The parser reads the claim's expression as one more clause of an engine,
`claim` (`device::CLAIM`), through the same `engine_expr` the engine's
clauses use: `ENGINE_VALUES` lists `claim` among the clauses of the seven
values above, `list_value` lowers each to its kernel name, and
`bare_kernel_name` answers a bare name with the engine's. No list of the
claim's names is kept anywhere else. A test holds the table of
`docs/language.md` (§Engines on devices), which gains a column for the
claim, equal to `ENGINE_VALUES`.

## Before and after

`examples/papers/bari_rad.sq:41-42`, before:

```serq
  claim optimal_tiling given (vp == bcol * floor(vp / bcol)):
    every iteration of E (tokens == bcol || decoders == residents);
```

After:

```serq
  claim optimal_tiling given (vp == bcol * floor(vp / bcol)):
    every iteration of E (batch.tokens == bcol || batch.decoding == running.count);
```

`dai_sarathi.sq` and `dai_fastertransformer.sq` write `batch.tokens` for
`tokens` the same way. `serq ir` of each of the three programs before the
change, on the binary before it, and after it, on the binary after it,
is the same byte for byte (`bari_rad` with its workload at horizon
1e8, the two others with `dai_periodic.sq` at 5e6), and
`tools/claims/*.ir.json` and `lean/Serq/Claims.lean` do not move.

## What is refused, and why

```text
in a claim over an engine's iterations, `tokens` is `batch.tokens` here: one value, one name
in a claim over an engine's iterations, `decoders` is `batch.decoding` here: one value, one name
in a claim over an engine's iterations, `residents` is `running.count` here: one value, one name
```

A name renamed since #139 (`ntok`, `ndec`, …) is answered the same way.
An engine value the claim's moment does not hold is refused with why:

```text
a claim over an engine's iterations is read once the batch is formed, where the worker's `forward` is: it reads the batch, `batch.…`, and the residents' number, `running.count`; `running.decoding` counts the residents, in the batch or not, and the batch's is `batch.decoding`
… ; a claim reads a queue as `queued(P)`
… ; `running.preempted` is what the iteration's schedule did, read in its `if`, `while` and `set`
… ; `request.input` is one request's, read in its engine's `taking` and `cache`
```

Outside an engine and its claims, a list's value says where it is read,
from the table: `` `batch.…` is a value of an engine, read in its
`scheduler` and `worker`, and in a claim over its iterations ``. A claim
`at end` and a claim's `given` are not read at an iteration, so they read
no engine value, as before.

No moment but an engine's clauses and a claim over its iterations
supplies a variable the engine's values name (`CtxVar::moments`), and
each of them now reads it by the engine's name, so the kernel's bare name
is no word of a program anywhere. Out of them, `bare_kernel_name` answers
it with the engine's names and where each is read, from the table, at the
word; a `let` or an attribute named so, which the linker refuses
(`takes_supplied`), says the same:

```text
`tokens` is a value of an engine, read as `batch.tokens` in its `scheduler` and `worker`, and in a claim over its iterations
`decoders` is a value of an engine, read as `running.decoding` in its `tokens cap`, `each at most`, `only`, `by` and `scheduler`; or `batch.decoding` in its `worker`, and in a claim over its iterations
`tokens` is a name the language supplies, read in an engine, as `batch.tokens` in its `scheduler` and `worker`, and in a claim over its iterations; a `let` constant named `tokens` would make the one word mean two things
```

Before, a claim `at end (tokens > 0)` was refused by the IR's check of
moments, in the IR's words: `` `tokens` … exists only in an engine's
`worker` … or a claim over iterations ``, both of which refuse `tokens`.
The messages of an engine's clauses that say where a `batch.…` value is
read (`` `tokens cap` is read before the batch is formed … ``) read the
table too, so they name the claim with the worker.

## What is not added

- **The residents' totals, `running.decoding`, `running.kv_decode`,
  `running.kv_prefill`.** At an iteration claim's moment the kernel's
  `decoders`, `kv_decode` and `kv_prefill` are the batch's: the context
  holds no residents' totals then. Reading them would take a new context
  variable, or a snapshot from before the batch, which is an IR change
  `scripts/gen_lean_claims.py` would have to learn, and no program needs
  it. Refusing the names now keeps them free for that change, with the
  program that needs it, without changing what any program means.
- **`waiting.count`.** It lowers to the sum of `queued` over the pools the
  engine admits, which the linker resolves for the engine's own clauses; a
  claim would need the same plumbing, and `queued(P)` already says it.

## A sentence fixed on the way

`docs/language.md` said the iteration advances the clock by `worker`'s
expression in the batch's values "and `running.count`", but the worker
does not read `running.count`: the parser refuses it (`the worker reads
what the scheduler scheduled, batch.…`). #580 made the worker read only
`batch.…` and changed the table's `worker` column, not the sentence. It is
prose, so no check applies to it; the table beside it
is now held to `ENGINE_VALUES` by a test, and the new Claims text, which
says a claim reads the batch as the worker does and `running.count`
besides, would have contradicted it.

## Self-critique

**Keeping the kernel's names in a claim.** Rejected on criterion 0: the
engine's table gives `decoders` two meanings, and a claim's reader would
have had to know the claim's moment to tell which. The kernel's names
stay the IR's and the Lean statements' (`r.stats.decoders`), where there
is one moment per expression.

**Accepting both spellings.** Rejected: one meaning, one spelling. Two
names for `batch.tokens` in one program are what the engine form refused
in its own clauses (#428).

**Engine names for `demand`, `served` and `arrived`.** Rejected: the
engine has no such values, so a new name would disambiguate nothing and
add a spelling.

**Reading the residents' totals as the iteration starts.** Rejected for
now, above: an IR change that no program asks for.

**Claims inside the engine's body.** A claim written in the engine block
would read the engine's names without a clause of its own. Rejected: a
claim is about a path, not a part of the deployment, `at end` claims have
no engine, and moving the construct would grow the frontend more than a
clause name does.

**Naming the engine's spelling in the IR's message.** The IR's check of
moments (`Moment::place`) could have said `batch.tokens` for an
iteration. Rejected: the IR is read without the frontend, where the
variable is `tokens`, and `demand`, `served` and `arrived` are read bare
at the same moment. The frontend answers the bare name before the IR is
built instead, so from a program the IR's message is reached only by a
variable a program writes bare (`demand` in a session statement).
