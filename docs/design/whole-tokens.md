# Whole token counts

A number of tokens is a count, and serQ kept it in an `f64` that a program
could fill with any real number. Chapter 4 of the tutorial read a hit rate
of 0.9999 with nothing evicted: its workload drew real-valued token counts,
the prefix a session cached in two releases read back an ulp short of the
`K` the workload summed, and `cached >= K` missed a prefix that was all
there (#661). This document makes every token count a whole number: a
program in which one may be fractional does not link, and the program
writes the rounding it means. A stricter check: no IR field moves, and
`IR_VERSION` does not.

## Before

`docs/tutorial/programs/workloads/04-cache.sq` at serQ 13c7166:

```serq
  turn {
    set n = K == 0 ? ~uniform(1000, 3000) : ~exp(500);
    set o = ~exp(200);
    set more = ~bernoulli(p);
  }
  send (K, n, o, final: more == 0);
```

and the server of `docs/tutorial/programs/04-cache.sq`:

```serq
    hold kv (cost(kv, K + n + o)) {
      set warm = cached >= K;
      …
    } cache (cost(kv, K + n + o));
```

`serq run docs/tutorial/programs/04-cache.sq --workload
docs/tutorial/programs/workloads/04-cache.sq --horizon 20000 --warmup 2000
--seed 1` at 13c7166:

```text
observe  count    mean   95% CI    cv2     p99
-------  -----  ------  -------  -----  ------
hit      35986  0.9999  ±0.0001  0.000  1.0000

client    count     mean    95% CI    cv2      p99
--------  -----  -------  --------  -----  -------
response  44889  0.06320  ±0.00072  0.640  0.23978
```

The pool evicted nothing (`evict(n)` 0). The five misses (sessions 1149,
6103, 7461, 7983, 9100, their third or fourth turn) were rounding: the
trie (`src/engine/cache.rs`) keeps a session's prefix as the runs its
releases wrote, `[0, L₁)` and then `[L₁, L₂)`, and `cached` is their sum
`L₁ + (L₂ − L₁)`, which for real `L₁, L₂` need not be `L₂`. Writing
`cached >= K - 0.001` in the server made the hits 35986 of 35986.

## After

The same pair does not link:

```text
docs/tutorial/programs/workloads/04-cache.sq: 13:13: `n` is a number of tokens (`hold kv`: `cost(kv, K + n + o)`), and `K == 0 ? ~uniform(1000, 3000) : ~exp(500)` may not be a whole number (`~uniform` draws a real number); write the rounding it means: `ceil(…)`, or `floor(…) + 1` (#661)
13 |     set n = K == 0 ? ~uniform(1000, 3000) : ~exp(500);
   |             ^
```

The workload writes its rounding, as `examples/workloads/chat.sq` already
did:

```serq
  turn {
    // token counts are whole numbers: the workload rounds what it draws
    set n = ceil(K == 0 ? ~uniform(1000, 3000) : ~exp(500));
    set o = floor(~exp(200)) + 1;
    set more = ~bernoulli(p);
  }
```

and the same command gives:

```text
observe  count    mean   95% CI    cv2     p99
-------  -----  ------  -------  -----  ------
hit      35986  1.0000  ±0.0000  0.000  1.0000

client    count     mean    95% CI    cv2      p99
--------  -----  -------  --------  -----  -------
response  44889  0.06336  ±0.00074  0.640  0.24050
```

## Why it happened

The assumption that was wrong: that a token count may be any real number.
It was written in two places. #479 found it at a run: a prompt of
`0.4 + 1.2` prefilled in chunks of 1 summed to an ulp short of itself, and
a finished prefill read as unfinished; the interpreter was changed to end a
run at its start plus its work. #661 found it in the trie. Each fix of one
place left the other, and the trie's tolerance (`EPS`, 1e-9) does not reach
a comparison the program writes.

With whole numbers the interpreter's arithmetic on tokens is exact (an
`f64` holds every integer below 2⁵³), so the class is gone wherever it is
written. The Lean model's executable fragment already computes over ℕ
("values are natural numbers (tokens)", `lean/Serq/Exec.lean`), and its
generator refuses a division not under `floor`
(`scripts/gen_lean_oracle.py`): the rule brings the programs the
interpreter accepts closer to the numbers the proofs are about.

## The rule

A token count is a whole number. The token counts are:

| Where | IR |
|---|---|
| a request's input and output, `llm (input, output)` | `CStmt::Request.input`, `.output` |
| a hold's units on a KV pool, its `reuse`, `cache`, `prefix` lengths | `CStmt::Hold` |
| a `grow` or `shrink` of a KV pool, a `load` | `Grow`, `Shrink`, `Load` |
| the tokens a run on an engine computes | `Run.work` on a step stage or with `growing` |
| an engine's `tokens cap`, `each at most`, granule | `CStep.budget`, `.chunk`, `.granule` |
| an engine's `taking` of a KV pool, its `reuse`, `cache`, `prefix` | `CRequest` |
| a connector's `load` and `save` | `CConnector.load`, `.save` |
| a KV pool's `cap` (`inf` too) and `block` | `CPool` |
| a trace's `new` and `out` | read by `trace::Corpus::from_csv` |

