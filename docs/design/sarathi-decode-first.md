# Sarathi-Serve's order, written by the program

`examples/papers/dai_sarathi.sq` models Sarathi-Serve as Dai et al.
describe it (arXiv 2504.07347 §4): decodes first, then the oldest
prefills, mixed in one batch. It writes the order as `advance running
admission`, which is decodes first only while the residents' admission
order puts every decode ahead of every prefill. On its engine, which
takes nothing, it does not. This document decides where Sarathi-Serve's
order is written: in the program, as `advance running decode first`. The
language's `admission` keeps its meaning. The program changes once the
Lean fragment serves residents by keys (#328), because the paper's
propositions are proved in Lean about this program.

## The finding

`examples/papers/dai_sarathi.sq:21-24`:

```serq
    scheduler {
      advance running admission;
      admit waiting while (running.preempted == 0);
    }
```

The residents are in the order of their sessions' latest hold admissions
(`src/engine/interp.rs:4911-4919`, `lean/Serq/Exec.lean:920-922`), and a
session's admission number is set when a hold of it is admitted
(`interp.rs:3680-3683`, `Exec.lean:780`). Dai's engine takes nothing, so
no request is admitted to a pool and every resident has the same key: a
run joins the end of the residents, in the order the runs started. A
request's decode is a run of its own, started when its prefill ends, so
it stands behind every prompt that arrived while it prefilled.

`serq run examples/papers/dai_sarathi.sq --workload
examples/workloads/dai_periodic.sq --horizon 20000000 --log dai.json`,
read by #734's `starve.py` (an iteration in which a decoding resident
gets no token while a prefill gets some, with no more decoders than the
budget): 2970 of 4269 iterations. The first is at t = 4581500: session
96's prefill ends as session 97 arrives, and 96's decode waits nine
iterations behind 97's prompt. The stage's mean service is 2042552.2606.
The three claims hold: work conservation, the token rate and the backlog
bound hold whatever order the greedy fill takes, so they do not see the
order.

`tests/serve_only.rs`,
`an_engine_that_takes_nothing_serves_its_runs_in_the_order_they_started`,
pins the order on two requests, derived by hand: a decode that starts
behind a prompt waits a unit in admission order and is served first under
`decode first`.

## The decision

#734 offered three answers:

- (a) the language gives a request that holds nothing an admission number
  when it is called;
- (b) the program writes the order, `advance running decode first;`;
- (c) nothing changes, and the use case serves another order than the
  scheduler it names.

The answer is (b). Sarathi-Serve's order is a policy, and the program
writes policy (criterion 2). The paper states it as decodes first, and so
should the program (criterion 1): read as `advance running admission`, it
is Sarathi-Serve only to a reader who knows the shape theorem
(`Exec.assign_decode_first`) and that this engine's residents keep the
shape, and here they do not. `decode first` says it; its ties fall in
admission order, which on this engine is the order the prompts arrived,
the paper's "then the oldest prefills". `examples/multi-turn/replica.sq`
writes the same order.

After, run with line 22 as `advance running decode first;` and the same
command: 0 of 4269 iterations, mean service 2033134.5286, the three claims
hold.

## When the program changes

The paper's propositions are proved about this program's IR:
`tools/claims/dai_sarathi.ir.json` is generated into
`lean/Serq/Claims.lean`, and `examples/papers/Dai*.lean` prove its claims.
The Lean fragment serves residents in admission order only:
`fragment_step` (`scripts/gen_lean_oracle.py`) refuses any other
`serve`, and `decode first` lowers to `By([decoding ? 0 : 1])`. On the IR of the
changed program:

```
Fragment: serve {'By': [{'Cond': [{'Ctx': 'Decoding'}, {'Num': 0.0}, {'Num': 1.0}]}]}: the fragment serves residents in admission order (`By([])`)
```

so `make claims-ir` would stop, and the Dai proofs would lose the program
they are about. The fragment learns `serve by (…)` with #328 (Exec's
`assign` takes the least key, with the three rules of #322). The change
after it moves:

- `examples/papers/dai_sarathi.sq:22`, `advance running decode first;`,
  and its header (`:1-3`), which says the program does not serve
  Sarathi-Serve's order;
- `tools/claims/dai_sarathi.ir.json` and `lean/Serq/Claims.lean` (`make
  claims-ir`): the deployment's serve keys;
- `examples/papers/DaiChain.lean`, `absSlot`: a finished prefill's decode
  is served ahead of the prefills, not after them (`DaiChain.lean:40-49`),
  and the batch is the greedy fill in that order; `DaiSim.simulation`
  follows it, and `absSlot_leaves_shape`, the witness that the chain
  leaves the shape, no longer describes the program. Most of
  `DaiRecurrent`'s lemmas are about `shares` of any list;
- `docs/use-cases/dai.md`: the engine section, which says why the program
  does not write `decode first`, and its table row;
- `docs/language.md`: the example of an engine whose requests hold
  nothing, which names this program.

## The two questions

**Why it happened.** The program and its page read `admission` as vLLM's
running list on every engine: a request enters once and keeps its place.
That holds where a request is admitted to a pool. On an engine that
takes nothing the order is the runs', and a request's decode is a run of
its own. The page said the order was decodes first without a chunk cap,
and `docs/language.md` called `admission` vLLM's `running` list on every
engine; #744 corrected both, and the program's header, to the order the
program serves.

**Should the language have refused the program?** No. `advance running
admission` on an engine that takes nothing is defined, the interpreter
and Lean agree on it, and a program may mean it. The program named a
scheduler whose order it did not write.

## Self-critique

**(a), an admission number at the call.** The interpreter already says
that an engine that takes nothing "admits the request as it is called"
(`interp.rs:2467-2468`) and records `E.admitted` then, so for such a
request `admission` and `E.admitted` name different events, and (a)
would make them one. Rejected here. It changes what `admission` means on
every engine that takes nothing, `bari_rad.sq` and
`dai_fastertransformer.sq` besides this one, and Lean's model of each: an
IR version, priced at a handshake. Lean's statements have no admission
at an untaken request (`gen_lean_oracle.py`, `request_untaken`, writes it
as two runs), so it needs one. And it would still not make the program
say Sarathi-Serve's order; the program would serve it through the shape
theorem. What admission number a request that holds nothing has is
#741's question (the interpreter's `u64::MAX`, Lean's 0). Whatever it
answers, the program writes Sarathi-Serve's order the same way.

**(c), no change.** The use case would model a scheduler other than the
one it names, with latencies Sarathi-Serve does not have (mean service
2042552 against 2033134 at point C).

**A pool, so that requests are admitted.** The issue's preview of (a)
gives the device `kv cap 100000000;` and the engine `pool kv on gpu {
block 1; evict lru; preempt lifo; }` and `taking kv (request.known)`, and
serves decodes first through the admissions. Rejected: memory the
paper's model does not have, written to get an order (criterion 1), and
the Dai proofs have no pool.

**Changing the program now.** Rejected: the claims generator stops on
it (above). A program that serves another order than Sarathi-Serve's,
while its header and its page say so, is a smaller loss than the paper's
propositions unproved about the program the page shows.

**Decodes first in Exec as an insertion order.** Exec could keep its job
list with the decodes first, a decode inserted after the last one, which
on an engine that holds nothing is the interpreter's order, and the
fragment could take `decode first` without #328. Rejected: it is a second
mechanism for a serve order beside #328's keyed `assign`, which would
replace it, and the Dai proofs would move twice.
