# Preempted in recompute

The report's per-pool count `stuck` is `preempted_in_recompute` since
report version 2: the JSON key `pools[].preempted_in_recompute`, the
pyserq attribute `Pool.preempted_in_recompute`, and in the text report the
column and the note `preempted in recompute`. What is counted does not
change. The name and the documentation that read the count as a livelock
do (#480).

## What is counted

`Interp::preempt` (`src/engine/interp.rs`) records, per pool, the position
a session's hold had reached there when it was preempted (`preempt_pos`,
cleared when the hold completes). A second preemption at the same pool at
the same or a lower position sets the session's flag and counts it in that
pool: once per session in the run, at the first pool where it happens. The
hold was preempted again before its recompute passed the point the first
preemption dropped.

## Why `stuck` was the wrong name

The count is the cheap half of [IR v4](ir-v4.md) §7: a session preempted
twice without progress, meant to make the self-preemption livelock
visible. The wrong assumption was that a position at or below the last
preemption's is no progress. In vLLM a preemption keeps a request's tokens
and drops its KV: `_preempt_request` sets `num_computed_tokens` to 0
(`scheduler.py:1561`), a re-admitted request is appended to `running`
(`scheduler.py:1296`), and the victim is `running.pop()`
(`scheduler.py:799`). The request just re-admitted to recompute is the
next victim, and until its recompute passes its last position a
preemption finds it below that position. It has lost KV, not output.

The issue's run counts 31:

```
$ serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --Lambda 0.5 --blocks 1000 --max_seqs 64 --chunk_cap 128 --horizon 20000 --warmup 2000 --seed 1
```

With `--horizon 4000 --warmup 400 --log`, 13 turns, one per session, are
preempted twice or more at `kv`; the count is 4, and all 13 turns finish.
`tests/vllm_semantics.rs` holds both cases on the shipped
`examples/multi-turn/vllm.sq`: a request preempted again during its
chunked re-prefill (counted, and it finishes), and one preempted again
after its recompute passed the first position (not counted).

The documentation said the count was a livelock, or sessions that
contribute no completed samples (`docs/language.md`, `docs/development.md`,
`docs/getting-started.md`, the CLI reference, pyserq's `Pool`, the
report's note). A reader of a nonzero count would discard a run of
ordinary overload.

## Decision

- The name says the condition: preempted while recomputing what a
  preemption dropped. "Preemption and recompute" is how the API index
  names vLLM's preemption already (`docs/api/index.md`), so a serving
  engineer reads it as vLLM (criterion 1).
- `REPORT_VERSION` 2. A renamed field bumps it (`docs/ir.md`, Stability),
  and version 1 is in a tagged release (v0.1.7-alpha.2).
  `tests/report.rs` asserts the version beside the shape, so a diff that
  moves one moves the other.
- The count is unchanged: same condition, same once-per-session flag.
  Whether a run that never finishes a hold should fail is #64's question.
- The note stays, reworded: until #64, it is the report's only line for a
  hold that can never fit (`tests/pool_semantics.rs`, `ended` 0).
- In the interpreter, "stuck" now names only the hold-and-wait cycle
  (`waiting_for_each_other`), sessions that cannot move: one word, one
  meaning.

## Should the language have refused the program?

No. `vllm.sq` under `chat.sq` is a correct program and the interpreter
counted what it meant to; the name and the documentation were wrong. A
preemption during a recompute is ordinary under overload, and no static
check can tell an overload from a livelock. A hold that can never fit,
the real livelock, vLLM refuses at start-up; the run-time judgement of it
is #64.

## Self-critique

**`repreempted`, the issue's proposal.** It reads as every second
preemption, and the count is not that: 13 sessions preempted again at
horizon 4000, 4 counted. `a_request_preempted_again_past_its_recompute_is_not_counted`
has a session preempted twice and a count of 0. Rejected.

**`thrashed`.** The documentation already says the caches thrash for
prefix-cache churn on the same `kv` pool (`docs/use-cases/thunderagent.md`).
Rejected: one word, two meanings.

**Keep `stuck` and fix the documentation.** The name is the misreading:
every document that got it wrong took it from the name. And `stuck`
already names the hold-and-wait cycle in the interpreter. Rejected.

**Drop the column or the note.** That would leave a hold that can never
fit with no line in the report until #64. Rejected.

**Count only livelocks.** A run cannot tell, at the second preemption,
whether the hold will ever fit; that criterion is #64's, with the exit
code. Deferred, not rejected.

**A short header** (`re-preempt`, `in recomp`). Ambiguous or cryptic.
The price of the full phrase is width: the issue's pool table grows from
102 to 119 characters, against 117 for the same report's `run:` line.
Headers with spaces exist already (`prefill only`, `decode batch`).

**Once per session.** A multi-turn session is counted at its first such
preemption and never again, so the count is a lower bound on the turns it
describes. Kept, because this change does not move the count; a per-turn
count is a new field, for #64 to decide with the criterion.
