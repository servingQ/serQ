# A queue's engine keeps its requests' state

#555's steps 3 and 4 ([the scheduler and the worker](scheduler-worker.md))
made a top-level engine keep a request's state and gave it a KV connector.
A queue's engine still did not: its entries wrote the hold and the runs
themselves, with the five lines step 3 removed (`kept`, `known = max(kept,
computed + 1)`, `prefill (known - c)`, `decode (o - 1 - (known - prompt))`),
and a decoder with remote KV wrote its NIXL read inside that hold. This
document is the record of the change that lets a queue's engine take its
requests as any engine does (#592's follow-up).

## In vLLM

A decode instance behind a disaggregating proxy is one vLLM engine. A
request whose prefill ran elsewhere carries `kv_transfer_params`
(`do_remote_prefill`), and the scheduler asks the NIXL connector how many
tokens it can read beyond the local hit
(`nixl/pull_scheduler.py:59-64`, `nixl/push_scheduler.py:119-124`,
`scheduler.py:950-953`). The request is given blocks for them and no new
work (`scheduler.py:1069-1072`) and waits, holding the blocks and no
running slot, in `WAITING_FOR_REMOTE_KVS` (`scheduler.py:1264-1267`). When
the KV has arrived it waits again as any waiting request, and when it has
KV for every token it computes its last again to sample the next
(`scheduler.py:3075-3076`). A request without remote KV is scheduled as on
a colocated engine.

## In serQ

`examples/pd-disaggregation/llmd_nixl_pull.sq`'s decode instance.

Before:

```serq
  queue D[ND] : decode {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksD * bs; }
    engine on gpu {
      reqs cap max_seqsD;
      tokens cap B;
      scheduler { advance running; admit waiting while (running.preempted == 0); }
      …
    }
    …
    decode (prompt) from P {
      set produced = min(o, max_model_len - prompt);
      set transferred = 0;
      set kept = prompt;
      hold kv (cost(kv, known)) reserve (cost(kv, known)), reqs (cost(reqs, 0)) reserve (cost(reqs, 1))
           reuse (cost(kv, reqs, floor(known - 1, bs)))
           at admission (known = max(kept, computed + 1)) {
        set kept = known;
        set c = cached;
        if (!transferred) {
          observe remote_tokens = prompt - c;
          transfer (prompt - c) from P to kv (prompt - 1 - c);
          mark read_done;
          set transferred = 1;
          set c = prompt - 1;
        }
        hold reqs (cost(reqs, 1)) {
          run D prefill (cost(D, known - c)) growing kv;
          run D decode (cost(D, produced - 1 - (known - prompt))) growing kv;
        }
      } cache (cost(kv, reqs, prompt + produced));
    }
  }
```

After:

```serq
  queue D[ND] : decode {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksD * bs; }
    engine on gpu {
      reqs cap max_seqsD;
      tokens cap B;
      connector P { load (request.preempted ? 0 : request.input - request.hit); }
      scheduler {
        advance running;
        admit waiting while (running.preempted == 0)
          taking kv (request.loaded > 0 ? request.known : min(request.known, request.hit + budget_left(D)))
                   reserve (request.known),
                 reqs (request.loaded > 0 ? 0 : 1) reserve (1)
          reuse (floor(request.known - 1, bs))
          cache (request.input + request.output);
      }
      …
    }
    …
    decode (prompt) from P {
      set produced = min(o, max_model_len - prompt);
      D (prompt, produced);
    }
  }
```

- **A queue's engine admits by `taking`**, as any engine, and its entries
  call it, `D (prompt, produced);`; the server and the gateway call the
  entry, not the engine (refused with the entry's form). An engine family's
  `taking` names the family's pools without an index: each member takes
  its own (`D.kv[j]` for `D[j]`), as its memory is its own.
- **A connector may name its queue's KV relation**: `connector P` in an
  engine whose queue reads KV `from P` (`kvt.nixl_pull (P, D, …)`,
  `kvt.nixl_push`). Its load is the read the entry wrote before: the
  poster's wait, one flow over P's NIC and D's (the relation's `share`),
  the tokens' KV into the request's hold, and the lease of the request's
  source ended; in a push it first waits for the prefill leg (`join`). It
  reads from the member the request's entry took `from` (the entry sets
  `D.from` before its call, which the IR's `CConnector::from` names), and
  a request whose entry took no `from` loads nothing (vLLM's
  `do_remote_prefill`): one engine serves both of llm-d's ways in. It has
  no `per_token` (the NICs' rates are the relation's) and no `save`.
- **After a load, the last token again.** A request with KV for every token
  it has computes its last again, as vLLM does after any connector's load
  (`scheduler.py:3075-3076`): a load counts its tokens, and the language
  takes the one back. `vllm_lmcache.sq`'s L2 loads all but the last token
  already, so it does not move.
- **After a load, the admission again.** The admission's pools are read
  again, with `request.loaded` 0, and those the request took none of and
  now takes it waits for, in their queues, as a part of its hold: the
  program says a request reading its KV takes no running slot, `reqs
  (request.loaded > 0 ? 0 : 1) reserve (1)`, and the slot it takes once the
  KV is there is the second admission. A program that writes `reqs (1)`
  holds its slot through the load (`vllm_lmcache.sq`, as before), so this
  is policy the program states (criterion 2). The part goes with the
  request's hold at its end and at a preemption.
- **Records.** Every record is an attribute its caller reads after the
  call: `E.reused`, `E.prefill`, `E.admitted`, `E.first`, and a connector's
  `E.loaded`, `E.saved` and `E.load_done`, when its latest load ended
  (llm-d's `lease`, the push program's `parked`). The per-admission ones are
  observations of the report too. The marks the entries wrote
  (`read_done`, `parked`, `written`, `first_token`, `admitted`) and the
  observations of what the engine records (`remote_tokens`, `local_tokens`,
  `d_cached`, `reused_tokens`, `prefill_tokens`) go.

The four programs that wrote the five lines (`thunderagent.sq`,
`pd_batching.sq`, `llmd_nixl_pull.sq`, `vllm_nixl_push.sq`) give the reports
they gave before line for line but for the observations, the events
included (seed 1; llm-d colocated and split, `pd_batching.sq` split and
colocated), and the deployment figures are the same but `pd_batching.sq`'s,
which now draws the decoder's slot pool as `llmd_nixl_pull.sq`'s did. Two
differences, both in what is observed: ThunderAgent's `hit` is now one per
call, read after it (`E.reused > S`, its last admission's), where it was
one per admission; and the engine's records now count the remote requests
too (a remote request's `D.prefill` is the one token it recomputes).

`RunMode::Decode` is now in no program's text (`tools/unused.json`): every
decode is an engine request's, which the interpreter runs in that mode.

## IR

`CConnector` gains `load_done` (an attribute slot) and `from` (the
attribute a KV relation's request's entry sets; `None` for an L2's), and
`RequestRecords` the attribute slots of `reused` and `prefill`. A
connector's blocks are written per member of an engine family. IR 13 has no
tag, so the version does not move; the coming tag's message names these.

## Self-critique

**The read as a block at the call** (`D (prompt, produced) loading {
transfer (…) from P to kv (…); }`). The source is the call's, so the call
could carry the read; but the block is a procedure in the request again,
what step 3 removed, and the request's state across a preemption (read
once, then recompute) would be the program's to keep. The engine's
connector says what vLLM's engine is configured with, and the call says,
by its entry's `from`, where the request's KV is.

**`E.from` as an attribute.** The source member is per request, as vLLM's
`kv_transfer_params` are. An IR field on `CStmt::Request` would say it
without an attribute, but every consumer of the request would carry a
reference only the NIXL connector reads; the entry already knows the
member, and an attribute the expansion sets is the kernel's way to pass it.
Its cost: the IR's check that a `release` names a lease the session took
is syntactic, and the connector's `release P.kv[D.from]` is not the
`lease` the source's entry wrote (`P.kv[i]`); the check accepts a
connector's release of its relation's source, which `relations` in the
parser guarantees.

**The second admission as a mechanism without the program's say.** vLLM
always parks a loading request without a slot; the language could do the
same. But `vllm_lmcache.sq` models its L2 copy holding the slot (an
approximation its comment states), and a program should be able to state
either; reading the admission again costs nothing a program does not write.

**Recomputing the last token as the program's.** It is no policy: a
request that has KV for every token must compute one to sample, and vLLM
does it after every connector's load. A rule every program would write the
same is the language's.
