# The server's request interface

The server reads what a request carries and nothing else of the
workload; the workload reads what the reply carries and nothing else of
the server. The signature is the deployment's: `server (P, …) reply (R, …)`.
The workload binds the parameters, `send (P: e, …);`, and reads the reply as
`reply.R`. The IR does not change.

## Before and after

`examples/pd-disaggregation/llmd_nixl_pull.sq` at f4b2682. The server read
the client's context `K` and new tokens `n`, its gateway set `prompt`, and
the client read `prompt` and the decoder's `D.produced` back:

```serq
    route {
      set prompt = K + n;
      choose j in ND by (holders(D[j].kv) + queued(D[j].kv));
      set hitD = min(cachedin(D[j].kv), floor(prompt - 1, bs));
      …
  workload {
    arrive poisson(Lambda);
    hidden o;
    init { set K = 0; }
    turn { … }
    session {
      turn;
      while (more) {
        set K = prompt + D.produced;
        tool (~exp(Z));
        turn;
      }
    }
  }

  server {
    observe too_long = K + n >= max_model_len;
    if (K + n >= max_model_len) { end; }
    gw.route();
  }
```

After:

```serq
    route {
      choose j in ND by (holders(D[j].kv) + queued(D[j].kv));
      set hitD = min(cachedin(D[j].kv), floor(input - 1, bs));
      …
  workload {
    arrive poisson(Lambda);
    init { set K = 0; }
    turn { … }
    send (input: K + n, output: o);
    session {
      turn;
      while (more) {
        set K = K + n + reply.produced;
        tool (~exp(Z));
        turn;
      }
    }
  }

  server (input, hidden output) reply (produced) {
    observe too_long = input >= max_model_len;
    if (input >= max_model_len) { end; }
    gw.route();
    set produced = D.produced;
  }
```

The decoder's entries read `output` where they read `o`. The report is the
same; the figure's exit label reads `out (input >= 16384)`.

## Why

