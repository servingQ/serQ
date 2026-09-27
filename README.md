# seQ

A language for the formal verification and simulation of LLM serving
systems. The name follows seL4: a serving system is specified once, as a
program, and the same program is simulated and checked against the real
system.

A seQ program describes a serving deployment: its memory pools and
stages, a workload, and the route every session takes through them. This
repository holds the reference implementation, the Rust crate `seq-lang`
(library `seq`, CLI `seq-lang`), together with example programs and the
vLLM oracle. The vLLM v1 program (`programs/vllm.seq`) reproduces the
upstream scheduler request for request. The Lean formalisation of the
language lives in the research repository that uses seQ
(`serving-queue-theory`, `lean/ServingQueueTheory/Seq*.lean`). There the
vLLM scheduler scenarios below are theorems.

| Path | Contents |
|---|---|
| `src/lexer.rs`, `parser.rs`, `ast.rs` | surface syntax |
| `src/link.rs` | name resolution, constants, compiled expressions |
| `src/sim.rs` | the interpreter: pools, stages incl. the `step` engine, sessions |
| `src/report.rs`, `stats.rs`, `trace.rs` | reports, statistics, trace corpora |
| `programs/*.seq` | example deployments: M/G/1, PS, closed, agentic replica, PD tandem, routing, the vLLM v1 engine and its A100 replay; `programs/data/*.csv` replay traces |
| `tools/vllm_oracle.py`, `tools/vllm_replay_oracle.py`, `tools/oracle/` | the real vLLM v1 scheduler as an oracle, its recorded scenarios and the A100 engine's answers |
| `tools/a100/` | A100 step sweeps behind the cost model of `vllm_replay.seq` |
| `docs/language.md` | the language: syntax, semantics, vLLM correspondence, validation |
| `docs/review.md` | review of the first version against vLLM, design, verification-tooling survey |
| `scripts/fetch_vllm_ref.sh` | checks out the upstream vLLM source the docs cite (`ref/vllm`, 0c87a197) |

## Use

```bash
make check      # fmt, clippy, tests, every program links, the oracles agree
cargo run --release -- run programs/vllm.seq --seed 2 --horizon 3000
cargo run --release -- run programs/agentic.seq --set N=32 --set C=3e5 --set maxctx=1.5e5 --json
cargo run --release -- check programs/replica.seq
```

As a dependency, pin a release tag:

```toml
seq = { package = "seq-lang", git = "https://github.com/vrvrv/seQ", tag = "v0.1.0-dev1" }
```

The CLI can be installed with
`cargo install --git https://github.com/vrvrv/seQ --tag v0.1.0-dev1 --locked --root <dir>`,
or taken from the release assets.

## Releases

Pushing a tag `vX.Y.Z` or `vX.Y.Z-devN` that matches `Cargo.toml` runs
`.github/workflows/release.yml`. It runs the full check, then publishes a
GitHub release with the packaged crate and the Linux CLI. A tag with a
dash is published as a prerelease. Publishing to crates.io is a separate
job, which stays off until two things are set: the repository variable
`PUBLISH_CRATES_IO` is `true`, and the secret `CARGO_REGISTRY_TOKEN`
exists. crates.io is public and permanent, and the crate needs a
`license` field first.
