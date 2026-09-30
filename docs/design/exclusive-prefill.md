# Separate prefill/decode batches

`serve exclusive prefill` must constrain the selected batch, including
waiting admission. A priority rule for prefills already resident is not
sufficient. This change strengthens the existing IR mechanism without adding
a node or a second spelling. It is part of the still-untagged IR v8.

## Evidence and scope

Native [vllm-rbln v0.11.3a21](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/rbln_scheduler.py#L196)
keeps a step to a lone prefill or decodes. A resident prefill skips waiting
admission; a waiting prefill can displace selected resident decodes. Its
first chunk uses the full prefill budget rather than the decode remainder.
The displaced decodes retain pending allocation deltas.
[Waiting selection](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/rbln_scheduler.py#L638),
[batch replacement](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/rbln_scheduler.py#L863)
are the source basis. Only full attention is considered.

This mechanism covers phase isolation and takeover for fresh local prefills.
It is not the entire native scheduler: PP hard/soft caps, remote-KV promotion,
the guard after admitting decode-ready waiting requests, asynchronous output
and sub-block copies are not modeled here. In particular, ordinary seQ holds
can execute arbitrary commands before joining a stage: admitting a hold and
selecting its engine work are distinct observations.

## Before

The interpreter captured whether residents contained a prefill once, then
continued to admit from waiting queues. The decisive line was:

```rust
let blocked = exclusive && any_prefill && (mode == RunMode::Decode || prefill_taken);
```

A's two-token prefill finishes at t=1; B arrives then with four tokens. The
resident snapshot contains only A's decode. A gets one token and B is admitted
with the remaining three; both appear in the same iteration. The next step
finishes B alone. The trace before this change was:

```text
ITER 0.0000 0:0:p2
ITER 1.0000 0:0:d1 1:0:p3
ITER 2.0000 1:0:p1
ITER 3.0000 0:0:d1
```

This also advanced A's computed position during selection, before it was
known whether the work would survive a waiting-prefill takeover.

## After: an executable counterexample

```seq title="examples/single-turn/separate_phases.seq"
--8<-- "examples/single-turn/separate_phases.seq"
```

The same IR selection is now interpreted as a whole-batch policy. A resident
prefill has priority. Otherwise decodes remain tentative while waiting
admission is possible; a selected prefill clears them and receives the full
budget. Selecting that prefill stops further admission. Only the final
selection advances computed positions; displaced decode allocations remain
held. The resulting trace is:

```text
ITER 0.0000 0:0:p2
ITER 1.0000 1:0:p4
ITER 2.0000 0:0:d1
ITER 3.0000 0:0:d1
```

B finishes its prefill at 2 and A finishes at 4. With ordinary serving order,
the same program still selects mixed batches; phase isolation is a policy
the program chooses.

## Verification and remaining cost

`tests/exclusive_prefill.rs` derives expected schedules by hand and inspects
actual iteration assignments. It checks takeover/full budget, discarded
work's cache extent, ordinary mixed batching, insufficient slot/KV capacity,
and a two-chunk resident prefill that must not admit another waiting request.
The takeover scenario also runs from serialized IR. Existing vLLM scenarios
and artifacts remain unchanged.

No schema or frontend construct was added; syntax highlighting needs no new
keyword. The semantic change is restricted to `CServe::ExclusivePrefill`,
which is outside the Lean executable fragment. There is no new IR number or
generator pin before v8 is tagged. This is not a vendor-runtime differential
test or a proof of the exclusive policy in Lean.

Device padding is separate: the selected logical tokens advance requests,
while physical input padding affects shape, buffers and cost. RBLN's input
stager pads a lone prefill to its compiled token dimension and decode rows to
a configured bucket. That must not be modeled by increasing prefill/decode
work or publishing KV for dummy tokens.
