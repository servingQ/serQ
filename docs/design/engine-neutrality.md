# Engine neutrality

serQ is meant to write the scheduler of any inference engine. Its step
stage was built against one, vLLM v1, and checked against that one alone.
This document reads three other engines' schedulers from their source,
writes each as a serQ program, and records where the language is vLLM's
rather than a mechanism a program parameterises (design criterion 2). It
proposes a design and prices it. It does not change the IR.

The sources are pinned:

| Engine | Revision | Path read |
|---|---|---|
| SGLang | [`b792228b`](https://github.com/sgl-project/sglang/tree/b792228b35b21565067520857319dfc05e4d134e) (2026-10-04) | `python/sglang/srt/managers/` (`scheduler.py`, `schedule_policy.py`, `schedule_batch.py`), `mem_cache/` |
| TensorRT-LLM | [`bf414e37`](https://github.com/NVIDIA/TensorRT-LLM/tree/bf414e37291b9d15a5328af99e349db8dedf7a4d) (2026-10-04) | the PyTorch backend's default for a dense model, which is the C++ pair `cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp` and `microBatchScheduler.cpp` |
| TGI | [`b4adbf2f`](https://github.com/huggingface/text-generation-inference/tree/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed) (2026-03-21, maintenance mode) | `backends/v3/src/` (`backend.rs`, `queue.rs`, `radix.rs`) |

The programs are `examples/engines/{sglang,tensorrt_llm,tgi}.sq`. They link
and run; each one's header says where it is not its engine.

## What fits today

The vocabulary carries most of every engine unchanged. A pool is a request
cap or a KV block pool. A step stage's `budget` is a token budget. `admit
via` is an engine's waiting loop. `queue by` is a waiting order, FCFS or
priority. `reserve` separates the admission test from the allocation.
`computed` lets a preempted request keep its output tokens. `hidden` keeps
the output length from the scheduler, and every engine here knows
`max_tokens` and not the length.

| Engine, default path | Fits | Does not fit |
|---|---|---|
| **TGI**, chunking on | A mixed step, FCFS with head-of-line blocking ([`queue.rs` L278-L328](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/queue.rs#L278-L328)), the whole `input + max_new_tokens` allocated at admission ([L315-L318](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/queue.rs#L315-L318)), no preemption, a prompt-only prefix cache | The forward after one that admitted admits nothing: the loop prefills the new batch with the running one and then decodes once more before it looks at the queue again ([`backend.rs` L221-L236](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/backend.rs#L221-L236), [L285-L288](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/backend.rs#L285-L288); V2, V7); the cross-request radix cache |
| **TensorRT-LLM**, `GUARANTEED_NO_EVICT` ([`llm_args.py` L3923-L3927](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/llmapi/llm_args.py#L3923-L3927)) | Two levels as pool and budget, generation requests before contexts, FCFS | The reservation is held for the request's life, not tested once; no chunking (a context runs whole or waits); prefix-aware scheduling, on by default, skips a context whose first blocks a context admitted in the same pass will contribute, so FCFS is not strict ([`llm_args.py` L3946-L3948](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/llmapi/llm_args.py#L3946-L3948), [`capacityScheduler.cpp` L367-L376](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L367-L376); it needs the shared cache) |
| **SGLang**, `fcfs`, no mixed chunk ([`schedule.py`](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/arg_groups/fields/schedule.py#L82-L98), [L202-L205](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/arg_groups/fields/schedule.py#L202-L205)) | FCFS, the chunked request, recomputation after retraction, a prompt-only cache after retraction | Several prefills in one prefill-only batch; the retraction victim and where it re-enters; `new_token_ratio`; the radix cache |

## Where the language is vLLM's

A step stage's iteration is a fixed procedure in the interpreter,
`start_iteration` in `src/engine/interp.rs`, and it is vLLM's `schedule()`:
serve the residents in order, then, unless the iteration preempted, admit
waiting requests one at a time with the budget left; the first that does not
fit stops. Each rule below is fixed there, and each is contradicted by an
engine's default.

| # | Rule fixed in the language | vLLM | Contradicted by |
|---|---|---|---|
| V1 | Residents are served first, admission follows, and an admitted prefill joins the decodes already chosen | `scheduler.py:624-823, 868-1128` | SGLang forms a prefill batch first and, if one forms, runs it alone, so decodes wait ([`scheduler.py` L3754-L3756](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/scheduler.py#L3754-L3756)); TGI without chunking prefills a new batch in its own forward ([`backend.rs` L237-L261](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/backend.rs#L237-L261)) |
| V2 | Admission is tried in every iteration that has budget left | `scheduler.py:868-1128` | TGI admits at most every other forward even with chunking (above), and without chunking forms a new batch only when `floor(batch_size · waiting_served_ratio)` requests wait or `max_waiting_tokens` decode steps have passed ([`backend.rs` L184-L196](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/backend.rs#L184-L196)); TensorRT-LLM's `STATIC_BATCH` admits only into an empty engine ([`capacityScheduler.cpp` L307-L309](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L307-L309)) |
| V3 | The preemption victim is the resident admitted last (`preempt lifo`) | `scheduler.py:742-813` | SGLang retracts the fewest output tokens first, then the longest prompt ([`schedule_batch.py` L3426-L3454](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/schedule_batch.py#L3426-L3454)); TensorRT-LLM's `MAX_UTILIZATION` pauses the last *started* in arrival order, which keeps a paused request's rank ([`capacityScheduler.cpp` L591-L612](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L591-L612)); vLLM's own PRIORITY policy (§7 lists it as not modelled) |
| V4 | A victim re-enters at the head of the queue | `scheduler.py:1539-1582` | SGLang appends a retracted request at the tail ([`scheduler.py` L4225-L4272](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/scheduler.py#L4225-L4272) into [L3268-L3283](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/scheduler.py#L3268-L3283)); vLLM's own PRIORITY policy puts it back into a heap by `(priority, arrival)` (`request_queue.py:159-164`) |
| V5 | `reserve` is a test at admission and nothing keeps it | `scheduler.py:1223`, `config/scheduler.py:191` | TensorRT-LLM subtracts every running request's blocks still to come ([`capacityScheduler.cpp` L265-L305](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L265-L305)); SGLang subtracts `new_token_ratio` times each running request's remaining `max_new_tokens` ([`schedule_policy.py` L694-L698](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/schedule_policy.py#L694-L698), per request [L800-L811](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/schedule_policy.py#L800-L811)) |
| V6 | A prefill takes any number of tokens up to `chunk` | `scheduler.py:606-616, 675-676` | TensorRT-LLM's default schedules a context whole or not at all, and with chunking rounds a chunk down to a block ([`microBatchScheduler.cpp` L228-L263](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/microBatchScheduler.cpp#L228-L263)); TGI without chunking stops at a prompt over the budget |
| V7 | The scheduler keeps no state between iterations | — | SGLang's `new_token_ratio` decays every decode step and jumps after a retraction; TGI counts decode steps since the last new batch (`waiting_tokens`), and with chunking skips admission in the forward after one that admitted |

`exclusive prefill` and `serve only` were added to escape V1 for one engine
each (vllm-rbln, FasterTransformer as Dai et al. model it) and neither
escapes it for SGLang. `exclusive prefill` selects one prefill; three
two-token prompts on a budget of eight run in three iterations, where SGLang
runs one. The program (`SERQ_TRACE_ITER=1 serq run` on it prints the
iterations):

```
pool reqs { cap 8; admit via engine; }
pool kv { cap 100; }
stage engine : step { budget 8; cost 1; memory kv; serve exclusive prefill; }
workload { arrive batch(3); init { set prompt = 2; } }
session {
  hold reqs (1), kv (min(prompt, left)) reserve (prompt + 2)
       at admission (left = budget_left(engine)) {
    prefill prompt growing kv;
    decode (2) growing kv;
  }
  end;
}
run { horizon 20; warmup 0; seed 1; }
```

```
ITER 0.0000 0:0:p2 | used 2 cached 0
ITER 1.0000 1:0:p2 | used 5 cached 0
ITER 2.0000 2:0:p2 | used 8 cached 0
```

`serve only (decoders < residents ? !decoding : decoding)` mixes, because
the decodes are chosen before the prefill is admitted. And the two do not
combine: `serve only (…) exclusive prefill` does not parse, because which
of them a predicate would exclude would be a third rule. Each escape is
one more rule beside the procedure, and the combinations are where the
next rule would be needed.

Of the four constructs #8 found to fail criterion 2, `exclusive prefill` is
V1's escape above. The rule that an iteration that preempted admits
nothing (#68) is contradicted by none of the three engines: SGLang retracts
only in decode iterations ([`scheduler.py` L3758-L3760](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/scheduler.py#L3758-L3760)), which admit nothing; TensorRT-LLM
stops its scan at the victim ([`capacityScheduler.cpp` L591-L612](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L591-L612)); TGI never preempts. Head-of-line blocking (#65) is
every engine's default but TensorRT-LLM's prefix-aware skip, which needs the
shared cache. Both are still rules rather than mechanisms, but no engine
here asks for their opposite in a program serQ can write today.

Not vLLM's, and missing for every engine including vLLM: the prefix cache
shared across requests (every engine here has one: SGLang's and TGI's radix
trees, TensorRT-LLM's block reuse; §9 of the language), scheduling one
iteration ahead (SGLang's overlap scheduler, TensorRT-LLM's, vLLM's
asynchronous scheduling; all on by default), and speculative decoding.

## Two designs

### A. A parameter per rule

Each rule becomes an option whose default is vLLM's:

```
pool kv { …; preempt by (position - prompt, -prompt) requeue tail; reserve held; }
stage engine : step { …; serve exclusive prefill (inf); admit when (queued(reqs) >= …); granule bs; }
```

Every row of the table closes with one option, but the options act on one
procedure, and each pair needs a rule for how they meet: `serve only`
beside `exclusive prefill` already does not parse, `admit when` beside
`exclusive prefill (n)` would need its own answer, and so on for each new
pair. That is the shape that made `exclusive prefill` a named rule.

### B. The iteration as a program

A step stage's iteration is written by the program, from three statements
over the residents and the queues (#355; the first part is #362):

```
serve [only ( p )] [order] ;             -- give tokens to the residents not yet served; those p excludes stay unserved
admit [while ( e )] ;                    -- admit the heads of the stage's queues, each served at once, while budget is left, they fit and e holds
branch ( e ) { … } [else { … }]          -- over the iteration so far
```

`e` reads the residents' totals and what the iteration has done so far:
`tokens` and `prefilled` (scheduled so far), `admitted`, `preempted`; `p`
and the keys read what a serve key reads. vLLM is the procedure that is
fixed today, written out, and every other row is another body:

```
// vLLM v1: running first, then the waiting, until a preemption happens
iteration { serve; admit while (!preempted); }

// SGLang, no mixed chunk: the chunked request and new prefills alone;
// a decode batch only when no prefill forms
iteration {
  serve only (!decoding);
  admit;
  branch (tokens == 0) { serve; }
}

// TGI, no chunking: a new batch when enough wait or enough steps passed,
// prefilled in its own forward
state since = 0;
iteration {
  branch (queued(reqs) >= floor(ratio * residents) || since >= max_wait) { admit; }
  branch (admitted > 0) { set since = 0; } else { serve; set since = since + 1; }
}

// TensorRT-LLM STATIC_BATCH
iteration { serve; branch (residents == 0) { admit; } }

// FasterTransformer as Dai et al. model it
iteration { branch (decoders > 0) { serve only (decoding); } else { serve; admit; } }
```

`exclusive prefill` and `serve only` become bodies, and V1 and V2 go with
them: neither is a rule any more, and the opposite of each is a program.
`state` (V7, #367) is a stage's register, written only in its own `iteration`
and read by that stage and the pools it admits; SGLang's ratio is one, written after a retraction
from the residents' totals.

The memory-side rules are the pool's in both designs, and are options in
both:

- `preempt by (k₁, …) [requeue head | requeue tail]`, the victim the
  resident of the engine with the least keys, ascending as `queue by` and
  `serve by` are (V3, V4; #356, #360). `preempt lifo` is `preempt by
  (-admission) requeue head`. The keys read a candidate's attributes,
  `admission`, `decoding` and `position`, where its hold is now; SGLang is
  `preempt by (1 - decoding, position - prompt, -prompt) requeue tail`.
- `reserve held`, a hold's `reserve` counted against every later admission
  for its scope: `used + Σ max(0, rᵢ − allocᵢ) + r ≤ cap` (V5). TensorRT-LLM
  is `kv (prompt) reserve (prompt + max_tokens)` with `growing kv`. SGLang's
  reservation shrinks as the request generates and is scaled by the ratio,
  which needs the keys of `reserve` re-read, as `queue by` keys are.
  Written today as a `reserve` that reads `holders(reqs)`, the test is
  judged once more when the session joins the queue, against the cap: under
  load it exceeds the cap and the session is rejected, where SGLang only
  makes it wait. Since #364 the part that reads state is not judged at the
  join, and waits; `examples/engines/sglang.sq` reads the load uncapped.
- `granule g` on a prefill run or on the stage: a grant is the whole
  remainder or a multiple of `g` (V6). `g = 1` is today; `g = inf` is
  TensorRT-LLM's default.

## The price

The iteration body is a new statement list in `CStep`, so a new IR node,
and `CServe` and `only` leave the IR for the body. IR 11 is not tagged, so
nothing bumps (`docs/ir.md` §Stability), but every consumer moves:

- the interpreter: `start_iteration` becomes an interpreter of the body. Its
  loop is the vLLM body's, so the existing tests are its regression;
- the Lean model: `Exec.startIteration` reads only `By([])` today. The vLLM
  body must give the same iteration (a theorem, not a test), and every
  other body is outside the fragment until the generator is taught it;
- `scripts/gen_lean_oracle.py` reads `CStep` by field name;
- `serving-queue-theory` reads the IR at a pinned version.

The pool options are each an added field with vLLM's default, which a
reader that ignores them reads as before only when they are absent; each
is a meaning change when present.

The order that buys the most for the least: `preempt by … requeue` (closes
V3 and V4 for SGLang, TensorRT-LLM and vLLM's PRIORITY), then the
`iteration` body (V1, V2, and retires two named rules), then `reserve held`
and `granule` (TensorRT-LLM exact), then `state` (SGLang's ratio). The
shared prefix cache is the largest item and is not about vLLM; it belongs
in its own design.

## Self-critique

- **B is a language inside the language.** A body can loop forever or
  admit nothing while waiting requests fit. A body is straight-line code
  with `branch` and no `loop`, each statement runs once where it is written,
  and a resident is served at most once in an iteration however many
  `serve`s the body has, so it terminates. Whether a body can leave an engine idle
  with work waiting is the #263 question, and the check is the one `serve
  only` already has: a body whose every path may skip both `serve` and
  `admit` while residents exist does not link.
- **A is cheaper and was rejected for the combinations, not the cost.**
  Every option in A is smaller than the body, and three of them (`preempt
  by`, `reserve held`, `granule`) are kept in B. What A cannot do is say how
  two iteration-level options meet without a third rule.
- **The programs here are not checked against their engines.** The
  correspondence is read from the source, as §7's was before the oracle.
  An oracle per engine — the real scheduler driven by a fake model runner,
  as `tools/vllm_oracle.py` drives vLLM's — would make each row a check.
  None needs a GPU; a GPU run, as for vLLM's A100 testbed, checks a cost
  model, not a scheduler.
- **SGLang's LPM waiting order** sorts by the prefix match in the shared
  tree. `queue by (-cachedin(kv))` links but matches only a session's own
  prefix, so it is LPM for multi-turn reuse alone. Real LPM waits for the
  shared cache.
