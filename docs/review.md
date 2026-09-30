# serQ: review of the lecture's language against vLLM, the v2 design, and the tooling survey

Date: 2026-09-27. Companion of `docs/language.md` (the language
as built). Sections: §1 what the lecture's language (Lecture 1,
"A language for serving deployments") gets right and wrong about vLLM
v1; §2 the design decisions of v2; §3 what the validation found and
fixed (the self-review); §4 the survey of verification frameworks the
user asked for and the recommendation.

## 1. The lecture's serQ against vLLM v1 (`ref/vllm` at 0c87a197)

Read: `vllm/v1/core/sched/scheduler.py` (`schedule`, `_preempt_request`,
`update_from_output`, `add_request`, `_free_request`),
`kv_cache_manager.py` (`get_computed_blocks`, `allocate_slots`, `free`),
`block_pool.py` (`get_new_blocks`, `touch`, `free_blocks`),
`single_type_kv_cache_manager.py` (`get_num_blocks_to_allocate`,
`find_longest_cache_hit`), `sched/request_queue.py`, `request.py`, the
KV-connector base class.

What the lecture's definition captures correctly:

1. **Admission decides the hit.** `[Admit]` sets `H` from whether the
   session's own prefix survived in the cache. vLLM: the prefix lookup
   happens in `schedule()` when the waiting request is admitted
   (`_get_local_prefix_cache_hit`, scheduler.py:932-939), and the hit's
   blocks are `touch`ed out of the free queue at that moment.
2. **Cache never blocks, allocation does.** The guard `|h_m| + c ≤ M`
   counts allocated bytes only and the eviction in `[Admit]` makes room
   in the cache. vLLM: `get_num_free_blocks()` counts cached-but-unreferenced
   blocks as free, and `get_new_blocks` evicts their hashes as it hands
   them out (block_pool.py:668-702); a hit's own blocks are subtracted
   from the free count (`_get_num_evictable_blocks`,
   single_type_kv_cache_manager.py:245-263) — the lecture's `C_m := C_m − r`
   before the guard.
3. **Strict FCFS with head-of-line blocking.** Only the head of the queue
   can be admitted. vLLM: `if new_blocks is None: break` (scheduler.py:1228-1235).
4. **Free with cache.** `[Free]` keeps `ℓ ≤ c` bytes cached. vLLM: a
   finished request's full blocks stay in the hash table and join the
   free queue (block_pool.py:776-805).
5. **The tool call is outside the system** (external stage): vLLM has no
   notion of a session; the client re-sends.

What it misses or gets wrong:

6. **No colocated engine.** vLLM has "no decoding phase nor prefill
   phase" (scheduler.py:559-568): one token budget per step is handed to
   the running requests in order, then to waiting ones; a prefill is a
   chunk of that budget and decode steps and prefill chunks are the
   *same* iteration. The lecture's `serial`/`shared` stages cannot say
   this (Exercise L1:exr:colocated admits it). This is the paper's own
   replica and the RBLN and A100 testbeds, so the language could not
   describe the systems the paper measures.
7. **Memory is allocated at admission for the whole turn.** vLLM
   allocates the blocks of the chunk it schedules now
   (`allocate_slots(request, num_new_tokens)` with the budget left) and
   grows the request block by block as it decodes
   (kv_cache_manager.py:533-590). A request whose growth finds no block
   preempts the last admitted one. The lecture has no growth and no
   preemption ("a running decode that needs a block when none is free is
   preempted and later recomputed, which serQ has no instruction for",
   §L1:sec:implies).
8. **Whole-prefix cache entries.** vLLM's cache is per block, evicted
   tail first (blocks are freed in reverse order,
   single_type_kv_cache_manager.py:557-585), so a session keeps a shorter
   prefix rather than losing it whole; hits are partial and
   block-aligned, and never cover the whole prompt
   (`max_cache_hit_length = num_tokens − 1`, kv_cache_manager.py:289-300).
9. **The request cap.** `max_num_seqs` bounds the running set
   (scheduler.py:877-879); the lecture's `shared(φ)` flattens throughput
   but never refuses admission. The lecture's own remark that a pool is
   "a multi-server queue of `m_P` slots" is the fix: a request cap is a
   pool of `max_num_seqs` units.
10. **No priority queue** (`PriorityRequestQueue`,
    request_queue.py:131-197) and no per-request chunk cap
    (`long_prefill_token_threshold`).
11. **Continuation by a coin.** `branch_p` is a constant; a replayed
    trace decides continuation and think time per session.
12. **No measurement.** TTFT and the price of a miss are defined in
    prose (L1:eq:ttft); the program cannot state them.
