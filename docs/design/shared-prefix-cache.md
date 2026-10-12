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
  The linker refuses one that does. A segment of length 0 is left out of
  the chain: `(tools: 0, serial: inf)`, a session without tools, is its own
  chain.
- A chain is cached in blocks: on a pool without `block` (a slot pool,
  whose `cache` keeps a slot) the hold's chain is the session's own, and a
  hold with `prefix` that takes no pool with `block` does not link. One
  slot cached along a shared chain would be read by every hold as
  allocated already, and the slots' cap would hold no more.
- A node of the trie is a run of one segment, `[off, off + len)`. A
  segment's runs follow one another; the next segment hangs off the run
  that ends where its segment ends, under its key.
- A session's *entry* is what hangs under the key of its serial: the whole
  chain without `prefix`, the `serial` segment of `(system: S, serial:
  inf)`. Everything else is shared, whoever computed it.
- Admission with `cache` reads the cached prefix of the chain, at most
  `reuse` of it, and locks the nodes it read: they are allocated while the
  hold lasts, counted once in the pool however many holds read them
  (vLLM's `ref_cnt`, `block_pool.py:754-768`), and an admission does not
  ask again for what another hold already reads. A hold with `prefix`
  reads at most what it allocates (what it reads is part of its
  allocation, and a prefix another hold reads cannot shrink to fit it); a
  hold without consumes its entry whole, as `Exec.lean`'s `admitPool`
  does, and what its allocation does not hold of it is gone. The run of a
  node the match stops inside is split, unless a hold reads it, in which
  case it is locked whole.
- Release lets the nodes go, and `min(ℓ, computed)` units stay cached
  along the chain, aged now. With `ℓ > 0` the session's entry past it,
  which no hold reads, goes: the entry is replaced. With `ℓ = 0` the nodes
  of its entry the hold read go. A shared segment stays.
- A node of the session's entry that only the session's holds read is
  its consumed entry: its other holds do not see it (#230: a hold nested
  in one that consumed the prefix finds none), and a write of the
  session's chain there displaces it, unreachable, until its hold lets it
  go. A shared segment is seen by whoever reads it, two legs of one
  session included.
- `reuse` bounding the match cuts what the session's entry held past the
  bound off, cached and unusable, as a dead node with its age and the
  attributes it is cut with, unless a live hold reads it. Past a bound in
  a shared segment nothing is cut: a bound on one hold's read says
  nothing of the cache.
- Eviction takes cached leaves, so no cached suffix outlives its prefix
  (SGLang's radix leaf eviction, TGI's LRU over leaves), one block at a
  time from the leaf's tail; `evict lru` and `evict by (…)` order the
  leaves, and a node a live hold reads is not evictable (SGLang's
  `lock_ref`).
- `drop P` drops the cached leaves of the session's entry, leaves first,
  and no shared segment.
- `evict by (…)` reads the session that wrote a node first while it lives,
  else the snapshot of the last write.
- `cachedin(P)` is the cached prefix of the session's waiting hold's
  chain, else of its innermost hold's, else of its own, in whole blocks.

## What it buys

- Agent workloads with a shared system prompt. On
  `vllm_system_prompt.sq` (seed 1, horizon 3000 s, warm-up 300 s), one
  product's sessions prefill 710 tokens a turn on average and wait 16 ms
  for the first token; a thousand products' (no sharing to speak of)
  prefill 1529 and wait 44 ms. About half of the difference is the system
  prompt a first turn no longer computes; the other half is memory: a
  thousand copies of it evict live sessions' contexts, which their next
  turns compute again. Over seeds 1–5 the one product prefills 690–840
  tokens and the thousand 1529–2601, the latter sensitive to bursts.
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
  their theorems: the per-session case is the same list, consumed whole
  at admission. Every example's report is the same, byte for byte, as
  before.
- Frontend: one clause on `hold`, one keyword (`prefix`).

## Order

1. The trie with keys, the same answers for per-session chains, and a
   program that shares a system prompt (this document).
2. A vLLM oracle scenario with a shared system prompt, run by the real
   scheduler (`tools/vllm_replay_oracle.py`), before any other engine.
3. The engine form's request with a chain: `prefix (k: n, …);`, the
   request's first item, written into the hold of its admission
   (`examples/multi-turn/vllm_system_prompt.sq`). Then LPM, and the agent
   workloads written with it.

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
  not link; `prefix (serial: 1e12)` does. It keys the cache as no clause
  does, but it is a chain: its hold reads at most what it allocates, where
  a hold without the clause consumes its entry whole. Refusing every
  length a run cannot reach would read the run; the literal default is
  the second spelling a reader writes.
- **A key's value as its session.** A segment keyed `serial` made the
  node the entry of the session its value named, so a program that set
  `serial` made another session's context that session's to drop. A node
  is a session's entry when its key is the writing session's own serial.
- **A writer as owner.** With the last writer as a node's owner, whether
  `drop` took a shared system prompt, and whose attributes `evict by`
  read, depended on which session came last; with the first writer, the
  session that computed the prompt first could cut, trim or drop it for
  everyone, which no other session could. An entry is what hangs under a
  session's serial; a writer is only whose attributes an eviction key
  reads.
- **A `reuse` bound cuts the subtree past it.** One session's `reuse
  (800)` inside a shared system prompt took the prompt's tail off the
  trie, and every other session's context under it. A bound limits the
  hold's read; it cuts only the session's own entry.
- **A hold reads its whole match.** A hold allocating less than the cached
  prefix it matched read all of it, and the pool's `used` went negative
  when another hold read the same prefix. A read on a shared chain is
  part of the allocation.
- **Every hold reads at most what it allocates.** Bounding a hold without
  `prefix` too would be vLLM's rule (the blocks past a hit stay in the free
  queue), but it changes what a program without the clause does, and
  `Exec.lean`'s `admitPool`, which consumes the entry whole, would no
  longer be the interpreter's. Left for a change that moves both.
- **Consumed means locked by the session alone.** A shared node read at
  one moment by one session's holds alone was taken for that session's
  entry: its nested hold displaced it, the other sessions' contexts below
  it went with it, and a second leg of the session did not see it. A
  consumed entry is part of the session's entry.
- **A chain on every pool of the hold.** A slot pool's `cache` keeps a
  slot; shared along a chain, every hold read it as allocated, and five
  holds held two slots. Until `cache P (ℓ)` names the pool (#366), a
  chain is cached on pools with `block`.
- **An empty segment as a key.** No node of length 0 is made, so a walk
  stopped at one, the session's context under it was never read, and a
  new entry was made every turn. It is left out.
- **A run split where any hold's match stops.** Splitting a run a hold
  reads would leave that hold's nodes stale. Such a run is locked whole by
  the second reader; its units are allocated once either way.
- **Spill along the chain.** A spilled block goes to the tier under its
  session's serial. A chain of shared segments has none, and a hold with
  `prefix` on a pool that spills or is spilled to does not link yet.
