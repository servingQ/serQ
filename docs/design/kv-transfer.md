# The KV-transfer library

A connector moves KV between an engine and something outside it: NIXL
between a prefiller and a decoder, LMCache or Mooncake between an engine
and a cache server. What the copy is made of (a run over links, a load
into the destination, maybe a release of the source) is the language's;
which connector does which copy, in which direction and who waits before
it, is the connector's. This document moves the connectors' vocabulary
into a library, `std/kvt` (KV transfer), beside `std/args`. Issue #526:
parse-time sugar, no IR change.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.sq` at `02b4a41`, line 175, and
`vllm_nixl_push.sq`, line 147:

```serq
  D pull P latency x0 share maxmin;
```

```serq
  P push D latency x0 share maxmin;
```

`pull` and `push` were keywords of the language, so NIXL's two modes were
written as if every deployment had them. A program that models another
connector (#522: an engine reading from and writing to an LMCache server)
would add its own keywords next to them, and the language would grow one
form per connector.

## After

```serq
use "std/kvt";
…
  share maxmin;
  kvt.nixl_pull (P, D, latency = x0);
```

```serq
use "std/kvt";
…
  share maxmin;
  kvt.nixl_push (P, D, latency = x0);
```

The three programs' IR, and their runs at `--horizon 3000 --seed 7`, are
byte-identical to `02b4a41`.

## The design

**The language keeps the mechanism, the library names the connector.**
`transfer (X) from P to Q (n)`, the `nic` a queue owns, the `share` that
divides links and the delay before a copy stay the language's: a program
that writes its own copy uses them directly. A connector is a way of
combining them, and the library gives it the name vLLM registers it under
(`NixlPullConnector`, `NixlPushConnector`:
`vllm/distributed/kv_transfer/kv_connector/factory.py:184-192`).

| Surface | Kernel |
|---|---|
| `kvt.nixl_pull (P, D, latency = x);` | `stage D.nic.latency[N] : delay;`, `x` the constant `D.pull.time`; a `transfer` without `on` in `D`'s entry `from P` is the read over `P.nic[i]` and `D.nic[j]` after `D`'s wait ([the pull relation](pull-relation.md)) |
| `kvt.nixl_push (P, D, latency = x);` | the same copy over the same NICs, the wait `P`'s ([the push mode](push-mode.md)) |

The constants keep the names the relations gave them (`D.pull.time`,
`P.push.time`), so the IR is the same.

**std/kvt is the parser's.** As `std/args` is, the module is a namespace
the parser defines: a connector is a function there, and adding one is
parser code, a grammar line and a section of the spec, as a keyword was.
What changes is where it lives: the core grammar and its keywords do not
grow with the connectors, and a reader sees which words are a vendor's.

**The source first.** Both functions take the source, then the reader:
the order the bytes go, whichever side posts the copy. The old keywords
were sentences, `D pull P` and `P push D`, and their word order was their
meaning; in a call the same place would take a different role by the
function's name.

**The program's `share`.** Concurrent copies divide the NICs by the
program's `share maxmin;` or `share bottleneck;`, one policy for every run
over several stages. A program with a relation must write it: the policy
is not left to a default. The relation no longer names it, so a second
connector does not have to repeat it and agree.

**Checks.** Those of the relation, with the call written in the message
(`` `kvt.nixl_pull (P, D)`: queue `P` has no `nic` ``). Besides:

- a `kvt.…` call without `use "std/kvt";`;
- a function the library does not have, and an argument the function does
  not take (`share = …` says it is the program's);
- a reader with no entry `from` the source, which is the queues the wrong
  way round, at the call rather than at the gateway;
- a relation in a program without `share`;
- `D pull P …;`, which says what it is now;
- a declaration named `kvt` beside the import.

`pull` and `push` are no longer keywords; a program may name a thing with
either.

## The L2 cache

Issue #522. An LMCache server, or a Mooncake store behind it, holds KV
outside the engine for every engine that talks to it. At vLLM v0.31.0
(the connector; `ref/vllm` 0c87a197 for the scheduler, which v0.31.0
contains), LMCache's multi-process connector:

- is asked, when the scheduler admits a request, how much of the prompt it
  holds past the GPU's prefix hit (`scheduler.py:941-954`), and answers in
  whole chunks, and nothing for a preempted request
  (`lmcache_mp_connector.py:758`, `:775`, `:788`);
- gives the request blocks for that KV and none for new work, frees its
  slot while the KV is copied in, then admits it again
  (`scheduler.py:1069-1072`, `:1267`, `:3081-3091`), and recomputes the
  last prompt token after a full copy (`scheduler.py:3075-3076`);
- stores each step's full chunks as they are computed
  (`lmcache_mp_connector.py:296-366`).

### Before

A link queue with a pool links, and every use of it is refused: an
engine's request reading it (`` `vllm.request` reads `lmc.kv`, which is
not a pool or stage of `vllm` ``), the server holding it (`` `kv` is a
pool of queue `lmc`, and only `lmc`'s entries hold it ``), and a link's
entry, which only moves (`` a transfer names the pool it takes from and
the pool it fills ``).

### After

`examples/multi-turn/vllm_lmcache.sq`:

```serq
  queue lmc : l2 {
    device host { kv cap l2_cap; }
    pool kv on host { block chunk; evict lru; }
    serve ps(1);
  }
  …
      kvt.l2_load (lmc, kv, got, per_token = r);
      …
      kvt.l2_store (kv, lmc, floor(prompt + output, chunk), per_token = r);
