# Language cheatsheet

Common syntax at a glance. See the [API reference](../api/index.md) for
argument rules and [The language](../language.md) for complete semantics.

## Shape of a program

```serq
let NAME = expr;                    // fixed constants
def NAME(x, …) { expr }              // a name for an expression, expanded where it is used
def NAME(x, …) { … }                // a name for statements: NAME(a, …);
use "file.sq";                     // the defs of a library, relative to this file

use "std/args";
fn main() {
  let RATE = args.number("rate", 0.3); // explicit external input

  pool NAME [ '[' N ']' ] { … }       // a counted resource
  stage NAME [ '[' N ']' ] : kind;    // where time passes

  workload { …  }  // arrivals, turns and client actions
  server { … }                       // what one request runs
  share maxmin;                       // or bottleneck: how a run over several stages divides them
  gauge NAME = expr;                  // a time average of the state: max k in N (used(kv[k]))
  claim NAME: every iteration of E (demand < B || tokens == B);   // checked on the run; also `some iteration of`, `at end`
}
```

## Pools

```serq
pool kv {
  cap 160000;                 // capacity in units (default: inf)
  block 16;                   // allocate and cache in blocks
  evict lru;                  // or: evict by (k1, k2, …)  ascending
  preempt lifo;               // or: preempt none | preempt by (k1, …) [requeue tail]   whom a failed grow preempts
  queue fifo;                 // or: queue by (k1, …)     keys reevaluated at selection
  admit via engine;           // the queue is served by a step stage's scheduler
  reserve held;               // a hold's unallocated reserve counts against later admissions
  spill tier via link (w) when (c);
}
```

## Stages

| Kind | Meaning |
|---|---|
| `fifo` / `fifo(c)` | `c` servers, one job each at rate 1, arrival order |
| `ps(φ)` | processor sharing: throughput `φ`, which may read `present` (jobs present), split equally |
| `delay` | infinite servers — every job at rate 1, no waiting |
| `step { … }` | an iterating engine (continuous batching) |

```serq
stage engine : step {
  budget B;                   // tokens per iteration
  cost <expr>;                // clock time per iteration (1: the step clock)
  chunk C;                    // cap on one request's prefill chunk (0: none)
  granule G;                  // a prefill gets all it has left or a multiple of G (inf: whole or nothing)
  serve by (remaining);       // admission (default) | by (keys…) | decode first | exclusive prefill
                              // | only (p) [order]: serve the residents where p holds
  memory kv;                  // the pool that gives kv_decode / kv_prefill
}
```

## Statements

```serq
turn;                              // draw attributes and wait for the response
while (more) { … }                 // continue while the condition holds
// server { gw.route(); }          // select a gateway outside the workload
set x = expr;                      // a session attribute
observe name = expr;               // record a sample

hold P (u) [reserve (r)] [, Q (v)]* [reuse (ρ)]
     [at admission (name = e, …)]      // names evaluated at admission; body access is restricted
     { … } [cache (ℓ)] [lease P (t)];   // lease: P's units outlive the scope until released, t seconds, or the end
grow P (d);                        // enlarge the innermost hold
drop P;                            // discard the own cached prefix
release P;                         // give the enclosing hold's units of P back now, or end a lease of P
load P (n);                        // the KV of n tokens arrived: computed position += n

run S [prefill|decode] (w) [growing P];
run S, T (w);                      // one job holding ps stages S and T at once, at the rate share gives

// the serving vocabulary: the same statements, named by the request lifecycle
prefill W;  decode W;  tool Z;     // run on the stage of that name: W is
                                   // time on a fifo/ps/delay stage (seconds)
prefill T;  decode T;              // on a step engine: T is tokens, the budget's unit
transfer (X) from P to Q (n);      // run link (X); load Q (n); release P
transfer on S, T (X) from P to Q (n);   // uses both links for the transfer
prefill[j] W;  prefill on P (W);   // an instance of an array; an explicit stage

branch (e) { … } [else { … }]      // a test: e is 0 or 1
branch with (p) { … } [else { … }] // a draw: with probability p
loop { … }
choose j in n by (k1, …);          // least key over indices 0 through n-1
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
| `price(s, s_hit, ds)` | `blocksize(p)` (folded) | |

[Context variables](../api/context.md) depend on the evaluation moment:
`waited` in queue keys, `size` and `age` in eviction keys, `present` in `ps`
capacity, `tokens` in step cost, and `decoding` in serve and preemption keys.
`now` is the clock, but is forbidden in gauges, `given`, `only` predicates
and iteration guards.

**Built-in session attributes** `serial` `turn_no` `cached` `computed` (what a
preempted hold had computed; 0 initially and after the hold completes), and with a trace `new` `out`
`think` `more` `forced`.

## Workload

```serq
workload {
  arrive poisson(λ);        // renewal(~h2(mean, cv2)), closed(n), batch(n), none
  trace "file.csv" [ordered];
  init { … }                // once, at arrival
  turn { … }                // at every `turn` statement
  hidden o;                 // the scheduler may not read these (headers, keys, budgets)
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
