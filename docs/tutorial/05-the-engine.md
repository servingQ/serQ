# 5. The engine

Replace the per-turn service time with an engine. Each iteration
allocates a token budget across the requests it is running and then admits waiting
requests with the remaining budget. This model allows prefill and decode
to share an iteration.

## The program

```serq title="docs/tutorial/programs/05-engine.sq"
--8<-- "docs/tutorial/programs/05-engine.sq"
```

```serq title="docs/tutorial/programs/workloads/05-engine.sq"
--8<-- "docs/tutorial/programs/workloads/05-engine.sq"
```

Its deployment, drawn by [`serq draw`](../visualization/index.md):

![The engine as a queueing network](../assets/05-engine.deployment.svg)

## The engine on its device

The `stage engine` of chapters 3 and 4 is now an `engine`, and `engine` is
the word that declares one, so the engine is named `llm`. Leaving its
admission aside for now:

```serq
device gpu {
  compute (t) = t * alpha;
  hbm (k) = omega + beta * k;
  kv cap blocks * bs;
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
engine llm on gpu {
  reqs cap max_seqs;
  tokens cap B;
  scheduler {
    advance running;
    admit waiting while (running.preempted == 0)
      taking …;
  }
  worker {
    forward (max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
  }
}
pool reqs on llm { queue fifo; }
```

The `device` says what the hardware offers: the time to compute `t`
tokens, the time to read `k` tokens of KV, and the KV capacity. `pool kv
on gpu` is the device's memory, with its rules: blocks, eviction,
preemption. The `engine` says what runs on it. `reqs cap` is how many
requests may run at once (`max_num_seqs`), `tokens cap` the tokens one
iteration may schedule (`max_num_batched_tokens`). `pool reqs on llm`
holds the engine's `reqs cap`: its running slots, and the queue of
requests waiting for one, served `fifo`.

`scheduler` is what each iteration does: give the running requests their
tokens, then admit waiting ones while nothing was preempted. `taking` says
what an admission takes of a request, below. `worker` is how long the
iteration takes, as an expression in what it scheduled:

| Value | Meaning |
|---|---|
| `batch.tokens` | tokens scheduled this iteration |
| `batch.decoding` | decoding requests scheduled |
| `batch.prefilled` | prefill tokens scheduled |
| `batch.kv_decode`, `batch.kv_prefill` | memory held by the scheduled decode / prefill requests |
| `batch.attention` | attention work of the prefill chunks, \(\sum n(K + n/2)\) |

