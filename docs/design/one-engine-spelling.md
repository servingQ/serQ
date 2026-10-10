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
iterations mean something only of it. The docs said "step stage" a hundred
and nineteen times to tell the two apart, and #419 (`admit via` a stage
that runs no iterations) linked because the kinds shared a name.

The IR did not move: an engine lowers at parse time to the step stage the
kernel spelling wrote, so every program's IR is the one it had, and the
interpreter, the oracles and the Lean model read it as before. Who admits
a pool on the engine's device is said where the pool is, `on gpu` or `on
vllm.gpu` (#424), so the settle-time admission the kernel spelling could
write by leaving out `admit via` is written too, and nothing the kernel
said is lost.

## Self-critique

**An IR of its own for the engine.** `CStageKind::Step` could leave
`Program::stages` for a `Program::engines`, so that `admit via`, `memory`,
a run's mode and an iteration claim name an engine by type, and #419's
kind of bug cannot be written in the IR either. It is the stronger check
(criterion 3), and it is an `IR_VERSION` bump with the Lean generator and
every index that names a stage (`admit via`, claims, registers, `run`)
moving with it. Kept apart: the frontend already refuses the program, and
the price is a handshake, not a line.

**The kernel spelling kept for tests.** About a hundred and ten test
programs were written as `stage E : step`. Keeping the spelling for them
would have kept two spellings of one meaning in the language the tests
check; they were written as engines with the IR they had (#423, #438), and
the tests of the kernel spelling's own messages went with it.

**`admit via` on a plain pool.** A pool declared with its own `cap` and
`admit via E` is still admitted by engine `E`, which `pool X on E` also
says: a second spelling of who admits. No example writes it any more;
narrowing it is its own change (#439).
