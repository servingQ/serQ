# Context variables

A context variable is supplied by the scheduler at one *moment* and exists only
there (`now` at every moment). Reading it anywhere else is a link error rather
than a 0: `set x = ntok;` in a session, or `evict by (ntok)`, does not link.

## Moments

| Moment | Where an expression is evaluated | Evaluated |
|---|---|---|
| `Session` | a statement of `init`, `turn`, `session`, `server`; a run's work; a hold's `cache` | by the session, when it gets there |
| `Admit` | a hold's units, `reserve`, `reuse`, `at admission`; a pool's `queue by` | for one session, when the scheduler admits or orders it |
| `Evict` | an eviction key; a `spill` clause | for one cache entry |
| `Ps` | a `ps` stage's capacity | for the stage's jobs |
| `Budget` | a step stage's `budget` and `chunk` | before the iteration, from the residents |
| `Step` | a step stage's `cost` | after the iteration is scheduled |
| `Serve` | a step stage's `serve by` keys | for one resident |

## Variables

| Name | Type | Moments | Meaning |
|---|---|---|---|
| `now` | number | all | simulation clock |
| `size` | number | `Evict` | units of the entry |
| `age` | number | `Evict` | `now − last` |
| `last` | number | `Evict` | time the entry was released |
| `queued` | 0 / 1 | `Evict` | 1 if the entry's session waits in a pool queue |
| `n` | number | `Ps` | jobs present |
| `nres` | number | `Budget`, `Step`, `Serve` | residents, scheduled or not |
| `ndec` | number | `Budget`, `Step`, `Serve` | decode residents |
| `kvb` | number | `Budget`, `Step`, `Serve` | memory held in the stage's `memory` pool by the decode residents (0 without `memory`) |
| `kvp` | number | `Budget`, `Step`, `Serve` | the same, by the prefill residents |
| `ntok` | number | `Step` | tokens scheduled this iteration |
| `npre` | number | `Step` | prefill tokens scheduled |
| `attn` | number | `Step` | attention work of the prefill chunks, `Σ n (K + n/2)`, `K` the position before a chunk (exact for `growing` runs) |
| `decoding` | 0 / 1 | `Serve` | 1 if the resident is decoding, 0 if prefilling |
| `admission` | number | `Serve` | the resident's admission sequence number (its place in vLLM's `running` list) |
| `remaining` | number | `Serve` | tokens the resident's run has left |

`nres`, `ndec`, `kvb` and `kvp` are totals over the residents, the same for
every resident at `Serve`. At `Budget` and `Serve` they count every resident
before the iteration. At `Step`, `ndec`, `kvb` and `kvp` count only the
residents the iteration scheduled, and `nres` every resident.

`queued` (a context variable at `Evict`) and `queued(p)`
([function](functions.md#pool)) are different things: a 0/1 for one entry, and
the length of a pool's queue.

## Reading hidden attributes

[`hidden`](workload.md#hidden) attributes are legal at `Session` only, so every
moment above other than `Session` rejects them.
