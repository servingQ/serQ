# Engine

```serq
device NAME [ '[' N ']' ] { RESOURCE (PARAM, …) = expr; CAP cap expr; … }
engine NAME on DEVICE {
  CAP cap expr;              // a capacity the engine holds, as a pool `on NAME`
  tokens cap expr;           // required: the tokens one iteration computes (inf: no cap)
  granule c;                 // a prefill gets all it has left or a multiple of c
  state NAME = c;            // a register the schedule sets
  connector QUEUE { … }      // its KV connector: an L2 queue, or its queue's KV source (below)
  scheduler { stmt … }        // required: what each iteration does (below)
  worker {
    forward (expr);            // required: how long the iteration takes
  }
}
pool NAME on DEVICE { … }          // the device's capacity: the engine's memory
pool NAME on ENGINE.DEVICE { … }   // the same, admitted by the engine
pool NAME on ENGINE { … }          // the engine's capacity, admitted by it
```

An engine runs iterations (continuous batching) on a device. Each
iteration gives the running requests their tokens, one to a decoding run
and a chunk to a prefilling one, then admits waiting requests while the
budget lasts, and lasts what `worker` says. A `growing` run first grows
its hold to the position it will reach. An attempt that schedules no token
and preempts nobody waits for the next event without running a timed
iteration. The server calls it with a
[request](../language.md#an-engines-request), `ENGINE (input, output);`,
whose state it keeps; a queue's entry calls its queue's engine the same
way, `Q (prompt, 1);`.

An engine cannot be named `engine`: it is named for what it models (`vllm`,
`sglang`, `tgi`) or `llm`. Inside a [`queue`](../language.md#queues), `device
gpu` is the member's and `engine on gpu` is the queue's engine, named after
the queue. One device runs one engine.

The form is parse-time sugar for the IR's step stage
([Engines on devices](../language.md#engines-on-devices) maps each clause);
the [IR](../ir.md) and the Lean model see only that.

## `device`

| Item | Description |
|---|---|
| `RESOURCE (PARAM, …) = expr;` | A time resource: a demand to time, read only in the engine's `worker`, as a `def` is. Its body reads every parameter. |
| `CAP cap expr;` | A capacity, which a `pool CAP on DEVICE { … }` declares with its rules. |

## Items of an engine

| Item | Type | Read | Description |
|---|---|---|---|
| `CAP cap` | `expr` | | A capacity the engine holds (`reqs cap max_seqs`, vLLM's `max_num_seqs`), declared with `pool CAP on ENGINE`. One per capacity: a family of engines, or of an engine's capacities, is a queue family (#539). |
| `tokens cap` | `expr` | as the iteration starts | Tokens per iteration (vLLM's `max_num_batched_tokens`). Reads `running.count`, `running.decoding`, `running.kv_decode`, `running.kv_prefill` and `waiting.count`, not `batch.…`. |
| `granule` | `const`, positive | | Prefill chunk alignment; see below. |
| `state NAME = c` | `const` | | A register; see below. |
| `scheduler` | statements | as the iteration is planned | Whom the iteration serves, in what order, and when it admits. |
| `worker { forward (T); }` | `expr` | after the batch is formed | What the worker does with the batch the scheduler formed (vLLM's `SchedulerOutput`): its forward's clock time. Reads `batch.…` and the device's time resources, not `running.…` or `waiting.…`. |
| `connector QUEUE { … }` | clauses | at a request's admission and its end | Its KV connector to an L2 queue or from its queue's KV source; see below. |

The values an engine reads, and in which clause, are in
[Engines on devices](../language.md#engines-on-devices): `running.…` is
the running requests as they stand, `waiting.…` the queues the engine
admits, `batch.…` the iteration's batch.

## Pools on a device or an engine

A pool on a device or an engine takes its name and its capacity from it;
its body holds the rules ([Pool](pool.md)). Who admits a request waiting
for it is said where the pool is:

| Pool | Admitted |
|---|---|
| `pool reqs on llm` | by engine `llm`, in an iteration's `admit waiting` |
| `pool kv on llm.gpu` | by engine `llm`, which runs on `gpu` |
| `pool kv on gpu` | as soon as it fits |

A pool on an engine is a family only in a queue family, one per member;
a top-level engine's is one pool, which its admission's `taking` names.

The pool option `admit via ENGINE;` is refused: who admits a pool is said
where the pool is, in its `on`.

The pool on the engine's device is the engine's memory either way: its
holds give `running.kv_…` and `batch.kv_…`, and a growth that does not fit
preempts among the engine's running requests.

## `scheduler`

```serq
scheduler {
  let NAME = expr;                                        // read once, as the iteration starts
  advance running [only (p)] [admission | by (k, …) | decode first] [each at most (c)];
  admit waiting [only (p)] [while (e)] [each at most (c)];
  exclusive prefill [each at most (c)];
  if (e) { … } [else { … }]
  set NAME = e;
}
```

The statements run once each, in order; a schedule has no loop, so an
iteration ends. A running request is served at most once per iteration.

| Statement | What it does |
|---|---|
| `advance running [only (p)] [order]` | Gives the running requests not yet served their tokens, in the order (admission order when none), while budget is left. A request `p` reads as 0 is skipped and stays unserved, for a later statement. A grower that preempts itself ends the statement. |
| `admit waiting [only (p)] [while (e)]` | Admits the head of the queues the engine admits and serves the newcomer, one at a time, while budget is left, the head fits and `e` (read before each) is 1. A newcomer for which `p` is 0 is admitted and waits unserved. |
| `exclusive prefill` | One prefill alone, or a decode-only batch; see below. |
| `if (e) { … } else { … }` | A test: the first statements when `e` is 1, the second when it is 0. |
| `set NAME = e` | Updates one of this engine's registers; see below. |

vLLM's schedule serves the running requests, then admits the waiting
while nothing was preempted (`scheduler.py:624`, `scheduler.py:869`):

```serq
scheduler { advance running; admit waiting while (running.preempted == 0); }
```

This spelling of the gate is the engine's built-in procedure and the one
`serq target` takes; the same test written otherwise is a schedule of the
program's own ([Engines on devices](../language.md#engines-on-devices)).

Guards and `while` conditions read the running requests as they stand, the
tokens scheduled so far (`batch.tokens`, `batch.prefilled`),
`waiting.admitted`, `running.preempted`, pool and stage queries, and
constants. They cannot draw or read `now`, `work(…)` or this engine's
`budget_left(…)`. Every path through a schedule must reach an `advance
running` or an `admit waiting`; otherwise it is a link error. This does
not guarantee progress: `admit waiting while (batch.tokens > 0)` cannot
admit into an empty engine. If the run ends with running or waiting
requests and the last attempt scheduled nothing, the report flags `idle:
stage …` (`idle_with_work` in JSON).

### Orders and `only`

| Order | Meaning |
|---|---|
| `admission` | admission order (the default) |
| `by (k1, …)` | ascending keys per request, ties by admission order |
| `decode first` | decodes before prefills |

Keys read `decoding`, `admission`, `remaining`, `running.count`,
`running.decoding`, `running.kv_…` and the clock (`now`, `work(…)`), and
may not draw. `by (remaining)` is
shortest-remaining-first; `by (-remaining)` is the opposite.

`only (p)` is read for each request at its turn, on the running requests
as they stand at that read (one the iteration admitted included). Unlike
a key it may not read `now` or `work(…)`: an engine whose requests it all
excludes waits for the next event, and the clock moving is none. It may
not draw. A request it excludes gets no token this iteration, keeps its
allocation and advances no computed KV. A predicate both statements read
is named with a `def`, read where it stands
([FasterTransformer](../use-cases/fastertransformer.md)):

```serq
def in_phase() { running.decoding > 0 ? decoding : !decoding }
…
scheduler {
  advance running only (in_phase());
  admit waiting only (in_phase()) while (running.preempted == 0);
}
```

### `each at most`

`each at most (c)` caps one request's tokens in the iteration; one cap per
run for the whole iteration, so every statement that has one says the
same. `inf` is no cap. `c` chooses among constants (its condition may
read the running requests as the iteration starts, through a `let`), and
one at or below 0 does not link. A cap that follows the iteration is
written `max(k, e)` with `k` a constant above 0: `e` reads what the
condition reads, and `k` keeps every run above 0. vLLM's adaptive
threshold (`scheduler.py:609-622`), with `threshold` and `budget` the
program's constants, is

```serq
let n = running.count + waiting.count;
let c = n > 1 ? max(threshold, floor(budget / n)) : inf;
```

It is vLLM's only when `budget` is the engine's `tokens cap`, vLLM's
`max_num_batched_tokens` (`scheduler.py:580`); `serq target` exports it
as `long_prefill_token_threshold` with
`long_prefill_token_threshold_adaptive`. `max(0, e)`, `min(k, e)`
or a bare `e` does not link; a cap that shrinks writes its floor,
`max(1, min(k, e))`.

### `exclusive prefill`

```serq
scheduler {
  exclusive prefill [each at most (c)];
  admit waiting while (running.preempted == 0) [each at most (c)];
}
```

A running prefill takes precedence and runs alone. Otherwise running
requests are considered for decode; while budget is left, a fitting
waiting prefill can replace that selection and use the full budget. A
selected prefill admits no further waiting request in that iteration.
Cancelled decode work neither runs nor advances computed KV; any capacity
already allocated remains held. During these admissions `budget_left`
supplies the full budget. Ordinary fit, queue-head and
no-admission-after-preemption gates still apply. It appears only in the
form above: it takes back decodes already chosen, which another schedule
cannot ([RBLN](../use-cases/rbln.md)).

### `granule`

A prefill receives all its remaining work, or a multiple of `granule`.
After `each at most` caps the work, it is rounded down to that multiple;
a result of 0 waits. `granule inf` schedules a whole prefill or none. A
waiting prefill stays running: later requests may still be served, but
the iteration admits no further requests. The constant must be positive,
cannot exceed a constant `each at most` or the floor `k` of a
`max(k, e)` one, and cannot be combined with
`exclusive prefill`.

### Registers

`state NAME = c;` declares a numeric register initialized by constant `c`.
The schedule updates it with `set NAME = e;`, where `e` follows the guards'
rules. Values persist between iterations. Updates are undone if the
attempt schedules no token, preempts nobody and admits nobody.

A register may be read in:

- its engine's `tokens cap`, `worker`, keys, schedule and iteration claims;
- queue keys of pools the engine admits;
- hold headers whose first pool the engine admits, and its request's `taking`;
- gauges and `at end` claims.

Other reads are link errors. Each register name must be unique and cannot
also name an attribute, constant, context variable, pool or stage. Engine
families cannot declare registers. A register the schedule never writes
with `set` (in an `if` or not) does not link: it would keep its first
value, which is a `let`. vLLM's procedure sets none, so its engine declares
none.

A guard based on a register must allow recovery when the engine empties.
TGI's engine skips admission after an iteration that admitted, unless no
request is running (`examples/engines/tgi.sq`):

```serq
state just = 0;
scheduler {
  advance running;
  if (just == 0 || running.count == 0) { admit waiting; }
  set just = waiting.admitted > 0;
}
```

Without the `running.count == 0` condition, an empty engine with `just ==
1` could never admit again: its unsuccessful attempt would undo the reset.
The report flags an engine left in this state as `idle: engine …`.

## A request and its admission

The server calls the engine, `ENGINE (input, output);`: a request of
`input` tokens of prompt that generates `output`. The engine keeps its
state, the tokens it has and what of them has KV, prefills what the cache
did not supply and decodes the rest, and the session goes on when it has
generated `output`; a preempted request keeps its tokens. What the
admission takes is written on the schedule's `admit waiting`, the same on
each:

```serq
admit waiting … taking POOL (units) [reserve (units)], …
  [reuse (e)] [prefix (k: n, …)] [cache MEMORY (e)];
```

`taking` names the engine's capacities and its device's, and reads the
request as `request.input`, `request.output`, `request.known`,
`request.hit`, `request.loaded` and `request.preempted`; `cache` names the
engine's memory, the pool on its device that the request's KV is in (a
`cache` on another pool does not link), and, read when the request gives
its pools back, reads `request.finished`. The engine's
records are attributes the caller reads after the call: `ENGINE.reused`,
`ENGINE.prefill`, `ENGINE.admitted` and `ENGINE.first` (a connector adds
`ENGINE.loaded`, `ENGINE.saved` and `ENGINE.load_done`); `ENGINE.reused`,
`ENGINE.prefill` and `ENGINE.loaded` are observations of the report too,
one per admission, which the report's engine table sums per engine
([engine statistics](../reference/cli.md#engine-statistics)).

An engine that is a KV relation's source
([`kvt.nixl_pull`](std/kvt.md)) keeps its memory past each request's end,
with nothing written here: the request registers a *send*, and its blocks
stay, neither evictable nor a preemption victim, until the reader's
[`connector`](#connector) has read them, the relation's `ttl`, or the
session's end (vLLM's NIXL connector returns `delay_free_blocks` when the
request finishes, `nixl/pull_scheduler.py:248-262`); `cache` is read when
the request ends and applies when the send does
([the design](../design/request-state.md)).

```serq
admit waiting while (running.preempted == 0)
  taking reqs (1),
         kv (min(request.known, request.hit + budget_left(P))) reserve (request.known)
  reuse (floor(request.known - 1, bs))
  cache kv (request.input);
```

(`examples/pd-disaggregation/llmd_nixl_pull.sq`'s prefiller, whose entry is
`prefill (prompt) { P (prompt, 1); }`.)

A queue's engine admits the same way, and the queue's entries call it,
`E (input, output);`; the server calls the entry, not the engine. A
family's `taking` names its pools without an index, and each member takes
its own. [An engine's request](../language.md#an-engines-request) has the
rules.

## `connector`

```serq
connector QUEUE {
  per_token (r);      // an L2's: the time a token's copy takes
  load (n);           // read at the admission, after request.hit
  save (n);           // an L2's: read when the request has generated its output
}
```

The engine's KV connector (vLLM's `KVConnector`). `QUEUE` is one of two:

- a queue that plays `l2` (LMCache's, `lmcache_mp_connector.py`). A request
  that loads `n > 0` tokens copies them at the queue's bandwidth,
  `per_token` a token, into its hold on the engine's memory before its
  prefill. A finished request copies to its entry in the queue the part of
  `n` tokens it lacks before it gives its pools back. `per_token` is
  required, and at least one of `load` and `save`. Under
  [`kvt.l2_link`](std/kvt.md#kvtl2_link) both copies also cross the
  engine's own `nic`, at once;
- the source of the engine's queue's KV relation (`kvt.nixl_pull (QUEUE,
  Q, …)` or `kvt.nixl_push`; NIXL's). A request its entry was called `from
  QUEUE[i]` with reads `n` tokens' KV over the two NICs the relation
  declares, once the source's request has registered its send (in a push,
  the dispatch's leg's prefill), and the send ends; one its entry took
  from no source loads nothing.
  It has `load` alone.

`taking` reads `n` as `request.loaded`. After the load, a request that has
KV for every token it has computes its last again (`scheduler.py:3075-3076`),
and its admission is read again: a pool it took none of and takes now, it
waits for (vLLM's `WAITING_FOR_REMOTE_KVS`, `scheduler.py:1264-1267`), so
`reqs (request.loaded > 0 ? 0 : 1) reserve (1)` holds no running slot
while the request loads. The engine records `ENGINE.loaded` and sets
`ENGINE.loaded`, `ENGINE.saved` and `ENGINE.load_done` (when its latest
load ended). `examples/multi-turn/vllm_lmcache.sq`:

```serq
connector lmc {
  per_token (r);
  load (request.preempted ? 0 : beyond(request.known, request.hit));
  save (floor(request.input + request.output, chunk));
}
```

and the decoder of `examples/pd-disaggregation/llmd_nixl_pull.sq`, whose
`decode (prompt) from P` entry calls it:

```serq
connector P { load (request.preempted ? 0 : request.input - request.hit); }
```

## Examples

`examples/multi-turn/vllm.sq`'s engine, its comments elided:

```serq
device gpu {
  compute (t) = t * a;
  hbm (k) = omega + beta * k;
  kv cap blocks * bs;
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
engine vllm on gpu {
  reqs cap max_seqs;
  tokens cap B;
  scheduler {
    let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
    advance running each at most (threshold);
    admit waiting while (running.preempted == 0) each at most (threshold)
      taking reqs (1), kv (min(request.known, request.hit + budget_left(vllm)))
        reserve (request.known)
      reuse (floor(request.known - 1, blocksize(kv)))
      cache kv (request.input + request.output);
  }
  worker {
    forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
  }
}
pool reqs on vllm { queue fifo; }
```

A complete program, a deployment and its workload:

```serq title="model.sq"
fn main() {
  device gpu { }
  engine llm on gpu {
    tokens cap 8;
    scheduler { advance running; admit waiting while (running.preempted == 0); }
    worker {
      forward (1);
    }
  }
  server {
    llm (8, 3);
    observe finished = now;
  }
}
```

```serq title="workload.sq"
workload {
  arrive batch(2);
}
```

Save the deployment as `model.sq` and its workload as `workload.sq`, then run:

```sh
serq run model.sq --workload workload.sq --horizon 10
```

## See also

[Stage](stage.md), [`run`](statements.md#run), [Pool](pool.md),
[Engines on devices](../language.md#engines-on-devices),
[`pyserq.Stage`](../python/stage.md).
