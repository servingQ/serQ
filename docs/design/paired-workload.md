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
as JSON, byte for byte.

## The rules

- The deployment file is a program without `workload`; paired, a
  `workload` in it is refused.
- The workload file has no `fn main()`. It declares its `use`s, constants
  and inputs (`let`, `args.number` among them), definitions (`def`), the
  stages it runs on itself, and exactly one `workload { … }`; anything else
  (`pool`, `engine`, `server`, …) is the deployment file's and refused.
- Each file reads its own names. No constant or definition of the
  workload file is read by the deployment, wherever (the server, a pool's
  capacity, an engine's schedule, a constant), and none of the deployment's
  by the workload; both may declare `p`, each its own. The workload runs on
  its own stages (and the `tool` it needs none for), the server on none of
  them. What crosses is the signature.
- The inputs of both are the program's.
- A message names its file, at its own line (`chat.sq: 3:5: …`).

## How

The tool lexes both files and puts the workload file's tokens at the end
of the deployment's `fn main()` body (its `use`s before the deployment's),
then parses one program. At the end, each declaration keeps the index it has
in the one file the pair would make, so a program split into a pair keeps
its IR (the oracle's and the claims' among them). Each file's constants and definitions are
renamed apart inside the pair (`w·Z`, `d·B`), so a name of the other file
resolves to nothing wherever it is read, in an expression, a capacity or a
family's size, and a message says whose it is in place of "unknown"; the
names a message shows are as written. Stages keep their names (a report and
a figure show them): the linker checks, by the side running one, that it is
its file's (`Linker::paired_stage`). The workload's lines are numbered after
the deployment's, in one text (`Paired::combined`), so every position the
parser, the linker and the IR's check record, and each one a message quotes
(`declared at 6:5`), maps back to its file (`Paired::render`), and the
outline splits into the two files' (`Outline::split`, `serq::outline_paired`).

## Self-critique

- **`use "./workload.sq"` in a third file.** Imports would make a program a
  set of files with module namespaces (`chat.Lambda`): a module system for
  one pairing. The tool pairs two files instead; serQ's programs stay one
  program.
- **A top-level `workload` in every program.** The workload file writes
  `workload` outside `fn main()`; a single-file program still writes it
  inside. Moving it out everywhere would be a second migration for no
  meaning: in one file, the workload reads `fn main()`'s inputs.
- **Checking each read across.** A first version refused a token of the
  other file's name (it refused `hbm (k)`, a parameter, beside a workload's
  `let k`), then the linker's reads of constants by side: a pool's capacity,
  a family's size and a `def` slipped past both. Renaming each file's names
  apart closes every place at once, since nothing resolves a name of the
  other file; messages take the renaming off.
- **Refusing a name both declare.** With each file its own namespace there
  is nothing to refuse: the chat workload and the engine may both say `p`.
- **`serq fmt` on half a pair.** A half links only with its other half;
  `fmt` parses it for its syntax beside an empty other half
  (`parser::parse_half`), and `serq check D --workload W` checks the rest.
- **pyserq.** It compiles one file; a pair is the CLI's and the library's
  for now.

## Validation

Measured 2026-10-14 against 9a49115, `serq run … --horizon 200 --json`: each
of the four pairs reports what its single-file example does, as JSON
byte for byte (`tests/paired.rs`).
`scripts/check_rust.sh` links and draws every deployment under every
workload. No IR, oracle, claim or Lean file moves.
