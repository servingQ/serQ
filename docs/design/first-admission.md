# An engine request's first admission

An engine's request records when it was admitted, `E.admitted`, which its
caller reads after the call. It was the latest admission; it is now the
first, which a readmission after a preemption does not move. IR 16 (#727).

## In vLLM

vLLM's request metrics keep the time a request was first scheduled and
ignore its preemptions: `scheduled_ts` is set at a `SCHEDULED` event only
while it is 0 (`metrics/stats.py:524`), and the queued time, the prefill
time and the inference time are measured from it. A decoder preempted after
its read is rescheduled without a second transfer
(`nixl/pull_scheduler.py:187-189`), its computed tokens reset
(`scheduler.py:1560-1561`); its first scheduling is still the one its read
followed.

## In serQ

`examples/pd-disaggregation/pd_batching.sq`'s gateway reads the decoder's
records after the call:

```serq
observe admit_wait = D.admitted - t_prefill;  // the decoder's admission: the prompt's blocks
observe transfer = D.load_done - D.admitted;  // the read: the wait before it and both NICs
observe send_held = D.load_done - t_prefill;  // how long the prefiller kept its blocks for the read
```

With decoders short of memory (`--blocksD 300 --Bw 2e4 --x0 0.01`, seed 1,
`--horizon 100`), 89 of 811 decoders are preempted after their read and
admitted again, reading nothing (`request.preempted ? 0 : …`).

Before (v0.1.7-alpha.2): `E.admitted` was the readmission's. With the
readmission also ending the load (the bug #727 fixes first), `transfer`
was 0 for the 89 and `admit_wait` and `send_held` ran to the readmission;
with that fixed alone, `transfer` was negative for them (the read's end
less the readmission). The program could not say the read's admission.

After: `E.admitted` is the first admission. For every request, `admit_wait`
is the wait for the decoder's blocks, `transfer` the read, and the two add
up to `send_held`, which the run log's send (registered to done) equals
session by session; `transfer`'s mean, 0.16940 s over all 811, is what the
log gives for the reads.

- `RequestRecords::admitted` is set at a request's first admission only
  (`Interp::request_admitted`, `!preempted`).
- The Lean generator's kernel hold runs its body again after a
  preemption: from IR 16 it sets the record while `again` is still 0, its
  first pass. `tests/lean-regress/first_admission.sq` is a request
  preempted and admitted again that reads it; its theorem checks the Lean
  execution against the interpreter's.
- The examples read it unchanged. At their defaults no request is
  preempted after its admission where a program reads the record
  (`pd_batching.sq`, `vllm_nixl_push.sq`, `ascend.sq`): every example's
  report is the same.

## IR

IR 15 is tagged (v0.1.7-alpha.2), and the field keeps its shape while its
meaning changes: an IR 15 program run under the new meaning gives another
answer wherever a request is preempted, so the version moves to 16
(`docs/ir.md`, Stability). The Lean generators read 16 (and 15, with the
latest admission).

## Self-critique

**Observe the read only where the latest admission read** (`if (D.loaded >
0) { … }`, #727's first version). Each sample it keeps is right, but it
drops the decoders preempted after their read, the requests memory pressed
hardest: in the committed sweep's `kv_d_8k` rows `admit_wait`'s mean fell 6
to 10 % below the read's (λ 40: 23.84 ms against 25.92), further off than
the latest admission's +3 %. A rule a program must remember, whose
omission is silent, about a value the language could keep.

**A second record, the read's admission, beside the latest.** Two
admission times, one of which a program must pick, and nothing in the
examples or in vLLM's metrics reads the latest: a readmission after a
preemption is the scheduler's business, not the caller's. One record, the
first, as vLLM keeps.

**The latest admission, as it was.** It is when the request last entered
the batch, but the caller reads it after the call, against times of the
call (the prefill's end, the load's end), and the call began at its first
admission. Read against those, the latest is wrong for every preempted
request.
