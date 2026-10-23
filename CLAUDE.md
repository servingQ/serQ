# serQ

A language in which an LLM serving deployment is a program. `docs/language.md`
is the spec, `docs/ir.md` the definition, `docs/design/` the design record.

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
- `scripts/gen_lean_oracle.py` pins that version and reads the IR by field
  name — it has to move in the same change, and `make lean` must pass;
- a new `CExpr` or `CStmt` variant drops every oracle program that uses it
  out of the Lean fragment until the generator is taught it (the generator
  raises `Fragment` on a construct it does not know).

Price an IR version at a cross-repository handshake, not a line of code.

## Checks

```
make check        # fmt, clippy, tests, every program links and draws, oracles agree
make oracle-ir    # regenerate tools/oracle/*.ir.json
make draw-golden  # regenerate docs/assets/*.deployment.svg
make lean         # the Lean model: oracle theorems current, lake build, no sorry, axiom audit
make metrics      # regenerate tools/metrics.json (the language's size)
make unused       # regenerate tools/unused.json (the IR variants no program uses)
```

`make check` is the gate. Generated files (`tools/oracle/*.ir.json`,
`docs/assets/*.deployment.svg`, `tools/metrics.json`, `tools/unused.json`) are
committed, so regenerate them in the change that moves them.

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
   otherwise write. Two shipped constructs still fail this (#65
   head-of-line blocking, #756 the estimates' fixed smoothing and cap):
   apply it to them before using it against something new.
3. **Checkability.** A claim the program makes should be mechanically checkable.
   This is what earns an IR change.

Cost is not a criterion, it is a budget. A new IR node costs every consumer.
Sugar that rewrites to the kernel at parse time costs no consumer of the IR,
but it is not free: it costs the frontend, which nothing verifies.

## Keeping the frontend small

From v0.1.4 to #521 the IR's statements and expressions held (one stage
kind went) and `src/frontend` grew by a third,
with 24 `fix(frontend)` commits, most of them one name read two ways. The
Lean theorems are about the IR; every line between the text and the IR is
trusted, not proved. Three rules:

- **One meaning, one spelling.** A new form that lowers to the IR of a form
  the language already has says which one it replaces, or why both stay (the
  serving vocabulary of `docs/language.md` stays for criterion 1). Check it:
  write both, diff `serq ir`. A new clause whose default can be written or
  left out (`queue fifo;`) is two spellings too.
- **One list, one place.** A name the language supplies (a context
  variable, a function, a distribution, a built-in attribute, a keyword) is
  spelt once, in its table, and every other site reads the table:
  `CONTEXT_VARS`, `Fun`, `DistKind`, `BUILTIN_ATTRS` and `SCHEDULER_ATTRS`,
  `KEYWORDS`. A check over a class of them reads the table, not a list of
  string literals. A copy that cannot read the table
  (`docs/hooks/serq_lexer.py`) is held equal to it by a test, both ways. The
  entry check's copy of the context variables lacked six (#528), and a `def`
  parameter could be named like a distribution because four checks of "a
  word of the language" kept their own lists (#530).
- **A construct some program uses.** A construct no example, tutorial or
  companion program (`serving-queue-theory/programs`) uses is a path only
  tests exercise. Add the program that needs it, or do not add it.
  `tools/unused.json` lists the IR variants no program here uses (`make
  unused`, checked by `make check`); a change that moves it says why.

## The vLLM reference: read the code, not the memory of it

