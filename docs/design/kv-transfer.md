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
  kvt.nixl_pull (D, P, latency = x0, share = maxmin);
```

```serq
use "std/kvt";
…
  kvt.nixl_push (P, D, latency = x0, share = maxmin);
```

The three programs' IR, and their runs at `--horizon 3000 --seed 7`, are
byte-identical to `02b4a41`.

## The design

**The language keeps the mechanism, the library names the connector.**
`transfer (X) from P to Q (n)`, the `nic` a queue owns, the `share` that
divides links and the delay before a copy stay the language's: a program
that writes its own copy uses them directly. A connector is a way of
combining them, and the library gives it a name a serving engineer reads
as that connector.

| Surface | Kernel |
|---|---|
| `kvt.nixl_pull (D, P, latency = x, share = s);` | `stage D.nic.latency[N] : delay; share s;`, `x` the constant `D.pull.time`; a `transfer` without `on` in `D`'s entry `from P` is the read over `P.nic[i]` and `D.nic[j]` after `D`'s wait ([the pull relation](pull-relation.md)) |
| `kvt.nixl_push (P, D, latency = x, share = s);` | the same copy over the same NICs, the wait `P`'s ([the push mode](push-mode.md)) |

**A call, with named arguments.** A library function is called, as
`args.number ("name", default)` is. The two queues are positional, reader
first in a pull and source first in a push, so the call reads in the
direction the bytes go in a push and the direction the request goes in a
pull, as the old keywords did. The parameters are named: `latency = …` is
optional, `share = …` is required, each once, in any order.

**Checks.** Those of the relation, unchanged, with the call written in the
message (`` `kvt.nixl_pull (D, P)`: queue `P` has no `nic` ``). Besides:

- a `kvt.…` call without `use "std/kvt";`;
- a function the library does not have, and an argument the function does
  not take, or one given twice;
- `D pull P …;`, which says what it is now;
- a declaration named `kvt` beside the import.

`pull` and `push` are no longer keywords; a program may name a thing with
either.

## What comes next

The L2 cache of #522 is the second connector: a queue that holds KV and
serves copies at its bandwidth, an engine that reads it at admission and
writes it when a request ends. It goes into `std/kvt` beside NIXL, with
the queue role and the copy that does not release its source in the
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
them is its own change. A library call is a call, so it takes named
arguments without making the core a second grammar.

**`keep` for the copy that does not release.** `keep` is a word #136
retired (`keep` is now `cache`); giving it a new meaning would make an old
program mean something else rather than fail.

**`kvt.pull` without the connector's name.** A shorter name reads as the
only way to pull. NIXL's pull and push differ from Mooncake's transfer
engine in who posts the copy and what is registered; naming the connector
keeps the library open to a second one with the same direction.
