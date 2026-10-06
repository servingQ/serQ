# 5. The engine

Replace the per-turn service time with a `step` engine. Each iteration
allocates a token budget across its residents and then admits waiting
requests with the remaining budget. This model allows prefill and decode
to share an iteration.

## The program

```serq title="docs/tutorial/programs/05-engine.sq"
--8<-- "docs/tutorial/programs/05-engine.sq"
```

Its deployment, drawn by [`serq draw`](../visualization/index.md):

![The engine as a queueing network](../assets/05-engine.deployment.svg)

## The `step` stage

```serq
stage engine : step {
  budget B;
  cost max(omega + beta * (kv_decode + kv_prefill), tokens * alpha);
  memory kv;
}
```

`budget` is the tokens one iteration may schedule (`max_num_batched_tokens`).
`cost` is how long the iteration takes, as an expression in what it scheduled:

| Variable | Meaning |
|---|---|
| `tokens` | tokens scheduled this iteration |
| `decoders` | decoding residents scheduled |
| `prefilled` | prefill tokens scheduled |
| `residents` | residents, scheduled or not |
| `kv_decode`, `kv_prefill` | memory held by the scheduled decode / prefill residents |
| `attention` | attention work of the prefill chunks, \(\sum n(K + n/2)\) |

The cost is the larger of the time to read weights and KV memory and the
time to compute the scheduled tokens. Fit these parameters to measurements
of the engine you want to model.

`chunk` limits each request's prefill allocation. `serve` controls resident
order and selection; see the [stage reference](../api/stage.md).

## The workload and the server

The `session` inside `workload` describes the conversation: a `request;`
per turn, followed by a tool call or the end of the session. The `server`
block describes how each request is served. The deployment figure shows
the server side.

## Three new pieces of the server

```serq
set hitmax = floor((prompt - 1) / bs) * bs;
hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, hit + budget_left(engine))))
     at admission (hit = min(cachedin(kv), hitmax)) {
  set c = min(cached, floor((prompt - 1) / bs) * bs);
  run engine prefill (cost(engine, prompt - c)) growing kv;
  run engine decode (cost(engine, o - 1)) growing kv;
} cache (cost(reqs, kv, prompt + o));
```

**`growing kv`** allocates additional blocks as the request advances instead
of reserving its full memory at admission. With `preempt lifo`, a failed
growth can preempt the most recently admitted engine resident. The hold is
retried; see [preemption semantics](../language.md) for the state preserved
across retries.

**`budget_left(engine)`** is the token budget left after serving residents.
The admission reserves memory for the cached prefix plus the prefill chunk
that fits this budget, capped by `prompt`.

The header reads both the cache and the budget at admission. A prefix may
be evicted while the request waits, so reading the cache earlier in a `set`
would use an outdated value. `at admission (hit = …)` names this value for
the header.

**`admit via engine`** on a pool (`reqs` in the program above, and in
`examples/replay/vllm_replay.sq`) hands the pool's queue to the engine's
scheduler: waiting requests are admitted at the start of an iteration, with
the budget left, and never in an iteration that preempted.

## Running it

```bash
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1
```

```text
run: horizon 20000 end 20000 warmup 2000 seed 1 events 9210071 arrivals 9905 ended 8905 turns 44301 mean live 6.006

observe   count    mean   95% CI    cv2     p99
--------  -----  ------  -------  -----  ------
hit       44301  0.7938  ±0.0032  0.260  1.0000
ttft      44301  0.0182  ±0.0005  1.274  0.0798
response  44301  0.0629  ±0.0009  0.638  0.2370

stage   number   util   done    thru    wait  service    iters
------  ------  -----  -----  ------  ------  -------  -------
engine   0.153  0.138  88602  4.9223  0.0000   0.0311  9160680
tool     5.851  0.997  35398  1.9666  0.0000   2.9755        0

step    prefill only  decode only  mixed   idle  decodes  decode batch  decode step   itl p50   itl p99
------  ------------  -----------  -----  -----  -------  ------------  -----------  --------  --------
engine         0.036        0.096  0.006  0.862    0.110         1.079     0.000223  0.000209  0.000240

pool   used   cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  -----  -------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    735.9  28664.1  0.000    0.153     NaN   49390       258   1935856        0      0    0      0
reqs    0.2      0.0  0.002    0.153  0.0007   49390         0         0        0      0    0      0
```

`done 88602` at the engine against 44 301 turns — two runs per turn, prefill
and decode.

TTFT is now separately observable, and at 18 ms it is a different quantity from
the 63 ms response. Iteration-level simulation lets the program measure both.

!!! note "`wait NaN` on the `kv` pool"
    A hold on several pools joins the queue of the **first** one, so `kv` never
    has a queue of its own and has no queue-wait samples. Read the wait on `reqs`.

---

Next: turn the load up. → [The cliff](06-the-cliff.md)
