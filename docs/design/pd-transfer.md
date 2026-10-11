# The KV transfer: a hold whose blocks outlive it

The prefill/decode split of llm-d over vLLM's NIXL connector, checked
against the source (llm-d `8a2f37d`, the router `13eebdb`, vLLM `0c87a197`),
and what the language had to gain to state it. Before is the repository as
it was; After runs (`examples/pd-disaggregation/llmd_pd.sq`, `tests/pd_semantics.rs`,
`docs/use-cases/pd.md`). IR version 5.

## What the systems do

**Pull mode** (`NixlConnector`, the llm-d P/D guide's deployment). The
router's endpoint picker runs the decode profile first and picks a decode
pod; a decider compares the prompt's uncached suffix on that pod with a
threshold and, above it, runs the prefill profile for a prefill pod
(`disagg_profile_handler.go:316-379`, `prefix_based_pd_decider.go:266-303`).
The decode pod's sidecar sends the prompt to the prefiller with
`max_tokens = 1`, waits for the answer, and only then sends the decode
request to its own engine (`connector_nixlv2.go:69-232, 261-379`). On the
prefiller, the request's slot is freed when its token is sampled and its
blocks are *leased*: `request_finished` returns `delay_free_blocks`
(`nixl/pull_scheduler.py:191-292`), the blocks stay allocated
(`scheduler.py:2628-2657`) until the decoder's read completes
(`scheduler.py:3135-3138`) or the lease expires (30 s, granted at
`nixl/pull_scheduler.py:248-269`, reaped at `nixl/base_worker.py:2982-3008`,
extended by the decoder's heartbeats while the request waits:
`nixl/base_scheduler.py:199-238`, `nixl/base_worker.py:3141-3170`, `3010-3030`;
the design note is vLLM's `docs/design/nixl_kv_cache_lease.md`).
On the decoder, the scheduler takes the request from its waiting queue at a
step with budget left and a running slot free (`scheduler.py:872-879`),
allocates blocks for the whole prompt beyond its local hit
(`nixl/pull_scheduler.py:34-106`, `scheduler.py:1214-1226`), and parks it,
`WAITING_FOR_REMOTE_KVS`, holding the blocks and no slot
(`scheduler.py:1264-1294`); the worker reads the KV over NIXL; the read
done, the blocks are cached, the last prompt token is marked uncomputed and
the request is back in the waiting queue, served before new arrivals
(`scheduler.py:3032-3092`, `2383-2385`), for a slot and one token of budget.

**Push mode** (`NixlPushConnector`, `docs/design/nixl_kv_push_connector.md`
in vLLM). The decoder allocates and *registers* its blocks with the
prefiller (`nixl/push_scheduler.py:128-205`); the prefiller writes the KV
when it has both the finished blocks and a registration
(`nixl/push_scheduler.py:207-294`). The router may send the two legs at
once (vLLM's `disagg_proxy_pushconnector_demo.py:227-270` does; the llm-d
sidecar does for MoRI-IO only, `connector_nixlv2.go:60-67`), so the decoder
can allocate *during* the prefill and the write starts the moment the
prefill ends. Through the llm-d sidecar's serial dispatch the two modes have
the same lifecycle and differ in which side's worker moves the bytes and by
one notification.

**Neither is store-and-forward.** In both, the destination's blocks exist
before the bytes move and the source's blocks are freed after. There is no
buffer on the link.

## Before

`examples/pd-disaggregation/lecture_pd.sq` holds the prefill instance's memory through the
transfer and queues for the decode instance's afterwards:

```
enter memP (kappa * T) { prefill S; transfer (x0 + kappa * T / Bw); } keep (kappa * T);
enter memD (kappa * T) { decode (o * w); }
```

That is a link with a buffer, and it gets the coupling backwards: while the
decoder has no room, the real prefiller fills up with leased blocks and
stops admitting; the lecture's prefiller keeps working and the sessions
queue for `memD` holding nothing. The scoped `hold` cannot write the real
thing: `memP` is held from the prefiller's admission to the end of the read,
`memD` from the decoder's admission to the end of the decode, and the second
begins before the first ends without ending after it. Two scopes either nest
or are disjoint, and the prefiller's request is *finished* — out of its
scope — while its blocks are still its own.

Two smaller things the program also could not say: the prefiller frees the
request's *slot* while its *blocks* stay (one hold on `reqsP, kvP` ends both
at once), and the tokens a transfer delivers count as computed on the
decoder (`keep` and `cached` count only what a `growing` run computed).

## After

One clause on a hold, two kernel statements, one serving form.

```
hold P (u), … { … } cache (ℓ) lease P (t);   // P's allocation outlives the scope: until the session's release of it, t seconds, or its end
release P;           // the enclosing hold's allocation on P, or the session's lease of it, given back now, caching per keep
load Q (n);          // the KV of n tokens arrived: the enclosing hold's computed position on Q advances by n
transfer (w) from P to Q (n);   // = run link (w); load Q (n); release P;
```

The transfer of `examples/pd-disaggregation/llmd_pd.sq`, from the server's side:

```
admit if reqsP[i] (1), kvP[i] (min(prompt, hit + budget_left(P[i]))) reserve (prompt) fit
      where hit = min(cachedin(kvP[i]), hitmax) {
  set c = cached;
  prefill on P[i] (prompt - c) growing kvP[i];
} keep (prompt) lease kvP[i] (inf);       // finished on P: the slot goes, the blocks wait for the decoder's read
admit if kvD[j] (known) reserve (known), reqsD[j] (0) reserve (1) fit
      reuse (floor((known - 1) / bs) * bs)
      where known = computed < prompt ? prompt : computed + 1 {
  set known = computed < prompt ? prompt : computed + 1;
  set c = cached;
  branch (!transferred) {
    transfer[j] (x0 + (prompt - c) / Bw) from kvP[i] to kvD[j] (prompt - 1 - c);   // takes the lease
    set transferred = 1;
    set c = prompt - 1;
  }
  admit if reqsD[j] (1) fit {
    prefill on D[j] (known - c) growing kvD[j];
    decode on D[j] (o - 1 - (known - prompt)) growing kvD[j];
  }
} keep (prompt + o);
```

The program reads in the order the request travels: the prefiller's
scope, the decoder's scope, and between them the one line that says what
the prefiller's `}` does *not* free. Every line is one thing the source
does ([the P/D use case](../use-cases/pd.md) has the table). `lease kvP[i] (inf)` is
`delay_free_blocks` with a lease the decoder's heartbeats renew; `30` would
be a prefiller nobody heartbeats. `reqsD[j] (0) reserve (1)` is the
decoder's gate for a parked request: there must be a free running slot,
and it takes none. `transferred` is `do_remote_prefill`, spent after one
transfer, so a request preempted on the decoder afterwards recomputes
locally.

A lease is an allocation without a scope: it stays in `used`, it is not
evictable, and it is not a preemption victim (no hold to unwind). It ends
in one of three ways, all of them finite — the session's `release` of the
pool (a `transfer … from` it), the expiry, or the session's end — and
`cache` applies then. The linker lets a `release P` stand outside any hold
of `P` when some hold of the program leases `P`; inside a hold it ends the
innermost hold's allocation on `P` first. `load` must fit the allocation:
vLLM's decoder allocates the whole prompt before it reads, and a program
that wants growth writes `grow` first.

Two consequences in the interpreter, both readings of rules the language
already claimed:

- `preempt lifo` names vLLM's `running[-1]`. A holder away from the engine
  is in no `running` list — the prefiller's leased request, the decoder's
  parked one — so the victim is the holder that is a resident of the stage
  the pool is the memory of and was admitted last, by the session's latest
  admission: the decoder's request took its place in `running` when it
  queued again for a slot, not when its blocks were allocated. A pool that
  is no engine's memory preempts its last holder, as before.
- A stage that serves several queues tries them in declaration order and
  stops at the first head that does not fit. The decoder's requests whose
  KV has arrived (`reqsD`) are declared before the new ones (`kvD`), which
  is `skipped_waiting` before `waiting`. Without it the program deadlocked
  at 2 000 decode blocks: a new request that did not fit blocked the parked
  one that only needed a slot.

And one in the linker with no IR change: a pool family served `admit via`
a stage family of the same count is served member for member, and a step
family's `memory` names its pool family's members, so `pool reqsD[2] {
admit via D; }` and `stage D[2] : step { memory kvD; }` mean what they say.

## Why these and not others

**Not `acquire`/`free` as separate statements.** That was the lecture's
language, and v2 folded them into a scope so that balance is syntactic.
A lease is the one allocation that outlives its scope, and it is bounded
three ways where a
free `acquire` was bounded by nothing: the lease names its pool at the
scope, its expiry is a number, and the session's end collects it.

**Not the decoder's hold nested inside the prefiller's.** That was the
first form of this change: `release reqsP` inside the prefiller's scope,
the decoder's `admit if` inside it, `release kvP` inside the transfer. It
is the same IR, and it reads as the wrong thing — a decoder admitted
*inside* a prefiller's request — when what happens is a prefiller's request
that ends with its blocks still pinned. The lease says that at the `}`
where it happens, and the two scopes stand in the order the request
travels.

**Not a timed lease alone.** vLLM's first design was a single 480 s
timeout on the prefiller, and its lease note names the two failures: a
crashed decoder pins gigabytes for minutes, and a short timeout frees
blocks a queued decoder was about to read. The lease here ends at the
transfer first and at the bound second, and the bound is a number the
program chooses.

**Not a `move P -> Q` node with its own admission.** One statement that
admits at `Q`, transfers and frees `P` would need a preemption rule of its
own (re-executing it would read from a freed `P`), and would hide the
decoder's two admissions — blocks first, slot after — which are the point.
The three statements are each one line of the scheduler.

**Not `growing` on the link run.** `growing` means the allocation grows as
tokens are computed at a step stage; the link has no tokens and the
decoder allocated already. `load` is the arrival of computed KV, and it
serves an offload tier's reload the same way.

**Not a rule that a hold's pool is released when the session leaves the
engine.** The lease is the program's to state; a prefiller that recomputed
instead of leasing would be a different program, and the language may not
choose between them.

## Self-critique

- **Push mode's concurrent legs were not written here.** A session waited
  at one pool at a time, so the decoder's admission was written after the
  prefill, which is the serial dispatch. [The push mode](push-mode.md)
  writes them as two legs of the request (`fork`, `join`), not as the
  reservation sketched here, which is a fork whose leg is one hold plus a
  second meaning of hold:

  ```
  book kvD (prompt) reserve (prompt);            // join D's queue; the scheduler allocates when it gets there
  admit if reqsP (1), kvP (…) fit { prefill on P (…) growing kvP; } keep (prompt) lease kvP (inf);
  enter kvD { transfer (w) from kvP to kvD (n); … }   // open the booking, waiting if it is not granted yet
  ```

  It is one more kernel statement and a second kind of pending admission in
  the interpreter (a session queued at a pool while it runs elsewhere; a
  granted booking holds units before its scope opens and is not a
  preemption victim). Priced at a version, not written until an oracle
  can check it: under decoder memory pressure the serial and the
  concurrent forms both lease at the prefiller, and differ by the decoder's
  blocks being taken a prefill earlier and one decoder step of latency.

- **No scheduler oracle yet.** `tools/vllm_oracle.py` drives one scheduler
  with a fake model runner; a P/D oracle drives two, with a fake connector
  that completes a read after a chosen number of steps, and checks the
  parked request's admission step, the prefiller's free step and the
  decoder's first-token step. The P/D use case's table is checked against
  the source by line; the program's answers are not yet checked against the
  scheduler's.

- **The heartbeat is a number, not a construct.** The lease's bound is
  one expression: `inf` for a prefiller whose lease the decoder renews
  every 5 s (`nixl/base_worker.py:3141-3170`), `30` for one nobody renews.
  A decoder that dies mid-wait, which is what the renewal exists for
  (`nixl/base_worker.py:2982-3008`), is not a session the language has.

- **`lease`, `load` and Lean.** None of the three is in the Lean
  fragment. A lease is the `[Free]` transition deferred to a later event,
  and `load` moves the position `SerqExec.lean` does not keep per pool yet.
  The generator fails on all of them, so no oracle program is affected.

- **The figure.** Two enclosures that cross at the link station is the
  right picture and the layout draws them at one depth, with their glyph
  columns close. A deployment with three instances in a row would want the
  crossing box on its own row.

## Cost

| Consumer | Gains | Pays |
|---|---|---|
| interpreter | the lease coupling, xPyD families, the decoder's two admissions | leases on the session with an expiry event; one release path shared with the scope's end; the victim rule; the queue order made explicit |
| Lean | nothing yet; the fragment refuses the clause and both statements | the generator's pin moves to 5 (one line in `gen_serq_oracle.py`) |
| oracle | nothing; the seven IR files change only in `version` and a `null` lease | regeneration (`make oracle-ir`) |
| reader | `lease`, `release`, `load`, `transfer … from … to …`; the two-line rule for the victim and the queue order | one clause, two statements and one form to learn |
