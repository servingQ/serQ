# seQ

Formal verification and simulation of LLM serving systems. The name
follows seL4: a serving system is specified once, as a program, and the
same program is simulated, checked against the real system, and reasoned
about in a proof assistant.

The language is **ROUTE**: a serving deployment is a program of memory
pools and stages, a workload, and the route every session takes. The
reference implementation is the Rust crate `seq-lang` (library `seq`,
CLI `route`). The formal model is the Lean package `seq` (library `SeQ`).
The vLLM v1 program (`programs/vllm.route`) reproduces the upstream
scheduler request for request, and the vLLM scheduler scenarios are Lean
theorems (`lean/SeQ/RouteOracle.lean`).

| Path | Contents |
|---|---|
| `src/lexer.rs`, `parser.rs`, `ast.rs` | surface syntax |
| `src/link.rs` | name resolution, constants, compiled expressions |
| `src/sim.rs` | the interpreter: pools, stages incl. the `step` engine, sessions |
| `src/report.rs`, `stats.rs`, `trace.rs` | reports, statistics, trace corpora |
| `programs/*.route` | example deployments: M/G/1, PS, closed, agentic replica, PD tandem, routing, the vLLM v1 engine and its A100 replay; `programs/data/*.csv` replay traces |
| `tools/vllm_oracle.py`, `tools/vllm_replay_oracle.py`, `tools/oracle/` | the real vLLM v1 scheduler as an oracle, its recorded scenarios and the A100 engine's answers |
| `tools/a100/` | A100 step sweeps behind the cost model of `vllm_replay.route` |
| `lean/SeQ/` | syntax and pool semantics, executable semantics, oracle theorems, serving order |
| `docs/language.md` | the language: syntax, semantics, vLLM correspondence, validation |
| `docs/review.md` | review of the first version against vLLM, design, verification-tooling survey |
| `scripts/fetch_vllm_ref.sh` | checks out the upstream vLLM source the docs cite (`ref/vllm`, 0c87a197) |

## Use

```bash
make check                     # Rust (fmt, clippy, tests, programs link, oracles agree) + Lean (build, axiom audit)
cargo run --release -- run programs/vllm.route --seed 2 --horizon 3000
cargo run --release -- run programs/agentic.route --set N=32 --set C=3e5 --set maxctx=1.5e5 --json
cargo run --release -- check programs/replica.route
```

As a dependency (pin a release tag):

```toml
# Cargo.toml
seq = { package = "seq-lang", git = "https://github.com/vrvrv/seQ", tag = "v0.1.0-dev1" }
```

```toml
# lakefile.toml
[[require]]
name = "seq"
git = "https://github.com/vrvrv/seQ"
rev = "v0.1.0-dev1"
subDir = "lean"
```

The CLI is installed with
`cargo install --git https://github.com/vrvrv/seQ --tag v0.1.0-dev1 --locked --root <dir>`,
or taken from the release assets. Installing it into a directory on `PATH`
shadows the system `route` command, so give it its own root.

## Releases

Pushing a tag `vX.Y.Z` that matches `Cargo.toml` runs
`.github/workflows/release.yml`: the full check, then a GitHub release
with the packaged crate, the Linux CLI and a source archive of the Lean
package. Publishing to crates.io is a separate job, off until the
repository variable `PUBLISH_CRATES_IO` is `true` and the secret
`CARGO_REGISTRY_TOKEN` is set; crates.io is public and permanent, and the
crate needs a `license` field first.
