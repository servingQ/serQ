# How serQ is checked

A language whose claim is "this program *is* your serving system" has to earn
it. Four independent kinds of evidence, all run by `make check`.

## 1. Closed forms

`examples/single-turn/mg1.sq`, `ps.sq` and `closed.sq` are checked against the answers
queueing theory already knows: M/M/1 sojourn time, Pollaczek–Khinchine for four
service laws, processor-sharing insensitivity, and mean value analysis for the
closed network. A run whose confidence interval does not cover the closed form
is a failure.

This catches the boring class of bug — an off-by-one in the event loop, a
statistic computed over the warm-up — that would otherwise hide under
everything else.

## 2. A second implementation

`libqueuingsim` (in the research repository) has hand-written models of the
same deployments, built independently. `libqueuingsim/tests/seq_*.rs` runs the
serQ programs next to them:

| Program | Agreement |
|---|---|
| `replica.sq` | TTFT 0.253 vs 0.250 s, response 0.336 vs 0.333 s; over 20 seeds, hit rate, TTFT and throughput agree (Mann–Whitney p ≥ 0.05) |
| `routing.sq` | within 1–2 % on response and hit rate across five policies |

## 3. The real scheduler as an oracle

Described in full in the [vLLM case study](case-study-vllm.md). Three oracles
agreeing on six deterministic scenarios, and request-for-request agreement on a
3 321-request trace.

The important part is the method rather than the result:
`scripts/exp/diff_seq_vllm.sh` and `first_divergence.sh` do not check that the
aggregates match — they find the **first step** at which serQ and the real
scheduler disagree. That is what found the six semantic differences the first
version of the program had. An aggregate that matches can still be wrong for
compensating reasons; a first-divergence search cannot be fooled that way.

### The lints

Two of the defects this repository shipped are now link errors: a hold header
that reads a `set` bound before the session queued, and a `branch` whose
constant guard is a probability. Each is checked against the whole corpus in
`tests/lints.rs` — they are errors, not warnings, which is only defensible
while nothing real trips them.

### The citations themselves

§7's table is eleven rows of "vLLM does X, here is the serQ construct, here is the
upstream line range". `make check` now resolves all 42 of those ranges against the
pinned `ref/vllm` and hashes their text (`scripts/check_citations.py`,
`tools/citations.json`). A range that has moved, a file that no longer exists, or a
citation nobody recorded fails the build.

It checks the evidence, not the claim: a changed hash means an upstream range moved
and someone has to re-read it, which is the work the table exists to make possible.
The checkout is sparse and blobless — 4 MB, two seconds — and the paths it needs come
from the checker itself, so a citation into a new file widens it automatically.

That check is against the pin, so it cannot see upstream move. A second job,
`.github/workflows/citation-drift.yml`, asks that question daily: it fetches the
same files at `vllm-project/vllm@main` (`scripts/fetch_vllm_tip.sh`, nine raw files
over the GitHub API, no clone) and looks for each cited range's pinned text anywhere
in the current file (`scripts/check_citations.py --tip`). Line numbers are ignored;
a range whose text is gone turns the run red and names every place that cites it.
It gates no pull request. It says the cited code is gone, not that the scheduler
computes something else — that is the oracle's question, and reading upstream's
diff is the next step.

## 4. Theorems

The same program is an inductive type in Lean with an operational semantics
(`serving-queue-theory`, `lean/ServingQueueTheory/Seq*.lean`), and the Lean
program is **generated from the IR** — not hand-written alongside it.

| | |
|---|---|
| `Seq.lean` | the syntax, the pool semantics, the memory invariant |
| `SerqExec.lean` | an executable semantics of the pool and step-engine fragment |
| `SerqOracle.lean` | the vLLM scenarios as theorems, one per scenario, generated |
| `SerqServe.lean` | serving order of a step engine |

Two results worth naming:

- **`SerqLang.Step.invariant`** — `allocated + cached ≤ cap` in every reachable
  configuration of every pool, for every program.
- **`SerqLang.Serve.serve_eq_decode_first`** — without a per-request chunk cap,
  serving in admission order *is* serving decode-first (and
  `chunk_cap_breaks_shape` shows a cap breaks it). This is why the paper's
  "prefill from the budget decode leaves" describes vLLM too, and it is a
  theorem rather than an observation.

## What is not proved

- The step stage and the session-level semantics (continuations, flow) are not
  formalised yet.
- There is no proof that the Rust interpreter implements the Lean relation. The
  oracle agreement and the closed-form checks are the evidence; the [design
  review](review.md) §4 surveys the tools that would close the gap.
- Cache entries are per session. Cross-session prefix sharing — a common system
  prompt, SGLang's RadixAttention — needs a content-addressed cache and is not
  written.
- The prefill/decode program (`examples/pd-disaggregation/llmd_nixl_pull.sq`) is checked against the
  source line by line and by deterministic tests of its statements, not yet
  against a machine or a scheduler oracle: that oracle would
  drive two vLLM schedulers and a fake connector ([case
  study](case-study-pd.md)).

## Against a real machine

Beyond agreement with other models, `examples/replay/vllm_replay.sq` is fitted to an
A100 running Qwen3-8B and predicts measured runs it was not fitted on: hit
rates within 1–6 points across eight held-out configurations, **including the
one where the replica collapses**. The cost model comes from 3 022 engine steps
stepped by hand (MAPE 2.7 % decode, 5.6 % prefill, 5.7 % mixed), and the two
overhead constants are fitted on two light-load runs only.

And one pre-registered prediction, written down before the experiment: pinning
a waiting request's prefix would take a collapsed 2.5 s replay from 39.1 s mean
TTFT to 0.888 s. Measured afterwards on the real engine: 34.6 s → 0.878 s.
