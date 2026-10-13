# std/share

```serq
use "std/share";
```

The sharing policies: how the flows that hold several `ps` stages at once
divide the stages' capacity. A policy is a value a job over several stages
names, `share = share.maxmin`, in a [run](../statements.md#run) or `transfer
on` over several stages or in a [relation](kvt.md#kvtnixl_pull-and-kvtnixl_push)
of std/kvt:

```serq
kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);
run egress[i], ingress[j] (cost(egress, ingress, tokens), share = share.maxmin);
```

| Policy | A flow's rate |
|---|---|
| `share.maxmin` | Max-min fair: every flow's rate rises together until a stage fills; the flows through it stop there, the others go on. Uses all the capacity it can. |
| `share.bottleneck` | Its equal share at the tightest of its stages, `min over s of φ_s / n_s`. What that leaves at its other stages is unused. |

A job over several stages names its policy, which has no default; a job on
one stage names none. The stages that jobs join, however indirectly, are
one *fabric*: their flows are solved together, so a fabric has one policy,
and runs that join their stages and name two do not link. Two fabrics that
share no stage may name two (NVLink inside a node one way, the network
between nodes another). Every job on a stage of a fabric, a single-stage
run included, is a flow of the fabric's policy.

Writing `share.…` without the import is an error, and so is declaring
something named `share` beside it. The policies are the IR's `Share`
(`Run.share`, [the IR](../../ir.md)).
