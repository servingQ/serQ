# A key reads what the scheduler sees

A `hidden` parameter is the target's, not the scheduler's
([`hidden`](../api/workload.md#hidden)). `Program::validate` refuses the
attribute itself in every key a pool or an engine reads of a session
(`src/ir.rs`, the `CExpr::Attr` arm of `Validator::expr`), at every moment
but a session statement's and a claim's `given`. An attribute set from it
passed: the server could copy `o` into `y` and order its queue by `y`
(#554, found in the review of #550). The linker's `hidden_in_server`
(`src/frontend/link.rs`) follows what is set from a hidden attribute until
a run reveals it, and refused the copy in a server `if`, a `choose`, a
hold's header and an engine's admission, but not in a key, nor where a
preempted hold reads its header again. This document
makes a key follow the same rule as the `if`, where the scheduler reads
the session. A stricter check: no IR field moves, and `IR_VERSION` does
not.

## Before

The workload of each program below:

```serq
workload { arrive poisson(1); turn { Size o = ~exp(10); } send (o); }
```

The hidden attribute itself in a queue key does not link (`serq check`
at d50ac83):

```serq
fn main() {
  pool kv { cap 30; queue by (o); }
  stage A : fifo;
  server (hidden o) {
    hold kv (cost(kv, 1)) { run A (cost(A, o)); }
  }
}
```

```text
pool `kv`: `o` is hidden from the scheduler, but is read in a pool's queue keys, read before selecting a waiting session
```

A copy of it, set by the server, linked:

```serq
fn main() {
  pool kv { cap 30; queue by (y); }
  stage A : fifo;
  server (hidden o) {
    set y = o;
    hold kv (cost(kv, 1)) { run A (cost(A, o)); }
  }
}
```

```text
OK: 1 pool(s), 1 stage(s), 11 attribute(s), 4 block(s)
```

So did one the workload's turn set and sent, `turn { Size o =
~exp(10); set x = o; } send (x, o);` to `server (x, hidden o)` with
`queue by (x)`, and the copy `y` in `evict by (y)` and `preempt by (y)`,
and, in `examples/multi-turn/vllm.sq` with `set y = output;` before the
request, in `advance running by (y)` and `advance running only (y < 1e9)`.
The same `y` and `x` read by a server `if` did not link:

```text
link error: `y` is set from the hidden `o` in the server, but the server's `if` reads it before a run reveals it
link error: `x` is set from the hidden `o` in the workload's turn, but the server's `if` reads it before a run reveals it
```

## After

Each copy is refused in validate's words for the attribute itself, at
the key's moment (`ir::Moment`'s `Display`):

```text
link error: pool `kv`: `y` is set from the hidden `o` in the server, but is read in a pool's queue keys, read before selecting a waiting session
help: a run whose work reads `o` reveals it when the run ends (the end of a decode is the EOS the scheduler sees), and from there a key may read what was set from it; before, the key is the scheduler reading what it cannot see
link error: pool `kv`: `x` is set from the hidden `o` in the workload's turn, but is read in a pool's queue keys, read before selecting a waiting session
link error: engine `vllm`: `y` is set from the hidden `output` in the server, but is read in an engine's `advance running by (…)` keys or an `only`, read for each running session
```

A hold whose body sets the copy its header reads, on a pool that
preempts (`pool kv { cap 30; preempt by (1); }`), is refused in the
words of the header check, since a preempted hold is admitted again:

```serq
server (hidden o) {
  set y = 1;
  hold kv (cost(kv, y)) { set y = o; run A (cost(A, o)); }
}
```

```text
link error: `y` is set from the hidden `o` in the server, but the server's admission of a preempted hold reads it before a run reveals it
```

and a copy set once a run has revealed `o` links, in any key:

```serq
server (hidden o) {
  run A (cost(A, o));
  set y = o;
  hold kv (cost(kv, 1)) { run B (cost(B, 1)); }
}
```

No program in the repository reads a copy of a hidden attribute in a key
(thunderagent's lease keys read `reasoning` and `prompt`), so every
example and tutorial links as before and no generated file moves.

## The rule

A key, a hold's header and a lease's time are checked where the
scheduler reads the session, on the attributes as they stand there, as
the `if` is checked where it stands:

| Read | Moment | Checked |
|-----|--------|---------|
| a pool's `queue by` | `Select` | at a `hold` on the pool; during a request to an engine that takes the pool or admits from it; wherever the session waits in a hold on a pool that preempts with `requeue tail` (the victim's hold goes back to its queue as a newcomer, `Interp::preempt`) |
| a pool's `preempt by` | `Victim` | wherever the session waits in a hold on the pool; during a request to an engine that takes it |
| an engine's `advance running by`, `advance running only`, `admit waiting only` | `Serve` | during a request to the engine, before its end reveals the output |
| an engine's `taking`, `reuse`, `prefix`, its connector's `load` | `Request` | at the request (as before; `load` is new) |
| a pool's `evict by`, its `spill … when` | `Evict` | everywhere the session is (below) |
| a pool's `preempt leases by`, `suspend by` | `Victim` | everywhere the session is |
| a hold's units, `reserve`, `reuse`, `prefix`, its pools' index | `Admit` | at the hold (as before); on a pool that preempts, wherever its body waits too (new) |
| a hold's `lease` time | `Session` | at the hold (as before) and at its end (new) |

Under `requeue head`, the default, and `preempt lifo`, a victim's hold
goes back ahead of the keys: `Interp::next_waiter_among` returns a
resumed head without evaluating them, so a copy set in the hold's body is
never read by them. Its header is, under either: `Interp::preempt`
evaluates the units and the pools' index again (`Interp::wanted`) and
the `prefix`, and each admission it tries evaluates the units, `reserve`
and `reuse` (`Interp::pending_now`), on the attributes the session has
then. A lease's time is read where the lease starts, at the hold's end
(`Interp::end_hold`).

"Wherever the session waits" is a run, a hold's queue, an engine
request, a `grow` and a `load`. "Everywhere the session is" adds each
release (a hold's end, `release P;`, a `shrink`), the server's end, and
the workload's own waits and releases between requests (a `tool` call,
a hold and its end, a `release`, a `shrink`): an eviction key reads the
session that wrote the entry wherever it is now (`Interp::node_writer`),
or the attributes its release saw once it has ended, and a lease, which
outlives the hold that took it, is a candidate while its session is
anywhere. A release reads it twice over: the entry it writes keeps the
attributes as they are then, and what it frees admits what waits
(`Interp::try_admit_all`), whose admission may evict an entry this
session wrote, a release of the workload's own pool included.

The hidden attribute itself is left to `Program::validate`, which
refuses it at every moment and covers an IR not written as text: the
new check skips it, as the hold-header check does.

## Why the linker

The IR does not tell the server's statements from the workload's, so
`Program::validate` cannot know which attributes were set from a hidden
one by the scheduler's side before a run revealed it. The linker walks
the program as the parser expanded it and already keeps that state for
the `if`; the keys read the same state, so the rule is written once.

## Self-critique

**Every preempting pool's queue keys at every wait.** The first
version put every `preempt by` pool's queue keys in the waits of a hold
on it. Under `requeue head` they never read the victim, and
`set y = 1; hold kv (cost(kv, 1)) { set y = o; run A (cost(A, o)); }`
with `queue by (y); preempt by (1);` was refused with "read before
selecting a waiting session", which is false of that program (the review
of this change): only `requeue tail` puts the hold back among the keys.

**The header at the hold only.** The hold-header check read the header
where the hold is written, and the lease's time there too. A preempted
hold reads its header again where its body waits, and a lease's time is
read at the hold's end: `set y = 1; hold kv (cost(kv, y)) { set y = o;
run A (cost(A, o)); }` under `preempt by (1)` linked on main, as did
`hold kv (…) { run A (…); set y = o; } lease kv (y);`.
Both are checked now where they are read, and the check at the hold
stays: a `lease` time is still refused there, where it is not read, so
`hold kv (…) { run A (cost(A, o)); } lease kv (o);` does not link though
the lease reads `o` after `A` has revealed it. Dropping that check is a
loosening of its own, left to a separate change.

**The workload's waits only.** The workload's walk first gathered the
session where it waits (a `tool` call, a `grow`, a `load`, a hold's
queue), and the server's walk checked a release and a hold's end but
not a `shrink`. A `release`, a `shrink` and a hold's end free a pool
and admit what waits, and the admission may evict the deployment's
entry the session wrote, whose key reads it there: `set x = o; release
c; set x = 1;` in the session, on the workload's own pool `c`, linked
with `evict by (x)` on the deployment's `kv` (the review of this change).
They are gathered now, on both sides.

**One set of names for every key.** A simpler rule collects every name
set from a hidden attribute anywhere in the server and refuses it in
every key. It would refuse
`set y = o; run A (cost(A, o)); hold kv { … }` with `queue by (y)`,
where the queue reads `y` after `A` has revealed `o`: a key would be
stricter than an `if` at the same place, and its message ("read before
selecting a waiting session … before a run reveals it") would be false.
Criterion 0 puts one rule on both.

**The check in `Program::validate`.** It is where the attribute itself
is refused, and would cover IR written by hand. It cannot see what the
server set before a run, which needs the server/workload split the IR
does not keep; adding it to the IR to move this check would cost every
consumer for a rule the text frontend can state.

**Leases tracked by the hold that took them.** A lease is the session's
until it ends, and the walk could carry which pools the session leases.
But a lease outlives the request: thunderagent takes it in one turn and
is a candidate during the next turn's tool call, which one walk of the
server does not see. The lease keys are checked everywhere the session
is instead, which is never weaker.

**A release is checked when it happens.** The entry a release writes
keeps the attributes as they are then, and an eviction key reads them
once the session has ended; by then a later run may have revealed what
they were set from. A program that sets a copy just before a release,
sets it again, and reveals the hidden attribute after
(`hold kv (…) { …; set y = o; } set y = 0; run B (cost(B, o));` with
`evict by (y)`) is refused, though the key never reads an unrevealed
value: the entry holds `y = o`, read only after `B` has ended. Following
each entry to the session's end would carry a set of snapshots through
the walk; no program sets a copy where a key reads it.

**A request's release and a connector's `save` are not checked.** Both
are read at `Moment::Release`, where the server may cache by a hidden
length: every vLLM program's `cache (request.input + request.output)`
does. Refusing a copy there while `request.output` passes would be a
third rule. The asymmetry with `Program::validate`, which refuses the
attribute itself at `Release`, stays open.

**The message names the last place a copy was set.** A server that
copies a name the workload set from `o` (`turn { set x = o; }`, then
`set y = x * 2;` in the server) is told `` `y` is set from the hidden
`o` in the server ``: the verdict is right, but the reader looks for `o`
in the server and finds `x`. The `if`'s message has the same gap; naming
the chain is a separate change to both.

**A fork's leg.** A leg runs on a copy of its parent's attributes and
holds none of its parent's pools (`Interp::fork`), so its waits read the
eviction and lease keys and not the preempt keys of the parent's holds.
An eviction key reads the root session's attributes (an entry's writer
is found by its serial), so checking the leg's copy there is stricter
than it need be; no program's key reads a copy, in a leg or elsewhere.

**Across requests.** The server's walk starts each request from what the
workload hands it, so what the server set from `o` in one request and a
key reads in the next (`set y = o;` at the server's end, `queue by (y)`)
is not followed; nor is it for the `if`. The key checks here read what
the walk reads, and the end-of-server check catches the eviction and
lease keys only. Carrying the server's end into the next request is a
change to both rules, and a separate one.
