# The entry's source

An entry that takes the KV another queue leased says which queue: `decode
(prompt) from P`. Issue #516: parse-time sugar, no IR change.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.sq` at `f644a99`, lines 141
and 156:

```serq
    decode (prompt) from src {
          transfer (prompt - c) from src to kv (prompt - 1 - c);   // over P's NIC and this pod's
```

`src` was a name the call bound: `from P[i]` made it the pool `P.kv[i]` where
a pool is read and the index `i` in an index. Any name linked, and the
header said neither which queue nor what the name was. The three programs
that read KV used it in one place, the `transfer`, where the relation, the
call and the source's lease had already fixed it.

## After

```serq
    decode (prompt) from P {
          transfer (prompt - c) from P to kv (prompt - 1 - c);   // over P's NIC and this pod's
```

The three programs' IR, and their runs at `--horizon 3000 --seed 7`, are
byte-identical to `f644a99`.

## The design

**`from P` names a queue.** In the header it is the queue whose entry
leased the KV; in the body `P` is that lease, the pool `P`'s entry leased
for this request on the member the call names (`P.kv[i]` for a call `from
P[i]`). It is never a number, not in an index either.

| Surface | Kernel |
|---|---|
| `D[j].decode (…) from P[i]`, to the entry `decode (…) from P` | the body, with `P` := `P.kv[i]` |
| `transfer (n) from P to kv (m)` in it, under `kvt.nixl_pull (D, P, …)` | `run D.nic.latency[j] (x); run P.nic[i], D.nic[j] (n); load D.kv[j] (m); release P.kv[i];` |

**Checks, each where the fact is written.**

- At the header: `P` is a queue declared above, not this one, and every
  entry of `P` leases its pool on every way through it.
- At the relation: an entry of the reader takes the KV from the relation's
  source.
- At the call: `from Q[i]` to an entry `from P` is refused.
- In the body: `P` read as a number, or in an index, is refused.

The first two were checked at every call before, against whatever the call
named; a source that leased nothing was reported at the gateway, not at the
entry that needed the lease.

## Self-critique

**`from P` is written twice.** The header and the `transfer` both name `P`,
and under a relation the header and `kvt.nixl_pull (D, P, …)` do too. A
`transfer (n) to kv (m)` with the source implied was considered: one word
less, but a
`transfer` without `from` in a relation entry and with it everywhere else
is two shapes of one statement, and the kernel's `transfer (X) from P to Q
(n)` always names both ends. Each repetition is checked against the other,
so writing a different one does not link.

**The source member's own link is a `nic`.** A link family indexed by the
source member (`transfer on egress[src], …`) has no spelling now: the
member's index is not readable in the entry. A pod's NIC is `nic` and the
[pull relation](pull-relation.md) runs the read over the source member's;
no shipped program used the index, and the tests that did share one
`egress` link instead.

**One `from` entry per verb.** A decoder that reads from two prefill
families (`decode from P`, `decode from P2`) could be told apart by the
name the header now carries, but a queue still has one `decode … from` and
one relation; lifting both is its own change.
