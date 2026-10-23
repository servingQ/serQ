# A cache names its pool

A hold's `cache` clause names the one pool that keeps the session's
prefix: `} cache kv (ℓ);`, and an engine's `taking … cache kv (ℓ)` names
the engine's memory. Only that pool consumes the prefix at admission and
keeps `min(ℓ, computed)` at the release; `cached` is what it consumed, and
its `computed` is its own. The hold's other pools are memory alone. Until
now `cache (ℓ)` applied to every pool of the hold, `cached` was the most
any of them consumed, and whether a pool had computed its position or its
allocation was one flag for the whole hold (#366). IR 16:
`CStmt::Hold.cache` and `CRequest.cache` are `(pool, ℓ)` where they were
`ℓ`.

## Before and after

`examples/multi-turn/replica.sq` holds a request slot and the KV around
the engine, and caches with the cost of both, which #390 made the only
way to write it:

```serq
    hold reqs (cost(reqs, 1)), kv (cost(kv, K + n + o)) {
      …
    } cache (cost(reqs, kv, K + n + o));
```

```serq
    hold reqs (cost(reqs, 1)), kv (cost(kv, K + n + o)) {
      …
    } cache kv (cost(kv, K + n + o));
```

Neither pool grows, so each cached its allocation: the slot pool kept a
slot per session, 317 of which were evicted. Run with `--horizon 300
--warmup 50` (seed 1), the `reqs` row before and after:

```text
pool     used    cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
reqs      0.7       7.3  0.000    0.660  0.0000     612       317       317        0      0    0      0
reqs      0.7       0.0  0.000    0.660  0.0000     612         0         0        0      0    0      0
```

The `kv` row and every other number of the report are the same. A turn
whose KV entry had been evicted read `cached = 1`, the slot's "prefix",
the larger of the two (`tests/pool_semantics.rs`,
`a_hold_caches_only_the_pool_its_cache_names`, with `drop kv`); here no
KV entry was evicted, so it did not show.

An engine's admission names its memory, one word more
(`examples/multi-turn/vllm.sq`):

```serq
      reuse (floor(request.known - 1, blocksize(kv)))
      cache (request.input + request.output);
```

```serq
      reuse (floor(request.known - 1, blocksize(kv)))
      cache kv (request.input + request.output);
```

Its numbers do not move: the request's prefill grows `kv` before it
ends, and with one flag per hold that made `reqs` cache its position, 0.
Every example and tutorial program under each of its workloads
(`examples/pairs.json`, 39 pairs) gives the same text and JSON report
before and after, seed 1, but `replica.sq`'s `reqs` row; the oracles
(`make oracle-ir`, `make lean`) agree as before.

The title's bug, a pool's cache decided by another pool: in
`hold kv (…), aux (cost(aux, 4)) { run llm prefill (…) growing kv; }
cache aux (cost(aux, 4))`, `aux` never grows and keeps its allocation, 4.
With one flag, `kv`'s growth made it keep its position, 0
(`a_pools_computed_is_its_own_not_the_holds`).

## Why it happened

The assumption: every pool a hold takes is a position in one token
sequence. Then one clause can say what all of them keep, one flag can say
whether "the" position advanced, and the most any pool consumed is "the"
cached prefix. It held for the hold #326 and #340 had in mind, one KV
pool, and broke on the first hold of a slot pool and a KV pool. The
rule was written in six places, each of which this change moves:

- the interpreter: `Hold.grown`, the maximum in `admit`, every release
  (`src/engine/interp.rs`);
- the Lean model: `HoldRec.grown`, `admitPool`'s `cache.isSome`
  (`lean/Serq/Exec.lean`);
- the figure: `cache_targets` followed the growing runs to guess which
  pools a hold cached in, and drew a strip on `replica.sq`'s `reqs`
  (`src/view/deployment.rs`, `tests/draw.rs`);
- the vLLM target: any cached hold that took the KV pool caching
  (`src/target.rs`);
- the whole-number inference: every pool of a cached hold a KV pool
  (`src/ir/whole.rs`);
- the `prefix` check: a hold with a chain needed some pool with `block`,
  not the one it caches in (`src/ir.rs`).

It had been met once before and patched: `docs/review.md` §3 records
`cached` read from the hold's first pool, `replica.sq`'s slots, so every
turn missed, fixed by taking the largest consumed prefix among the hold's
pools. That made the slot pool's prefix harmless as long as the KV entry
outlived it.

## Should the language have refused it

The state is a prefix on a pool that has none, a slot. No check over the
old program could refuse it: `cache (ℓ)` on a hold of several pools was
the ordinary way to cache the KV, and the language cannot tell a slot
pool from a per-session token pool (tutorial 4's `kv` has no `block`).
The fix takes the spelling away: nothing caches a pool the program did
not name. A hold may still name a slot pool (`hold reqs … cache reqs`),
and caches a slot then, because that is what it says. Four checks come
with the name, each with its test:

- the pool is one the hold takes, written as the hold writes it, index
  included (`Program::validate`, as `lease P` is): "`cache reqs`: the hold
  does not take that pool";
- an engine's names its memory, where the request's position is, which
  `request.hit` reads: "`cache reqs`: an engine's `cache` keeps the KV its
  requests compute, whose position is on its memory (`kv`); name the
  memory (a slot pool has no prefix, #366)", and "`cache kv`: the request
  does not take that pool";
- `ℓ` and `reuse` are units of the named pool (`cost(kv, ℓ)`; the cost of
  both pools no longer types);
- a hold with `prefix` caches a pool with `block`: "a hold with `prefix`
  caches `slots`, a pool without `block`", even when another pool has one.

The spelling without a pool is a parse error that says what to write:
the hold's pool when it has one, else `cache P (ℓ)` with the pools to
choose from; an engine's, its memory.

## Decisions

**The clause names its pool, `cache kv (ℓ)`** (the issue's (a)). Criterion
0: one pool keeps the prefix, and `cached` is its. Criterion 1: the
program says "cache the KV". The spelling is the one the hold's other
clause after the `}` already has, `lease P (t)`.

**An engine's `taking` names its memory too.** `taking` is a hold's
header, and its `reuse` and `prefix` are spelled as on a hold; a
`cache (ℓ)` that meant the memory there and every pool on a hold would be
one spelling with two meanings. The check that it is the memory is
needed anyway: a cache on another pool would keep a prefix no prefill
skips.

**`grown` per pool.** The interpreter's `Held.grown` is set by a growing
run's position and by a `load` on that pool, and `Held::computed` reads
it. Lean keeps `HoldRec.grown : Bool`, and `mapHold` sets it only when
the pool that grew is the one the hold caches (a preemption sets it as
before). Only the cached pool's `computed` is ever read (the release, a
lease, `release P`), so the one flag is that pool's, and `HoldRec`, the
preemption lemma of `Inv.lean` and `examples/papers/Kong.lean`'s records
keep their shape.

**`reuse` and `prefix` stay where they are.** They act on the cache's
pool. The lint already refuses either without `cache`, so in a program
they always come with a pool; moving them into the clause would grow the
IR change for no program.

**One cached pool per hold.** No example, tutorial or companion program
caches two pools of one hold: they nest holds (`agentic.sq`'s `hold slot
{ hold kv { … } cache kv (…) }`).

**IR 15 files in the Lean generator.** A hold of one pool caches that
pool, and a request its engine's memory: in 15 a request's other pools
cached 0 (its prefill grows the memory before it ends, and a preemption
caches positions), so the translation is exact. A hold of several pools
with `cache` kept a prefix in each, which the 16 model cannot say: it
is outside the fragment.

## The handshake

IR 16 (15 is tagged in v0.1.7-alpha.2): `scripts/gen_lean_oracle.py` and
`gen_lean_claims.py` read 16, the Lean `Route.hold`'s cache is
`Option (ℕ × (Env → V))` and its syntax `} cache p (ℓ);`, and every
committed IR file is regenerated. `serving-queue-theory/programs` writes
one word at five sites: `lecture_pd.sq:63` (`cache memP`), `agentic.sq:57`
and `agentic_model.sq:111` (`cache kv`), and the engines of
`open_vllm.sq:57` and `replay_vllm.sq:68` (`cache kv`).

## Self-critique

**`grown` per pool, nothing else.** Tried on 2026-10-11 and reverted:
every slot pool then cached its slot, `reqs`'s cached went from 0 to
15.8–88.9 in the vLLM programs with evictions, and a turn with only the
slot entry left read `cached = 1` and prefilled one token less (SGLang's
TTFT 0.5267 → 0.5265). The flag was a symptom; the clause applying to
every pool was the cause.

**The cost type picks the pool, `cache (cost(kv, ℓ))`** (the issue's (b)).
The IR keeps its shape, but the meaning moves into a type annotation
every consumer would have to infer through attribute aliases, an engine's
`taking` has no cost types to read, and `cost(reqs, kv, ℓ)` would still
be writable and cache the slot again.

**Keep the clause and redefine `cached`** (the issue's (c)). It leaves the
slot pool's prefix, the state nothing should be in, and only changes who
reads it.

**`cache (ℓ)` on an engine meaning its memory.** One word fewer in fifteen
programs, but `cache (ℓ)` after `taking reqs (1), kv (…)` is exactly the
reading that caused this bug, and a hold would spell it differently.

**`cache (ℓ)` kept for a hold of one pool.** It is unambiguous there, but
it is a second spelling of `cache kv (ℓ)`, and a pool added to the hold
later would change what it means.

**A list of cached pools.** No program caches two pools of one hold, and
a list is a construct only tests would use.

**A fix-it for the old spelling.** The error names the pool to write; a
diagnostic code and a rewrite would be frontend for five companion sites
and the repository's own programs, which move by hand once.
