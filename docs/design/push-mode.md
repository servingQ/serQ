# The push mode

NIXL's push mode, as vLLM's push proxy runs it, sends the prefill request
and the decode request of one request at once: the decoder allocates its
blocks while the prefill runs, and the prefiller writes the KV into them as
soon as it has both its finished blocks and the decoder's registration. A
serQ session waited at one place at a time, so the decoder's allocation
could only be written after the prefill, which is the serial dispatch. This
document is the design of `fork { … }`, `join;` and `P push D …;`, issue
#368: two IR statements and a parse-time relation.

## What vLLM does

At `ref/vllm` 0c87a197:

- The proxy starts the prefill leg as a task and streams the decode leg,
  awaiting the prefill at the end
  (`disagg_proxy_pushconnector_demo.py:227-279`).
- The decoder's scheduler takes the decode request when the whole prompt
  fits in its blocks and parks it, holding the blocks and no running slot,
  as in pull mode; on allocation it stashes the registration its worker
  sends to the prefiller (`nixl/push_scheduler.py:128-205`).
- The prefiller's scheduler frees the slot when the one token is sampled
  and keeps the blocks leased for the write
  (`nixl/push_scheduler.py:207-294`).
- The prefiller's worker matches finished blocks against registrations:
  whichever arrives second starts the WRITE
  (`nixl/push_worker.py:254-264`, `308-313`, `714-722`).

A rendezvous of two legs of one request, each queued and served by its own
scheduler.

## Before

`docs/design/pull-relation.md` at `1f682bf`, *Self-critique*:

```
**No `push`.** `P push D` would read as the push a reader expects, vLLM's
proxy, which sends the prefill and the decode request at once
(`disagg_proxy_pushconnector_demo.py:227-270`), so that the decoder
allocates during the prefill and the prefiller writes as soon as it has
both the finished request and the decoder's registration
(`nixl/push_scheduler.py:207-294`). That needs a reservation the language does not
have ([The KV transfer](pd-transfer.md)).
```

and the reservation it pointed to, `docs/design/pd-transfer.md`,
*Self-critique*:

```
book kvD (prompt) reserve (prompt);            // join D's queue; the scheduler allocates when it gets there
admit if reqsP (1), kvP (…) fit { prefill on P (…) growing kvP; } keep (prompt) lease kvP (inf);
enter kvD { transfer (w) from kvP to kvD (n); … }   // open the booking, waiting if it is not granted yet
```

## After

`examples/pd-disaggregation/vllm_nixl_push.sq`:

```serq
queue gw : gateway {
  route {
    …
    branch (concurrent) {
      fork { P.prefill (prompt); }     // asyncio.create_task: the prefill leg runs beside
    } else {
      P.prefill (prompt);              // the sidecar's order: the prefill's answer first
    }
    D.decode (prompt) from P;          // the decode request; it waits for the prefill's KV inside
    …
  }
}
queue D : decode {
  …
  decode (prompt) from src {
    hold kv (known) reserve (known), reqs (0) reserve (1) … {
      …
        mark parked;
        join;                                                     // P's finished blocks meet the registration: the write can start (nixl/push_worker.py:254-264)
        transfer (prompt - c) from src to kv (prompt - 1 - c);   // P's WRITE over both NICs
      …
    } cache (prompt + o);
  }
}
P push D latency x0 share maxmin;
```

## The design

