# Context variables

A context variable is supplied at specific evaluation moments. Reading it
elsewhere is a link error: a session cannot use `set x = tokens;`.
Context-variable names are reserved and cannot be assigned by `set`,
`choose` or `let`.

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
| `Serve` | a step stage's `serve by` keys and `only` predicates | for one resident |
| `Victim` | a pool's `preempt by` keys | for one candidate victim, when a growth does not fit |
| `Plan` | a step stage's `iteration` body: a `branch` guard, an `admit`'s `while`, a register assignment | as the iteration is planned, from the residents and what it has done so far |
| `Gauge` | a gauge expression | after each instant settles, held until the next event |
| `Given` | a claim's `given` | for each session, once its `init` has run |
| `Iteration` | a claim over iterations | when an iteration starts, where the cost is read |
| `End` | a claim `at end` | once, when the run ends |

## Variables

| Name | Type | Moments | Meaning |
|---|---|---|---|
| `now` | number | except `Gauge`, `Given`, `only` predicates and iteration guards | simulation clock |
| `waited` | number | `Select` | seconds since this hold entered the queue; reset on re-entry |
| `size` | number | `Evict` | units of the entry |
| `age` | number | `Evict` | `now − last` |
| `last` | number | `Evict` | time the entry was released |
| `waiting` | 0 / 1 | `Evict` | 1 if the entry's session waits in a pool queue |
| `present` | number | `Ps` | jobs present |
| `residents` | number | `Budget`, `Step`, `Serve`, `Plan`, `Iteration` | residents, scheduled or not |
| `decoders` | number | `Budget`, `Step`, `Serve`, `Plan`, `Iteration` | decode residents |
| `kv_decode` | number | `Budget`, `Step`, `Serve`, `Plan`, `Iteration` | memory held in the stage's `memory` pool by the decode residents (0 without `memory`) |
| `kv_prefill` | number | `Budget`, `Step`, `Serve`, `Plan`, `Iteration` | the same, by the prefill residents |
| `tokens` | number | `Step`, `Plan`, `Iteration` | tokens scheduled this iteration (in a body, so far) |
| `admitted` | number | `Plan` | sessions the iteration has admitted so far |
| `preempted` | number | `Plan` | the residents the iteration has preempted so far |
| `prefilled` | number | `Step`, `Plan`, `Iteration` | prefill tokens scheduled (in a body, so far) |
| `attention` | number | `Step`, `Iteration` | attention work of the prefill chunks, `Σ n (K + n/2)`, `K` the position before a chunk (exact for `growing` runs) |
| `decoding` | 0 / 1 | `Serve`, `Victim` | 1 if the resident (or candidate) is decoding, 0 otherwise |
| `admission` | number | `Serve`, `Victim` | the resident's admission sequence number (its place in vLLM's `running` list); at `Victim`, the candidate's place in the candidates' admission order |
| `remaining` | number | `Serve` | tokens the resident's run has left |
| `position` | number | `Victim` | the position the candidate's hold has computed on the pool: what `computed` becomes if it is the victim |
| `demand` | number | `Iteration` | tokens the residents could take this iteration with no budget: `min(1, remaining)` per decode, the remaining work up to the `chunk` per prefill, over the residents after the batch is scheduled (those `serve only` leaves out included) |
| `served` | number | `Iteration` | tokens the stage scheduled in its earlier iterations, from the start of the run |
| `arrived` | number | `Iteration` | sessions the workload has started by the iteration's start, one arriving at that instant included |

At `Budget`, totals describe residents before the iteration. At `Serve`
they describe the residents present when the expression is read, including
new admissions and excluding preempted residents. At `Step` and
`Iteration`, `decoders`, `kv_decode` and `kv_prefill` count scheduled
residents, while `residents` counts all residents.

`waiting` (a context variable at `Evict`, 0 or 1 for one entry) is not
`queued(p)` ([function](functions.md#pool)), the length of a pool's queue.

## Reading hidden attributes

[`hidden`](workload.md#hidden) attributes are legal at `Session` and in a
claim's `Given` condition. Scheduler expressions cannot read them.

## Examples

A complete program:

```serq
fn main() {
  stage engine : step {
    budget 8;
    cost 1 + 0.1 * tokens;
    serve by (remaining);
  }
  workload {
    arrive batch(2);
    session { request; end; }
  }
  server {
    run engine prefill (8);
    observe finished = now;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[Functions](functions.md), [attributes](attributes.md),
[hidden attributes](workload.md#hidden).