13. **Cross-session sharing.** vLLM's cache is content-addressed
    (block hashes): two sessions with the same system prompt share
    blocks. The lecture's cache is keyed by session; v2 keeps that
    (Section 2) and lists it as the first gap.
14. **Preemption order of the lecture's "commands are urgent, earliest
    session first" is the arrival order of *sessions*; vLLM's order is
    the arrival order of *requests* plus the running list. v2 uses the
    order in which sessions became ready and, on a step stage, the
    admission order, which is what vLLM's `running` list is.

## 2. Design of v2 (what changed and why)

* **One resource abstraction.** A pool is a counted resource with an
  optional cache and its own queue; KV memory, request slots, a cap on
  live sessions and an offload tier are pools. The guard, the queue and
  the eviction are written once. `hold m₁(c₁), m₂(c₂) { … }` admits
  atomically on several pools, which is how vLLM checks slots and
  blocks together.
* **Scoped holds.** `hold … { body } cache (ℓ)` replaces `admit`/`free`.
  Balance becomes syntactic, the memory invariant is a lemma about one
  command, and preemption is "abort the scope and re-execute the
  statement", which is exactly `_preempt_request`
  (`num_computed_tokens = 0`, `waiting.prepend_request`): the prefix the
  hold computed is cached, so the re-execution finds a partial hit.
* **The `step` stage.** Budget, chunk cap, cost expression in the
  scheduled tokens and the residents' memory, `exclusive prefill`,
  `growing` with a per-hold position, `budget_left`. This is vLLM's
  `schedule()` reduced to its scheduling decisions; the same stage with
  `budget max(ndec, (ω+β·kvb)/a)` is the paper's fluid two-resource
  replica at iteration granularity, so the paper's model and vLLM are
  the same program with two budget expressions.
* **`cached` instead of `H`.** The prefix consumed at admission, in
  units, block-rounded; partial hits fall out of block-wise eviction.
* **Expressions everywhere.** Units, work, keys, probabilities and
  budgets are expressions over the session's attributes and observables
  (queue lengths, unfinished work, online price estimates), so the
  scheduler of §3 (priced eviction, offloading, routing by lookahead) is
  written in the program rather than in the interpreter.
* **`observe` and records.** The program says what is measured; runs
  dump per-turn records for pairing with measured runs.
* **Traces.** A workload can replay a corpus in order (`trace … ordered`)
  with forced-miss flags, so a measured run and its program consume the
  same sessions.

Kept from the lecture: the syntax's three parts (deployment, workload, and
the per-session program, which the lecture called the route and v2 calls
`session`); commands take no time, flow at stages; the memory invariant; the
hit decided at admission; sessions as the unit that moves.

## 3. What the validation found (self-review)

Each item was found by a check, not by reading.

| Found by | Defect | Fix |
|---|---|---|
| PS insensitivity (M/G/1-PS mean number) | the virtual clock was not advanced at a departure, so service was double counted | advance `v` to `now` before removing the job |
| `replica.sq` vs `TwoStage` | `cached` was read from the first pool of the hold (`batch` slots), so every turn missed | `cached` = the largest consumed prefix among the hold's pools |
| same | ended sessions' prefixes stayed cached and, being the largest, survived shortest-first eviction while live small ones went | `end` removes the session's prefixes (they can never be hit) |
| GPU replay stalled at 382 s | a run of zero work (`decode(out − 1)` with `out = 1`) stayed resident forever holding its blocks and slot | a zero-work run completes at once |
| vLLM oracle `longchunk` | an iteration started between two arrivals at the same instant, so the second request lost a step | iterations start only when no event is pending at the current time |
| vLLM oracle `preempt` | admitting with the first full chunk reserved too many blocks; vLLM reserves the chunk the budget leaves | `budget_left(stage)` |
| `replica.sq` at the paper's cap of 24 | the scenario is bistable (thrash edge); seeds of the two engines land on different branches | compare at 20 and without a limit; documented |

Second pass (the same day), found by replaying the trace through the real
vLLM scheduler on serQ's clock and searching for the first differing step
(`scripts/exp/first_divergence.sh`):

