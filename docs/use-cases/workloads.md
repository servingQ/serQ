# Different workloads

These examples keep the same engine and vary the client: single requests,
chat, tool-using agents and an approximation of delegated subagent traffic.
The program is the deployment: a device, the engine `vllm` on it, its pools
and `server`. The workload, which supplies arrivals and turns, is a file of
its own given when the program runs (`serq run DEPLOYMENT --workload
WORKLOAD`); `examples/pairs.json` names each program's. Client delays (`tool`, `user`, `delegate`) represent time
outside the engine.

One deployment, `examples/multi-turn/vllm.sq`, runs under each of them:
`examples/pairs.json` lists six workloads for it, its own `chat.sq` first
(`serq run examples/multi-turn/vllm.sq --workload examples/workloads/single_turn.sq`).
The table compares the first four; the last two put a burst on the prefix
cache ([below](#bursts-and-the-prefix-cache)).
Its server hides the output length from scheduling expressions. See the
[vLLM use case](vllm.md) for the engine's validation scope.

| | single turn | multi-turn chat | multi-turn agent | agent with subagents |
|---|---|---|---|---|
| workload (`examples/workloads/`) | `single_turn.sq` | `human_chat.sq` | `chat.sq` | `subagents.sq` |
| a session is | one request | a conversation | a task | a task and the subtasks it hands out |
| carried to the next turn | nothing | the conversation, `K = K + n + o` | the same | the same |
| between turns | — | a person, `run user (cost(user, ~exp(Z)))` | a tool, `tool (~exp(Z))` | a tool, or waiting for the subagents |
| who makes arrivals | the environment | the environment | the environment | a Poisson approximation of parent and child traffic |
| the prefix cache | never read back | the last turn's prefix | the last turn's prefix | same, and a subagent misses the context it copied |

## Single turn

```serq title="examples/workloads/single_turn.sq"
--8<-- "examples/workloads/single_turn.sq:workload"
```

No `session` declaration is needed: each arrival makes one turn and then
leaves. `K` stays 0 because nothing carries over, so the `input: K + n`
the workload sends is just the new tokens. The engine's `cache
(request.input + request.output)` still caches the prefix when the request finishes, but no later
request of that session will read it, so here the cache only takes up space
until LRU evicts it. The hit rate is 0.

This does not model shared system prompts. vLLM identifies cached blocks
by content and prefix hashes (`kv_cache_utils.py:650-680`,
`block_pool.py:197-223`), so a later request can reuse a matching cached
prefix from another request. serQ's cache is per session; see
[limitations](../language.md#9-known-limitations).

## Multi-turn chat

```serq title="examples/workloads/human_chat.sq"
--8<-- "examples/workloads/human_chat.sq:workload"
```

Each turn sends the conversation `K` plus a new message of mean 100 tokens.
After the request, `set K = K + n + o` carries its input and output into the
next turn. `run user (cost(user, ~exp(Z)))` models reading and typing time on a delay
stage, with mean `Z = 20` seconds.

## Multi-turn agent

```serq title="examples/workloads/chat.sq"
--8<-- "examples/workloads/chat.sq:workload"
```

The agent has the same turn structure as chat, with a 3-second mean tool
delay and 500 mean new tokens per turn. Its initial prompt is a longer task
description. The workload determines these differences.

## Agent with subagents

This example approximates subagents as independent sessions. It does not
create child sessions in response to a parent's delegation or join their
actual completion times.

```serq title="examples/workloads/subagents.sq"
--8<-- "examples/workloads/subagents.sq:workload"
```

The approximation has three parts:

- **The children arrive on their own.** A parent's turn that continues
  delegates with probability `q` to `k` subagents. A parent continues
  `p / (1 − p)` times on average, so it creates
  `kids = q · k · p / (1 − p)` subagents. The program folds them into the
  one Poisson stream at rate `Lambda · (1 + kids)`, and marks an arrival as a
  subagent (`sub`) with probability `kids / (1 + kids)`.
- **The parent waits a constant.** A delegating turn runs `run delegate (cost(delegate, W))`.
  The supplied `W = 15.4` seconds is a model input; it does not track the
  simulated children's completion times. It was calibrated as the mean
  maximum of `k` `subagent` durations collected with `--dump`, iterated
  until it stopped moving (seed 1, 10 000 s, 1 000 s of warm-up); seeds
  1–5 give 15.22 ± 0.25 s.
- **A subagent's context is its own.** It starts from `K ~ uniform(2000,
  6000)`, which stands for the parent's context it copies. The cache does not
  share entries across sessions, so the subagent misses that prefix.

`--q 0` gives the same agents with no delegation. Here are both next to
the other three, `vllm.sq` under each workload (seed 1, 1 800 s after
warm-up; requests/s is the number of `response` observations over those
1 800 s; each workload arrives at its own rate, so compare the rows within
a pair, not across the table):

| workload | requests/s | hit rate | prefill tokens | TTFT (ms) | response (ms) | engine busy |
|---|---|---|---|---|---|---|
| `single_turn` | 2.98 | 0.00 | 1 995 | 43 | 96 | 0.23 |
| `human_chat` | 1.62 | 0.80 | 221 | 5 | 66 | 0.10 |
| `chat` | 2.94 | 0.89 | 727 | 16 | 62 | 0.16 |
| `subagents --q 0` | 0.90 | 0.89 | 665 | 13 | 58 | 0.05 |
| `subagents` | 3.77 | 0.68 | 1 997 | 53 | 112 | 0.29 |

Delegation increases request traffic and the mean prefill work in this
approximation. The first turn of each subagent misses its copied context;
a shared content-addressed cache could reuse that prefix. These are
illustrative model results, not measurements of a subagent runtime.

### Limits of the approximation

- Parent wait `W` is fixed, so slower child responses do not delay the
  parents. The model cannot reproduce that feedback dynamically.
- The parent holds no engine resources while it waits. This example cannot
  expose deadlocks caused by a parent retaining resources its child needs.
- Prefixes are not shared across sessions.
- Children have no parent identity. Scheduling and statistics cannot follow
  an actual task tree.

The language's `fork` runs a leg of one turn that calls a KV relation's
source (the push P/D proxy's prefill), which the session waits for only by
reading its send; it does not create the independent child sessions this
workload approximates, nor wait for them.

## Bursts and the prefix cache

A turn that waits can lose its prefix before it runs; it then computes the
prefix again, and the turns behind it wait longer. Two workloads put that
loop on `vllm.sq`. Every conversation has one fixed context of
`K0 = 16 000` tokens, and each turn adds a message of 500 tokens and an
answer of 200 on average. A turn either finds its context in the cache (a
*full hit*, `vllm.reused` at least `K0`) or computes about 16 700 tokens.
With `--blocks 34000` the KV holds about 32.6 such contexts.

```serq title="examples/workloads/closed_burst.sq"
--8<-- "examples/workloads/closed_burst.sq:workload"
```

`closed_burst.sq` keeps `N = 40` people in a closed loop. Its burst is a
shorter think time: `--burst_start 1000 --burst_len 300` thinks
`Z_burst = 1.5` s instead of `Z = 4` s for 300 s. `--Z_warm` and
`--warm_until` set the start. Until `warm_until`, a person thinks `Z_warm`
seconds: a short think time sends all first turns at once into an empty
cache, and a long one fills the cache a turn at a time.

```serq title="examples/workloads/open_burst.sq"
--8<-- "examples/workloads/open_burst.sq:workload"
```

`open_burst.sq` lets conversations arrive at `Lambda` a second. A
conversation continues with probability `p = 0.9` after each turn, so it
has 10 turns on average. The burst raises the arrival rate to
`Lambda_burst`. An arrival's rate may not read the clock (`arrive` takes
constants and draws), so the stream runs at the larger rate `top`, and an
arrival is a conversation with probability `rate() / top`; the others
leave without a turn. A thinned Poisson stream is Poisson at the rate it
keeps, as in [the subagent workload](#agent-with-subagents). `--max_live`
caps the conversations in at once with the pool `live`, as
`examples/workloads/replica.sq` does; the others wait to enter.

serQ's cache matches positions, not content. A turn also counts as hit
the new tokens that sit where the previous turn's message and answer were,
up to the smaller of the two: about 3 % of the context. A full hit does
not depend on it.

`tools/metastability/` runs both workloads under three eviction orders and
four changes to the deployment, five seeds each; its
[README](https://github.com/servingQ/serQ/blob/main/tools/metastability/README.md)
gives the results. They are simulations, not measurements, with
`--blocks 34000`:

- The closed loop recovers. After a 300 s burst its full hits and waiting
  turns are back at their level before the burst within 25–275 s. At each
  think time from 2 to 4 s, the full hits after the burst are those of a
  warm start and of a flushed start over the same windows.
- The open conversations do not. Without a burst no seed collapses up to
  `Lambda = 0.50` in 8000 s. With a burst of 1.5 conversations/s for 60 s,
  every seed from 0.45 up, and four of five at 0.35 and 0.40, stay
  collapsed to the end. A collapsed engine computes every turn whole, so
  it serves about 3 turns/s; 0.30 conversations/s bring 3 turns/s.
