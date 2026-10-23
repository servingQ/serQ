# Collapse and recovery of vLLM's prefix cache

Issue #120; the theory side (the Lean theorems, the exact chain, the
minimal models) is serving-queue-theory's `research/metastability.md`.
When turns wait, their prefixes can be evicted before they run; a turn
that loses its prefix computes it again, which makes the turns behind it
wait longer:

> more waiting → prefixes lost while waiting → more recompute → slower service → more waiting

This study runs `examples/multi-turn/vllm.sq`, as committed, through that
loop: a burst on a closed loop of people and on conversations that arrive
from outside, from different starts, under three eviction orders and four
changes to the deployment. These are simulations of the committed
programs, not testbed measurements.

```
cargo build --release
tools/metastability/study.py              # about three minutes on 56 cores; writes raw.jsonl (not committed), results.csv, summary.md
tools/metastability/study.py --summary    # rewrites results.csv and summary.md from raw.jsonl, without running
```

| File | |
|---|---|
| `examples/workloads/closed_burst.sq` | `N` people in a closed loop over a fixed context of `K0 = 16 000` tokens, 500 new and 200 output tokens a turn; the burst is a shorter think time; `Z_warm` until `warm_until` sets the start |
| `examples/workloads/open_burst.sq` | conversations of 10 turns on average arriving at `Lambda` a second, the same context and turns; the burst is a higher arrival rate; `max_live` caps the conversations in at once |
| `study.py` | the grid, the runs, the measures, `results.csv` and `summary.md` |
| `results.csv` | one row per run and horizon read: experiment, arm, load, start, burst, horizon, seed, and the measures over the last half of the horizon |
| `summary.md` | mean ± 95 % CI over five seeds, the windows around the closed burst, and the band |
| `test_study.py` | the edits apply once and link, the measures on made-up series, two rows of `results.csv` rerun (`make check`) |

