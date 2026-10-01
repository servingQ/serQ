# Colocated vs split engines at equal throughput

Issue #208, theory side serving-queue-theory#28. Prefill/decode
disaggregation can shorten the time per output token without serving more:
a colocated engine's decodes advance only in its decode steps, a decode
engine's in every step, and a decode step costs about the same whatever it
carries. These are simulations of the committed programs, not testbed
measurements.

```
cargo build --release
tools/pd_batching/sweep.py     # about two minutes; writes raw.jsonl (not committed), results.csv, summary.md
```

| File | |
|---|---|
| `examples/pd-disaggregation/pd_ps.sq` | the idealisation: decode as processor sharing at a constant fraction `f` of each of `N` engines, colocated or pooled as `N f` engines |
| `examples/pd-disaggregation/pd_batching.sq` | the engines: 4 colocated vs 3 prefill + 1 decode, vLLM's step cost, exclusive steps (one prefill alone, or decodes only; a waiting prefill goes first), memory that never binds, a free and instantaneous transfer |
| `results.csv` | one row per run (mode, load, case, seed) |
| `summary.md` | mean ± 95 % CI over five seeds |

Both modes of a program draw the same requests from a seed: the output
tokens of every session agree between the modes, and so does the output
throughput of a seed.

TPOT of a request is (last token − first token)/(o − 1), with o ≥ 2 by
the output law. *Request-weighted* is the mean of that over requests,
*token-weighted* all decode time over all gaps. A ratio below is
token-weighted unless it says otherwise.

## What the runs show

- **The PS identity holds and is exact in the model.** Split is colocated
  with arrival rate and capacity both multiplied by `N`: the decodes
  present at a station are the same (0.25, 0.67, 1.5, 4.0 at λ = 5–20, as
  M/G/1-PS's ρ/(1 − ρ) with ρ = 0.2–0.8), and the decode time is divided
  by 4.0 at every load. Per station: in the whole system, colocated has
  four times as many decoding. And the 4× holds at any load only because
  `f` is fixed: an idle colocated engine still decodes at a quarter of its
  speed.
- **With a 0.2 ms decode step the engines gain little at low load.** At
  λ = 20 a colocated engine is idle 60 % of the time, its decodes rarely
  wait for a prefill, and TPOT is 0.219 ms colocated against 0.208 ms
  split (5 %; 15 % request-weighted). The gain grows as the colocated
  engines fill: 0.304 vs 0.211 ms at λ = 40, 0.542 vs 0.214 at λ = 60,
  0.839 vs 0.215 at λ = 70 (3.9×; 8.6× request-weighted). The decode step
  is 0.20–0.21 ms in both modes; a decode iteration carries 3.1 decodes
  split at λ = 70 and 2.8 colocated.
- **`f` is not constant there, because the decodes drain.** A request's
  whole decode (200 tokens of 0.2 ms) is as long as one prefill step
  (2000 tokens, 40 ms), so a colocated engine empties its decodes between
  prefills and sits idle. Its decode fraction is 0.20, 0.32, 0.32, 0.27 at
  λ = 20–70, its idle time 0.60 down to 0.03. The denominator is
  wall-clock time, idle included.
- **With a 10 ms decode step (`omega = 0.01`) the ratio is 1/(1 − p).**
  The decodes no longer drain: a colocated engine is never idle, its
  decode fraction is exactly 1 − p for a prefill fraction p, and TPOT is
  16.7, 25.6, 34.6 ms colocated against 10.4, 10.5, 10.6 ms split at
  λ = 40, 60, 70 — 1.6×, 2.4×, 3.3× for p = 0.4, 0.6, 0.7, where 1/(1 − p)
  is 1.7, 2.5, 3.3. The issue's 4× is p = 0.75, at λ = 75, where the three
  prefill engines are exactly saturated: a limit, not an operating point.
  At 0.2 ms the ratio at λ = 70 exceeds 1/(1 − p) = 3.3, because a decode
  that has just started waits behind the prefill that arrived during the
  last one, and over 200 tokens that wait is not averaged away.
- **Request- and token-weighted TPOT differ colocated.** A short answer
  that sits behind a prefill has a large TPOT: at λ = 60 the request mean
  is 1.05 ms and the token mean 0.54 ms. Split, they agree.
- **The split pays in TTFT.** Three prefill engines carry what four did:
  at λ = 70, TTFT is 56 ms colocated and 124 ms split. With prompts of the
  same mean and squared coefficient of variation 9 (`h2cv9`: Poisson
  arrivals, rare very long prompts), it is 175 ms against 907 ms, and the
  response time turns worse split as well (498 vs 951 ms; at λ = 60 the
  intervals overlap). The mean prefill fraction alone does not decide the
  comparison.
- **The h2cv9 point at λ = 70 is near saturation.** The prefill engines are
  94 % busy and the prompts vary widely, so 300 s is far from steady state
  (the ±360 ms). Run for 2000 s with 200 s of warm-up, seeds 1–3, TTFT is
  154, 160, 162 ms colocated and 808, 754, 850 ms split: the conclusion
  stands and the summary's mean is high. To repeat:
  `serq run examples/pd-disaggregation/pd_batching.sq --set mode=1 --set Lambda=70 --def 'prompt_len=max(1, floor(~h2(2000, 9)))' --horizon 2000 --warmup 200 --seed 1`.

## Not covered yet

Each is a variation of `pd_batching.sq` the issue lists: mixed batches
(drop `serve exclusive prefill`, or chunk), a transfer with the cost and
the double occupancy of `llmd_nixl_pull.sq`, KV capacity and prefix reuse,
a closed or tool-loop workload, and other P/D ratios. `N`, `NP` and `ND`
size queue families, which `--set` refuses, so another ratio is another
copy of the program. Routing ties go to the lowest index (at λ = 70 the
first colocated engine admits 8 % more than the last). The report has
the time split and batches per engine but not the distribution of
inter-token latencies, which would need a per-token record.