The cost is the larger of the time to read weights and KV memory and the
time to compute the scheduled tokens. Fit these parameters to measurements
of the engine you want to model. `each at most (c)` limits each request's
prefill in an iteration, and `advance running` takes an order and an
`only`; see [Engines on devices](../language.md#engines-on-devices).

## What the engine does with one request

The engine keeps each request's state, as vLLM's scheduler does: the
tokens it has, `request.known` (its prompt, `request.input`, and what it
has generated), and how many of them have KV. Each iteration it computes
what has no KV yet, a chunk of the prompt as the budget allows, and when
they all have it samples a token; after that it decodes one token per
iteration until it has generated `request.output`. Prefill and decode are
that one rule with many tokens left or one.

## What an admission takes

```serq
admit waiting while (running.preempted == 0)
  taking reqs (1), kv (min(request.known, request.hit + budget_left(llm)))
    reserve (request.known)
  reuse (floor(request.known - 1, bs))
  cache (request.input + request.output);
```

`taking` sits on `admit waiting` because that is when the scheduler sizes
the request: it reads the request at the moment the iteration admits it,
not when it arrived. A prefix may be evicted while the request waits.

**`reuse (floor(request.known - 1, bs))`** is how much of its tokens the
cache may supply: whole blocks, never the last token, which is computed
again for its logits. `request.hit` is what the cache does supply,
`min(cachedin(kv), reuse)`, read when the request is admitted; the engine
computes the rest.

**`reqs (1)`** is one running slot.

**`kv (min(request.known, request.hit + budget_left(llm)))`** is the blocks
of the reused prefix plus the prefill chunk the iteration's budget still
allows, capped by the tokens the request has. `budget_left(llm)` is the
token budget left after serving the running requests. The KV pool grows
with the request as it computes, block by block.

**`reserve (request.known)`** admits the request only if every token it
has fits beside the memory the running requests hold, although only the
first chunk is allocated. This is vLLM's default
(`scheduler_reserve_full_isl`). Without it, a long prompt is admitted on
its first chunk and its later chunks preempt other requests; at the loads
of the next chapter, that raises the preemption count about tenfold.

**`cache (request.input + request.output)`** leaves the request's
sequence cached when it ends, for the session's next turn, as in chapter
4.

With `preempt lifo`, a growth that does not fit preempts the most recently
admitted running request. It goes back to the head of the queue keeping
the tokens it has, and when it is admitted again computes their KV again
and generates only the rest (vLLM's `_preempt_request`).

Waiting requests are admitted only by `admit waiting`, in an iteration,
with the budget left, and never in an iteration that preempted: `pool reqs
on llm` hands the pool's queue to the engine's scheduler.

The engine records what it did with each admission: `llm.reused`, the
tokens it read from the cache, and `llm.prefill`, the tokens it computed.
The time to the first token needs no `observe`: the report's `client`
section gives it for every turn, from the `turn;` to the token the
prefill samples, once however many times the request is preempted, and
the time per output token after it
([what the client sees](../language.md#what-the-client-sees)).

## The workload and the server

The workload file is chapter 4's sessions. Its `session` describes the
conversation: each `turn;` waits for a response before the client decides
whether to continue. The workload sends each request its prompt, the
session's context and new tokens, its output, and whether it is the
session's last, `send (input: K + n, output: o, final: more == 0);`, and
the `server` says what a turn is:

```serq
server (input, output, final) {
  llm (input, output);
  // a session's last turn leaves nothing worth keeping
  if (final) { drop kv; }
}
```

one request to the engine, with `input` as its prompt and `output` as its
output: `request.input` and `request.output` in `taking`. After the
session's last turn the server drops its cached KV, which is the
deployment's, as in chapter 4. The server reads what the request carries,
not the client's `K`; the client keeps its context itself,
`set K = K + n + o;`. The deployment figure shows the server side.

## Running it

```bash
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1
```

```text
run: horizon 20000 end 20000 warmup 2000 seed 1 events 9310920 arrivals 9905 ended 8902 turns 44889 mean live 6.146

observe      count       mean    95% CI    cv2         p99
-----------  -----  ---------  --------  -----  ----------
llm.reused   44889  3988.7417  ±79.2073  0.861  16640.0000
llm.prefill  44889   876.4606  ±33.4152  1.471   3492.0000

client    count       mean      95% CI    cv2        p99
--------  -----  ---------  ----------  -----  ---------
ttft      44889    0.01906    ±0.00111  1.870    0.09567
tpot      44660  0.0002516  ±0.0000146  7.313  0.0007231
response  44889    0.06450    ±0.00217  0.817    0.26212

stage  number   util   done    thru    wait  service    iters
-----  ------  -----  -----  ------  ------  -------  -------
llm     0.158  0.141  89549  4.9749  0.0000   0.0318  9261013
tool    5.985  0.998  35986  1.9992  0.0000   2.9932        0

step  prefill only  decode only  mixed   idle  decodes  decode batch  decode step   itl p50   itl p99
----  ------------  -----------  -----  -----  -------  ------------  -----------  --------  --------
llm          0.037        0.097  0.007  0.859    0.113         1.080     0.000225  0.000209  0.000245

pool   used   cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  -----  -------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    783.2  29643.4  0.000    0.158     NaN   49906       435   3514144        0      0    0      0
reqs    0.2      0.0  0.003    0.158  0.0011   49906         0         0        0      0    0      0
```

`done 89549` at `llm` against 44 889 turns — two runs per turn, prefill
and decode, but for the 229 turns whose prefill samples their one output
token (`o` is 1), which never decode: `tpot` counts the 44 660 turns that
do.

TTFT is now separately observable, and at 19 ms it is a different quantity from
the 64 ms response. Iteration-level simulation gives both.

Measured 2026-10-15 on serQ 13c7166 with whole token counts (#661), by
the command above.

!!! note "`wait NaN` on the `kv` pool"
    An admission that takes several pools joins the queue of the **first**
    one `taking` names, `reqs`, so `kv` never has a queue of its own and has
    no queue-wait samples. Read the wait on `reqs`.

---

Next: turn the load up. → [The cliff](06-the-cliff.md)
