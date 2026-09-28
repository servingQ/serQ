# seQ

A language in which an LLM serving deployment is a program. `docs/language.md`
is the spec, `docs/ir.md` the definition, `docs/review.md` the design record.

## The thing that is easy to get wrong

**The IR is the definition of a program, not the text.** `src/ir.rs` is what the
interpreter runs, what the Lean model is generated from, and what the oracle
tests read; `.seq` is one frontend. So:

- any change to the types in `src/ir.rs` bumps `IR_VERSION`, a field rename
  included;
- `serving-queue-theory`'s `scripts/gen_seq_oracle.py` pins that version and
  reads the IR by field name — it has to move in the same change;
- a new `CExpr` or `CStmt` variant drops every oracle program out of the Lean
  fragment until the generator is taught it.

Price an IR version at a cross-repository handshake, not a line of code.

## Checks

```
make check        # fmt, clippy, tests, every program links and draws, oracles agree
make oracle-ir    # regenerate tools/oracle/*.ir.json
make draw-golden  # regenerate tests/golden/
```

`make check` is the gate. Generated files (`tools/oracle/*.ir.json`,
`tests/golden/`) are committed, so regenerate them in the change that moves them.

## Design criteria

A program is a specification someone reads, so the notation is part of the
product. Three criteria, in the order they bite:

1. **Intention-revealing.** The program should say what it means, not how it
   computes it. A serving engineer who does not know seQ should be able to read
   `programs/vllm.seq` as vLLM.
2. **Neutrality.** No construct may encode one system's policy. seQ exists to
   let you write vLLM's rule and then disagree with it; a rule the language
   supplies is a rule you cannot change.
3. **Simplicity.** A construct pays for the concept it adds. Sugar that rewrites
   to the kernel at parse time costs nothing (`docs/language.md`, the serving
   vocabulary); a new IR node costs every consumer. Prefer narrowing an existing
   form to adding one.

## Issues

**Every design issue carries a Before/After** — the code as it is today next to
the code as it would be, and the same for any generated artefact (a figure
label, a report line) the change touches. Copy the "before" from the repository
rather than paraphrasing it: half the value is that the reader can check it.

Keep an issue to one change. An issue that needs several Before/Afters for
unrelated things is several issues.
