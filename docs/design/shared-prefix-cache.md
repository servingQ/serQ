# A shared prefix cache

Every engine of [Engine neutrality](engine-neutrality.md) shares cached KV
across requests by content. vLLM hashes full blocks, SGLang and TGI keep a
radix tree, and TensorRT-LLM reuses blocks. serQ keys a cache entry by the
session that released it (`entries: serial → entry`,
`src/engine/interp.rs`), so a system prompt common to every session is
computed once per session. That gap is not vLLM's: it is the largest one
the survey found, and it is every engine's. This document proposes the
construct and prices it. It changes nothing yet.

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
hold reqs (1), kv (…) prefix (sys: 1200, task: 300, serial: K) … {
  …
} cache kv (prompt + o);
```

A segment's key is an expression the program evaluates at admission:
here `sys` and `task` are attributes the workload draws, and `serial` is
the session's own. Two holds share their first `j` segments when their
first `j` keys are equal. The engine's lookup then finds the longest
cached prefix, up to the block, along the chain of equal keys.

## The construct

- `prefix (k₁: n₁, …, k_m: n_m)` on a hold: the hold's prefix chain.
  Without it, a hold's chain is `(serial: ∞)`, a single segment that is the
  session's own. That is today's cache, so every program runs as before.
- `cache P (ℓ)` names the pool it caches on (#366). Only `P` caches, the
  first `ℓ` units of the chain, and `cached` is what `P` consumed.
- The pool's cache is a trie of segments. An entry is a node: a key, a
  length up to the segment's, a reference count of live holds that read
  through it, and the release time and order for eviction.
- `cachedin(P)` is the longest cached prefix along the hold's chain, read
  as a hold's header reads it today.
- Eviction takes leaves only, so that no cached suffix outlives its
  prefix (SGLang's radix leaf eviction, TGI's LRU over leaves). `evict
  lru` and `evict by (…)` order the leaves, and a node a live hold reads
  through is not evictable (SGLang's `lock_ref`).
- `drop P` drops the leaves only the session owns.

The per-session cache is the case where every chain is `(serial: …)`:
the trie is a forest of one-node trees, which is today's map from serial
to entry. The vLLM oracle scenarios and the trace replay compile to it
unchanged.

## What it buys

- SGLang's LPM waiting order. `queue by (-cachedin(kv))` is then the
  longest prefix match in the shared tree, as SGLang sorts it
  ([`schedule_policy.py` L487-L495](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/schedule_policy.py#L487-L495)).
- TensorRT-LLM's prefix-aware skip
  ([`capacityScheduler.cpp` L367-L376](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L367-L376)),
  which defers a context whose first blocks another one admitted in the
  same pass will compute. That needs a "will contribute" read, a later
  step.
- Agent and subagent workloads (`docs/design/subagents.md` lists
  cross-session cache as its gap) with a shared system prompt. These are
  where the cliff of [the tutorial](../tutorial/06-the-cliff.md) is most
  sensitive to the hit rate.

## The price

- IR: `CStmt::Hold` gains `prefix: Option<Vec<(CExpr, CExpr)>>` and its
  `cache` names a pool. Every program changes one word (`cache` →
  `cache kv`), and the IR files change shape. IR 11 is untagged, so the
  change goes in the coming tag's message.
- Interpreter: the pool's `entries` become a trie. Eviction, `drop`,
  spill and the conservation check move with it. A dead entry, the
  unmatched tail that `reuse` leaves today, becomes a leaf past the match.
- Lean: `Exec.lean`'s cache is a list of per-session entries. A trie is a
  new model. Until it is written, a program with a `prefix` is outside the
  fragment, and the generators raise `Fragment` on one. Programs without
  one keep their theorems if the per-session case is the same list.
- Oracles: vLLM's own cache is shared by content. A scenario of two
  sessions with a common system prompt, run by the real scheduler
  (`tools/vllm_replay_oracle.py`), would check the trie against vLLM
  before any other engine.

## Order

1. #366, `cache P (ℓ)`. It is needed by this design, it fixes the
   slot-pool cache on its own, and it costs one word per program.
2. The trie with keys, the same answers for per-session chains, and a
   vLLM oracle scenario with a shared system prompt.
3. LPM and the agent workloads written with it.

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
