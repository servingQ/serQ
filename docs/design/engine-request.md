# The engine's request

An engine says what it does with one request: how much it takes when it
admits the request, what the request computes, and what it leaves cached
when it finishes. The client says what it sends and what it measures. The
`server` block that sits between them today goes (#501).

This document is the design for #501's three steps. Step 1 is specified
completely; it lowers at parse time to the kernel the server writes today,
and the IR does not move. Steps 2 and 3 are sketched. Every step is checked by the report, not the
IR text (*How a step is checked*, below).

## The problem

`examples/multi-turn/vllm.sq` is vLLM on one device, and it takes two
blocks to say so (the server's thirteen lines of comment elided):

```serq
  engine vllm on gpu {
    reqs cap max_seqs;
    tokens cap B;
    schedule {
      // long_prefill_token_threshold binds only while another request is
      // running or waiting; a request alone takes the whole budget (scheduler.py:606-616)
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold);
    }
    execute (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
  }
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  pool reqs on vllm { queue fifo; }   // the engine admits the waiting, FCFS
  …
  // The engine: what vLLM does with one request.
  server {
    set t0 = now;
    set prompt = K + n;
    // …
    set kept = prompt;
    hold reqs (cost(reqs, 1)), kv (cost(kv, min(known, reused + budget_left(vllm)))) reserve (cost(kv, known))
         at admission (known = max(kept, computed + 1),
                       reused = min(cachedin(kv), reusable(known, blocksize(kv)))) {
      set kept = known;
      set c = min(cached, reusable(known, blocksize(kv)));
      observe hit = c > 0;                    // any reused block, again after a preemption
      observe prefill_tokens = known - c;
      run vllm prefill (cost(vllm, known - c)) growing kv;
      branch (known == prompt) { observe ttft = now - t0; }   // not a resumed request's re-prefill
      run vllm decode (cost(vllm, o - 1 - (known - prompt))) growing kv;
    } cache (cost(reqs, kv, prompt + o));
    observe response = now - t0;
  }
```

The comment above `server` says it: the block is "what vLLM does with one
request". The slot and the blocks it takes and when, the full-sequence
reservation, the high-water mark `kept` that a preempted request resumes
from (#478), the prefix reuse and what stays cached at the release are
vLLM's scheduler and KV manager. Of the request there are the prompt, the
output length and what the client measures. A reader who wants vLLM reads
two blocks and must know where one ends; a writer must choose which block
each line goes in. #454 found the same split for the admission alone.

The three other programs whose engine is this one,
`vllm_single_turn.sq`, `vllm_chat.sq` and `vllm_subagents.sq`, repeat the
server text, and `tests/workloads.rs` holds them to it, comparing text
because "the IR has no engine to compare".

## Three steps

**Step 1. The engine owns what it does with a request.** The admission's
sizing goes on the schedule's `admit waiting`, as `taking`; what the
request computes, its admission's bindings and what it leaves cached go in
a `request` block of the engine, in the order they happen, in the
kernel's words (`set`, `at admission`, `cache`). `kept` is written there.
The server calls the engine.

```serq
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  engine vllm on gpu {
    reqs cap max_seqs;
    tokens cap B;
    // What vLLM does with one request: the tokens it has, the prefix it
    // reuses, what it computes.
    request (prompt, output) {
      set kept = prompt;                   // the most tokens an attempt has had (scheduler.py:1560-1561)
      at admission {
        known = max(kept, computed + 1);   // request.num_tokens
        reused = min(cachedin(kv), reusable(known, blocksize(kv)));
      }
      set kept = known;
      set c = min(cached, reusable(known, blocksize(kv)));
      observe hit = c > 0;
      observe prefill_tokens = known - c;
      prefill (known - c) growing kv;
      decode (output - 1 - (known - prompt)) growing kv;
      cache (prompt + output);
    }
    schedule {
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold)
        taking reqs (1), kv (min(known, reused + budget_left(vllm))) reserve (known);
    }
    execute (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
  }
  pool reqs on vllm { queue fifo; }
  …
  server {
    set prompt = K + n;
    vllm (prompt, o);
  }
```

The time to first token and the response time are the client's, which
the report gives for every turn with no program text
([What the client sees](client-metrics.md)).

Each clause reads one moment: `request` what one request is and computes,
`schedule` what an iteration admits and advances (its `taking` read per
request at admission), `execute` how long an iteration lasts.

**Step 2. Observations move to the client, and routing to gateway
entries.** What the client measures follows the turn in the workload's
session; its time to first token, time per token and response time the
report already gives ([What the client sees](client-metrics.md)). A server that chooses an
instance (`routing.sq`, `replica.sq`'s `choose j by …`) becomes a gateway
queue's `route`, as the P/D programs already write it.

**Step 3. A turn calls one component, and `server { … }` is refused.**

```serq
  session {
    turn vllm (K + n, o);
    while (more) { set K = K + n + o; tool (~exp(Z)); turn vllm (K + n, o); }
  }
```

`turn X (…)` names what the turn calls: an engine's `request`, a gateway's
`route` (`turn gw.route (…)`) or a station.

## Step 1: the grammar

New productions, beside those of `docs/language.md` §Syntax:

```
eitem    += request ( NAME , ... ) { rstmt* [at admission { bind* }] rstmt* [cache ( expr ) ;] }
                                                 -- what the engine does with one request; one per engine
sstmt     : admit waiting [only ( expr )] [while ( expr )] [each at most ( expr )] [taking take , ... [reuse ( expr )]] ;
take     := POOL ( expr ) [ reserve ( expr ) ]   -- units of POOL, one of the engine's capacities or its device's
bind     := NAME = expr ;                        -- read when the request is admitted
rstmt    := stmt                                 -- and:
          | prefill ( expr ) [ growing POOL ] ;  -- this engine's prefill of expr tokens
          | decode ( expr ) [ growing POOL ] ;   -- this engine's decode of expr tokens
          | mark NAME ;                          -- a moment the caller reads as ENGINE.NAME
stmt     += ENGINE ( expr , ... ) ;              -- in `server`: the engine's request, in its place
```

The items of a `request` come in the order they happen: the statements
when the call is reached, `at admission { … }`, the statements while the
request is admitted, and `cache (e);`, what the release leaves cached.
Without `at admission`, the admission is at the top. A binding reads the
parameters, what the request sets before its admission, the bindings
before it, the engine's pools and stage, `cached`, `computed` and the
constants; a statement reads those, `now`, what the request sets and the
workload's `hidden` attributes, and nothing else the session has set.
That is the rule a queue's entry has ([Queues](queue.md)), for the same
reason. `taking`, its `reserve` and `reuse` read what a binding reads and
the bindings; `cache` reads the parameters, the constants and what the
request sets. A request may not `set` a parameter. A caller reads a
request's marks (`mark prefilled;`, read as `vllm.prefilled`) and nothing
else of it.

An engine whose schedule has several `admit waiting` writes the same
`taking` on each; a request without `taking` holds nothing, and then has no
`at admission` and no `cache`.

## Step 1: how it lowers

A call `E (a₁, …, aₙ);` is replaced by the kernel statements below, with
each parameter substituted by its argument as a queue entry's is (an
argument may not draw). Everything lowers inside the server, so every
statement keeps the side `Server` it has in `sides`.

| Step 1 | Kernel at the call | Moment (`docs/ir.md` §Moments) |
|---|---|---|
| `E (a₁, …);` | the statements of this table, in this order | — |
| the statements before `at admission` | the same, before the hold | `Session`, when the call is reached |
| `taking P (u) reserve (r), …` | `hold P (cost(P, u)) reserve (cost(P, r)), …`, the pools in the order written | `Admit` |
| `taking … reuse (e)` | the hold's `reuse (cost(P₁, …, e))` over every pool taken | `Admit` |
| `at admission { x = e; … }` | the hold's `at admission (x = e, …)`; the kernel's hold sets `x` at its body's top when the body reads it (`docs/language.md` §`at admission`) | `Admit` |
| the statements after it | the hold's body | `Session` |
| `prefill (e) growing P;` | `run E prefill (cost(E, e)) growing P;` | `Session` |
| `decode (e) growing P;` | `run E decode (cost(E, e)) growing P;` | `Session` |
| `mark m;` | `set E.m = now;`, read by the caller afterwards as `E.m` | `Session`, at the mark |
| `cache (e);`, the last item | the hold's `cache (cost(P₁, …, e))` over every pool taken | `Session`, at the release |

A request's own names (its bindings and `set`s) keep their names
in the IR, as the server's do; a program attribute or another request's
name that shares one does not link. A queue's entry renames its own
(`D.known`); a request cannot without moving every attribute name of the
IR.

**The mark.** A request's `mark m` lowers as a queue's does, to `set E.m
= now`, and the caller reads `E.m` afterwards. One construct, one
lowering.

Applied to the After of step 1, the call `vllm (prompt, o);` with
`prompt ↦ prompt` and `output ↦ o` writes the server the program has
since #509, line for line: `set kept = prompt;`, the hold with the header
of `taking`, the bindings in `at admission`, the body with its runs, and
`cache (cost(reqs, kv, prompt + o))`.

Why the server keeps `set prompt = K + n;` and calls `vllm (prompt, o)`
rather than `vllm (K + n, o)`: a parameter is substituted, so `K + n`
would be written where the IR reads the attribute `prompt` today
(`kept = prompt`, `known == prompt`, the decode's work, the `cache`).
`vllm_subagents.sq`'s session also reads `prompt` back (`set K = prompt +
o;`).

## How a step is checked

The report decides. For every converted program, `serq run` before and
after the step prints the same report, byte for byte, over several seeds
and horizons. The IR may differ only where this document says the step
moves something, and a script checks that: it rewrites the IR before the
step by the expected moves and compares the result with the IR after,
exactly. The expected moves:

- step 1: none; `serq ir` is byte-identical;
- steps 2 and 3: `sides`, `attr_types` and statement order where ownership
  moves from the server to the client (*What steps 2 and 3 move*, below).

`tests/workloads.rs` compares the `engine vllm` block of the four vLLM
programs instead of their server, and `make check` and `make lean` pass.

The first draft of step 1 moved the vLLM programs' first-token
observation into a mark the server read after the call, and that moved
the `ttft` line of their reports, since the observation was then recorded
when the request ended. #509 took the observation out of the programs:
the time to first token is the client's, recorded at the first token
([What the client sees](client-metrics.md)). Step 1 has no mark to move.

## What steps 2 and 3 move

`sides` marks each statement `Workload` or `Server`, and `attr_types` gives
an attribute set by the workload the type `Size` and one set by the server
`Value`. The interpreter and the Lean generator read neither; validation
does. Two hand-written variants of `vllm.sq`, compiled with the current
`serq` (`--horizon 100`), show what the steps move:

| Variant | `serq ir` differs from `vllm.sq` in | `serq run` (`--horizon 300 --warmup 50`) |
|---|---|---|
| step 2: `observe response = now - t0;` after each `turn;` in the session instead of in the server | `sides` only: the two `Observe` statements are `Workload` | identical |
| step 3: also `set t0 = now;` before each `turn;` instead of in the server | `sides`; `attr_types` (`t0` becomes `Size`); `blocks` (the `Set` of `t0` precedes `Turn`) | identical |

So a move from the server to the client is recorded in the IR, as it
should be: `sides` is the record of who owns a statement. Step 2 keeps the
blocks and the run, not the bytes; step 3 keeps the run. These are the
moves the check of steps 2 and 3 allows. And with the server gone, the stamp `t0` must
be set by someone: the client before its `turn` (the second row), or a
`turn` form that stamps a name the program gives it on the server side.

## Two shapes of request: P/D decoders

The decoder of `examples/pd-disaggregation/llmd_nixl_pull.sq` admits a
request one of two ways. A local prefill:

```serq
      hold kv (cost(kv, min(known, reused + budget_left(D)))) reserve (cost(kv, known)), reqs (cost(reqs, 1))
           at admission (known = computed < prompt ? prompt : computed + 1,
                         reused = min(cachedin(kv), reusable(known, bs))) {
```

and with the KV read from a prefiller, parked without a running slot, then
admitted to one inside:

```serq
      hold kv (cost(kv, known)) reserve (cost(kv, known)), reqs (cost(reqs, 0)) reserve (cost(reqs, 1))
           reuse (cost(kv, reqs, reusable(known, bs)))
           at admission (known = computed < prompt ? prompt : computed + 1) {
        …
        hold reqs (cost(reqs, 1)) {
```

A queue's entries are already named per shape (`decode (prompt)`,
`decode (prompt) from src`). The proposal: a queue's entries are its
engine's requests, with the `request` grammar for their bodies, and
`taking` is written once per entry on the engine's `admit waiting`:

```serq
      admit waiting while (running.preempted == 0)
        taking decode: kv (min(known, reused + budget_left(D))) reserve (known), reqs (1)
        taking decode from: kv (known) reserve (known), reqs (0) reserve (1) reuse (reusable(known, bs));
```

A top-level engine with two shapes would name its requests the same way,
`request local (…)`, `request remote (…)`, called `vllm.local (…)`. The
inner `hold reqs (cost(reqs, 1)) { … }`, the parked request taken into the
running set, is a second admission of the same request; whether the body
keeps it as a `hold` or says it as an admission of the engine's is open,
and decided with the P/D programs' conversion, not in step 1.

The decoders also show why `kept` is the program's and not the language's:
their `known = computed < prompt ? prompt : computed + 1` keeps no
high-water mark.

## Relation to #366 and #454

**#454** (say what an engine takes at admission in the engine) is absorbed.
Its `admits (known, hit) { reqs 1; kv …; }` is step 1's `taking`, on the
statement whose moment it is; its `on vllm (bindings) { … } cache (…)` is
the request's `at admission`, statements and `cache`. Its one-spelling
rule follows once every engine program is converted: a `hold` of a pool an
engine admits is refused, and the engine's request is the only way to it.
Its third question has an answer in the program itself: the vLLM hold takes
`reqs`, which the engine admits (`pool reqs on vllm`), and `kv`, a pool on
its device that no engine admits, together.

**#366** (the cache names its pool) is independent and composes. Step 1's
`cache (e)` lowers to the joint `cache (cost(reqs, kv, e))` the programs
write today, every pool taken; #366's `cache kv (e)` is the same item with
the pool named, with #366's IR change and nothing more.

## Self-critique

**B: a single engine as a queue.** `queue vllm : serve { … }` with a fifth
role `serve` uses the queue's entry machinery as it is. It calls one engine
on one device a "queue", a station among stations, in the program a vLLM
reader opens first (criterion 1), and adds a role. Rejected: the request
is that machinery without the word.

**C: the boundary only.** Two blocks stay; the admission's sizing moves to
the engine (#454's `admits`), the runs stay in the server. It is the
smallest change and keeps the question "which block?" for everything but
the sizing. Rejected as the end state; its content is step 1's `taking`.

**The runs in `execute`.** `execute (T)` is read once per iteration, over
the batch, with no request (`Step`); `prefill (known - c)` is a token count
of one request, read when the session reaches it (`Session`), which the
schedule then splits over iterations. One clause reading two moments is
the ambiguity criterion 0 refuses. Rejected.

**`taking` inside the request.** `request (prompt, output) taking reqs (1),
kv (…) { … }` keeps each shape's sizing with its shape, which the P/D
decoders would find simpler than one `taking` per entry. Not chosen
because the sizing is read when the scheduler admits, and `admit waiting`
is where the schedule says when that is (#501, second comment). If the
per-entry `taking` above reads worse in the P/D programs than this, it is
the fallback.

**`kept` as a language mechanism.** The engine would keep the request's
high-water mark of `known` itself. The P/D decoders write a `known` without
one, so the mark is a policy, and a program must be able to leave it out
(criterion 2). `set kept = prompt;` before `at admission` and `set kept =
known;` after it say it.

**Three spellings of assignment.** A first draft wrote `state kept =
prompt;`, a bare `known = …;` for a binding and `set kept = known;`.
`state` already means an engine's register in the same block, and a bare
binding reads as a one-time assignment though it is read at every
admission. The request writes the kernel's words in the order they
happen: `set`, `at admission { … }`, `set`, and `cache (e);`, the kernel's
`} cache (…)`.

**`release cache (e);` at engine level.** It read the request's
parameters outside the request, and `release` is the kernel's word for
giving a hold back. Rejected for `cache (e);` as the request's last item,
where #366's `cache kv (…)` fits too.

**`run vllm prefill (cost(vllm, …))` inside the request.** It is the kernel
spelling and needs no new statement. Inside the engine it names the engine
and its unit twice per run; `prefill (e)` says the mode, which is all that
differs. `growing kv` stays written, since an engine may run without
growing (`tgi.sq`, `ascend.sq`).

**A parameter copied, not substituted, in step 1.** `vllm (K + n, o)`
could set `prompt = K + n` at the call. Substitution is the queue entries'
rule; a second rule for engines in step 1 would be two meanings of one
parameter. Step 1 substitutes, and the server keeps `set prompt = K + n;`.
Copying comes with step 3's `turn vllm (K + n, o)`, where the client has
no `set` on the server's side to write (#501).

**The word `request`.** `request;` was a session statement and is refused
with its replacement; inside an engine `request (…) { … }` declares
vLLM's `Request`. `serve` is a queue's item and `handle` says nothing.

**The mark recorded at the mark.** An `observe` after the call that reads
`E.m` could be lowered to the same observation with `now` for `E.m`, at
each `mark m;` the request reaches. It is a second lowering of `mark`
beside the queue's `set Q.m = now`, so one statement would mean two
things by where it stands (criterion 0), and an observation written after
the call would be recorded before it ends. Rejected (#501); the case that
asked for it, the time to first token, is the client's (#509).

**Byte-identical IR as the check.** It holds for step 1, and for no step
that moves a statement to the client, since `sides` records who owns it. The report is what a program
claims; the IR may move where a step says it does.

## Open decisions

1. **P/D.** Per-entry `taking` on `admit waiting` or `taking` in the entry;
   and the parked decode's second admission. Decided with the P/D
   programs' conversion, its own step.