A **KV pool** is one whose units are positions of a token sequence: it has a
`block`, is an engine's memory (`CStep.memory`), is taken by a hold or an
engine's admission with `cache`, `reuse` or `prefix` (the prefix cache's
pools), is a `load`'s or a growing run's pool, or is spilled to by one.
Nothing else in the IR says a pool counts tokens, and nothing needs to:
these are the pools whose units the trie and the positions are made of.

What is **not** a token count is not checked, and may be fractional:

- the units of a pool that holds no token sequence: request slots, and
  ThunderAgent's router count, which `--acting-token-weight` below 1 shrinks
  by a fraction (`examples/multi-turn/thunderagent.sq`);
- a hold's `reserve`: an admission test, and SGLang's scales what the
  running requests may generate by a real `new_token_ratio`
  (`examples/engines/sglang.sq`);
- seconds (a run's `latency`, a lease, `tool`), a stage's work, a spill's
  work.

### The inference

The check reads the expression (`src/ir/whole.rs`, `Program::validate_whole`,
called by `validate_located`). An expression is whole when:

| Expression | Whole when |
|---|---|
| a constant | an integer, or `inf` |
| `a + b`, `a - b`, `a * b`, `-a` | both are |
| `a / b`, `a ^ b` | never |
| a comparison, `&&`, `\|\|`, `!` | always (0 or 1) |
| `c ? a : b` | both branches are |
| `floor`, `ceil` (and `floor(x, step)`, `floor(x / step) * step`) | always; with `step` whole |
| `min`, `max`, `abs` | every argument is |
| `queue`, `busy`, `holders`, `queued` | always (counts) |
| `used`, `free`, `cachedin`, `allocated` | of a KV pool |
| `budget_left(E)` | `E`'s `tokens cap` is |
| `work`, `sqrt`, `exp`, `ln`, `price`, the estimators | never |
| `~bernoulli` | always |
| `~det(c)` | `c` is |
| `~exp`, `~uniform`, `~erlang`, `~h2` | never |
| a context variable | all but `now`, `waited`, `age`, `last` and `attention` |
| an attribute | every `set` of it is, and every explicit session's value (the greatest fixpoint) |
| a built-in attribute | all but `think` and the times an engine records (`E.admitted`, `E.first`, `E.load_done`); `new` and `out` are whole by the trace's reader; `cached` and `computed` are whole when every count is |
| an engine's register | its initial value and every `set` of it are |

The fixpoint starts from every attribute whole and refutes one at a time:
`set K = K + n + o` keeps `K` whole while `n` and `o` are, and makes it
fractional once one is not. What the interpreter derives (`cached`,
`computed`, `request.known`, `request.hit`, a position) is a sum, a
difference, a minimum or a block rounding of counts the check has made
whole, so it is whole by induction over the run's events.

The check is by the expression's form, not its value: `now * 0` is 0 and is
refused in a `tokens cap` (a test writes `floor(now * 0)`). A program writes
`floor` or `ceil` where the form cannot tell.

### The message

When an attribute makes the count fractional, the error points at the
assignment that made it so, where the rounding goes, and names the count:

```text
workload.sq: 13:13: `n` is a number of tokens (`hold kv`: `cost(kv, K + n + o)`), and `K == 0 ? ~uniform(1000, 3000) : ~exp(500)` may not be a whole number (`~uniform` draws a real number); write the rounding it means: `ceil(…)`, or `floor(…) + 1` (#661)
```

Through several attributes, it follows them back to the first assignment
fractional by itself (`input` ← `K + n` ← `n` ← `~uniform`). A count
fractional by itself is named where it is:

```text
`llm`'s `tokens cap` is a number of tokens, and `max(running.decoding, (2e-4 + 2e-9 * running.kv_decode) / 2e-5)` may not be a whole number (`/` divides); write the rounding it means: `floor(…)` or `ceil(…)` (#661)
```

A trace with a fractional `new` or `out` is refused when it is read:
``line 2: column 3 (new): `2004.5` is not a whole number of tokens``.

## What it reaches

Every example and tutorial program under each of its workloads
(`examples/pairs.json`), the oracle IR (`tools/oracle/*.ir.json`), the
claims IR (`tools/claims/*.ir.json`) and the Lean regressions
(`tests/lean-regress/*.ir.json`): three programs are refused, nothing else.
A run-time assertion at every token count (a scratch build, every pair at
`--horizon 100` and `--horizon 3000`) found the same three, and nothing the
static check passes.

| Program | Fractional | Edit |
|---|---|---|
| `docs/tutorial/programs/04-cache.sq` | its workload's `n`, `o` | `n = ceil(…)`, `o = floor(~exp(200)) + 1` |
| `docs/tutorial/programs/05-engine.sq` | its workload's `n`, `o` | the same (`o` was `~exp(200) + 1`) |
| `examples/multi-turn/replica.sq` | `tokens cap max(running.decoding, (omega + beta * running.kv_decode) / a)`; `llm (P, o + 1)`, `P` the prefill work in token equivalents; its workload's `n`, `o` | `tokens cap max(running.decoding, floor(…))`, `llm (ceil(P), o + 1)`, `n = ceil(…)`, `o = floor(~exp(300)) + 1` |

None of the three is an oracle or a claim's program: `tools/oracle`,
`tools/claims`, the Lean fragment and `scripts/gen_lean_oracle.py` do not
move. Chapter 3's workload draws real counts too, but its program holds no
KV pool, so it links unchanged and keeps its numbers. `replica.sq` was a
fluid model (its `tokens cap` filled each iteration "exactly" to its memory
time); it now fills it to the whole token below.

Tests that wrote fractional token counts write their rounding, each test's
point unchanged; three change more than that:

- `tests/pool_semantics.rs`, `a_run_ends_at_its_start_plus_its_work` (#479):
  the prompt `0.4 + 1.2` is now refused, which the test asserts; the
  same scenario with a whole prompt (2) keeps its two assertions.
- `tests/target.rs`, `the_adaptive_chunk_cap_is_taken_and_another_share_refused`:
  an `each at most (max(1000, B / r))` without vLLM's floor (`//`,
  scheduler.py:617-622) was refused by the vLLM target; now the language
  refuses it before the target reads it.
- `tests/pool_semantics.rs`, `bad_amounts_and_indices_fail_the_run`: the
  NaN `load` is `floor(z / z)`, still NaN at run time (#270).

`tests/whole_tokens.rs` holds the new ones: #661 as found refused and, with
the workload's rounding, every hit one; a trace's fractional `new` and
`out` refused; a `reserve`, a non-KV pool's units and a stage's work that
are fractions linking, ThunderAgent at `w = 0.5` and SGLang among them; a
KV pool's fractional cap refused.

## Validation

Measured 2026-10-15, serQ 13c7166 (Before) and this change (After), seed 1,
`--horizon 20000 --warmup 2000`:

| Program | Before | After |
|---|---|---|
| `04-cache.sq` | hit 0.9999 (35986), response 0.06320 s | hit 1.0000 (35986 of 35986), response 0.06336 s |
| `05-engine.sq` | TTFT 0.01906 s, response 0.06472 s, `llm` done 89779 | TTFT 0.01906 s, response 0.06450 s, `llm` done 89549 |
| `replica.sq` | hit 0.8415, response 3.602 ± 3.150 s | hit 0.8428, response 3.481 ± 2.374 s |

The commands: `serq run P --workload W --horizon 20000 --warmup 2000 --seed 1`
for each pair of `examples/pairs.json`, with the binary of each commit.
Chapter 4, 5 and 6 of the tutorial were measured again with the commands
on their pages. In chapter 5, 229 turns now draw an output of one token,
which the prefill samples, so they never decode: `done` is two runs per
turn but for them, and `tpot` counts the other 44 660. `replica.sq` is
bistable at its cap (`docs/review.md`), and its change is within the
seeds' spread.

## Self-critique

- **The interpreter rounds.** Rejected. Which way to round is policy, `ceil`,
  `floor` or `floor + 1` (vLLM's `max_tokens` is at least one), and a
  language that chooses it is one in which a program cannot state the
  opposite (criterion 2). `cost(kv, 12.3)` would mean 12 or 13 without the
  program saying which (criterion 0). Rounding at the use is not rounding
  at the draw: `K` sums the turns in the workload, and rounding each use of
  `K + n + o` is not the sum of rounded draws, so the run would not be the
  distribution the program states.
- **A tolerance in the program**, `cached >= K - 0.001` (#661's other
  question). Rejected: every comparison of token counts would carry an
  epsilon of the program's choosing, which says how the simulator computes,
  not what the deployment does (criterion 1). The trie's `EPS` shows that a
  tolerance in the interpreter does not reach the program's comparison.
- **A run-time refusal alone** (the run fails at the first fractional
  count). Rejected as the rule: a static check recognises the class before
  the run ("When a bug is found", 2), and a run-time failure depends on the
  path a seed draws: one seed would pass the program and another fail it.
  The static check found exactly what the run-time assertion found on every
  program here. The trace's columns, data the check cannot read, are
  refused when the trace is read, before the run.
- **Every pool counts whole units.** Rejected: a pool's unit is the
  program's to choose unless it holds a token sequence. ThunderAgent counts
  an idle program at a weight below 1 (`tests/thunderagent.rs` shrinks by
  2158.5), and a pool of fluid work is the paper's.
- **The reserve as a token count.** Rejected: it is an admission test, and
  SGLang's own reservation is a float.
- **A declared type of tokens** (`ValueType::Tokens`, a `tokens` keyword).
  Rejected: a new variant bumps the IR and the Lean generator, and the text
  a new spelling; the inference gives the same check with neither. It may
  come back if a consumer of the IR needs to know a value is a count.
- **Inference by value** (fold `now * 0` to 0). Not done: the form is what
  the program says, and a constant that is a whole number by accident is
  rare; `floor` says it.

What is left: the interpreter's tolerance on token comparisons
(`EPS` in `src/engine/cache.rs`, and #479's run end) is now unnecessary on
token counts; removing it is a change of its own.