| Found by | Defect | Fix |
|---|---|---|
| step 19.2 s of the 40-session diff: serQ holds 720 blocks vLLM has free | a waiting request was admitted with a zero budget, which pinned its prefix; vLLM admits only at a step with budget left | `admit via engine`; unit expressions evaluated at admission |
| accounting differs by one block after a turn of prompt 2845 + 4 tokens | vLLM caches every computed full block, generated tokens included; the program cached the prompt only | `cache (prompt + out − 1)` with `reuse (common prefix)`; the rest is a dead entry |
| 520 blocks cached in vLLM, 0 in serQ, after a session's last turn | `end` dropped the session's prefixes; vLLM keeps them | `end` keeps the cache (the lecture's `[End]`); `drop` is explicit |
| two requests finishing a prefill in one step swap places | the "keep your place" rule | residents in admission order |
| a request admitted with budget left by serQ, not by vLLM | `scheduler_reserve_full_isl`: the whole prompt must fit | `reserve (prompt)` |
| two sessions released in the same step evicted in the wrong order | LRU ties broken by session number | ties by release order |
| the forced-miss replay diverges from the first forced turn | the oracle added a second nonce; `drop` removed blocks vLLM keeps | the trace's ids as they are; `reuse (0)` |
| the cost model reads 0 | a `let` named like a session attribute is shadowed by it | the linker rejects the clash |
| "bistable at 24 sessions" (first pass) | not supported by 20 seeds | withdrawn; the engines differ by 10 % throughput at 24 |

After these, serQ and the real scheduler agree on every request of the
full trace (`docs/language.md` §7). The A100 comparison is in §8.

## 4. Tooling survey: build serQ on an existing verification framework?

The user asked whether the language should reuse frameworks from OS /
systems formal verification instead of a hand-written lexer, parser and
interpreter, in any language. What exists, and what fits a language
whose programs must be *simulated* (stochastic, timed, millions of
events) and *verified* (invariants of the semantics, properties of
programs):

**Semantics-first language frameworks.**
* *K framework* — from one definition (configuration + rewrite rules) K
  generates a parser, an interpreter (LLVM backend), a symbolic executor
  and a reachability prover; used for the semantics of C, Java, EVM,
  P4 ([overview](https://pi2labs.org/papers/overview-k-semantic-framework),
  [K tutorial](https://kframework.org/exports/K.html),
  [user manual](https://kframework.org/docs/user_manual/),
  [LLVM backend](https://pi2labs.org/papers/semantics-based-execution-llvm-k-framework)).
  Fit: the operational semantics of §3 of `language.md` is a K
  configuration with cells for sessions, pools and stages, and
  `kprove` could check reachability properties of small programs.
  Against: K has no native stochastic or timed semantics (random draws
  and the flow step must be encoded), the generated interpreters are
  orders of magnitude slower than a Rust event loop, and the proofs are
  reachability claims, not Mathlib theorems that the paper's other
  results build on.
* *Lean 4 as the language workbench* — `declare_syntax_cat` and
  `macro_rules` embed a surface syntax; the semantics is a Lean
  definition that is both the object of proofs and executable
  (`#eval`, compiled to C) ([metaprogramming book: DSLs by elaboration](https://leanprover-community.github.io/lean4-metaprogramming-book/main/08_dsls.html),
  [syntax](https://leanprover-community.github.io/lean4-metaprogramming-book/main/05_syntax.html),
  [macros](https://leanprover-community.github.io/lean4-metaprogramming-book/main/06_macros.html)).
  Fit: the project already has its proofs in Lean/Mathlib; the surface
  syntax was added this way in `Serq.lean` (`[route| … ]`) with no
  parser of its own, and the two replica programs are proved well formed
  by `rfl`. Against: Lean's runtime has no random-number or statistics
  library to speak of and a compiled Lean interpreter would still be
  slower than Rust; real-valued costs make the semantics noncomputable
  (`Real.decidableLE`) unless the executable version uses `Float`/`ℚ`.

**Verifying the implementation.**
* *Verus* — SMT-checked specifications on Rust code, incl. unsafe code,
  used for verified storage and OS components; PLDI 2026 gives it a
  semantic foundation (VerusBelt) ([Verus, OOPSLA 2023](https://dl.acm.org/doi/10.1145/3586037),
  [VerusBelt, PLDI 2026](https://pldi26.sigplan.org/details/pldi-2026-papers/82/VerusBelt-A-Semantic-Foundation-for-Verus-s-Proof-Oriented-Extensions-to-the-Rust-Ty)).
  Fit: the pool accounting in `interp.rs` (`used + cached ≤ cap`, block
  rounding, holder lists) is the kind of invariant Verus proves on the
  actual code. Against: Verus needs its own toolchain and `vstd`; the
  event loop with `f64` time is outside its comfort zone.
* *Kani* — bounded model checking of Rust via CBMC, harnesses that look
  like unit tests, contracts and loop invariants for unbounded proofs
  ([Kani, ASE 2026](https://arxiv.org/abs/2607.01504),
  [Amazon Science](https://www.amazon.science/publications/kani-a-model-checker-for-rust)).
  Fit: the cheapest step: harnesses over `PoolState`-like structs with
  symbolic units to prove the invariant of `admit`/`release`/`grow` for
  all inputs up to a bound, on the existing crate.
* *Aeneas* — translates safe Rust (via Charon/LLBC) to pure Lean
  functions; used in 2026 to verify SymCrypt-scale cryptographic code
  ([Aeneas, ICFP 2022](https://dl.acm.org/doi/10.1145/3547647),
  [Lean use case](https://lean-lang.org/use-cases/aeneas/),
  [Aeneas/Rust/Lean report, Sept 2026](https://arxiv.org/html/2609.15648),
  [Rust-to-Lean pipeline experience report](https://arxiv.org/html/2605.30106)).
  Fit: the most direct way to connect the Rust pool code to
  `SerqLang.Step`: translate `interp.rs`'s pool functions (they are safe
  Rust over `Vec`/`HashMap`) and prove them refinements of the Lean
  relation. Against: `HashMap`/`BinaryHeap`/`f64` and the borrow
  patterns of the event loop would need restructuring into a pure core
  (which is a good idea anyway).

**Model checking of designs.**
* *P* — communicating state machines with systematic testing of
  interleavings; used across AWS (S3, EBS, DynamoDB, EC2), with PObserve
  checking production logs against the P spec
  ([P](https://p-org.github.io/P/), [GitHub](https://github.com/p-org/P),
  [systems correctness practices at AWS](https://cacm.acm.org/practice/systems-correctness-practices-at-amazon-web-services)).
  Fit: sessions, pools and stages are machines; P would find
  scheduling-order bugs (the two found here, same-instant arrivals and
  the budget-left rule, are of that kind). Against: untimed and
  non-quantitative; no queueing statistics.
* *Modest toolset* — a compositional modelling language with a formal
  semantics as stochastic timed automata; `modes` is a discrete-event
  and statistical model checker (rare-event simulation, non-Markovian
  distributions), `mcsta` an explicit-state probabilistic model checker
  ([overview, 2022](https://arxiv.org/pdf/2203.09881),
  [modes](https://link.springer.com/article/10.1007/s10009-020-00563-2)).
  Fit: the closest existing tool to "one model, simulated and checked":
  a serQ program with finitely many sessions is a stochastic timed
  automaton. Against: no memory-pool/cache primitives (everything is
  variables and guards), no Lean connection, and the model would be
  written a second time.
* *PRISM / Storm* — CTMC/MDP model checkers with steady-state (`S`) and
  transient properties; queueing networks are a standard case study
  ([PRISM: CTMC checking](https://www.prismmodelchecker.org/papers/probmiv01.pdf),
  [stochastic model checking](https://www.prismmodelchecker.org/papers/sfm07.pdf),
  [frontiers of quantitative verification](https://arxiv.org/pdf/2405.13583)).
  Fit: exact stationary numbers for the Markovian special cases
  (M/M/1//N, the finite-source price) as an independent check of both
  the Lean closed forms and the simulator. Against: state explosion
  beyond a few sessions; nothing non-Markovian.

**Recommendation.** Keep the split the seL4 project used (an executable
specification and a fast implementation, related by proof or by
differential testing), but stop hand-writing the parts a framework
gives for free:

1. *Lean is the language workbench.* The surface syntax lives in Lean
   (`[route| … ]`, done); the next step is an executable Lean semantics
   over `Float` or `ℚ` (`def step : Config → Config`) so that small
   programs run under `#eval` and the vLLM oracle scenarios of
   `tests/vllm_oracle.rs` become Lean test vectors checked by
   `decide`/`native_decide`-free evaluation. The Rust crate stays the
   simulator; its parser should be regenerated from the Lean syntax or
   replaced by a `pest`/`lalrpop` grammar generated from one source.
2. *Aeneas for the pool core.* Factor `interp.rs`'s pool operations into a
   pure module (`Vec`-based, no `f64` in the invariant-carrying part:
   units as `u64` tokens) and translate it with Aeneas; prove the
   translation refines `SerqLang.Step`. Until then, *Kani* harnesses on
   that module give bounded proofs cheaply.
3. *P or Modest for scheduling designs*, not for the language: when a
   new scheduler policy is written as a serQ program, a P model of the
   same state machines checks its interleavings; Modest/PRISM give exact
   numbers for the Markovian cases. Neither replaces the Lean model or
   the Rust simulator.
4. *Not K.* K would replace the Rust interpreter with a slower one and
   the Lean proofs with a different prover; the project's investment is
   in Lean.
