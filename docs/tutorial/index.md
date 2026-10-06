# Tutorial

Build a serving model in five runnable programs, then explore its behavior
under load in chapter 6.

By the end you will have built, from nothing, a colocated LLM engine with
continuous batching, chunked prefill, a block-level prefix cache and LIFO
preemption — and watched it fall off a cliff.

| | Chapter | The idea | New syntax |
|---|---|---|---|
| 1 | [A queue](01-a-queue.md) | requests wait for a server | `stage`, `workload`, `session`, `run`, `observe` |
| 2 | [Memory is a resource](02-memory.md) | requests also wait for *memory* | `pool`, `hold` |
| 3 | [Sessions and turns](03-sessions.md) | a session is many turns with thinking in between | `loop`, `turn`, `branch`, `delay` |
| 4 | [The prefix cache](04-prefix-cache.md) | a finished turn leaves its context behind | `cache`, `cached`, `evict`, `drop` |
| 5 | [The engine](05-the-engine.md) | prefill and decode share one iteration | `step`, `budget`, `growing`, `preempt`, `admit via` |
| 6 | [The cliff](06-the-cliff.md) | the cache and the queue feed each other | — |

The programs are in
[`docs/tutorial/programs/`](https://github.com/servingQ/serQ/tree/main/docs/tutorial/programs).
Run the examples with the listed settings to reproduce the reports.

!!! tip "Run as you read"
    ```bash
    cargo build --release
    ./target/release/serq run docs/tutorial/programs/01-queue.sq --horizon 100000 --warmup 5000 --seed 1
    ```
    Every chapter ends with a sweep you can reproduce with `--set`.
