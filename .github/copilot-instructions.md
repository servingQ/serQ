# Reviewing seQ

seQ is a language in which an LLM serving deployment is a program. `docs/language.md` is the spec, `docs/ir.md` the definition, `CLAUDE.md` the working rules. Write review comments in Korean, short: the finding and the evidence, not the argument for it.

## What to check, in order

**The IR is the definition of a program, not the text.** `src/ir.rs` is what the interpreter runs, what the Lean model is generated from, and what the oracle tests read.

- Any change to the types in `src/ir.rs` — a field rename included — must bump `IR_VERSION`. Flag a diff that touches those types and leaves the version alone.
- An IR version bump has a counterpart in `serving-queue-theory` (`scripts/gen_seq_oracle.py` pins the version and reads fields by name). Ask for the linked change if the PR does not mention it.
- A new `CExpr` or `CStmt` variant drops every oracle program out of the Lean fragment until the generator learns it. Say so when you see one.

**Generated files move with their source.** `tools/oracle/*.ir.json` (`make oracle-ir`) and `tests/golden/` (`make draw-golden`) are committed. A change to the IR, a program, or the renderer without the regenerated files is incomplete.

**Claims about vLLM cite the source.** Statements about what vLLM does are checked against `ref/vllm` (pinned by `scripts/fetch_vllm_ref.sh`) and cited as `file:line`, which `scripts/check_citations.py` verifies. Flag an uncited claim, or one that paraphrases the paper or docs instead of the code.

## Design criteria for language changes

In the order they bite:

0. **Unambiguity.** One construct, one meaning.
1. **Intention-revealing.** A program says what it means, not how it computes it; `programs/vllm.seq` should read as vLLM to a serving engineer who does not know seQ.
2. **Policy is written in the program, not in the language.** For each construct: can a program state the opposite? The language supplies mechanisms a program selects and parameterises, not rules.
3. **Checkability.** A claim the program makes should be mechanically checkable. This is what earns an IR change.

Sugar that rewrites to the kernel at parse time is cheap; a new IR node costs every consumer. Prefer narrowing an existing form to adding one.

## Not worth a comment

Formatting and lint — `make check` runs `cargo fmt` and `clippy` in CI.
