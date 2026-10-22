<h1><img src="docs/assets/brand/serq-logo-on-dark.svg" alt="serQ" width="300"></h1>

*pronounced "ser-Q" — **se**rving + **Q**ueue*

A serving specification language: an LLM serving deployment is a program
— memory pools, stages and the policy every request runs, under a workload —
written once and both simulated and formally checked against the real
system. What P4 is to a packet switch, serQ is meant to be to a serving
engine; running a program in place of the engine's scheduler is the next
step ([the design](docs/design/serving-specification-language.md)).

[![CI](https://github.com/servingQ/serQ/actions/workflows/ci.yml/badge.svg)](https://github.com/servingQ/serQ/actions/workflows/ci.yml)
[![Docs](https://github.com/servingQ/serQ/actions/workflows/docs.yml/badge.svg)](https://servingq.github.io/serQ/)
[![Release](https://img.shields.io/pypi/v/pyserq?label=release)](https://github.com/servingQ/serQ/releases)
[![Rust](https://img.shields.io/badge/rust-1.98.1-orange.svg)](rust-toolchain.toml)

**[Read the docs →](https://servingq.github.io/serQ/)**

## Why

A serving system today is described three times and reconciled never: a
paper's queueing model, a scheduler's source, and whatever load test last
ran against it. serQ is one program instead. Its definition is an
intermediate representation, the **IR** (`src/ir.rs`, [`docs/ir.md`][ir]) —
a closed, versioned data structure that the interpreter runs, the Lean
model is generated from, and tools build or edit as JSON. The text syntax
([`examples/*/*.sq`][examples], [`docs/language.md`][language]) is one
frontend that compiles to it. The Lean formalisation is in `lean/`
([docs/lean.md](docs/lean.md)), where the vLLM scenarios below are
theorems.

[ir]: docs/ir.md
[language]: docs/language.md
[examples]: examples/

## Quickstart

```bash
cargo run --release -- run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --seed 2 --horizon 3000 --warmup 200
cargo run --release -- ir examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 3000 > vllm.json      # the IR
cargo run --release -- draw examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --format svg --out vllm.svg   # experimental
```

A program is the deployment, constructed inside `fn main()`; its workload
(how sessions arrive and what each turn sends) is a file of its own, given
when it runs with `--workload` (pyserq: `workload=`). The examples'
workloads are in `examples/workloads/`, and `examples/pairs.json` names each
program's. Import `"std/args"` and use `let rate = args.number("rate", 0.3);`
to expose a numeric input; pass it as
`serq run model.sq --workload workload.sq --horizon 10 --rate 0.5`. Ordinary
`let` constants stay internal. See the [complete example](examples/single-turn/arguments.sq)
(its workload: [`examples/workloads/batch.sq`](examples/workloads/batch.sq))
and [input reference](docs/api/std/args.md).

As a dependency, pin a release tag:

```toml
serq = { git = "https://github.com/servingQ/serQ", tag = "v0.1.6" }
```

Or install the CLI directly:

```bash
cargo install --git https://github.com/servingQ/serQ --tag v0.1.6 --locked --root <dir>
```

Working on serQ itself, the gate is:

```
make check   # fmt, clippy, tests, every program links and draws, the oracles agree
```

Every claim this project makes about vLLM is checked against the pinned
source in `ref/vllm` and cited `file:line`, not remembered from a paper or a
model's training data — see [`CLAUDE.md`](CLAUDE.md).
