# Case study: one engine, four workloads

A serving system is an engine plus the traffic it serves, and a program says
which part is which. The pools, the stages and the `server` block are the
engine: what vLLM does with one request. The `workload` block is the client:
when sessions arrive, what each turn sends, what happens between turns, and
when a session leaves. [The vLLM case study](case-study-vllm.md) held the
workload fixed and checked the engine against the real scheduler. This page
holds the engine fixed and changes the client.

The four programs below run `programs/vllm.seq`'s engine line for line: the
constants from `B` to `c0`, `pool kv` to the end of `stage engine`, and the
`server` block. `tests/workloads.rs` fails if one of them drifts, so the only
thing that differs between them is what is written inside `workload`.

| | single turn | multi-turn chat | multi-turn agent | agent with subagents |
|---|---|---|---|---|
| program | `vllm_single_turn.seq` | `vllm_chat.seq` | `vllm.seq` | `vllm_subagents.seq` |
| a session is | one request | a conversation | a task | a task and the subtasks it hands out |
| carried to the next turn | nothing | the conversation, `K = prompt + o` | the same | the same |
| between turns | — | a person, `run user (~exp(Z))` | a tool, `tool (~exp(Z))` | a tool, or waiting for the subagents |
| who makes arrivals | the environment | the environment | the environment | the environment **and the engine's own answers** |
| the prefix cache | never read back | the last turn's prefix | the last turn's prefix | same, and a subagent misses the context it copied |

## Single turn

```seq title="programs/vllm_single_turn.seq"
--8<-- "programs/vllm_single_turn.seq:workload"
```

`session { turn; request; end; }` is the whole client: one request, then the
session leaves. `K` stays 0 because nothing carries over, so the server's
`prompt = K + n` is just the new tokens. The `keep (prompt + o)` in the
server still caches the prefix when the request finishes, but no later
request of that session will read it, so here the cache only takes up space
until LRU evicts it. The hit rate is 0.

What this does not show: in a real single-turn deployment, requests share a
system prompt, and vLLM's cache is content-addressed, so they hit on it. seQ
keeps one cache entry per session ([language](language.md) §9), so a shared
prefix cannot be written yet.

## Multi-turn chat

```seq title="programs/vllm_chat.seq"
--8<-- "programs/vllm_chat.seq:workload"
```

Turns are what make the cache matter. Each turn sends back the whole
conversation (`K`) plus a short message (`~exp(100)` tokens), so most of the
prompt is the previous turn's prefix, and `set K = prompt + o` after `request`
is how the program says so. The gap between turns is a person reading and
typing. It is written as `run user (…)` on a `delay` stage of its own rather
than as `tool …`, because it is not a tool, and the serving form `tool` would
say it was.

## Multi-turn agent

```seq title="programs/vllm.seq"
--8<-- "programs/vllm.seq:workload"
```

This is `programs/vllm.seq`, the program the vLLM case study checks. Its
shape is the same as the chat's. The differences are only in the numbers: the
gap is a tool call (3 s rather than 20 s), each turn brings back a tool's
output (`~exp(500)` new tokens rather than `~exp(100)`), and the first prompt
is a long task description. seQ has no separate construct for "agent" versus
"chat", and it should not: the difference is policy in the workload, which a
program states and the language does not.

## Agent with subagents

A subagent is a parent agent sending requests to the same engine and waiting
for the answers. From the parent's side it looks like a tool call. From the
system's side it is not one: the wait is the engine's own response time, a
function of load, and the arrivals come from inside the system. [Subagents in
the IR](design/subagents.md) explains why that needs two new statements,
`spawn` and `join`. Until those exist, the program below approximates
subagents, and it is useful to see exactly what the approximation gives up.

```seq title="programs/vllm_subagents.seq"
--8<-- "programs/vllm_subagents.seq:workload"
```

The approximation has three parts:

- **The children arrive on their own.** A parent's turn delegates with
  probability `q` to `k` subagents, so a parent creates
  `kids = q · k / (1 − p)` of them on average. The program folds them into the
  one Poisson stream at rate `Lambda · (1 + kids)`, and marks an arrival as a
  subagent (`sub`) with probability `kids / (1 + kids)`.
- **The parent waits a constant.** A delegating turn runs
  `run delegate (W)` on a `delay` stage. `W` comes from a previous run: the
  mean of the slowest of `k` `subagent` samples. For this program,
  `seq-lang run programs/vllm_subagents.seq --dump out` followed by that
  statistic over `out/subagent.csv` returns about 15 s when `W = 15`, which is
  the fixed point.
- **A subagent's context is its own.** It starts from `K ~ uniform(2000,
  6000)`, which stands for the parent's context it copies. The cache does not
  share entries across sessions, so the subagent misses that prefix.

`--set q=0` gives the same agents with no delegation. Here are both next to
the other three (seed 1, 1 800 s after warm-up; each program runs at its own
arrival rate, so compare the rows within a pair, not across the table):

| program | requests/s | hit rate | prefill tokens | TTFT (ms) | response (ms) | engine busy |
|---|---|---|---|---|---|---|
| `vllm_single_turn` | 2.98 | 0.00 | 2 004 | 43 | 96 | 0.23 |
| `vllm_chat` | 1.61 | 0.80 | 221 | 4 | 67 | 0.10 |
| `vllm` | 3.15 | 0.89 | 767 | 17 | 64 | 0.17 |
| `vllm_subagents --set q=0` | 1.00 | 0.90 | 686 | 14 | 58 | 0.06 |
| `vllm_subagents` | 3.84 | 0.67 | 2 020 | 54 | 114 | 0.29 |

For the same parent arrival rate, delegation makes 3.8 times as many requests.
Each first request of a subagent is a full miss on the context it copied, so
the mean prefill triples and TTFT rises from 14 ms to 54 ms. The prefill row
is an upper bound: the real vLLM would hit on the shared prefix, and this seQ
program cannot.

What the approximation cannot show, in the order of
[the subagent design](design/subagents.md):

1. **Feedback.** `W` is a number, not the engine's response time. If the
   engine slows down, the parents do not wait longer, so the loop that makes a
   system with subagents collapse (slower answers, more parents waiting, more
   children in flight) is cut. The fixed point matches at steady state, but
   the cliff is not there.
2. **Hold-and-wait.** A parent in `delegate` holds nothing, which is also the
   rule the design proposes. A program that joins while holding `reqs` cannot
   be written today, and so the deadlock it causes cannot be written either.
3. **Shared prefixes.** This is the prefill row above.
4. **Trees.** Nothing ties a subagent to its parent, so a scheduler that
   serves the child a parent is waiting for first cannot be written, and
   statistics are per session, not per task.

With the statements the design proposes, the delegating branch would read
as follows. It does **not** parse today:

```
branch (fan) {
  spawn (k) into kids { set K = K; set sub = 1; }   // read in the parent: a child starts from its context
  join kids;                                        // the wait is the engine's own
} else {
  tool (~exp(Z));
}
```

The rest of the program would stay as it is. `arrive` would go back to
`poisson(Lambda)`, because the children would no longer come from the
environment.
