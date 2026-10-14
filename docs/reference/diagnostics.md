# Error codes

Some refusals carry a code, so an editor such as
[serq-lar](https://github.com/servingQ/serq-lar) recognises the rule by
its code and not by its wording. The first line of such a message reads
`error[E-…]: …`, after the place in the text:

```
57:16: error[E-pull-forked]: `kvt.nixl_pull (P, D)`: `P` is called in a leg (`fork`), and in a pull nothing waits for that leg (`join`) as a push's connector does, so the session could end while it runs
57 |         fork { P.prefill (prompt); }
   |                ^
help: fix-it: take the call `P.prefill (…);` out of its `fork { … }`
help: or make the copy a push: `kvt.nixl_push (P, D, …)`
```

The code is the contract: it names a rule and stays while the rule does.
The wording of the message and of its help is for a reader and may
change; an editor does not parse it.

A `help: fix-it:` line says, for a reader, the one edit that fixes the
program without a choice the program has not made: taking a call out of a
`fork`, renaming a connector. The table names that edit for each code, so
an editor makes it from the code and the place, finding the text by the
[outline](outline.md). Where the fix is a choice (what a request loads,
how much of a pool it takes), the refusal has a `help:` that names the
choice and no fix-it: the program states its policy, not the compiler.

The list is `serq::frontend::diagnostic::Code`; a test holds this table
equal to it.

| Code | The rule | Fix-it |
|---|---|---|
| `E-pull-forked` | In a pull (`kvt.nixl_pull (S, R, …)`), nothing waits for a leg as a push's connector does, so a call of `S` forked in a program that writes no `join` would outlive its session | take the call of `S` at the place out of its `fork { … }` |
| `E-copy-no-connector` | A KV relation (`kvt.nixl_pull (S, R, …)` or `kvt.nixl_push`) copies only through `R`'s engine's `connector S`, which loads each copy, or an entry of `R` that `transfer`s it | when `R`'s engine has a connector to another queue, rename it to `connector S`; otherwise none (what a request loads is the program's) |
| `E-connector-untaken` | What an engine's connector loads goes into the engine's memory, the KV pool on its device, so its `taking` takes that pool | none (how much of it a request takes is the program's) |
