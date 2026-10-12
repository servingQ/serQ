# ThunderAgent as a serQ program

[ThunderAgent](https://github.com/ThunderAgent-org/ThunderAgent) is a proxy
in front of vLLM or SGLang replicas that schedules *programs*, not requests
(arXiv 2602.13692). A program is an agent's whole trajectory, named by the
`program_id` the client adds to every request. The router places a program
on a replica at its first request and keeps it there. The program's tokens
count against that replica's KV capacity while it waits for a tool, as
well as while it reasons. When a replica is over capacity, the router
pauses programs, and a paused program's next request waits until the
router resumes it. The claim is that a program back from its tool still
finds its prefix cached, where a request-level router lets the caches
thrash.

`examples/multi-turn/thunderagent.sq` writes the router in TR mode (the
default, `--router tr`) in front of two vLLM replicas. Each replica has
`examples/pd-disaggregation/llmd_nixl_pull.sq`'s local decoder, without
its `max_model_len` cap. The
program runs a fixed number of agents at once, as a rollout or a benchmark
run does. `--tr 0` is the router's `default` mode. This document gives
the correspondence, what the program shows and what it does not write. All
source references are to ThunderAgent at
[`7ddc861`](https://github.com/ThunderAgent-org/ThunderAgent/tree/7ddc8610270e56d3b109eed8796b3a4360fc67c9).

## The correspondence

| ThunderAgent | The program |
|---|---|
| A replica's capacity, `cache_config.total_tokens_capacity`, against `active_program_tokens + buffer` ([`backend/state.py#L185-L194`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L185-L194)) | `pool prog[J] { cap blocks * bs; … }`, one member per replica, the same tokens as the replica's `kv` |
| `BUFFER_PER_PROGRAM = 100` per active program ([`backend/state.py#L23`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L23)) | `let buffer = 100;`, added to every hold of `prog` |
| A new program goes to the replica with the fewest active tokens among those where it fits, and waits when none has room ([`scheduler/router.py#L330-L362`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L330-L362)) | `choose j in J by (free(prog[j]) >= prompt + buffer ? 0 : 1, used(prog[j]))` at the first turn, then `set home = j` |
| The program stays on its replica ([`scheduler/router.py#L450-L458`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L450-L458)) | every later turn holds `prog[home]` and calls `E[home]` |
| Before a request, the program's tokens are the prompt's estimate ([`scheduler/router.py#L387-L389`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L387-L389)) | `hold prog[home] (cost(prog, prompt + buffer))` |
| After the response the program is ACTING and still counted, at `acting_token_weight` 1.0, the default ([`backend/state.py#L104-L107`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L104-L107), [`__main__.py#L30`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/__main__.py#L30)) | `lease prog[home] (inf)`: the allocation outlives the turn, through the tool call |
| The next request replaces the count ([`scheduler/router.py#L387-L389`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L387-L389)) | `release prog[home];` then the turn's hold |
| Resume order: programs with a pending request, then new ones, each group smallest first ([`scheduler/router.py#L851-L866`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L851-L866)) | `queue by (turn_no == 0, prompt)` on `prog` |
| `/programs/release` at the end of the trajectory ([`scheduler/router.py#L514-L547`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L514-L547)) | the session's end ends the lease |
| `--router default`: the replica with the fewest programs, no capacity ([`scheduler/router.py#L252-L271`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L252-L271)) | `tr = 0`: `cap inf` and `choose … by (0, holders(prog[j]))` |

The capacity formula in the wiki subtracts `shared_tokens`, the prefix
cache's savings. At `7ddc861` nothing calls `update_shared_tokens`
([`backend/state.py#L129-L136`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L129-L136)
is its only occurrence), so `shared_tokens` stays 0. The program leaves it
out.

## What the program shows

The program runs 32 agents on two replicas of 64 000 tokens each. A
task's first prompt is 4 000-8 000 tokens, and each step adds about 1 000
tokens of observation and 300 of answer, so 32 contexts are several times
what the replicas hold. The model's constants are of the order of a 32B
model on one GPU (an iteration of 20 ms), not a fit. The runs use a
horizon of 3 000 s and a warm-up of 500 s:

```
serq run examples/multi-turn/thunderagent.sq --horizon 3000 --warmup 500 --seed S --tr T
```

| seed | turns, TR | turns, default | `hit`, TR | `hit`, default |
|---|---|---|---|---|
| 1 | 1069 | 861 | 0.52 | 0.015 |
| 2 | 1107 | 935 | 0.53 | 0.015 |
| 3 | 1097 | 868 | 0.56 | 0.018 |

In default mode every agent is let in, the caches thrash, and almost every
turn recomputes its whole context. In TR mode three to four agents per
replica are in at a time (the pool report's `holders` of `prog`, 3.6-4.2),
and the rest wait (`resume_wait`). The replicas serve about 22 % more
turns. With 4 agents, whose contexts fit, the two modes serve the same
turns (728 and 726 at seed 1, horizon 2 000 s). `tests/thunderagent.rs`
holds both.

The gain is a closed workload's, and it is paid in waiting. At seed 1
over 5 000-25 000 s, TR mode's mean time to first token is 62 s against
default mode's 81 s, but its 99th percentile is 1 760 s against 142 s:
a program the router holds back waits for room. Under open Poisson
arrivals of programs, at rates default mode keeps up with (0.01-0.02 a
second), the two serve the same turns and TR mode's TTFT is a little
lower (0.84 s against 1.05 s at 0.02). Above them default mode falls
behind and its queue grows without bound; TR mode keeps up, with a mean
TTFT of 85 s against 396 s at 0.05 and a 99th percentile of 2 098 s
against 648 s.

`hit` counts every turn, the first one included, which has nothing
cached. In TR mode some returning turns miss too. The replica's `kv`
holds a running request's answer and its own dead cache entries next to
the leased prefixes, and the router's 100-token buffer does not cover a
300-token answer, so the LRU evicts the prefix of a program that waits.

## What the program does not write

Each item is a mechanism of the router that the program approximates,
and the construct the language would need to write it.

1. **Pausing another program.** When a replica is over capacity,
   ThunderAgent pauses the ACTING programs on it, the smallest first. If
   that is not enough, it marks the REASONING ones, which it pauses when
   their response arrives
   ([`scheduler/router.py#L773-L805`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L773-L805)).
   A serQ lease is neither evictable nor a preemption victim, so no other
   program can take its tokens back. In the program, the program whose
   next turn does not fit waits, and the idle ones keep their place: the
   victim is the grower, not the smallest idle program. Writing it needs
   a lease that a pool may revoke, in an order of the program's (`preempt
   by` on leases), with the revoked session waiting at its next hold of
   the pool.
2. **Overflow between ticks.** ThunderAgent does not check capacity before
   a running program's request: it lets the replica go over capacity and
   corrects it at the next tick of its scheduler, every 5 s
   ([`scheduler/router.py#L746-L771`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L746-L771),
   [`__main__.py#L28`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/__main__.py#L28)).
   A serQ pool never goes over its cap, and it admits at the instant room
   appears. A periodic scheduler is a timer, which the language does not
   have: queue selection "sets no timer" (`docs/language.md`, Waiting
   selection).
3. **One queue, placed at resume.** A paused program waits in one global
   queue. It is placed at resume time by best-fit decreasing, on the
   replica with the most room, which may not be the one it came from
   ([`scheduler/router.py#L807-L932`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L807-L932)).
   A new program also waits whenever that queue is not empty, room or no
   room
   ([`scheduler/router.py#L344-L346`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L344-L346)).
   A serQ hold picks its member before it joins that member's queue, so a
   program waits on one replica, and one that fits a member is let in
   there whatever waits elsewhere. A new program in the program waits on
   the least-used replica, and a returning one on its own.
4. **Skipping a program that does not fit.** The resume selection takes,
   in priority order, every program whose tokens still fit the total, and
   skips the ones that do not. The head of a serQ pool's queue that does
   not fit blocks the rest.
5. **Token estimates.** ThunderAgent estimates a request's tokens from the
   payload's characters with a ratio it learns by momentum
   ([`scheduler/router.py#L479-L491`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L479-L491)).
   It replaces the estimate with the response's `total_tokens`, the answer
   included, once the response arrives. The program uses the prompt's
   exact length, and it counts the answer only from the next turn, since
   the answer's length is `hidden`.
6. **Acting weight and decay.** An `--acting-token-weight` below 1 counts
   an ACTING program for less. `--use-acting-token-decay` weighs its tokens
   by `2^-t` in the resume test
   ([`backend/state.py#L196-L214`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L196-L214)).
   A weight is a second hold, a smaller one, that leases. Decay is an
   allocation that shrinks with time, which a hold is not.
7. **A program larger than a replica.** ThunderAgent force-resumes a
   program after 30 minutes of waiting, onto its replica or, for a new
   one, the least-loaded
   ([`scheduler/router.py#L934-L956`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L934-L956)).
   In serQ, a hold that can never fit is refused, and its session ends
   (the pool report's `rej`).
8. **Weight sync**, the RL trainer's pause of every request while it
   broadcasts weights, is not part of an inference deployment, and the
   program leaves it out.

Items 1 to 3 are the router's mechanism. Item 1 is the one that decides
which program loses its cache, so it is the first construct to price.
