# A shared prefix cache

Every engine of [Engine neutrality](engine-neutrality.md) shares cached KV
across requests by content. vLLM hashes full blocks, SGLang and TGI keep a
radix tree, and TensorRT-LLM reuses blocks. A serQ hold names what its
sequence is made of, as a chain of keyed segments, and a pool's prefix
cache is a trie of those segments (`src/engine/cache.rs`), so a system
prompt common to every session is computed once, not once per session.

## What a program has to say

A prompt is the concatenation of parts that different requests share to
different extents. In an agent workload these are a system prompt that
every session shares, a tool schema that some share, and the session's
own history. Content is not in the model: no token exists. What the
program can say is which requests share which part, and how long the
part is. That is enough for all four engines, because each of them
matches a prefix and never a part in the middle (vLLM's hash chains a
block to its parent; a radix tree is a tree of prefixes).

So a hold names its prefix as a sequence of segments, each a key and a
length:

```
hold reqs (cost(reqs, 1)), kv (…) prefix (system: S, serial: inf) … {
  …
} cache (cost(reqs, kv, prompt + o));
```

A segment's key is an expression of attributes and numbers: here
`system` is an attribute the workload draws (which product's system prompt
the session sends), and `serial` is the session's own. A key is what it
reads and its value: an attribute and its value, or any other expression
and its value, so `system = 3` is not serial 3. Two holds share their
first `j` segments when their first `j` keys are equal. The lookup
finds the longest cached prefix, in whole blocks, along the chain of
equal keys. `examples/multi-turn/vllm_system_prompt.sq` is `vllm.sq`
serving `M` products this way.

## The construct

- `prefix (k₁: n₁, …, k_m: n_m)` on a hold: the hold's chain. Without it
  the chain is `(serial: ∞)`, a single segment that is the session's own,
  and the trie is a forest of one-node trees, one per session: the map
  from serial to entry the interpreter kept before, with the same answers
  (`make check`, the oracles and the cache trace unchanged). Written out,
  `prefix (serial: inf)` is a second spelling of no clause, and does not
  link.
- A key and a length read attributes and numbers, not the state, the
  clock, a draw or the `cached` and `computed` admission sets. They are
  read when the hold asks and again when it is admitted (a waiting order
  may read `cachedin`), and the trie they index must not move under them.
  The linker refuses one that does.
- A node of the trie is a run of one segment, `[off, off + len)`. A
  segment's runs follow one another; the next segment hangs off the run
  that ends where its segment ends, under its key.
