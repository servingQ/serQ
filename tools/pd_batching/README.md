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
tools/pd_batching/sessions.py  # about fifteen minutes; writes sessions.csv, sessions.md
```

| File | |
|---|---|
| `examples/pd-disaggregation/pd_ps.sq` | the idealisation: decode as processor sharing at a constant fraction `f` of each of `N` engines, colocated or pooled as `N f` engines |
| `examples/pd-disaggregation/pd_batching.sq` | the engines: 4 colocated vs 3 prefill + 1 decode, `llmd_nixl_pull.sq`'s engines and hand-over without its prefix cache; the baseline is exclusive steps on the engines that prefill prompts (one prefill alone, or decodes only; a waiting prefill goes first), memory that never binds, a free and instantaneous transfer |
| `results.csv` | one row per run (experiment, mode, load, case, the program's lines a variation edits, seed) |
| `summary.md` | mean ± 95 % CI over five seeds |
| `sessions.py`, `sessions.csv`, `sessions.md` | the same comparison on `llmd_nixl_pull.sq` as written: multi-turn sessions with a tool call between turns and prefix reuse, open and closed |

Both modes of a program draw the same requests from a seed: the output
tokens of every session agree between the modes. The output throughput of
a seed differs by up to 1 %, because the sessions that end inside the
measured span are not the same; over five seeds the means agree within
their intervals.

The client's first token is the decode engine's when split, as llm-d
streams it: the prefill engine's token is dropped, the decode engine reads
the KV and recomputes the last prompt token. The decode engine's admission
and the read are in the TTFT. TPOT of a request is (last token − first
token)/(o − 1), with o ≥ 2 by the output law. *Request-weighted* is the
mean of that over requests, *token-weighted* all decode time over all
gaps. A ratio below is token-weighted unless it says otherwise. The ITL is
the report's: the gaps between a request's successive tokens.

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
  wait for a prefill, and TPOT is 0.220 ms colocated against 0.208 ms
  split (6 %; 16 % request-weighted). The gain grows as the colocated
  engines fill: 0.305 vs 0.211 ms at λ = 40, 0.545 vs 0.215 at λ = 60,
  0.844 vs 0.216 at λ = 70 (3.9×; 8.7× request-weighted). The decode step
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
  16.8, 25.7, 34.8 ms colocated against 10.4, 10.6, 10.7 ms split at
  λ = 40, 60, 70 — 1.62×, 2.43×, 3.26× for p = 0.4, 0.6, 0.7, where
  1/(1 − p) is 1.67, 2.5, 3.33. The 2–3 % short is the step: the split's
  decode engine carries larger batches (83–150 against 34–122), and its
  step is longer by the KV term. The issue's 4× is p = 0.75, at λ = 75,
  where the three prefill engines are exactly saturated: a limit, not an
  operating point. At 0.2 ms the colocated engine idles, so 1/(1 − p) is
  not the reference there; the decode step over the decode fraction
  (0.21/0.27 ≈ 0.78 ms at λ = 70) comes within 8 % of the measured
  0.84 ms. The split's TTFT is larger at 10 ms (72–151 ms against 54–72)
  by about two and a half of the decode engine's steps: the rest of the
  step running at admission, the step that moves the parked request to
  running, and the recompute's own.
- **Request- and token-weighted TPOT differ colocated.** A short answer
  that sits behind a prefill has a large TPOT: at λ = 60 the request mean
  is 1.06 ms and the token mean 0.55 ms. Split, they agree.
- **The split pays in TTFT.** Three prefill engines carry what four did:
  at λ = 70, TTFT is 56 ms colocated and 125 ms split. With prompts of the
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
  high) and 802, 917, 955, 901, 920 ms split (mean 899; the summary's 907
  stands), response 458–493 against 846–999 ms. To repeat:
  `serq run examples/pd-disaggregation/pd_batching.sq --set mode=1 --set Lambda=70 --def 'prompt_len=max(1, floor(~h2(2000, 9)))' --horizon 8000 --warmup 500 --seed 1`.

## The variations

One change to the baseline at a time, prompts of 2000 tokens; the last
table of `summary.md`. A policy or a family's size is a line of the
program, which `sweep.py` edits in a copy (`exclusive prefill` read as
`advance running`, `let NP = 2;`) and records in `results.csv`; the rest are `--set`.
The committed `results.csv` and `summary.md` record the edit as it was
written before the engines were (#422): deleting `serve exclusive
prefill;`, which has the IR of the edit above.

- **Mixed batches help the colocated engines only when a decode step is
  long.** Without exclusive steps a decode rides in the step of a whole
  prompt instead of waiting for it, and gains a token per prefill step: a
  share of the order of a decode step over a prefill step. At 0.2 ms that
  is nothing — TPOT 0.534 ms at λ = 60 against 0.545 exclusive — and
  prompts in chunks of 512 (`chunk_cap`) give 0.494, a 9 % that this
  share does not account for and the runs do not explain; either is far
  from the split's 0.215. At 10 ms (`omega = 0.01`, λ = 60) it is most of
  it: 19.8 ms mixed and 13.0 ms in chunks, against 25.7 exclusive and the
  split's 10.6. A decode riding with a prompt waits for it: at λ = 70 the
  ITL p99 is 40 ms whole and 20 ms in chunks; at λ = 60, where fewer than
  1 % of the gaps hold a prefill, 0.28 ms whole and 10 ms in chunks. A
  chunk limit is per prompt, so several prompts' chunks share a step of
  the 8192-token budget: at 10 ms the chunked ITL p99 is still 41 ms. The split's prefill
  engines batch several prompts once exclusive steps are gone, and pay
  for it in TTFT: 151 ms at λ = 70 against 125.
- **A transfer is in the TTFT.** Over NICs of 2e5 tokens/s after a 2 ms
  wait, the read takes 17 ms and TTFT rises from 65 to 82 ms at λ = 60;
  TPOT stays at 0.215 ms.
- **The decode engine's NIC takes every read.** Three prefill engines send
  and one decode engine receives, so its NIC carries λ × 2000 tokens/s.
  At 1e5 tokens/s it is 80 % busy at λ = 40: the read takes 88 ms and TTFT
  is 134 ms against 46; at λ = 45 (90 %), 172 and 220 ms. At 5e4
  tokens/s λ = 60 asks 2.4 times what the NIC carries.
- **A decode engine short of KV holds the prefill engines' memory.** With
  16384 tokens it preempts now and then (0.2–0.4 preemptions a second)
  and admission takes 1–3 ms. With 8192 tokens, at λ = 40 a request waits
  25 ms for its blocks, 1.3 preemptions a second recompute what they
  lost, TTFT is 71 ms against 46 and TPOT 0.230 ms, and the prefill
  engines hold 5200 leased tokens against 3200; at λ = 45 the wait is
  153 ms and the leases 17000 tokens (TTFT 202 ms over 300 s, 196–286 ms
  over 3000 s); at λ = 50 it does not keep up. The
  decode engine then serves about 48 requests a second: a third of its
  time goes to recomputing preempted requests, and three decodes fit its
  blocks. Meanwhile every waiting prompt stays leased on a prefill
  engine; their memory never binds here, and with a bounded one the
  prefill engines would stop admitting in turn.
- **The colocated engines meet their memory limit too.** At 8192 tokens
  each, 4.6 preemptions a second at λ = 60 and TTFT 85 ms against 50; at
  λ = 65, 8.4 a second and 244 ms over 300 s, 294–399 ms over 3000 s with
  300 s of warm-up (seeds 1–3): close to saturation, slow to settle; at
  λ = 70 they do not keep up (about 67 requests a second). The totals differ: four
  colocated engines hold 32768 tokens, the split's decode engine 8192 or
  16384 and its prefill engines what their leases need.
- **Another split moves the cost between TTFT and TPOT.** 2P/2D at
  λ = 30, 40: TPOT 0.206 ms against 0.251 and 0.305 colocated, TTFT 53
  and 78 ms against 41 and 43. Prefill engines 1.5 times as fast
  (`gP = 1.5`) make 2P/2D's TTFT 33 ms at λ = 40 and 52 ms at λ = 60,
  with TPOT 0.207–0.208 ms; specialisation is what lets the split have
  both.

## Sessions on llm-d's deployment

`sessions.py` asks the same question of `llmd_nixl_pull.sq` as it is
written: the llm-d router, vLLM's default mixed batches, the NIXL read at
2e5 tokens/s after 2 ms, bounded KV, and multi-turn sessions that call a
tool for 3 s between turns and reuse their prefix. Three deployments of
four engines: *colocated* is 4 decode pods that prefill every prompt
themselves (`thr` past any prompt), the router picking a pod by load as
the program's decode profile does; *colocated_cache* is the same pods,
the router picking one that has the session's prefix first, as it picks
a prefill pod; *split* is 3 prefill pods and 1 decode pod, every prompt
remote (`thr = 1`). `sessions.md` has the table; `sessions.py` adds one
`observe` to the prefill pods' entry to count what they compute, and
records each deployment's edits in `sessions.csv`.

- **With prefix reuse the cache decides before the batching does.** At
  3 sessions/s (27 turns/s) TTFT is 167 ms colocated, 36 ms
  colocated_cache and 192 ms split, the response 344, 86 and 237 ms, the
  prompt tokens computed a turn 5016, 1316 and 2294. A colocated pod
  caches the prompt and the answer of a turn, and the next turn's prompt
  is them and the new tokens; a prefill pod caches the prompt only
  (`cache (prompt)`), so the split computes the previous answer again.
  Routed by load, the colocated pods lose the prefix and prefill most of
  every prompt: their decodes then wait behind prefills, 177 ms of decode
  against 46 split, an ITL of 0.89 ms against 0.23 (a mean above the
  p99 of 0.38: the gaps beyond the p99 are the prefills a decode rides
  with). Routed by prefix, they prefill little, and the decode is 50 ms
  against 46, the ITL 0.25 against 0.23 ms.
- **At low load the split answers before the load-routed colocated pods,
  and that is the cache too.** At 1 session/s TTFT is 57 ms colocated,
  18 ms colocated_cache and 43 ms split; the decode time is 43.6 ms
  colocated and split. The split's 43 ms are its prefill of 1157 tokens
  (23 ms), the read of the 2050 the decode pod lacks (2 ms of wait and
  10 on the NIC), and the waits between.
- **Closed sessions: a faster answer brings the next turn a little
  sooner.** With 60 users colocated serves 21.11 turns/s, split 21.39 and
  colocated_cache 21.90, with responses of 180, 142 and 77 ms. A turn's
  cycle is 60/21.11 = 2.84 s, most of it the tool; Little's law, users =
  turns/s × cycle, gives 21.11 × 2.84 / (2.84 − 0.038) = 21.40 for the
  split and 21.11 × 2.84 / (2.84 − 0.103) = 21.90 for colocated_cache.
  With a tool call that long, a gain in the answer is the user's more
  than the throughput's.

Not covered here: N, NP, ND as constants a
run can set (`--set` refuses a family's size); and how concurrent reads
share a NIC beyond max-min fairness. Routing ties go to the lowest index.
