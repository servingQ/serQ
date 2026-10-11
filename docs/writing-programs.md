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
`run STAGE (cost(STAGE, work));` in the model performs stage work.
Keep workload behavior separate from serving policy: put the client's
`session` inside `workload`, and request handling in `server` or a named gateway.
Omit `session` for one turn. A multi-turn session uses `turn;` and states how
the next turn follows its response. Route to a gateway in the server with
`gw.route();`.

Current syntax requires `pool`, `stage`, and other deployment declarations
inside `main`, even when their settings are fixed. Group them together before
the workload where dependencies allow: a declaration that reads a request
attribute, such as a `queue by` key, follows the workload that sets it.
Do not enforce a rule that `main` contains only `workload`
and `server`; input-dependent configuration belongs there too.
Imported libraries contain `def` and `use`, not concrete resource declarations.

## Name the policy and keep its evidence nearby

Prefer serving concepts such as `waiting_class` over names describing an
expression's mechanics. Keep scheduling choices explicit in the program.
Extract a `def` when its name explains a policy or avoids meaningful duplication;
keep a short expression inline when a separate definition obscures its use.

Use the same names for the same roles across examples and documentation:

| Role | Name |
|---|---|
| Pool limiting concurrent requests | `reqs` (capacity in request slots) |
| KV memory pool | `kv` |
| Generic service stage | `svc` |
| LLM inference stage | `llm`, or the engine it models (`vllm`, `sglang`, `tgi`); `engine` is the keyword of the engine form |
| Request response time, including queueing and service | `response` |
| Cache-hit indicator (0 or 1) | `hit` (its mean is the hit rate) |

These are naming conventions, not reserved names. Use `response` for the
observation called sojourn time in queueing theory or end-to-end latency in
serving. State where its clock starts and ends. A `hit` predicate and its
sampled population must remain explicit: full-prefix reuse and any prefix
reuse are different measurements even when both report a hit rate.
Keep different roles distinct: `mem` is generic memory, `live` limits whole
sessions, and `reqs` limits requests within them. `latency` on a link is a
language construct for a transfer's fixed delay.

State units beside settings when the name alone does not make them clear.
Comments should explain assumptions and source correspondence, not repeat syntax.

See [`examples/vendors/ascend.sq`](https://github.com/servingQ/serQ/blob/main/examples/vendors/ascend.sq):
its fixed thresholds and `waiting_class` definition precede `main`, while
the workload, resources and request handler form the model inside it. The
workload comes first because the queue key reads the attributes it sets.
Its execution conditions are supplied by the caller, for example
`serq run examples/vendors/ascend.sq --horizon 1`.
When reorganizing a program, preserve its linked IR and observed behavior.
Run `serq check` and inspect `serq draw`, then compare a repeatable run before
and after. Use the repository's `make check` gate for committed changes.
See [Working with a program](development.md) for the commands and reports.
