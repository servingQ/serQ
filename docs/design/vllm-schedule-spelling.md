# vLLM's schedule, as written

vLLM's `schedule()` serves the running requests, then admits the waiting
unless the step preempted one (`scheduler.py:869`, `if not
preempted_reqs`). In serQ it is the schedule

```serq
scheduler { advance running; admit waiting while (running.preempted == 0); }
```

which the frontend lowers to the step with no iteration body: the
engine's built-in procedure, the form the Lean fragment and the oracle
read, and the one `serq target` takes. Since #721 the frontend recognises
it by that spelling alone. Before, it took three spellings of the gate
and refused the fourth one would write, so the same program had two IRs
and two verdicts.

## The finding

`not_preempted` in `src/frontend/parser/device.rs` (from #403) recognised
the gate by its syntax: `!running.preempted`, `running.preempted == 0`
or `0 == running.preempted`. Any other gate made the schedule an
iteration body. `examples/multi-turn/vllm.sq`, its gate respelt,
`serq ir --workload examples/workloads/chat.sq --horizon 100` and `serq
target`, on d50ac83:

| gate | the step's IR | `serq target` |
|---|---|---|
| `running.preempted == 0` | `serve: By([])`, no `iteration` | taken |
| `!running.preempted` | the same | taken |
| `0 == running.preempted` | the same | taken |
| `running.preempted < 1` | `iteration: [Serve, Admit {gate: Lt(Preempted, 1)}]` | refused: `` `vllm` has its own schedule `` |

`serq run --horizon 2000 --json` gives the same report for all four. The
gates mean the same, and the target's verdict depended on how one was
written.

The run of an example does not exercise the gate: nothing in it is
preempted. `tests/vllm_semantics.rs`,
`the_gates_spellings_run_alike_where_the_gate_matters`, runs two
requests of 40 tokens and 60 out on 8 blocks of 16, where they cannot
both finish resident. With the gate the run preempts 9 times, without it
17; each of the four spellings gives the same report as the others,
before this change and after it.

## The change

The procedure has one spelling, `running.preempted == 0`, as written.
`vllm_gate` (the old `not_preempted`) accepts only that: the variable on
the left, the number 0 on the right. Every other gate, whatever it
computes, makes the schedule a body of the program's own. On the
branch:

| gate | the step's IR | `serq target` |
|---|---|---|
| `running.preempted == 0` | `serve: By([])`, no `iteration` | taken |
| `!running.preempted` | `iteration: [Serve, Admit {gate: Not(Preempted)}]` | refused |
| `0 == running.preempted` | `iteration: [Serve, Admit {gate: Eq(0, Preempted)}]` | refused |
| `running.preempted < 1` | `iteration: [Serve, Admit {gate: Lt(Preempted, 1)}]` | refused |

The refusal names the spelling: `` `vllm` has its own schedule; vLLM's
is `advance running; admit waiting while (running.preempted == 0);` ``.
The reports are unchanged.

Why `== 0`: every example, tutorial, document and companion program
already writes it, and so do the target's refusal and the `exclusive
prefill` refusal. `running.preempted` is a count
([context](../api/context.md)), and `== 0` says what is asked of it.

### The target's written-out body

`src/target.rs` kept the same rule a second time (#350):
`is_vllm_iteration` took the body `[Serve, Admit while !preempted]`, no
`only` and no order, as vLLM's schedule written out. After the frontend
change that body is exactly what `advance running; admit waiting while
(!running.preempted);` lowers to, so the target would have taken the
spelling the frontend dropped and refused `< 1`: the issue's verdict by
notation, moved into the target. The function goes. The target takes
vLLM's schedule only from a step with no iteration body and refuses
every body. `tests/target.rs`, `vllms_schedule_is_spelt_one_way`,
replaces #350's `vllms_schedule_written_out_as_a_body_is_vllm`: the
body that test built is now refused.

### The price

No IR field, variant or meaning changes; `IR_VERSION` does not move. The
frontend writes a different IR only for the spellings it no longer
recognises, and no example, tutorial, oracle or claims program uses one,
so `tools/oracle/*.ir.json`, the figures, `tools/metrics.json` and
`tools/unused.json` are unchanged, and every pair of
`examples/pairs.json` gives the same `serq ir` and `serq run` as before.
The Lean generator reads only a step with no body as the plain
procedure already, and its `only (p)` body is the linker's write-out,
which does not change.

What a program sees: a schedule spelt `!running.preempted` or `0 ==
running.preempted` runs as it did, as a body, and `serq target` refuses
it. `exclusive prefill; admit waiting while (!running.preempted);` no
longer links, with the refusal that names the rule's one schedule.

## The two questions

**Why it happened.** The recogniser stood for a meaning ("the step has
not preempted") with a list of the spellings in use when it was written:
the kernel's `!preempted`, the engine's `== 0`, and the swap. A list of
spellings is never the meaning, so `< 1` fell outside it. The same rule
was kept in `src/target.rs` (`is_vllm_iteration`), fixed here. One more
rule lives on the procedure's path alone: the linker refuses a register
that nothing sets only for the procedure (`src/frontend/link.rs`, "is
set by nothing"), so a body with `state k = 0;` and no `set k` links.
That gap was there for `< 1` and every other body before this change;
it is a separate rule, left to its own issue.

**Should the language have refused the program?** No. `!`, `0 ==` and
`< 1` are correct programs that run as the procedure does. Only the
target, whose job is to say whether vLLM runs a program unchanged,
refuses them, and it names the spelling it takes.

## Self-critique

**Recognising the procedure by meaning.** The frontend could normalise
every gate that is 1 exactly when `running.preempted` is 0 (`< 1`, `<=
0`, `!`, `0 ==`) to one gate. Rejected: it adds rules to the frontend,
which nothing verifies, and it never ends (`running.preempted * 2 < 1`),
so the language would still have no one spelling.

**`!running.preempted` as the one spelling.** It is closer to vLLM's `if
not preempted_reqs`. Rejected: it reads a count as a truth value, and
every program, document and refusal writes `== 0`; choosing `!` would
rewrite about a hundred sites for no change in meaning.

**Keeping the target's written-out body.** A hand-written IR could
carry vLLM's schedule as a body, and the target took it. Rejected: it is
the same recognition by notation, in a second place, and after this
change it would take exactly the spelling the frontend no longer
recognises. A hand-written IR that means vLLM's schedule writes no body.

**Writing the `only (p)` procedure's gate as `== 0`.** The linker writes
`only (p)` on both statements as the body `[Serve {only: p}, Admit {only:
p, gate: !preempted}]` (#355). Rewriting it as `Eq(Preempted, 0)` would
make the IR agree with the source spelling. Rejected: it moves the IR of
`fastertransformer.sq`, `bari_rad.sq` and `dai_fastertransformer.sq` and
the Lean generator's `NOT_PREEMPTED` for no change of meaning.

**The `only (p)` body with `!` is the procedure's IR.** `advance running
only (p); admit waiting only (p) while (!running.preempted);` lowers, as
a body, to the same IR as the procedure spelt `== 0` with `only (p)`,
since that body is the linker's write-out. Accepted: the frontend does
not recognise it as the procedure; the generic lowering happens to
produce the kernel's form, and the target refuses both for the `only`.
Refusing the `!` spelling there would bring back recognition by
notation.

**Renaming `running.preempted`.** `RequestStatus.PREEMPTED`
(`scheduler.py:1560`) is a request's state until it is scheduled again,
which serQ has as `request.preempted`; `preempted_reqs` is a list local
to one `schedule()` (`scheduler.py:573`). `batch.preempted` would break
the pair `running.preempted` and `waiting.admitted`, what happened this
iteration to each list. Both rejected in #721.
