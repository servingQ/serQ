# The run log

`serq run FILE --workload W --log LOG.json` keeps what each session did, when, and writes
it to `LOG.json`; with `--json`, the report carries the same object as
`log`. The report is a summary of the run; the log is the run itself, as a
replay would need it: a session arriving, waiting in a pool's queue,
admitted, running on a stage, preempted, ending, each change of an engine
request's [status](cli.md#request-statuses) and each move of its KV, and
each iteration of an engine with the requests in its batch, the tokens each
takes, its budget and its memory pool's use. [serq-lar](https://github.com/servingQ/serq-lar)
animates it over the [deployment view](../visualization/deployment.md).

The log is off unless asked for, and the run is the same with it: the
interpreter checks one flag per event. In the API it is
`serq::run_ir_logged(&program, base, cap)`, with a cap on the events kept
(the CLI keeps all of them): an hour of a busy engine is about a million
iterations.

```bash
serq run examples/pd-disaggregation/llmd_nixl_pull.sq --workload examples/workloads/chat_reply.sq --horizon 30 --log log.json
jq '.events[] | select(.s == 0)' log.json
```

## Shape (version 0)

The first session of `llmd_nixl_pull.sq`, as above (the events of other
sessions and the engines' iterations left out, `…`):

```json
{
  "version": 0,
  "stages": [{"name": "P", "index": 0}, {"name": "P", "index": 1}, {"name": "P.nic", "index": 0}, …],
  "pools":  [{"name": "P.reqs", "index": 0}, …],
  "truncated": false,
  "events": [
    {"t": 0, "ev": "arrive", "s": 0},
    {"t": 0, "ev": "turn", "s": 0, "turn": 1},
    {"t": 0, "ev": "wait", "s": 0, "pool": 0},
    {"t": 0, "ev": "status", "s": 0, "r": 1, "stage": 0, "status": "waiting"},
    {"t": 0, "ev": "admit", "s": 0, "pool": 0},
    {"t": 0, "ev": "run", "s": 0, "stages": [0], "mode": "prefill"},
    {"t": 0, "ev": "status", "s": 0, "r": 1, "stage": 0, "status": "running", "mode": "prefill"},
    {"t": 0, "ev": "iter", "stage": 0, "dur": 0.027420000000000003, "tokens": 1371, "budget": 8192, "reqs": [[0]], "each": [1371], "used": 1376, "cached": 0},
    …
    {"t": 0.027420000000000003, "ev": "done", "s": 0, "stage": 0},
    {"t": 0.027420000000000003, "ev": "status", "s": 0, "r": 1, "stage": 0, "status": "finished"},
    {"t": 0.027420000000000003, "ev": "transfer", "s": 0, "r": 1, "stage": 0, "kind": "send", "at": "registered"},
    {"t": 0.027420000000000003, "ev": "wait", "s": 0, "pool": 6},
    {"t": 0.027420000000000003, "ev": "status", "s": 0, "r": 2, "stage": 4, "status": "waiting"},
    {"t": 0.027420000000000003, "ev": "admit", "s": 0, "pool": 6},
    {"t": 0.027420000000000003, "ev": "transfer", "s": 0, "r": 2, "stage": 4, "kind": "load", "at": "registered"},
    {"t": 0.027420000000000003, "ev": "status", "s": 0, "r": 2, "stage": 4, "status": "waiting_for_kv"},
    {"t": 0.027420000000000003, "ev": "transfer", "s": 0, "r": 2, "stage": 4, "kind": "load", "at": "start"},
    {"t": 0.027420000000000003, "ev": "transfer", "s": 0, "r": 1, "stage": 0, "kind": "send", "at": "start"},
    {"t": 0.027420000000000003, "ev": "run", "s": 0, "stages": [2, 6]},
    {"t": 0.036275, "ev": "done", "s": 0, "stage": 2},
    {"t": 0.036275, "ev": "transfer", "s": 0, "r": 1, "stage": 0, "kind": "send", "at": "done"},
    {"t": 0.036275, "ev": "transfer", "s": 0, "r": 2, "stage": 4, "kind": "load", "at": "done"},
    {"t": 0.036275, "ev": "wait", "s": 0, "pool": 4},
    {"t": 0.036275, "ev": "status", "s": 0, "r": 2, "stage": 4, "status": "waiting"},
    {"t": 0.036275, "ev": "admit", "s": 0, "pool": 4},
    {"t": 0.036275, "ev": "run", "s": 0, "stages": [4], "mode": "prefill"},
    {"t": 0.036275, "ev": "status", "s": 0, "r": 2, "stage": 4, "status": "running", "mode": "prefill"},
    {"t": 0.036477752, "ev": "done", "s": 0, "stage": 4},
    {"t": 0.036477752, "ev": "run", "s": 0, "stages": [4], "mode": "decode"},
    {"t": 0.036477752, "ev": "status", "s": 0, "r": 2, "stage": 4, "status": "running", "mode": "decode"},
    {"t": 0.052298968, "ev": "done", "s": 0, "stage": 4},
    {"t": 0.052298968, "ev": "status", "s": 0, "r": 2, "stage": 4, "status": "finished"},
    {"t": 0.052298968, "ev": "end", "s": 0},
    …
  ]
}
```

`stages` and `pools` are the program's, in the IR's order: an event names
one by its index, and an array's members share a name and carry an
`index` (`null` for a single stage or pool). `truncated` says whether the
cap stopped the log before the run ended. Events are in time order; ties
keep the order they happened in.

`s` is the session's serial. A leg of it ([`fork`](../api/statements.md))
shares the serial and carries its number as `leg` in every event about
it; it begins with `fork`, not `arrive`, so a count of `arrive` is the
report's `arrivals`. The keys are short because the events are many.

| `ev` | fields | means |
|---|---|---|
| `arrive` | `s` | the session arrives |
| `fork` | `s`, `leg` | a leg of the session is forked: its number |
| `turn` | `s`, `turn` | a turn of the session starts: its `turn` number, from 1 |
| `wait` | `s`, `pool` | it waits in the queue of `pool` for a hold; a hold of several pools waits in the queue of its first |
| `admit` | `s`, `pool` | it is admitted to the hold it waited for at `pool` |
| `refuse` | `s` | a hold that can never fit refuses it (the pool's `rejected`); an `end` follows (a leg's, then its session's) |
| `run` | `s`, `stages`, `mode` | it runs on `stages`, one usually; two or more for a run that holds shared stages at once, a KV transfer over both NICs (`also`). `mode` is `prefill` or `decode` on an engine, absent otherwise |
| `done` | `s`, `stage` | its run on `stage` (the first of several) is done; the session is ready again |
| `preempt` | `s`, `pool` | it is preempted from `pool`: back to the queue, where a `wait` follows |
| `revoke` | `s`, `pool` | its lease of `pool` is taken (`preempt leases by`) |
| `status` | `s`, `r`, `stage`, `status`, `mode` | its engine request `r` (its `r`-th call, from 1) to engine `stage` enters [`status`](cli.md#request-statuses) (`waiting`, `waiting_for_kv`, `running`, `preempted`, `finished`), or its `mode` there changes: `prefill` or `decode` while `running`, absent otherwise |
| `transfer` | `s`, `r`, `stage`, `kind`, `at` | a KV move of that request: `kind` `load`, `save` or `send`, `at` `registered` (it exists: a load or save begun, a send its request's end left), `start` (its copy runs) or `done` (copied, read, or expired) |
| `iter` | `stage`, `dur`, `tokens`, `budget`, `reqs`, `each`, `used`, `cached` | an iteration of engine `stage` starts: it lasts `dur`, schedules `tokens` of its `budget` (the engine's `tokens cap` as this iteration reads it, `null` when it is `inf`), over the requests `reqs` in the batch's order, each `[s]` for a session's or `[s, leg]` for a leg's. `each` is the tokens of each of `reqs`, in its order (a decode's 1, a prefill's chunk), and sums to `tokens`. `used` and `cached` are the units allocated and cached in the engine's memory (its device's pool, `pool kv on gpu`) once the iteration is scheduled: `used(p)`, and what the report's `mean_used` and `mean_cached` average. They are units, not blocks (divide by the pool's `block` for blocks), and absent for an engine with no memory |
| `end` | `s` (`leg`) | the session (the leg) ends |

A request to an engine is its session's `wait` and `admit` at the engine's
pools, then a `run` in `prefill` mode and, once its prefill is done, one in
`decode` mode on the same stage; a preemption between them is a `preempt`,
a `wait` and an `admit` again. A session at a `tool` runs on the tool's
stage.

A request is a call of an engine (`E (input, output)`, vLLM's `Request`):
the session or leg `s` that called it, its call number `r` and the engine
`stage` name it, and a P/D turn's prefill and decode are two. A `status`
event follows the event that moved it there, at the same instant; two in a
row about one request differ, and `finished` is its last. A send is about
the source's request whoever holds it: a leg's prefill's send passes to its
session, and the read that ends it is the session's (`vllm_nixl_push.sq`).
Above, the prefiller's request (`r` 1) is `finished` from its end until the
decoder's load reads its send, while the decoder's (`r` 2) is
`waiting_for_kv`: one status each, the copy one move of both
([the design](../design/request-state.md)). A session that calls no engine
has neither.

An engine's iterations, one line each: the batch, each request's tokens
and the memory's use. A prefill in chunks is the same `[s]` in successive
iterations with its chunk in `each`; whether a request prefills or decodes
is its last `run`'s `mode`, which a `run E decode (…)` outside any engine
request has too, with no `status`.

```bash
jq -c '.events[] | select(.ev == "iter") | [.t, .stage, .reqs, .each, .used, .cached]' log.json
```

The same, in the form `tools/vllm_replay_oracle.py` prints vLLM's
scheduler steps with `ORACLE_TRACE_ITER=1`, so the two diff line by line
(`ITER t s:turn:mn … | used U cached C`, the memory in blocks of
`--block` units):

```bash
python3 tools/serq_iter_trace.py log.json --block 16
```

Not logged, since no time passes or no session is there: a run of no work
(a decode of zero tokens, a transfer of nothing: no `run`, no `done`); a
failed `grow` waiting for room (the pool's `growing_at_end`; a request's
resident stalled so stays `running`); and a spill's transfer, which no
session runs (`spill … via`).

Not in version 0: the units each hold takes, and the use of a pool that
is no engine's memory; which entries a pool's cache holds, beyond their
total; a columnar encoding for long runs. A renamed, removed or retyped
field bumps `version` (`LOG_VERSION`); an added one does not, nor an added
event a reader may pass over (`status` and `transfer` came after version 0
did, and so did `iter`'s `budget`, `each`, `used` and `cached`).
