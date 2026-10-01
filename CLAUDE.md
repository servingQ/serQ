# serQ

A language in which an LLM serving deployment is a program. `docs/language.md`
is the spec, `docs/ir.md` the definition, `docs/review.md` the design record.

## The thing that is easy to get wrong

**The IR is the definition of a program, not the text.** `src/ir.rs` is what the
interpreter runs, what the Lean model is generated from, and what the oracle
tests read; `.sq` is one frontend. So:

- `IR_VERSION` identifies meaning, not shape (`docs/ir.md` §Stability): a
  removed, renamed or retyped field or variant bumps it, and so does a change
  of meaning under the same shape, whatever the serde attributes (`turns`
  bumped 2); an added field a reader may ignore, or a stricter check, does
  not. The lines apply to a *tagged* version: while `IR_VERSION` has no tag,
  nothing bumps and the change goes in the coming tag's message;
- `serving-queue-theory`'s `scripts/gen_serq_oracle.py` pins that version and
  reads the IR by field name — it has to move in the same change;
- a new `CExpr` or `CStmt` variant drops every oracle program that uses it
  out of the Lean fragment until the generator is taught it (the generator
  raises `Fragment` on a construct it does not know).

Price an IR version at a cross-repository handshake, not a line of code.

## Checks

```
make check        # fmt, clippy, tests, every program links and draws, oracles agree
make oracle-ir    # regenerate tools/oracle/*.ir.json
make draw-golden  # regenerate tests/golden/ and docs/assets/*.deployment.svg
```

`make check` is the gate. Generated files (`tools/oracle/*.ir.json`,
`tests/golden/`, `docs/assets/*.deployment.svg`) are committed, so regenerate
them in the change that moves them.

## Design criteria

A program is a specification someone reads, so the notation is part of the
product. Argued in #8, in the order they bite:

0. **Unambiguity.** One construct, one meaning. First because it is the only one
   where being wrong produces a wrong *answer* rather than a slow reader.
1. **Intention-revealing.** The program should say what it means, not how it
   computes it. A serving engineer who does not know serQ should be able to read
   `examples/multi-turn/vllm.sq` as vLLM.
2. **Policy is written in the program, not in the language.** The test: for every
   construct, can a program state the opposite? The language may supply a
   *mechanism* a program selects and parameterises, not a *rule* it would
   otherwise write. Four shipped constructs fail this today (#8) — apply it to
   them before using it against something new.
3. **Checkability.** A claim the program makes should be mechanically checkable.
   This is what earns an IR change.

Cost is not a criterion, it is a budget: sugar that rewrites to the kernel at
parse time costs nothing (the serving vocabulary of `docs/language.md`), a new
IR node costs every consumer. Prefer narrowing an existing form to adding one,
and price the handshake above.

## The vLLM reference: read the code, not the memory of it

Every claim about what vLLM does is checked against the source in `ref/vllm`
(`scripts/fetch_vllm_ref.sh`, pinned at `0c87a197`), and cited by `file:line`
so that `scripts/check_citations.py` can catch the line moving. Not the
paper, not the docs, not what a model remembers of the scheduler: open
`vllm/v1/core/sched/scheduler.py`, `kv_cache_manager.py`, `block_pool.py` and
read the function. The recovery after a preemption is the standing example:
"abort the scope and re-execute" *sounded* like `_preempt_request`, and it is
only for a request still in prefill — the function resets
`num_computed_tokens` and keeps the generated tokens, which the program did
not, and no oracle scenario exercised the path (#41 §5). If `ref/vllm` is not
checked out, fetch it before answering; if the question is about a version
other than the pinned one, say so and check out that revision.

## Design documents

`docs/design/` holds the design record: the philosophy, the frontend sketch,
the IR v4 RFC and the subagent review. The IR is expected to keep evolving; a
design change adds a document there, and a rejected idea stays in that
document's self-critique with the reason.

Everything under `docs/` is written in English (the site is); issues and pull
request descriptions are written in Korean. A nav title that contains `#`
must be quoted in `mkdocs.yml`, or YAML reads it as a comment and the strict
build aborts.

## Reviewing a change

`.github/copilot-instructions.md` is the review checklist, for Copilot and
for anyone else: the IR rules, the four design criteria, and six questions
answered in order (purpose kept, simpler possible, complexity not grown,
intention plainer, evidence is the code, one change). A review that does not
answer all six is not done. Comments are in Korean, short: the finding and
the evidence.

A Claude Code session asked to review a PR spawns at most two reviewers, in
parallel, defined in `.claude/agents/`: `design-reviewer` (the checklist
above, plus the readability of the code) and `sim-reviewer` (do the numbers
reproduce, satisfy Little's law and the closed forms, and support the claim).
Call `sim-reviewer` only when the PR moves a simulated or measured number.

## Working with GitHub here

`gh pr edit` and `gh issue edit` fail on this repository:

```
GraphQL: Projects (classic) is being deprecated ... (repository.pullRequest.projectCards)
```

`gh` queries `projectCards` when it edits, and that field now errors. Use the
REST API instead — `gh api -X PATCH repos/vrvrv/serQ/pulls/N --input -` with a
JSON body, or `.../issues/N`. Creating works; only editing is affected.

PR titles are Conventional Commits, `type(scope): subject`, and the subject is
written in English; CI checks the form (`.github/workflows/pr-title.yml`).

Branch from `origin/main`, not from whatever `main` points at locally:
`git fetch && git checkout -b <name> origin/main`. Three branches were built
on a stale `main` in one sitting, and one of them would have reverted a rename
that had landed in between.

## Issues

**Every design issue carries a Before/After** — the code as it is today next to
the code as it would be, and the same for any generated artefact (a figure
label, a report line) the change touches.

**Copy the Before from the repository, and run the After.** Half the value of a
Before is that the reader can check it; an After that does not compile costs
more than none. Both rules were broken in the first hour they existed — #14's
Before was paraphrased and got the line wrong in a way that weakened its own
argument, and #12's After was a parse error.

**Write issues in Korean, short, and unwrapped** — one paragraph per line, no
hard wrap at a column, so the browser sets the width. Say the finding and the
evidence; leave out the argument for the argument.

Keep an issue to one change. An issue that needs several Before/Afters for
unrelated things is several issues.
