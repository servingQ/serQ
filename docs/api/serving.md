# Serving vocabulary

Names for the parts of a request's life that are not one engine run. An
admission is the kernel's
[`hold … { … } cache (…)`](statements.md#hold), which an
engine's scheduler writes as `taking`, `reuse` and `cache`, reading the
engine's state of the request (`request.known`, `request.hit`). Prefill and
decode are the mode of a [`run`](statements.md#run) on a step engine, which
runs them for the request it keeps: the server and a queue's entry call the
engine with the request's input and output, `E (in, out);`.

Serving forms accept ordinary quantities and perform the named resource
[`cost`](functions.md#cost) conversion before the primitive operation.
An already converted cost belongs in `run` or `load` directly.

| Form | The request… | Kernel |
|---|---|---|
| [`transfer (X) from P to Q (n);`](#transfer-from-to) | has its KV moved to another instance | `run link (cost(link, X)); load Q (cost(Q, n)); release P;` |
| [`sleep (Z);`](workload.md#sleep) | is the client's for `Z`, between turns (a tool call, a person reading) | `run sleep (cost(sleep, Z));`, on a delay of the workload's own that the parser supplies |

Each form is shorthand for the [kernel statements](statements.md) in the
last column.

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
| `[j]` | `expr` | Index into the role's stage array: `transfer[j] (X) from P to Q (n);`. |
| `on STAGE` | `stage` | Names the stage explicitly: `transfer on S2 (X) from P to Q (n);`. |

`run link (cost(link, X)); load Q (cost(Q, n)); release P;`. Without `[j]`
or `on`, the form finds its link among the stages declared above it: the
stage named `link` or `transfer`. Exactly one must qualify: with none or
both the parser stops at the form. The run is plain: a `transfer` on a
step engine is a link error. A link that only takes time is `run link
(cost(link, X));`. The KV of a prefill/decode split lives
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
  pool kvP { cap 100; }
  pool kvD { cap 100; }
  stage prefill : fifo;
  stage link : ps(1);
  stage decode : fifo;
  server {
    hold kvP (cost(kvP, 8)) {
      run prefill (cost(prefill, 1));
    } lease kvP (inf);
    hold kvD (cost(kvD, 8)) {
      transfer (0.5) from kvP to kvD (8);
      run decode (cost(decode, 3));
    }
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
