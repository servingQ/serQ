# Design

The design record of serQ. `docs/language.md` describes the language as it
is and `docs/ir.md` the IR as it is; this directory records **why they have
the shape they have, what is pushing them to change, and what the next shape
is**.

## The IR is a living thing

The IR is never finished. It evolves with the requirements and keeps looking
for a better solution. That is a working rule, not a slogan, and it unfolds
into five items.

**1. Selection pressure comes from verification.** What asks the IR to change
is not taste but what a check caught: the fifteen defects of
`docs/review.md` §3, a path no oracle exercised, a statement Lean cannot
write, a regime where the simulator is slow. A requirement is written in
that form. "This program could produce this defect" is the requirement;
"this concept makes that class impossible" is the solution.

**2. Variation is an RFC; selection is done by the three consumers.** A
proposal is an issue (labels `rfc`, `design`), and the test it must pass is
what the IR's three consumers gain. The interpreter: a wrong answer that is
now impossible, or speed. Lean: a statement that can now be written, or an
undefined behaviour that is gone. The oracle: a path that now agrees. And a
fourth, the reader: a rule that disappears. A variation that answers none of
the four is the charm of a paradigm, not the power of a tool. The verdict
table of the [frontend sketch](frontend.md) applies that test.

**3. The price is a handshake.** One IR version moves `serving-queue-theory`'s
generator, the seven oracle IR files and the Lean fragment together. The
version policy that identifies meaning rather than shape (#21) sets that
price. The same shape with a different meaning is the most expensive change,
because an old reader is silently wrong.

**4. A decision keeps its reason.** A decision can be reversed only if its
"why" is recorded, so a rejected idea is recorded too, with its reason, in
the self-critique section of the document that rejected it. That is so the
same proposal is not argued twice.

**5. Nothing goes extinct.** Committed IR files must always be regenerable
(`make oracle-ir`). An old program must compile through the new frontend to
the same report (`tests/ir.rs`), and a change for which that is not possible
says what changed in its release note.

## How it evolves

```
what a check caught ──▶ RFC issue (Before/After, picture books)
                                │
   judged by the three consumers and the reader ◀──┘
                                │
   adopted ──▶ IR version (handshake) ──▶ docs/ir.md, docs/language.md updated
   rejected ──▶ the design document's self-critique, with the reason
```

The design criteria themselves (unambiguity, intention-revealing, policy in
the program, checkability) are in `CLAUDE.md` and #8 and are not repeated
here. This directory is the record of **applying** them.

## Documents

| Document | What | Status |
|---|---|---|
| [Philosophy](philosophy.md) | What serQ is mathematically, and the principles by which its vocabulary grows | discussion of 2026-09-28 |
| [The stochastic process](stochastic-model.md) | The deployment as a GSMP: state space, events, random inputs and kernel, copied from the interpreter; determinism, the prefill queue as a FIFO queue in the decode batch's environment, Markov structure, a regeneration conjecture; the paper's queueing model as a projection and its three non-Markov gaps; a production deployment (clients, router, engines, links) as a composition; six things the definition found the language did not say, four of them rules now (every instant settles, an iteration with tokens lasts, per-session random streams, the memory invariant asserted) and two candidates (an observation channel, a shared iteration clock) | definition, 2026-10-03 |
| [Frontend](frontend.md) | Model / instance split, effects and handlers, the admission block, trait vocabulary, units. With two self-critiques and a verdict table | sketch, before issues |
| [IR v4](ir-v4.md) | Making the implicit static: moments, ownership, declared orders, integer ticks, the session automaton, step coalescing, progress checks | RFC #41 |
| [Subagents](subagents.md) | A session that spawns sessions is not a tool call: endogenous arrivals, hold-and-wait, cross-session cache, `Spawn`/`Join` | review, for v4b |
| [The KV transfer](pd-transfer.md) | Prefill/decode disaggregation as llm-d and the NIXL connector do it, in pull and push mode; `release`, `load`, `transfer … from … to …` | IR v5, with `examples/pd-disaggregation/llmd_pd.sq` |
| [Renewal arrivals](renewal-arrivals.md) | Finite open runs, execution deadlines, report times and first-arrival compatibility | IR v6, #108 |
| [Bandwidth sharing](bandwidth-sharing.md) | A transfer holds the sender's and the receiver's link at once: `Run.also`, `share maxmin` or `bottleneck`, the flow solver | RFC #118, design before implementation |
| [Client and server](workload-server.md) | One frontend structure: sessions inside workloads, explicit request handling, unchanged expanded IR | implemented |
| [One admission](one-admission.md) | One spelling for one admission: `hold … at admission (…) … cache`; `enter`, `admit if … fit where` and `keep` retired, the serving name moved into a `def` | #136 |
| [One engine, one spelling](one-engine-spelling.md) | A stage that runs iterations is written only as an engine on a device; `stage`, `serve` and `nic` take `fifo`, `ps`, `delay`; the IR's step stage unchanged | #421 |
| [Separate prefill/decode batches](exclusive-prefill.md) | Whole-batch isolation and waiting-prefill takeover using the existing serve policy | untagged IR v8 semantics |
| [Serving a subset](serve-only.md) | `serve only (p)`: which residents an iteration serves, beside the order that `by` gives; FasterTransformer's decode-only batches and their opposite | untagged IR 11, #261 |
| [Waiting selection](waiting-selection.md) | Selection-time queue keys, elapsed wait and Ascend-style FCFS aging | IR v9 |
| [Queues](queue.md) | gateway, prefill, link and decode as roles of one `queue` that owns its pools, its stage and the entries holding a request's admission, allocation and service; what an entry may read; parse-time sugar; a link whose `serve` is its cost | RFC #72, with `examples/pd-disaggregation/llmd_nixl_pull.sq` |
| [Explicit gateways](explicit-gateways.md) | `request gw;` selects a gateway by name; predefined vocabulary and the proposed import boundary | PR #87 follow-up |
| [The pull relation](pull-relation.md) | A pod owns its `nic`; `D pull P latency x share s;` is a transfer's topology, mode and policy in one line, and a `transfer` without `on` its read | #200, parse-time sugar |
| [The entry's source](entry-source.md) | `decode (prompt) from P` names the queue whose lease the entry takes, and in the body `P` is that lease, never a number; the checks move from every call to the header and the relation | #516, parse-time sugar |
| [The push mode](push-mode.md) | A request's legs: `fork { … }` and `join;` for NIXL's push mode, where the proxy sends the prefill and the decode request at once; `P push D latency x share s;`; why not a reservation | #368, IR 11 (untagged) |
| [The KV-transfer library](kv-transfer.md) | The language keeps the copy (`transfer`, `nic`, `share`, the wait); `std/kvt` names the connectors that combine them: `kvt.nixl_pull (P, D, latency = x);` and `kvt.nixl_push (P, D, …);`, the source first, replace `D pull P …;` and `P push D …;`, and `share` is the program's alone; the L2 cache of #522: an `l2` queue, `kvt.l2_load` and `kvt.l2_store` | #526, #522, parse-time sugar |
| [Engine neutrality](engine-neutrality.md) | SGLang, TensorRT-LLM and TGI read from their source and written as serQ programs (`examples/engines/`); seven rules of the step iteration that are vLLM's, which engine contradicts each; a parameter per rule against the iteration as a program body; the price | survey and proposal, 2026-10-05 |
| [Engines on devices](engine-device.md) | A `device` lists the hardware's time resources and capacities; an `engine … on` it declares its slots (`reqs cap`) and `tokens cap B`, a required `scheduler { advance running …; admit waiting …; }` chooses the batch and `worker { forward (T); }` says how long it takes over the device's resources; its `tokens cap`, `worker` and `scheduler` read `running.…`, `waiting.…` and `batch.…` rather than bare context names, the residents apart from the batch (#416); `pool X on D` declares a capacity with the rules that hand it out. `memory`, `admit via`, `budget`, `chunk`, `cost` on a step and the default iteration go; parse-time sugar, no IR value moves; #395 | forms implemented, 2026-10-07 |
| [A serving specification language](serving-specification-language.md) | The position: a program specifies a serving system that is simulated, proved and later run in place of an engine's scheduler, as a P4 program is for a switch; the deployment as the circuit and the workload as the testbench; architectures, moments as block signatures, externs, instance files and the interface; a vLLM `scheduler_cls` target; the order, cheapest first | position and plan, 2026-10-04 |
| [A shared prefix cache](shared-prefix-cache.md) | Cached KV shared across sessions, as every engine surveyed shares it: a hold names its prefix as a chain of keyed segments, the pool's cache is a trie of them, eviction takes leaves; the per-session cache is the one-segment case; a prefix several holds read allocated once; the trie before `cache P (ℓ)` (#366) | implemented (#374), 2026-10-12 |
| [The engine's request](engine-request.md) | An engine says what it does with one request: `taking` on `admit waiting` sizes the admission, `request (…) { … }` holds what the request computes, its `at admission` and its `cache`; the server calls the engine, then the client measures and a turn calls one component; each step parse-time sugar checked by an identical report, the IR moving only where the step says (not in step 1, `sides` in steps 2 and 3) | design, #501 |
| [The scheduler and the worker](scheduler-worker.md) | An engine is a `scheduler { … }` that forms the batch and a `worker { forward (T); }` that computes it, as vLLM's scheduler and model runner; the worker reads only `batch.…`; `schedule` and `execute (T)` are refused with the new forms; parse-time, no IR value moves; step 3, a request as the engine's state (`E (input, output);`, `taking … reuse … cache`), and step 4, a KV connector to an L2 | steps 1–4 implemented, #555, #592 |
| [A queue's engine keeps its requests' state](queue-engine-request.md) | A queue's engine admits by `taking` and its entries call it, `D (prompt, o);`, each family member taking its own pools; a connector may name the queue's KV relation (`kvt.nixl_pull`/`push`): the read from the lease the request's entry took `from`, then the last token again and the admission read again (a slot taken once the KV is there); every engine record an attribute; the four programs that wrote the five lines give the same reports | implemented, #592's follow-up |
| [What the client sees](client-metrics.md) | Every turn is one request from a virtual client: the report's `client` section gives its `ttft`, `tpot` and `response` with no program text, from the turn and the tokens its engines commit; vLLM's definitions; the programs' hand-written ones removed where they meant the same, sample by sample; no IR change | #501 |
| [ThunderAgent](thunderagent.md) | ThunderAgent's program-aware router in front of vLLM replicas (`examples/multi-turn/thunderagent.sq`): programs admitted against the KV capacity and kept counted through their tool calls as a `lease`; a running program's growth pauses idle ones with `preempt leases by`, `grow` of a lease and `allocated(p)`, rejected alternatives (every lease a `preempt by` victim, a revoking hold, a timer); the paper's Fig. 4–5 beside the program's sweep; what it still approximates | example #518, IR 13 (untagged) additions #524 |
| [A policy per fabric](fabric-share.md) | How concurrent flows divide the stages they hold is the fabric's: `kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);` and `run a, b (w, share = share.maxmin)` replace the program's one `share maxmin;`; the policies are `std/share`'s; the stages runs join are one fabric with one policy; `Run.share` replaces `Program.share` | #559, untagged IR 13 |
| [A run's latency](run-latency.md) | A link's `serve … latency` and a relation's `latency = …` are the transfer run's own wait, `Run.latency`: present at its stages, taking no capacity, the report's `wait`; the delay stages `D.nic.latency` and `L.latency` go, and the figure reads the wait from the run, not a name | #611 (after #193), IR 14 |
| [Braced definitions](braced-definitions.md) | One braced spelling for expression and statement definitions; unchanged expansion and IR | implemented |

| [Sizes and costs](size-and-cost.md) | Workload-owned request sizes, explicit resource costs, and IR validation without a new request execution frame | IR 12 |

A new design document adds a row to this table.

- [Sessions describe completed turns](session-turns.md): optional single-turn sessions, server-independent continuation, and conditional loops.
