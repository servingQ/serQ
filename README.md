<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/serq-logo-dark.svg">
    <img src="docs/assets/brand/serq-logo.svg" alt="serQ" width="300">
  </picture>
</h1>

*pronounced "ser-Q" — **se**rving + **Q**ueue*

A language in which an LLM serving deployment is a program: memory pools,
stages, a workload and the policy every session runs, written once and both
simulated and formally checked against the real system. `examples/oracle/vllm_request.sq`
and `examples/replay/vllm_replay.sq` reproduce the upstream vLLM v1 scheduler
request for request on its oracle scenarios — the
correspondence, cited `file:line` against `ref/vllm`, is
[`docs/language.md` §7][language].

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
frontend that compiles to it. The Lean formalisation is the package in
`lean/` ([docs/lean.md](docs/lean.md)), where the vLLM scenarios below are
theorems.

[ir]: docs/ir.md
[language]: docs/language.md
[examples]: examples/

## Quickstart

```bash
cargo run --release -- run examples/multi-turn/vllm.sq --seed 2 --horizon 3000
cargo run --release -- ir examples/multi-turn/vllm.sq > vllm.json      # the IR
cargo run --release -- draw examples/multi-turn/vllm.sq --format svg --out vllm.svg   # experimental
```

As a dependency, pin a release tag:

```toml
serq = { git = "https://github.com/servingQ/serQ", tag = "v0.1.2" }
```

Or install the CLI directly:

```bash
cargo install --git https://github.com/servingQ/serQ --tag v0.1.2 --locked --root <dir>
```

## Layout

| | |
|---|---|
| `src/` | IR, text syntax (lexer/parser/AST), the linker, the interpreter, reporting, drawing |
| `examples/` | example deployments by workload — `single-turn/`, `multi-turn/`, `subagent/`, `pd-disaggregation/`, `replay/` (trace replays, with their `data/`), `oracle/` (the vLLM scheduler oracle's program) |
| `tools/` | the real vLLM v1 scheduler as an oracle, its recorded scenarios, the A100 cost-model sweeps |
| `docs/` | [the IR definition][ir], [the text syntax and vLLM correspondence][language], the design record (`docs/review.md`, `docs/design/`) |

```
make check   # fmt, clippy, tests, every program links and draws, the oracles agree
```

Every claim this project makes about vLLM is checked against the pinned
source in `ref/vllm` and cited `file:line`, not remembered from a paper or a
model's training data — see [`CLAUDE.md`](CLAUDE.md).
