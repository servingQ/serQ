# Tutorial

Six chapters. Each one is a complete, runnable program that adds exactly one
idea to the last, and each ends with something you can measure.

By the end you will have built, from nothing, a colocated LLM engine with
continuous batching, chunked prefill, a block-level prefix cache and LIFO
preemption — and watched it fall off a cliff.

| | Chapter | The idea | New syntax |
|---|---|---|---|
| 1 | [A queue](01-a-queue.md) | requests wait for a server | `stage`, `workload`, `route`, `run`, `observe` |
| 2 | [Memory is a resource](02-memory.md) | requests also wait for *memory* | `pool`, `hold` |
| 3 | [Sessions and turns](03-sessions.md) | a session is many turns with thinking in between | `loop`, `turn`, `branch`, `delay` |
| 4 | [The prefix cache](04-prefix-cache.md) | a finished turn leaves its context behind | `cache`, `cached`, `evict`, `drop` |
| 5 | [The engine](05-the-engine.md) | prefill and decode share one iteration | `step`, `budget`, `growing`, `preempt`, `admit via` |
| 6 | [The cliff](06-the-cliff.md) | the cache and the queue feed each other | — |

The programs are in
[`docs/tutorial/programs/`](https://github.com/vrvrv/seQ/tree/main/docs/tutorial/programs).
Every number quoted in these pages came from running them.

!!! tip "Run as you read"
    ```bash
    cargo build --release
    ./target/release/seq-lang run docs/tutorial/programs/01-queue.seq
    ```
    Every chapter ends with a sweep you can reproduce with `--set`.
