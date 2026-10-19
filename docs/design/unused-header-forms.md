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
top of the body; `mark x;` was `set Q.x = now;`, the IR an entry's own
`set x = now;` gives, so `mark` was also a second spelling of one meaning.
Neither had an IR node,
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

A queue entry's mark, and the same IR without it:

```text
decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; mark first; } }
```

```text
decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; set first = now; } }
```

Both are `set D.first = now;` in the IR, read by the caller as `D.first`
(`tests/queues.rs`, `a_transfer_between_queues_is_the_flat_transfer`,
whose golden IR is unchanged). `pd_batching.sq`'s gateway read the
prefiller's `mark prefill_done;` as `P.prefill_done`; since #719 it
writes `set t_prefill = now;` after the call, the same instant, and the
engine request's records (`D.admitted`, `D.load_done`) give the others.

The parser refuses both spellings and says what to write:

```text
`at admission (x = e)` is gone: a hold's header is read at admission, so write `e` in the header where `x` stood, and `set x = e;` at the top of the body if it reads `x`
`mark NAME;` is gone: write `set NAME = now;` in its place, read by the caller as `Q.NAME` as before, or read the engine request's records (`D.admitted`, `D.load_done`)
```

`mark` is a retired word (`RETIRED`, beside `keep` and `where`): it
leaves `KEYWORDS`, the highlighter's `STATEMENTS` and
`docs/hooks/serq_lexer.py` together, the statement says what to write,
and a name `mark` is refused as a retired word. `at` and `admission`
stay keywords: `each at most` and a scheduler's `admission` order read
them.

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

**Keeping `mark` for a moment inside an entry.** It reads as the
intention ("mark the first token here"), criterion 1. But it gave
nothing `set NAME = now;` does not: the entry writes that anywhere in its
body, inside a hold too, and the caller reads it as `Q.NAME` either way.
Two spellings of one meaning is what the frontend rules refuse.
