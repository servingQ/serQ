# One client/server program structure

The text frontend admits a session only inside `workload`. Anonymous
`request;` runs `server`; `request NAME;` runs a named gateway's route.
The IR still contains the expanded session. No IR field, version or
execution rule changes.

## Before and after

The beginning of the queue tutorial previously declared `stage server :
fifo;` and placed this block at top level:

```serq
session {
  turn;
  set t0 = now;
  run server (s);
  observe response = now - t0;
  observe wait = now - t0 - s;
  end;
}
```

The runnable replacement is `docs/tutorial/programs/01-queue.sq`. Its
stage is named `svc`, so a stage identifier does not also introduce the
request-handling block:

```serq
workload {
  arrive poisson(Lambda);
  turn { set s = ~exp(S); }
  session { turn; request; end; }
}

server {
  set t0 = now;
  run svc (s);
  observe response = now - t0;
  observe wait = now - t0 - s;
}
```

The report's stage row changes from `server` to `svc`; its numbers do not
change. The second tutorial makes the same rename. Other migrated programs
keep their names and expanded IR. Deployment drawings now select the
request body, consistently with existing client/server programs: for
example, the closed-loop example's thinking stage belongs to its client.

## Why narrow the frontend

The old rule allowed either a top-level session or a workload session with
a server. It assumed that equivalent expansion made these interchangeable
user-facing forms. That obscured the distinction between the client and a
request handler in the tutorial. It also allowed a top-level session to
avoid the frontend's additional hidden-attribute check on server statements.

The parser can recognize the old form before execution and now rejects it,
including an empty top-level session, with instructions to put the session
inside `workload` and request handling in `server`. The regression preserves
the original hidden-attribute example and checks both rejection of its old
structure and the hidden-read diagnostic after moving its handler to a server.

This is a structural rule, not a new policy about client statements.
Clients still need holds across turns, delays, observations and control
flow. Their statements retain their existing evaluation moments. Tests
about client control or IR validation can use an empty server; an empty
request completes immediately and adds no IR statements.

## Self-critique

- Keeping both spellings and merely documenting a preferred one would leave
  both the structural ambiguity and the old check bypass available. Rejected.
- Automatically wrapping an old session would guess where requests start and
  finish. A multi-turn session may also hold resources across requests. The
  parser must not invent those boundaries. Rejected.
- Requiring an anonymous `server` beside a named gateway would add an unused
  handler. Gateway routes already supply request handling. Retain explicit
  named requests.
- Removing session blocks from the IR would charge every consumer for a
  frontend organization choice. Keep the existing expansion.
- Prohibiting all resource operations in the client would remove legitimate
  session-wide reservations and tool calls. This change does not claim to
  classify every client statement as a scheduling operation or to replace
  the IR's evaluation-moment checks.

## Validation

Compare the 23 migrated `.sq` files with their pre-migration forms using
the earlier compiler (removing `main` and substituting input defaults for
its older frontend). All expanded IR is identical after normalizing the
two tutorial stage names.
The parser tests reject the former top-level spelling, preserve nested and
repeated requests, and retain explicit gateway dispatch. The normal gates
exercise the migrated test programs, oracle IR, Lean regressions, formatter,
deployment drawings and the documentation site.
