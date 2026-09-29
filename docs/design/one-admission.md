# One admission, one spelling

An admission is the kernel's `hold`, written the same way in a `session`
and in a `server`:

```seq
hold reqs (1), kv (min(known, hit + budget_left(engine)))
     at admission (known = computed < prompt ? prompt : computed + 1,
                   hit = min(cachedin(kv), reusable(known, blocksize(kv)))) {
  …
} cache (prompt + o);
```

(`lib/vllm.seq`.) There were three spellings of this one `CStmt::Hold`:
`hold … at admission (…) … cache`, `enter … at admission (…) … keep` in a
session, and `admit if … fit where … keep` in a server. #136 kept the
first. The IR did not move: every program's IR is the one it had.

The serving vocabulary a reader from vLLM looks for is now written by the
program, as a name: `vllm_request` is a [`def`](../api/program.md#def) in
a library, and the call is what the server block says. The language keeps
the mechanism (`hold`, `at admission`, `reserve`, `reuse`, `cache`,
`lease`) and the program the policy (criterion 2).

The words `enter`, `keep`, `fit`, `where` and `admit` as a statement are
refused with the spelling that replaced them; `admit via` is the pool
option and is unchanged.

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
the blocks" less readily than "admit". The answer is the definition's name,
`vllm_request`, not a keyword: the name can say vLLM, a keyword cannot say
every engine.
