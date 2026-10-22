# The constants in the figure: a legend

The frontend folds a `let` constant and an `args.number` input into their
values (`src/frontend/parser.rs`, the `let` branch; `src/frontend/link.rs`,
the constants loop), so the deployment figure prints the values. Where the
author wrote `beta`, the reader saw `2e-9`, and `replica.sq` has a `b` of
the same value (#15). This document adds a legend under the figure: each
constant the program declares, with its value, its definition, and the
comment on its line. The figure draws only from the IR (#265), so the IR
says what the legend says, in one field that a reader may ignore.
`IR_VERSION` does not move, and `scripts/gen_lean_oracle.py` does not
change.

## Before

`serq draw examples/multi-turn/replica.sq --workload
examples/workloads/replica.sq --format svg` at `1cb55ea`. The labels:

```
pool live                 cap 24
pool reqs                 cap 8 · evict lru
pool kv on llm's device   cap 600000 · evict by (waiting, size)
engine llm                tokens cap max(running.decoding, floor((2e-4 + 2e-9 * running.kv_decode) / 2e-5))
client                    new sessions: Poisson 0.28
```

The source (`examples/multi-turn/replica.sq`):

```
let a = 2e-5;        // prefill compute per token (s)
let b = 2e-9;        // prefill attention per token per context token (s)
let omega = 2e-4;    // weights read per iteration (s)
let beta = 2e-9;     // KV read per iteration per context token (s)
…
    tokens cap max(running.decoding, floor((omega + beta * running.kv_decode) / a));    // fill the memory time
```

`docs/visualization/index.md` listed it as a limitation: "Constants are
folded: `cap blocks * bs` shows its value rather than the original names."

## After

The same command. The labels are unchanged, and under the figure:

```
deployment
B       8       batch cap (prefilling and decoding turns)
C       600000  KV pool, tokens
s0      5e-3    per-turn overhead (s)
a       2e-5    prefill compute per token (s)
b       2e-9    prefill attention per token per context token (s)
omega   2e-4    weights read per iteration (s)
beta    2e-9    KV read per iteration per context token (s)

workload
Lambda  0.28    sessions per second
maxctx  300000  half the KV pool (the server's C / 2)
cap     24      live sessions (entry queue beyond)
```

`serq draw examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq
--max_seqs 32` draws `cap 32` for `pool reqs`, and the legend says why:

```
max_seqs   32     (--max_seqs; 16 in the program)  max_num_seqs
blocks     10000  (--blocks)                       num_gpu_blocks (less the null block)
```

## `Program::consts`

```rust
pub consts: Vec<Const>,   // serde: default, skipped when empty
pub struct Const {
    pub name: String,              // as written
    pub side: Side,                // Server: the deployment's; Workload: the workload file's
    pub value: f64,                // folded, "inf" in JSON when infinite
    pub written: Option<String>,   // the definition, when it is not one number
    pub arg: Option<ConstArg>,     // an input: { key, supplied }
    pub doc: Option<String>,       // the `//` comment that ends its line
}
```

No expression reads it. Every expression holds its value already, so a
reader that ignores the field runs the same program (`docs/ir.md`,
Stability), as with `routes` ([the client, the server and the gateway in
the figure](client-server-gateway.md)). The interpreter, the Lean generators
and the queue tests' comparison of two spellings
(`tests/queues.rs` `run_json`) ignore it. The committed oracle and claims IR
files gain the field, and the Lean files generated from them come out
byte-identical.

`Program::validate` refuses a name given twice on one side: nothing reads
the field to run, so a second entry would only draw a wrong legend. The
frontend cannot produce one. A hand-written IR could. A stricter check does
not move the version.

## Decisions

- **Which constants.** Every `let` the program writes, in both files, in
  the order each file declares them. A name declared twice in one file
  appears once, at its last declaration, which every expression after it
  reads (`lk.consts` keeps the last). A constant the parser makes for itself
  (a link's or a relation's `latency`, `D.pull.time`) is not listed. The
  rule the reader can state is "the program's constants" (criterion 0).
- **Which file.** Both files may declare one name, each meaning its own
  (`frontend/paired.rs`), so a legend without the file would give one name
  two values. The IR has no file paths, and the figure must be drawable from
  the IR. `Side` is what it can say, and the headings use the CLI's own
  words, `deployment` and `workload`. The side is read from where the `let`
  is (a token after the pair's split line is the workload's), not from the
  pair's renaming (`w·`): a word of the language is not renamed, and
  `replica.sq`'s workload declares `let cap = 24;`.
- **Derived constants.** The definition is written as it is in the text,
  `C  160000  = blocks * bs`, read from the tokens before the parser
  expands a `def`'s use in it. The tokens keep their own spelling (`1e5` is
  not `100000`, `lexer.rs` `Token.text`), and the pair renames `tok`, not
  `text`. It is omitted when the definition is one number (`8`, `6e5`, `-1`,
  `inf`): the value says it. The field is `written`, not `expr` as the issue
  sketched: every `expr` of the IR (`Gauge.expr`, `Claim.expr`) is a
  `CExpr`, and one name with two types breaks criterion 0.
- **Inputs.** `arg` carries the key and whether the run supplied it. An
  input left at its default reads `(--blocks)`, one whose default is
  computed `= C / 2 (--x)`, and a supplied one `(--max_seqs; 16 in the
  program)`, with `written` holding the default even when it is one number.
  Every `tools/oracle` IR is built from supplied inputs
  (`tests/vllm_oracle.rs`), so the IR has to say "supplied", not only the
  CLI. `supplied` inside `arg` makes a supplied value with no input
  unwritable.
- **The comment.** The `//` comment on the line of the `let`'s `;`, when
  that `;` is the last token of its line. `docs/writing-programs.md`
  already prescribes it (`let B = 8192;  // max_num_batched_tokens`), and
  246 of the 305 constants the example programs declare carry one. No
  grammar changes. A
  comment above a `let` is not its: it covers several, or marks a snippet
  (`// --8<-- [start:settings]`).
- **Finding the comment in a pair.** By line, over all the tokens' trivia,
  not as the next token's. In a pair the `}` that closes the deployment's
  `fn main()` moves after the workload's tokens (`paired.rs`, `tokens`), so
  the comment of the last `let` in `fn main()` no longer leads the token
  after its `;`. The workload's trivia were numbered by their own file, so
  `tokens` now numbers them as it numbers the workload's tokens, and keeps
  the comments after the workload's last token. Only the lexer, `fmt` and
  the highlighter read trivia otherwise, on one file's tokens.
- **Always drawn.** Whenever the program has a constant, under the figure
  and outside the server's frame. A `--legend` option would make two
  figures of one program and a knob the reader cannot see.
- **Cut short.** The IR keeps the full text. The legend cuts a comment at
  64 characters and a definition at 40 (`QUEUE_KEYS_MAX`), at a space, with
  ` …`: one helper, `cut_short`, which the queue keys' label used inline.

## Next: names in the labels

The legend names each number. It does not say which number a label was
computed from: `vllm.sq`'s `cap 16` is `max_seqs` and its `block 16` is
`bs`, and the legend lists both as 16. That needs the labels to keep the
names, and `CExpr::Const(idx)`, evaluated as a number and printed as a
name, reaches few of them: `CPool.cap`, `CPool.block`, `CArrival::Poisson`
and `CStageKind::Fifo` are numbers, not `CExpr`s. Done alone it would put
`tokens cap B` beside `cap 160000`, a rule the reader cannot infer from the
figure.

So it is one change or none: promote `cap`, `block`, `Poisson` and `Fifo`
to `CExpr`, require closed constant expressions there in `validate`, and
keep the names with `Const`. That is a new `CExpr` variant and retyped
fields, an IR version and a handshake: the generator drops every oracle
program that uses the variant out of the Lean fragment until it learns it.
The argument that would pay for it is not the figure: it is that an IR
could then take `--name value` (`src/lib.rs` `refuse_text_overrides`
refuses it because "an IR's constants are already folded"), a sweep over
the IR alone. Until a use asks for that, the legend is the step.

## Self-critique

- **`CExpr::Const` first, as the issue began.** Rejected. It does not
  reach a pool's `cap` or `block`, an arrival's rate or a server count,
  which are most of the numbers a deployment figure prints; it does not
  close the limitation the documentation lists (`cap` is a number); and it
  costs a version.
- **List only the constants whose value a label prints.** Rejected. It is
  a heuristic the reader cannot state: it would list `b` because it equals
  `beta`, and miss `cap 160000`, which is `blocks * bs` and no constant's
  value.
- **`let B = 8192 "max_num_batched_tokens";`.** Rejected. A second
  spelling of the comment the programs already write, and a grammar rule.
- **The comment above a `let`.** Rejected: it covers several `let`s or
  marks a snippet.
- **The files' paths in the headings.** Rejected: the IR has none, and a
  figure drawn from the IR would then differ from one drawn from the text.
- **Every declaration of a name declared twice.** Rejected: the earlier
  value is read only by the expressions between the two, and a legend with
  two rows for one name in one file is the ambiguity it exists to remove.
  A name declared twice is read two ways by the frontend already (a family
  sized at parse time by the first, its `cap` by the linker's last), which
  is a defect of its own, not the legend's.
- **A `--legend` option.** Rejected: two figures of one program.
- **What it does not do.** `cap 16` and `block 16` are both named and still
  not told apart (above). A word of the language the pair does not rename
  (`cap`) declared in both files is one name to the linker, whatever the
  legend says of each file's; that is the pair's defect, not the legend's.
- **Two spellings and `serq ir`.** `CLAUDE.md` checks that a new form
  lowers to the IR of the form it replaces by diffing `serq ir`. Two
  spellings of a constant (`let C = 2 * 3e5;` and `let C = 6e5;`) now
  differ in `consts[].written`, and in nothing that runs. The diff of the two
  spellings of a construct leaves `consts` out, as `tests/queues.rs` does.
