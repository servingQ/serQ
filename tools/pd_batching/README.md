# Colocated vs split engines at equal throughput

Issue #208, theory side serving-queue-theory#28. Prefill/decode
disaggregation can shorten the time per output token without serving more:
a colocated engine's decodes advance only in its decode steps, a decode
engine's in every step, and a decode step costs about the same whatever it
carries. These are simulations of the committed programs, not testbed
measurements.

```
cargo build --release
tools/pd_batching/sweep.py     # about ten minutes; writes raw.jsonl (not committed), results.csv, summary.md
```

| File | |
|---|---|
| `examples/pd-disaggregation/pd_ps.sq` | the idealisation: decode as processor sharing at a constant fraction `f` of each of `N` engines, colocated or pooled as `N f` engines |
| `examples/pd-disaggregation/pd_batching.sq` | the engines: 4 colocated vs 3 prefill + 1 decode, the step cost and the admission of `examples/multi-turn/vllm.sq`, the KV handed over as in `llmd_nixl_pull.sq`; the baseline is exclusive steps (one prefill alone, or decodes only; a waiting prefill goes first), memory that never binds, a free and instantaneous transfer |
| `results.csv` | one row per run (experiment, mode, load, case, seed) |
| `summary.md` | mean ± 95 % CI over five seeds |

Both modes of a program draw the same requests from a seed: the output
tokens of every session agree between the modes. The output throughput of
a seed differs by up to 1 %, because the sessions that end inside the
measured span are not the same; over five seeds the means agree within
their intervals.

TPOT of a request is (last token − first token)/(o − 1), with o ≥ 2 by
the output law. *Request-weighted* is the mean of that over requests,
*token-weighted* all decode time over all gaps. A ratio below is
token-weighted unless it says otherwise. The ITL is the report's: the gap
between a decode's successive tokens on its engine.

## The baseline

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
  wait for a prefill, and TPOT is 0.220 ms colocated against 0.209 ms
  split (5 %; 14 % request-weighted). The gain grows as the colocated
  engines fill: 0.305 vs 0.213 ms at λ = 40, 0.545 vs 0.216 at λ = 60,
  0.844 vs 0.218 at λ = 70 (3.9×; 8.4× request-weighted). The decode step
  is 0.20–0.21 ms in both modes; a decode iteration carries 3.1 decodes
  split at λ = 70 and 2.8 colocated (prompts of 2000 tokens, the `fixed`
  table).
- **`f` is not constant there, because the decodes drain.** A request's
  whole decode (200 tokens of 0.2 ms) is as long as one prefill step
  (2000 tokens, 40 ms), so a colocated engine empties its decodes between
  prefills and sits idle. Its decode fraction is 0.20, 0.33, 0.32, 0.27 at
  λ = 20–70, its idle time 0.60 down to 0.03. The denominator is
  wall-clock time, idle included.
- **With a 10 ms decode step (`omega = 0.01`) the ratio is about 1/(1 − p).**
  The decodes no longer drain: a colocated engine is never idle, its
  decode fraction is exactly 1 − p for a prefill fraction p, and TPOT is
  16.8, 25.7, 34.8 ms colocated against 10.5, 10.6, 10.7 ms split at
  λ = 40, 60, 70 — 1.60×, 2.41×, 3.24× for p = 0.4, 0.6, 0.7, where
  1/(1 − p) is 1.67, 2.5, 3.33. The 3–4 % short is the step: the split's
  decode engine carries larger batches (83–150 against 34–122), and its
  step is longer by the KV term. The issue's 4× is p = 0.75, at λ = 75,
  where the three prefill engines are exactly saturated: a limit, not an
  operating point. At 0.2 ms the colocated engine idles, so 1/(1 − p) is
  not the reference there; the decode step over the decode fraction
  (0.21/0.27 ≈ 0.78 ms at λ = 70) comes within 8 % of the measured
  0.84 ms.
- **Request- and token-weighted TPOT differ colocated.** A short answer
  that sits behind a prefill has a large TPOT: at λ = 60 the request mean
  is 1.06 ms and the token mean 0.55 ms. Split, they agree.
- **The split pays in TTFT.** Three prefill engines carry what four did:
  at λ = 70, TTFT is 56 ms colocated and 124 ms split. With prompts of the
  same mean and squared coefficient of variation 9 (`h2cv9`: Poisson
  arrivals, rare very long prompts), it is about 160 ms against 900 ms
  over a long run (below), and the response time turns worse split as
  well (about 480 against 940 ms; at λ = 60, in the 300 s runs, the
  intervals overlap). The mean prefill fraction alone does not decide the
  comparison.
