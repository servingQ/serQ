# std/kvt

```serq
use "std/kvt";
```

Two kinds of call. A *relation* is an item of `main`, written once
between two queues: which queue's entries read the KV from which, and
who starts each copy. A *copy* is a statement in a body: it moves the KV
of some tokens through an L2 KV cache. In every call the source comes
first, as the bytes go. See [Queues](../../language.md#queues) for the
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

## `kvt.l2_load` and `kvt.l2_store`

```serq
kvt.l2_load (CACHE, POOL, n, per_token = r);
kvt.l2_store (POOL, CACHE, n, per_token = r);
```

Copies through an L2 KV cache outside the engine (an LMCache server, a
Mooncake store): a queue that plays `l2`, with one pool, the KV it holds,
and a `serve`, the bandwidth its copies divide.

`kvt.l2_load` copies the KV of `n` tokens the cache holds into the
enclosing hold on `POOL`, which counts them as computed; the cache keeps
them. It is `run CACHE (cost(CACHE, n · r)); load POOL (cost(POOL, n));`,
which a request may also write itself.

`kvt.l2_store` makes the session's entry in the cache hold `n` tokens and
copies only the part it lacks, `max(0, n - cached)`, at the cache's
`serve`. Its hold takes the entry the cache has and grows in its body, so
`n` may read what a header may not, such as a request's hidden length.

| Argument | Type | Description |
|---|---|---|
| `CACHE` | queue | A queue that plays `l2`, declared above. |
| `POOL` | `pool` | In `l2_load`, the pool of the enclosing hold the KV arrives in; in `l2_store`, the pool it comes from, which the copy does not change. A pool of another queue is refused: only that queue's entries hold it. |
| `n` | `expr` | The tokens copied in (`l2_load`), or the tokens the entry comes to hold (`l2_store`). See below for what it may not read. |
| `per_token = r` | `expr` | The time a token takes at the cache's bandwidth; required. |

In `l2_store`, `n` and `r` may not read `cached` or `computed`: inside the
copy they are the cache's entry, not the enclosing hold's. Read them
above the call, `set c = cached;`.

The entry in the cache is the session's, as a pool's cache is. Used in
`examples/multi-turn/vllm_lmcache.sq`.

## See also

[`use`](../program.md#use), [Queues](../../language.md#queues),
[`transfer`](../serving.md#transfer-from-to),
[the P/D use case](../../use-cases/pd.md).
