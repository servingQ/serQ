# Vendor requirements for the IR

Requirements from the [latest-tag full-attention investigation](../use-cases/index.md), checked on 2026-09-30 against the current **IR v8**. This is a design proposal, not an implemented extension or new syntax. The executable examples live under `examples/vendors/`.

## What the current IR already supplies

Do not propose mechanisms that already exist. IR v8 has per-iteration resident ordering (`CServe::By`), exclusive resident prefill, admission-time bindings, recovery progress (`computed`), source leases, explicit release/load and runs that hold several stages simultaneously. [One admission](one-admission.md), [the KV transfer](pd-transfer.md) and [bandwidth sharing](bandwidth-sharing.md) record the relevant decisions. `lib/vllm.seq` uses known progress to re-prefill after local preemption without requesting all outputs again.

The remaining goal is to reproduce selected requests and full-attention KV state transitions on the same clock. Kernel numerical computation and tensor layout are outside that goal unless layout affects admission, lifetime or observable cost.

## 1. Batch selection before execution

**Evidence:** RBLN can replace selected decodes with a waiting prefill; TPU combines per-rank schedules; Ascend waiting classes age and share job history. Each use case links the tagged implementation.

**Before** (`src/ir.rs`, current definition):

```rust
pub struct CStep {
    pub budget: CExpr,
    pub cost: CExpr,
    pub chunk: CExpr,
    /// How the iteration serves its residents: an order, or the
    /// exclusive-prefill rule.
    pub serve: CServe,
    pub memory: Option<usize>,
}
```

This orders residents, then admits waiting requests. A different resident key
does not turn that into RBLN's waiting-first replacement. [PR #171](https://github.com/vrvrv/serQ/pull/171)
strengthens the existing `ExclusivePrefill` meaning to provide lone local
prefill selection and takeover without adding an IR node. Its regression
checks do not establish the native PP/remote-KV rules or a general batch
selection/shape contract.

**After — semantic sketch, not executable syntax:**

```text
snapshot → select candidates → validate resources/shape → commit batch
                                                       → execute → complete
```

Selection order, head skipping, phase mixing, per-request token caps and batch request caps belong to the program's policy. Allocation/commit must preserve resources; rejected candidates must roll back allocations or explicitly retain them. Start with closed order expressions; shared predictor state is a separate requirement.

**Checks:** selected tokens stay within budget; requests are ready; resource acquisition precedes commit; ties are deterministic; policy cannot read future actual output length.

## 2. Cache objects, lookup units and copy references

**Evidence:** native RBLN sub-block copy pins a source; full-attention prefixes can be shared by different requests; TPU allocation chooses physical intervals. The current cache associates extents with session serials. `CPool.block` supplies one rounding unit, not independent lookup and physical allocation units.

**After — data sketch, not a new IR definition:**

```text
CacheObject = (pool, object_id, content_key, logical_extent, version)
Lease       = (holder, object, access, until_event)
Lookup      = content_key → reusable extent and object references
logical progress → required/write extent → physical objects and placement
```

A pool owns physical space; requests and transfers reference its objects. Completed read-only prefixes and mutable tails need distinct access rules. Content keys must include relevant model/adapter identity. A smaller lookup unit must not silently shrink physical allocation. Placement is optional: TPU's contiguous preference still allows scattered fallback.

**Checks:** one physical owner; live references forbid reallocation; shared capacity is counted once; writes fit acquired extents; lookup granularity preserves valid reuse; dummy objects have explicit capacity.

First establish whether the current per-session cache lowers to a restricted fragment of this model. Do not force physical IDs into simple capacity models.

## 3. Extend explicit completion to connector protocols

IR v8 already expresses acquire-P → acquire-D → release-P → release-D through a source lease, destination hold and transfer. The [TPU example](../use-cases/tpu.md) runs that sequence. It is no longer a gap in lexical scope expressiveness.

