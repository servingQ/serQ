# Separate the model from execution settings

A model defines the deployment, workload, session behavior, measurements and
claims. An invocation supplies the horizon, warm-up, seed and arrival limit.
Previously `run STAGE (work);` performed stage work while `run { horizon T; }`
inside `fn main()` declared an experiment. Removing the latter gives `run`
one role in a model and lets the same model serve multiple experiments.

Before (the Python latency example):

```serq
  gauge occupied = used(slots);
  run { horizon 10; }
```

After, the model ends with the gauge and the caller supplies the duration:

```python
program = pyserq.compile(source=source, horizon=10)
report = pyserq.run(program)
```

```sh
serq run model.sq --horizon 10
```

Source compilation and IR export require an explicit horizon. Warm-up and
seed retain their invocation defaults, 0 and 1. Existing example conditions
move verbatim to explicitly selected `instances/<model>/default.sq` files.
Named instances include their experiment's conditions as well; no sidecar is
automatically read. Instance `run { ... }` remains configuration syntax in
the external control plane, not a model declaration.

The fully resolved IR still includes all four execution fields. Its shape
and interpretation do not change, nor does IR_VERSION. Oracle, claim and
Lean regression IR should remain identical. A model with the removed block
is rejected even when external options would otherwise override it. The
error explains where to put those settings.

`check`, `draw` and `target` need to validate a model without choosing an
experiment. Internally they use a finite maximum horizon when none is
supplied; this inspection IR is not executed or exported by those commands.
`fmt` only parses. Execution and IR export never supply a placeholder.

## Self-critique

- Renaming the model block to `simulation` would clarify the noun, but keep
  experiment conditions in the model. Rejected because the requested
  boundary is between the model and its invocation.
- Removing execution fields from IR would lose a complete record of the
  experiment and require an IR consumer handshake. Not necessary here.
- Moving Python settings from `compile` to `run` requires a distinct
  unconfigured model object, separate from the executable `Program` IR.
  Deferred: this change keeps the existing API and serialization contract.
- Removing instance configuration syntax as well would require a new
  external file format. Deferred; the role removed here is the model item.
- Silently stripping the old block or automatically loading sidecars would
  hide which experiment is run. Both are rejected; migration is explicit.

## Review

1. Purpose kept: models retain their resources, workload, policy and claims.
   Oracle, claim and regression IR and deployment figures remain identical.
2. Simpler possible: narrowing the model grammar removes a role without a
   new IR node, Python model type or settings format.
3. Complexity: the model loses one item; complete executable IR is unchanged.
   Instance syntax and API argument precedence retain their meanings.
4. Intention: `run` inside a model performs stage work. The removed block's
   diagnostic names the CLI flags and external instance destination.
5. Evidence: migrated invocations preserve the original example conditions;
   tests exercise missing conditions, rejection of the old block, inspection
   without execution conditions and complete IR round-trips. Python reference
   examples are executed, not merely rendered by MkDocs.
6. One change: grammar, examples, callers, documentation and syntax highlighting
   move together to separate model declarations from invocation conditions.

Executing the Python landing-page example also found a stale top-level
`session` left after the workload/server migration on main. The assumption
that a rendered documentation example was runnable was false; MkDocs does
not compile source inside code fences. The example now uses the existing
workload session and server syntax. The language already refuses the stale
form, so no additional static check is needed.

The PR review found two experiment runners and the Rust `no_run` example
still relying on settings embedded in their source models. The migration
assumed example and test callers covered every executable entry point;
`tools/pd_batching/` was missed. Both runners now explicitly select the
original model's instance before applying the requested seed, including
when editing a temporary copy. The gate runs small cases through both
runners, and the Rust example is now an executed doctest. The linker already
rejects missing horizons; no new language check is needed.
