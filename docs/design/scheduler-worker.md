# The scheduler and the worker

An engine is two components, as vLLM is: a scheduler that chooses what an
iteration computes, and a worker that computes it. The engine's blocks are
named for them, `scheduler { … }` and `worker { … }`, and what passes from
one to the other is the batch (#555).

This document is the record of #555's first two steps, which are parse-time
renames: the IR does not move. Steps 3 and 4 (a request as the engine's
state, and a KV connector's load and save) extend it.

## In vLLM

`Scheduler.schedule()` returns a `SchedulerOutput`
(`vllm/v1/core/sched/output.py:232`): the tokens scheduled per request and
in total (`num_scheduled_tokens`, `total_num_scheduled_tokens`), the
requests finished and preempted, and the connector's metadata. The model
runner takes it and runs the forward; a KV connector's loads start around
the forward and its saves are waited for after it
(`kv_connector_model_runner_mixin.py:84-92`). Nothing the runner does reads
the waiting queue or a request the scheduler did not schedule.

## In serQ

```serq
  engine vllm on gpu {
    reqs cap max_seqs;
    tokens cap B;
    scheduler {
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold)
        taking reqs (1), kv (min(known, reused + budget_left(vllm))) reserve (known);
    }
    worker {
      forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
    }
  }
```

- `scheduler { … }` was `schedule { … }`; the old word is refused with the
  new one. Its statements, and what they read, are unchanged.
- `worker { forward (T); }` was `worker { forward (T); }`, refused with the new form.
  `forward` is the only item a worker takes until step 4 adds `load` and
  `save`; it is required, once.
- The worker reads the batch, `batch.…`, the device's time resources, the
  constants and the program's definitions. It does not read `running.…` or
  `waiting.…`: what the residents and the queues hold beside the batch is
  the scheduler's. `worker` read `running.count` and `waiting.count`; no
  program did, and the check is stricter, so the IR version does not move.

## Self-critique

**`return SchedulerOutput(…)` in the scheduler, `worker (out) { … }`.**
It would say the hand-off in the text. But the batch is what the
scheduler's statements scheduled, which the language computes; a program
that wrote it could write a `tokens` other than the tokens scheduled, two
sources for one quantity (criterion 0), and the language has no record
type to write it with. The hand-off is a check instead: the worker reads
`batch.…` and nothing else of the iteration.

**`before_execute` and `after_execute` hooks.** A hook that adds time is a
term of the forward's time, a second spelling of it; a hook of arbitrary
statements per iteration is a procedure in the engine again. The worker
takes named items, each of which says what it does to the state.

**`schedule` kept, `worker` added.** `schedule` names the act and `worker`
a component; a reader of vLLM finds `Scheduler` and the model runner, two
components. Both blocks are named for what they are.
