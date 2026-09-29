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
          | queue NAME [ '[' expr ']' ] [ : ROLE [, ROLE]* ] { qitem* }   -- a station: its pools, stage and entries (below, *Queues*)
          | run { horizon expr ; warmup expr ; seed expr ; }
qitem    := pool NAME { poolopt* }              -- the queue's own; only its entries hold it
          | serve kind ;                        -- the queue's stage, named after the queue
          | VERB [ ( NAME, ... ) ] [ from NAME ] block   -- an entry of one of the queue's roles
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
                   [serve admission ; | serve by ( expr , ... ) ; | serve decode first ;
                    | serve exclusive prefill ;]
                   [memory POOL ;] }
wlitem   := arrive poisson ( rate ) ; | arrive closed ( n ) ; | arrive batch ( n ) ; | arrive none ;
          | trace "file.csv" [ordered] ;      -- replay sessions from a trace
          | init block | turn block          -- only set / observe
          | session block                    -- the session's side; says `request`
          | hidden NAME [, NAME]* ;           -- the scheduler may not read these
stmt     := turn ;                           -- next turn's attributes (workload `turn`, trace)
          | request ;                        -- the server block, once (workload `session` only)
          | set NAME = expr ;
          | observe NAME = expr ;
          | hold POOL ( expr ) [reserve ( expr )] [, POOL ( expr ) [reserve ( expr )]]*
                 [reuse ( expr )] [at admission ( NAME = expr , ... )]
                 block [ cache ( expr ) ] [ lease POOL ( expr ) ] ;
                                             -- lease: that pool's allocation outlives the scope,
                                             -- until a release/transfer takes it, expr seconds, or the end
          | grow POOL ( expr ) ;
          | drop POOL ;                      -- discard the own cached prefix
          | release POOL ;                   -- give the enclosing hold's allocation on POOL back now, or end a lease of it
          | load POOL ( expr ) ;             -- the KV of expr tokens arrived: the enclosing hold's computed position advances
          | run STAGE [prefill | decode] ( expr ) [ growing POOL ] ;
          | branch ( expr ) block [ else block ]          -- a test
          | branch with ( expr ) block [ else block ]     -- a draw, w.p. expr
          | loop block
          | choose NAME in expr by ( expr ) ; -- NAME := argmin over 0..n
          | end ;
          | QUEUE [ '[' expr ']' ] . VERB ( expr, ... ) [ from QUEUE [ '[' expr ']' ] ] [ to POOL ( expr ) ] ;
                                             -- a queue's entry, in its place (*Queues*)
          | mark NAME ;                        -- in an entry: the moment, read by the caller as QUEUE.NAME
          | serving                          -- the serving vocabulary, sugar for hold and run
