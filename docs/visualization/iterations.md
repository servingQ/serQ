# Iteration view

The [deployment view](deployment.md) draws a program and runs nothing. The
iteration view draws a run: one engine, iteration by iteration, with the
requests in each batch, the tokens each took, and what the engine's memory
held. It reads the [run log](../reference/log.md) a run wrote and runs
nothing either:

```bash
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 600 --seed 1 --Lambda 2.0 --log cliff.json
serq draw --log cliff.json --from 591 --iterations 120 --format svg --out cliff.svg
```

![The cliff's engine at 2.0 sessions/s, 120 iterations from t = 591](../assets/cliff-2.0.iterations.svg)

!!! warning "Experimental notation"
    Diagram notation, options and output may change between releases.

## Reading it

- **Columns** are the engine's iterations, one each, in order: the first
  at or after `--from`, and at most `--iterations` of them (120 unless
  told). The times under the strips are every twentieth iteration's. The
  axis counts iterations, not seconds: a long prefill is as wide as a short
  decode.
- **Rows** are the requests in those batches, one per session (`34/0` for
  a session's leg), in the order they first appear.
- **A cell** is what its request took of the iteration: orange while it
  prefills, blue while it decodes (its run's mode), as tall as its tokens'
  share of the iteration's `tokens cap`. A decode of one token is the
  thinnest line drawn; a prefill chunk of the whole cap fills its row.
- **A cross** in a row is the request's preemption, at the engine's first
  iteration after it.
- **The tokens strip** stacks each iteration's prefill tokens under its
  decode tokens against its `tokens cap`, as the engine read it that
  iteration (the log's `budget`). An engine with no cap is drawn against
  the window's largest iteration, and says so.
- **The memory strip** stacks the engine's memory pool, the units
  allocated (`used`) under the units cached, against the pool's `cap`; the
  two together never pass it. A triangle over a column is an eviction from
  the cache since the engine's previous iteration. An engine with no
  memory pool has no strip.

In the figure above, the engine is short of blocks, not of tokens: its
iterations take 249 of their 2048 tokens on average, the batch holds at
most 9 of its 16 slots (`reqs cap`), and the memory is allocated nearly to
its cap with little cached, so half the columns evict; one request is
preempted. The prefills, which recompute what the cache no longer holds,
are 17 of the 120 columns and 95 % of the window's time. [The cliff](../tutorial/06-the-cliff.md#one-engine-iteration-by-iteration)
compares it with the same engine below the cliff.

## Options

`serq draw --log LOG` takes no program: the log names the stages and
pools it refers to, and gives each pool's cap.

| Flag | Meaning |
|---|---|
| `--log LOG` | the log `serq run … --log LOG` wrote |
| `--engine NAME` | the engine, as the log names it: `llm`, or `D[0]` for a member of an array; needed when several engines iterate |
| `--from T` | the first iteration drawn is the first at or after `T` seconds (default 0) |
| `--iterations N` | at most `N` columns (default 120) |
| `--format` | `tikz` (default) or `svg` |
| `--out PATH` | write to a file instead of stdout |

A window past the engine's last iteration is an error that gives the time
of its last. A log of an older serq, written before what the figure reads
(each request's tokens, the evictions, a pool's cap), is refused: run the
program again. A log the run's event cap truncated is drawn as far as it
goes, and the figure says so.

## Limitations

- The axis is iterations, not time: an iteration's duration is not drawn.
- A row is a session (or a leg), as the log names a batch's requests: two
  engine calls of one session in the window share a row.
- The figure is the batch, not the queue: the requests waiting for an
  engine are not drawn, nor its `reqs cap`.
- An eviction is marked, not measured: the triangle says that the cache
  lost units, not how many.
- A long run's log is large: 600 s of the engine above is 41 MB at 2.0
  sessions/s and 88 MB at 1.5, where its iterations are shorter and more.
