# The run log

`serq run FILE --workload W --log LOG.json` keeps what each session did, when, and writes
it to `LOG.json`; with `--json`, the report carries the same object as
`log`. The report is a summary of the run; the log is the run itself, as a
replay would need it: a session arriving, waiting in a pool's queue,
admitted, running on a stage, preempted, ending, and each iteration of an
engine with the requests in its batch. [serq-lar](https://github.com/servingQ/serq-lar)
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
    {"t": 0, "ev": "admit", "s": 0, "pool": 0},
    {"t": 0, "ev": "run", "s": 0, "stages": [0], "mode": "prefill"},
    {"t": 0, "ev": "iter", "stage": 0, "dur": 0.027420000000000003, "tokens": 1371, "reqs": [[0]]},
    …
    {"t": 0.027420000000000003, "ev": "done", "s": 0, "stage": 0},
    {"t": 0.027420000000000003, "ev": "wait", "s": 0, "pool": 6},
    {"t": 0.027420000000000003, "ev": "admit", "s": 0, "pool": 6},
    {"t": 0.027420000000000003, "ev": "run", "s": 0, "stages": [8]},
    {"t": 0.02942, "ev": "done", "s": 0, "stage": 8},
    {"t": 0.02942, "ev": "run", "s": 0, "stages": [2, 6]},
    {"t": 0.036275, "ev": "done", "s": 0, "stage": 2},
    {"t": 0.036275, "ev": "wait", "s": 0, "pool": 4},
    {"t": 0.036275, "ev": "admit", "s": 0, "pool": 4},
    {"t": 0.036275, "ev": "run", "s": 0, "stages": [4], "mode": "prefill"},
    {"t": 0.036477752, "ev": "done", "s": 0, "stage": 4},
    {"t": 0.036477752, "ev": "run", "s": 0, "stages": [4], "mode": "decode"},
    {"t": 0.052298968, "ev": "done", "s": 0, "stage": 4},
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
| `iter` | `stage`, `dur`, `tokens`, `reqs` | an iteration of engine `stage` starts: it lasts `dur`, schedules `tokens`, over the requests `reqs` in the batch's order, each `[s]` for a session's or `[s, leg]` for a leg's |
| `end` | `s` (`leg`) | the session (the leg) ends |

A request to an engine is its session's `wait` and `admit` at the engine's
pools, then a `run` in `prefill` mode and, once its prefill is done, one in
`decode` mode on the same stage; a preemption between them is a `preempt`,
a `wait` and an `admit` again. A session at a `tool` runs on the tool's
stage.

Not logged, since no time passes or no session is there: a run of no work
(a decode of zero tokens, a transfer of nothing: no `run`, no `done`); a
session at a `join`, waiting for its legs; a failed `grow` waiting for room
(the pool's `growing_at_end`); and a spill's transfer, which no session
runs (`spill … via`).

Not in version 0: a request's token counts beyond `iter`'s total, the
units a hold takes, and what a pool's cache holds; a columnar encoding
for long runs. A renamed, removed or retyped field bumps `version`
(`LOG_VERSION`); an added one does not.