Client and server attributes were one namespace (`language.md`, "Client and
server attributes remain shared"). Every vLLM program's server read `K`, the
context the client had accumulated, `routing.sq`'s read `first`, and a
client read what the server set (`prompt`, `D.produced`). Nothing said what
a request carries apart from what either side keeps, so a deployment was
bound to its workload's names: pairing one deployment with another workload
meant matching variable names. That is criterion 1 (the program does not
say what crosses) and it blocks a workload and a deployment from being
written apart.

The deployment owns the interface, as an API's provider does: the server's
signature says what a request carries (the parameters), and what its reply
carries. The workload conforms.

## The rules

- `server (P, hidden Q) reply (S) { … }`. A parameter may be `hidden` (the
  workload's `hidden` clause moves here, and is refused beside a
  signature).
- `send (P: e, …);` in `workload`, once. Each `turn;` evaluates it after the
  `turn` block's draws. `send (s)` sends the attribute `s` as it is; `s: s`
  is refused as a second spelling of it. Every parameter is named; a name the server does not take is an error; a parameter named
  as an attribute the workload sets is that attribute, sent as it is, and
  binding it to another value is refused (one name, two values).
- The server, its gateways' routes and the scheduler's expressions (pool
  keys, an engine's schedule) read the parameters and what the server sets,
  never an attribute the workload keeps; a queue's entry reads what it is
  passed and the hidden parameters, as it read the hidden attributes. The
  workload reads what it keeps, the parameters it sends as they are, and
  `reply.S` after a `turn;` of its session: never a parameter it binds to
  an expression (the server's name) nor an attribute the server, the
  scheduler or an engine sets (`cached`, a queue's marks, `vllm.first`).
  `serial`, the session's number, every request carries: a queue keys its
  prefix by it.
- The server sets each reply attribute on every path that completes the
  request; a path that `end;`s completes none.
- What the server sets stays the session's from one request to the next (a
  sticky `home`); the parameters are bound anew at each.

The parser lowers it to the kernel: a binding `P: e` becomes `set P = e;` at
the end of the `turn` block, a hidden parameter an entry of `hidden`, and `reply.S` the attribute `S`. The checks
are the linker's, at the one place a name is resolved, by the side reading
it (`Linker::cross`). A parameter sent as it is adds no statement, so the
IR of a program migrated that way is the IR it had.

## Transition

Two changes. The first (#631) added the signature beside the bare
`server { … }`, which kept the shared namespace, and gave every example and
tutorial whose server reads the workload a signature. The second makes a
bare `server { … }` a server that takes nothing and replies nothing, and
refuses the workload's `hidden`: the test programs and the reference
pages' examples were given their signatures in it, every one by sending
attributes as they are (`send (s)`), which keeps each test's IR. There is
one form again: `server () { … }` and `send ();`, which would spell the
bare forms twice, are refused.

`serving-queue-theory` pins a released serQ (pyserq and `.serq/`,
`research/serq.md` there): its 24 programs take their signatures in the
change that moves the pin to a release with this one, since the pinned
serQ parses neither `send` nor a signature.

## Self-critique

- **`request (…)` for the binding.** `request` was retired for `turn;`, and
  `request.` is the engine scheduler's list. Rejected for `send`.
- **Arguments on `turn (…)`.** `vllm.sq` writes `turn;` twice; two copies of
  the binding could drift. One clause, evaluated at every turn.
- **Positional arguments.** A pairing would hinge on order. Named.
- **The reply by bare name.** It would reopen the shared namespace on the
  client's side. `reply.S`, read only in the session.
- **A separate `interface` declaration.** The deployment owns the
  interface, so its signature is the interface; a third construct to name
  and import would buy nothing until programs span files.
- **The server stamping its own `t0`.** Five programs' servers read the
  client's `t0`. The server starts at the turn, so `set t0 = now;` in the
  server gives the same numbers, but it moves `t0`'s slot, and
  `lean/Serq/Slot.lean` names slot 9 as the request's stamp for the Bari
  and Dai proofs. They send `t0` as a parameter: the request carries the
  time it was sent, and the IR and the proofs stay as they are.
- **A deprecation warning for a bare server.** serQ has no warnings, by
  design (`src/frontend/lint.rs`); the second change refuses instead.
- **A default, `server (input, context = 0)`.** It would let a deployment
  that reads a context pair with a single-turn workload, but no program
  needs one yet, and a construct no program uses is a path only tests
  exercise (CLAUDE.md). Refused with a message; added with the first
  program that pairs that way.
- **Names.** The vLLM family's twelve servers take `input`/`output`; the
  others take the workload's names as they are (`routing.sq`'s `first`,
  `thunderagent.sq`'s `K`, the oracle programs' `prompt`, `o`, whose IR the
  oracle files fix). A common spelling for them would move their IR and
  their numbers' provenance; it is left to the program that needs it.
- **`reply.` as a dotted name.** The parser hands the linker
  `reply.S`, which it resolves to `S`; the side and the turn before it are
  the parser's to check, which it does where it reads the name.
- **Typing parameters (`Size`, a cost).** A parameter is a `Size` slot the
  workload sets, as before; a cost is still constructed by the server.

## Validation

Measured 2026-10-14 against f4b2682, `serq run FILE --horizon 30 --json` for
every program under `examples/` and `docs/tutorial/programs/` (41, two of
them instance files that do not run alone), before and after the
migration: every report identical. `serq ir FILE --horizon 30`: the IR of
the 12 programs whose server now reads `input`/`output` moved (a slot and a
`set` in the `turn` block); the other 27 are identical, so
`tools/oracle/*.ir.json`, `tools/claims/*.ir.json` and `lean/` do not move.
`tests/request_interface.rs` holds the rules, the IR of a parameter sent as
it is, and a sent expression's run against the same expression read from
parameters sent as they are.
