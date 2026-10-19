# The prefiller takes its request as an engine

> **Folded into [one status per request](request-state.md), its last
> clause not shipped.** The prefiller's entry is `P (prompt, 1);` as below,
> but its `taking` does not end `lease kv (inf)`, and the IR has no
> `CRequest.lease`: the relation `kvt.nixl_pull (P, D, …)` makes `P`'s
> requests *sent*, each keeping its blocks until the reader's load ends
> the send or the relation's `ttl` does (`CRequest.send`). The lease wrote
> in the program what the relation already says, and its one parameter,
> how long an unread send lasts, is vLLM's setting of the connector, so it
> went to the relation. The record below is the change as #715 proposed it.

[A queue's engine keeps its requests' state](queue-engine-request.md)
moved the P/D decoder to the engine's request: its entry is `D (prompt,
produced);`, and its engine's `taking` and `connector P` say the
admission and the read. The prefiller stayed a hold in its entry, the
five lines the engine's request had removed from every other program,
because the one thing its hold says that a `taking` could not is `lease
kv (inf)`: the prompt's blocks stay held after the request ends, until
the decoder has read them. This document is the record of the change that
lets the `taking` say it.

## In vLLM

The prefiller of a P/D pair is one vLLM engine with the NIXL connector.
The proxy sends it the prompt with `max_tokens = 1`
(`disagg_proxy_pushconnector_demo.py:227-279`; llm-d's sidecar does the
same): the request computes the prompt in chunks, samples one token and
finishes. When a request finishes, the scheduler's `_free_request` asks
the connector's scheduler side what to do with its blocks
(`scheduler.py:2636`), and skips freeing them when it answers
`delay_free_blocks` (`scheduler.py:2653-2655`). The NIXL connector answers
so for every request with blocks, "Prefill request on remote. It will be
read from D upon completion", and gives them a time to live,
`kv_lease_duration`, 30 s by default (`nixl/pull_scheduler.py:248-262`,
`nixl/push_scheduler.py:260-271`, `nixl/base_scheduler.py:81-85`), which
the decoder's heartbeats renew while it waits. The worker side only
reports the blocks sent (`finished_sending`), and the scheduler frees them
then (`scheduler.py:3135-3138`).

So the hold-until-read is a decision of the scheduler, about a request
that has finished, made when the request finishes; the worker does
nothing but say when the copy is done.

## In serQ

`examples/pd-disaggregation/llmd_nixl_pull.sq`'s prefill instance.

Before:

```serq
  queue P[NP] : prefill {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksP * bs; }
    engine on gpu {
      reqs cap max_seqsP;
      tokens cap B;
      scheduler { advance running; admit waiting while (running.preempted == 0); }
      worker {
        forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
      }
    }
    pool reqs on P { queue fifo; }
    pool kv on gpu { block bs; evict lru; preempt lifo; }
    nic ps(BwP);
    prefill (prompt) {
      hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, reused + budget_left(P)))) reserve (cost(kv, prompt))
           at admission (reused = min(cachedin(kv), floor(prompt - 1, bs))) {
        set c = cached;
        run P prefill (cost(P, prompt - c)) growing kv;
      } cache (cost(reqs, kv, prompt)) lease kv (inf);
    }
  }
```

After:

```serq
  queue P[NP] : prefill {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksP * bs; }
    engine on gpu {
      reqs cap max_seqsP;
      tokens cap B;
      scheduler {
        advance running;
        admit waiting while (running.preempted == 0)
          taking reqs (1),
                 kv (min(request.known, request.hit + budget_left(P))) reserve (request.known)
          reuse (floor(request.known - 1, bs))
          cache (request.input)
          lease kv (inf);
      }
      worker {
        forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
      }
    }
    pool reqs on P { queue fifo; }
    pool kv on gpu { block bs; evict lru; preempt lifo; }
    nic ps(BwP);
    // the prompt, and one token sampled and discarded (llm-d's prefill
    // request has max_tokens = 1)
    prefill (prompt) {
      P (prompt, 1);
    }
  }
