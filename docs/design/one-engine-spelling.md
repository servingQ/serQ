# One engine, one spelling

A stage that runs iterations is written as an engine on a device, and only
so (#421):

```serq
device gpu { kv cap blocks * bs; }
engine vllm on gpu {
  reqs cap max_seqs;
  tokens cap B;
  schedule { advance running; admit waiting while (running.preempted == 0); }
  execute (c0 + a * batch.tokens);
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on vllm { queue fifo; }
```

A `stage`, a queue's `serve` and its `nic` take `fifo`, `ps (φ)` or
`delay`, the disciplines in which time passes at a rate; `step` is refused
with the engine it is written as. There were two spellings of one IR step
stage, `stage E : step { budget …; cost …; memory …; serve …; iteration {
… } }` and the engine form ([Engines on devices](engine-device.md)), and
the two put different things under one word: `fifo`, `ps` and `delay` take
work in time and no mode, an engine takes tokens, a `prefill` or `decode`
mode, a budget, a memory and a schedule, and `admit via` and a claim over
iterations mean something only of it. The docs said "step stage" forty-eight
times to tell the two apart, and #418 (`admit via` a stage that runs no
iterations) linked because the kinds shared a name.

The IR did not move: an engine lowers at parse time to the step stage the
kernel spelling wrote, so every program's IR is the one it had, and the
interpreter, the oracles and the Lean model read it as before. Who admits
a pool on the engine's device is said where the pool is, `on gpu` or `on
vllm.gpu` (#424), so the settle-time admission the kernel spelling could
write by leaving out `admit via` is written too.

## Self-critique

**An IR of its own for the engine.** `CStageKind::Step` could leave
`Program::stages` for a `Program::engines`, so that `admit via`, `memory`,
a run's mode and an iteration claim name an engine by type, and #418's
kind of bug is a type error rather than a check. It is the stronger check
(criterion 3), and it is an `IR_VERSION` bump with the Lean generator and
every index that names a stage (`admit via`, claims, registers, `run`)
moving with it. Kept apart: `Program::validate` already refuses an
`admit via` of a stage that runs no iterations (#419), so the IR has the
check if not the type, and the type's price is a handshake, not a line.

**The kernel spelling kept for tests.** About a hundred and ten test
programs were written as `stage E : step`. Keeping the spelling for them
would have kept two spellings of one meaning in the language the tests
check; they were written as engines with the IR they had (#423, #438), and
the tests of the kernel spelling's own messages went with it.

**What the kernel spelling said that an engine cannot.** A `chunk` was
any expression; an engine's `each at most` chooses among constants or
`max(k, e)` above a positive constant `k` (#442), which spells a cap that
follows the number of requests, vLLM's adaptive threshold
`max(long_prefill_token_threshold, input_budget // num_eligible_reqs)`
(`scheduler.py:617-622`, off by default); any other computed cap is
refused, so no run is given 0 or below. And `stage E[2] : step {
memory kv; }` beside one `pool kv` let two engines share one memory, which
an engine, one per device, cannot; no program here wrote it.

**`admit via` on a plain pool.** A pool declared with its own `cap` and
`admit via E` is still admitted by engine `E`, which `pool X on E` also
says: a second spelling of who admits. No example writes it any more;
narrowing it is its own change (#439).
