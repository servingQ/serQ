# Two forms no program used

A hold's `at admission (x = e, …)` and a queue entry's `mark NAME;` are
gone from the language. After #719 no program used either: not an
example, not a tutorial, not a program of `serving-queue-theory/programs`
that links. CLAUDE.md's rule is that a construct some program uses stays
and one only tests exercise goes; [A request's
state](request-state.md#follow-ups) and [the prefiller as an engine
request](prefiller-engine-request.md) listed both as follow-ups.

Both were frontend sugar. `at admission (x = e)` substituted `e` for `x`
in the hold's header and, when the body read `x`, put `set x = e;` at the
top of the body; `mark x;` was `set Q.x = now;`. Neither had an IR node,
so the IR, `IR_VERSION`, the Lean generator and the oracle do not move,
and every example's report is the same byte for byte (seed 1,
`examples/pairs.json`).

## Before and after

The prefiller's hold, the last one to write the clause before #719
(`tests/prefiller_send.rs` keeps the hold, now in the second form, as the
spelling of a source that does not link):

```text
hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, reused + budget_left(P)))) reserve (cost(kv, prompt))
     at admission (reused = min(cachedin(kv), floor(prompt - 1, bs))) { … }
```

```text
hold reqs (cost(reqs, 1)),
     kv (cost(kv, min(prompt, min(cachedin(kv), floor(prompt - 1, bs)) + budget_left(P))))
     reserve (cost(kv, prompt)) { … }
```

`pd_batching.sq`'s gateway read the prefiller's `mark prefill_done;` as
`P.prefill_done`; since #719 it writes `set t_prefill = now;` after the
call, the same instant. A moment the caller needs is the caller's own
`set t = now;`, or the engine request's record (`D.admitted`,
`D.load_done`).

The parser refuses both spellings and says what to write:

```text
`at admission (x = e)` is gone: a hold's header is read at admission, so write `e` in the header where `x` stood
`mark NAME;` is gone: the caller writes the moment itself, `set t = now;` after the entry's call, or reads its engine request's records (`D.admitted`, `D.load_done`; docs/design/request-state.md)
```

`at`, `admission` and `mark` stay in `KEYWORDS` and in the highlighter's
lists, as `join` and `step` do, because the parser still matches them to
say what happened.

## What went with them

The checks that existed only for the clause: a binding's name its own
(#484), a binding the body reads only of attributes and constants, a
binding read outside its hold, a binding that draws, a later binding
read by an earlier one. The header itself keeps its checks: it is read at
admission, cannot draw, and a `set` of live state it reads is the lint it
was. The formatter's alignment of the clause's bindings, and a queue's
list of marked names, went too.

## Self-critique

**Keeping the clause as the one way to name a header's value.** A long
header reads better with a name, and the lint for a stale `set` pointed
at the clause. But a name that means "this expression, substituted" is a
second meaning beside `set`, and nothing used it; the header writes the
expression, and the lint now says so. If a program needs the name again,
it comes back with that program.

**Keeping `mark` for a moment inside an entry.** A moment inside a hold's
body, before its end, is one the caller cannot write after the call. No
program reads one: the P/D programs' moments are the engine request's
records, which the interpreter keeps for every request.
