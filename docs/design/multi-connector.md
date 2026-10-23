# Several connectors on one engine

An engine had one KV connector, so a deployment that both splits prefill
from decode and shares an L2 (LMCache, a Mooncake store) between them could
not be written. vLLM writes it with `MultiConnector`, which holds an
ordered list of connectors. #599 found it in a study of coding-agent
workloads, where "P and D share LMCache, so P fetches from the L2 the KV
its HBM lacks" stopped linking after #592; in the old syntax (`3d47a68`,
`kvt.l2_load` with `kvt.nixl_pull`) the shared L2 raised the split
deployment's throughput by 41-49% at paper scale (N = 96/192).

An engine now has its `connector` clauses in the order written, and a
request loads from the first that has tokens for it, vLLM's rule. IR 16:
`CConnector.load` becomes `loads`.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.sq` (at `1cb55ea`) with an L2
declared before `queue D[ND]` and a second connector on the decoder, after
`connector P`:

```serq
  queue lmc : l2 {
    device host { kv cap 1e6; }
    pool kv on host { block 256; evict lru; }
    serve ps(1);
  }

  // --8<-- [start:decoder]
  queue D[ND] : agg, decode {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksD * bs; }
    engine on gpu {
      reqs cap max_seqsD;
      tokens cap B;
      connector P { load (request.preempted ? 0 : request.input - request.hit); }
      connector lmc {
        per_token (5e-6);
        load (request.preempted ? 0 : max(0, min(floor(cachedin(lmc.kv), 256), request.known - 1) - request.hit));
        save (floor(request.input + request.output, 256));
      }
      scheduler { … }
```

```text
$ serq check multi_before.sq --workload examples/workloads/chat_reply.sq
multi_before.sq: 93:7: `connector` twice: an engine has one KV connector
93 |       connector lmc {
   |       ^
```

## In vLLM

At `ref/vllm` `0c87a197`:

- "Load KV from the first connector that advertises available tokens from
  get_num_new_matched_tokens(), based on the order in the config. Save to
  all connectors." (`multi_connector.py:134-141`)
- The load: `get_num_new_matched_tokens` asks every connector in order
  and assigns the request to the first whose tokens are above 0
  (`multi_connector.py:429-448`); the others are told 0 tokens
  (`update_state_after_alloc`, `multi_connector.py:450-460`).
- The save: every connector saves (`save_kv_layer`, `wait_for_save`,
  `multi_connector.py:334-346`), and the request's blocks are freed after
  the last asynchronous save (`multi_connector.py:357-375, 525-554`).
- The NIXL connector advertises a remote prefill's tokens once: its first
  `update_state_after_alloc` clears `do_remote_prefill`
  (`nixl/pull_scheduler.py:59-65, 188`), so a preempted request asks the
  next connector. Told 0 tokens, it still notifies the prefiller to free
  its blocks (`nixl/pull_scheduler.py:145-147`).
- The documented P/D setups put the point-to-point connector first and
  the shared store second: NIXL then a storage connector
  (`docs/features/disagg_prefill.md:32-35`), `MooncakeConnector` then
  `MooncakeStoreConnector` (`docs/features/mooncake_store_connector_usage.md:86-88`).

## The design

### The text

An engine has zero or more `connector Q { … }` clauses, each an L2 queue's
or the source of its queue's KV relation, as before. They are asked in the
order written, the source's first; one with no `load` is not asked, and
where it stands does not matter. Each queue is named once: one connector
holds its `load` and its `save`. One connector at most has `save`.

### What a request does

At its admission, after `request.hit`, each connector's `load` is read in
their order; the source's connector of a request whose entry took its KV
from no source (`E.from` -1) is not asked. The first above 0 is the
request's load: `request.loaded`, `E.loaded`, `E.load_done` are its, and
the request copies through that connector alone. The others load nothing.
A read of a send that has ended (its `ttl`, no leg left to leave another)
is still the request's load when its amount is above 0, and loads
nothing: the request computes the tokens, as before. When the request has
generated its output, the one `save` runs.

The decoder of `examples/pd-disaggregation/llmd_nixl_lmcache.sq` asks
NIXL first. A remote request, `decode (prompt) from P[i]`, reads its
prompt from the prefiller; a local one took its KV from no prefiller, and
the L2 is asked; a preempted one asks for nothing from either, as vLLM's
NIXL and LMCache connectors answer 0 for it. Both save to the L2 at the
end. The prefiller loads from the L2 what an earlier turn of its session
left there.

### IR 16

`CConnector.load: Option<CTransfer>` becomes `loads: Vec<CTransfer>`, in
the program's order. `save` stays one `Option<CTransfer>`, and the slots
`loaded`, `saved`, `load_done`, `from` and `record` stay one per engine:
a request runs one load and one save at most, so they are the request's,
which answers whose records they are. 15 was tagged (v0.1.7-alpha.2), so
the retyped field is IR 16: `scripts/gen_lean_oracle.py` reads 15 and 16
alike (a connector is outside the Lean fragment, by the field's name,
which stays), and the generated IR moves its version only.

### The checks

- **The parser**: a queue named by two connectors ("`connector lmc` twice:
  one connector names its queue, with its `load` and its `save`"); a
  second `save` ("`connector b` saves, and so does `connector a`: a
  request here runs one save, where vLLM runs every connector's at once
  (multi_connector.py:334-346); keep `save` in one"). A connector named
  for a queue the engine's queue does not read from, where the engine has
  the source's connector already, is
  `E-connector-misnamed` with no fix-it: the rename would name the source
  twice. A relation whose reader's engine lacks the source's connector is
  `E-copy-no-connector`, its help now to write it before the others that
  load, where it told the program to replace the one it had.
- **`Program::validate`**: at most one load reads a source ("two of a
  connector's loads read a source: a request's entry names one (`from`),
  which one load reads"); the text cannot write two (one relation per
  reader, one connector per queue), IR from JSON can.
- **The source's connector first** (`Validator::connector`): a connector
  before it that has tokens is the request's load and passes the read over
  (`multi_connector.py:445-447`). In a push the session would end while
  its leg runs, which was a run-time error. In a pull the send would keep
  the prefiller's blocks until its `ttl` or the session's end, where
  vLLM's NIXL, told 0 tokens, frees them at once
  (`nixl/pull_scheduler.py:145-147`): a wrong answer, and a large one
  (below, *A free order in a pull*). Both are refused when the program
  links: "engine `D`: `connector P` is asked after `connector lmc`: a
  request `lmc` loads passes its read of `P` over, which keeps `P`'s
  blocks until the send's `ttl` or the session's end, where vLLM frees
  them at once (nixl/pull_scheduler.py:145-147), and in a push ends the
  session while its leg runs; write `connector P` first". F1, a read
  passed over ending its send, would lift it for a pull.

### Criterion 2: the order and the amounts are the program's

"The first that has tokens" is vLLM's rule, and a program cannot state
"load from the one that has more". It does not need to: the rule is a
mechanism the program parameterises with the order of its L2s, after the
source's, and every amount, and the opposite rule is written in the
amounts. "Load from the prefiller
unless the L2 holds the whole prompt" is

```serq
connector P { load (request.preempted || floor(cachedin(lmc.kv), chunk) >= request.input ? 0 : request.input - request.hit); }
```

before `connector lmc`, which links and runs. A selection clause (`load
by (…)`) would be a second spelling of what that guard says. Its read of 0
for a remote request leaves the prefiller's send to its `ttl`, as a load
of 0 does with one connector (F1).

## After

The Before links. `examples/pd-disaggregation/llmd_nixl_lmcache.sq` is
`llmd_nixl_pull.sq` with the L2, the decoder's two connectors as above and
a prefiller's `connector lmc` that loads; its admissions take blocks for
a load and no running slot until the KV is there, for either connector
(`scheduler.py:1069-1072, 1264-1267`):

```serq
connector P { load (request.preempted ? 0 : request.input - request.hit); }
connector lmc {
  per_token (r);
  load (request.preempted ? 0 : beyond(request.known, request.hit));
  save (floor(request.input + request.output, chunk));
}
```

```text
$ serq run examples/pd-disaggregation/llmd_nixl_lmcache.sq --workload examples/workloads/chat_reply.sq --horizon 300 --warmup 30
…
P.loaded     742   295.5903   ±34.1925   2.698   2192.0000
D.loaded    1270  1228.3197  ±147.7124   2.441  10437.0000
…
ttft       1270    0.02353    ±0.00119  0.859    0.08516
…
stage     number   util  done    thru    wait  service   iters
lmc        0.025  0.025  1625  6.0185  0.0000   0.0042       0
…
D.nic[0]   0.020  0.016   544  2.0148  0.0020   0.0081       0
D.nic[1]   0.015  0.013   198  0.7333  0.0020   0.0182       0
```

Copies run at both the L2 and the decoders' NICs. `llmd_nixl_pull.sq`
under the same run gives `ttft` 0.02553; this is one seed of a short run,
not the 41-49% of #599's study, which this document does not reproduce.

## What moved

- Every program with one connector runs as before: the reports of every
  pair of `examples/pairs.json` (seed 1, horizon 200) and the run logs of
  `vllm_lmcache.sq`, `llmd_nixl_pull.sq` and `vllm_nixl_push.sq` are the
  same byte for byte, but for one case below.
- The deployment figure draws an engine's loads as ways from the
  admission, of which a request takes one, as an `if`'s arms are drawn
  (`Walker::alternatives`); one load is drawn as before, and
  `docs/assets/*.deployment.svg` do not move.

### A load of no tokens is no load

The one case: a read whose send has ended was the request's load
whatever its amount. A decoder's request preempted after its read is
admitted again with its load `request.preempted ? 0 : …` asking for
nothing, and its send was ended by the read; the admission found the send
gone and set `E.load_done` to the new admission. `llmd_nixl_pull.sq`'s
`send_held` (`D.load_done - t1`) then measured the prefiller's blocks as
held until the re-admission (`--blocksD 1000`, horizon 300: mean 0.03220,
now 0.03216; p99 0.1112, now 0.1079). Under several connectors the same
test would have skipped the L2 for that request, where vLLM's NIXL
answers 0 and the next connector is asked.

*Why it happened.* The test for a send gone ran before the amount, on the
assumption that a request whose connector reads a source always reads it.
The amount is the program's statement of whether it reads; it now comes
first, and a send gone matters only to a load that has tokens
(`Interp::request_hit`). The other test of a send gone, when a chosen
read's copy would start (`Interp::request_next`, a push's read waiting for
its leg), already comes after the choice.

*Should the language have refused the program?* No: the program was right
and the interpreter wrong. `tests/connector.rs`
`a_preempted_request_that_loads_nothing_keeps_its_read_s_end` reproduces
it.

## Decisions

1. **One `CConnector` with ordered `loads`**, not a list of connectors:
   the request's records are one set.
2. **The order of the L2s and the amounts are the program's, the rule
   vLLM's** (criterion 2, above).
3. **One `save`**, refused in the text otherwise.
4. **A queue named once**: its `load` and `save` in one connector.
5. **`E.from` -1 passes over the source's load only**, and the next is
   asked.
6. **The source's read is the first load**, in a pull as in a push,
   checked when the program links.
7. **No fix-it** for a missing source connector: where it stands among
   the others is the program's.

## Self-critique

**A list of connectors in the IR (`Vec<CConnector>`)**, each with its own
`loaded`, `saved` and `load_done`. A request uses one of them, so the
others' slots would equal the chosen one's or be unset, and every reader
of the IR would have to know which (criterion 0).

**A selection clause** (`load by (…)`, a `choose` over connectors). The
amounts already say it; a second spelling (above).

**Every saving connector saving, one after the other.** A request runs
one transfer at a time (`docs/design/request-state.md`): two saves would
add their times where vLLM overlaps them, a wrong answer. **At once**
would need a second transfer per request. No program needs either: in
vLLM's documented setups the point-to-point side is the relation's send,
not a save. The limit is the parser's message; a program that needs two
L2s saving lifts it with concurrent transfers.

**A free order in a pull.** Nothing waits on a pull's read, and which
store is asked first looked like policy. But a read passed over keeps
the prefiller's send, and with it its blocks, until the send's `ttl` or
the session's end, where vLLM frees them at once (F1). The Before with
`connector lmc` written before `connector P` (horizon 300, warmup 30,
the check switched off): `ttft` 0.02633 (±0.00153) becomes 0.08848
(±0.04739): `P.kv[0]`'s use averages 71672 tokens where it was 368, its
cache falls from 148524 to 75286, and the prefiller's mean
reuse falls from 4505 tokens to 1806, its prefill rising from 1428 to
4118. One order of two clauses would give that answer silently
(criterion 0), so the order is a push's until F1.

**Ending an unchosen pull read's send at once**, as NIXL notifies the
prefiller when it is told 0 tokens. It is the right rule, but a change of
its own for one connector too (a load of 0 tokens keeps the send today):
follow-up F1, after which a pull's order can be free.

**Keeping the test for a send gone before the amount**, so that every
single-connector run stays byte for byte. It keeps a wrong `E.load_done`
and would skip the L2 for a preempted request; above.

## Follow-ups

- **F1.** A pull read whose amount is 0 keeps the prefiller's send until
  its `ttl` or the session's end; vLLM's NIXL frees it by notification
  (`nixl/pull_scheduler.py:145-147`). A push's keeps it until its lease
  ends, which serQ matches. With it, a read that is not chosen ends its
  send too, and a pull's connectors can stand in any order.
- **F2.** A save keeps its request, and the session, waiting until it is
  done; vLLM returns the response in the step the request stops
  (`scheduler.py:2153-2159, 2199-2216`) and keeps only the blocks until
  the save is done (`scheduler.py:2653-2655`, the saves counted by
  `multi_connector.py:525-554`). `llmd_nixl_pull.sq` with `connector lmc
  { per_token (5e-6); load (…); save (floor(request.input, 256)); }` on
  the prefiller, which links: `ttft` 0.02553 becomes 0.03012 (horizon
  300, warmup 30), and the prefiller's `finished` stay 0.0121 becomes
  0.0197. `llmd_nixl_lmcache.sq` therefore saves on the decoder only.
- **F3.** Two connectors that save. #599 asked that every connector with
  `save` save, as vLLM does (`multi_connector.py:334-346`); here one
  saves, and a second `save` is refused (*Every saving connector
  saving*, above). Lifting it needs a request to run its saves at once,
  a second transfer per request (`docs/design/request-state.md`).
