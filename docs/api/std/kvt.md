# std/kvt

```serq
use "std/kvt";
```

A *relation* is an item of `main`, written once between two queues:
which queue's entries read the KV from which, and who starts each copy.
The source comes first, as the bytes go. See [Queues](../../language.md#queues) for the
queues and roles these calls name.

## `kvt.nixl_pull` and `kvt.nixl_push`

```serq
kvt.nixl_pull (SOURCE, READER, latency = x, share = POLICY);
kvt.nixl_push (SOURCE, READER, latency = x, share = POLICY);
```

NIXL's two modes of one copy between two queues with a `nic`, declared
above the call. `kvt.nixl_pull` is the *pull relation*: the KV goes from
`SOURCE` to `READER`, and the reader's worker starts each read, waiting
`x` before it (`READER.nic.latency`). `kvt.nixl_push` is the same copy
written by the source (NIXL push): the source's worker posts the write
and waits `x` before it (`SOURCE.nic.latency`). In both, an entry of the
reader called `from SOURCE[i]` reads with a `transfer` without `on`,
over both NICs.

| Argument | Type | Description |
|---|---|---|
| `SOURCE` | queue | The queue the KV leaves; it has a `nic`. |
| `READER` | queue | The queue the KV arrives in; it has a `nic` and an entry `from SOURCE`. |
| `latency = x` | `const` | The wait before each copy; optional. |
| `share = POLICY` | [std/share](share.md) | How concurrent copies divide the two NICs, `share.maxmin` or `share.bottleneck`; required. |

A reader with no entry `from` the source is refused at the call.
Concurrent copies divide the two NICs by the relation's `share`, which
has no default; two relations whose copies cross one NIC are one fabric and
name one policy.
A push and a pull of the same constants run the same numbers. Used in
`examples/pd-disaggregation/llmd_nixl_pull.sq` and `vllm_nixl_push.sq` ([the P/D use
case](../../use-cases/pd.md)).

## An L2 KV cache

A copy through an L2 KV cache outside the engine (an LMCache server, a
Mooncake store), a queue that plays `l2`, is the engine's
[`connector`](../engine.md#connector), not a call of this module:
`kvt.l2_load (…)` and `kvt.l2_store (…)` are refused with the connector to
write instead (#555).

## See also

[`use`](../program.md#use), [Queues](../../language.md#queues),
[`connector`](../engine.md#connector),
[`transfer`](../serving.md#transfer-from-to),
[the P/D use case](../../use-cases/pd.md).
