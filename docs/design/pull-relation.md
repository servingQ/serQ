# The pull relation

A KV transfer between two pods is one fact of the deployment: which pod's
KV goes to which, who starts the copy, what it waits for, and how
concurrent copies share the wire. `examples/pd-disaggregation/llmd_nixl_pull.sq`
wrote that fact in three places, none of which said it. This document is
the design of `nic` and `D pull P latency x share s;`, issue #200: parse-time
sugar, no IR change.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.sq` at `80e55e0`, lines 78–87
and 156:

```serq
// The NICs: a prefiller's, which the reads out of it share, and a
// decoder's, which the reads into it share. A read holds both at once; how
// concurrent reads divide them is UCX's and the fabric's, not vLLM's, and
// max-min fairness is the model, not a measurement
// (docs/design/bandwidth-sharing.md).
queue egress[NP] : link { serve ps(BwP); }
// The decoder's worker posts the READ and finds it done at a later step:
// a fixed wait per read, not bandwidth.
queue ingress[ND] : link { serve ps(BwD) latency x0; }
share maxmin;
…
        transfer on egress[src], ingress[self] (prompt - c) from src to kv (prompt - 1 - c);
```

- The NICs are families of their own, next to the pods, and the program
  joins `egress[i]` to `P[i]` only through the index, with `src` read as a
  number (`egress[src]`), a pun the [queue design](queue.md) lists in its
  self-critique.
- `share maxmin;` stands alone: it says neither what is shared nor between
  whom. A reader finds out by reading the transfer.
- That the decoder reads (NIXL's pull mode) is in a comment. The latency is
  on `ingress` because the decoder's worker posts the READ, which the
  program does not say either.

## After

The program in the repository:

```serq
queue P[NP] : prefill {
  …
  nic ps(BwP);                          // the pod's NIC: the reads out of it share it
  prefill (prompt) { … }
}
queue D[ND] : decode {
  …
  nic ps(BwD);                          // the pod's NIC: the reads into it share it
  decode (prompt) from src {
    …
        transfer (prompt - c) from src to kv (prompt - 1 - c);   // over P's NIC and this pod's
  }
}
D pull P latency x0 share maxmin;
```

The two programs run the same: at seeds 1 and 7 every count, observable,
pool row and stage row is byte-identical once `egress`, `ingress` and
`ingress.latency` are read as `P.nic`, `D.nic` and `D.nic.latency`.

## The design

**A pod owns its NIC.** `nic kind;` in a queue is the stage `Q.nic`, one per
member, declared after the `serve` and before the entries. It is a stage
like any other: `transfer on P.nic[i], …` still works, and the deployment
figure puts it in the pod's box.

**The relation is one line for the topology and the mode.** `D pull P
latency x share s;`, after both queues:

| Part | Says |
|---|---|
| `D pull P` | the KV goes from `P` to `D`, and `D` starts the copy |
| `latency x` | `D` waits `x` before each read (a number or a constant over `let`s); optional |
| `share s` | concurrent reads divide the two NICs by `s`: the program's `share`, written here |

**A `transfer` without `on` is the read.** Inside an entry of `D` called
`from P[i]`, `transfer (n) from src to kv (m);` names no stage, and the
relation supplies both ends:

| Surface | Kernel |
|---|---|
| `nic k;` in `queue Q[N]` | `stage Q.nic[N] : k;` |
| `D pull P latency x share s;` | `stage D.nic.latency[N] : delay; share s;`, `x` a constant of its own |
| `transfer (n) from src to kv (m)` in `D[j]`, called `from P[i]` | `run D.nic.latency[j] (x); run P.nic[i], D.nic[j] (n); load D.kv[j] (m); release P.kv[i];` |

The latency's constant has a name of its own (`D.pull.time`), as a link's
does, so that a parameter or a local of the entry with the same name does
not replace it, and `--set` still reaches the expression.

**Checks.** Each is a parse error, at the line:

- either queue is not declared above the relation, or has no `nic`;
- a reader has two relations, or reads from itself;
- the relation names no `share`, or a policy other than the program's;
- a `share` on its own as well as the relation's;
- an entry of `D` called `from` a queue other than `P`;
- a `transfer` without `on` in a queue that pulls from none.

## What it earns

- `llmd_nixl_pull.sq` says NIXL's pull mode in the program, not in a
  comment, and the policy says what it divides.
- The `src` pun is gone from the transfer: `egress[src]` was the only use.
- No IR change: the relation is the stages and runs the program wrote
  before.

## Self-critique

**No `push`.** `P push D` would read as the push a reader expects, vLLM's
proxy, which sends the prefill and the decode request at once
(`disagg_proxy_pushconnector_demo.py:227-270`), so that the decoder
allocates during the prefill and the prefiller writes as soon as it has
both the finished request and the decoder's registration
(`nixl/push_scheduler.py:207-294`). That needs a reservation the language does not
have ([The KV transfer](pd-transfer.md)). The push this model can write
today is llm-d's serial one, which differs from pull only in whose worker
waits; under the word `push` it would mean less than it says (criterion 0).
Serial push is still written with link queues and `transfer on`
([the P/D use case](../use-cases/pd.md), *The two modes in the program*).

**One policy per program.** The IR's `share` is the program's, so every
relation names the same. NVLink one way and RDMA another is a policy per
relation, and an IR change.

**Two places for a latency.** A link queue's `serve … latency` (#193) and
the relation's `latency`. They are not the same fact: the link's is a
property of the wire, waited by any transfer over it; the relation's is the
reader's, waited by the reads it starts. A program uses one or the other,
and a pod's `nic` takes no `latency` of its own.

**`nic` is one NIC.** A pod with a NIC per tensor-parallel rank, or a
separate NIC for each direction, is not written; the read's fan-out over
ranks is not modelled either.

**The relation is declared after both queues.** A forward reference
would read better at the top of a file, and the stage rule elsewhere is
"declared above", which the relation follows.