- **The h2cv9 point at λ = 70 is near saturation.** The prefill engines are
  94 % busy and the prompts vary widely, so 300 s is far from steady state
  (the ±360 ms). Run for 8000 s with 500 s of warm-up, seeds 1–5, TTFT is
  155, 159, 163, 159, 161 ms colocated (mean 160; the summary's 172 is
  high) and 802, 916, 954, 901, 919 ms split (mean 898; the summary's 907
  stands), response 458–493 against 846–999 ms. To repeat:
  `serq run examples/pd-disaggregation/pd_batching.sq --set mode=1 --set Lambda=70 --def 'prompt_len=max(1, floor(~h2(2000, 9)))' --horizon 8000 --warmup 500 --seed 1`.

## The variations

One change to the baseline at a time, prompts of 2000 tokens; the last
table of `summary.md`. A policy or a family's size is a line of the
program, which `sweep.py` edits in a copy (`serve exclusive prefill;`
deleted, `let NP = 2;`); the rest are `--set`.

- **Mixed batches do not rescue the colocated engine.** Without exclusive
  steps a decode rides in the 40 ms step of a whole prompt instead of
  waiting for it: TPOT 0.534 ms at λ = 60 against 0.545 exclusive, and the
  ITL p99 at λ = 70 is the prefill step, 40 ms. Chunks of 512 tokens
  (`chunk_cap`) spread a prompt over four 10 ms steps: TPOT 0.494 ms at
  λ = 60, ITL p99 10 ms; the split stays at 0.217 ms. The split's prefill
  engines batch several prompts once exclusive steps are gone, and pay
  for it in TTFT: 151 ms at λ = 70 against 124.
- **A transfer delays the second token, not the first.** Over NICs of
  2e5 tokens/s after a 2 ms wait, the read takes 17 ms; TTFT is unchanged
  (65 ms at λ = 60) and TPOT rises from 0.216 to 0.299 ms, because the gap
  between the first token (the prefill engine's) and the second (the
  decode engine's) holds the read.
- **The decode engine's NIC takes every read.** Three prefill engines send
  and one decode engine receives, so its NIC carries λ × 2000 tokens/s.
  At 1e5 tokens/s it is 80 % busy at λ = 40: the read takes 88 ms and TPOT
  is 0.651 ms, twice the colocated engines' 0.305; at λ = 45, 172 ms and
  1.068 ms. At 5e4 tokens/s λ = 60 asks 2.4 times what the NIC carries.
- **A decode engine short of KV holds the prefill engines' memory.** With
  16384 tokens it preempts now and then (0.2–0.4 per second) and TPOT is
  0.224–0.238 ms. With 8192 tokens at λ = 40 a request waits 25 ms for its
  blocks, 1.3 requests a second are preempted and recompute, TPOT is
  0.355 ms, and the prefill engines' leased KV is 5200 tokens against
  3200 at the baseline. At λ = 60 it does not keep up: in a seed-1 run the decode
  engine's admission queue reaches 1944 requests on average while their
  prompts stay leased on the prefill engines (1.1–1.5 million tokens on
  each), and TTFT stays at 64 ms. A shortage behind the decode engine does
  not show at the first token.
- **The colocated engines meet their memory limit too.** At 8192 tokens
  each, 4.6 requests a second are preempted at λ = 60 and TTFT rises from
  50 to 85 ms; at λ = 70 they do not keep up (stable at λ = 65). The
  totals differ: four colocated engines hold 32768 tokens, the split's
  decode engine 8192 or 16384 and its prefill engines what their leases
  need.
- **Another split moves the cost between TTFT and TPOT.** 2P/2D at
  λ = 30, 40: TPOT 0.206–0.207 ms against 0.251–0.305 colocated, TTFT 53
  and 77 ms against 41 and 43. Prefill engines 1.5 times as fast
  (`gP = 1.5`) make 2P/2D's TTFT 33 ms at λ = 40 and 52 ms at λ = 60,
  with TPOT 0.207–0.209 ms; specialisation is what lets the split have both.

Not covered: the distribution of where a decode engine's NIC time goes
between concurrent reads beyond max-min sharing, and N, NP, ND as
constants a run can set (`--set` refuses a family's size). Routing ties go
to the lowest index.
