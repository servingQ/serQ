# Admission reads what is set above

A hold's header is read when the request is admitted, before the hold's
body runs, and an engine's admission (`taking`, `reuse`, `prefix`, a
connector's `load`) when its scheduler admits the request the call `E (…)`
makes. An attribute the
server sets keeps its value on the session from one request to the next,
so until this request sets it, it holds what an earlier request left there,
or 0 at the session's first. A header that read what its own body set
therefore read 0 or a stale value, and the program linked (#504). This
document refuses it: what the server sets is read at the admission only
once it is set above the hold, or the call, on every way to it. A stricter
check: no IR field moves, and `IR_VERSION` does not.

## Before

A server whose hold's header reads `m`, which only the hold's body sets
(the issue's `body_only.sq`), with chapter 2's workload
(`docs/tutorial/programs/workloads/02-memory.sq`, one turn per session,
`send (work_size, items)`):

```serq
fn main() {
  pool mem { cap 10; }
  stage svc : fifo(4);
  server (work_size, items) {
    hold mem (cost(mem, m)) {
      set m = items;
      run svc (cost(svc, work_size));
    }
  }
}
```

At serQ f8483cd:

```text
$ serq check body_only.sq --workload docs/tutorial/programs/workloads/02-memory.sq
OK: 1 pool(s), 1 stage(s), 12 attribute(s), 4 block(s)
$ serq run body_only.sq --workload docs/tutorial/programs/workloads/02-memory.sq --horizon 2000
pool  used  cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  ----  ------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
mem    0.0     0.0  0.000    0.811  0.0000    1595         0         0        0      0    0      0
```

Every session makes one request, so the header read `m` before anything
set it, and the hold took 0 units. With `set m = items;` above the hold
(`above.sq`) the same command gives `mem 2.8`, a queue of 0.083 and a wait
of 0.1039. Two more forms linked: `set m = items;` in one arm of an `if`
above the hold, and `set m = items;` after the hold.

An engine's admission had the same hole. `examples/engines/tgi.sq` with
`taking kv (request.input + extra - 1)` in place of `request.input + mnt -
1`, and `set extra = mnt;` after `tgi (input, output);`, linked, and its
admission read 0 for `extra` on each session's first turn: `kv used`
2485.6 against the example's 2515.3
(`--workload examples/workloads/tgi.sq --horizon 2000`).

The entry form refused the first case, on the entry's own statements
(#390, #475, #503), with "the admission header reads local `kept`, which
the entry does not set above this hold". An entry expands into exactly the
server form that did not.

## After

```text
$ serq check body_only.sq --workload docs/tutorial/programs/workloads/02-memory.sq
body_only.sq: 6:25: link error: `hold mem`: the admission header reads `m`, which is not set above this hold on every way to it; the header is read when the request is admitted, so it would read what an earlier request left there, or 0, not what the hold's body or a later statement sets
6 |     hold mem (cost(mem, m)) {
  |                         ^
help: set it above the hold, in both arms of an `if` and not in a loop, `while` or `fork` that ends before it, or write its value in the header; to mean an earlier request's value, copy it into another attribute above the hold and read that
```

```text
$ serq check tgi_after.sq --workload examples/workloads/tgi.sq
tgi_after.sq: 51:5: link error: `tgi (…)`: the engine's admission (`taking`, `reuse`, `prefix`, a connector's `load`) reads `extra`, which is not set above this request on every way to it; the admission is read when `tgi`'s scheduler admits the request, so it would read what an earlier request left there, or 0, not what a later statement sets
51 |     tgi (input, output);
   |     ^^^
note: `tgi` reads it at 37:38
help: set it above `tgi (…)`, in both arms of an `if` and not in a loop, `while` or `fork` that ends before it
```

A gateway that passes an entry what it sets only after the call (`gw.sq`,
whose `E.decode (p)` holds `kv (cost(kv, p))`) is refused at the call,
where the name is set above:

```text
$ serq check gw.sq --workload pj.sq
gw.sq: 2:42: link error: `hold E.kv`: the admission header reads `m`, which the call passes to the entry and which is not set above the call on every way to it; the header is read when the request is admitted, so it would read what an earlier request left there, or 0, not what a later statement sets
2 |   queue gw : gateway { route { E.decode (m); set m = prompt; } }
  |                                          ^
note: the entry's header reads it as its parameter at 6:25
help: set `m` above the call, in both arms of an `if` and not in a loop, `while` or `fork` that ends before it
```

`above.sq` runs as before (`mem 2.8`). Every program in `examples/` and
`docs/tutorial/programs/` links and reports what it did; no generated file
moves.

## Why it happened

The assumption that was wrong: that a value the header reads is one this
request computed. For the server's parameters it holds, since they are
bound at each request and the server cannot assign one. For what the
server sets it does not, because the attribute is the session's and
outlives the request. The rule was written once, for the entry (#390
refused a header's read of a local, #503 let it read one set above), on
the entry's statements in the parser; the server form was
never given it, though `docs/language.md` already described the entry's
rule "as a server's header reads an attribute set before it". The engine's
admission, which reads the server's attributes when the engine admits the
call's request (since the engine took its requests, #555), was not given
it either.

The rule "what every way through these statements sets" was written three
times: the entry's `above` in `split_reads`, the reply check's
`sets_always` (`Parser::interface`: the server sets its reply on every way
that completes a request), and the check this document needed. They are one
walk now, `parser::set_above`.

## The rule

What the server sets (`set` or `choose`, anywhere in it, an entry's
expansion included, and the records a call `E (…)` writes for its caller,
`E.prefill`, `E.first`, a connector's `E.loaded`) is read in an admission
only once it is set above it on every way to it:

- **the reads**: a hold's units, `reserve`, `reuse` and `prefix` (keys and
  lengths), and an engine's `taking`, `reuse`, `prefix` and connector's
  `load` at each call `E (…)`: the `Admit` and `Request` moments of
  `docs/ir.md`'s table, the reads `hidden_in_server` takes for the
  admission, from the same lists (`hold_header`,
  `EngineReads::admission`). `cache` and a lease's time are read at the
  release, and are not checked; neither is a pool's index.
- **set above**: a `set` or `choose` sets its name for the statements after
  it, and a call `E (…)` its records; a hold's body runs to its end before
  the next statement; after an `if`, what both arms set, and an arm that
  `end;`s goes no further; what a loop, a `while` or a `fork` sets may not
  have run, so it is not set after it.
- **the parameters** are not checked: bound at each request, and the server
  sets none (`the server cannot assign request size …`).

A program that means an earlier request's value says so with a copy above
the hold, `set earlier = m;` and `hold mem (cost(mem, earlier))`, which
links and reads what the text says. No program here meant it: of the
servers in `examples/` and `docs/tutorial/programs/`, none had a header read
a name it sets below; `examples/multi-turn/routing.sq`'s sticky `home` is
read by a `choose` key and by `set j = home;`, which are not admissions.

**Where.** The linker checks it (`link::admission_reads_set_above`), on
`Program::server` as the parser expanded it, before the IR is lowered. The
IR splices the server in at every `turn;` and marks where a request starts
only in `sides`, so the server's statements as one request are the text
linker's, as they are for `hidden_in_server` (`docs/ir.md`, the last
paragraph of Moments).

**The entry.** An entry's hold is checked where the entry is called, on the
server it expands into, and the parser's check of the entry is gone. The
cases it refused are refused there with the same reason: a local is the
queue's attribute (`E.kept`), which only an entry sets, and the entry's
statements are in place. Three things move:

- a parameter is the caller's argument, so `route { E.decode (m); set m =
  prompt; }` is refused: the entry's header reads `m` before the route sets
  it. Neither check refused it before. The error stands at the call's `m`,
  where it is set above, with a note at the entry's parameter (`gw.sq`
  above).
- an entry no statement calls is not checked; its header never runs.
- an entry whose header reads a local it sets only in its body links when
  an earlier call of the queue's entries in the same request set it on every
  way: the header reads that call's value, which is there.

`tests/admission_reads.rs` holds all three: the gateway refused at the
call, `route { E.prefill (prompt); E.decode (prompt); }` and a route that
calls only `E.prefill` linked, and `if (…) { E.prefill (prompt); }
E.decode (prompt);` refused.

The reply check reads the same walk: the server sets its reply when the
walk goes on past the server and `above` holds it, as `sets_always` said.

## Self-critique

- **In `Program::validate`, on the IR.** Rejected. The IR has the
  server's statements once per `turn;`, inside the session's blocks, and a
  request's start only as a change of `sides`: a check there would rebuild
  the request from those, for one frontend's construct. `hidden_in_server`
  is the precedent for a rule on the server as one request. The cost is that
  another frontend writing the IR does not get the check; the run does not
  fail loudly either, it reads the earlier value.
- **Keep the entry's check as well.** Rejected: the same rule in two
  places, the one this repository's rule says is the next bug. What it adds
  is the second and third cases above: a header that never runs, and one
  that reads a value that is set.
- **Refuse every read of what the server sets before its `set`**, bodies
  too. Rejected: `routing.sq` reads the earlier request's `home` on purpose,
  in a `choose` key, where the read happens when the statement is reached
  and says what it reads. The admission is not the only read at another
  moment than the text's place; a pool's keys are the others, below.
- **A pool's `queue by` keys at a hold on it.** Left out. They are read
  while the session waits in the hold's queue (`Select` in `docs/ir.md`'s
  table), before the body, so `pool mem { cap 10; queue by (prio); }` with
  `hold mem (cost(mem, items)) { set prio = items; … }` links and queues a
  session's first request with `prio` 0. But a key is the scheduler's,
  read again for every waiting hold before each admission (and with
  `every (dt)`, on a clock), and it reads the session as it stands, which
  a policy may mean: `examples/multi-turn/thunderagent.sq` preempts leases
  by `reasoning`, which its server sets to 1 at a request's start and to 0
  at its end, so the key tells a session between requests from one inside
  a request.
  Whether a `queue by` key may read what an earlier request left is a
  question about keys, not about the admission, for an issue of its own;
  no program here has one read a name it sets below the hold.
- **A warning.** Rejected: `src/frontend/lint.rs`'s policy is that a check
  with no legitimate instance is an error, and the earlier request's value
  has a spelling that says so.
- **A connector's `load (n)`**, also read at the request's admission.
  Included: #554 put it in the admission's one list
  (`EngineReads::admission`), which this check reads as `hidden_in_server`
  does, so neither keeps a list of its own.
- **Leave the engine's admission out.** Rejected: the same moment, the same
  hole (the TGI variant above).
- **Leave the engine's records out.** Rejected: `E.prefill` is what the
  server's call `E (…)` sets, so `hold mem (cost(mem, E.prefill)) { } E
  (prompt, o);` read an earlier request's record, or 0, and linked.
- **Name the entry in the message**, as the parser's check did (`E.decode`,
  local `kept`). Not done: the linker reads the server as the parser
  expanded it, where an entry's hold is `hold E.kv` and its local `E.kept`,
  the names the server reads them by (`E.kept` from outside the queue) and
  the other checks of the expanded server use (`D.kv`). The error stands
  where the name is written, in the entry's header, or, for a parameter,
  at the call's argument with a note at the parameter, which says which
  entry it is; naming it too would carry the entry's name through the
  expansion for one message.
