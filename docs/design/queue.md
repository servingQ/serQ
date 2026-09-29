# Queues: a station declared whole

Everything a request passes through inside a deployment is a queue: a
waiting line, an admission, a service, memory of its own. The router, the
prefill instances, the NIC and the decode instances of
`programs/llmd_pd.seq` are four of them, and the program wrote them as
pools, stages and `admit if … fit where …` at the call site. This document
is the design of `queue`, RFC #72, as it shipped: one construct, four roles,
three rules, all of it parse-time sugar over IR v5. Before is
`programs/llmd_pd.seq` at `a8e8370`; After is the program in the repository.

## The problem

The `server` block of `llmd_pd.seq` held the code of two repositories. The
llm-d router's (`choose j`, the `remote` decision) and the sidecar's (wait
for the prefiller's answer, then send the decode request), and vLLM's
scheduler's: `admit if reqsP[i] (1), kvP[i] (…) reserve (prompt) fit where
…` once for the prefiller and twice for the decoder (remote, local), each a
copy of the engine of `programs/vllm.seq`. The oracle checks `vllm.seq`;
nothing said the three copies were the same program. A reader who came for
the router passed vLLM's admission three times. The link's cost, `x0 +
(prompt - c) / Bw`, is the NIC's and sat in the router's body.

## The design

**One construct.** A queue owns pools, a stage (`serve kind;`, named after
the queue) and one *entry* per verb of the roles it plays. An entry's body
is the server's statements: `admit if`, `prefill`, `decode`, `transfer`,
`set`, `observe`, `branch`, `choose`. `stage` and top-level `pool` stay
the kernel's forms; the client's `tool` is a stage, since it is the
client's thinking time and not a station of the deployment.

```
item  := queue NAME [ '[' expr ']' ] [ : ROLE [, ROLE]* ] { qitem* }   -- expr a let constant: queue D[ND]
qitem := pool NAME { poolopt* }                  -- the queue's own; only its entries hold it
       | serve kind ;                            -- the Q of admit via Q, budget_left(Q), work(Q)
       | VERB [ ( NAME, ... ) ] [ from NAME ] block
stmt  += QUEUE [ '[' expr ']' ] . VERB ( expr, ... ) [ from QUEUE [ '[' expr ']' ] ] [ to POOL ( expr ) ] ;
       | mark NAME ;                             -- in an entry: now, read by the caller as Q.NAME
expr  += QUEUE [ '[' expr ']' ] . NAME            -- from outside, reads only: holders(D[j].kv), D[j].first_token
       | self                                     -- a member's own index; a bare Q inside Q is Q[self]
```

**Four roles**, the serving vocabulary's, and a queue says which it plays.

| Role | Entries | |
|---|---|---|
| `gateway` | `route` | where the workload's `request;` enters; its body is the server; one per program |
| `prefill` | `prefill (prompt)` | computes the prompt; how the KV is left (`lease`, `keep`, a transfer) is the entry's |
| `decode` | `decode (prompt)`, `decode (prompt) from Q` | told apart by the `from` |
| `link` | `transfer (n)` | the body sees `n`; `from S to P (m)` are the call's, and the linker writes `load P (m); release S` after the run |

**Three rules.**

1. *An entry sees its own.* The header — the units, `reserve`, `reuse`, the
   `where` bindings — reads the parameters, the queue's pools and stage and
   the `let` constants. The body also reads `now`, `cached`, the context
   variables and the request's `hidden` attributes; any other session
   attribute is an error at the entry. This is the `hidden` rule
   generalised to the queue's boundary, and why `decode (prompt)` has no
   `o`: the scheduler knows `max_tokens`, not the length, and the body
   alone reads `o`.
2. *An entry's `set` is the queue's; its `observe` is the program's; a
   moment is a `mark`.* `set c = cached` becomes the attribute `P.c`;
   `mark first_token` is `set D.first_token = now`, read by the gateway as
   `D[j].first_token`. The gateway keeps the session's names, since it sets
   what the session reads back (`prompt`).
3. *`from P[i]` is the pool `P`'s entry leases.* The queue is the handle;
   there is no value for a leased allocation. A `from` on a queue whose
   entries lease nothing does not link. The rule is at the call, not on the
   role: a `prefill` entry that ends in a transfer (push mode) or a `keep`
   (store and forward) is a program's to write.

**The kernel.** Every construct is the parser's.

| Surface | IR v5 |
|---|---|
| `queue Q[N] { pool p {…} serve k; … }` | `pool Q.p[N] {…}; stage Q[N] : k;` |
| `Q[i].verb (e, …)` | the body in place, parameters substituted as a `where` binding is (no `~`), `set` names renamed |
| `Q[i].decode (…) from S[k]` | the body's `src` := `S.kv[k]` |
| `L[j].transfer (n) from src to kv (m)` | `run L[j] (t); load Q.kv[i] (m); release src`, `t` the link entry's `run (…)` |
| `mark x;` / `Q[i].x` | `set Q.x = now;` / the attribute |
| the gateway's `route` | the `server` block; `request;` splices it |

`IR_VERSION` is 5, `serving-queue-theory`'s generator is untouched, and
`tools/oracle/*.ir.json` did not move: the oracle programs
(`vllm_request.seq`, `vllm_replay.seq`) are not written with queues in this
change.

## Before and After

Before (`programs/llmd_pd.seq` at `a8e8370`, lines 42–62 and 112–158): four
pools and four stages named by suffix, and the router's `branch (remote)`
holding the prefiller's admission, the decoder's remote admission and the
decoder's local admission, with the link's cost in the transfer statement:

```
pool reqsP[2] { cap max_seqsP; admit via P; }
pool kvP[2] { cap blocksP * bs; block bs; evict lru; preempt lifo; }
…
    admit if reqsP[i] (1), kvP[i] (min(prompt, hit + budget_left(P[i]))) reserve (prompt) fit
          where hit = min(cachedin(kvP[i]), hitmax) {
      set c = cached;
      prefill on P[i] (prompt - c) growing kvP[i];
    } keep (prompt) lease kvP[i] (inf);
    set t1 = now;
    admit if kvD[j] (known) reserve (known), reqsD[j] (0) reserve (1) fit
          reuse (floor((known - 1) / bs) * bs)
          where known = computed < prompt ? prompt : computed + 1 {
      …
        transfer[j] (x0 + (prompt - c) / Bw) from kvP[i] to kvD[j] (prompt - 1 - c);
      …
```

After is `programs/llmd_pd.seq`: the gateway's `route` has no `admit if`;
`P`'s `prefill` entry and `D`'s two `decode` entries hold vLLM's admissions
once each; `nic`'s `transfer (n)` holds the NIC's cost. The two programs
run the same: at seeds 1 and 7 the event, arrival, end and turn counts are
equal, every observable's count, mean and p99 agree to 10⁻¹², and only the
confidence intervals of `ttft` and `lease` differ (by 0.05 %), because
those two are now observed when the request ends rather than at the moment
marked, so the batch-means estimator sees them in another order. The IR is
not byte-identical: `set transferred = 0` moved from the router into the
decoder's entry (it is the pull scheduler's per-request flag,
`nixl/pull_scheduler.py:187-189`), and two `mark`s are two `set`s.

## What it earns

- The gateway's body says what the llm-d repository says; `P` and `D` say
  what the vLLM repository says. The program's boundaries are the code's.
- Three link errors that were comments: an own pool held outside its queue,
  a `from` on an entry that leases nothing, a header that reads past its
  parameters.
- No IR change.

## Self-critique

**`mark` exports one moment, not an observation.** `ttft` and `lease` are
the deployment's names and the gateway observes them; but a queue may still
`observe` its own (`local_tokens`, `remote_tokens`), so the boundary is not
clean.

**The router reads pod internals.** `holders(D[j].kv)`, `cachedin(P[i].kv)`,
`work(P[i])`. llm-d's router reads metrics and its own prefix-cache
estimate. An `expose` clause naming what a queue lets the deployment read
is the next step, and the moment the router's estimate and the pod's truth
part in the notation.

**`self` stands in for ownership.** The NIC is the decode pod's; `queue D
{ queue nic { … } }` would say so, and the flat family with `nic[self]` is
the cheaper start. Push mode shows the cost: the copy runs on the
*prefiller's* NIC, and the decoder's `decode … from src` entry cannot name
`nicP[i]` — it sees `src` and nothing else of the prefiller.

**`P` and `D` still write vLLM twice.** `D`'s local entry is `vllm.seq`'s
engine and `P`'s entry is that minus `decode`. One definition deployed
twice needs a parameterised declaration (`queue P[NP] = vllm { … }`), the
model/instance split of the [frontend sketch](frontend.md), item 4. This
document makes the unit that split will parameterise.

**No `role` declaration.** The four roles are the vocabulary; a program
cannot add one. Criterion 2 is met by the queue choosing its roles and the
entry choosing how the KV leaves; a fifth role would be an issue.

**The oracle programs are not queues.** `vllm.seq` and its three workload
variants are held to the same text by `tests/workloads.rs`, and the oracle
harness programs feed `serving-queue-theory`; moving them is its own change
and the reason the IR-identity claim is a test on small programs
(`tests/queues.rs`) and a run comparison on `llmd_pd.seq`.
