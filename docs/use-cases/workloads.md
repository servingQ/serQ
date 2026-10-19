# Different workloads

These examples keep the same engine and vary the client: single requests,
chat, tool-using agents and an approximation of delegated subagent traffic.
The program is the deployment: a device, the engine `vllm` on it, its pools
and `server`. The workload, which supplies arrivals and turns, is a file of
its own given when the program runs (`serq run DEPLOYMENT --workload
WORKLOAD`); `examples/pairs.json` names each program's. Client delays (`tool`, `user`, `delegate`) represent time
outside the engine.

One deployment, `examples/multi-turn/vllm.sq`, runs under all four
workloads: `examples/pairs.json` lists them for it, its own `chat.sq` first
(`serq run examples/multi-turn/vllm.sq --workload examples/workloads/single_turn.sq`).
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
