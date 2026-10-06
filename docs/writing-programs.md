# Writing readable programs

Separate reusable definitions, model configuration and execution conditions.
A reader should find the serving policy, resources and request flow without
having to trace unrelated setup. These are writing conventions; the
[language specification](language.md#entry-point-and-external-inputs) defines valid syntax.

## Place definitions by their dependencies

Put imports first, then fixed constants and reusable `def` declarations,
then `fn main()`. Keep related constants beside the definitions using them.
Prefer definitions outside `main` when they do not depend on its local names.
Keep a definition inside when it depends on that experiment's inputs.
Do not move it out by introducing an implicit dependency on a local name.

Read external inputs explicitly with `args.number` inside `main`.
Use plain `let` for fixed values; expose only intended experiment parameters.
Pass dependencies as `def` parameters when that makes reuse clearer.
Avoid extra parameters or wrappers whose only purpose is moving text outside.

## Make the model easy to scan

Inside `main`, group inputs and derived settings, resource declarations,
and request behavior in that order where dependencies allow. Supply execution
conditions with CLI flags, Python arguments or an explicit instance file;
`run STAGE (work);` in the model performs stage work.
Keep workload behavior separate from serving policy: put the client's
`session` inside `workload`, and request handling in `server` or a named gateway.
Omit `session` for one turn. A multi-turn session uses `turn;` and states how
the next turn follows its response. Route to a gateway in the server with
`gw.route();`.

Current syntax requires `pool`, `stage`, and other deployment declarations
inside `main`, even when their settings are fixed. Group them together before
the workload. Do not enforce a rule that `main` contains only `workload`,
and `server`; input-dependent configuration belongs there too.
Imported libraries contain `def` and `use`, not concrete resource declarations.

## Name the policy and keep its evidence nearby

Prefer serving concepts such as `waiting_class` over names describing an
expression's mechanics. Keep scheduling choices explicit in the program.
Extract a `def` when its name explains a policy or avoids meaningful duplication;
keep a short expression inline when a separate definition obscures its use.
State units beside settings when the name alone does not make them clear.
Comments should explain assumptions and source correspondence, not repeat syntax.

See [`examples/vendors/ascend.sq`](https://github.com/servingQ/serQ/blob/main/examples/vendors/ascend.sq):
its fixed thresholds and `waiting_class` definition precede `main`, while
resources, workload and request handler form the model inside it. Its execution
conditions are supplied by the caller, for example
`serq run examples/vendors/ascend.sq --horizon 1`.
When reorganizing a program, preserve its linked IR and observed behavior.
Run `serq check` and inspect `serq draw`, then compare a repeatable run before
and after. Use the repository's `make check` gate for committed changes.
See [Working with a program](development.md) for the commands and reports.
