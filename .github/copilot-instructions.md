# Reviewing serQ

serQ is a language in which an LLM serving deployment is a program. `docs/language.md` is the spec, `docs/ir.md` the definition, `CLAUDE.md` the working rules. Write review comments in Korean, short: the finding and the evidence, not the argument for it.

## What to check, in order

**The IR is the definition of a program, not the text.** `src/ir.rs` is what the interpreter runs, what the Lean model is generated from, and what the oracle tests read.

- `IR_VERSION` identifies meaning, not shape (`docs/ir.md` §Stability). First check whether `IR_VERSION` is the version of the latest tag (`git describe --tags`, `src/ir.rs` at the tag): if it is not, no change bumps it — ask instead that the PR name the change for the tag message. If it is: a removed, renamed or retyped field or variant must bump it, and so must a change of meaning under the same shape — the signal is a diff to `docs/language.md` (Semantics) or `src/engine/interp.rs` that changes what an existing statement or field does while `src/ir.rs` keeps the field or node. An added field bumps it when an old reader would miss meaning (`turns` bumped 2, default and all) and not when the field is a check, a label or a legend a reader may ignore; a stricter check never bumps it.
- An IR version bump moves `scripts/gen_lean_oracle.py` in the same PR (it pins the version and reads fields by name), with `lean/Serq/Oracle.lean` regenerated and `make lean` passing. Ask for it if the PR does not do it.
- A new `CExpr` or `CStmt` variant drops out of the Lean fragment every oracle program that uses it, until the generator learns it (`gen_lean_oracle.py` raises `Fragment` on a construct it does not know). Say so when you see one.

**Generated files move with their source.** `tools/oracle/*.ir.json` (`make oracle-ir`) and `docs/assets/*.deployment.svg` (`make draw-golden`) are committed. A change that alters one of them — the IR of an oracle program, or a figure the site shows — is incomplete without the regenerated file. Do not ask for regeneration a change does not affect.

**Claims about vLLM cite the source.** Statements about what vLLM does are checked against `ref/vllm` (pinned by `scripts/fetch_vllm_ref.sh`) and cited as `file:line`, which `scripts/check_citations.py` verifies. Flag an uncited claim, or one that paraphrases the paper or docs instead of the code.

## Design criteria for language changes

In the order they bite:

0. **Unambiguity.** One construct, one meaning.
1. **Intention-revealing.** A program says what it means, not how it computes it; `examples/multi-turn/vllm.sq` should read as vLLM to a serving engineer who does not know serQ.
2. **Policy is written in the program, not in the language.** For each construct: can a program state the opposite? The language supplies mechanisms a program selects and parameterises, not rules.
3. **Checkability.** A claim the program makes should be mechanically checkable. This is what earns an IR change.

Sugar that rewrites to the kernel at parse time is cheap; a new IR node costs every consumer. Prefer narrowing an existing form to adding one.

## The questions a review answers

Ask them in this order and write down the answer to each, including "no finding"; a review that skips one is incomplete. Every finding names a file and line and says what a reader would see there.

1. **Purpose kept.** After the change, is a program still a specification a serving engineer reads, and do the three consumers still agree: does `make check` pass, are the oracle scenarios and the trace unchanged or changed for a stated reason, does the vLLM correspondence table of `docs/language.md` still hold line by line against `ref/vllm`? A change that makes `examples/multi-turn/vllm.sq` read less like vLLM fails here even if every test passes.
2. **Simpler is possible.** Could the change be sugar instead of an IR node? Could an existing form be narrowed instead of a new one added? Does one meaning now have two spellings, or one thing two mechanisms (criterion 0)? If a construct was added, name the construct that could have carried it.
3. **Complexity did not grow.** Count what a reader must now know: the rules of `docs/language.md` (Syntax and Semantics), the fields and variants of `src/ir.rs`. A change that adds a rule must remove one or say which rule it replaces. A new IR field or variant is in the Lean fragment, or the change says it is outside it and why.
4. **Intention could be plainer.** Every new name is read from the side it is written on (the session's side in a `session` block, the scheduler's in a `server` block or a pool option); an upstream name is used where the mechanism is upstream's and not otherwise; an error message says what was read, where, and where it exists. Propose the plainer spelling, not the observation that one might exist.
5. **Evidence is the code, not the implementation.** A Before is copied from the repository, an After was run. A test's expected numbers are derived from the upstream code or the semantics by hand and the derivation is in the test's comment; a test that asserts what the implementation happens to do is not evidence. What is not verified is stated as such in the PR (an oracle path never exercised, a scenario that needs a machine the author lacks).
6. **One change.** The PR does one thing and moves everything that thing touches: the spec, `docs/ir.md`, the tutorial, the editor grammars and `docs/hooks/serq_lexer.py`, the regenerated `tools/oracle/*.ir.json` and `docs/assets/*.deployment.svg`. Anything else in the diff is a separate PR.

## Not worth a comment

Formatting and lint — `make check` runs `cargo fmt`, `clippy`, and `serq fmt --check` on the `.sq` examples in CI.
