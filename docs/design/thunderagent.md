# ThunderAgent as a serQ program

[ThunderAgent](https://github.com/ThunderAgent-org/ThunderAgent) is a proxy
in front of vLLM or SGLang replicas that schedules *programs*, not requests
(arXiv 2602.13692, ICML 2026). A program is an agent's whole trajectory,
named by the `program_id` the client adds to every request. The router
places a program on a replica at its first request and keeps it there. The
program's tokens count against that replica's KV capacity while it waits
for a tool, as well as while it reasons. When a replica is over capacity,
the router pauses programs, the idle ones first. A paused program's next
request waits until the router lets it in again. The claim is that a
program back from its tool still finds its prefix cached, where a
request-level router lets the caches thrash.

`examples/multi-turn/thunderagent.sq` writes the router in TR mode (the
default, `--router tr`) in front of two vLLM replicas. Each replica has
`examples/pd-disaggregation/llmd_nixl_pull.sq`'s local decoder, without
its `max_model_len` cap. The
program runs a fixed number of agents at once, as a rollout or a benchmark
run does. `--tr 0` is the router's `default` mode. This document gives
the correspondence, the language change the pause took, what the program
shows against the paper, and what it still does not write. Source
references are to ThunderAgent at
[`7ddc861`](https://github.com/ThunderAgent-org/ThunderAgent/tree/7ddc8610270e56d3b109eed8796b3a4360fc67c9),
the paper's to the PDF in its `assets/paper/`.

## The correspondence

| ThunderAgent | The program |
|---|---|
| A replica's capacity, `cache_config.total_tokens_capacity`, against `active_program_tokens + buffer` ([`backend/state.py#L185-L194`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L185-L194)) | `pool prog[J] { cap blocks * bs; … }`, one member per replica, the same tokens as the replica's `kv` |
| `BUFFER_PER_PROGRAM = 100` per active program ([`backend/state.py#L23`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L23)) | `let buffer = 100;`, in every amount of `prog` |
| A new program goes to the replica with the fewest active tokens among those where it fits, and waits when none has room ([`scheduler/router.py#L330-L362`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L330-L362)) | `choose j in J by (free(prog[j]) >= prompt + buffer ? 0 : 1, used(prog[j]))`, then a `hold` of `prog[j]` |
| The program's tokens are counted while it reasons and while it calls a tool, at `acting_token_weight` 1.0, the default ([`backend/state.py#L104-L107`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L104-L107), [`__main__.py#L30`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/__main__.py#L30)) | `lease prog[home] (inf)`: the allocation outlives the hold |
| A running program's next request is let through without a capacity check; its count becomes the new prompt's estimate ([`scheduler/router.py#L384-L389`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L384-L389), [`#L450-L458`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L450-L458)) | `grow prog[home] (cost(prog, prompt + buffer - allocated(prog[home])))`, on the lease |
| After the response, the count is the response's usage, the answer included ([`scheduler/router.py#L493-L494`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L493-L494)) | `grow prog[home] (cost(prog, o))` after the engine's request |
| Over capacity, pause the ACTING programs, the smallest first, then the REASONING ones ([`scheduler/router.py#L773-L805`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L773-L805)) | `preempt leases by (reasoning, prompt);` on `prog`: the growth ends idle programs' leases first, the smaller first |
| A paused program's next request waits to be let in again, on any replica with room ([`scheduler/router.py#L411-L424`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L411-L424), [`#L807-L932`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L807-L932)) | `allocated(prog[home])` is 0: the program is placed as a new one, and may move |
| A program that has waited 30 minutes is let in whatever the capacity; otherwise, programs with a pending request, then new ones, each group smallest first ([`#L934-L956`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L934-L956), [`scheduler/router.py#L851-L866`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L851-L866)) | `queue by (waited < 1800, turn_no == 0, prompt)` on `prog` |
| `/programs/release` at the end of the trajectory ([`scheduler/router.py#L514-L547`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L514-L547)) | the session's end ends the lease |
| `--router default`: the replica with the fewest programs, no capacity ([`scheduler/router.py#L252-L271`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L252-L271)) | `tr = 0`: `cap inf` and `choose … by (0, holders(prog[j]))` |

Where the paper and the code differ, the program follows the code, as the
vLLM programs follow `ref/vllm`. The paper restores reasoning programs
first and the smallest first (Eq. 10, p. 8), where the code puts programs
with a pending request before new ones. The paper has no per-program
buffer, and it weighs acting tokens by a decay `2^-t` in its experiments
(p. 9), where the code's default is a weight of 1 and no decay.

Two things the code does differently from what its wiki says:

- The wiki's capacity subtracts `shared_tokens`, the prefix cache's
  savings. At `7ddc861` nothing calls `update_shared_tokens`
  ([`backend/state.py#L129-L136`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L129-L136)
  is its only occurrence), so it stays 0.
- Marking a REASONING program adds its tokens to `future_paused_tokens` "so
  the scheduler doesn't over-pause". But `_pause_until_safe` loops on
  `remaining_capacity()`, which does not read `future_paused_tokens`. Its
  only reader is `capacity_overflow`, which only `to_dict` calls. So once
  the ACTING programs are gone, the loop marks every REASONING program on
  the replica. The program pauses only as many as the growth needs.

## The change: a lease a growth may take back

Before this change, the program could only approximate the pause. A
lease is neither evictable nor a preemption victim, so no growth could
take an idle program's tokens. The program released its lease and held
anew at every turn, and a turn whose tokens did not fit waited. The waiting
program was the one that came back, not the smallest idle one. At 32
agents that served 1.22 times the turns of default mode, with a hit rate
of 0.52.

Three additions write the pause. None is a new statement or a new kind of
node:

| Addition | IR | What it says |
|---|---|---|
| `preempt leases by (k, …);` on a pool | `Preempt::Leases { keys }`, a variant beside `By` | A growth that does not fit ends the lease with the least keys among the sessions that lease the pool and hold it in no scope. The session is not interrupted, and no hold runs again. |
| `grow P (e)` outside every hold of `P`, in a session that leases it | `Grow` unchanged; `validate` lets it act on a lease, as `Release` already may | The session's latest lease of `P` grows. If it does not fit, the pool's `preempt` applies. |
| `allocated(P)` | `Fun::Allocated` | What the session has allocated in `P`, in its holds and its leases: how a program finds that its lease is gone. |

The program's turn is then:

```serq
pool prog[J] {
  cap tr ? blocks * bs : inf;
  queue by (turn_no == 0, prompt);
  preempt leases by (reasoning, prompt);
}
…
server {
  set prompt = K + n;
  // vLLM refuses a prompt its KV cannot hold (max_model_len is at most
  // the KV, as in examples/pd-disaggregation/llmd_nixl_pull.sq): the
  // agent's trajectory ends there, in either mode
  branch (prompt + buffer >= blocks * bs) { end; }
  set reasoning = 1;
  set placed = K > 0 && allocated(prog[home]) > 0;
  branch (placed) {
    grow prog[home] (cost(prog, prompt + buffer - allocated(prog[home])));
  } else {
    choose j in J by (tr ? (free(prog[j]) >= prompt + buffer ? 0 : 1) : 0,
                      tr ? used(prog[j]) : holders(prog[j]));
    set home = j;
    set t_q = now;
    hold prog[home] (cost(prog, prompt + buffer)) {
      branch (K > 0) { observe resume_wait = now - t_q; }
    } lease prog[home] (inf);
  }
  E[home].decode (prompt);
  // the response's usage counts the answer too (update_program_after_request),
  // up to what a replica holds
  branch (allocated(prog[home]) > 0) {
    grow prog[home] (cost(prog, min(o, blocks * bs - allocated(prog[home]))));
  }
  set reasoning = 0;
}
```

A new program's `hold` never pauses anyone: in serQ only a growth
preempts, and in ThunderAgent only an over-capacity replica pauses. The
growth of a running program is what overflows a replica in both.

### The price

IR 13 is untagged, so nothing bumps, and the three additions go in the
coming tag's message. The Lean generator raises `Fragment` on `Leases` and
on `Allocated`, and no oracle program uses either. `make check` links and
draws the 30 programs and runs the oracles as before. No other program's
IR or report moves: `preempt by` keeps its candidates, and a `grow`
outside every hold of its pool did not link before.

The keys are a preempt key's: they may not draw, nor read `computed`.
Two checks are new, both stricter, so neither bumps: `preempt leases by`
does not link on an engine's memory, where a lease is KV another
instance has still to read (a prefiller's blocks before the decoder's
transfer), nor on a pool no hold leases, where it would take nothing.
Two run errors are new: a `grow` of a pool the session neither holds nor
leases any more, and a growth of a lease past the pool's cap, which would
otherwise end every lease, its own last, and go on as if it had grown.
Under `preempt none`, leases that all wait to grow for room the others
hold are named at the end of the run by the note that already names
holds waiting on each other; under `preempt leases by` no growth waits,
since the grower is a candidate itself (`tests/lease_preemption.rs`).

### Rejected on the way

- **Every lease a `preempt by` candidate.** This was the first version,
  and it is smaller: no new spelling, only wider candidates on a pool that
  is no engine's memory. But it changes what `preempt by` means for every
  program with a lease, and `tests/pd_semantics.rs`
  (`a_lease_is_not_a_preemption_victim`) pins the old meaning on purpose.
  A program could also not state the opposite (criterion 2): it could
  order leases last, never exclude them. The pool now says which it takes.
- **A clause on the hold that may revoke.** A `hold … revoking` would let
  the program say which admissions pause others. The victim order would
  then live in two places, and ThunderAgent needs only the growth to
  pause, which is the rule serQ already has.
- **A revoked lease waiting in the queue.** ThunderAgent puts a paused
  program in its waiting queue at once and may resume it before its next
  request. Written that way, a session could wait in two queues at once,
  one for its lease and one at its tool or engine. The interpreter keeps
  one pending admission per session, and that invariant is worth more
  than the early resume. The program lets a paused program in again at
  its next request.
- **A timer for the periodic scheduler.** ThunderAgent checks capacity
  every `Δt` and lets a replica overflow in between; serQ pauses at the
  instant of the growth, the limit `Δt → 0`. A timer is a new kind of
  actor that every consumer would have to learn. The paper's ablation
  moves throughput little: Δt = 2, 5, 7, 10 give about 363, 378, 370 and
  343 steps/min (Fig. 6b, p. 11, read from the figure).
- **A slack argument instead of the shared prompt.** A `slack`, the share
  of the KV the router's count leaves unused, moves TR mode's hit rate as
  the shared prompt does. It says what the prompt does rather than what it
  is, and it leaves default mode without the prompt's hits. `S` writes the
  prompt, and both effects follow from it.
- **`held(p)`** as the function's name: `held` is already the keyword of
  `reserve held`, so the name would mean two things. It is `allocated(p)`.

## What the program shows, against the paper

`tools/thunderagent/sweep.py` runs both modes over the agent counts, four
seeds each by default, with a horizon of 25 000 s and a warm-up of 5 000 s.
Steps/min are turns per minute after warm-up, the paper's unit (a
reasoning and an acting period, p. 9). The hit rate is reused tokens over
prompt tokens, vLLM's `prefix_cache_hits / prefix_cache_queries`.
"Resumed" is the share of TR mode's turns that a paused program sends.

At the example's own scale (`tools/thunderagent/sweep.py`), each replica
holds 64 000 tokens. A task's first prompt is 4 000-8 000 tokens, and each
step adds about 1 000 tokens of observation and 300 of answer:

| N | TR steps/min | default steps/min | ratio | TR hit | default hit | TR resumed/turn |
|---|---|---|---|---|---|---|
| 4 | 21.3 | 21.1 | 1.01 | 0.90 | 0.88 | 0.007 |
| 8 | 28.5 | 24.0 | 1.19 | 0.86 | 0.53 | 0.049 |
| 16 | 29.0 | 18.7 | 1.55 | 0.81 | 0.02 | 0.090 |
| 32 | 28.6 | 18.6 | 1.54 | 0.79 | 0.02 | 0.109 |
| 48 | 28.5 | 18.6 | 1.54 | 0.78 | 0.02 | 0.116 |

The paper's Fig. 4 and Fig. 5 (p. 9-10), with values read from the
figures except the printed ratios:

| Panel | N | vLLM steps/min | ThunderAgent steps/min | ratio at the largest N | hit, vLLM | hit, ThunderAgent |
|---|---|---|---|---|---|---|
| (a) SWE-agent, GLM-4.6 | 24-96 | 218, 275, 162, 140 | 220, 350, 382, 370 | 2.65 | 98 → 30 % | 98 → 93 % |
| (b) OpenHands, GLM-4.6 | 24-96 | 152, 55, 47, 52 | 155, 190, 188, 186 | 3.58 | 98 → 32 % | 98 → 94 % |
| (c) ToolOrchestra, Qwen3-8B | 24-48 | 5.05 → 4.75 | 6.05 → 7.05 | 1.48 | 13 → 12 % | 28 → 18 % |
| (d) SWE-agent, Qwen3-235B | 144-192 | 83, 145, 125 | 333, 388, 378 | 3.02 | 45 → 73 % | 96 % |
| (e) OpenHands, Qwen3-235B | 24-96 | 162, 172, 122, 70 | 164, 173, 164, 168 | 2.43 | 97 → 38 % | 97 → 96 % |
| (f) ScienceAgent, GLM-4.6 | 48-120 | 39.5 → 40 | 39.5 → 49.5 | 1.24 | 51 % | 52 → 60 % |

The gain is a closed workload's, and it is paid in waiting. At 32 agents
(seed 1), TR mode's mean time to first token is 52 s against default
mode's 81 s, but its 99th percentile is 1 836 s against 147 s: a program
the router holds back waits for room. Under open Poisson arrivals of
programs (the same program with `arrive poisson`), at rates default mode
keeps up with, 0.01-0.02 a second, the two serve the same turns and TR
mode's TTFT is a little lower (0.54 s against 1.05 s at 0.02). At 0.05 a
second default mode falls behind and its queue grows without bound,
while TR mode keeps up, with a TTFT of 16 s (99th percentile 313 s).

The example's ratio, 1.5, is below the paper's, and its hit rates are
well below the paper's on both sides. The sections below find
the reasons, one at a time.

### Why the numbers differ

Write the throughput by Little's law: steps/min = agents × 60 / (time a
step takes, its wait included). At 32 agents on the example's scale (seed
1), TR mode's step is 65 s: 63 s from request to answer, of which about
50 s is the wait in the router's queue, and 3 s at the tool. Two thirds
of that wait is the 15 % of returns that wait the full 30 minutes before
the force-resume puts them first; without that rule a large paused
program waited up to five hours, and throughput was 1-4 % higher only
because those programs never answered. Default mode's
is 99 s: 97 s at the replica, most of it waiting for blocks behind the
requests that recompute, and 3 s at the tool. The replicas run about as
many requests at once either way (a decode batch of 2.7 against 3.0); what
differs is what each request recomputes, 3 600 prompt tokens in TR mode
against 16 000 in default mode. Four things decide how much
that is worth, and the program has an argument for each:

