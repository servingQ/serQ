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
| Over capacity, pause the ACTING programs, the smallest first, then the REASONING ones ([`scheduler/router.py#L773-L805`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L773-L805)) | `preempt leases by (reasoning, prompt);` on `prog`: the growth ends idle programs' leases first, the smaller first |
| A paused program's next request waits to be let in again, on any replica with room ([`scheduler/router.py#L411-L424`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L411-L424), [`#L807-L932`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L807-L932)) | `allocated(prog[home])` is 0: the program is placed as a new one, and may move |
| Resume order: programs with a pending request, then new ones, each group smallest first ([`scheduler/router.py#L851-L866`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L851-L866)) | `queue by (turn_no == 0, prompt)` on `prog` |
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
- **`held(p)`** as the function's name: `held` is already the keyword of
  `reserve held`, so the name would mean two things. It is `allocated(p)`.

## What the program shows, against the paper

The program runs on two replicas of 64 000 tokens each. A task's first
prompt is 4 000-8 000 tokens, and each step adds about 1 000 tokens of
observation and 300 of answer. The model's constants are of the order of
a 32B model on one GPU (an iteration of 20 ms), not a fit. Each row is
three seeds, with a horizon of 3 000 s and a warm-up of 500 s. Steps/min
are turns per minute after warm-up, the paper's unit (a reasoning and an
acting period, p. 9). The hit rate is reused tokens over prompt tokens,
vLLM's `prefix_cache_hits / prefix_cache_queries`:

| N | TR steps/min | default steps/min | ratio | TR hit | default hit |
|---|---|---|---|---|---|
| 4 | 20.9 | 20.8 | 1.00 | 0.90 | 0.88 |
| 8 | 29.4 | 24.9 | 1.18 | 0.85 | 0.54 |
| 12 | 30.2 | 21.6 | 1.40 | 0.82 | 0.16 |
| 16 | 32.4 | 20.4 | 1.59 | 0.79 | 0.03 |
| 24 | 33.6 | 20.7 | 1.63 | 0.76 | 0.02 |
| 32 | 33.3 | 21.8 | 1.53 | 0.76 | 0.02 |
| 48 | 35.4 | 22.4 | 1.58 | 0.72 | 0.02 |

The paper's Fig. 4 and Fig. 5 (p. 9-10), with values read from the
figures except the printed ratios:

| Panel | N | vLLM steps/min | ThunderAgent steps/min | ratio at the largest N | hit, vLLM | hit, ThunderAgent |
|---|---|---|---|---|---|---|
| (a) SWE-agent, GLM-4.6 | 24-96 | 218 → 140 | 220 → 370 | 2.65 | 98 → 30 % | 98 → 93 % |
| (b) OpenHands, GLM-4.6 | 24-96 | 152 → 52 | 155 → 186 | 3.58 | 98 → 32 % | 98 → 94 % |
| (c) ToolOrchestra, Qwen3-8B | 24-48 | 5.05 → 4.75 | 6.05 → 7.05 | 1.48 | 13 → 12 % | 28 → 18 % |
| (d) SWE-agent, Qwen3-235B | 144-192 | 83 → 125 | 333 → 378 | 3.02 | 45 → 73 % | 96 % |
| (e) OpenHands, Qwen3-235B | 24-96 | 162 → 70 | 164 → 168 | 2.43 | 97 → 38 % | 97 → 96 % |
| (f) ScienceAgent, GLM-4.6 | 48-120 | 39.5 → 40 | 39.5 → 49.5 | 1.24 | 51 % | 52 → 60 % |

What agrees:

- **The shape.** Both systems serve alike while the contexts fit. Past
  that, the baseline's hit rate falls and its throughput stops growing,
  while ThunderAgent's throughput levels off at a higher value.
- **The ratio.** Ours is 1.53-1.63 at 16 agents and more. The paper's is
  1.24-3.58 at its largest N, so ours is at the low end, near the
  SWE-agent and ToolOrchestra panels.
- **The pause is what earns it.** Without the pause (the approximation
  above), the ratio was 1.22.

What does not, and why:

- **The hit rates are lower on both sides.** The paper's are ≈ 93-96 % for
  ThunderAgent and ≈ 30 % for vLLM. Ours are 0.72-0.79 and 0.02. The
  paper's programs share a system prompt (p. 7: "the shared prompt across
  programs"). serQ's cache is per session (`docs/language.md`, Known
  limitations; `shared-prefix-cache.md`), so a first turn has nothing
  cached, and nothing is shared between programs. ThunderAgent's own
  number also falls in ours as N grows: more programs are paused, and a
  paused one comes back to a cache that no longer holds it. The 100-token
  buffer does not cover a 300-token answer either, so the replica's LRU
  evicts a little more than the router counts.
- **Our baseline does not collapse.** The paper's vLLM loses up to two
  thirds of its throughput as N grows (b: 152 → 52), with decode
  dominating its latency (Fig. 6a, p. 11: 54 s of 58). Ours stays at 21-25
  steps/min. Dropping vLLM's full-prompt reservation, so that more
  requests are admitted and preempted while decoding, did not make it
  collapse either (22 steps/min at 48 agents). The paper does not state
  its vLLM version, its KV capacity, its context lengths or its node count
  per panel, so the cause cannot be checked from it.
- **The absolute numbers are not comparable.** The paper runs GLM-4.6 and
  Qwen3-235B in FP8 on 8×H100 nodes. The program's two replicas, their
  capacity and the agents' contexts are choices. The ratio and the shape
  are the comparison; the steps/min are not.

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
   and counts the answer once it arrives. The program counts the exact
   prompt, and the answer from the next turn, since its length is
   `hidden`.
7. **Acting weight and decay.** A weight below 1 is a second, smaller
   lease. A decay `2^-t` is an allocation that shrinks with time, which no
   allocation is. The paper's ablation of the decay base moves throughput
   by under 13 % (Fig. 6b: 373, 382, 360, 333 steps/min for x = 2, 4, 6,
   8).
8. **A program larger than a replica.** ThunderAgent force-resumes after
   30 minutes
   ([`scheduler/router.py#L934-L956`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L934-L956));
   and a waiting program longer than any replica can wait that long. The
   program ends a trajectory whose prompt the replica's KV cannot hold, as
   vLLM refuses a prompt of `max_model_len` tokens, which is at most the
   KV; the force-resume of a program that fits is not written.
9. **Weight sync**, the RL trainer's pause of every request while it
   broadcasts weights, and the tool lifecycle (sandbox garbage collection,
   p. 8), are not part of the serving deployment the program writes.
