# The outline of a text

Where each declaration of a program's text is written: for an editor that
changes the text in place, as [serq-lar](https://github.com/servingQ/serq-lar)
does from the figure. It finds the place here and leaves the meaning to
the compiler: the outline says where `queue D` is written, not what `D`
does.

In the API it is `serq::outline(&src)`, a `serq::frontend::outline::Outline`,
and `Outline::to_json()`. It reads a deployment alone: the program's text
without the workload it runs with, which is a file of its own
([paired files](../language.md#paired-files)). For the two files,
`serq::outline_paired(&deployment, &workload)` gives each file's outline,
its ranges in that file's own bytes. It parses and does not link, so a program with a
link error has an outline; a parse error is the diagnostic `serq check`
prints. Nothing in it is part of the program: it adds no syntax and no IR,
and the parser records it only when asked.

## Shape

Every range is a half-open range of bytes of the text, `[start, end]`
(UTF-8 bytes, not columns). A declaration's range runs from its first
word to its `;` or `}`; a `body` is what is between its braces.

| Field | Each item | Range |
|---|---|---|
| `main` | `{range, body}` | `fn main() { … }` |
| `uses` | `{path, range}` | `use "std/kvt";` |
| `decls` | `{kind, name, range}`, `kind` one of `let`, `stage`, `engine`, `pool`, `device` | a declaration outside a queue, in order |
| `queues` | `{name, roles, family, range, body, members, accepts}` | `queue P[NP] : prefill { … }`; `family` is the text between the brackets, or null |
| `queues[].members` | `{kind, name?, from?, range, body?}`, `kind` one of `device`, `engine`, `pool`, `nic`, `serve`, `entry` | in order; a device's or pool's `name`, an entry's verb as `name` and its `from Q` |
| `queues[].accepts.pools` | `{on, cap, admitted_by}` | where the queue may still take a pool: each capacity of its engine and devices no pool is on, as `pool NAME on …` spells it there (`on` is `Q`, `gpu` or `Q.gpu`), and the engine that would admit it, or null (`on gpu`: none; `on Q.gpu`: `Q`). The device an engine runs on holds one pool, so once it has one it offers none. What the members already say is not repeated: an entry is what a call names (a NIC, a pool or a device is not), a `nic` is what a KV copy's end needs, a `gateway` routes |
| `relations` | `{mode, source, reader, range, function}` | `kvt.nixl_pull (P, D, …);`; `mode` is `pull` or `push`, `function` the range of `nixl_pull` |
| `routes` | `{gateway, range, body}` | a gateway's `route { … }` |
| `server` | `{range, body}` | `server { … }` |
| `workload` | a range | `workload { … }` |
| `calls` | `{queue, index, verb, from, range, block, owner?}` | `D[j].decode (prompt) from P[i];` to its `;`; `index` the text between the brackets; `block` is `server`, `route` or `entry`, `owner` the queue whose route or entry it is in |
| `chooses` | `{var, count, keys, by, range, block, owner?}` | `choose i in NP by (work(P[i]), ~uniform(0, 1));` to its `;`; `count` the range of the count after `in`, `keys` the range of each key in order, `by` the key list with its parentheses; `block` and `owner` as a call's |
| `workload_items` | `{kind, name?, block, value, range}` | a statement of the workload, to its `;`, in order: `kind` its first word (`arrive`, `send`, `set`, `tool`, `turn`, …), `name` a `set`'s, `block` where it is (`workload` at its top, or `init`, `turn`, `session`), `value` what it says or null: an `arrive`'s process (`poisson(Lambda)`), a `set`'s right-hand side, what a `tool` or `send` takes between its parentheses. A statement holding a block (`while`, `if`) is not an item; the statements in it are |
| `devices` | `{name, engine, memory, caps}` | every `device`: `name` as a pool's `on` names it (`gpu`, `D.gpu` in queue `D`), `engine` the engine that runs on it or null, `memory` the engine's memory, its KV: the first `pool … on` the device, or null without an engine |
| `engines` | `{name, device, caps}` | every engine, and the device it runs on |
| `devices[].caps`, `engines[].caps` | `{name, range, pool, admitted_by}` | each capacity declared, `NAME cap …;`: `range` its name where it is declared, `pool` the pool on it (`pool NAME on OWNER`, as the program names it: `D.kv`) or null, `admitted_by` the engine whose scheduler admits that pool's queue (the engine it is on, or `on ENGINE.DEVICE`'s) or null for a pool `on DEVICE`, an engine's KV too (`pool kv on gpu`), a hold is admitted as soon as it fits. The IR's `admit_via`, read without linking |

A device an engine runs on holds one pool, its KV
([`E-device-two-pools`](diagnostics.md)); a pool on a device or an engine
is one of its capacities ([`E-pool-no-capacity`](diagnostics.md)). An
editor reads where a pool may go from `devices` and `engines`, the rule
from the refusal. The outline lists what is written: a text with two
pools on an engine's device, which does not link, lists both, and the
first `pool … on` it is its `memory`, as the linker takes it.

A call, a `choose` or a workload statement a `def` expands to is written
in the `def`, and is not among the `calls`, the `chooses` or the
`workload_items`.

A pair (a deployment file and a workload file, `serq::outline_paired`) has
two outlines, each with ranges into its own file: the workload file's has
its `uses`, `decls`, `workload` and `workload_items`; the deployment's,
the rest.

From `examples/pd-disaggregation/llmd_nixl_pull.sq`, a few items:

```json
{
  "relations": [{"mode": "pull", "source": "P", "reader": "D",
                 "range": [4220, 4277], "function": [4224, 4233]}],
  "calls": [{"queue": "P", "index": "i", "verb": "prefill", "from": null,
             "range": [1673, 1694], "block": "route", "owner": "gw"}, …]
}
```

`&src[4220..4277]` is `kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);`.
An editor that turns the pull into a push replaces `function`; one that
sends requests to another decoder writes a call after the last of
`calls`; one that changes how the router picks a decoder replaces a
`choose`'s `by`, or one of its `keys`; one that changes how often
sessions arrive replaces the `arrive` item's `value`.
