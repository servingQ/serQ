# The Lean model

The Lean package defines formal semantics for a fragment of serQ and proves
properties of programs in that fragment. The Rust interpreter and Lean
semantics are tested for agreement; their equivalence is not proved.
A release tag identifies both implementations.

## What you can verify

| Result | Scope | Example |
|---|---|---|
| A scenario's outcome | One program and an explicit workload | Generated oracle theorems in `lean/Serq/Oracle.lean` |
| A program claim | All paths in the claim's workload family satisfying its assumptions | Claims and proofs for the [paper examples](use-cases/index.md#formal-verification) |
| A stochastic result | A separately defined arrival law and the theorem's assumptions | Stability and recurrence results in the Dai and Bari examples |

## Claims

Write a [`claim`](api/program.md#claim) in a program to state a property of its
paths. The simulator checks the path it runs. A Lean proof establishes the
corresponding statement over the workloads and paths admitted by that
statement.

For `examples/papers/*.sq`:

1. `make claims-ir` compiles the programs into `tools/claims/*.ir.json` and
   generates statements in `lean/Serq/Claims.lean`.
2. Proofs live beside the programs in `examples/papers/*.lean`.
3. The generated `ClaimsProved.lean` requires a proof for each claim.

The generated workload families allow up to 500 sessions. Drawn attributes
range over natural numbers, subject to `given`; this is not a probability
law. Results about random arrivals need the separate stochastic models
described in the [Dai](use-cases/dai.md), [Bari](use-cases/bari.md) and
[Kong](use-cases/kong.md) examples.

## Build and check

The package is in `lean/` and uses Mathlib `v4.34.0`. Download its cache for
the first build, then run the repository check:

```bash
(cd lean && lake exe cache get)
make lean
```

`make lean` checks generated statements, builds the semantics and paper
proofs, rejects `sorry`, and audits the declarations listed in
`lean/scripts/AxiomAudit.lean`. The permitted axioms are `propext`,
`Classical.choice` and `Quot.sound`.

To check one paper proof after building its dependencies:

```bash
cd lean
lake env lean ../examples/papers/Dai.lean
```

## Use from another Lean project

Add a pinned dependency to your Lake configuration:

```toml
[[require]]
name = "Serq"
git = "https://github.com/servingQ/serQ"
rev = "<tag or commit>"
subDir = "lean"
```

Import `Serq` or a specific module. Definitions use the namespace `SerqLang`.
The library `Serq` contains the semantics; `papers` contains proofs from
`examples/papers/`.

## Compare with the interpreter

After `make lean` and `cargo build --release`, run:

```bash
make drt
```

This compares every session's observations between the Rust interpreter and
the Lean executable for 200 fixed random cases and the full replay trace.
Random cases vary the engine configuration, iteration costs and explicit
sessions. A failing case is saved under `target/lean-drt/<seed>/`.

The random cases exercise the vLLM replay program, not arbitrary serQ
programs. Targeted regressions in `tests/lean-regress/` and
`lean/Serq/Regress.lean` cover additional cases. Passing these comparisons
is evidence of agreement on the tested paths, not a proof of equivalence.

For an individual replay comparison, use `scripts/lean_bench.py` with
`small`, `full`, `cost` or `fullcost`. The cost modes compare observations
using integer microsecond or nanosecond clocks.

## What is not covered yet

The generators accept a fragment of the IR, not the full language. See the
[IR reference](ir.md) for the translation boundary. Unsupported constructs
are rejected rather than silently omitted.

- Values and time use natural numbers. Iteration costs have a constant
  term of at least one and supported terms in `tokens`, `prefilled`,
  `decoders`, `kv_decode` and `attention`. Attention is represented by
  twice its value, so its IR coefficient must be even. Fractional
  coefficients and costs reading `residents`, `kv_prefill` or `now` are
  rejected.
- A `while` guard must be a literal 0/1, a comparison or logical expression,
  a conditional with such results, or the read-only `more` attribute of an
  explicit-session workload whose presets for `more` are all 0/1. Assignments to `more`
  and other bare attribute guards are rejected. Rust reports invalid guards
  at runtime; Lean's executable model has no corresponding error state.
- The claims fragment also supports `serve only`, a chunk expression over
  `residents` and `queued(p)`, one queue key on a pool that no run grows,
  and cost terms of the form `k * ceil(tokens / b)`.
- Renewal arrivals, transfers between pools and flows over several stages
  are outside the executable fragment. Stochastic paper results use
  separate constructions, rather than translating arbitrary stochastic
  workloads.

## Find a definition or proof

| Topic | Source |
|---|---|
| Pool semantics | `lean/Serq/Core.lean` |
| Executable semantics | `lean/Serq/Exec.lean` |
| Claims and reachable states | `lean/Serq/Claim.lean`, `lean/Serq/Inv.lean` |
| Serving order and work conservation | `lean/Serq/Serve.lean`, `lean/Serq/Fill.lean`, `lean/Serq/Work.lean` |
| Random arrivals, drift and recurrence | `lean/Serq/Chain.lean`, `lean/Serq/Poisson.lean`, `lean/Serq/Foster.lean`, `lean/Serq/Recurrence.lean` |
| Generated scenarios and claims | `lean/Serq/Oracle.lean`, `lean/Serq/Claims.lean` |
| Program-specific proofs | `examples/papers/*.lean` |
