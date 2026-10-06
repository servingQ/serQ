# Different workloads

These examples keep the same engine and vary the client: single requests,
chat, tool-using agents and an approximation of delegated subagent traffic.
The deployment uses pools, `stage engine` and `server`; `workload` supplies
arrivals and turns. Client delays (`tool`, `user`, `delegate`) represent time
outside the engine.

All four programs share the engine constants, resource declarations and
server body of `examples/multi-turn/vllm.sq`, checked by `tests/workloads.rs`.
They hide output length `o` from scheduling expressions. See the
[vLLM use case](vllm.md) for the engine's validation scope.

| | single turn | multi-turn chat | multi-turn agent | agent with subagents |
|---|---|---|---|---|
| program | `vllm_single_turn.sq` | `vllm_chat.sq` | `vllm.sq` | `vllm_subagents.sq` |
| a session is | one request | a conversation | a task | a task and the subtasks it hands out |
| carried to the next turn | nothing | the conversation, `K = prompt + o` | the same | the same |
| between turns | — | a person, `run user (cost(user, ~exp(Z)))` | a tool, `tool (~exp(Z))` | a tool, or waiting for the subagents |
| who makes arrivals | the environment | the environment | the environment | a Poisson approximation of parent and child traffic |
| the prefix cache | never read back | the last turn's prefix | the last turn's prefix | same, and a subagent misses the context it copied |

## Single turn

```serq title="examples/single-turn/vllm_single_turn.sq"
--8<-- "examples/single-turn/vllm_single_turn.sq:workload"
```

`session { turn; request; end; }` is the whole client: one request, then the
session leaves. `K` stays 0 because nothing carries over, so the server's
`prompt = K + n` is just the new tokens. The `cache (prompt + o)` in the
server still caches the prefix when the request finishes, but no later
request of that session will read it, so here the cache only takes up space
until LRU evicts it. The hit rate is 0.

This does not model shared system prompts. vLLM identifies cached blocks
by content and prefix hashes (`kv_cache_utils.py:650-680`,
`block_pool.py:197-223`), so a later request can reuse a matching cached
prefix from another request. serQ's cache is per session; see
[limitations](../language.md#9-known-limitations).

## Multi-turn chat

```serq title="examples/multi-turn/vllm_chat.sq"
--8<-- "examples/multi-turn/vllm_chat.sq:workload"
```

Each turn sends the conversation `K` plus a new message of mean 100 tokens.
After the request, `set K = prompt + o` carries its input and output into the
next turn. `run user (cost(user, ~exp(Z)))` models reading and typing time on a delay
stage, with mean `Z = 20` seconds.

## Multi-turn agent

```serq title="examples/multi-turn/vllm.sq"
--8<-- "examples/multi-turn/vllm.sq:workload"
```

The agent has the same turn structure as chat, with a 3-second mean tool
delay and 500 mean new tokens per turn. Its initial prompt is a longer task
description. The workload determines these differences.

## Agent with subagents

This example approximates subagents as independent sessions. It does not
create child sessions in response to a parent's delegation or join their
actual completion times.

```serq title="examples/subagent/vllm_subagents.sq"
--8<-- "examples/subagent/vllm_subagents.sq:workload"
```

The approximation has three parts:

- **The children arrive on their own.** A parent's turn that continues
  delegates with probability `q` to `k` subagents. A parent continues
  `p / (1 − p)` times on average, so it creates
  `kids = q · k · p / (1 − p)` subagents. The program folds them into the
  one Poisson stream at rate `Lambda · (1 + kids)`, and marks an arrival as a
  subagent (`sub`) with probability `kids / (1 + kids)`.
- **The parent waits a constant.** A delegating turn runs `run delegate (cost(delegate, W))`.
  The supplied `W = 14.7` seconds is a model input; it does not track the
  simulated children's completion times. To calibrate it, collect
  `subagent` durations with `--dump` and estimate the mean maximum of `k`
  durations under the intended delegation model.
- **A subagent's context is its own.** It starts from `K ~ uniform(2000,
  6000)`, which stands for the parent's context it copies. The cache does not
  share entries across sessions, so the subagent misses that prefix.

`--set q=0` gives the same agents with no delegation. Here are both next to
the other three (seed 1, 1 800 s after warm-up; requests/s is the number of
`response` observations over those 1 800 s; each program runs at its own
arrival rate, so compare the rows within a pair, not across the table):

| program | requests/s | hit rate | prefill tokens | TTFT (ms) | response (ms) | engine busy |
|---|---|---|---|---|---|---|
| `vllm_single_turn` | 2.98 | 0.00 | 1 994 | 43 | 94 | 0.22 |
| `vllm_chat` | 1.56 | 0.78 | 258 | 5 | 66 | 0.10 |
| `vllm` | 3.07 | 0.89 | 770 | 18 | 65 | 0.17 |
| `vllm_subagents --set q=0` | 0.82 | 0.88 | 706 | 14 | 57 | 0.05 |
| `vllm_subagents` | 3.65 | 0.70 | 1 823 | 45 | 100 | 0.27 |

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

The language's `fork`/`join` runs parallel legs of one request; it does not
create the independent child sessions this workload approximates.
