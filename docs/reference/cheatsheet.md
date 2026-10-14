# Language cheatsheet

Common syntax at a glance. See the [API reference](../api/index.md) for
argument rules and [The language](../language.md) for complete semantics.

## Shape of a program

A program is the deployment; its workload is a file of its own
([below](#workload)).

```serq
let NAME = expr;                    // fixed constants
def NAME(x, …) { expr }              // a name for an expression, expanded where it is used
def NAME(x, …) { … }                // a name for statements: NAME(a, …);

use "std/args";
fn main() {
  let RATE = args.number("rate", 0.3); // explicit external input

  pool NAME [ '[' N ']' ] { … }       // a counted resource
  stage NAME [ '[' N ']' ] : kind;    // where time passes

  server { … }                       // what one request runs
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

An iterating engine (continuous batching) is an `engine` on a `device`:

```serq
device gpu { kv cap K; }          // what the hardware offers
engine llm on gpu {
  reqs cap S;                     // running slots (max_num_seqs)
  tokens cap B;                   // tokens per iteration (max_num_batched_tokens)
  granule G;                      // a prefill gets all it has left or a multiple of G (inf: whole or nothing)
  scheduler {
    advance running by (remaining) each at most (C);   // admission (default) | by (keys…) | decode first; only (p)
    admit waiting while (running.preempted == 0) each at most (C);
  }                               // or: exclusive prefill; admit waiting …
  worker {
    forward (<expr>);               // clock time per iteration, in batch.tokens, … (1: the step clock)
  }
}
pool kv on gpu { … }              // the engine's memory, admitted as soon as it fits
pool reqs on llm { … }            // its slots, admitted by the engine (`on llm.gpu` for memory it admits)
```

## Statements

```serq
turn;                              // draw attributes and wait for the response
while (more) { … }                 // continue while the condition holds
// server { gw.route(); }          // select a gateway outside the workload
Size items = expr;                 // workload request quantity
Cost duration = cost(svc, expr);    // scalar stage cost
Cost processing = { mem: expr, svc: expr }; // independent resource costs
hold mem(processing.mem) { run svc(processing.svc); }
set x = expr;                      // a session attribute
observe name = expr;               // record a sample

hold P (cost(P, u)) [reserve (r)] [, Q (v)]* [reuse (cost(P, ρ))]
     [at admission (name = e, …)]      // names evaluated at admission; body access is restricted
     { … } [cache (ℓ)] [lease P (t)];   // lease: P's units outlive the scope until released, t seconds, or the end
grow P (cost(P, d));                        // enlarge the innermost hold
drop P;                            // discard the own cached prefix
release P;                         // give the enclosing hold's units of P back now, or end a lease of P
load P (cost(P, n));                        // the KV of n tokens arrived: computed position += n

run S [prefill|decode] (w) [growing P];
run S, T (cost(S, T, w), share = share.maxmin); // one job holding ps stages S and T at once (use "std/share")

// the serving vocabulary: the same statements, named by the request lifecycle
tool Z;                            // run on the stage `tool`: Z is time (seconds)
transfer (X) from P to Q (n);      // run link (X); load Q (n); release P
transfer on S, T (X) from P to Q (n);   // uses both links for the transfer
tool[j] Z;  tool on S (Z);         // an instance of an array; an explicit stage

if (e) { … } [else { … }]          // a test: e is 0 or 1
if (e) { … } else if (f) { … }     // f is read only when e is 0
if (~bernoulli(p)) { … } [else { … }] // a draw: with probability p
loop { … }
choose j in n by (k1, …);          // least key over indices 0 through n-1
end;
```

## Expressions

Arithmetic, comparisons (0/1), `&&`, `||`, `!`, `c ? a : b`.

**Functions** `min` `max` `abs` `floor` `ceil` `sqrt` `exp` `ln`

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

A file of its own (no `fn main()`), given when the program runs:
`serq run model.sq --workload workload.sq`.

```serq
// workload.sq: use, let (args.number at the top level), def, its own stages and pools
let rate = args.number("rate", 0.3);
pool live { cap 24; }       // its own: a cap on live sessions, no `on`

workload {
  arrive poisson(rate);     // renewal(~h2(mean, cv2)), closed(n), batch(n), none
  trace "file.csv" [ordered];
  init { … }                // once, at arrival
  turn { … }                // at every `turn` statement
  send (input: K + n, output: o);   // what each request carries
}

// model.sq, in fn main()
server (input, hidden output) reply (done) {   // hidden: the scheduler may not read it
  …                         // reads the parameters; sets `done`, read as `reply.done`
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
6. **`end` keeps the session's cached prefixes.** To have them gone, the
   workload sends `final` on the last request and the server writes
   `if (final) { drop P; }`.
