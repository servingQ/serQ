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
| A replica's capacity, `cache_config.total_tokens_capacity`, against `active_program_tokens + buffer` ([`backend/state.py#L185-L194`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L185-L194)) | `pool prog[J] { cap tr ? room : inf; … }`, one member per replica, `room` the replica's `kv` less the shared prompt when it is counted once |
| `BUFFER_PER_PROGRAM = 100` per active program ([`backend/state.py#L23`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L23)) | `let buffer = 100;`, in every amount of `prog` |
| The scheduler loop: every `--scheduler-interval`, 5 s by default, it lets waiting programs in, then pauses ([`__main__.py#L28`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/__main__.py#L28), [`scheduler/router.py#L746-L771`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L746-L771)) | `stage tick : delay;` and `next_tick(order)`: a wait until a millisecond before the next tick, at the program's place in the router's order |
| A new program goes to the replica with the fewest active tokens among those where it fits, if no program waits; else it waits in the global queue ([`scheduler/router.py#L330-L362`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L330-L362)) | `choose j in J by (free(prog[j]) >= charged ? 0 : 1, used(prog[j]))`, then a `hold` of `prog[j]`, unless `used(waiting_queue) > 0` or no replica has room |
| The global waiting queue ([`scheduler/router.py#L560-L571`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L560-L571)) | `pool waiting_queue`, which a program holds while it waits for ticks |
| The program's tokens are counted while it reasons and while it calls a tool, at `acting_token_weight` 1.0, the default ([`backend/state.py#L104-L107`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L104-L107), [`__main__.py#L30`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/__main__.py#L30)) | `lease prog[home] (inf)`: the allocation outlives the hold |
| A running program's next request is let through without a capacity check; its count becomes the new prompt's estimate, its characters over a ratio learnt from earlier responses ([`scheduler/router.py#L384-L389`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L384-L389), [`#L450-L458`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L450-L458), [`#L479-L491`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L479-L491)) | `charged`, the prompt off by at most `est` (0: exact), to which the lease grows or shrinks |
| After the response, the count is the response's usage, the answer included ([`scheduler/router.py#L493-L494`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L493-L494)) | the lease grows or shrinks to `prompt + o + buffer` after the engine's request |
| An ACTING program counts `acting_token_weight` of its tokens ([`backend/state.py#L104-L107`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L104-L107)) | `shrink prog[home] (cost(prog, (1 - w) * (allocated(prog[home]) - buffer)))` after the response, grown back at the next request |
| Over capacity, pause the ACTING programs, the smallest first, then the REASONING ones ([`scheduler/router.py#L773-L805`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L773-L805)) | `preempt leases by (reasoning, prompt) every (dt) suspend;` on `prog`: a growth goes through, and at the tick idle programs' leases are set aside first, the smaller first |
| A paused program with no request pending is let in again at a later tick if it fits, last of the groups, smallest first ([`scheduler/router.py#L851-L866`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L851-L866)) | `suspend`: the tick gives a set-aside lease back, before it pauses, if it fits |
| A paused program's next request waits for a tick that lets it in, on the replica with the most room if one has room for it, wherever it was before ([`scheduler/router.py#L411-L424`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L411-L424), [`#L807-L932`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L807-L932)) | `allocated(prog[home])` is 0: the program releases what is set aside, waits for the next tick, then for the next while `no_room(charged)`, and is placed as a new one |
| A program that has waited 30 minutes is let in whatever the capacity; at a tick, programs with a pending request, then new ones, each group smallest first ([`#L934-L956`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L934-L956), [`scheduler/router.py#L851-L866`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L851-L866)) | after 1 800 s the program stops waiting for ticks and is let in on the replica it chooses, past the capacity if it must; programs back at one tick wake in that order, with a step first, then new ones, the smaller first in each |
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
  than writing the early resume this way. `suspend` writes it without a
  second queue (*The router's tick*).
- **A slack argument instead of the shared prompt.** A `slack`, the share
  of the KV the router's count leaves unused, moves TR mode's hit rate as
  the shared prompt does. It says what the prompt does rather than what it
  is, and it leaves default mode without the prompt's hits. `S` writes the
  prompt, and both effects follow from it.
- **`held(p)`** as the function's name: `held` is already the keyword of
  `reserve held`, so the name would mean two things. It is `allocated(p)`.

## The router's tick

ThunderAgent's router is a loop that wakes every `--scheduler-interval`,
5 s by default ([`scheduler/router.py#L746-L771`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L746-L771)).
At each tick it lets waiting programs in, those with a request pending
first, then new ones, then paused ones that have none, the smaller first
in each, and then pauses on every replica that is over its capacity.
Between ticks a running program's request goes through unchecked, and a
replica's count may go over. Two parts write it: the waiting is the
program's, and the pause and the early resume are the pool's.

**The waiting.** A paused program, and a new one when another waits or
no replica has room for it, waits for the next tick on a delay stage,
and at each tick goes in if a replica has room for it, on the one with
the most room, wherever it was before; else it waits for the next. The
global queue is a pool its waiters hold:

```serq
  stage tick : delay;
  pool waiting_queue { }
  def next_tick(order) {
    run tick (cost(tick, floor(now + 1e-3, dt) + dt - 1e-3 + order - now));
  }
  def no_room(x) { max r in J (free(prog[r])) < x }
  …
    } else {
      // paused and not let in again since: what it had is the router's
      if (K > 0) { release prog[home]; }
      set t_q = now;
      // a paused program, or a new one behind others or with no room
      set waits = tr && (K > 0 || used(waiting_queue) > 0 || no_room(charged));
      if (waits) {
        set order = (K > 0 ? 0 : 1e-4) + 1e-10 * charged;
        hold waiting_queue (cost(waiting_queue, 1)) {
          next_tick(order);
          while (no_room(charged) && now - t_q < force_resume) { run tick (cost(tick, dt)); }
        }
      }
      choose j in J by (…);
      set home = j;
      hold prog[home] (cost(prog, tr ? buffer : charged)) { … } lease prog[home] (inf);
      if (tr) { count_as(charged); }
```

A program let in enters with its buffer and grows the rest: within the
room it was let in for, or, once it has waited 30 minutes, past the
capacity, as `_wait_for_resume` lets it in whatever the room; the next
tick then pauses others.

Every waiter wakes a millisecond before the tick, at an instant that
puts it in the router's order: those with a step before new ones, the
smaller first, microseconds apart. Each takes a replica only if one has
room for it, so a program that does not fit leaves the room to those
behind it that do, as the resume's selection skips it.

**The pause and the early resume.** The pool's own rule takes the rest:

```serq
  pool prog[J] {
    cap tr ? room : inf;
    preempt leases by (reasoning, prompt) every (dt) suspend;
  }
```

`every (dt)` lets a lease's growth through at once, past the cap if it
must, and at each multiple of `dt` takes leases in the keys' order while
the pool is over its cap: ThunderAgent's pause, at its tick. `suspend`
sets a taken lease aside instead of ending it, and at each later tick,
before it takes any, gives set-aside leases back in the same order while
they fit: the resume of a paused program that has no request pending,
the last of `_greedy_resume`'s groups. The program finds its lease back
(`allocated(prog[home]) > 0`) and goes on as a running one; a lease still
set aside at its next request it releases, and waits.

### The price

`Preempt::Leases` gains two fields a reader may ignore when absent:
`every`, a number of seconds, and `suspend`. IR 13 is untagged, so
nothing bumps, and they go in the coming tag's message with the rest of
13. The Lean generator already raises `Fragment` on `Leases`; no oracle
program uses either. No other program's IR moves: both are absent unless
written. `suspend` without `every`, or `every` on a pool with `reserve
held`, does not link (`tests/lease_ticks.rs`).

### Rejected on the way

- **`suspend` refused on a pool with `block`.** The first version refused
  it there, since a lease set aside kept its lock on the cached prefix it
  had read, which no allocation then held. A pool without `block` caches
  too (a session's own slot), so the rule was the wrong condition (#591's
  review). A lease set aside now lets the prefix go, as a release does,
  and the rule went.

- **A timer statement.** A `every (dt) { … }` block the program writes
  would run the router's loop as its own actor. It would need statements
  that act on other sessions' leases, a new kind of node every consumer
  learns, and the pause rule would live in two places, the pool's
  `preempt leases by` and the loop. The pool already has the victim
  order; a period on it is a mechanism the program selects.
- **The early resume as a second queue.** A paused program waiting in the
  pool's queue while it calls its tool would wait in two places at once
  (above, *A revoked lease waiting in the queue*). Under `suspend` the
  session waits nowhere: the pool keeps the lease aside and gives it
  back; the session only finds it again.
- **The waiters' order as a queue key.** The waiters check for room
  themselves at each tick, so a `queue by` on `prog` orders only those
  that waited past the force-resume. Their instants within the tick are
  what orders them, and they are the program's.

## What the program shows, against the paper

`tools/thunderagent/sweep.py` runs both modes over the agent counts, four
seeds each by default, with a horizon of 25 000 s and a warm-up of 5 000 s.
Steps/min are turns per minute after warm-up, the paper's unit (a
reasoning and an acting period, p. 9). The hit rate is reused tokens over
prompt tokens, vLLM's `prefix_cache_hits / prefix_cache_queries`.
"Resumed" is the share of TR mode's turns that a program still paused at
its request sends; one a tick gave back before it is not counted.

At the example's own scale (`tools/thunderagent/sweep.py`), each replica
holds 64 000 tokens. A task's first prompt is 4 000-8 000 tokens, and each
step adds about 1 000 tokens of observation and 300 of answer:

| N | TR steps/min | default steps/min | ratio | TR hit | default hit | TR resumed/turn |
|---|---|---|---|---|---|---|
| 4 | 21.4 | 21.1 | 1.01 | 0.90 | 0.88 | 0.007 |
| 8 | 28.9 | 24.1 | 1.20 | 0.84 | 0.53 | 0.059 |
| 16 | 28.7 | 18.7 | 1.53 | 0.78 | 0.02 | 0.111 |
| 32 | 27.6 | 18.6 | 1.48 | 0.71 | 0.01 | 0.161 |
| 48 | 26.8 | 18.6 | 1.44 | 0.65 | 0.02 | 0.202 |

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
(seed 1), TR mode's mean time to first token is 55 s against default
mode's 81 s, but its 99th percentile is 1 817 s against 140 s (1 817-1 818 s
over seeds 1-4): a program the router holds back waits for room, and
since the router lets the smaller in first, the largest wait until the
force-resume lets them in at 30 minutes: 14 % of the resumes (2 % of
the turns, seed 1) wait exactly that long, programs with prompts 2.7
times the others'. Without the size in the order (those with a step
still first) the 99th percentile is 639-788 s over seeds 1-4, and with
the waiters in the order they reach the tick 409-486 s, at 1.5 % fewer
steps/min. Under open Poisson arrivals of
programs (the same program with `arrive poisson`), at rates default mode
keeps up with, 0.01-0.02 a second, the two serve the same turns and TR
mode's TTFT is a little lower (0.46 s against 1.05 s at 0.02). At 0.04 a
second default mode falls behind and its queue grows with the run (TTFT
126-216 s at 25 000 s, 243-449 s at 50 000 s, seeds 1-2), while TR mode
is at the edge of its limit: its TTFT is low but still grows with the run
(2-5 s, 4-7 s in the longer run). At 0.05 neither keeps up: the turns offered, 30 a minute, are more
than TR mode serves at its knee.