```

- **`lease P (t)` closes a `taking`**, after `cache`, as it closes a hold
  after `cache`: the request keeps its allocation of `P`, one of the pools
  it takes, past its end, neither evictable nor a preemption victim, until
  a `release` of it, `t` seconds, or the session's end. It is written on
  the scheduler's `admit waiting`, the scheduler's side, as vLLM's
  `_free_request` decides it; the `worker` says nothing about it.
- **`cache` is read when the request ends** and applied when the lease
  does. A lease ends inside the decoder's connector, when the session's
  request is the decoder's, so `cache (request.input)` read then would
  read the decoder's request; the interpreter reads it at the request's
  end and keeps the number with the lease (vLLM's `_connector_finished`
  is asked at the request's end too, `scheduler.py:2636`).
- **The entry is the request**: `P (prompt, 1);`. A request for one token
  samples it at its prefill's end and decodes nothing; the engine's
  records (`P.reused`, `P.prefill`, `P.first`) are the caller's after the
  call, as the decoder's are.
- **`from P` takes what the request leased.** A queue's entry that calls
  its engine, whose `taking` leases `kv`, leases `kv`, as a hold's `lease`
  in it does, so `decode (prompt) from P` links and the decoder's
  `connector P` ends `P.kv[D.from]` as before.
- **A request's lease nothing ends does not link** (below).

The other two programs that wrote the prefiller's hold move the same way.
`vllm_nixl_push.sq`'s prefiller is the same engine; its entry's `observe
p_cached = c;` is now the engine's record `P.reused`, which the report
gives one per admission. `pd_batching.sq`'s prefiller has `exclusive
prefill` and no prefix cache: its `taking` is `reqs (1), kv
(min(request.known, budget_left(P))) reserve (request.known) lease kv
(inf)`, with no `reuse` and no `cache`, as its hold had none, and the
gateway's `set t_prefill = P.prefill_done;`, a `mark` at the end of the
hold's run, is `set t_prefill = now;` after the call, the same instant.

The reports are the same line for line but for the observations, seed 1:
`llmd_nixl_pull.sq` split (`thr` 512), every request split (`thr 0`),
colocated (`thr 1e9`) and with the decoders' memory short (`thr 0 blocksD
300`), horizon 600; `vllm_nixl_push.sq` with `concurrent` 1 and 0;
`pd_batching.sq` split, colocated (`mode 0`), and split with the decoder's
memory short and the NIC slow (`blocksD 300 Bw 2e4 x0 0.01`, 100
preemptions on `D.kv[0]`), horizon 100. The observations that move are
the prefiller's records, which the hold did not keep, and the push
program's `p_cached`, which is `P.reused` with the same numbers:

```
llmd_nixl_pull.sq (thr 512):
> P.reused    1819  4721.5481  ±320.9617   0.743  14912.0000
> P.prefill   1819  1352.9615   ±60.9935   0.359   3590.0000
vllm_nixl_push.sq (concurrent 1):
< p_cached     836  5159.1770  ±578.5813  0.848  20704.0000
> P.reused     836  5159.1770  ±578.5813  0.848  20704.0000
> P.prefill    836  1408.0000  ±203.9927  1.720   8639.0000
```

`tests/prefiller_lease.rs` holds the first: `llmd_nixl_pull.sq` with the
prefiller written either way gives the same report but those two lines.
The deployment figures are the same: the figure draws a request's leased
pool held past it, as a hold's, and an engine whose requests generate one
token as a prefill instance (its decode mode is drawn only where a
request's output is not 1).

`RunMode::Prefill` is now in no program's text (`tools/unused.json`), as
`RunMode::Decode` has been since the decoder moved: every prefill is an
engine request's, which the interpreter runs in that mode. Nor is `at
admission (…)`, which no example, tutorial or companion program writes any
more; it is parse-time sugar, which `tools/unused.json` does not list, and
it stays for this change (a follow-up removes it or finds the program that
needs it).

## Should the language refuse a lease nothing reads?

A request's lease is KV kept for another instance's read, which ends it.
One that no `release` in the program names keeps its blocks for nobody
until the session's end: the prefiller's memory fills with what no
decoder will read, and the run says nothing. A static check sees it:
`Program::validate` refuses a `CRequest.lease` on a pool no `Release` in
any block names, with the relation and the connector to write. A
`kvt.nixl_pull (P, D, …)` and the reader's `connector P` write that
release, so every P/D program passes. A hold's lease is not checked so: a
hold's lease is also ended by `preempt leases by` and is grown and shrunk
outside the hold (ThunderAgent's), which a request's lease is not.

## IR

`CRequest` gains `lease: Option<(CRef, CExpr)>`, the hold's field, read at
`Moment::Release`. A reader that ignored it would free the prefiller's
blocks at the request's end and run another program, so it is a field an
old reader needs: IR 14 is tagged (v0.1.6), and the version moves to 15.
The Lean generator pins 15; a request with a lease is outside the
fragment, as a hold with one is, and no oracle program has one, so a 14
file and a 15 file translate alike. The oracle, claims and regression IR
files move only their version.

## Self-critique

**A `connector` on the prefiller's engine** (`connector D { lease (inf);
}`). This was the other spelling weighed, and the closer to vLLM's
objects: the delay is the NIXL connector's answer, a connector on the
prefiller's side. It was rejected on three counts. *One meaning, one
spelling*: a pool kept past the holder's end is a `lease P (t)` already
(a hold's), and the connector's form would be a second spelling of the
same IR, `CRequest.lease`. *Unambiguity*: `connector C` names where the
engine's KV comes from (an L2's tokens, a relation's source); on the
prefiller it would name where the KV goes, one word for two directions.
*Nothing new to say*: the relation `kvt.nixl_pull (P, D, …)` names the
reader already, and the connector's form would still need the pool and
the time, which is the `lease` clause. Both are on the scheduler's side;
the `taking` is where the scheduler's admission and release of a request
are written, and `_free_request` is that release.

**No text at all: the relation implies the lease.** `kvt.nixl_pull (P,
D, …)` could make every request of `P`'s engine keep its memory until a
reader's connector ends it. It fails criterion 2: the time to live is a
setting (`kv_lease_duration`), and a program could not state a finite
one, or none; and criterion 1: the prefiller's text would not say that
its blocks outlive the request.

**The lease in the `worker`.** The worker is where the copy is posted, so
the lease could be said there. vLLM's worker does not decide it: it
reports `finished_sending`, and the scheduler frees
(`scheduler.py:3135-3138`). A `worker` clause would put a scheduler's
decision on the wrong side.

**`cache` read when the lease ends.** A hold's `cache` is read then, with
the session's attributes; a leg's lease keeps the leg's attributes for
it. A request's `cache` reads `request.…`, the request's values, and the
request is gone when the lease ends, so the number is read at its end.
Reading the clause late would read the decoder's request when the
decoder's connector ends the lease: the same number in these programs
(both inputs are the prompt), another in a program whose decoder reads a
part.

**Refusing a lease nothing reads, rather than warning.** A lint would let
the program run. But the run is not slower, it is wrong: blocks that no
decoder reads count against the prefiller's memory, and nothing in the
report says why. CLAUDE.md's rule is that a program that can only hold
what nobody reads should not link.

**Moving `at admission` out of the language here.** It is now in no
program, and the rule says a construct no program uses goes. It is a
frontend form with its own design record and tests; removing it is its
own change, not this one's. That change removed it, with `mark`
([Two forms no program used](unused-header-forms.md)).