serving  := enter POOL ( expr ) … block [ keep ( expr ) ] [ lease POOL ( expr ) ] ;   -- as hold … cache
          | admit if POOL ( expr ) [reserve ( expr )] [, POOL ( expr ) [reserve ( expr )]]* fit
                 [reuse ( expr )] [where NAME = expr , ... ] block [ keep ( expr ) ] ;
                                             -- the same, in a server block
          | prefill  [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
          | transfer [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
          | transfer [ '[' expr ']' | on STAGE ] expr from POOL to POOL ( expr ) ;
                                             -- the KV moves: run link; load; release
          | decode   [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
          | tool     [ '[' expr ']' | on STAGE ] expr [ growing POOL ] ;
```

Expressions: arithmetic, comparisons (0/1), `&&`, `||`, `!`, `c ? a : b` (a
non-zero operand is true; only a `branch` guard is held to 0 or 1),
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
chunks, `K` the position before the chunk), `decoding`, `admission`,
`remaining` (a step stage's `serve by` keys, per resident; the keys read the
residents' four as well). Each context variable exists at the one place named in
its parenthesis (`now` everywhere), and reading it anywhere else is a link
error rather than a 0: `set x = ntok;` in a session, or `evict by (ntok)`,
does not link (`docs/ir.md`, Moments). A name may not be both a `let` constant and a session
attribute (the linker rejects it: an attribute would shadow the constant,
and a stage's cost, which has no session, would read it as undefined). Built-in session attributes: `serial`, `turn_no`, `cached` (the
prefix consumed at the last admission), `computed` (the position the hold
had computed when it was preempted, 0 otherwise; below), and with a trace
`new`, `out`, `think`, `more`, `forced`. Every name assigned by `set` or
`choose` is a session attribute.

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
| `prefill W;` | `run prefill (W);`, or on a step engine `E`: `run E prefill (T);` |
| `transfer X;` | `run link (X);` |
| `transfer (X) from P to Q (n);` | `run link (X); load Q (n); release P;` — the KV of `n` tokens moves from the session's lease (or hold) on `P` to its hold on `Q`: the link takes the time, the tokens count as computed at `Q`, and `P` is given back (below, *A KV transfer*) |
| `decode W;` | `run decode (W);`, or on a step engine `E`: `run E decode (T);` |
| `tool Z;` | `run tool (Z);` |
| `prefill (T) growing kv;` | `run E prefill (T) growing kv;` (`growing` passes through; a form never adds it) |
| `prefill[j] W;` | `run prefill[j] (W);`, or `run prefill[j] prefill (T);` when the array is step engines (the index applies to the role's stage array) |
| `prefill on P[j] (W);` | `run P[j] (W);`, or `run P[j] prefill (T);` when `P` is a step engine |

The argument is work in the unit of the stage it runs on, and the two
metavariables say which: `W` is the time the job takes alone on a `fifo`,
`ps` or `delay` stage (seconds, when the program's clock is seconds; a `ps`
stage serves it at `φ(n)/n`), `T` is tokens on a step engine, the unit of
its `budget`. The same form takes either; the Which-stage rule below
decides.

**Which stage.** A form finds its stage among the stages declared above
it (declarations come first in every program here): the stage whose name
is the role's, `prefill`, `link` (or `transfer`), `decode`, `tool`;
failing that, for `prefill` and `decode`, the `step` engine, since prefill
and decode share its iteration. Exactly one must qualify: with none
(`stage svc : fifo;` and `prefill W;`) or several (two step engines) the
parser stops at the form and says so. `on STAGE` names the stage
explicitly; with several instances of a role, `choose j …; prefill[j] W;`
serves an array and `prefill on P2 (W);` stages that are not one. On a
step engine the run gets the role's mode (`run E prefill`), elsewhere it
is plain, so the linker's rule (the mode is required on a step stage and
forbidden elsewhere) is met by construction; `transfer` and `tool` on a
step engine are rejected by the linker as `run E (X)` would be. A linker
error inside a form (an unknown name in `W`, say) speaks of the kernel
statement.

The lecture's disaggregated replica (`programs/lecture_pd.seq`) then reads
(`T`, `S` and the rest are that program's own constants):

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

**A KV transfer.** The lecture's replica above holds the prefill
instance's memory through the transfer and queues for the decode instance's
afterwards: a store-and-forward link with a buffer nobody has. A NIXL
transfer between two vLLM instances has no buffer: the decode instance
allocates the prompt's blocks *first*, the bytes are read into them, and the
prefill instance frees its copy *after*. The prefiller's blocks outlive the
request's scope — its slot is freed when the token is sampled, its blocks
are *leased* until the decoder has read them — which is what `lease` says:

```
admit if reqsP (1), kvP (…) fit … {
  prefill on P (prompt - c) growing kvP;
} keep (prompt) lease kvP (inf);       // finished on P: the slot goes, the blocks wait for the decoder's read
admit if kvD (prompt) reserve (prompt), reqsD (0) reserve (1) fit … {
  transfer (x0 + (prompt - c) / Bw) from kvP to kvD (prompt - 1 - c);   // takes the lease
  admit if reqsD (1) fit { prefill on D (1) growing kvD; decode on D (o - 1) growing kvD; }
} keep (prompt + o);
```

`lease P (t)` names one of the hold's pools whose allocation stays the
session's after the scope's end, neither evictable nor a preemption
victim, until the session's `release P` (a `transfer … from P` contains
one), `t` seconds, or the session's end, and then `keep` applies. vLLM's
prefiller leases for 30 s and the decoder's heartbeats renew it while the
request waits, so `inf` is the served behaviour and `30` a prefiller
nobody heartbeats. `release P` with a hold on `P` gives the innermost
enclosing hold's allocation there back now, caching per that hold's `keep`,
and the scope's end then has nothing left there. `load Q (n)` says the KV
of `n` tokens arrived from outside the engine: the enclosing hold's
computed position on `Q` advances by `n` (within its allocation), as a
`growing` run's would token by token, so `keep` and `cached` count them.
`transfer (X) from P to Q (n)` is the two around the link run.
`programs/llmd_pd.seq` is the whole path, and `docs/case-study-pd.md` its
line-by-line correspondence with llm-d and the NIXL connector.

**Against vLLM.** Each form is one part of a request's life in the v1
scheduler (`ref/vllm` at 0c87a197; §7 has the rule-by-rule table):

| Form | In the lifecycle | vLLM |
|---|---|---|
| `admit if reqs (1), kv (hit + …) fit where hit = … { … }` | admission: the waiting request is looked up in the prefix cache and gets the blocks of its first chunk | the waiting loop of `schedule()`, `scheduler.py:868-1128`; `get_computed_blocks`, `kv_cache_manager.py:264-321`; `allocate_slots`, `kv_cache_manager.py:371-608`, called at `scheduler.py:1214` |
| `prefill (n) growing kv` | prefill in chunks of the budget, a block allocated as the request advances; a missing block preempts `running[-1]` | the running loop, `scheduler.py:624-823`; `allocate_slots` at `scheduler.py:743`; `_preempt_request`, `scheduler.py:1539-1582` (`preempt lifo`) |
| `decode (o) growing kv` | one token per iteration, a block every `block_size` tokens | the same loop and `allocate_slots` with one new token |
| `} keep (prompt) lease kvP (inf)` on the prefiller's hold, then `transfer (X) from kvP to kvD (n)` inside the decoder's | the KV of a prefilled request moves to the decode instance: the prefiller's blocks wait, the decoder allocates and reads, the prefiller frees | the KV connector, `programs/llmd_pd.seq`: the decoder parks the request at `scheduler.py:1264-1294` (`WAITING_FOR_REMOTE_KVS`), its blocks allocated for the whole prompt; the read done, `_update_waiting_for_remote_kv`, `scheduler.py:3032-3077`; the prefiller keeps its blocks leased at `_connector_finished`, `scheduler.py:2929-2982`, and frees them at `scheduler.py:3135-3138` |
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
  hidden o;
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

### Queues

A request travels through stations, and each station is a queue: a waiting
line, an admission, a service, memory of its own. `programs/llmd_pd.seq`
has four kinds — the router, the prefill instances, the NIC, the decode
instances — and a `server` block writes them as pools, stages and `admit if`
at the call site, so that vLLM's admission appears three times in a program
about the router. A `queue` declares one station whole:

```
queue P[NP] : prefill {
  pool reqs { cap max_seqsP; admit via P; }
  pool kv { cap blocksP * bs; block bs; evict lru; preempt lifo; }
  serve step { budget B; cost …; memory kv; }
  prefill (prompt) {
    admit if reqs (1), kv (min(prompt, hit + budget_left(P))) reserve (prompt) fit
          where hit = min(cachedin(kv), floor((prompt - 1) / bs) * bs) {
      set c = cached;
      prefill (prompt - c) growing kv;
    } keep (prompt) lease kv (inf);
  }
}
```

The pools are the queue's (`P.kv` from outside, `kv` within), the stage is
named after the queue (`admit via P`, `budget_left(P)`, `work(P[i])`), and
an *entry* — one per verb of the queue's roles — holds what the station
does with one request. The body is the server's statements; `run (X)` with
no stage names the queue's own; `self` is the member's index in a family.
A family's size may be a `let` constant (`queue D[ND]`).

Four roles are the vocabulary, and a queue declares which it plays:

| Role | Entries | The queue |
|---|---|---|
| `gateway` | `route { … }` | where `request;` enters: its body is the server; one per program |
| `prefill` | `prefill (prompt)` | computes the prompt; how it leaves the KV (`lease`, `keep`, a transfer) is the entry's |
| `decode` | `decode (prompt)`, `decode (prompt) from Q` | a local prefill, or with the KV `Q`'s entry leased for this request |
| `link` | `transfer (n)` | the NIC: the body is the time to read `n` tokens |

The deployment calls an entry where the request goes: `P[i].prefill
(prompt);`, `D[j].decode (prompt) from P[i];`, and inside the decoder
`nic[self].transfer (prompt - c) from src to kv (prompt - 1 - c);`, which
is the kernel's `run nic[j] (…); load D.kv[j] (…); release P.kv[i];`.
`from P[i]` is the pool `P`'s entry leases; a `from` on a queue that leases
nothing does not link. Arguments are substituted like a `where` binding, so
none may draw.

An entry sees its own. Its header — the units, `reserve`, `reuse`, the
`where` bindings — reads the parameters, the queue's pools and stage and the
constants; its body also reads `now`, `cached` and the request's `hidden`
attributes, and nothing else the session has set. That is the `hidden` rule
at the queue's boundary: vLLM's scheduler knows `max_tokens`, not the length,
so `o` is not a parameter of `decode` and the body alone reads it. What the
body `set`s is the queue's (`D.known`); what it `observe`s is the program's;
a moment the caller needs is `mark first_token;`, read afterwards as
`D[j].first_token`. The gateway is the exception on the session's side: its
`route` reads the request's attributes as a `server` does and sets the ones
the session reads back (`prompt`).

Everything here is the parser's. A queue's pools and stage are the program's
under their long names, an entry call is its body in place, `mark` is a
`set`, and the linker, the IR and the interpreter see the program the
`server` form compiled to. `stage` and top-level `pool` remain the kernel's
forms; the client's `tool` is a stage, not a queue.

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
is the allocation, or the position a `growing` run or a `load` reached).
`release m` inside the body does the same for `m` alone, at that point:
the innermost enclosing hold on `m` gives its allocation there back,
caching per its clause, and holds `m` no longer. A hold's `lease m (t)`
keeps its allocation on `m` past the scope's end, neither evictable nor a
preemption victim, until the session's `release m`, `t` seconds, or the
session's end, and `cache` applies then; a `release m` outside any hold on
`m` ends the lease. A session that holds and leases nothing on `m` releases
nothing (a hold re-executed after a preemption reaches the statement
again). `load m (n)` advances the innermost
enclosing hold's position on `m` by `n` tokens, which its allocation must
cover; the KV of a transfer counts as computed from then on. The invariant
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
`scheduler.py:868-1128`). Families are joined member for member: `pool
q[N] { admit via S; }` next to `stage S[N]` serves `q[i]` by `S[i]`, and
`stage E[N] : step { memory kv; }` next to `pool kv[N]` counts `kv[i]` for
`E[i]`; next to a family of one, every member gets that one, and any other
pair of counts is a link error (`programs/llmd_pd.seq` is the xPyD case,
`docs/case-study-pd.md` §Writing xPyD). A stage that serves several queues tries them in
the order their pools are declared, and the first head that does not fit
stops the iteration's admissions; `programs/llmd_pd.seq` declares the
decoder's queue of requests whose KV has arrived before its queue of new
ones, as vLLM serves `skipped_waiting` before `waiting`
(`scheduler.py:2383-2385`). `budget_left(S)` then evaluates to the budget
left. Until then a waiting session's cached prefix is evictable, which is
the *wait channel* of Lecture 5.

`grow m (d)` enlarges the innermost hold on `m` by `d` (rounded to
blocks). If it does not fit: with `preempt none` the session waits and
resumes where it was; with `preempt lifo` the holder that is a
resident of the step stage the pool is the memory of and was admitted last
— by the session's latest admission, the residents' serving order — is
preempted (vLLM `running[-1]`, `scheduler.py:742-813`: a holder away from
the engine — a prefiller's finished request keeping its blocks leased, a
decoder's request parked for a read — is in no `running` list; a pool that
is no engine's memory preempts its most recently admitted holder): its job
leaves its stage, its hold is released with its computed prefix cached, and
it re-enters the head of the pool's queue with the hold statement to
execute again. The grower
itself can be the victim. The re-executed hold finds `computed` set to the
position the hold had computed (0 on a first execution and after a hold
completes), so a program can resume rather than restart: vLLM's
`_preempt_request` resets `num_computed_tokens` and keeps the request's
output tokens (`scheduler.py:1560-1561`), so the request is rescheduled with
`num_tokens = prompt + outputs`, reserves and recomputes that many
(`kv_cache_manager.py:515-531`) and generates only the rest. The vLLM
programs write `known = computed < prompt ? prompt : computed + 1` (the token
sampled at `computed` is the request's too), `prefill (known - c)` and
`decode (o - 1 - (known - prompt))`. A program that recomputes from the
prompt alone says so by not reading `computed`. The Lean fragment does not
set `computed` on a preemption yet (`SeqExec.lean`'s `preemptLast` touches
no attribute), so on the decode-preemption path it restarts from the prompt
and diverges from the interpreter; no oracle scenario takes that path, and
the recorded scenario that will is the change that teaches the Lean model
the attribute.

**Ties.** Every order in the semantics is a declared key followed by a
declared number, the sequence number of a named event (for `choose`, the
index), so that two items with equal keys never fall to the order a data
structure happens to hold them in. A pool's queue:
the key (`queue by`), then the order the sessions joined the queue (`fifo`
is that order alone); a preempted session re-enters at the head, ahead of
the key. Eviction: the keys (`evict by`) or the release time (`lru`), then
the order the entries were released. A step stage's residents: the
`serve by` keys, then admission order. The preemption victim: the most
recently admitted holder. `choose`: the smallest key, then the smallest
index. A pool's growers: the order they stalled, the head blocking the
rest. Events at one instant: the order they were scheduled; sessions run
in the order they became ready; jobs of a `ps` stage with equal finish
tags finish in the order they started. Each pool has one queue, and a hold
on several pools waits in its first pool's; where the heads of two queues
both wait for room in one pool, or one stage serves several queues, the
pool declared first is served first. A reader who finds an order not
covered here has found a bug.

**Stages.** `fifo(c)`: `c` servers, jobs in arrival order at rate 1.
`ps(φ)`: every job at once, each at `φ(n)/n`. `delay`: every job on its
own at rate 1. `step { budget B; cost C; }`: an engine that runs
iterations. A plain `run`'s work is time at rate 1, the clock's unit; a step
engine's `prefill` and `decode` work is in the unit of `B`, tokens. The
clock itself has no unit: a program whose costs are seconds runs in seconds,
and `programs/vllm_request.seq` runs on the step clock with `cost 1`, so
its times are iterations. The residents are served the way `serve` names, said once per
stage: an order, `by (k₁, …)` (ascending keys evaluated for each resident
with `decoding`, 1 for a decoding resident, `admission`, its admission
sequence number, `remaining`, the tokens its run has left, and the
residents' `nres`, `ndec`, `kvb`, `kvp`; ties in admission order; a key may
not draw), or the rule `exclusive prefill`, below, which is not an order and
so cannot be combined with one. `admission` (the order their sessions were
admitted, vLLM's `running` list; the default) is `by` with no keys, where
every resident ties, and `decode first` is `by (decoding ? 0 : 1)`; the IR
knows only `by`. A scheduler that serves the shortest
remaining run first is `serve by (remaining)`, the opposite `serve by
(-remaining)`. One token to a decoding job, up to `chunk` to a prefilling
one,
until the budget is spent; a `growing` job first grows its hold to the
position it will reach (block by block, preempting if needed); then the
stage admits from the queues it serves. The iteration advances the clock by
`C`, an expression in `ntok`, `ndec`, `npre`, `nres`, `kvb`, `kvp`, `attn`; its
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

**`hidden`.** The output length `o` is drawn at `turn`, before the request,
and nothing in the semantics stops a hold's header, a queue key or a budget
from reading it: `reserve (prompt + o)` is a program vLLM cannot be, since
the scheduler knows `max_tokens` (scheduler.py:639) and learns the length
only when `check_stop` sees EOS or the cap (sched/utils.py:98-119, called at
scheduler.py:2426). `hidden o;`
in the workload says so, and the check is the same per-position table that
places the context variables: a hidden attribute may be read in a session
statement (`decode (o - 1)`), a run or a hold's `cache`, and is a link
error in a hold's units, `reserve` or `reuse` (read at admission, whether
written `enter … at admission` or `admit if … where`), a queue or eviction
key, a spill clause, a `ps` stage's capacity, or a step stage's budget,
cost, chunk or serve keys. An attribute the scheduler itself sets
(`cached`, `computed`) cannot be hidden.
The three vLLM programs hide `o` (`out` in the replay); a bound the
scheduler may know (`max_tokens`) would be a second, unhidden attribute, as
in the IR v4 design record.

**`enter`, `admit if` and `admit via`.** The scheduler admits; the session
enters. The statement is named from the side it is written on: `enter` in
a `session` block, like every other statement there, and `admit if … fit`
in a `server` block (§2, the two sides), where the scheduler is the one
speaking. `admit` is also the name of the *pool option* that hands a queue
to a stage's scheduler (`admit via S`), the scheduler's side again.

**Branching.** `branch (e)` takes the first block when `e` is 1 and the
second when it is 0; any other value (a fraction, a count, a negative
number, NaN) is a run-time error, since a guard is a test and a test has
two answers. `branch with (p)` takes the first block with probability `p`,
and is sugar the parser rewrites to `branch (~bernoulli(p))` — the IR, the
interpreter and the Lean model know only the one form, and the draw is a
0 or a 1 by the time the guard sees it. A constant guard that is not 0 or
1 is refused at link time (one strictly between 0 and 1 reads as a test
and was meant as a draw); a computed one is refused when it is evaluated.
Write the draw as
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
| `llmd_pd.seq` | llm-d's prefill/decode disaggregation on vLLM with the NIXL connector: the router, the sidecar, two prefill and two decode instances (`docs/case-study-pd.md`) | the source (llm-d at 8a2f37d, the router at 13eebdb, vLLM at 0c87a197), `tests/pd_semantics.rs`; no scheduler oracle yet |
| `vllm.seq` | vLLM v1 engine (Section 7) | scheduler semantics tests, the upstream oracle |
| `vllm_single_turn.seq`, `vllm_chat.seq`, `vllm_subagents.seq` | `vllm.seq`'s engine under a single-turn, a chat and an approximated subagent workload ([case study](case-study-workloads.md)) | the engine is `vllm.seq`'s text (`tests/workloads.rs`) |
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
| a preempted request keeps its output tokens: it is rescheduled with `num_tokens = prompt + outputs`, reserves and recomputes that many, and generates the rest | `computed` read by the re-executed hold: `known = computed < prompt ? prompt : computed + 1`, `prefill (known - c)`, `decode (o - 1 - (known - prompt))` | `scheduler.py:1560-1561`, `kv_cache_manager.py:515-531` |
| the scheduler reserves by `num_tokens` (prompt and generated so far), never by the final length: it knows `max_tokens` and learns the length when `check_stop` sees EOS or the cap | `hidden o;`: no header, key or budget reads `o` | `kv_cache_manager.py:517, 533-534`, `config/scheduler.py:191`, `scheduler.py:639, 2426`, `sched/utils.py:98-119` |
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
when the scheduler admits the request, not when it queues. `known` is
every token the request has: the prompt, or after a preemption the tokens
it had computed plus the one sampled there (`known = computed < prompt ?
prompt : computed + 1`, vLLM's `request.num_tokens`). The hit is the
cached *full blocks* of those, never all of them, since the last token is
recomputed for its logits: `floor((known − 1) / bs) · bs`
(`kv_cache_manager.py:289-300`). `cachedin(kv)` is read in the header, so
it is read in the waiting loop (`scheduler.py:932-939`), and until then a
waiting request's prefix is still evictable — which is the point of the
wait channel, and the moment the first version of the program got wrong
(§3). The units are the hit plus the chunk the budget the running requests
leave can take now, `min(known, hit + budget_left(engine))`
(`scheduler.py:1078-1128, 1214-1226`): every known token, or as far as the
hit and the budget reach, whichever is less. The rest is allocated as the
request runs (`growing kv`).

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
* A session is one sequence of statements, so it waits at one pool at a
  time. NIXL's push mode lets a proxy send the decode request while the
  prefill runs, so the decoder allocates *during* the prefill and the write
  starts the moment it ends; `programs/llmd_pd.seq` writes the decoder's
  admission after the prefill, which is the pull mode and the llm-d
  sidecar's serial dispatch. A reservation a session joins now and enters
  later is the construct for it (`docs/design/pd-transfer.md`).
* A lease's bound is one number: `programs/llmd_pd.seq` writes `inf` for
  a prefiller whose lease the decoder's heartbeats renew, `30` would be
  one nobody renews; the heartbeat itself (a 5 s message that adds 20 s)
  is not a construct, and a decoder that dies mid-wait is not a session
  the language has.
* One eviction order per pool; the priced orders use `price(stage, …)`
  with the stage's online estimates, as `libqueuingsim` does.
* The Lean model covers the syntax, the pool semantics and the stage
  rates; the step stage and the session-level semantics (continuations,
  flow) are not formalised yet, and there is no proof that the Rust
  interpreter implements the Lean relation (the vLLM oracle and the
  closed-form checks are the evidence; `docs/review.md` §4 lists
  the tools that would close this gap).