The example's ratio, 1.4-1.5, is below the paper's, and its hit rates are
well below the paper's on both sides. The sections below find
the reasons, one at a time.

### Why the numbers differ

Write the throughput by Little's law: steps/min = agents × 60 / (time a
step takes, its wait included). At 32 agents on the example's scale (seed
1), TR mode's step is 68 s: 65 s from request to answer, of which 51 s
is the wait for the router's ticks (24 of the 32 agents wait for one at
any moment, Little's law at 0.47 turns a second), 14 s at the replica
and its queues, and 3 s at the tool. A sixth of the turns come from a paused program.
Default mode's is 99 s: 96 s
at the replica, most of it waiting for blocks behind the requests that
recompute, and 3 s at the tool. The replicas run about as many requests
at once either way (a decode batch of 2.7 against 3.0); what differs is
what each request recomputes, 5 000 prompt tokens in TR mode against
16 000 in default mode. Four things decide how much
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
   that subtraction. `S` is its length. The replica's request names its
   prompt as two segments, `prefix (system: S, serial: inf)`: the system
   prompt every program shares, then the program's own context
   (`examples/multi-turn/vllm_system_prompt.sq`). Its cache keeps the
   first once, allocated once however many requests read it (vLLM's
   `ref_cnt`), from the first request on a replica that ends: serQ's
   cache takes a hold's blocks when it ends, vLLM's when it allocates
   them, so the requests a replica admits before then each compute their
   own copy (a dozen in the first second of a run, inside the warm-up).
   Once cached it is never evicted here: every request's chain starts
   with it, an admission reads it before it evicts, and the cache evicts
   a chain's last blocks first, so it outlives every context cached
   after it (no request missed it in any run of the tables below, nor
   with `blocks` 1 200). It behaves as the constant prefix it stood for
   before the cache was shared.