[The workloads' page](../../docs/use-cases/workloads.md#bursts-and-the-prefix-cache)
explains the two workloads.

## Setup

**The deployment.** `vllm.sq` with `--blocks 34000`: 544 000 tokens, about
32.6 contexts of 16 700 tokens (`K0` plus a turn's mean message and
answer). A turn that finds its context computes about 500 tokens; one that
does not computes about 16 700, a third of a second at `a = 2e-5` s a
token.

**The measures**, per window of 25 s:

- *full hit*: an admission that reused at least `K0` tokens (`vllm.reused`
  from `--dump`). serQ's cache matches positions, not content, so a turn
  also counts as hit up to about 500 of its new tokens; the full-hit
  criterion does not see them.
- *recompute*: the prefill tokens an admission computes (`vllm.prefill`).
- *goodput*: turns whose TTFT is at most 2 s, per second.
- *waiting* and *KV used*: two gauges the study adds before the server,
  `gauge q = queued(reqs);` and `gauge kv_used = used(kv);`, and for the open
  workload `gauge door = queued(live);`, the conversations waiting to enter.
  They observe and change nothing. *KV used* is the allocated part of the
  pool; the cached part is not a gauge.
- *collapsed*: a window below 0.1 full hits; a *collapse* is the first 4
  collapsed windows in a row. *Collapsed at end*: the last 500 s all
  collapsed.
- *recovered*: the first 4 windows in a row, after the burst, within 0.1
  full hits and 2 waiting turns (plus conversations at the door) of the
  level over 500–1000 s; never is `inf`.
- *growing* (open only): the backlog's mean over the last quarter of the
  run exceeds the third quarter's by more than 5 and a fifth.

**The arms** are lines of the program the study states otherwise (each
must occur exactly once, or the study stops), or inputs:

| Arm | Change | What it is |
|---|---|---|
| `lru` | none: `evict lru;` | vLLM's free queue (`docs/language.md` §7, which cites the source) |
| `full` | `evict by (waiting);` | a waiting turn's prefix goes after every other entry |
| `partial` | `evict by (waiting * (age < 10));` | the same, for 10 s after the entry's release |
| `compute2x` | `let a = 1e-5;` | half the compute per scheduled token |
| `kv1.5x` | `--blocks 51000` | 1.5 times the KV |
| `live30`, `live25` | `--max_live 30`, `--max_live 25` | at most 30 or 25 conversations in at once (open only) |

**The experiments**, seeds 1–5 each:

- E1: the closed loop, `N = 40`, `Z = 4` s, a burst of `Z = 1.5` s for 60
  or 300 s from 1000 s, horizon 2000 s; `lru`, `full`, `partial`.
- E2: the closed loop, no burst, `Z` in 2, 2.5, 3, 4 s, from a warm start
  (think 50 s until 600 s) or a flushed one (all first turns at once),
  horizons 2000 and 8000 s; `lru`, `full`.
- E3: the closed loop, warm, the 300 s burst, then `Z` in 2–4 s; `lru`.
- E4: the open conversations, `Lambda` from 0.20 to 0.90 by 0.05, without
  and with a burst of 1.5 conversations/s for 60 s from 1000 s, horizons
  2000 and 8000 s; every arm.

A run of 2000 s is the first 2000 s of the run of 8000 s with the same
seed (the interpreter draws nothing ahead of the clock), so the study
reads both horizons from one run; `test_study.py` reruns one at 2000 s.

## Findings

Numbers are from `summary.md`, mean ± 95 % CI over five seeds unless a
count of seeds is given.

### Collapse and recovery (Q1)

- **The closed loop has a cliff and no hysteresis.** With `lru` and no
  burst, the full hits are 0.74 at `Z = 4` s, 0.64 at 3, 0.34 at 2.5 and
  0.03 at 2 (8000 s). A warm and a flushed start give the same level at
  every `Z`, at 2000 s and at 8000 s, and so does the run after the 300 s
  burst over the same windows (E3). The serving-queue-theory note found
  the same with v0.1.3 ("a cliff without hysteresis").
- **The closed loop recovers from a burst.** In the 300 s burst `lru`
  falls to 0.12 ± 0.06 full hits, about 20 waiting turns and a TTFT of
  7 s; it is back at its level 25 s after the burst in four seeds and
  275 s in the fifth. After the 60 s burst: 15–90 s. The note's v0.1.3 runs
  gave 65 s and 50 s. A closed loop cannot outrun its server: with fewer
  people thinking, fewer turns arrive.
- **The open conversations are bistable between two loads.** Without a
  burst no seed of `lru` collapses in 8000 s up to `Lambda = 0.50`; two of
  five do at 0.55 and all from 0.60. With the burst (1.5 conversations/s
  for 60 s), the collapse lasts to the end of 8000 s in two seeds at 0.30,
  four at 0.35 and 0.40, and all five from 0.45. Collapsed, the engine
  serves 2.96 turns/s with no full hit, the TTFT is hundreds of seconds
  and grows, and the KV is about half allocated.
- **A long transient is not a lasting collapse, and the horizons tell them
  apart.** At 0.25 every seed recovers from the burst, but after 290–2215 s
  in four of them; at 2000 s two of five were still collapsed. At 0.30
  two seeds recover after 1965 and 5290 s and two do not within 8000 s.
  Without a burst at 0.55, no seed has collapsed by 2000 s and two have by
  8000 s. Finite runs show persistence over 8000 s, not instability.

### Protection (Q2)

- **In the closed loop, protecting waiting turns removes the cliff in
  this range.** `full` keeps 0.69 full hits at `Z = 2` s where `lru` has
  0.03, and 9.2 turns/s against 3.1. Through the 300 s burst it keeps 0.67
  and recovers in the first window after it. Protection here is an
  eviction order: it never refuses an admission, so it costs no
  concurrency.
- **In the open conversations it moves the band by one step at most.**
  `full` holds to 0.55 without a burst (`lru` 0.50) and recovers from the
  burst up to 0.30 (`lru` 0.25). Once the conversations waiting outnumber
  the contexts the KV holds, an order cannot keep them all.
- **Partial protection is never better than full here.** With a 10 s
  window it equals `full` through the closed loop's bursts, whose think
  time is 4 s on average, and `lru` in the open conversations, whose waits
  in a collapse are far longer than 10 s.

### What to improve (Q3)

The open conversations, 8000 s. *Holds to*: the largest `Lambda` up to
which no seed collapses or grows without a burst. *Recovers to*: up to
which every seed recovers from the burst and does not grow.

| Arm | holds to | recovers to | goodput at 0.7 (turns/s) | door at 0.7 |
|---|---|---|---|---|
| `lru` | 0.50 | 0.25 | 0.00 | – |
| `full` | 0.55 | 0.30 | 0.00 | – |
| `partial` | 0.50 | 0.25 | 0.00 | – |
| `compute2x` | 0.70 | 0.55 | 7.07 ± 0.21 | – |
| `kv1.5x` | 0.70 | 0.25 | 7.07 ± 0.21 | – |
| `live30` | 0.65 | 0.60 | 7.11 ± 0.25 | 27.0 ± 22.2 |
| `live25` | 0.55 | 0.55 | 6.53 ± 0.04 | 344.1 ± 140.7 |

- **The ranking depends on the question.** More KV holds as far as twice
  the compute without a burst (0.70), but after the burst it recovers no
  further than `lru` (0.25): a collapsed engine computes every turn whole,
  and memory does not make that faster. Twice the compute moves both.
- **A cap on conversations recovers furthest.** `live30` recovers up to
  0.60 and never collapses: it is the admission rule of the note's Lean
  result, written as a cap on the workload's pool `live`. It pays at the
  door: at 0.70, 27 conversations wait to enter on average, and from 0.75
  the door grows in four seeds of five. `live25` caps too low: its door
  grows from 0.60.
- Transfer bandwidth is not an arm (Limits).

### The band, predicted

Two numbers per arm from the program's constants and, for the upper one,
the response time measured at `Lambda = 0.20` (`summary.md`, The band):

- **The lower end held.** *Λ_low* = μ_cold / E[turns], μ_cold = 1 / (a
  (K0 + E[n] + E[o])) the turns a collapsed engine serves a second: above
  it, a collapse cannot drain. It is 0.299 for every arm but
  `compute2x` (0.599). The first load where a seed does not recover from
  the burst is 0.30 for `lru`, `partial` and `kv1.5x`, 0.35 for `full` and
  0.60 for `compute2x`: within one step of the grid. The measured μ_cold is
  2.96–2.98 turns/s against 2.99 (5.78 against 5.99 for `compute2x`).
- **The upper end did not.** *Λ_high* = C / ((E[turns] − 1) Z + E[turns]
  R_warm), the load at which the conversations in at once fill the KV on
  average, is 0.88 for `lru`; the runs collapse from 0.55. Nor is the
  chance that the conversations in at once exceed C (Poisson, M/G/∞) the
  trigger: the arms first break where it is 0.0061 (`lru`), 0.00017
  (`kv1.5x`) and 0.17 (`compute2x`). The escape from the warm state
  depends on how often the cache overflows and on what a miss costs; a
  model of it is the minimal-model work left to serving-queue-theory.

## Limits

- **Not measurement.** Every number is the interpreter's run of the
  committed programs. A testbed run of the burst protocol (loss onset,
  `C − N`, memory per waiting turn) needs a GPU.
- **One context per conversation.** `K0` is fixed and the cache matches
  positions: a turn may count up to about 500 of its new tokens as hit.
  The full-hit measure is unaffected; *recompute* is low by up to that.
- **The open burst thins a faster stream.** An arrival's rate may not read
  the clock, so the burst is a Poisson stream at the larger rate whose
  arrivals are kept with probability `rate() / top`.
- **Protection is an order, not a pin.** An eviction order costs no
  concurrency, so the trade-off of Q2 — memory held for waiting turns
  against turns admitted — is not in this study. A pin that takes blocks
  cannot be written on one engine today: a server's lease (`hold kv … lease
  kv (t)` after the engine's call) takes the blocks, but the engine's next
  admission does not read them as its hit (closed, `Z = 4` s, seed 1,
  200–1000 s:
  0.94 of the KV in use and 2 801 tokens reused a turn on average, against
  12 542 without the lease). The P/D source lease (`kvt.nixl_pull … ttl`,
  `examples/pd-disaggregation/llmd_nixl_pull.sq`) is the pin the language
  has.
- **No transfer bandwidth on one engine.** That arm, and the note's
  prediction that a P/D decoder holding prompt blocks while it waits for
  the transfer is metastable, need the P/D programs.
- **Finite runs.** Persistence over 8000 s, five seeds, is not a proof of
  instability; the band's ends are read on a grid of 0.05.