1. **The scale.** A replica of the example holds four of the agents'
   contexts, so TR mode runs few at a time. The paper's node runs a
   GLM-4.6-class model in FP8 on 8×H100. With a BF16 KV of 377 KB a token
   it has room for about 500 000 tokens, dozens of 20 000-token contexts
   (Fig. 3, p. 5). The `paper` preset is that node, twice over: `blocks`
   31 250, `a` 1.6e-5 s (64 GFLOP a token at 4 PFLOP/s, about half the
   node's BF16 peak), `omega` 15 ms, `beta` 14 ns a context token.
2. **The cost of a recompute.** The example's prefill costs `a` per token,
   whatever the context. The paper's argument is that a recompute costs
   the square of the context (Eq. 8, p. 7), because each token attends to
   all those before it. `g` is that attention, per token pair:
   `compute(batch.tokens) + g * batch.attention`. At 4.5 MFLOP a pair
   (92 layers, 96 heads of 128), it is 1.1 ns at the preset's rate and
   3.3 ns at a third of it. The two bound a range; neither is measured.
3. **A system prompt every program shares.** An agent framework starts
   every program with the same system prompt and tool descriptions. vLLM
   caches it once, and every request reuses it. ThunderAgent's code counts
   it in every program's tokens (`count_shared` 1), so with k programs on a
   replica its count is (k − 1) × S above what vLLM holds. The wiki's
   capacity subtracts it (`count_shared` 0), but the code never computes
   that subtraction. `S` is its length: the replica keeps its blocks
   resident (`kv cap blocks * bs - S`), a request holds and caches only its
   own tokens beyond it, every request reads and attends to it, and its
   tokens count as hits.
4. **The tool time.** While a program calls a tool its KV is held and the
   GPU does not run it. The longer the call, the fewer programs TR mode
   keeps running for the same KV.

Runs are 25 000 s long with 5 000 s of warm-up, four seeds a row. A
shorter run measures the agents before the longest of them have grown,
and reads default mode about a sixth too fast at the paper's scale. The
seeds' ratios at one point spread over up to 0.5 (2.93-3.40 at N = 96),
about 0.2 of standard deviation, so differences below that are noise. At the paper's scale (`tools/thunderagent/sweep.py --preset
paper`), one factor at a time:

| Setting | ratio at N = 96, 144, 192 | TR hit | default hit |
|---|---|---|---|
| preset: `g` 3.3e-9, `S` 3 968 | 3.09, 3.10, 3.09 | 0.90-0.91 | 0.23-0.24 |
| `S` 0 | 2.20, 2.19, 2.20 | 0.68 | 0.01 |
| `S` 2 048 | 3.00, 2.97, 2.97 | 0.86 | 0.12-0.13 |
| `count_shared` 0 | 2.16, 2.19, 2.20 | 0.73-0.74 | 0.23-0.24 |
| `g` 0 | 1.18, 1.21, 1.21 | 0.90-0.91 | 0.23-0.28 |
| `g` 1.1e-9 | 1.88, 1.91, 1.91 | 0.90-0.91 | 0.23-0.26 |
| `Z` 1 s | 3.33, 3.31, 3.28 | 0.90 | 0.23-0.24 |
| `Z` 10 s | 2.00, 2.13, 2.12 | 0.89 | 0.23-0.30 |

The preset in full:

| N | TR steps/min | default steps/min | ratio | TR hit | default hit | TR resumed/turn |
|---|---|---|---|---|---|---|
| 24 | 170.9 | 170.9 | 1.00 | 0.94 | 0.94 | 0.000 |
| 48 | 294.9 | 285.7 | 1.03 | 0.94 | 0.93 | 0.013 |
| 96 | 310.3 | 100.6 | 3.09 | 0.91 | 0.24 | 0.151 |
| 144 | 308.3 | 99.5 | 3.10 | 0.91 | 0.23 | 0.151 |
| 192 | 306.7 | 99.4 | 3.09 | 0.90 | 0.23 | 0.149 |

Beside Fig. 4a and Fig. 5a (SWE-agent on GLM-4.6), whose vLLM serves 218,
275, 162 and 140 steps/min and ThunderAgent 220, 350, 382 and 370 at 24-96
programs, with hit rates falling to 30 % and 93 %:

- **The shape** comes with the scale. The two modes serve alike while the
  contexts fit. Past the knee TR mode's throughput levels off, and
  default mode's falls to a third and stays there.
- **The cost of a recompute sets how much the knee costs.** Without `g`,
  default mode loses a tenth past the knee and the ratio is 1.2. With
  `g` it loses two thirds (286 → 101 at 3.3 ns), as the paper's vLLM
  loses half to two thirds (a: 275 → 140, b: 152 → 52, e: 172 → 70).
- **The router's overcount of the shared prompt keeps paused prefixes,
  and it raises the ratio too.** With the prompt counted once
  (`count_shared` 0) a paused program comes back to a cache that has
  evicted it: TR mode's hit rate is 0.73 and the ratio 2.2. Counted per
  program, the (k − 1) × S tokens the router never lets in are KV that
  holds paused prefixes: the hit rate is 0.91, the paper's 93-96 %, and the
  ratio 3.1, all of what the shared prompt adds: with S = 0 the ratio is
  the same 2.2. Counted once, the share of turns from paused programs is
  1.4 times as large (0.21 against 0.15), and each recomputes: prefill is
  2.9 times as many tokens a turn.
- **Default mode's hits are the shared prompt.** Its own prefix survives
  in 1.5-4.5 % of turns. The rest is S over the mean prompt, 3 968 / 18 000
  = 0.22, which S was chosen to bring near the paper's 30-38 %; the paper
  does not state its prompts' length.
- **The tool time bounds the gain.** With 10 s tools the ratio is 2.0-2.1;
  with 1 s tools it is 3.3, within the seeds' spread of the preset's 3.1,
  so the 10 s case is the evidence. With 10 s tools TR mode's decode batch
  falls from 20 to 12 while default mode is unchanged. The paper's smallest gains are on its workloads with
  the longest and most variable tools, ScienceAgent (1.24) and
  ToolOrchestra (1.48); ScienceAgent runs OpenHands on GLM-4.6 as the
  coding agents do, and ToolOrchestra runs Qwen3-8B on an RTX 5090.
- **The headline ratio rests on two estimates.** 3.1 is with `g` at the
  pessimistic end of its range and `S` chosen; with `g` 1.1 ns it is 1.9.
  The paper's ratios are 2.43-3.58 on the coding agents. The program
  shows which mechanisms produce a gain of that size, not that it
  reproduces a panel.
- **The absolute steps/min** are within a fifth of Fig. 4a's for TR mode
  (307-310 against 370-382) and about a third below for default mode
  (99-101 against 140-162), with `g` at 3.3 ns; at 1.1 ns default mode is
  above the paper's (183-187). The preset's constants are estimates, so
  this agreement is not a check.
- **A few trajectories end at a preemption.** At the example's scale a
  preempted request that has grown past what the replica's KV holds
  cannot be let in again and its session ends (the replica pool's `rej`,
  1-3 a run); at the paper's scale none does.

What the program does not reproduce:

- **A panel.** The paper does not state its KV capacity, its context
  lengths, its output lengths, its coding tools' times, its system
  prompts' length or the vLLM version, so the program cannot be fitted
  to one.
- **Sharing beyond one prompt.** `S` is one prefix every program shares
  and every replica keeps. Programs that share part of a task, or a
  prompt a replica may evict, need a shared prefix cache
  (`shared-prefix-cache.md`); serQ's cache is per session otherwise.

## What the program still does not write

1. **Overflow between ticks.** ThunderAgent lets a replica go over capacity
   and pauses at its next tick, every 5 (`Δt`, unit not stated in the
   paper, seconds in the code: [`__main__.py#L28`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/__main__.py#L28)).
   serQ pauses at the growth (above, *Rejected on the way*).
2. **Paused programs resumed early.** ThunderAgent may resume a paused
   program before its next request, at the lowest priority. The program
   lets it in at its next request (above).
3. **One queue, placed at resume.** A program waits in ThunderAgent's one
   global queue and is placed when it is resumed. A serQ hold picks its
   member before it joins that member's queue. A new program in
   ThunderAgent also waits whenever that queue is not empty, room or no
   room
   ([`scheduler/router.py#L344-L346`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L344-L346)).
4. **Skipping a program that does not fit.** The resume selection skips the
   programs whose tokens no longer fit; the head of a serQ pool's queue
   blocks the rest.
5. **A marked program counted until its response.** ThunderAgent keeps a
   marked REASONING program's tokens counted until its response arrives;
   the program's lease ends at once.
6. **Token estimates.** ThunderAgent estimates a request's tokens from the
   payload's characters by a ratio it learns
   ([`scheduler/router.py#L479-L491`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L479-L491))
   and counts the answer as it streams. The program counts the exact
   prompt, and the answer when the response has arrived, since its length
   is `hidden` until then.
7. **Acting weight and decay.** A weight below 1 is a second, smaller
   lease. A decay `2^-t` is an allocation that shrinks with time, which no
   allocation is. The paper's ablation of the decay base moves throughput
   by under 13 % (Fig. 6b: 373, 382, 360, 333 steps/min for x = 2, 4, 6,
   8).
8. **The force-resume.** ThunderAgent lets a program in after 30 minutes
   of waiting whatever the capacity, onto its replica or the least loaded
   ([`scheduler/router.py#L934-L956`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L934-L956)).
   The program puts such a program first in the queue, within the
   capacity; without that, the smaller-first order kept large paused
   programs waiting for over an hour. It ends a trajectory whose prompt
   the replica's KV cannot hold, as vLLM refuses a prompt of
   `max_model_len` tokens, which is at most the KV.
9. **Weight sync**, the RL trainer's pause of every request while it
   broadcasts weights, and the tool lifecycle (sandbox garbage collection,
   p. 8), are not part of the serving deployment the program writes.
