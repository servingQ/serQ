# Language cheatsheet

The complete specification is [the language](../language.md). This is the
one-page version.

## Shape of a program

```rust
let NAME = expr;                    // constants, overridable with --set

pool NAME [ '[' N ']' ] { … }       // a counted resource
stage NAME [ '[' N ']' ] : kind;    // where time passes

workload { … }                      // how sessions arrive and turns evolve
route { … }                         // what every session does
run { horizon …; warmup …; seed …; }
```

## Pools

```rust
pool kv {
  cap 160000;                 // capacity in units (default: inf)
  block 16;                   // allocate and cache in blocks
  evict lru;                  // or: evict by (k1, k2, …)  ascending
  preempt lifo;               // or: preempt none          what a failed grow does
  queue fifo;                 // or: queue by (expr)       admission order
  admit via engine;           // the queue is served by a step stage's scheduler
  spill tier via link (w) when (c);
}
```

## Stages

| Kind | Meaning |
|---|---|
| `fifo` / `fifo(c)` | `c` servers, one job each at rate 1, arrival order |
| `ps(φ in n)` | processor sharing: throughput `φ(n)` split equally |
| `delay` | infinite servers — every job at rate 1, no waiting |
| `step { … }` | an iterating engine (continuous batching) |

```rust
stage engine : step {
  budget B;                   // tokens per iteration
  cost <expr>;                // seconds per iteration
  chunk C;                    // cap on one request's prefill chunk (0: none)
  exclusive prefill;          // a prefill chunk runs alone
  decode first;               // decoding residents before prefilling ones
  memory kv;                  // the pool that gives kvb / kvp
}
```

## Statements

```rust
turn;                              // draw the next turn's attributes
set x = expr;                      // a session attribute
observe name = expr;               // record a sample

hold P (u) [fits (r)] [, Q (v)]* [reuse (ρ)] { … } [cache (ℓ)];
grow P (d);                        // enlarge the innermost hold
drop P;                            // discard the own cached prefix

run S [prefill|decode] (w) [growing P];

branch (e) { … } [else { … }]
loop { … }
choose j in n by (expr);           // j := argmin over 0..n
end;
```

## Expressions

Arithmetic, comparisons (0/1), `&&`, `||`, `!`, `c ? a : b`.

**Functions** `min` `max` `abs` `floor` `ceil` `sqrt` `exp` `ln` `pow`

**Distributions** `~exp(m)` `~det(x)` `~uniform(a,b)` `~erlang(k,m)`
`~h2(m,cv2)` `~bernoulli(p)`

**Observables**

| Stage | Pool | Step |
|---|---|---|
| `queue(s)` `busy(s)` `work(s)` | `used(p)` `free(p)` | `budget_left(s)` |
| `est_lambda(s)` `est_rho(s)` `est_wait(s)` | `cachedin(p)` `holders(p)` `queued(p)` | |
| `price(s, s_hit, ds)` | | |

**Context variables**

| Where | Names |
|---|---|
| anywhere | `now` |
| eviction keys, spill predicates | `size` `age` `last` `queued` |
| `ps` capacity | `n` |
| `step` budget and cost | `ntok` `ndec` `npre` `nres` `kvb` `kvp` `attn` |

**Built-in session attributes** `serial` `turn_no` `cached`, and with a trace
`new` `out` `think` `more` `forced`.

## Workload

```rust
workload {
  arrive poisson(λ);        // or closed(n), batch(n), none
  trace "file.csv" [ordered];
  init { … }                // once, at arrival
  turn { … }                // at every `turn` statement
}
```

## Rules worth memorising

1. **Commands take no time.** Only `run` lets the clock move.
2. **A hold's unit expressions are evaluated at admission**, not when the
   session queues. Anything reading the cache or `budget_left` depends on this.
3. **A hold on several pools joins the queue of the first one**, and is
   admitted only when every pool has room.
4. **Only the head of a queue can be admitted** — head-of-line blocking.
5. **Cached units never block an admission**; they are evicted to make room.
6. **`end` keeps the session's cached prefixes.** Write `drop P;` first if you
   want them gone.
7. **`allocated + cached ≤ cap`** holds in every reachable configuration.
