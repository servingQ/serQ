# One admission, one spelling

An admission is the kernel's `hold`, written the same way in a `session`
and in a `server`:

```serq
hold reqs (1), kv (min(known, hit + budget_left(engine)))
     at admission (known = computed < prompt ? prompt : computed + 1,
                   hit = min(cachedin(kv), reusable(known, blocksize(kv)))) {
  …
} cache (prompt + o);
```

(`examples/multi-turn/vllm.sq`.) There were three spellings of this one `CStmt::Hold`:
`hold … at admission (…) … cache`, `enter … at admission (…) … keep` in a
session, and `admit if … fit where … keep` in a server. #136 kept the
first. The IR did not move: every program's IR is the one it had.

The serving vocabulary a reader from vLLM looks for is written by the
program: the server block says the hold, its admission and its runs. The
language keeps the mechanism (`hold`, `at admission`, `reserve`, `reuse`,
`cache`, `lease`) and the program the policy (criterion 2).

The words `enter`, `keep`, `fit`, `where` and `admit` as a statement are
refused with the spelling that replaced them, and none of them may name a
definition, a parameter, an attribute or a binding, so that no name means
the old statement in one place and the program's thing in another. `admit
via` is the pool option and is unchanged.

## Self-critique

**The subject of the sentence.** [The philosophy](philosophy.md) argued that
a keyword's subject should be its block's: the scheduler admits, so
`admit if` in a server; the session enters, so `enter` in a session. That
argument chose a word per side for one construct, which is two spellings of
one meaning, and it put a serving word in the language where a `def` puts
it in the program. The subject survives where it separates constructs
(`turn`, `end` and `request` are the session's and are refused in a
server); it no longer names one construct twice.

**`hold` is a resource word.** A serving engineer reads "hold a slot and
the blocks" less readily than "admit". A keyword cannot say every engine.
A [`def`](../api/program.md#def) in a library, `vllm_request`, was the
first answer, and it was removed: it put the one statement a reader of a
vLLM program has to see, what a request holds and when it is admitted, in
another file behind a name, to save four programs from writing it.
`tests/workloads.rs` holds those four servers to one text instead.