```

| Surface | Kernel |
|---|---|
| `queue C : l2 { pool kv …; serve …; }` | the pool `C.kv` and the stage `C` |
| `kvt.l2_load (C, P, n, per_token = r);` | `run C (cost(C, n * r)); load P (cost(P, n));` |
| `kvt.l2_store (P, C, n, per_token = r);` | `hold C.kv (cost(C.kv, cachedin(C.kv))) { grow C.kv (cost(C.kv, m)); run C (cost(C, m * r)); load C.kv (cost(C.kv, m)); } cache (cost(C.kv, n));`, `m = max(0, n - cached)` |

A request may write the load's two statements itself (`serq ir` is the
same), and `kvt.l2_load` stays beside them, as `transfer` stays beside
`run; load; release`: it says which copy the two are. The store has one
spelling, since an entry may not hold another queue's pool.

The language gains the role and one reach across the queue boundary: an
entry or an engine's request reads an L2 queue's pool and copies through
its stage. The library gains the two copies. No IR changes: both lower to
`hold`, `grow`, `run`, `load` and `cache`, so the Lean generator sees
nothing new.

**An L2 is a queue.** It is a station with memory and bandwidth of its
own, as LMCache's server is a process of its own, so it is declared as
one, not as a pool beside the engine's. The `serve` is its bandwidth, and
`ps` makes loads and stores divide it.

**The store holds what the cache has and grows by the rest.** A header is
read at its hold's admission, and a request's length is hidden from the
scheduler: a store whose header read `prompt + output` would be refused.
Taking `cachedin(C.kv)` in the header and growing in the body reads the
length where a body may.

**The program says what the scheduler asks.** How much to load, from
which tokens, and that a preempted request loads nothing are the
program's: `vllm_lmcache.sq` writes them as one `def` its admission and
its body both read. The body reads the cache again rather than the
admission's binding (a body sees only bindings over attributes and
constants), at the instant the body starts, which is the admission's.

**What the model leaves out**, in the program's header as well:

- a store is one copy when the request ends, where LMCache stores each
  step's chunks; the request keeps its slot and blocks while it copies;
- a request copying in keeps its slot and goes on to its prefill, where
  vLLM frees the slot and admits it again (`scheduler.py:877-878`);
- the copies do not slow the iteration they overlap (HBM is not shared);
- a lookup is never deferred to a later step;
- the cache's entry is the session's, as a pool's is: a prompt shared by
  sessions is not found (#374);
- the L2 is one queue; Mooncake's shards are one pool and one bandwidth.

**The numbers.** `vllm_lmcache.sq` at `--horizon 600` with 2000 GPU
blocks computes 735 prompt tokens per turn, against 4334 in `vllm.sq` with
the same blocks, and its time to first token is 0.034 s against 0.131 s:
a hit of about 3300 tokens copies in at `r = 5e-6` s per token in about
0.017 s, where computing it takes about 0.07 s. Over seeds 1 to 5 the
prompt tokens are 714 to 766 against 3497 to 5889, and the time to first
token 0.028 to 0.046 s against 0.103 to 0.243 s. One session in each run
asks for more than the GPU holds and ends at `kv`, the same in both.

**Checks.** An L2 queue with an engine, a `nic`, an entry, another role,
more than one pool or no `serve`, or as a family; a `kvt.l2_…` naming
something that is not an L2 queue declared above; a store whose `n` or
`per_token` reads `cached` or `computed`, which inside its copy are the
cache's; no `per_token`; an
argument the statement does not take, or one twice.

## Self-critique

**The relation stays core, with an L2 keyword beside it.** The first
draft of #522 added `l2 P { … }` and `store kv to P;`. One keyword per
connector is the growth this library stops, and a program could not tell
which of two keyword families was the language's rule and which a
vendor's.

**`transfer (…, from = P, to = Q, keep_from = 0)` everywhere.** Named
arguments for the core `transfer` were considered. Every other form of
the language takes trailing clauses (`reserve (…)`, `latency x`, `on S`),
and changing one form alone gives a program two grammars; changing all of
them is its own change. A library call takes `latency = x` where a link's
`serve` takes `latency x`: the call's grammar is a call's, and this is the
price of putting the vocabulary in a namespace.

**`keep` for the copy that does not release.** `keep` is a word #136
retired (`keep` is now `cache`); giving it a new meaning would make an old
program mean something else rather than fail.

**`share = …` on the call.** The first version of this change kept the
relation's `share` as a named argument. It set the program's one policy
from inside a call, two spellings of one thing, and two connectors would
each have had to name it and agree.

**The reader first in a pull.** The first version kept the old keywords'
word order, reader first in a pull and source first in a push. Swapped,
the call linked and the error came at the gateway's call, naming neither
the relation nor its order.

**`kvt.pull` without the connector's name.** A shorter name reads as the
only way to pull. vLLM registers NIXL's two modes as connectors of their
own (`factory.py:184-192`), and naming the connector keeps the library
open to a second one with the same direction.

**`l2 P { … }` and `store kv to P;`, with `CPool.store` in the IR.** The
first draft of #522. A write-through on every iteration needs the engine
to wait for its copies, an IR change; the request-level store is the
kernel's, and the iteration is left alone.

**Copies inside the iteration.** A draft made loads and stores
synchronous, the iteration waiting for them. LMCache copies on its own
stream, so a load is the request's wait outside the batch, which a run in
the request already is.

**A `copy` form, or `transfer … keep`.** A copy that does not release is
what `kvt.l2_load` lowers to; a new core form for it would be a second
`transfer`. `keep` is retired (#136).

**A `link` with a pool.** It links today, and every use of it is refused;
admitting them for links would let any NIC hold KV. A role says what the
queue is.

**A family of caches.** Mooncake shards a store across nodes. The program
would pick the shard of a session, a key the store chooses; one pool is
what the scheduler sees.

**The role named `cache`.** The first version. A request's body already
says `cache (prompt + output)`, `cached` and `cachedin` of the GPU's
prefix cache, and a reader would have to tell by the place which cache a
word meant; `l2` names the tier, as `kvt.l2_load` does.

**A path stage, `on = S`.** The first version took a stage of the path
held with the cache's (a GPU's PCIe), as `transfer on` does. No program
has more than one engine on one L2 yet, where the path would matter, and
a construct no program uses is a path only tests take; it waits for that
program.