4. **The tool time.** While a program calls a tool its KV is held and the
   GPU does not run it. The longer the call, the fewer programs TR mode
   keeps running for the same KV.

Runs are 25 000 s long with 5 000 s of warm-up, four seeds a row. A
shorter run measures the agents before the longest of them have grown,
and reads default mode about a sixth too fast at the paper's scale. The
seeds' ratios at one point spread over up to 0.5 (3.01-3.52 at N = 96),
about 0.2 of standard deviation, but that is default mode's spread (6 %),
and every row runs the same seeds: default mode's runs are the same in
every row that does not change it, so rows are compared seed by seed,
where TR mode's spread is about 1 %. At the paper's scale (`tools/thunderagent/sweep.py --preset
paper`), one factor at a time:

| Setting | ratio at N = 96, 144, 192 | TR hit | default hit |
|---|---|---|---|
| preset: `g` 3.3e-9, `S` 3 968 | 3.18, 3.21, 3.21 | 0.93 | 0.23-0.24 |
| `S` 0 | 2.40, 2.38, 2.34 | 0.73-0.74 | 0.01 |
| `S` 2 048 | 3.21, 3.19, 3.18 | 0.90 | 0.12-0.13 |
| `count_shared` 0 | 2.29, 2.26, 2.23 | 0.74-0.77 | 0.23-0.24 |
| `g` 0 | 1.18, 1.20, 1.20 | 0.93 | 0.23-0.28 |
| `g` 1.1e-9 | 1.89, 1.93, 1.93 | 0.93 | 0.23-0.26 |
| `Z` 1 s | 3.56, 3.56, 3.55 | 0.93 | 0.23-0.24 |
| `Z` 10 s | 2.01, 2.12, 2.12 | 0.91-0.92 | 0.23-0.29 |
| `w` 0.9 | 3.21, 3.24, 3.24 | 0.93 | 0.23-0.24 |
| `w` 0.75 | 3.19, 3.22, 3.21 | 0.91-0.92 | 0.23-0.24 |
| `w` 0.5 | 2.64, 2.63, 2.64 | 0.84 | 0.23-0.24 |
| `w` 0 | 1.29, 1.29, 1.29 | 0.43-0.45 | 0.23-0.24 |
| `dt` 0.05 s | 3.03, 3.01, 2.96 | 0.88-0.90 | 0.23-0.24 |
| `dt` 2 s | 3.17, 3.19, 3.19 | 0.92 | 0.23-0.24 |
| `dt` 10 s | 3.17, 3.20, 3.19 | 0.93 | 0.23-0.24 |
| `est` 0.2 | 3.17, 3.19, 3.19 | 0.93 | 0.23-0.24 |

