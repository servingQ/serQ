# Error codes

Some refusals carry a code, so an editor such as
[serq-lar](https://github.com/servingQ/serq-lar) recognises the rule by
its code and not by its wording. The first line of such a message reads
`error[E-…]: …`, after the place in the text:

```
121:3: error[E-copy-swapped]: `kvt.nixl_pull (D, P)`: `P` has no entry `… from D`, and `D` has one `… from P`: the source comes first, then the reader
121 |   kvt.nixl_pull (D, P, latency = x0, share = share.maxmin);
    |   ^
help: fix-it: swap the two: `kvt.nixl_pull (P, D, …)`, the source first, then the reader
```

The code is the contract: it names a rule and stays while the rule does.
The wording of the message and of its help is for a reader and may
change; an editor does not parse it.

A `help: fix-it:` line says, for a reader, the one edit that fixes the
program without a choice the program has not made: swapping a relation's
two queues written the wrong way round, renaming a connector to the queue
its relation reads. The
table names that edit for each code, so an editor makes it from the code
and the place, finding the text by the
[outline](outline.md). Where the fix is a choice (what a request loads,
how much of a pool it takes), the refusal has a `help:` that names the
choice and no fix-it: the program states its policy, not the compiler.

The list is `serq::frontend::diagnostic::Code`; a test holds this table
equal to it.

## In the API

`serq::diagnose(&deployment, Some(&workload), &overrides)` is `None` when
the pair compiles as `serq check` compiles it, else a
`serq::frontend::diagnostic::Diagnostic`, and `Diagnostic::to_json()`
(`llmd_nixl_pull.sq` with `kvt.nixl_pull (D, P, …)`):

```json
{
  "code": "E-copy-swapped",
  "message": "deployment: 121:3: error[E-copy-swapped]: `kvt.nixl_pull (D, P)`: `P` has no entry `… from D`, …",
  "file": "deployment",
  "line": 121,
  "col": 3,
  "range": [4200, 4203],
  "fixes": [
    {"label": "swap `D` and `P`: the source first, then the reader",
     "edits": [{"range": [4215, 4216], "text": "P"}, {"range": [4218, 4219], "text": "D"}]}
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
| `fixes` | the table's fix-it for this refusal, as edits of that file: each a byte range and what to write there. Made from the last, they take the refusal away; another may follow. serQ finds the edit from the place and the tokens around it; where the text there is not the one the fix-it is for, there is none |

| Code | The rule | Fix-it |
|---|---|---|
| `E-copy-no-connector` | A KV relation (`kvt.nixl_pull (S, R, …)` or `kvt.nixl_push`) copies only through `R`'s engine's `connector S`, which loads each copy, or an entry of `R` that `transfer`s it; an L2 link (`kvt.l2_link (C, Q, …)`) is crossed only by `Q`'s engine's `connector C` | none (what a request loads, and which connector an engine keeps, are the program's) |
| `E-connector-untaken` | What an engine's connector loads goes into the engine's memory, the KV pool on its device, so its `taking` takes that pool | none (how much of it a request takes is the program's) |
| `E-nic-unused` | A queue's `nic` carries a copy: a KV relation, an L2 link (`kvt.l2_link`) or a `transfer` runs on it | none (removing it leaves what reads `Q.nic`, and a relation names another queue) |
| `E-reserved-word` | A name the language uses for itself (a keyword, a function, a distribution) is not a name the program can give a constant, a parameter, a queue, a device or a pool | none (the name is the program's) |
| `E-device-two-pools` | The device an engine runs on holds one pool, the engine's KV (its memory): a second would leave which is the KV unsaid | none (which pool is the KV is the program's) |
| `E-pool-no-capacity` | A pool on a device or an engine (`pool NAME on OWNER`) is one of its capacities, `NAME cap …;` | none (how much it holds is the program's); a capacity of a nearby name is named |
| `E-copy-no-entry` | A KV relation (`kvt.nixl_pull (S, R, …)`) is read by an entry of `R` written `… from S` | none (the entry's body is the program's) |
| `E-copy-swapped` | A KV relation names the source first, then the reader: `(S, R)`, written `(R, S)` when `R` has the entry `… from S` and no relation into it; an L2 link names the L2 first, then the queue: `(C, Q)`, written `(Q, C)` when `C` plays `l2` and `Q` does not | swap the two queues of the relation |
| `E-connector-misnamed` | An engine's `connector` names the queue its queue's relation reads from (`kvt.nixl_pull (S, R, …)`: `connector S`) | rename the connector to `S` |
| `E-reader-two-sources` | A reader takes its KV from one source: one relation per reader, and its entries `… from` the same queue | none, in either form (which source stays is the program's) |
| `E-connector-no-copy` | An engine's `connector S` (not an L2's) loads what a KV relation from `S` to its queue copies, so the relation is declared | none (a pull or a push is the program's) |
| `E-copy-no-queue` | A KV relation, an L2 link, or an entry's `from`, names queues declared above it | none (which queue is the program's) |
| `E-copy-no-nic` | A copy runs over the source's NIC and the reader's: both queues have a `nic`; an L2 link's queue has the `nic` its engine's copies cross | none (its bandwidth is the program's) |
| `E-copy-same-queue` | A relation, or an entry's `from`, takes the KV from another queue than its own | none (which queue reads is the program's) |
| `E-copy-latency` | A relation's `latency` is a constant, one per side that posts the copies | none |
| `E-call-from` | A call `Q.v (…) from S` names the queue its entry takes the KV `from` | none (the call's `from`, or the entry's with its relation, is the one meant) |
| `E-one-server` | A deployment has one `server`, where each request enters (the frame an editor draws once) | none (how the two become one is the program's) |
| `E-one-workload` | A workload file has one `workload`, one client | none |
| `E-workload-in-deployment` | A workload is a file of its own, given when the program runs (`--workload W.sq`) | none (which `let`s and stages go with it is the program's) |
