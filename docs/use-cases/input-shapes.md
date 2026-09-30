# Input shapes and padding

A second pass over the same [latest tags](index.md) follows full-attention
requests from scheduler selection into the runner. A valid logical batch is
not necessarily a valid compiled input. Only the native RBLN path is included.

## Four distinct contracts

| Contract | Examples | Consequence for seQ |
|---|---|---|
| Logical selection | RBLN lone prefill versus decodes; PP decode cap | Determine which real requests/tokens execute |
| Input shape | RBLN compiled request buckets; Ascend SP/graph shapes; TPU request/token buckets | Derive padded dimensions separately from selected tokens |
| Metadata validity | Uniform query length; FIA query boundaries, dummy rows and block tables | Check per-request lengths and the valid/padded mask |
| Static storage/backend compatibility | TPU padded head dimension; MetaX kernel block/head support | Derive capacity or reject the selected backend configuration |

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
| [tpu-inference](tpu.md#static-inputs-and-padding), buckets `[8,16,32,48,64]` | Rank token counts 17 and 3 | Per-rank width 32, total width 64; requests are bucketed separately |
| TPU regular KV | Head dimensions 64, 80 and 129 | Dimensions 64, 128 and 256; logical sequence lengths unchanged |
| [vLLM-metax](metax.md#backend-input-constraints-and-padding), FLASH_ATTN | Head sizes 72 and 70 | 72 passes the support check; 70 does not. Block 17 fails KV shape construction |

CPU spot checks executed extracted pure helper functions from these tagged
sources: RBLN shape routes and uniform-count assertions, Ascend SP rounding,
TPU bucket/head helpers and MetaX block/head checks. Dependencies were replaced
with minimal helper inputs; no vendor SDK, runner or hardware execution was
performed. Graph replay, attention masks and per-request metadata have not
been validated end to end.

## What current seQ can express

For a **single rank**, a finite bucket map can be a source definition used
only in cost. This requires no new IR node:

```seq
// Selected configuration: three slots, decode buckets [1, 2, 4].
def decode_rows(n) = n <= 1 ? 1 : n <= 2 ? 2 : 4;
// Integer token alignment for a TP4 cost approximation.
def aligned_tokens(n) = 4 * floor((n + 3) / 4);
```

For an exclusive-prefill engine with compiled width 128, a cost can charge
`128` when `prefilled > 0` and `decode_rows(decoders)` otherwise. The engine
still executes the actual requested token count. For a mixed Ascend-like
single-rank approximation, a cost can use `aligned_tokens(tokens)`. These
are cost approximations, not graph eligibility checks or device execution.
The actual implementations of these definitions were compiled and run in a
small full-attention program; no measured latency is claimed.

Never increase `budget`, `prefill` or `decode` work just to account for dummy
tokens. That changes progress, output count and KV writes. Batch padding does
not automatically require live request KV blocks; static KV head padding is
a different capacity calculation. Dummy metadata and workspace need their
own accounting if they affect observable resource pressure.

## Remaining IR requirements

The current step cost sees aggregate logical `tokens`, `prefilled`,
`decoders` and context, not a selected per-request layout or synchronized DP
shape. A closed cost expression can approximate rounding; it cannot check
whether request query lengths are uniform, identify graph eligibility, or
synchronize several rank schedules.

A future refinement should expose:

```text
selected real requests + token intervals
  → validate phase/query/backend constraints
  → choose execution mode and padded shape
  → derive cost/workspace
  → execute only valid intervals → commit real progress
```

Keep shape selection and progress separate. Check that every real interval
fits the padded extent, dummy rows do not write request KV, and unsupported
shapes explicitly fall back or fail according to the tagged path. Graph
capture/replay and rank maxima must not become hidden vendor-name semantics.
This is a requirement sketch, not new syntax or implemented shape IR.

The [IR requirements](../design/vendor-ir.md) should be refined around these
contracts before introducing a general batch-layout node. Phase isolation is
one requirement; padding/shape validation is another.
