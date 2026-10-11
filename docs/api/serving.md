# Serving vocabulary

Names for the parts of a request's life that are not one engine run. An
admission is the kernel's
[`hold … at admission (…) { … } cache (…)`](statements.md#hold). Prefill and
decode are the mode of a [`run`](statements.md#run) on a step engine,
`run E prefill (cost(E, T));` and `run E decode (cost(E, T));`, so the
engine is always written.

Serving forms accept ordinary quantities and perform the named resource
[`cost`](functions.md#cost) conversion before the primitive operation.
An already converted cost belongs in `run` or `load` directly.

| Form | The request… | Kernel |
|---|---|---|
| [`tool Z;`](#tool) | waits outside the engine (a tool call, a person reading) | `run tool (cost(tool, Z));` |
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
at the form. The run is plain: `transfer` and `tool` on a step engine are
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
a `latency` (`serve ps(BwD) latency x0;`) is waited first: each named link
that has one adds `run L.latency[k] (cost(L.latency, x0));` before the `run`, in the order
named.

### Example

From `examples/pd-disaggregation/llmd_nixl_pull.sq`, the decoder's read of
the prefiller's leased blocks, over the prefiller's NIC and its own, in the
decoder's `decode (prompt) from src` entry (`src` is the prefiller's leased
pool). The NICs are the pods' (`nic ps(BwD);`), and the relation `D pull P
latency x0 share maxmin;` makes a `transfer` without `on` the read over
`P.nic[i]` and `D.nic[j]`, after the decoder's wait `x0`:

```serq
hold kv (cost(kv, known)) reserve (cost(kv, known)), reqs (cost(reqs, 0)) reserve (cost(reqs, 1)) … {
  transfer (prompt - c) from src to kv (prompt - 1 - c);   // over P's NIC and this pod's
  …
} cache (cost(kv, reqs, prompt + o));
```

## Examples

A complete program:

```serq
fn main() {
  device gpu { }
  engine llm on gpu {
    tokens cap 8;
    schedule { advance running; admit waiting while (running.preempted == 0); }
    execute (1);
  }
  stage tool : delay;
  workload {
    arrive batch(1);
  }
  server {
    run llm prefill (cost(llm, 8));
    run llm decode (cost(llm, 2));
    tool (3);
    observe finished = now;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[Statements](statements.md), [stages](stage.md), [workloads](workload.md).
