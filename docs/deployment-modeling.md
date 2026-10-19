# Deployment modeling

A serQ program describes a serving deployment. Before you write one, it helps
to know the model of a serving engine the language assumes, so that you can
place each part of a real engine. An engine has two components: a scheduler
that decides what each iteration computes and owns every resource, and a
worker that turns the scheduled batch into time. This page explains how the
two divide the work, where resources are taken and given back, and which
invariants every program keeps.

The language reference defines each construct:
[Engines](language.md#engines),
[Engines on devices](language.md#engines-on-devices) and
[An engine's request](language.md#an-engines-request). This page connects them.

## An engine is a scheduler and a worker

Every iteration of an engine runs in three steps:

1. The **scheduler** reads the running requests, the waiting queues and the
   pools. It chooses which requests advance and by how many tokens, admits
   waiting requests, and takes and gives back their resources. The result
   is a **batch**.
2. The **worker** reads the batch and returns the time its forward pass
   takes.
3. The clock advances by that time. The batch's tokens are then applied:
   prefills move forward, decodes sample a token, and finished requests
   leave.

`examples/multi-turn/vllm.sq` states each step in its own block:

```serq title="examples/multi-turn/vllm.sq"
--8<-- "examples/multi-turn/vllm.sq:running"
--8<-- "examples/multi-turn/vllm.sq:waiting"
```

```serq
--8<-- "examples/multi-turn/vllm.sq:worker"
```

| Component | serQ | What it reads | What it does |
|---|---|---|---|
| Scheduler | `scheduler { … }` | `running.…`, `waiting.…`, the pools, the engine's registers, and `request.…` in an admission | orders and advances the running requests, admits waiting ones, takes and gives back pools, preempts |
| Worker | `worker { forward (T); }` | `batch.…`, the device's time resources, constants and definitions | returns `T`, the time of the forward |

Only the batch passes from the scheduler to the worker. The worker cannot read
the residents or the queues, and the scheduler cannot read the batch's
timing. The linker enforces this split; the table in
[Engines on devices](language.md#engines-on-devices) lists which value each
clause may read.

vLLM has the same split. `Scheduler.schedule()` returns a `SchedulerOutput`
(`sched/output.py:232`) that says what each request is scheduled for. The
model runner applies it and runs the forward. Around the forward it only
starts a KV connector's loads and waits for its saves
(`kv_connector_model_runner_mixin.py:84-92`).

## The scheduler takes and gives back every resource

Every allocation and every release happens at one of the scheduler's events.
The worker never allocates or frees anything. Each row below is an event the
language knows, and a program states the amounts in its scheduler's clauses:

| Event | When | What happens to the pools | Where the program says it |
|---|---|---|---|
| Admission | `admit waiting` reaches the head of a queue | each pool of the admission is taken at once, all or nothing; the cached prefix is read | `taking P (units) reserve (r)`, `reuse (n)` |
| Growth | a running request's position passes its allocation | the KV pool grows block by block before the request is served; if it cannot, a victim is preempted | the pool's `block` and `preempt` |
| Preemption | a growth does not fit | the victim gives its pools back and caches what it computed; it keeps its tokens and waits again | `preempt lifo`, `preempt by (…) requeue head` |
| Release | a request has generated its output | the request gives its pools back and leaves its prefix cached | `cache (n)` |
| Connector load | admission of a request whose KV comes from elsewhere | the request is given blocks for the KV it loads and no new work until the KV arrives | `connector C { load (n); }` |
| Lease | a hold's scope ends while another component still reads its KV | the allocation outlives the hold until a `release` takes it, its time runs out, or the session ends | a hold's `lease P (t)` |

The amounts are read at the event, from what the scheduler sees then:
`request.hit` is read when the request is admitted, and `cache` when it
releases. For each expression, the moment it is read and the values it can
read are listed in [Moments](ir.md#moments).

### Every reference engine places them there

The examples model six engines. In each of them, the scheduler side allocates
and frees, and the model runner only uses the block indices it is given:

| Engine (example) | Allocation | Release | What the worker does |
|---|---|---|---|
| vLLM ([`vllm.sq`](use-cases/vllm.md)) | `allocate_slots` for running requests (`scheduler.py:742-813`) and waiting ones (`scheduler.py:1214-1226`) | `_free_request` (`scheduler.py:2628-2657`) | adds the scheduler's new block IDs to its block table and drops finished requests (`gpu_model_runner.py:1203-1217`) |
| SGLang (`sglang.sq`) | the scheduler's batch preparation: [request slot](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/mem_cache/allocation.py#L323), [KV slots](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/mem_cache/allocation.py#L396) | [`release_kv_cache`](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/managers/scheduler_components/batch_result_processor.py#L1432) when the batch's results are processed | writes KV at the indices in the batch |
| TGI (`tgi.sq`) | the router's queue, prompt plus `max_new_tokens` at once ([`queue.rs`](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/queue.rs#L321)) | dropping the allocation frees it ([`block_allocator.rs`](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/backends/v3/src/block_allocator.rs#L19-L22)) | the Python server [receives](https://github.com/huggingface/text-generation-inference/blob/b4adbf2f6e2e721280bd0ea5f91d70f7d033f5ed/server/text_generation_server/models/flash_causal_lm.py#L360-L361) the blocks and slots |
| TensorRT-LLM (`tensorrt_llm.sq`) | the executor's resource manager after scheduling ([context](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/_torch/pyexecutor/resource_manager.py#L1165), [generation](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/_torch/pyexecutor/resource_manager.py#L1191)) | [`free_resources`](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/_torch/pyexecutor/py_executor.py#L8219) | the model engine reads the block offsets |
| vllm-ascend ([`ascend.sq`](use-cases/ascend.md)) | vLLM's scheduler | vLLM's scheduler | the NPU model runner [delegates](https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/worker/model_runner_v1.py#L853) to vLLM's |
| vllm-rbln ([`rbln.sq`](use-cases/rbln.md)) | vLLM's scheduler, through its own KV cache manager | vLLM's scheduler | as vLLM's; it also copies KV blocks, but [the scheduler chooses](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/rbln_scheduler.py#L997) which ones |

During warmup and CUDA-graph capture, TGI and TensorRT-LLM let the model
runner allocate blocks for dummy requests. A serQ program has no warmup, so
the exception does not apply.

### Holding KV after the request ends

A prefill instance in a disaggregated deployment must keep a request's KV
after the request has finished, until the decode instance has read it. In
every engine above that supports disaggregation, this too is the scheduler's
decision. The worker only reports that the transfer is done:

- **vLLM.** When a request finishes, the scheduler asks its KV connector
  whether to free the blocks now (`scheduler.py:2628-2657`). NIXL's
  scheduler side keeps them for the lease duration
  (`nixl/pull_scheduler.py:248-269`). When the worker reports
  `finished_sending`, the scheduler frees them
  (`_update_from_kv_xfer_finished`, `scheduler.py:3139-3144`).
- **SGLang.** A finished prefill request goes to an
  [in-flight queue](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/disaggregation/prefill.py#L1015).
  The scheduler loop polls the KV sender and releases the request only
  [on success](https://github.com/sgl-project/sglang/blob/b792228b35b21565067520857319dfc05e4d134e/python/sglang/srt/disaggregation/prefill.py#L1182-L1188).
- **TensorRT-LLM.** A context-only request whose KV is still being sent
  [is not terminated](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/_torch/pyexecutor/py_executor.py#L8591).
  The transfer coordinator
  [terminates it](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/tensorrt_llm/_torch/disaggregation/orchestration/coordinator.py#L549)
  when the last transfer ends.
- **vllm-ascend.** Its Mooncake connector
  [delays the free](https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/distributed/kv_transfer/kv_p2p/mooncake_connector.py#L2036)
  in the same way as vLLM's NIXL connector.

In serQ the prefiller's `lease kv (inf)` says this, and the decoder's
connector ends the lease when its load is done
([Prefill/decode over NIXL](use-cases/pd.md)).

## The worker only takes time

`forward (T)` is the only item a worker has. `T` is read after the batch is
scheduled and can read only the batch. Within an engine, time passes only
here. Scheduling, admission, growth and release take no time.

Work that is not the forward runs on other stages, beside the iteration:

- a KV connector's copies run at the bandwidth of its L2 queue;
- a transfer between instances runs on the NICs;
- a tool call runs on its own stage.

The table above is why the language has no allocation or release in
`worker`: none of the reference engines puts them there.

## Invariants

A program can rely on the following invariants. Each one names the check that
holds it: the linker refuses a program that would break it, the run fails
with an error, or the Lean model proves it.

### Time and iterations

1. **Only the batch passes from the scheduler to the worker.** `forward`
   reads `batch.…` and nothing of the residents or the queues. The scheduler
   reads nothing of the forward's time. *Linker.*
2. **Decisions take no time.** Statements, admissions, growths, preemptions
   and releases happen at an instant. Time passes only at stages: an
   engine's iteration, a `fifo`, `ps` or `delay` stage, a NIC.
   *Semantics* ([Events and settling](language.md#events-and-settling)).
3. **An iteration sees every event of its instant.** An engine starts an
   iteration only after every event at that instant has been handled, so an
   arrival at the same instant is in its waiting queue. *Interpreter.*
4. **An iteration that schedules tokens lasts a positive time.** A `forward`
   that evaluates to 0 on a batch with tokens ends the run with an error.
   A step that only preempts may last zero time. *Run-time check*
   ([Every instant settles](language.md#every-instant-settles)).
5. **The batch fits the token budget.** An iteration schedules at most
   `tokens cap` tokens: a decoding request gets one, a prefilling request at
   most its `each at most`. *Semantics* ([Engines](language.md#engines)).
6. **A loop lets time pass.** Every path through a `loop` or `while` reaches
   a `run` with work, a `hold` whose body does, or `end`. A schedule that can
   neither advance nor admit does not link. What the linker cannot see is
   reported at run time: `stuck` sessions and `idle: engine …`
   ([diagnostics](reference/cli.md)). *Linker and run-time check.*

### Resources

7. **Admission is all or nothing.** The head of a queue is admitted when
   every pool it takes has room for `max(units, reserve)` beside what is
   allocated. A cached prefix never blocks an admission: it is evicted to
   make room. *Semantics* ([Pools](language.md#pools)).
8. **Memory is bounded.** On every pool, allocated plus cached is at most
   `cap` in every reachable configuration. A pool that preempts its leases
   at ticks may pass `cap` between ticks, and then nothing is cached.
   *Lean* (`SerqLang.Step.invariant` in `lean/Serq/Core.lean`) *and a check on
   every debug run.*
9. **Every unit is accounted for.** A pool's `used` equals what its holds and
   leases have allocated, with a prefix that several holds read counted
   once. Its `cached` equals the cached entries of its trie. A pool has one
   holder per hold and per lease. An allocation released twice, or never
   released, fails this check even when the pool stays within its cap.
   *Check on every debug run.*
10. **A reservation is kept.** On a pool with `reserve held`, what is
    allocated plus what live holds have reserved but not yet allocated fits
    `cap`, so a hold that grows within its reservation always finds room.
    *Check on every debug run.*
11. **An allocation has one owner.** `grow`, `shrink`, `load`, `release` and
    a `growing` run act on the enclosing hold of the same pool, or on a lease
    the session took. A hold leases only its own pool. *Validation*
    ([Validation](ir.md#validation)).

### Requests

12. **A request keeps its tokens, not its KV.** A preempted request gives its
    pools back, caching what it computed. It keeps every token it has
    (`request.known`) and what it has generated, and recomputes what the
    cache no longer holds when it is admitted again. *Semantics*
    ([An engine's request](language.md#an-engines-request)).
13. **A request samples when its KV covers its tokens.** The prefill computes
    `request.known` less what the admission read from the cache. When it is
    done, the request samples a token. A request that loads KV for every
    token computes its last token again in order to sample. *Semantics.*
14. **Token counts are whole.** Inputs, outputs, budgets, chunks, KV units
    and a connector's amounts are whole numbers, provably so from the
    expressions that compute them. *Validation*
    ([Token counts](language.md#token-counts)).
15. **An expression reads only what its moment supplies.** For example, a
    hold's header is read again at every admission attempt, so it may not
    draw. An `only` predicate may not read `now`. A gauge reads no session.
    *Validation* ([Moments](ir.md#moments)).

## What the model leaves out

These assumptions affect how you read a result:

- **Scheduling is free.** The scheduler's CPU time is not modelled. Its cost
  appears only if the program adds it to `forward`.
- **Releases take effect at once.** A release frees its blocks for the next
  iteration. SGLang and TensorRT-LLM with overlap scheduling process a
  batch's results after launching the next batch, so their frees arrive one
  iteration later.
- **An empty step costs a forward.** A step that only preempts lasts
  `forward` at zero tokens. A real engine skips that forward pass.
- The prefix cache, leases and the Lean fragment have further limits, listed
  under [Known limitations](language.md#9-known-limitations).