The preset in full:

| N | TR steps/min | default steps/min | ratio | TR hit | default hit | TR resumed/turn |
|---|---|---|---|---|---|---|
| 24 | 170.8 | 170.9 | 1.00 | 0.94 | 0.94 | 0.000 |
| 48 | 294.6 | 284.7 | 1.03 | 0.94 | 0.93 | 0.006 |
| 96 | 319.6 | 100.6 | 3.18 | 0.93 | 0.24 | 0.053 |
| 144 | 319.3 | 99.5 | 3.21 | 0.93 | 0.23 | 0.057 |
| 192 | 318.8 | 99.4 | 3.21 | 0.93 | 0.23 | 0.062 |

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
  evicted it: TR mode's hit rate is 0.74-0.77 and the ratio 2.2-2.3. Counted
  per program, the (k − 1) × S tokens the router never lets in are KV that
  holds paused prefixes: the hit rate is 0.93, the paper's 93-96 %, and the
  ratio 3.2, all of what the shared prompt adds: with S = 0 the ratio is
  about the same, 2.4. Counted once, the share of turns from paused
  programs is twice as large (0.11 against 0.05), and each recomputes:
  prefill is 3.3 times as many tokens a turn.
- **Default mode's hits are the shared prompt.** Its own prefix survives
  in 1.5-4.5 % of turns. The rest is S over the mean prompt, 3 968 / 18 000
  = 0.22, which S was chosen to bring near the paper's 30-38 %; the paper
  does not state its prompts' length.
