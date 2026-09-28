# seQ: the text syntax, the semantics, and the vLLM correspondence

The definition of a seQ program is its IR (`docs/ir.md`, `src/ir.rs`).
This document describes the text syntax, which compiles to the IR, and the
semantics of the IR's constructs, written in terms of that syntax. Tools
that know what they want to run build the IR directly (for example the
vLLM oracle scenarios, `tools/oracle/*.ir.json`).

Status: 2026-09-27 (moved into seQ from the research repository
`serving-queue-theory` the same day). Reference implementation: the crate
`seq-lang` at the repository root (Rust: parser, interpreter, CLI
`seq-lang`: `run`, `check`, and `ir` to print a program's IR). Programs:
`programs/*.seq`. Checks: `make check`. The review
of the first version against vLLM, the design decisions and the tooling
survey are in `docs/review.md`.

The formal model is in `serving-queue-theory`, which uses a pinned
release of seQ: `lean/ServingQueueTheory/Seq.lean` (syntax `Route Env V`
of the `session` block, pool semantics, memory invariant, surface syntax),
`SeqExec.lean` (an executable semantics of the pool and step-engine
fragment), `SeqOracle.lean` (the vLLM scheduler scenarios of
`tools/oracle/` as theorems, generated), `SeqServe.lean` (serving order
of a step engine) and `Deployments.lean` (the paper's two replicas as
programs). Other paths under `libqueuingsim/`, `scripts/exp/`,
`data/exp/`, `paper/` and `lectures/` also refer to that repository: its
simulator cross-checks seQ programs against hand-written models
(`libqueuingsim/tests/seq_*.rs`), and its testbed scripts produced the
A100 measurements of Section 8.

## 1. What seQ is for

A serving deployment is a program. The program names the resources of the
deployment (memory pools, stages), says how sessions arrive and how a
session's turns evolve (the workload), and gives the path every session
runs as a session. One program has three uses:

1. **Simulation.** `seq-lang run prog.seq` executes it as a discrete-event
   simulation and reports time averages, per-observation statistics and
   per-turn records. `libqueuingsim` now runs seQ programs next to its
   hand-written models (Section 6).
2. **Formal verification.** The same syntax is an inductive type in Lean
   with an operational semantics; properties of the language (the memory
   invariant of every pool, the shares of a processor-sharing stage) and
   of particular programs (well-formedness of the paper's replica) are
   theorems. Programs can be written in seQ's own syntax inside Lean
   (`[route| ... ]`; the Lean type and its quotation still carry the block's
   former name, `route`, until the companion change lands there).
3. **Specification of production systems.** vLLM v1's engine is a
   50-line program (`programs/vllm.seq`, `vllm_replay.seq`). It
   reproduces the real scheduler request for request: on six
   deterministic scenarios (also on the real A100 engine, and as Lean
   theorems) and on the full 333-session agent trace (3 321 requests,
   every first-token time and cached-token count identical to the real
   scheduler driven by the same clock). With the cost model measured on
   the A100 it predicts the measured runs' hit rates within 1–6 points,
   including where the replica collapses (Section 8).

The organising idea, unchanged from the lecture: a deployment does four
things to a request: makes it **wait for a resource**, **runs** it on a
stage, **frees** the resource (possibly keeping a prefix cached), and
**sends it somewhere next**. v2 makes the first three one scoped
statement (`hold`), generalises "resource" so that KV memory, request
slots and offload tiers are the same kind of object (a pool), and adds
the one stage kind the lecture could not express: the colocated engine
that prefills in the compute its decode step leaves (`step`).

## 2. Syntax

```
program  := item*
item     := let NAME = expr ;
          | pool NAME [ '[' N ']' ] { poolopt* }
          | stage NAME [ '[' N ']' ] : kind ;
          | workload { wlitem* }
          | session block                     -- the session, in one block
          | server block                      -- or its server side, with the session inside workload
          | run { horizon expr ; warmup expr ; seed expr ; }
poolopt  := cap expr ;                       -- capacity in units (default inf)
          | block expr ;                     -- allocate and cache in blocks
          | evict lru ; | evict by ( expr , ... ) ;   -- eviction order (ascending keys)
          | preempt none ; | preempt lifo ;  -- what a failed growth does
          | queue fifo ; | queue by ( expr ) ;        -- admission order
          | admit via STAGE ;                -- the queue is served by a step stage's scheduler
          | spill POOL via STAGE ( expr ) when ( expr ) ;  -- write evicted prefixes to a tier
kind     := fifo [ ( c ) ]                   -- c servers, one job each at rate 1
          | ps ( expr in n )                 -- throughput phi(n) shared equally
          | delay                            -- every job at rate 1, no waiting
          | step { budget expr ; cost expr ; [chunk expr ;]
                   [serve admission ; | serve decode first ; | serve exclusive prefill ;]
                   [memory POOL ;] }
wlitem   := arrive poisson ( rate ) ; | arrive closed ( n ) ; | arrive batch ( n ) ; | arrive none ;
          | trace "file.csv" [ordered] ;      -- replay sessions from a trace
          | init block | turn block          -- only set / observe
          | session block                    -- the session's side; says `request`
stmt     := turn ;                           -- next turn's attributes (workload `turn`, trace)
          | request ;                        -- the server block, once (workload `session` only)
          | set NAME = expr ;
          | observe NAME = expr ;
          | hold POOL ( expr ) [reserve ( expr )] [, POOL ( expr ) [reserve ( expr )]]*
                 [reuse ( expr )] [at admission ( NAME = expr , ... )]
                 block [ cache ( expr ) ] ;
          | grow POOL ( expr ) ;
          | drop POOL ;                      -- discard the own cached prefix
          | run STAGE [prefill | decode] ( expr ) [ growing POOL ] ;
          | branch ( expr ) block [ else block ]          -- a test
          | branch with ( expr ) block [ else block ]     -- a draw, w.p. expr
          | loop block
          | choose NAME in expr by ( expr ) ; -- NAME := argmin over 0..n
          | end ;
          | serving                          -- the serving vocabulary, sugar for hold and run
serving  := enter POOL ( expr ) … block [ keep ( expr ) ] ;   -- as hold … cache
          | admit if POOL ( expr ) [reserve ( expr )] [, POOL ( expr ) [reserve ( expr )]]* fit
                 [reuse ( expr )] [where NAME = expr , ... ] block [ keep ( expr ) ] ;
                                             -- the same, in a server block
          | prefill  [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
          | transfer [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
          | decode   [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
          | tool     [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
```

Expressions: arithmetic, comparisons (0/1), `&&`, `||`, `!`, `c ? a : b`,
`~exp(mean)`, `~det(x)`, `~uniform(lo,hi)`, `~erlang(k,mean)`,
`~h2(mean,cv2)`, `~bernoulli(p)`; `min`, `max`, `abs`, `floor`, `ceil`,
`sqrt`, `exp`, `ln`, `pow`; observables `queue(s)`, `busy(s)`, `work(s)`,
`used(p)`, `free(p)`, `cachedin(p)`, `holders(p)`, `queued(p)`,
`budget_left(step)`, `price(s, s_hit, ds)` (the online price of a miss,
`missPrice` with the stage's measured λ̂, ρ̂, Ŵ), `est_lambda(s)`,
`est_rho(s)`, `est_wait(s)`; context variables `now`, `size`, `age`,
`last`, `queued` (eviction keys and spill predicates), `n` (ps
capacity), `nres`, `ndec`, `kvb`, `kvp` (a step stage's budget, chunk and
cost: the residents before the iteration), `ntok`, `npre`, `attn` (its cost
only: what the iteration scheduled; `attn = Σ n (K + n/2)` over the prefill
chunks, `K` the position before the chunk). Each context variable exists at the one place named in
its parenthesis (`now` everywhere), and reading it anywhere else is a link
error rather than a 0: `set x = ntok;` in a session, or `evict by (ntok)`,
does not link (`docs/ir.md`, Moments). A name may not be both a `let` constant and a session
attribute (the linker rejects it: an attribute would shadow the constant,
and a stage's cost, which has no session, would read it as undefined). Built-in session attributes: `serial`, `turn_no`, `cached` (the
prefix consumed at the last admission), and with a trace `new`, `out`,
`think`, `more`, `forced`. Every name assigned by `set` or `choose` is a
session attribute.

### The serving vocabulary

The statements above are about resources: `hold` a pool, `run` a stage,
free and cache at the end of the scope. A reader from serving systems
expects the request lifecycle (admission, prefill, KV transfer, decode,
release, tool call, next turn) and had to reconstruct it from which stage
a `run` names. The serving forms name it. They are sugar: the parser
rewrites each to the kernel statement it stands for, so the AST, the IR
(`seq-lang ir` prints the kernel), the interpreter and the Lean model know
nothing of them, and every program written with `hold` and `run` is
unchanged.

| Serving form | Kernel |
|---|---|
| `enter P (c) … { body } keep (ℓ);` | `hold P (c) … { body } cache (ℓ);` (`reserve`, `reuse`, several pools: as in `hold`) |
| `prefill S;` | `run prefill (S);`, or on a step engine `E`: `run E prefill (S);` |
| `transfer X;` | `run link (X);` |
| `decode D;` | `run decode (D);`, or on a step engine `E`: `run E decode (D);` |
| `tool Z;` | `run tool (Z);` |
| `prefill (S) growing kv;` | `run E prefill (S) growing kv;` (`growing` passes through; a form never adds it) |
| `prefill[j] S;` | `run prefill[j] (S);` (the index applies to the role's stage array) |
| `prefill on P[j] (S);` | `run P[j] (S);`, or `run P[j] prefill (S);` when `P` is a step engine |

**Which stage.** A form finds its stage among the stages declared above
it (declarations come first in every program here): the stage whose name
is the role's, `prefill`, `link` (or `transfer`), `decode`, `tool`;
failing that, for `prefill` and `decode`, the `step` engine, since prefill
and decode share its iteration. Exactly one must qualify: with none
(`stage svc : fifo;` and `prefill S;`) or several (two step engines) the
parser stops at the form and says so. `on STAGE` names the stage
explicitly; with several instances of a role, `choose j …; prefill[j] S;`
serves an array and `prefill on P2 (S);` stages that are not one. On a
step engine the run gets the role's mode (`run E prefill`), elsewhere it
is plain, so the linker's rule (the mode is required on a step stage and
forbidden elsewhere) is met by construction; `transfer` and `tool` on a
step engine are rejected by the linker as `run E (X)` would be. A linker
error inside a form (an unknown name in `S`, say) speaks of the kernel
statement.

The lecture's disaggregated replica (`programs/lecture_pd.seq`) then reads

```
turn;
loop {
  enter memP (kappa * T) { prefill S; transfer (x0 + kappa * T / Bw); } keep (kappa * T);
  enter memD (kappa * T) { decode (o * w); }
  branch with (p) { tool Z; turn; } else { end; }
}
```

and vLLM's engine (`programs/vllm.seq`, whose `server` block spells the
same hold from the scheduler's side, below)

```
enter reqs (1), kv (min(prompt, hit + budget_left(engine)))
                    at admission (hit = min(cachedin(kv), hitmax)) {
  prefill (prompt - c) growing kv;
  decode (o - 1) growing kv;
} keep (prompt + o);
```

Both compile to the IR they compiled to before the rewrite
(`src/parser.rs` tests, `tests/ir.rs`).

**Against vLLM.** Each form is one part of a request's life in the v1
scheduler (`ref/vllm` at 0c87a197; §7 has the rule-by-rule table):

| Form | In the lifecycle | vLLM |
|---|---|---|
| `admit if reqs (1), kv (hit + …) fit where hit = … { … }` | admission: the waiting request is looked up in the prefix cache and gets the blocks of its first chunk | the waiting loop of `schedule()`, `scheduler.py:868-1128`; `get_computed_blocks`, `kv_cache_manager.py:264-321`; `allocate_slots`, `kv_cache_manager.py:371-608`, called at `scheduler.py:1214` |
| `prefill (n) growing kv` | prefill in chunks of the budget, a block allocated as the request advances; a missing block preempts `running[-1]` | the running loop, `scheduler.py:624-823`; `allocate_slots` at `scheduler.py:743`; `_preempt_request`, `scheduler.py:1539-1582` (`preempt lifo`) |
| `decode (o) growing kv` | one token per iteration, a block every `block_size` tokens | the same loop and `allocate_slots` with one new token |
| `transfer X` | the KV of a prefilled request moves to the decode instance | the KV connector: `WAITING_FOR_REMOTE_KVS` at `scheduler.py:1267`, `_update_waiting_for_remote_kv`, `scheduler.py:3032-3077`, `_connector_finished`, `scheduler.py:2929`; not in `vllm.seq`, which is one device |
| `} keep (prompt + o)` | release: the blocks go to the free queue, the full ones stay cached | `_free_request`, `scheduler.py:2628`; `free`, `kv_cache_manager.py:610-619`; `cache_blocks`, `kv_cache_manager.py:802-812` |
| `tool Z; turn;` | the session thinks and comes back with a longer prompt | outside the engine: the session's next request, `add_request`, `scheduler.py:2536` |
| `end` | the session leaves; its blocks stay in the free queue | `finish_requests`, `scheduler.py:2564` |

### The two sides

A `session` block writes a session's whole life in one place: what the
client does (arrive, think, decide whether to go on) next to what the
deployment does with each request. `programs/vllm.seq` is headed "vLLM v1
on one device", and its session block held three statements vLLM does not
execute — the tool call, the next turn, the exit — and one variable, the
context length `K`, that belongs to the conversation rather than to the
engine. The same program written from its two sides:

```
workload {
  arrive poisson(Lambda);
  init { set K = 0; }
  turn { … }
  session {
    turn;
    loop {
      request;
      set K = prompt + o;
      branch (more) { tool (~exp(Z)); turn; } else { end; }
    }
  }
}

server {
  set prompt = K + n;
  set hitmax = floor((prompt - 1) / bs) * bs;
  admit if reqs (1), kv (min(prompt, hit + budget_left(engine))) fit
        where hit = min(cachedin(kv), hitmax) {
    prefill (prompt - c) growing kv;
    decode (o - 1) growing kv;
  } keep (prompt + o);
}
```

`request;` runs the server once. The parser splices the server's
statements in its place — at any depth, as often as it is written — so the
AST, the IR and everything downstream see the one session they saw before.
The three vLLM programs compile to the IR they compiled to as `session`
blocks (`tests/ir.rs`, `src/parser.rs` tests), and `tools/oracle/*.ir.json`
did not move: the two sides are sugar, at the price of the serving
vocabulary.

Each side owns its words, and the parser holds a program to that, because
a decision written on both sides would be two constructs for one meaning:

| | the session's side (`session` inside `workload`) | the server's side (`server`) |
|---|---|---|
| the next turn, the exit | `turn;`, `end;` | refused: a server is done with a request when its block is |
| the request | `request;` | refused: a server does not request itself |
| admission | `enter P (u), … at admission (x = e) { … } keep (ℓ)` | `admit if P (u), … fit where x = e { … } keep (ℓ)` |
| the kernel | `hold` | `hold` |

`admit if … fit` is the hold `enter … at admission` is, spelled by the
scheduler: the pools listed are the ones that must have room (`used + r ≤
cap`, §3), the units are what the admission takes, and `fit` is the whole
condition. The condition is not an expression on purpose. A free predicate
would part the test from the allocation (a program could admit on 10 units
and take 20, and nothing could check it), would have to be re-evaluated at
every event rather than when a pool changes, and would leave the Lean
fragment; where the test does differ from the allocation, `reserve` says
so by name (vLLM's `scheduler_reserve_full_isl`). The binding clause is
`where` rather than `at admission` because in a server the header *is* the
admission and has no other moment to name; in a `session` the clause still
has to say when.

A `session` at top level stays the kernel form and the one the tutorial
teaches. The two forms are exclusive in one program; a workload's `session`
without a `server`, or a `server` that is never requested, is an error.

## 3. Semantics

**Configuration.** Time; the live sessions with their attributes,
continuation (a stack of block frames), status (ready, queued at a pool,
at a stage, waiting to grow, ended) and active holds; for each pool its
capacity, the allocations of its holders (in admission order), its cache
(entries of units, release time and release order, per session or dead),
its admission queue and its growers; for each stage its jobs.

**Commands take no time**; they run whenever a session is ready, in the
order sessions became ready. Flow lets time pass at the stages. After
every event the interpreter *settles*: it runs every ready session, then
retries growers and admissions at every pool not served by a stage, until
nothing changes; then it starts an iteration on every idle step stage that
has residents or a waiting queue it serves, provided no other event is
pending at the same instant (a scheduler step sees every arrival up to it).

**Pools.** `hold m₁(u₁) reserve(r₁), m₂(u₂) … reuse(ρ) { body } cache(ℓ)`
joins the queue of `m₁`. The unit expressions are evaluated *when the
session is admitted* (the lecture's `[Admit]` evaluates `c(x_r)` then;
observables such as the cache or an engine's budget change while a
session waits). The head of a queue is admitted when every pool of its hold
has room for its `reserve` units next to the allocated units (`used + r ≤ cap`,
`r = max(u, reserve)`; cached prefixes never block). `reserve` is the clause
for "do not let me in until there is room for this", which is separate from
how much the hold then takes; vLLM spells the same rule
`scheduler_reserve_full_isl`; the first that does not
fit blocks the rest (head-of-line blocking). On admission the session
consumes at most `ρ` units of its own cached prefix (`cached :=` what it
consumed); the rest of that entry stays in the cache as a *dead* entry with
the same age, unusable, until evicted; other entries are evicted in the
pool's order until allocations and cache fit; `u` units are allocated and
the body runs. At the end of the body the units are released and
`min(ℓ, computed)` units stay cached, rounded down to blocks (`computed`
is the allocation, or the position a `growing` run reached). The invariant
`allocated + cached ≤ cap` holds in every reachable configuration
(`SeqLang.Step.invariant`). `end` releases every hold but *keeps* the
session's cached prefixes: the cache does not know that a session has left
(lecture `[End]`; vLLM keeps the blocks). A program that models dropping
them writes `drop POOL;` before `end;`. Eviction is per entry, or per block
from the tail of the entry when the pool has `block b`; `evict lru` orders
by release time and then release order, `evict by (k₁, …)` by the keys and
then release order. A request that can never fit is rejected (vLLM
`FINISHED_IGNORED`).

A pool marked `admit via S` is not admitted at settle time: its queue is
served by step stage `S`, at the start of an iteration, after the
residents have taken their tokens, while the iteration has budget left, and
not in an iteration that preempted (vLLM's waiting loop,
`scheduler.py:868-1128`). `budget_left(S)` then evaluates to the budget
left. Until then a waiting session's cached prefix is evictable, which is
the *wait channel* of Lecture 5.

`grow m (d)` enlarges the innermost hold on `m` by `d` (rounded to
blocks). If it does not fit: with `preempt none` the session waits and
resumes where it was; with `preempt lifo` the most recently admitted
holder is preempted (vLLM `running[-1]`): its job leaves its stage, its
hold is released with its computed prefix cached, and it re-enters the head
of the pool's queue with the hold statement to execute again. The grower
itself can be the victim.

**Stages.** `fifo(c)`: `c` servers, jobs in arrival order at rate 1.
`ps(φ)`: every job at once, each at `φ(n)/n`. `delay`: every job on its
own at rate 1. `step { budget B; cost C; }`: an engine that runs
iterations. The residents are served in the order `serve` names, one order
per stage: `admission`, the order their sessions were admitted (vLLM's
`running` list; the default), `decode first`, the decoding residents before
the prefilling ones, or `exclusive prefill`, below. One token to a decoding
job, up to `chunk` to a prefilling one,
until the budget is spent; a `growing` job first grows its hold to the
position it will reach (block by block, preempting if needed); then the
stage admits from the queues it serves. The iteration lasts `C` seconds, an
expression in `ntok`, `ndec`, `npre`, `nres`, `kvb`, `kvp`, `attn`; its
tokens are applied when it ends. A run of zero work completes at once. An
iteration that schedules no token is not an iteration, unless it preempted:
then it is the scheduler step that only preempted (vLLM's `schedule()`
admits nothing in a step with `preempted_reqs`, `scheduler.py:869`, and the
oracle driver counts the step; the Lean model's `startIteration` returns
the empty iteration and its `tick` re-admits at the next one), and the next
iteration re-admits the victim. It lasts `C` at zero tokens, which is a
modelling choice: the real engine skips the forward pass of an empty step,
so the fixed part of `C` overstates it. Before this rule the interpreter
dropped that step, against the Lean model, and with the victim queued and
no event left the run ended with a session in the queue. A hold whose body
can never fit then preempts itself forever; vLLM never runs that program,
since it refuses at start-up a KV cache that cannot hold one request of
`max_model_len` (`kv_cache_utils.py:965`), a check seQ does not have, which
is what the `stuck` counter below is for.
`serve exclusive prefill` schedules only the first prefilling resident while
one exists (the RBLN stack). Without a per-request chunk cap, serving in
admission order *is* serving decode-first (`SeqLang.Serve.serve_eq_decode_first`;
a cap breaks it, `chunk_cap_breaks_shape`), which is why the paper's
"prefill from the budget decode leaves" describes vLLM too.

**`at admission`.** Everything in a hold's header — the units, `reserve`,
`reuse` — is evaluated when the session is admitted, and a `set` above the
hold is not (`cache` is read when the session releases, `SeqExec.lean`'s
`release` and the interpreter agree). The two look the same, which is how
`programs/vllm.seq` came to read its prefix cache at the moment the session
queued rather than the moment the scheduler took it. `at admission (hit = e)`
gives the header a place to name what it is written in terms of:

```
enter reqs (1), kv (min(prompt, hit + budget_left(engine)))
      at admission (hit = min(cachedin(kv), hitmax)) { … }
```

The bindings are substituted into the header's expressions by the parser, so
the AST, the IR, the interpreter and the Lean model know nothing of them, and
a program that uses the clause has the IR of the one that inlines by hand. A
later binding sees the earlier ones. A binding may not draw (`~`): it is
substituted, so a name used twice would draw twice.

**`enter`, `admit if` and `admit via`.** The scheduler admits; the session
enters. The statement is named from the side it is written on: `enter` in
a `session` block, like every other statement there, and `admit if … fit`
in a `server` block (§2, the two sides), where the scheduler is the one
speaking. `admit` is also the name of the *pool option* that hands a queue
to a stage's scheduler (`admit via S`), the scheduler's side again.

**Branching.** `branch (e)` takes the first block when `e` is non-zero.
`branch with (p)` takes it with probability `p`, and is sugar the parser
rewrites to `branch (~bernoulli(p))` — the IR, the interpreter and the Lean
model know only the one form. The two spellings are not interchangeable to a
reader and were not distinguishable before: a guard strictly between 0 and 1
has always been drawn as a probability (a guard of 0 or 1 consumes no draw, a
fractional one exactly one, from the session's stream). Write the draw as
`branch with` so that the program, and the figure, say which one it is.

**Workload.** `init` runs at arrival, `turn` at every `turn` statement;
with a `trace`, `turn` loads the next turn's `new`, `out`, `think`,
`forced` and sets `more` (`ordered`: session `i` replays trace session `i`).
Random draws use separate streams for arrivals, workload, the session and
eviction.

**Lints.** Linking rejects two programs that are well formed and almost
certainly not what their author meant. A `set` that reads live pool or stage
state (`cachedin`, `budget_left`, `used`, …) and is then used in a hold's
header: the header is read at admission and the `set` is not, so the value
reaching the header is the one from before the session queued — `at admission`
is the clause for it. And a `branch` whose guard is a constant strictly
between 0 and 1: that is a draw, and `branch with` is how to say so. Both are
errors rather than warnings; neither has a legitimate instance in
`programs/`, and a warning nobody acts on is worse than no check.

**Statistics.** `observe x = e` records a sample after warm-up with the
time, session and turn (`--dump DIR` writes them); the report gives per
stage the time-average number present, utilisation, throughput, mean wait
and service, and per pool the time-average used, cached, queue and
holders, the mean queue wait, admissions, evictions, preemptions, spills,
rejections and `stuck`: sessions preempted a second time without having
advanced past the position of their previous preemption. A hold that fits at
admission but can never grow to what its body needs (`prompt + out > cap`
under `preempt lifo`) preempts itself and re-executes forever; the run would
otherwise end at the horizon with nothing but a preemption count, and the
report now names the livelock.

**Executable semantics in Lean.** `SeqExec.lean` defines the same rules
for the fragment of pools and one step engine on the step clock (values in
ℕ): `Exec.run` interprets a `Route Env ℕ` program for `n` sessions (the Lean
type keeps the block's former name for now). It is
the semantics the oracle theorems are about.


## 4. From the lecture's version to v2

| Lecture (L1:def:syntax, L1:def:semantics) | v2 | Why |
|---|---|---|
| `admit m c` … `free m [cache ℓ]` as separate actions | `hold m (c) { … } cache (ℓ)` | balance is syntactic; preemption is "abort the scope"; the memory invariant is provable per command |
| one pool kind (KV bytes) | pools are counted resources with an optional cache: KV, request slots (`max_num_seqs`), live-session caps, offload tiers | the same guard and queue serve all of them; the lecture's "slots" remark becomes a pool (`reqs`, one unit per running request) |
| `serial`, `shared(φ)`, `external` | `fifo(c)`, `ps(φ)`, `delay`, **`step`** | the colocated engine (Exercise L1:exr:colocated) and vLLM's chunked prefill |
| hit indicator `H ∈ {0,1}` | `cached` (units found), block-rounded | partial hits (block eviction, tail first) |
| eviction order `E` as a name | `evict lru` / `evict by (keys)` with online estimates | the priced orders of §3 are expressible |
| no growth, no preemption | `grow`, `growing`, `preempt lifo` | decode grows the KV; vLLM preempts |
| `branch_p` with a constant | `branch (expr)` tests, `branch with (expr)` draws | traces decide continuation; the probability keeps a spelling of its own |
| no measurement | `observe`, `--dump` | TTFT and the price are defined in the program |
| no routing | stage arrays and `choose` | §3.2 |

The lecture's disaggregated replica is `programs/lecture_pd.seq` and,
in Lean, `SeqLang.disaggregatedReplica`; the paper's colocated
two-resource replica is `programs/replica.seq` and
`SeqLang.colocatedReplica`.

## 5. Programs

| Program | Deployment | Checked against |
|---|---|---|
| `mg1.seq`, `ps.seq`, `closed.seq` | M/G/1 FIFO, M/G/1-PS, M/M/1//N | closed forms (`seq_closed_forms.rs`): M/M/1 sojourn, PK for four laws, PS insensitivity, MVA |
| `agentic.seq` | `models::agentic` (one FIFO replica, finite KV, SF eviction) | hand-written model, throughput within 3 %, hit rate within 0.5 pt, response within 3 % (`seq_agentic.rs`) |
| `replica.seq` | the paper's two-resource replica (`TwoStage`) on the open-session scenario (`serve decode first`, `drop kv` before `end`) | no memory limit: TTFT 0.253 vs 0.250 s, response 0.336 vs 0.333 s; 20 seeds at 16 and 20 live sessions: hit rate, TTFT and throughput agree (Mann–Whitney p ≥ 0.05); at 24 live sessions the iteration-level engine has 10 % lower throughput and twice the mean TTFT (p = 0.017, 0.047), hit rate 0.80 vs 0.86 (p = 0.11); no seed of either engine falls below a 0.5 hit rate (`data/exp/seq/replica_seeds.csv`, `seq_replica_and_pd.rs`) |
| `pd_tandem.seq`, `lecture_pd.seq` | tandem PD, the lecture's disaggregated replica | capacity formulas within 2 %; stability |
| `routing.seq` | four replicas, five policies | `models::routing` within 1–2 % on response and hit rate |
| `vllm.seq` | vLLM v1 engine (Section 7) | scheduler semantics tests, the upstream oracle |
| `vllm_request.seq` | one vLLM v1 request on the step clock; compiled per scenario to `tools/oracle/*.ir.json` | the six upstream oracle scenarios (`tests/vllm_oracle.rs`), the Lean theorems generated from the same IR |
| `vllm_replay.seq` | vLLM v1 on the A100 testbed replaying the short-context trace | ten measured runs (Section 8) |

## 6. The simulator uses seQ

`libqueuingsim` depends on `seq-lang`; `libqueuingsim/tests/seq_*.rs`
run the programs above under `make sim` next to the hand-written models.
The hand-written models stay as the second implementation the programs
are checked against; new scenarios should be written as programs.

## 7. vLLM v1 as a seQ program

`programs/vllm.seq` and `programs/vllm_replay.seq` (upstream `ref/vllm`
at 0c87a197; the A100 testbed runs vLLM 0.30.0, whose scheduler gives
identical answers on the differential scenario below):

| vLLM | seQ | Where |
|---|---|---|
| a token budget per step, running requests first in `running` order, then waiting requests with the budget left | `step { budget B }`, residents in admission order; `pool reqs { admit via engine; }` | `scheduler.py:577, 624-823, 868-1128` |
| `max_num_seqs` | `pool reqs { cap max_seqs }` in the hold | `scheduler.py:877-879` |
| FCFS, head-of-line blocking (`if new_blocks is None: break`) | pool queue `fifo`; the first request that does not fit blocks | `scheduler.py:1228-1235` |
| admission needs blocks for the whole prompt (`scheduler_reserve_full_isl = True`), but only the first chunk is allocated | `kv (hit + min(prompt − hit, budget_left(engine))) reserve (prompt)` | `kv_cache_manager.py:515-531`, `config/scheduler.py:191` |
| a waiting request's prefix is looked up and its blocks touched only when it is scheduled | units evaluated at admission; the queue served by the engine | `scheduler.py:932-939`, `block_pool.py:754-770` |
| chunked prefill, `long_prefill_token_threshold` | `prefill (n) growing kv` (`run engine prefill (n) growing kv`), `chunk` | `scheduler.py:612-616, 675-676, 1115-1128` |
| `allocate_slots` block by block as the request advances | `growing kv` | `kv_cache_manager.py:371-608` |
| preemption of `running[-1]`, `waiting.prepend_request`, `num_computed_tokens = 0`, no admission in a step that preempted | `preempt lifo`, re-queued at the head, hold re-executed; `admit via` skips preempting iterations | `scheduler.py:742-813, 869, 1539-1582` |
| the prefix cache holds every *computed* full block, generated tokens included; a hit is the longest run of cached full blocks, at most `num_tokens − 1` | `cache (prompt + out − 1)`; `reuse (floor(min(prev prompt, prompt − 1)/bs)·bs)`; the unmatched blocks stay cached, dead | `kv_cache_manager.py:289-300, 602-606`, `single_type_kv_cache_manager.py:743-838` |
| the free queue: freed blocks appended tail first (LRU), in the order requests finish | `evict lru` per block from the tail, ties by release order | `block_pool.py:776-805`, `single_type_kv_cache_manager.py:557-585` |
| a finished session's blocks stay in the free queue | `end` keeps the cache | `block_pool.py:776-805` |
| a forced miss (a nonce at the head of the prompt) matches nothing; the old blocks stay | `reuse (0)` | trace |

Not modelled: the watermark (0 by default), the "alone" exception of the
long-prefill threshold, encoder inputs, speculative decoding, sliding
window, cross-session prefix sharing (out of scope), asynchronous
scheduling (Section 8).

**Admission.** The header of `admit if` in `programs/vllm.seq` is the
prefix-cache lookup and the allocation of the first chunk, and both happen
when the scheduler admits the request, not when it queues. The hit is the
cached *full blocks* of the prompt, never all of it, since the last token
is recomputed for its logits: `hitmax = floor((prompt − 1) / bs) · bs`.
`cachedin(kv)` is read in the header, so it is read in the waiting loop
(`scheduler.py:932-939`), and until then a waiting request's prefix is
still evictable — which is the point of the wait channel, and the moment
the first version of the program got wrong (§3). The units are the hit
plus the chunk the budget the running requests leave can take now,
`min(prompt, hit + budget_left(engine))` (`scheduler.py:1078-1128,
1214-1226`): the whole prompt, or as far as the hit and the budget reach,
whichever is less. The rest is allocated as the request runs
(`growing kv`).

**How the correspondence is checked.** Three oracles, all agreeing:

1. *Deterministic scenarios* (`tools/oracle/*.json`): the real
   scheduler driven by a fake model runner (`vllm_oracle.py`), the real
   A100 engine with Qwen3-8B stepped by hand (`a100_engine.json`,
   `scripts/exp/lambda/seq_cases.py`), the seQ program
   (`tests/vllm_oracle.rs`) and the Lean executable semantics
   (`SeqOracle.lean`, one theorem per scenario, `decide +kernel`) give
   the same first-token step, last-token step and preemption count for every
   request (6 scenarios: self-preemption, chunked prefill sharing the
   budget, the request cap, head-of-line blocking, the chunk cap, six mixed
   requests with staggered arrivals on 39 blocks).
2. *The trace at full scale* (`vllm_replay_oracle.py`,
   `scripts/exp/diff_seq_vllm.sh`): the real scheduler and KV-cache
   manager replay the 333-session short-context trace with the trace's
   token ids, on the same clock as seQ. seQ and the scheduler agree on
   every request's send time, first-token time and cached tokens: 3 321 of
   3 321, for a constant step cost and for the A100 cost model, on the
   base and the forced-miss traces, and on 40-session runs with 1 000,
   1 500 and 3 000 blocks (both deadlock at the same step on 1 000 blocks).
   The same scenario on vLLM 0.30.0's scheduler gives the same answers.
3. The search for the first differing step (`first_divergence.sh`) found
   the six semantic differences the first version of the program had
   (`docs/review.md` §3).

## 8. vLLM on the A100 testbed

`programs/vllm_replay.seq` replays the short-context trace of
`docs/testbed-gpu.md` (Qwen3-8B, block 16, budget 512, `max_num_seqs` 64,
128 160-token pool, prefix caching; session `i` sent at `i·spacing`, turn
`k+1` `think` seconds after turn `k`).

**Engine cost, measured.** 3 022 steps of the A100 engine stepped by hand
(decode batches of 1–64 at contexts 256–32k, prefill chunks at contexts
0–32k; `tools/a100/steps.jsonl`, `scripts/exp/lambda/seq_cases.py`)
fit `c + d·ndec + e·kvb + a·npre + b·attn` with MAPE 2.7 % (decode), 5.6 %
(prefill), 5.7 % (mixed): c = 13.9 ms, d = 41 µs, e = 0.138 µs, a = 51.5
µs, b = 4.02 ns (`tools/a100/step_fit.json`). The same `a` and `b`
explain the light-load one-chunk TTFTs of the served runs (slope 74 µs per
new token ≈ a + b·K̄).

**Why the first version under-predicted misses.** Not timing: the real
scheduler replayed on seQ's clock loses *more* prefixes than the
testbed. The first program pinned waiting requests' prefixes, cached only
prompts, dropped finished sessions' blocks and differed in three smaller
rules (§7, `docs/review.md` §3). With those fixed, the program and
the scheduler agree request for request.

**Two overhead constants.** What the served path adds (asynchronous
scheduling overlaps CPU work with the GPU; the API server tokenises the
text prompt) is two constants, `c_it` per step and `c0` per request.
Fitted on the two light-load runs only (`scripts/exp/calibrate_seq.py`,
grid 4–14 ms × 0–40 ms, `data/exp/seq/calibration.txt`: c_it = 4 ms,
c0 = 40 ms), the held-out runs are predicted as follows:

| run | TTFT measured / model (s) | full-hit measured / model |
|---|---|---|
| 5 s (fit) | 0.177 / 0.168 | 0.966 / 0.947 |
| 4.2 s (fit) | 0.236 / 0.264 | 0.935 / 0.878 |
| 3.5 s, two seeds | 0.507, 0.473 / 0.422 | 0.839, 0.840 / 0.821 |
| 3.0 s | 0.473 / 0.605 | 0.828 / 0.775 |
| 2.5 s (collapsed) | 34.6 / 39.1 | 0.216 / 0.192 |
| 4.2 s, 10 % forced | 0.627 / 0.576 | 0.769 / 0.732 |
| 3.5 s, 10 % forced, two seeds | 0.886, 1.122 / 0.795, 0.838 | 0.736, 0.724 / 0.712, 0.713 |
| 2.5 s, 10 % forced (collapsed) | 41.8 / 49.6 | 0.089 / 0.104 |

The ridge is flat: on a wider grid the light-load optimum moves to c_it = 0,
c0 = 60 ms, which fits the light runs better (mean |log ratio| 0.033 against
0.084) and the loaded runs worse (2.5 s: 14 s against 35 s measured). The
light-load data alone do not identify the split; the served step trace
(below) is what identifies it.

**The measured replica is on the cliff at 3 s.** The 3.0 s run of
2026-09-26 did not collapse (full-hit 0.83, TTFT 0.47 s); the rerun of
2026-09-27 with per-iteration logging on (which slowed the 3.5 s run by
30 %: TTFT 0.68 against 0.51 s) collapsed (full-hit 0.20, TTFT 32 s). The
same workload on the same engine flips with a small change in per-step
cost, as the feedback of Lecture 5 predicts at the edge.

**Hypothesis H-pin, on the real scheduler.** Pinning a waiting request's
cached prefix at arrival (`scripts/exp/lambda/steptrace/pinpatch.py`,
released once the scheduler admits it) and replaying the trace at 3.0 s
through the real scheduler on the A100 cost clock: full-hit 0.43 → 0.80,
mean TTFT 20 s → 0.72 s (`--pin` of `vllm_replay_oracle.py`). seQ
predicted the same with a one-line change of the program (no `admit via`):
0.80.

**Served step trace** (`scripts/exp/lambda/steptrace/sitecustomize.py`:
the time of every `schedule()` and `update_from_output()` of the served
engine with the step's composition; `data/exp/gpu_seq/trace/`). With
this light tracer instead of the stats logger the 3.0 s replay does not
collapse: full-hit 0.832, mean TTFT 0.441 s (2026-09-26: 0.828, 0.473 s),
so the logger's overhead is what pushed the earlier rerun over the cliff.
Median served step periods are 18.7 ms (decode only) and 42 ms (a
512-token prefill chunk), against 14 ms and 41–50 ms stepped by hand; a
regression of served periods on the step quantities is too noisy to
identify the constants (MAPE 25–56 %), so the calibrated pair
(c_it, c0) = (4 ms, 40 ms) is an effective light-load calibration, not a
decomposition of the served path.

**Pre-registered predictions** (`data/exp/seq/prereg/predictions.txt`,
written before the traced runs finished; the pinned variant is the same
program without `admit via engine`):

| run | predicted TTFT / full-hit | measured TTFT / full-hit |
|---|---|---|
| 3.0 s, vLLM rule | 0.605 s / 0.775 | 0.441 s / 0.832 |
| 3.0 s, pinned | 0.483 s / 0.792 | 0.413 s / 0.838 |
| 2.5 s, pinned | 0.888 s / 0.752 | 0.878 s / 0.784 |
| 2.5 s, vLLM rule | 39.1 s / 0.192 | 34.6 s / 0.216 (2026-09-26) |

**H-pin holds on the served A100 engine.** With the waiting request's
prefix pinned the 2.5 s replay does not collapse: mean TTFT 34.6 s → 0.88 s,
full-hit 0.22 → 0.78; seQ predicted 0.89 s / 0.75 before the run. At
3.0 s, where the vLLM rule does not collapse under the light tracer,
pinning changes little (0.441 → 0.413 s, 0.832 → 0.838), as predicted
(0.605 → 0.483 s). Caveat: the unpinned 2.5 s run is from 2026-09-26
without the step tracer; the tracer adds per-step cost, which works against
the pinned run, so the comparison is conservative. One run per point.

## 9. Known limitations

* Continuous work and fluid rates at `fifo`/`ps`/`delay` stages; the
  `step` stage is discrete. The paper's `TwoStage` fluid server is the
  step stage with the budget filled to the memory time; at ω = 0.2 ms
  that is 5 000 iterations per simulated second, so long horizons are
  slow (a `fluid` option for the step stage is the natural extension).
* Cache entries are per session; cross-session prefix sharing (a common
  system prompt) needs a content-addressed cache, not written yet.
* One eviction order per pool; the priced orders use `price(stage, …)`
  with the stage's online estimates, as `libqueuingsim` does.
* The Lean model covers the syntax, the pool semantics and the stage
  rates; the step stage and the session-level semantics (continuations,
  flow) are not formalised yet, and there is no proof that the Rust
  interpreter implements the Lean relation (the vLLM oracle and the
  closed-form checks are the evidence; `docs/review.md` §4 lists
  the tools that would close this gap).