- Admission with `cache` reads the cached prefix of the chain, at most
  `reuse` of it and at most what it allocates (what it reads is part of
  its allocation), and locks the nodes it read: they are allocated while the
  hold lasts, counted once in the pool however many holds read them
  (vLLM's `ref_cnt`, `block_pool.py:754-768`), and an admission does not
  ask again for what another hold already reads. The run of a node the
  match stops inside is split, unless a hold reads it, in which case it
  is locked whole.
- Release lets the nodes go, and `min(ℓ, computed)` units stay cached
  along the chain, aged now. With `ℓ > 0` the chain past it that only this
  session wrote, and no hold reads, goes: the session's entry is replaced.
  With `ℓ = 0` the nodes the hold read go, unless another hold reads them
  or another session wrote them.
- A node's owner is the session that wrote it first: its entry, or the
  session that computed a shared segment.
- A node the session owns, with nothing of another session's below it,
  that only the session's holds read is the session's consumed entry: its
  other holds do not see it (#230: a hold nested in one that consumed the
  prefix finds none), and a write of the session's chain there displaces
  it, unreachable, until its hold lets it go. A shared node is seen by
  whoever reads it.
- `reuse` bounding the match cuts what the session's own entry held past
  the bound off, cached and unusable, as a dead node with its age and the
  attributes it is cut with, unless a live hold reads it or another
  session's sequence passes through it: a bound on one hold's read says
  nothing of another session's cache.
- Eviction takes cached leaves, so no cached suffix outlives its prefix
  (SGLang's radix leaf eviction, TGI's LRU over leaves), one block at a
  time from the leaf's tail; `evict lru` and `evict by (…)` order the
  leaves, and a node a live hold reads is not evictable (SGLang's
  `lock_ref`).
- `drop P` drops the cached leaves the session owns, leaves first: its
  entry, and a shared segment it computed once nothing hangs off it.
- `evict by (…)` reads a node's owner while it lives, else the snapshot
  of the last write.
- `cachedin(P)` is the cached prefix of the session's waiting hold's
  chain, else of its innermost hold's, else of its own, in whole blocks.

## What it buys

- Agent workloads with a shared system prompt. On
  `vllm_system_prompt.sq` (seed 1, horizon 3000 s, warm-up 300 s), one
  product's sessions prefill 710 tokens a turn on average and wait 16 ms
  for the first token; a thousand products' (no sharing to speak of)
  prefill 1529 and wait 44 ms.
- SGLang's LPM waiting order: `queue by (-cachedin(kv))` is the longest
  prefix match in the shared tree, as SGLang sorts it
  ([`schedule_policy.py` L487-L495](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/schedule_policy.py#L487-L495)),
  once `engines/sglang.sq` writes its chains.
- TensorRT-LLM's prefix-aware skip
  ([`capacityScheduler.cpp` L367-L376](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L367-L376)),
  which defers a context whose first blocks another one admitted in the
  same pass will compute. That needs a "will contribute" read, a later
  step.

## The price

- IR: `CStmt::Hold` gains `prefix: Option<Vec<(CExpr, CExpr)>>`, absent
  from the IR when `None`, so no IR file changes. A reader that ignores it
  computes a different answer for a program that writes it: IR 13 is
  untagged, so it goes in the coming tag's message.
- Interpreter: a pool's entries are the trie. Eviction, `drop`, spill and
  the conservation check (now: `used` is the holds' allocations less what
  they read, plus what is read, once) moved with it.
- Lean: `Exec.lean`'s cache is a list of per-session entries. Until a trie
  is written there, a program with a `prefix` is outside the fragment, and
  `gen_lean_oracle.py` raises `Fragment` on one. Programs without one keep
  their theorems: the per-session case is the same list.
- Frontend: one clause on `hold`, one keyword (`prefix`).

## Order

1. The trie with keys, the same answers for per-session chains, and a
   program that shares a system prompt (this document).
2. A vLLM oracle scenario with a shared system prompt, run by the real
   scheduler (`tools/vllm_replay_oracle.py`), before any other engine.
3. The engine form's request with a chain, LPM, and the agent workloads
   written with it.

## Self-critique

- **Content hashes instead of keys.** A program could draw token ids and
  let the cache hash blocks, as vLLM does. That would simulate a tokenizer
  to say what a key says in one word, and it would put a random content
  model into every run. Rejected: what matters to the scheduler is who
  shares what, and keys say exactly that.
- **One shared segment instead of a chain.** `share (sys: 1200)` with the
  rest per session covers the system-prompt case alone. Tool schemas and
  subagents that share a parent's context need two levels or more, and a
  chain costs no more than one level.
- **Keys read at admission only.** A key that read state would move
  under the trie. Keys are attributes and numbers, as a hold's index is
  (`docs/language.md` §3), and the linker refuses one that reads state.
- **`cache P (ℓ)` first (#366).** The clause still caches on every pool of
  the hold, so a slot pool can cache a slot when nothing grows, and
  `cached` is the largest of the pools' reads. Naming the pool fixes that,
  and was planned before the trie; the trie went first, and it inherits
  the per-pool rule unchanged.
- **Cache a prefix while it is computed.** vLLM caches a request's blocks
  as it schedules them (`kv_cache_manager.py:602-606`), so two requests
  admitted one after the other read one system prompt while the first is
  still computing it; here the second computes it too. A hold would have
  to publish the units its `growing` run has reached, and a reader would
  have to wait for units not yet computed. Left out: the sharing that
  matters for agent workloads is across turns and sessions, seconds apart.
- **A key as its value alone.** Then `prefix (system: S)` with `system =
  3` shared the root with the entry of the session whose serial is 3, and
  `prefix (tool: T)` with `system: S` of equal values, and a program whose
  attributes count from 0 could not keep them apart. A key is what it
  reads and its value; the default `(serial: ∞)` reads `serial`, and is
  still the chain of no clause.
- **The written default refused by value.** `prefix (serial: inf)` does
  not link; `prefix (serial: 1e12)` does, and runs as no clause on any
  sequence shorter than 10¹² units. Refusing every length a run cannot
  reach would read the run; the literal default is the second spelling a
  reader writes.
- **The last writer as owner.** A shared system prompt's owner moved with
  every session that wrote it, so whether `drop` took it, and whose
  attributes `evict by` read, depended on which session came last. The
  owner is the first writer.
- **A `reuse` bound cuts the subtree past it.** One session's `reuse
  (800)` inside a shared system prompt took the prompt's tail off the
  trie, and every other session's context under it. A bound limits the
  hold's read; it cuts only the session's own entry.
- **A hold reads its whole match.** A hold allocating less than the cached
  prefix it matched read all of it, and the pool's `used` went negative
  when another hold read the same prefix. A read is part of the
  allocation.
- **Consumed means locked by the session alone.** A shared node read at
  one moment by one session's holds alone was taken for that session's
  entry: its nested hold displaced it, and the other sessions' contexts
  below it went with it. A consumed entry is the session's own subtree.
- **A run split where any hold's match stops.** Splitting a run a hold
  reads would leave that hold's nodes stale. Such a run is locked whole by
  the second reader; its units are allocated once either way.
- **Spill along the chain.** A spilled block goes to the tier under its
  session's serial. A chain of shared segments has none, and a hold with
  `prefix` on a pool that spills or is spilled to does not link yet.