- **The tool time bounds the gain.** With 10 s tools the ratio is 2.0-2.1;
  with 1 s tools it is 3.6, above the preset's 3.2 on every seed (TR mode
  11 % faster). With 10 s tools TR mode's decode batch falls from 16 to 11
  while default mode's stays at 31-34. The paper's smallest gains are on its workloads with
  the longest and most variable tools, ScienceAgent (1.24) and
  ToolOrchestra (1.48); ScienceAgent runs OpenHands on GLM-4.6 as the
  coding agents do, and ToolOrchestra runs Qwen3-8B on an RTX 5090.
- **The headline ratio rests on two estimates.** 3.2 is with `g` at the
  pessimistic end of its range and `S` chosen; with `g` 1.1 ns it is 1.9.
  The paper's ratios are 2.43-3.58 on the coding agents. The program
  shows which mechanisms produce a gain of that size, not that it
  reproduces a panel.
- **The tick's interval, against the paper's ablation, does not agree.**
  With `dt` 2, 5 and 10 s TR mode serves 317-318, 319-320 and 318-319
  steps/min: flat, within 0.6 % seed by seed, though a paused program sends
  fewer of the turns the longer the tick (0.10, 0.06 and 0.03), since a
  longer tick lets the pool's overflow and its early resume stand longer.
  Fig. 6b (p. 11, read from the figure) gives 363, 378, 370 and 343 for
  Δt = 2, 5, 7 and 10: a rise of 4 % and a fall of 9 %, neither of which
  the program shows. The paper's ablation runs with its decay, which the
  program does not write (item 4). At 50 ms, a waiting program is let in
  as soon as room appears, wherever it appears: a fifth of the turns come
  from paused programs, against a twentieth at 5 s, and the ratio is
  3.0: TR mode 5-8 % slower than at 5 s, on every seed.
- **The router's estimate costs nothing measurable.** With the estimate
  off by up to 20 % either way per request (`est` 0.2) the ratio is 3.2,
  as counted exactly.
- **The absolute steps/min** are within a sixth of Fig. 4a's for TR mode
  (319 against 370-382) and about a third below for default mode
  (99-101 against 140-162), with `g` at 3.3 ns; at 1.1 ns default mode is
  above the paper's (183-186). The preset's constants are estimates, so
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
- **Sharing beyond one prompt.** `S` is one prefix every program shares,
  which no replica evicts in these runs. Programs that share part of a
  task are not written.

