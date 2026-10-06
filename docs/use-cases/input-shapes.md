# Input shapes and padding

A scheduler selects real request work; a runner may pad that work into a
compiled input shape. Keep the two quantities separate when calibrating a
serQ model. This page covers the full-attention paths of the
[versions examined here](index.md#version-basis), including only native RBLN.

## Three distinct contracts

| Contract | Examples | Consequence for serQ |
|---|---|---|
| Logical selection | RBLN lone prefill versus decodes; PP decode cap | Determine which real requests/tokens execute |
| Input shape | RBLN compiled request buckets; Ascend SP/graph shapes | Derive padded dimensions separately from selected tokens |
| Metadata validity | Uniform query length; FIA query boundaries, dummy rows and block tables | Check per-request lengths and the valid/padded mask |

Padding is not uniformly caused by an NPU: the code imposes specific compiler,
graph, kernel and sharding contracts. A configuration can choose a different
backend or execution mode. Do not infer that every vendor forbids mixed
batching from the fact that it pads.

## Source-grounded examples

The numbers below are selected configurations, not vendor defaults.

| Repository and path | Logical input | Physical input or validation |
|---|---|---|
| [vllm-rbln](rbln.md#compiled-inputs-and-padding), local decode buckets `[1,2,4,8]` | Three one-token decodes | Four rows of query length one |
| Native RBLN text prefill, compiled width 128 | One 17-token chunk | One row padded to 128; still only 17 real tokens |
| Native RBLN specialized DP, a peer prefills | Local three-request decode, top bucket 8 and token target 128 | Top bucket, 128-token dimension; not the local four-row shape |
| [vllm-ascend](ascend.md#device-inputs-and-padding), SP enabled, TP4 | Five scheduled tokens | Eight tokens before graph/DP dispatch; query metadata must cover padding |

These examples describe source behavior. They are not end-to-end validation
of graph replay, attention masks or per-request metadata, and no vendor
hardware measurement is implied.

## What current serQ can express

For a **single rank**, a finite bucket map can be a source definition used
only in cost:

```serq
// Selected configuration: three slots, decode buckets [1, 2, 4].
def decode_rows(n) { n <= 1 ? 1 : n <= 2 ? 2 : 4 }
// Integer token alignment for a TP4 cost approximation.
def aligned_tokens(n) { 4 * floor((n + 3) / 4) }
```

For an exclusive-prefill engine with compiled width 128, a cost can charge
`128` when `prefilled > 0` and `decode_rows(decoders)` otherwise. The engine
still executes the actual requested token count. For a mixed Ascend-like
single-rank approximation, a cost can use `aligned_tokens(tokens)`. These
are cost approximations, not graph eligibility checks or device execution.


Never increase `budget`, `prefill` or `decode` work just to account for dummy
tokens. That changes progress, output count and KV writes. Batch padding does
not automatically require live request KV blocks. Dummy metadata and workspace need their
own accounting if they affect observable resource pressure.

## Limits

Step cost expressions see aggregate logical `tokens`, `prefilled`,
`decoders` and context. They do not see a selected per-request layout or a
synchronized data-parallel shape. Rounding can approximate cost, but cannot
validate uniform query lengths, graph eligibility, dummy-row KV writes or
coordination between rank schedules.

Use the [RBLN model](rbln.md) for logical phase isolation. Neither that model
nor these cost expressions emulate a vendor runner's padded buffers.
