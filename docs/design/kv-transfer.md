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

## What comes next

The L2 cache of #522 is the second connector: a queue that holds KV and
serves copies at its bandwidth, an engine that reads it at admission and
writes it when a request ends. It goes into `std/kvt` beside NIXL, with the
`cache` role and the engine's read of a cache queue's pool in the
language.

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
