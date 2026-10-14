# The server's request interface

A server with a signature reads what a request carries and nothing else of
the workload; the workload reads what the reply carries and nothing else of
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

- `server (P, hidden Q, R = c) reply (S) { … }`. A parameter may be `hidden`
  (the workload's `hidden` clause moves here, and is refused beside a
  signature) and may have a constant default, which a workload that does
  not send it gets: a deployment that reads a context still pairs with a
  single-turn workload.
- `send (P: e, …);` in `workload`, once. Each `turn;` evaluates it after the
  `turn` block's draws. `send (s)` sends the attribute `s` as it is; `s: s`
  is refused as a second spelling of it. Every parameter without a default
  is named; a name the server does not take is an error; a parameter named
  as an attribute the workload sets is that attribute, sent as it is, and
  binding it to another value is refused (one name, two values).
- The server, its gateways' routes, the queues' entries (expanded into it)
  and the scheduler's expressions (pool keys, an engine's schedule) read the
  parameters and what the server sets, never an attribute the workload
  keeps. The workload reads `reply.S` in its session, never an attribute the
  server or the scheduler sets (`cached`, `computed`, a queue's marks).
  `serial`, the session's number, every request carries: a queue keys its
  prefix by it.
- The server sets each reply attribute on every path that completes the
  request; a path that `end;`s completes none.
- What the server sets stays the session's from one request to the next (a
  sticky `home`); the parameters are bound anew at each.

The parser lowers it to the kernel: a binding `P: e` becomes `set P = e;` at
the end of the `turn` block, a default `set P = c;` in `init`, a hidden
parameter an entry of `hidden`, and `reply.S` the attribute `S`. The checks
are the linker's, at the one place a name is resolved, by the side reading
it (`Linker::cross`). A parameter sent as it is adds no statement, so the
IR of a program migrated that way is the IR it had.

## Transition

A bare `server { … }` has no signature and keeps the shared namespace. This
change gives every example and tutorial a signature where its server reads
the workload, and leaves the four whose server reads nothing bare. A second
change gives the test programs, the reference pages' and the Python
pages' inline examples (`docs/api/`, `docs/python/`) and the companion
programs (`serving-queue-theory`) their signatures, then makes a bare
`server { … }` a server with no parameters and refuses the workload's
`hidden`. Until then
the language has two forms; this document is where the second one is
scheduled to go.

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
it is, and a sent expression's run against the bare server's.