**A leg is a part of the request.** `fork { body }` runs `body` beside the
session from now: a copy of the attributes, holds of its own, a stream of
its own, the session's serial and so its cached prefix. It is not a
session: no arrival, no end, no `turn`. Its `set`s change its copy and end
with it, so two legs never write one attribute. What it leases passes to
the session when it ends, which is what lets the decode leg's `transfer …
from src` take the prefiller's lease.

**`join;` waits for every leg forked so far.** No handle: the push proxy
has one leg, and a `join` with a name would have to reach the entry of
another queue, which reads only its own. The blocking point is a statement
of its own, where the decoder's request waits, inside the hold of its
blocks.

**`P push D` says who moves the bytes, not when the decoder is asked.**
The same two NICs as `D pull P`, the same `share`; the wait before each copy
is the poster's, so it is `P.nic.latency`, indexed by the source member.
The dispatch is the gateway's, and `concurrent = 0` is llm-d's serial push
with the same relation (criterion 2: the program states the opposite).

| Surface | Kernel |
|---|---|
| `fork { … }` | `CStmt::Fork(body)` |
| `join;` | `CStmt::Join` |
| `P push D latency x share s;` | `stage P.nic.latency[N] : delay; share s;`, `x` the constant `P.push.time` |
| `transfer (n) from src to kv (m)` in `D[j]`, called `from P[i]` | `run P.nic.latency[i] (x); run P.nic[i], D.nic[j] (n); load D.kv[j] (m); release P.kv[i];` |

**Checks.** At link time: a leg may not `turn`, `end`, fork or `join`; a
leg acts on no hold around its fork; a `fork` stands in no hold that may be
preempted (the hold would run again and fork a second leg); a `join` needs
a fork in the program, and a fork a `join`; a queue posts the copies of one
relation. At run time: a session may not end while a leg runs; a run that
ends with sessions and legs that wait only for each other, one of them at
a `join` and none for a lease that expires, is an error naming each. A
leg's lease caches by the leg's attributes when it ends, after it has
passed to the session.

The latency stage is a delay, so a push and a pull of the same constants
run the same numbers: the relation names who posts the copy, and the
gateway's `fork` is what changes the schedule.

## What it earns

The program is the proxy's task and the connector's rendezvous: one
statement for the prefill request the proxy starts beside the decode
request, one where the decoder's request waits for the prefiller's
finished blocks (the proxy's own await, at the end of the stream, has
nothing left to wait for by then), and the decoder's admission is the one
the pull program has. On the
A6000 testbed's trace (96 sessions, 957 requests, the constants fitted on
lone requests; the program draws nothing, so every seed gives these
numbers), at the replay's spacing of 3 s and at half of it:

| | concurrent, 3 s | serial, 3 s | concurrent, 1.5 s | serial, 1.5 s |
|---|---|---|---|---|
| TTFT mean (s) | 1.04 | 0.99 | 34.6 | 33.6 |
| the decoder's blocks waiting for the KV, mean (s) | 0.95 | 0.03 | 18.2 | 0.08 |
| requests holding `D.kv`, time average | 0.33 | 0.04 | 5.85 | 0.05 |
| requests queued at `D.kv`, time average | 0.00 | 0.00 | 5.18 | 0.00 |

The time averages are over the 3 000 s horizon, most of which is empty
after the last request; their ratios are what to read. Holding the
decoder's blocks during the prefill does not bring the first token sooner:
when the decoder admits at once the prefill is the critical path, and when
the prefiller is the bottleneck the decoder waits for it either way. The
TTFT differences between the two forms are the schedule perturbed, not an
effect: request for request the median difference is 0, and its sign
changes with the spacing. What the concurrent form changes is the
decoder's memory: at 1.5 s it fills with requests waiting for their KV and
new ones queue behind them.

The measured push run (`tools/a6000/push1x1_s3_*.jsonl`, the push proxy)
collapsed: TTFT 95 s, the decoder's KV 94 % used, the prefiller's queue at
97 on average. It is not a run at the replay's load: the proxy, the
prefiller and the decoder each logged 1 944 requests where the replay sent
971 (957 and 14 probes; the pull run's proxy logged 971), from a second
client the records do not name. At about that load (1.5 s) the program
shows the same shape, a prefiller that cannot keep up and a decoder full of
parked requests, at a smaller TTFT; whether it gives the measured numbers
needs a run whose load is known.

## Self-critique

**Not a reservation.** `book kvD (…)` / `enter kvD { … }` would have been
a fork whose leg is one hold, plus a second meaning of hold: a granted
booking holds units before any scope opens, and one never entered needs a
rule of its own. vLLM has no booking either; the decoder schedules the
request as any other. Fork/join keeps one meaning of hold and is the
`Join` that [subagents](subagents.md) asked for.

**Not the prefill nested in the decoder's hold.** `hold kvD { P.prefill
…; transfer …; }` makes the prefill wait for the decoder's memory: when the
decoder is full the real prefiller goes on and the program stops, a wrong
answer.

**Not an implicit join.** A `transfer … from src` that waits for the
source's lease to exist writes one statement less and hides where the
request blocks.

**Not refused when the legs can wait for each other.** A decode leg holds
the decoder's memory waiting for its prefill, which waits for the
prefiller's memory, which another request's lease holds until its decode
leg is admitted to the decoder (`tests/fork_join.rs`). Whether that happens
depends on the capacities and the timing, not on the program's shape; a
rule against a `join` inside a hold would refuse push itself. The run
fails instead, naming who waits where. In vLLM the prefiller's lease
expires when no decoder has registered, and the request fails; the
program's lease is one number and does not know whether a decoder has
registered, so `lease kv (inf)` can deadlock where vLLM would drop a
request, and a finite lease copies nothing after its expiry without
failing the request.

**The registration is not a wait of its own.** The decoder's worker sends
it one step after the allocation; the program folds it into the copy's
latency, so a prefill shorter than a decoder step is written a step early.

**A leg's observations and marks.** A leg's `observe` is the program's,
but its `mark` sets its own copy and is lost at its end: the session cannot
read when its prefill leg ended. `D.parked` and `D.written` are the
decoder's.

**`join` has no handle.** A session with two kinds of legs that it joins
at different places cannot say which; none of the programs does. The
decoder's `join` waits for every leg its caller forked, so a gateway that
forked a second leg for something else would make the decoder wait for it
too.

**No scheduler oracle.** As for the pull program: two schedulers and a fake
connector would check the parked request's admission step and the write's
start; the numbers above are the program's, checked against the source by
line.
