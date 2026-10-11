# Stage

```serq
stage NAME [ '[' N ']' ] : kind;
```

A stage is where time passes, and where a [`run`](statements.md#run) takes it.

| Kind | Servers | Unit of `run`'s work |
|---|---|---|
| [`fifo(c)`](#fifo) | `c`, one job each | clock time at rate 1 |
| [`ps(φ)`](#ps) | all jobs at once | clock time, at rate `φ(present)/present` per job |
| [`delay`](#delay) | infinite | clock time at rate 1 |

The clock has no unit of its own: costs in seconds run in seconds. A stage
that runs iterations, whose work is tokens, is an [engine](engine.md) on a
device, written `engine NAME on DEVICE { … }`.

## `fifo`

```serq
fifo [ ( c ) ]
```

| Argument | Type | Default | Description |
|---|---|---|---|
| `c` | `const`, a positive integer | `1` | Servers. Jobs are served in arrival order. |

## `ps`

```serq
ps ( expr )
```

| Argument | Type | Moment | Description |
|---|---|---|---|
| `expr` | `expr` | `Ps` | Total throughput `φ(present)`, shared equally by the jobs present. The [context variable](context.md) `present` is the number of jobs. |

```serq
stage svc : ps(1);
```

(`examples/single-turn/ps.sq`, M/G/1-PS.) A `ps` stage some run holds
together with another is served by the program's
[`share`](program.md#share) instead: its jobs are flows, not an equal
split.

## `delay`

```serq
delay
```

Every job proceeds at rate 1 with no waiting. It is `ps(present)`, each of
`present` jobs at `present/present`, and links to the same IR: the run, the
drawing and the Lean model read the two spellings alike.

## Examples

A complete program:

```serq
fn main() {
  stage svc : fifo;
  workload {
    arrive poisson(0.5);
  }
  server {
    set t0 = now;
    run svc (cost(svc, ~exp(1)));
    observe response = now - t0;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[Engine](engine.md), [`run`](statements.md#run), [context variables](context.md),
[`pyserq.Stage`](../python/stage.md).
