# Paired files

A deployment and a workload can be written in two files and paired by the
tool: `serq run DEPLOYMENT --workload WORKLOAD`, `serq::compile_paired`.
The workload file has no `fn main()`. The two meet only at the server's
signature ([request interface](request-interface.md)). The IR does not
change: a pair compiles to the program the two files would make as one.

## Before and after

`examples/multi-turn/vllm.sq`, `vllm_chat.sq`, `single-turn/vllm_single_turn.sq`
and `subagent/vllm_subagents.sq` are one engine under four workloads, written
four times; `tests/workloads.rs` holds the four engines' text equal. After,
`examples/paired/deployments/vllm.sq` is the engine once, and
`examples/paired/workloads/{multi_turn,chat,single_turn,subagents}.sq` the
four workloads:

```
serq run examples/paired/deployments/vllm.sq --workload examples/paired/workloads/chat.sq --horizon 200
```

reports what `serq run examples/multi-turn/vllm_chat.sq --horizon 200` does
(stages and pools by name: a pair declares the workload's own stage first).

## The rules

- The deployment file is a program without `workload`; paired, a
  `workload` in it is refused.
- The workload file has no `fn main()`. It declares its `use`s, constants
  and inputs (`let`, `args.number` among them), definitions (`def`), the
  stages it runs on itself, and exactly one `workload { … }`; anything else
  (`pool`, `engine`, `server`, …) is the deployment file's and refused.
- Each file reads its own names. The server, its routes and the
  scheduler's expressions read no constant of the workload file; the
  workload reads none of the deployment's; a constant reads its own file's;
  a name both files declare is refused. What crosses is the signature.
- The inputs of both are the program's.
- A message names its file, at its own line (`chat.sq: 3:5: …`).

## How

The tool lexes both files and puts the workload file's tokens at the head
of the deployment's `fn main()` body (its `use`s before the deployment's),
then parses one program. Its lines are numbered after the deployment's, in
one text (`Paired::combined`), so every position the parser, the linker and
the IR's check record maps back to its file (`Paired::locate`), and the
outline splits into the two files' (`Outline::split`, `serq::outline_paired`).
The parser records the workload file's names (`Program::paired`), and the
linker refuses a read across at the one place it resolves a name, by the
side reading (`Linker::cross`), as it does the attributes.

## Self-critique

- **`use "./workload.sq"` in a third file.** Imports would make a program a
  set of files with module namespaces (`chat.Lambda`): a module system for
  one pairing. The tool pairs two files instead; serQ's programs stay one
  program.
- **A top-level `workload` in every program.** The workload file writes
  `workload` outside `fn main()`; a single-file program still writes it
  inside. Moving it out everywhere would be a second migration for no
  meaning: in one file, the workload reads `fn main()`'s inputs.
- **Renaming each file's names apart.** A hidden prefix would let both
  declare `p`, but every message would show the mangled name. A name both
  declare is refused.
- **Token-level reads across.** A first version refused a token of the
  other file's name; it refused `hbm (k)`, a parameter, beside a workload's
  `let k`. The linker refuses by meaning instead.
- **`serq fmt` on half a pair.** A half parses only with its other half, so
  `fmt` checks a half by its tokens alone (it never changes one); `serq
  check D --workload W` is its parse.
- **pyserq.** It compiles one file; a pair is the CLI's and the library's
  for now.

## Validation

Measured 2026-10-14 against 9a49115, `serq run … --horizon 200 --json`: each
of the four pairs reports what its single-file example does, stages and
pools compared by name (`tests/paired.rs` holds two of them equal as JSON).
`scripts/check_rust.sh` links and draws every deployment under every
workload. No IR, oracle, claim or Lean file moves.