The remaining evidence is Ascend offload failure/prefetch readiness, TPU backlog/cancellation and RBLN copy reference release. An explicit successful transfer does not capture every connector outcome. Automatic `spill` is also not equivalent to an explicitly completed remote load: its cache publication and transfer need their own audit.

**After — transition sketch:**

```text
begin_copy(source, destination, extent) → operation
complete → destination ready, source reference released
fail/cancel → explicit recovery and reference cleanup
```

Copying need not move ownership: both pools may retain copies. Existing leases should carry the lifetime wherever possible; price a new node only for a transition the current program cannot state.

**Checks:** no read before ready; cancellation returns every reference; in-flight objects cannot be selected as free victims; event order is deterministic.

## 4. Preserve batch shape and speculative progress

MetaX's runner regions and TPU's rank padding require more than aggregate `tokens`, `prefilled` and `decoders`. Selected per-request phase/extent must support closed reductions for buckets and rank maxima. Admission selection and runner permutation are separate observations.

Local decode recovery is already modeled with `computed` and the request library. Speculative proposal, scheduled work, verification and acceptance remain distinct states; a scalar known position does not describe rejection. Multi-stage runs express shared link resources, but do not alone describe arbitrary compute/communication dependencies.

**Checks:** committed progress is monotone; rejection discards only tentative progress; layout preserves request/token identity; cost reductions do not introduce arbitrary host callbacks that evade the IR and Lean fragment.

## Device shapes are not additional request tokens

The [runner input audit](../use-cases/input-shapes.md) adds concrete contracts:
RBLN fixed-width prefills and compiled decode buckets; Ascend SP alignment,
DP graph agreement and FIA metadata; TPU independent request/token buckets
and static head padding; MetaX full-attention block/head constraints and
uniform query padding checks.

A single-rank cost can express finite rounding with existing `def`/conditionals.
It cannot validate a per-request padded layout or combine rank maxima. Any
new shape representation must separate selected logical token intervals,
physical dimensions, validity masks and execution mode. Dummy input padding
must not advance a request or publish KV; static head padding instead changes
actual bytes per logical KV token and therefore the configured capacity.
Use the smallest closed refinement that can check those contracts, rather
than adding one opaque policy per accelerator.

## Usability and adoption

A serving engineer should write request lifetime, scheduling constraints and cache contracts. Versioned presets may provide explicit defaults, but expanding one must expose its meaning in the IR. The checker should reject unsupported policies rather than silently approximate them. Capacity constants and measured costs should be calibrated separately from structure, as in the [frontend sketch](frontend.md).

| Priority | Counterexample to reproduce | Required evidence |
|---|---|---|
| Batch selection | Waiting prefill supersedes resident decodes | Native RBLN per-step selected batch and allocation trace |
| Cache identity/units | Two requests share a prefix; sub-block copy pins its source | Cache object capacity/reference invariants and CPU cache oracle |
| Connector completion | Cancel transfer; offload fails; source release follows destination readiness | Fake connector outcomes with readiness/release traces |
| Batch shape and shared policy state | Unbalanced DP padding; MetaX regions; Ascend aging/prediction | Separate scheduler/runner traces and closed reduction/state semantics |

Any adopted semantic change follows the repository's version policy and moves the interpreter, oracle artifacts and `serving-queue-theory` generator together where required. Existing `lease`/`load` paths are outside the Lean fragment; runnable examples are not proofs. Compare each step's selection, allocation/reuse, readiness, committed progress and release, rather than throughput alone.

## Self-critique

This investigation reads public tagged source and identifies concrete refinement requirements. It does not exercise vendor SDKs/hardware or exhaustively compare inherited upstream code and every conditional patch. It cannot establish support for every vendor/model/configuration.

Batch commit and object references enlarge state and proof cost. Optional refinements need a stated observational relation to capacity-only fragments. Avoid introducing a general mutable collection for job prediction or arbitrary callbacks for runner cost before demonstrating that finite policy state and closed reductions are insufficient.
