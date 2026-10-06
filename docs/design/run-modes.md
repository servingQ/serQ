# Prefill and decode are a run's mode

`prefill W;` and `decode W;` were serving forms: the parser found a stage
for them, the one named `prefill` or `decode`, failing that the only step
engine, and wrote `run E prefill (cost(E, W));`. The form named the request's
phase and hid the stage it ran on, so `examples/vendors/ascend.sq` read as
if `prefill` were a stage of its own. The forms are removed. A program
writes the kernel run, which names the engine and the phase:

## Before and after

Before (`examples/vendors/ascend.sq`):

```serq
hold reqs (cost(reqs, 1)), kv (cost(kv, prompt + out)) {
  observe selected = serial;
  observe admitted = now;
  prefill prompt;
  decode out;
}
```

After:

```serq
hold reqs (cost(reqs, 1)), kv (cost(kv, prompt + out)) {
  observe selected = serial;
  observe admitted = now;
  run engine prefill (cost(engine, prompt));
  run engine decode (cost(engine, out));
}
```

The rewrite is the one the parser made, so every program's IR is unchanged
(`serq ir` before and after, byte for byte, for every tracked `.sq`). The
removed forms are refused with the kernel spelling as help. `transfer` and
`tool` remain: a transfer is three statements, and a tool call is not the
engine's.

The words stay keywords. On a step engine they are the mode the iteration
needs: prompt tokens share the budget in chunks, decode tokens come one per
iteration. That is a mechanism the engine supplies and the program selects
per run (criterion 2), and the linker requires the mode on a step stage and
refuses it elsewhere.

## Self-critique

Two other spellings were considered and rejected.

`run E phase.PREFILL (…)`, an enumerated value. As syntax, it is a longer
spelling of the same word. As a value, a program could compute the mode
(`x ? phase.PREFILL : phase.DECODE`), which moves the linker's check to run
time and changes `RunMode` in the IR. The language has no enumerations, and
its other fixed choices are bare words (`evict lru`, `preempt lifo`,
`serve decode first`).

`run E (p, d)`, one statement for a request's whole use of the engine. It
reads well where nothing happens between the phases, but `lib/vllm.sq` and
the P/D examples mark the first token between them (`observe ttft`,
`mark first_token`), and a prefill-only or decode-only instance would write
a zero. It could be added later as sugar for the two runs.

The kernel spelling is longer: `cost(engine, …)` repeats the stage. That is
`size-and-cost.md`'s choice for every `run`, and removing the repetition
belongs there, not to a form that hides the stage.
