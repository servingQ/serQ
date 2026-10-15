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

## In the API

`serq::diagnose(&deployment, Some(&workload), &overrides)` is `None` when
the pair compiles as `serq check` compiles it, else a
`serq::frontend::diagnostic::Diagnostic`, and `Diagnostic::to_json()`
(`llmd_nixl_pull.sq` with `fork { P[i].prefill (input); }`):

```json
{
  "code": "E-pull-forked",
  "message": "deployment: 39:16: error[E-pull-forked]: `kvt.nixl_pull (P, D)`: `P` is called in a leg …",
  "file": "deployment",
  "line": 39,
  "col": 16,
  "range": [1664, 1665],
  "fixes": [
    {"label": "take `P[i].prefill (input);` out of its `fork { … }`",
     "edits": [{"range": [1657, 1687], "text": "P[i].prefill (input);"}]}
  ]
}
```

| Field | What |
|---|---|
| `code` | the code of the message's first line, or null for a refusal without one |
| `message` | what `serq check` prints, the pair's files named `deployment` and `workload`, the line and the caret under it included |
| `file` | the file the place is in, `deployment` or `workload`; null for one text (`None` for the workload), or a refusal with no place |
| `line`, `col` | where it is in that file, 1-based, as the message's `LINE:COL:`; null for a refusal with no place |
| `range` | the token at that place, as a byte range of the text, `[start, end]` as in [the outline](outline.md); null with no place |
| `fixes` | the table's fix-it for this refusal, as edits of that file: each a byte range and what to write there. Made from the last, they take the refusal away; another may follow. serQ finds the edit from the place and the tokens around it; where the text there is not the one the fix-it is for (a `fork` with more than the call in it), there is none. `E-pull-forked`'s is the one written so far |

| Code | The rule | Fix-it |
|---|---|---|
| `E-pull-forked` | In a pull (`kvt.nixl_pull (S, R, …)`), nothing waits for a leg as a push's connector does, so a call of `S` forked in a program that writes no `join` would outlive its session | take the call of `S` at the place out of its `fork { … }` |
| `E-copy-no-connector` | A KV relation (`kvt.nixl_pull (S, R, …)` or `kvt.nixl_push`) copies only through `R`'s engine's `connector S`, which loads each copy, or an entry of `R` that `transfer`s it | when `R`'s engine has a connector to another queue, rename it to `connector S`; otherwise none (what a request loads is the program's) |
| `E-connector-untaken` | What an engine's connector loads goes into the engine's memory, the KV pool on its device, so its `taking` takes that pool | none (how much of it a request takes is the program's) |
| `E-nic-unused` | A queue's `nic` carries a copy: a KV relation or a `transfer` runs on it | none (removing it leaves what reads `Q.nic`, and a relation names another queue) |
| `E-reserved-word` | A name the language uses for itself (a keyword, a function, a distribution) is not a name the program can give a constant, a parameter, a queue, a device or a pool | none (the name is the program's) |
| `E-device-two-pools` | The device an engine runs on holds one pool, the engine's KV (its memory): a second would leave which is the KV unsaid | none (which pool is the KV is the program's) |
| `E-pool-no-capacity` | A pool on a device or an engine (`pool NAME on OWNER`) is one of its capacities, `NAME cap …;` | none (how much it holds is the program's); a capacity of a nearby name is named |
