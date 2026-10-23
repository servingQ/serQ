# Deployment modeling

A serQ program describes a serving deployment. Before you write one, it helps
to know the model of a deployment the language assumes, so that you can
place each part of a real system. This page starts from the outside: a
serving system is a client and a server; the deployment is the server, and
the client's behaviour, the workload, is a file apart. Then it goes inside
the server's engines: each has a scheduler that decides what each iteration
computes and owns the pools the engine admits, and a worker that turns the
scheduled batch into time. It explains how the two divide the work, where
resources are taken and given back, the one status each engine request is
in, and which invariants every program keeps.

The language reference defines each construct:
[The two sides](language.md#the-two-sides),
[Engines](language.md#engines),
[Engines on devices](language.md#engines-on-devices) and
[An engine's request](language.md#an-engines-request). This page connects them.

## A serving system is a client and a server

A serving system has two sides. The **client** sends a request and waits for
its reply; between requests it reads, thinks or calls a tool. The
**server** takes each request and replies to it: its gateways route it, its
engines compute it. serQ keeps the two sides in two files:

| Side | File | What it says |
|---|---|---|
| Server | the program, the deployment (`examples/multi-turn/vllm.sq`) | the `server` that takes each request, its gateways, queues, engines, devices and pools |
| Client | the workload (`examples/workloads/chat.sq`) | how sessions arrive, what each turn draws and sends, what the client does between turns |

The workload is not part of the deployment. One deployment runs under
several workloads, and one workload runs on several deployments; the two are
paired only when the program runs:

```sh
serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 600
```

The only thing the two sides share is the server's signature, as an API's
provider writes one. The server declares what a request carries (and, with
`reply (…)`, what its reply carries):

```serq title="examples/multi-turn/vllm.sq"
--8<-- "examples/multi-turn/vllm.sq:server"
```

and the workload binds those parameters once, with `send`, and submits a
request at each `turn;`:

```serq title="examples/workloads/chat.sq"
--8<-- "examples/workloads/chat.sq:workload"
```

Nothing else crosses: the server reads the parameters, never `K`, and the
workload reads its own attributes, and the reply when the server declares
`reply (…)`. The client's tool time
above runs on a stage of its own, not the server's.
[What crosses](language.md#what-crosses) and
[Paired files](language.md#paired-files) give the rules.

This split is what makes a result comparable. The workload is the
experiment's input and the deployment is the system under test, so changing
one leaves the other as it was. What the client saw of each turn (time to
first token, time per output token, response time) has its own table in the
report ([What the client sees](language.md#what-the-client-sees)).

## An engine is a scheduler and a worker

Every iteration of an engine runs in three steps:

1. The **scheduler** reads the running requests, the waiting queues and the
   pools. It chooses which requests advance and by how many tokens, admits
   waiting requests, and takes and gives back their resources. The result
   is a **batch**.
2. The **worker** reads the batch and returns the time its forward pass
   takes.
3. The clock advances by that time. The batch's tokens are then applied:
   prefills move forward, decodes sample a token, and finished requests
   leave.

`examples/multi-turn/vllm.sq` states each step in its own block:

```serq title="examples/multi-turn/vllm.sq"
--8<-- "examples/multi-turn/vllm.sq:running"
--8<-- "examples/multi-turn/vllm.sq:waiting"
```

```serq
--8<-- "examples/multi-turn/vllm.sq:worker"
```

| Component | serQ | What it reads | What it does |
|---|---|---|---|
| Scheduler | `scheduler { … }` | `running.…`, `waiting.…`, the pools, the engine's registers, and `request.…` in an admission | orders and advances the running requests, admits waiting ones, takes and gives back pools, preempts |
| Worker | `worker { forward (T); }` | `batch.…`, the device's time resources, constants and definitions | returns `T`, the time of the forward |

Only the batch passes from the scheduler to the worker. The worker cannot read
the residents or the queues, and the scheduler cannot read the batch's
timing. The linker enforces this split; the table in
[Engines on devices](language.md#engines-on-devices) lists which value each
clause may read.

## The scheduler takes and gives back the engine's resources

On the pools an engine admits, every allocation and every release happens at
one of the events below, and the worker never allocates or frees anything.
Each row says where the program states the amounts. Pools no engine admits
are taken and given back by the session's own statements instead: a `hold`,
`grow`, `shrink`, `release` or `drop`, retried as each instant settles
([Events and settling](language.md#events-and-settling)).

| Event | When | What happens to the pools | Where the program says it |
|---|---|---|---|
| Admission | `admit waiting` reaches the head of a queue | each pool of the admission is taken at once, all or nothing; the cached prefix is read | `taking P (units) reserve (r)`, `reuse (n)` |
| Growth | a running request's position passes its allocation | the KV pool grows block by block before the request is served; if it cannot, a victim is preempted | the pool's `block` and `preempt` |
| Preemption | a growth does not fit | the victim gives its pools back and caches what it computed; it keeps its tokens and waits again | `preempt lifo`, `preempt by (…) requeue head` |
| Release | a request has generated its output | the request gives its pools back and leaves its prefix cached | `cache (n)` |
| Connector load | admission of a request whose KV comes from elsewhere | the request is given blocks for the KV it loads and no new work until the KV arrives: a `load` transfer, while the request is `waiting_for_kv` | `connector C { load (n); }` |
| Connector save | a request has generated its output, its engine's connector saving to an L2 | the request keeps its blocks until a `save` transfer has copied them, then releases | `connector C { save (n); }` |
| Send | a request of an engine that a KV relation reads from ends | the relation registers a send on the request: its blocks stay, neither evictable nor preempted, until the reader's load has read them, the relation's `ttl` runs out before a read begins, or the session ends; a reader that comes after the `ttl` computes the tokens itself | `kvt.nixl_pull (P, D, …, ttl = t)`; without `ttl`, until the read or the session's end |

A request's blocks go back when it is finished and no transfer of it (a
save, a send) is registered or in flight.

The amounts are read at the event, from what the scheduler sees then:
`request.hit` is read when the request is admitted, and `cache` when it
releases. For each expression, the moment it is read and the values it can
read are listed in [Moments](ir.md#moments).

### Holding KV after the request ends

A prefill instance in a disaggregated deployment must keep a request's KV
after the request has finished, until the decode instance has read it. Its
entry is an engine call, `P (prompt, 1)`, and nothing in it keeps the
blocks: the KV relation `kvt.nixl_pull (P, D, …)` (or `kvt.nixl_push`)
registers a send on each of `P`'s requests when it ends. The request is
finished, its blocks kept, until the decoder's connector has read them,
which ends the send ([Prefill/decode over NIXL](use-cases/pd.md)). In a
push the prefill is a leg (`fork`), and the decoder's load waits for the
leg's send. The worker plays no part in it.

## Request statuses

A request is one call of an engine, `E (input, output)`; a disaggregated
turn is two, the prefiller's and the decoder's. It is in exactly one of
five statuses at any time:

| Status | The request |
|---|---|
| `waiting` | is in the engine's waiting queue, not scheduled; after a load, with its blocks, waiting for a running slot |
| `waiting_for_kv` | is admitted to its blocks and waits for its connector's load before it can be scheduled |
| `running` | is in the engine's batch, in prefill or decode (its mode); a resident stalled for room to grow is one |
| `preempted` | is in the waiting queue again after a preemption, until it runs |
| `finished` | has generated its output; it keeps its blocks while a save or a send of it is registered or in flight |

Its KV moves are **transfers** beside the status, each with its own time:

| Transfer | During | What it copies |
|---|---|---|
| `load` | `waiting_for_kv` | the KV it reads in at admission: from an L2, or from a source request's send |
| `save` | `finished` | its KV, to an L2, before its blocks go back |
| `send` | `finished` | its KV, kept for a reader's load, which is the same copy |

A request's blocks go back when it is `finished` and no transfer of it is
registered or in flight. A program does not read the status. The report's
[status table](reference/cli.md#request-statuses) gives the time spent in
each, and the [run log](reference/log.md) records every status and every
transfer. A program that calls no engine has no request: its stage and pool
tables say where its sessions are.

## The worker only takes time

`forward (T)` is the only item a worker has. `T` is read after the batch is
scheduled and reads the batch (`batch.…`), the device's time resources,
the constants and the program's definitions, never the residents or the
queues. Within an engine, time passes only
here. Scheduling, admission, growth and release take no time.

Work that is not the forward runs on other stages, beside the iteration:

- a KV connector's copies run at the bandwidth of its L2 queue;
- a transfer between instances runs on the NICs;
- a tool call runs on its own stage.

## Invariants

A program can rely on the following invariants. Each one says what holds it:

- *Semantics*: the language defines it so, and no check can fail;
- *Linker* or *Validation*: a program that would break it does not link, or
  its IR does not validate;
- *Run-time check*: the run ends with an error;
- *Debug check*: checked at every settled instant in a debug build, which
  `make check`'s test run is; a release `serq run` does not check it;
- *Lean*: a theorem of the Lean model, within the fragment it covers.

### Time and iterations

1. **Only the batch passes from the scheduler to the worker.** `forward`
   reads `batch.…` and nothing of the residents or the queues. The scheduler
   reads nothing of the forward's time. *Linker* (of the text; the IR's
   `Step` moment also allows the resident count).
2. **Decisions take no time.** Statements, admissions, growths, preemptions
   and releases happen at an instant. Time passes only at stages: an
   engine's iteration, a `fifo`, `ps` or `delay` stage, a NIC.
   *Semantics* ([Events and settling](language.md#events-and-settling)).
3. **An iteration sees every event of its instant.** An engine starts an
   iteration only after every event at that instant has been handled, so an
   arrival at the same instant is in its waiting queue. *Interpreter.*
4. **An iteration that schedules tokens lasts a positive time.** A `forward`
   that evaluates to 0 on a batch with tokens ends the run with an error.
   A step that only preempts may last zero time. *Run-time check*
   ([Every instant settles](language.md#every-instant-settles)).
5. **The batch fits the token budget.** An iteration schedules at most
   `tokens cap` tokens: a decoding request gets one, a prefilling request at
   most its `each at most`. *Semantics* ([Engines](language.md#engines)).
6. **A loop lets time pass.** Every path through a `loop` or `while` reaches
   a `run` with work, a `hold` whose body does, or `end`. A schedule that can
   neither advance nor admit does not link. What the linker cannot see is
   reported at run time: `stuck` sessions and `idle: engine …`
   ([the report](reference/cli.md)), and a session that can never complete
   ends the run blocked, exit 3 ([exit status](reference/cli.md#exit-status)).
   *Linker and run-time check.*

### Resources

7. **Admission is all or nothing.** The head of a queue is admitted only when
   every pool it takes has room for `max(units, reserve)` beside what is
   allocated, and, on a pool with `reserve held`, beside what earlier holds
   reserved and have not yet allocated. A cached prefix never blocks an
   admission: it is evicted to make room. On an engine's pool the iteration's
   budget, `while` and `only` may hold it back too. *Semantics*
   ([Pools](language.md#pools)); *Lean* for the cache
   (`SerqLang.admit_guard_units_only`).
8. **Memory is bounded.** On every pool, allocated plus cached is at most
   `cap` in every reachable configuration. A pool that preempts its leases
   at ticks may pass `cap` between ticks, and then nothing is cached.
   *Lean* for the model's one-pool commands (`SerqLang.Step.invariant` in
   `lean/Serq/Core.lean`); *Debug check* for the interpreter.
9. **Every unit is accounted for.** A pool's `used` equals what its holds and
   leases have allocated, with a prefix that several holds read counted
   once. Its `cached` equals the cached entries of its trie. A pool has one
   holder per hold and per lease (a send is one), a lease set aside by `suspend` excepted. An allocation released twice, or never
   released, fails this check even when the pool stays within its cap.
   *Debug check.*
10. **A reservation is kept.** On a pool with `reserve held`, what is
    allocated plus what live holds have reserved but not yet allocated fits
    `cap`, so a hold that grows within its reservation always finds room.
    *Debug check.*
11. **An allocation has one owner.** `load` and a `growing` run act on the
    enclosing hold of the same pool; `grow`, `shrink` and `release` act on it
    or on a lease the session took. A hold leases only its own pool. A
    request's blocks kept for a send are its own until the reader's load,
    which reads the send its copy chose, ends it; a source has one `ttl`
    however many relations read it. *Validation*
    ([Validation](ir.md#validation)) and the *Linker*.

### Requests

12. **A request keeps its tokens, not its KV.** A preempted request gives its
    pools back, caching what it computed. It keeps every token it has
    (`request.known`) and what it has generated, and recomputes what the
    cache no longer holds when it is admitted again. *Semantics*
    ([An engine's request](language.md#an-engines-request)).
13. **A request samples when its KV covers its tokens.** The prefill computes
    `request.known` less what the admission read from the cache. When it is
    done, the request samples a token. A request that loads KV for every
    token computes its last token again in order to sample. *Semantics.*
14. **Token counts are whole.** Inputs, outputs, budgets, chunks, KV units
    and a connector's amounts are whole numbers, provably so from the
    expressions that compute them. *Validation*
    ([Token counts](language.md#token-counts)).
15. **An expression reads only what its moment supplies.** For example, a
    hold's header is read again at every admission attempt, so it may not
    draw. An `only` predicate may not read `now`. A gauge reads no session.
    *Validation* ([Moments](ir.md#moments)).
16. **A request has one status at a time.** No statement of the program
    runs inside an engine request: its connector's moves are transfers of
    a fixed shape, a leg is a request of its own, and nothing joins. The
    status is read from the engine's own state of the request. *Semantics*;
    `tests/request_state.rs` checks every example's log.

## What the model leaves out

These assumptions affect how you read a result:

- **Scheduling is free.** The scheduler's CPU time is not modelled. Its cost
  appears only if the program adds it to `forward`.
- **Releases take effect at once.** A release frees its blocks for the next
  iteration.
- **An empty step costs a forward.** A step that only preempts lasts
  `forward` at zero tokens, where an engine may skip the forward.
- The prefix cache, leases and the Lean fragment have further limits, listed
  under [Known limitations](language.md#9-known-limitations).
