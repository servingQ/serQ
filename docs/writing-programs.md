# Writing readable programs

Separate reusable definitions, model configuration and execution conditions.
A reader should find the serving policy, resources and request flow without
having to trace unrelated setup. These are writing conventions; the
[language specification](language.md#entry-point-and-external-inputs) defines valid syntax.

## Place definitions by their dependencies

Put `use "std/args";` first, then fixed constants and reusable `def` declarations,
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
Keep workload behavior separate from serving policy: the program is the
deployment, and the client's `workload` and its `session` are a file of
their own, given when the program runs (`serq run model.sq --workload
workload.sq`, [paired files](language.md#paired-files)); request handling
is in `server` or a named gateway. A `workload` inside `main` is an error.
Omit `session` for one turn. A multi-turn session uses `turn;` and states how
the next turn follows its response. Route to a gateway in the server with
`gw.route();`.

Current syntax requires `pool`, `stage`, and other deployment declarations
inside `main`, even when their settings are fixed. Group them together before
the server where dependencies allow. Do not enforce a rule that `main`
contains only the `server`; input-dependent configuration belongs there
too. The workload file declares what only the client needs: its inputs
(`let rate = args.number("rate", 0.3);` at the top level), its `def`s, the
stages it runs on and the pools it holds itself (a cap on live sessions).
An example's workload goes in `examples/workloads/`, possibly shared with
other programs, and a line of `examples/pairs.json` names it.

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

State units beside settings when the name alone does not make them clear,
and the upstream name a setting stands for (`let B = 8192;  //
max_num_batched_tokens`): a reader of the program alone still sees the
correspondence. Comments should explain assumptions and source
correspondence, not repeat syntax.

A program a use case presents keeps, beyond those settings comments, only a
short header that says what it models and where it is explained. The page
explains it a part at a time: mark each part with a named snippet section
(`// --8<-- [start:request]` … `// --8<-- [end:request]`), include it with
`--8<-- "path/to/program.sq:request"`, and say beside it what the system does,
what the program needs for it, and how serQ writes it. The assumptions and
the citations of upstream source go in that prose. When a part leaves a
block open, or closes one an earlier part opened, the prose says so. Close
the page with the whole program in a collapsed `??? example` block.

See [`examples/vendors/ascend.sq`](https://github.com/servingQ/serQ/blob/main/examples/vendors/ascend.sq):
its fixed thresholds and `waiting_class` definition precede `main`, while
the resources and request handler form the model inside it; the queue key
reads the server's parameters, which its workload
(`examples/workloads/ascend_batch.sq`) sends. Its workload and execution
conditions are supplied by the caller, for example
`serq run examples/vendors/ascend.sq --workload examples/workloads/ascend_batch.sq --horizon 1`.
When reorganizing a program, preserve its linked IR and observed behavior.
Run `serq check` and inspect `serq draw`, then compare a repeatable run before
and after. Use the repository's `make check` gate for committed changes.
See [Working with a program](development.md) for the commands and reports.