Every claim about what vLLM does is checked against the source in `ref/vllm`
(`scripts/fetch_vllm_ref.sh`, pinned at `0c87a197`), and cited by `file:line`
so that `scripts/check_citations.py` can catch the line moving. Not the
paper, not the docs, not what a model remembers of the scheduler: open
`vllm/v1/core/sched/scheduler.py`, `kv_cache_manager.py`, `block_pool.py` and
read the function. The standing example: "abort the scope and re-execute"
*sounded* like `_preempt_request`, but that function resets
`num_computed_tokens` and keeps the generated tokens, and no oracle scenario
exercised the path (#41 §5). If `ref/vllm` is not checked out, fetch it
before answering; for a version other than the pinned one, say so and check
out that revision.

## When a bug is found

A fix is not done until two more questions are answered, in the pull request
or in the comment that reports the fix:

1. **Why did it happen?** Name the assumption that was wrong, not just the
   line that was wrong (#263: a serve predicate read the resident totals from
   before the iteration, so a request admitted in it was judged by totals
   that left it out). Then look for the same rule written elsewhere: a rule
   kept in two places was fixed in one, and the other is the next bug.
2. **Should the language have refused the program?** Ask whether the bad
   state is reachable only by a class of programs that a check could
   recognise before the run: in the linker or `Program::validate`, from what
   an expression reads at its moment (`docs/ir.md`, Moments). If so, add the
   check with its own test and its reason in the error message. A program
   that can only stall, loop or read a value it cannot see should not link:
   a serve key may not draw, nor since #259 a hold's header; `only` may not
   read `now` or `work(…)` (#263); a `Loop` that can pass without letting
   time pass is refused (11). If no static check can tell, the run must
   still fail loudly (a run-time error, a report note, a `stuck` counter),
   never silently. If the program was right and the interpreter wrong, say
   that no check applies and why.

A stricter check does not bump `IR_VERSION`. The regression test reproduces
the bug as it was found. A rule rejected on the way goes in the design
document's self-critique.

## Reviewing a change

`.github/copilot-instructions.md` is the review checklist, for Copilot,
CodeRabbit and anyone else: the IR rules, the four design criteria, and six
questions answered in order. A review that does not answer all six is not
done; one that finds a bug also asks the two questions above. Comments are
in Korean, short: the finding and the evidence.

Copilot and CodeRabbit review a pull request. CodeRabbit is set by
`.coderabbit.yaml`: it writes in Korean, reads this file, the checklist and
the writing guides as they stand (the config copies none of them), and
skips the generated files. It reviews again on each push, the commits since
its last review, and pauses after five reviewed commits:
`@coderabbitai review` asks for one while it is paused, `@coderabbitai full
review` reviews the whole pull request again. Each finding of either is answered on the pull request: fixed, or why not. A pull request merges
into main only once CodeRabbit has reviewed its head commit: the
"CodeRabbit review" ruleset requires its `CodeRabbit` status, and
automerge waits for it. When
Copilot's review limit is reached, the subagents in `.claude/agents/`
review instead, at most two in parallel: `design-reviewer` for the
checklist and the code, `sim-reviewer` — only when the PR moves a number —
for whether the numbers reproduce and support the claim. The session merges
their findings into one comment on the pull request.
## Writing

Follow [Writing readable programs](docs/writing-programs.md) for `.sq`
programs, [.github/documentation.md](.github/documentation.md) for the site,
and [.github/contributing.md](.github/contributing.md) for examples and
oracle scenarios. A design change adds a document to `docs/design/`, and a
rejected idea stays in that document's self-critique with the reason.

Everything under `docs/` is in English (the site is); issues and pull
request descriptions are in Korean. A nav title that contains `#` must be
quoted in `mkdocs.yml`, or YAML reads it as a comment and the strict build
aborts.

## Working with GitHub here

`gh pr edit` and `gh issue edit` fail on this repository (`GraphQL:
Projects (classic) is being deprecated`). Use the REST API: `gh api -X PATCH
repos/servingQ/serQ/pulls/N --input -` with a JSON body, or `.../issues/N`.
Creating works; only editing is affected.

PR titles are Conventional Commits, `type(scope): subject`, with the subject
in English; CI checks the form (`.github/workflows/pr-title.yml`).

Branch from `origin/main`, not from whatever `main` points at locally:
`git fetch && git checkout -b <name> origin/main`. A branch built on a stale
`main` once nearly reverted a rename that had landed in between.

## Issues

**Every design issue carries a Before/After** — the code as it is today next to
the code as it would be, and the same for any generated artefact (a figure
label, a report line) the change touches. **Copy the Before from the
repository, and run the After**: #14's Before was paraphrased and got the
line wrong, and #12's After was a parse error.

Write issues in Korean, short, and unwrapped — one paragraph per line, so
the browser sets the width. Say the finding and the evidence. Keep an issue
to one change.
