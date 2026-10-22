# An engine's admissions

The report gives each engine that takes requests a row: its admissions
after warm-up, the re-admissions after a preemption among them, and the
share of the admitted tokens its cache gave, its token hit (#594). The
engine already recorded what each admission read and computed, `E.reused`
and `E.prefill` (#592); the row sums them, so that no reader has to, and
says what the sums leave out.

## The problem

Four readers computed the token hit from the two records themselves, each
as `Σ E.reused / (Σ E.reused + Σ E.prefill)`: `tools/thunderagent/sweep.py`,
`tests/thunderagent.rs`, tutorial 6's *from cache* column and the
ThunderAgent page's tables. The formula has two faults.

**It leaves out a connector's tokens.** With a connector an admission's
tokens are what it read from the engine's cache, what it loads and what it
computes; the records hold the first and the last, and the loaded tokens
are a third record, `E.loaded`. `vllm_lmcache.sq` with `chat.sq` at
`--horizon 2000`: the formula reads 2944.6 / (2944.6 + 729.2) = 0.80, while
the engine's cache gave 0.394 of the tokens it admitted. The 0.80 is a
share of nothing a reader would name.

**It was called vLLM's counter, which it is not.** Four places
(`docs/use-cases/thunderagent.md` twice, `docs/design/thunderagent.md`,
`sweep.py`'s docstring) called `E.reused` and its sum vLLM's
`prefix_cache_hits` over `prefix_cache_queries`. The records count every
admission; vLLM's counters leave out the admissions of a request preempted
before (below). In the run of the issue, 10 of 1862 admissions are such.

## In vLLM

At the pinned revision the scheduler records each admission's local hits
when it admits the request, "so unscheduled lookups are not counted"
(`scheduler.py:1257-1261`). `record_prefix_cache_stats` passes the request's
tokens, `request.num_tokens`, the hits of the engine's own cache and
whether the request was preempted before (`kv_cache_manager.py:253-262`).
`PrefixCacheStats.record` puts an admission of a preempted request in
`preempted_requests`, `preempted_queries` and `preempted_hits`, the others in
`requests`, `queries` and `hits` (`metrics/stats.py:132-143`), and the
Prometheus counters `prefix_cache_queries` and `prefix_cache_hits` read only
the latter (`metrics/loggers.py:1052-1057`).

A connector's hits are counted apart, by `connector_prefix_cache_stats`,
over the tokens the local cache did not give: `request.num_tokens` less the
local hits (`scheduler.py:1009-1012`). The local counter's denominator is
every token of the request, the loaded ones included.

## Before and after

`serq run examples/multi-turn/thunderagent.sq --workload
examples/workloads/closed_agents.sq --horizon 3000 --warmup 300 --N 96 --S
2048 --tr 1`, before:

```text
observe      count        mean     95% CI    cv2         p99
-----------  -----  ----------  ---------  -----  ----------
resume_wait    319    303.0541   ±63.7030  2.673   1804.6951
hit           1853      0.7388    ±0.0275  0.354      1.0000
E.reused      1862  10215.3985  ±485.2263  0.531  31600.0000
E.prefill     1862   2626.6842  ±336.7481  1.692  15109.0000
…
status     stays     mean     p50      p99  number
---------  -----  -------  ------  -------  ------
waiting     1852   0.3590  0.0165   6.5250   0.247
running     1863  11.4123  8.1217  52.8441   7.861
preempted     10   3.5089  2.7486  13.6533   0.013
```

After, the same, and under the statuses:

```text
engine  admits  readmits  token hit
------  ------  --------  ---------
E         1862        10      0.795
```

The other programs that name the two faults:

| Program, workload | Row | The records' formula |
|---|---|---|
| `vllm_lmcache.sq`, `chat.sq`, `--horizon 2000` | `vllm  5752  6  0.394` | 0.80 |
| `llmd_nixl_pull.sq`, `chat_reply.sq`, `--horizon 300` | `P  765  0  0.751`, `D  1334  0  0.797` | 0.751 and 0.980 |

The decoder `D` of `llmd_nixl_pull.sq` reads 1127 tokens a request from
the prefiller (`D.loaded`) and recomputes the last: by the records' formula
it would hit 98 % of its tokens, where its own cache gave 80 %.

In the JSON the row is `engines.E`, `{"admissions": 1862,
"readmissions": 10, "token_hit": 0.795…}`, and pyserq's `Report.engines`
and `Report.engine("E")`.

## The design

**One row per engine name.** A family's members (`E[0]`, `E[1]`) are one
row, as its records `E.reused` and `E.prefill` are one observation per
family; the rows are the engines with records, in declaration order. A
program with no engine prints no table and its JSON has `"engines": {}`.

**After warm-up, as the records.** An admission counts when its records'
samples count, under the same flag, so `admissions` is the count of
`E.reused` by construction. The pools' `admissions` count the whole run
(the `E.kv` pools' 1130 and 1042 above); this table does not, because it
sits beside the records it sums.

**Every admission.** A request preempted and admitted again is admitted
again, and the row counts it, as the records do: one engine keeps one
population in one report. `readmissions` counts the admissions of a
request preempted since its call (`request.preempted`, vLLM's
`num_preemptions > 0`): the size of the difference from vLLM's counters,
which the documentation states with its citations.

**The token hit is the engine's own cache over every admitted token.**
`Σ E.reused / Σ request.known`, each read at an admission, as vLLM's local
counter. Without a connector an admission's tokens are what it read and
what it computes, so this is the readers' formula and none of their
numbers moves (tutorial 6's 0.442 and 0.032 are the table's). With one, a
loaded token is among the tokens and never a hit; `E.loaded` already gives
the loaded tokens, so the table has no column for them. NaN, `null` in the
JSON, when no admission had a token (`kong_svf.sq` admits requests of none).

**One function records an admission.** An engine that takes nothing
admitted its request where the call is, and wrote the records there a
second time. It now calls the same `request_admitted` as an admission by
`taking`, which also counts it: a rule kept in two places is fixed in one.
With nothing taken the function writes the same samples and attributes
(nothing read, the input computed), checked by every example and tutorial
pair's `serq run --json` at `--horizon 100`, identical before and after but
for `engines` and `serq_version`.

**Where it prints.** After the status table, before the gauges: both are
about engine requests, and `readmits` sits under the `preempted` row whose
stays it ends.

## The criteria

- **0, unambiguity.** The token hit has one denominator, stated where it is
  printed and documented; the claim that the records are vLLM's counters
  is replaced, in every place it was made, by what they share and how
  they differ.
- **1, intention-revealing.** A reader of the report reads the engine's
  token hit in the report, not in a formula each tool writes.
- **2, policy in the program.** What counts as a hit for a turn stays the
  program's (`observe hit = E.reused > S` in `thunderagent.sq`). The table
  counts what the engine did and adds no construct; a ratio of sums over
  admissions is not something an `observe` mean can say.
- **3, checkability.** `tests/report.rs` holds the row to its records over
  every engine of the corpus: its admissions their count, its token hit the
  readers' formula without a connector.

The IR does not change: the counts are interpreter state read when the
report is built, `RequestRecords` and `Program::observes` stay, and so do
`IR_VERSION`, the Lean generator and the oracles. The JSON gains a key, so
`REPORT_VERSION` stays 1 (`tests/report.rs` records the key).

## Known wrinkles

- The pools' counters span the whole run and this table the time after
  warm-up; a reader comparing `E.kv`'s `admits` with the engine's
  `admits` sees the warm-up's admissions. #480 and #64 touch the pool
  counters.
- `readmits` and the stays in `preempted` agree in every run checked (10
  and 10 above, 1 and 1 in `tests/report.rs`), but they are two counts: the
  table counts admissions, the status table the stays the interpreter
  closes when it reads a request's status again. The tests hold them equal
  for the programs they run, not for every schedule.

## Self-critique

**A token hit over first admissions only, as vLLM's Prometheus counters.**
It would match the dashboards' number, and it would put two populations in
one report: the records `E.reused` and `E.prefill` beside it count every
admission, and a reader comparing the row with their means would find a
difference nothing names. The row keeps the records' population and says
how large the difference is (`readmits`).

**`reused/adm` and `prefill/adm` columns (the issue's After).** They are
the means of `E.reused` and `E.prefill`, a few lines above in the same
report. One meaning, one place.

**A `loaded` share column.** vLLM's connector rate is hits over the tokens
the local cache did not give (`scheduler.py:1009-1012`), a denominator of
its own; a column over every token would be a third rate, and one over the
connector's would need a second denominator in the same row. `E.loaded`
gives the tokens.

**A row per member.** `E[0]` and `E[1]` each have a token hit, but their
records are one per family, and the four readers wanted the family's
number. A per-member row could not be checked against a record.

**An `E.readmitted` observation of 0 or 1.** Its mean times its count is
`readmits`, but it would add a field to `RequestRecords`, an IR addition,
for what a counter does.

**A confidence interval on the token hit.** A ratio of sums over
correlated admissions needs a batch-means estimate of its own; no reader
asked for one, and the sweep varies seeds instead.

**The readers' formula, kept.** It is what every reader wrote and needs no
new state. It is also wrong with a connector by a factor of two on
`vllm_lmcache.sq`, silently: the reason the row exists.
