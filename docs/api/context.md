# Context variables

A context variable is supplied by the scheduler at one *moment* and exists only
there (`now` at every moment). Reading it anywhere else is a link error rather
than a 0: `set x = tokens;` in a session, or `evict by (tokens)`, does not link.
Its name is its own: a `set`, `choose` or `let` may not take it (an attribute
of that name would be read where the context variable was meant: `set present
= 500;` anywhere in the program would make a stage's `ps(min(present, 16))`
read the attribute), and the linker rejects the program.

## Moments

| Moment | Where an expression is evaluated | Evaluated |
|---|---|---|
| `Session` | a statement of `init`, `turn`, `session`, `server`; a run's work; a hold's `cache` | by the session, when it gets there |
| `Admit` | a hold's units, `reserve`, `reuse`, `at admission` | for one session at admission |
| `Select` | a pool's `queue by` keys | for every waiting hold before each selection |
| `Evict` | an eviction key; a `spill` clause | for one cache entry |
| `Ps` | a `ps` stage's capacity | for the stage's jobs |
| `Budget` | a step stage's `budget` and `chunk` | before the iteration, from the residents |
| `Step` | a step stage's `cost` | after the iteration is scheduled |
| `Serve` | a step stage's `serve by` keys | for one resident |
| `Given` | a claim's `given` | for each session, once its `init` has run |
| `Iteration` | a claim over iterations | when an iteration starts, where the cost is read |
| `End` | a claim `at end` | once, when the run ends |

## Variables

| Name | Type | Moments | Meaning |
|---|---|---|---|
| `now` | number | all | simulation clock |
| `waited` | number | `Select` | seconds since this hold entered the queue; reset on re-entry |
| `size` | number | `Evict` | units of the entry |
| `age` | number | `Evict` | `now − last` |
| `last` | number | `Evict` | time the entry was released |
| `waiting` | 0 / 1 | `Evict` | 1 if the entry's session waits in a pool queue |
| `present` | number | `Ps` | jobs present |
| `residents` | number | `Budget`, `Step`, `Serve`, `Iteration` | residents, scheduled or not |
| `decoders` | number | `Budget`, `Step`, `Serve`, `Iteration` | decode residents |
| `kv_decode` | number | `Budget`, `Step`, `Serve`, `Iteration` | memory held in the stage's `memory` pool by the decode residents (0 without `memory`) |
| `kv_prefill` | number | `Budget`, `Step`, `Serve`, `Iteration` | the same, by the prefill residents |
| `tokens` | number | `Step`, `Iteration` | tokens scheduled this iteration |
| `prefilled` | number | `Step`, `Iteration` | prefill tokens scheduled |
| `attention` | number | `Step`, `Iteration` | attention work of the prefill chunks, `Σ n (K + n/2)`, `K` the position before a chunk (exact for `growing` runs) |
| `decoding` | 0 / 1 | `Serve` | 1 if the resident is decoding, 0 if prefilling |
| `admission` | number | `Serve` | the resident's admission sequence number (its place in vLLM's `running` list) |
| `remaining` | number | `Serve` | tokens the resident's run has left |
| `demand` | number | `Iteration` | tokens the residents could take this iteration with no budget: `min(1, remaining)` per decode, the remaining work up to the `chunk` per prefill, over the residents after the batch is scheduled (those `serve only` leaves out included) |
| `served` | number | `Iteration` | tokens the stage scheduled in its earlier iterations, from the start of the run |
| `arrived` | number | `Iteration` | sessions the workload has started by the iteration's start, one arriving at that instant included |

`residents`, `decoders`, `kv_decode` and `kv_prefill` are totals over the residents, the same for
every resident at `Serve`. At `Budget` and `Serve` they count every resident
before the iteration. At `Step`, `decoders`, `kv_decode` and `kv_prefill` count only the
residents the iteration scheduled, and `residents` every resident.

`waiting` (a context variable at `Evict`, 0 or 1 for one entry) is not
`queued(p)` ([function](functions.md#pool)), the length of a pool's queue.

## Reading hidden attributes

[`hidden`](workload.md#hidden) attributes are legal at `Session` only, so every
moment above other than `Session` rejects them.
