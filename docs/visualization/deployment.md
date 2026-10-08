# The deployment view

The program as a queueing network.

![llm-d prefill/decode over NIXL](../assets/llmd_nixl_pull.deployment.svg)

That is `examples/pd-disaggregation/llmd_nixl_pull.sq`: two prefill and two decode
instances, the KV read over NIXL. Each filled panel is an instance: what the
router's `choose` picks, a pod with its engine, its NIC and its pools; the
engine's slots and KV blocks are drawn in the engine's frame. The
read crosses from the prefiller's box to the decoder's: it holds the
prefiller's NIC and the decoder's at once, so its two stations are bracketed
as one job, and what it moves (`P.kv[i] → D.kv[j]`: the prefiller's blocks
stay leased until it ends, the decoder's are allocated before it starts) and
the link's latency are written on it.

## Request flow

For a program with a `server` or gateway, the figure shows one request's
path. Workload tool calls, subsequent turns and session exits are omitted,
but holds surrounding the workload's `request` remain visible. When there
is no unique request body, or when drawing IR directly, the figure shows
the full session.

| Element | Meaning |
|---|---|
| Station | A stage reached by a `run`; an array is labelled with its size. Consecutive prefill and decode runs at one engine share a station. |
| Arrow | Request flow between stages, including branches and loop returns. Guards label alternatives; `out` marks an exit. |
| Decision diamond | A branch or routing decision before the first station. |
| Filled panel | An instance selected by `choose`, grouping its indexed engine, NIC and pools. |
| Dashed enclosure | A pool held across several stations. |
| Unfilled frame | Pools held at one station, listed below its glyph. |
| Transfer bracket | A job using both endpoints' NICs; its label names the source and destination KV. |
| Latency label | A delay before a single transfer. A delay leading to several transfers remains a station. |

The diagram simplifies control flow. Consecutive visits to one station do
not add an arrow, and guard chains with no intervening station collapse.
A guard fixed by earlier constant assignments can omit an arm with no
station; parameter-controlled alternatives remain visible. Inspect the
program when you need the exact execution path.

![vLLM v1: one engine](../assets/vllm.deployment.svg)

That is `examples/multi-turn/vllm.sq`. Its request slot (`reqs`) and its
KV blocks (`kv`) are held only at `vllm`, so they are drawn in its frame.
The tool call and the next turn are the workload's, so the figure has
neither: a request arrives at `vllm` and goes out.

## Glyphs

| IR | Glyph |
|---|---|
| `Fifo(c)` | circle, `FIFO`, the server count when `c ≠ 1` |
| `Ps(φ)` | circle, `PS`, with `φ` beneath |
| `Delay` | rounded box with a duration-density glyph: decaying for `~exp` and `~h2`, a hump for `~erlang`, flat for `~uniform`, a spike for a deterministic duration. Other or long expressions use a row of small circles for infinite servers |
| `Step { … }` | rounded box with a token-budget bar: an iterating engine, not a queueing station. It is labelled `engine` and `tokens cap B` in the engine form's words, the residents' totals as `running.count` and `running.decoding`; the IR keeps no form, so a `stage : step` is drawn the same |
| a pool | a drum, its capacity and options written beside it (`cap 8192 · block 16`) |
| a pool held at one station alone | a row in the station's unfilled frame, under its glyph: drum, name, options |
| a pool held across several stations | dashed rounded box, the drum and options in the column at its left |
| a pool a hold caches in | a grey strip under its row, or along the bottom of its box |
| the queue a hold waits in | ahead of a dashed box, one per hold rather than per pool: a hold of several pools joins the queue of its first. A frame draws none: its pools are taken at its entrance |
| a step stage's `memory` | `pool X on S's device` as the pool's title: the engine form's pool on the device, which the IR does not name; `admitted by S` among its options when `S` also admits it |
| `admit via S` | `pool X on S` as the pool's title, unless it is `S`'s memory; from a dashed box, also a dashed edge from the queue to `S` |

A pool's eviction order is drawn only where something is cached in it: an order
over an empty cache says nothing.

### Which pool holds the cache

A hold with a `growing` run caches in that pool alone. A hold without one
caches in all its pools. The cache strip follows this behavior, rather
than assuming every cached pool represents KV memory.

## Nesting

![The paper's two-resource replica](../assets/replica.deployment.svg)

`replica.sq`'s workload holds `live` for a session's whole conversation,
and its server holds `reqs` and `kv` only around the engine. A request is
served inside the workload's hold, so `live` is drawn too. All three are
held at `llm` alone, so all three are rows in its frame.

## A program that holds nothing

![Four replicas and a migration link](../assets/routing.deployment.svg)

`routing.sq` has no pools at all. The figure therefore has no pool enclosures. The `choose j of 4` under `rep[j]` is the routing policy.