## What the program still does not write

1. **The order at a tick.** ThunderAgent places the programs it lets in
   at one tick largest first on the replica with the most room; here each
   goes, in the router's order, to the replica with the most room when it
   reaches the tick. A new program in ThunderAgent waits while any program
   is in the global queue, a paused one calling its tool included
   ([`scheduler/router.py#L344-L346`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L344-L346));
   here, while one with a request pending waits for a tick: a lease the
   pool set aside holds no place in `waiting_queue`. And ThunderAgent
   resumes a paused program with no request pending, as every other, on
   the replica with the most room
   ([`scheduler/router.py#L907`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L907),
   [`#L692-L717`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L692-L717)); `suspend` gives
   the lease back on the replica it was taken from.
2. **A marked program counted until its response.** ThunderAgent keeps a
   marked REASONING program's tokens counted until its response arrives,
   and so marks every REASONING program on the replica once the ACTING
   ones are paused (above); the program's lease is set aside at once. A reasoning
   program's lease is taken in 0.02 % of the preset's turns at N = 96,
   1.4 % with `count_shared` 0 (2.3-2.8 % at N = 192) and 10 % at `w`
   0.5.
3. **The estimate's error.** ThunderAgent estimates a request's tokens from
   the payload's characters by a ratio it learns
   ([`scheduler/router.py#L479-L491`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L479-L491))
   and counts the answer as it streams. The program writes the estimate
   with an error drawn uniformly per request (`est`), and counts the
   answer when the response has arrived, since its length is `hidden`
   until then. How far a payload's characters per token stray from the
   learnt ratio, the paper does not state.
4. **Decay.** `--acting-token-weight` is written: `w` below 1 gives back
   (1 − w) of a program's tokens, not its buffer, when its response
   arrives (`shrink`), and its next request grows it back. ThunderAgent
   counts the weight wherever it reads a replica's room: in placing a new
   program, in pausing, in resuming
   ([`backend/state.py#L104-L107`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L104-L107)).
   Counting idle programs for less costs throughput here, from 0.5 down:
   at the paper preset the ratio is 3.2 at `w` 1, 0.9 and 0.75, 2.6 at
   0.5 and 1.3 at 0. The programs the router lets in do not increase; the
   churn does. Their next requests grow back what they gave and pause
   others: at 0.5 paused programs send a sixth of the turns (0.18 against
   0.05), about half of them resume on the other replica, as at 1, and
   recompute, and prefill is 2.2 times the tokens a turn.
   The code's default, 1, is within 1 % of the best of these, 0.9. The `2^-t` decay the
   paper's experiments use is not this: it weighs an acting program by the
   time it has called its tool, and only in the resume check
   ([`backend/state.py#L196-L214`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L196-L214),
   [`scheduler/router.py#L833-L837`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L833-L837)),
   while pausing and placing count it whole; it is off by default
   (`--use-acting-token-decay`, base 2; the paper's Fig. 6b also runs
   x = 4, 6, 8: 373, 382, 360, 333 steps/min). The example does not
   write it. The tick's check could read it (a replica's reasoning
   tokens, its acting ones whole for 1 / ln x seconds after a response,
   its buffers), but the pool gives a set-aside lease back only while it
   fits (`suspend`), and a decayed program counts for almost nothing, so
   ThunderAgent resumes every paused program and its pause trims the
   excess at the same tick. A variant that lowers TR mode instead of keeping it
   near 373 (#587) lets in more than the replica holds and pauses others
   at once. Whether resuming all and pausing the excess is what the
   paper's runs have and the variant lacks, no run here tells, so decay's
   effect cannot be read off the program.
5. **Weight sync**, the RL trainer's pause of every request while it
   broadcasts weights, and the tool lifecycle (sandbox garbage collection,
   p. 8), are not part of the serving deployment the program writes.
