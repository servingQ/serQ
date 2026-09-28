# 5. The engine

The `fifo` stage of chapters 3 and 4 served one turn at a time, start to
finish. No LLM engine works like that. A real engine runs **iterations**: each
one hands a token budget to the requests already running — one token to each
decoding request, a chunk to each prefilling one — and then admits waiting
requests with whatever budget is left. Prefill and decode are not phases. They
are the same iteration.

vLLM's scheduler puts it plainly: there is *"no decoding phase nor prefill
phase"*. seQ has one stage kind for this, and it is the one the lecture's
language could not express.

## The program

```rust title="docs/tutorial/programs/05-engine.seq"
--8<-- "docs/tutorial/programs/05-engine.seq"
```

Its deployment, drawn by [`seq-lang draw`](../visualization/index.md):

![The engine as a queueing network](../assets/05-engine.deployment.svg)

## The `step` stage

```rust
stage engine : step {
  budget B;
  cost max(omega + beta * (kvb + kvp), ntok * alpha);
  memory kv;
}
```

`budget` is the tokens one iteration may schedule (`max_num_batched_tokens`).
`cost` is how long the iteration takes, as an expression in what it scheduled:

| Variable | Meaning |
|---|---|
| `ntok` | tokens scheduled this iteration |
| `ndec` | decoding residents scheduled |
| `npre` | prefill tokens scheduled |
| `nres` | residents, scheduled or not |
| `kvb`, `kvp` | memory held by the scheduled decode / prefill residents |
| `attn` | attention work of the prefill chunks, \(\sum n(K + n/2)\) |

The cost above is the standard roofline: an iteration takes either the time to
read the weights and the residents' KV, or the time to compute the tokens it
scheduled, whichever is larger. On an A100 with Qwen3-8B, fitting
`c + d·ndec + e·kvb + a·npre + b·attn` to 3 022 measured steps gives a MAPE of
2.7 % for decode and 5.6 % for prefill — the cost expression is where a real
measurement enters the model.

Other options: `chunk` caps one request's prefill chunk
(`long_prefill_token_threshold`), `decode first` serves decoding residents
before prefilling ones, and `exclusive prefill` makes a prefill chunk run alone
and stall every decode (the RBLN stack).

## Three new pieces of the route

```rust
set c = min(cachedin(kv), floor((prompt - 1) / bs) * bs);
hold slots (1), kv (c + min(prompt - c, budget_left(engine))) {
  run engine prefill (prompt - c) growing kv;
  run engine decode (o - 1) growing kv;
} cache (prompt + o);
```

**`growing kv`** — the request does not take all its memory up front. It is
admitted with the blocks for the chunk it can run now, and grows block by
block as it decodes. When growth finds no free block, `preempt lifo` throws
out the most recently admitted request: its blocks are freed *to the cache*, it
goes back to the head of the queue, and it recomputes what it lost. That is
vLLM's `_preempt_request`, and in seQ it is just "abort the scope and re-run
the statement" — which is what made `hold` a scope in chapter 2.

**`budget_left(engine)`** — how many tokens the next iteration leaves after
its residents. The admission asks for the hit's blocks plus the chunk that
budget can take *now*. The expression is evaluated when the request is
admitted, not when it queues, which is the rule chapter 2 flagged and this is
the program that needs it.

**`admit via engine`** on a pool (not used above, but in
`programs/vllm.seq`'s `slots`) hands the pool's queue to the engine's
scheduler: waiting requests are admitted at the start of an iteration, with
the budget left, and never in an iteration that preempted.

## Running it

```bash
seq-lang run docs/tutorial/programs/05-engine.seq
```

```text
run: horizon 20000 warmup 2000 seed 1 events 9212861 arrivals 9905 ended 8903 turns 43947 mean live 5.977
observe        count        mean      95% CI      cv2       p99
  hit           43947      0.7927 ±0.0040      0.262    1.0000
  ttft          43947      0.0181 ±0.0007      1.276    0.0798
  response      43947      0.0633 ±0.0011      0.645    0.2429
stage          number   util    done   thru      wait   service  iters
  engine         0.153  0.138   43947  4.8830    0.0000    0.0313 9164050
  tool           5.822  0.998   35044  1.9469    0.0000    2.9903      0
pool             used     cached  queue holders    wait  admits evict(n)  evict(u) preempt spill rej
  kv               729.8   28515.3  0.000   0.153     NaN   48810      218   1760208       0     0   0
  slots              0.2       0.0  0.000   0.153  0.0007   48810        0         0       0     0   0
```

**9 164 050 iterations.** That is what `step` costs you: the engine is
simulated iteration by iteration, and at ~2 ms each a 20 000-second horizon is
nine million of them. Nothing else in seQ is this expensive.

`done 87894` at the engine against 43 947 turns — two runs per turn, prefill
and decode.

TTFT is now separately observable, and at 18 ms it is a different quantity from
the 63 ms response. Separating them is the whole reason to model the engine at
iteration granularity instead of as one service time.

!!! note "`wait NaN` on the `kv` pool"
    A hold on several pools joins the queue of the **first** one, so `kv` never
    has a queue of its own and its mean wait is 0/0. Cosmetic; the queueing is
    all on `slots`.

---

Next: turn the load up. → [The cliff](06-the-cliff.md)
