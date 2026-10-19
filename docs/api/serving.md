# Serving vocabulary

Names for the parts of a request's life that are not one engine run. An
admission is the kernel's
[`hold … { … } cache (…)`](statements.md#hold), which an
engine's scheduler writes as `taking`, `reuse` and `cache`, reading the
engine's state of the request (`request.known`, `request.hit`). Prefill and
decode are the mode of a [`run`](statements.md#run) on a step engine; the
server calls the engine with the request's input and output, `E (in, out);`,
and a queue's entry writes `run P prefill (cost(P, T));`.

Serving forms accept ordinary quantities and perform the named resource
[`cost`](functions.md#cost) conversion before the primitive operation.
An already converted cost belongs in `run` or `load` directly.

| Form | The request… | Kernel |
|---|---|---|
| [`tool Z;`](#tool) | waits outside the engine (a tool call, a person reading) | `run tool (cost(tool, Z));`, on the workload's own delay when no stage `tool` is declared; the kernel form alone needs the declared stage (without it, `run tool` is an unknown stage) |
| [`transfer (X) from P to Q (n);`](#transfer-from-to) | has its KV moved to another instance | `run link (cost(link, X)); load Q (cost(Q, n)); release P;` |

Each form is shorthand for the [kernel statements](statements.md) in the
last column.

## `tool`

```serq
tool [ '[' j ']' | on STAGE [, STAGE]* ] work;
```

| Argument | Type | Description |
|---|---|---|
| `[j]` | `expr` | Index into the role's stage array: `tool[j] Z;`. |
| `on STAGE` | `stage` | Names the stage explicitly: `tool on S2 (Z);`. |
| `work` | `expr` | Clock time on a `fifo`, `ps` or `delay` stage. |

### Stage selection

Without `[j]` or `on`, the form finds its stage among those
declared above it: the stage named for the role (`link` or `transfer`,
`tool`). Exactly one must qualify: with none or several the parser stops
at the form. In the workload, a `tool` with none declared runs on a delay
of the workload's own: the parser supplies `stage tool : delay;` after the
declared stages, so a client's tool call declares nothing. A tool server
with capacity, which sessions share, is declared in the workload file,
which runs on its own stages only (`stage sandbox[4] : fifo;`), and named,
`tool on sandbox[j] (Z);`; a stage of the deployment's is refused there
("stage `sandbox` is the deployment file's"). The run is plain: `transfer` and `tool` on a step engine are
link errors, so they take no `growing`, which needs one. `transfer` finds its stage by the same rule and
always says where the KV goes ([below](#transfer-from-to)); a link that only
takes time is `run link (cost(link, X));`.

## `transfer … from … to`

```serq
transfer [ '[' j ']' | on STAGE [, STAGE]* ] (X) from P to Q (n);
```

| Argument | Type | Description |
|---|---|---|
| `X` | `expr` | The link's work, in its stages' unit: time at a `ps(1)`, tokens at a `ps` of tokens per second. |
| `P` | `pool` | The session's lease (or hold) the KV comes from. Given back at the end. |
| `Q` | `pool` | The session's hold the KV arrives in. |
| `n` | `expr` | Tokens counted as computed at `Q`. |

`run link (cost(link, X)); load Q (cost(Q, n)); release P;`. The KV of a prefill/decode split lives
in two pools whose lifetimes overlap without nesting: the decode instance
allocates before the prefill instance frees. `on a, b` names several stages
the transfer uses at once, the sender's link and the receiver's
([`run`](statements.md#run)): `transfer on egress[i], ingress[j] (X) from P to Q (n);`
is `run egress[i], ingress[j] (cost(egress, ingress, X)); load Q (cost(Q, n)); release P;`. A link queue with
a `latency` (`serve ps(BwD) latency x0;`) is waited first, at the named
stages and taking none of their capacity: `run … (…, share = s, latency = x0)`,
the named links' summed in the order named, or the transfer's own
`(X, share = s, latency = x0)` on links without one. The stage report
counts it as the run's wait.

### Example

Inside an entry `from P` of a queue with a KV relation, `P` is the KV the
request leased on the prefiller. The NICs are the pods' (`nic ps(BwD);`),
and the relation `kvt.nixl_pull (P, D, latency = x0);` from `std/kvt`
makes a `transfer (n) from P to kv (m)` without `on` the read over
`P.nic[i]` and `D.nic[j]`, after the decoder's wait `x0`. A decoder whose
engine reads the KV itself says it with its connector instead
([engine](engine.md#connector)), as `examples/pd-disaggregation/llmd_nixl_pull.sq`
does:

```serq
decode (prompt) from P {
  set produced = min(o, max_model_len - prompt);   // generation stops at max_model_len tokens (sched/utils.py:114-120)
  D (prompt, produced);
}
```

## Examples

A complete program, a deployment and its workload:

```serq title="model.sq"
fn main() {
  device gpu { }
  engine llm on gpu {
    tokens cap 8;
    scheduler { advance running; admit waiting while (running.preempted == 0); }
    worker {
      forward (1);
    }
  }
  stage tool : delay;
  server {
    llm (8, 3);
    tool (3);
    observe finished = now;
  }
}
```

```serq title="workload.sq"
workload {
  arrive batch(1);
}
```

Save the deployment as `model.sq` and its workload as `workload.sq`, then run:

```sh
serq run model.sq --workload workload.sq --horizon 10
```

## See also

[Statements](statements.md), [stages](stage.md), [workloads](workload.md).
