# Paired files

A program is the deployment; the workload is a file of its own, given when
the program runs: `serq run DEPLOYMENT --workload WORKLOAD` (also `check`,
`ir`, `draw`, `target`), `serq::compile_paired`, pyserq's
`compile(path, workload=…)`. The workload file has no `fn main()`. The two
meet only at the server's signature ([request interface](request-interface.md)).

This came in two steps. The first (#639) let a deployment and a workload be
written apart and paired by the tool, beside single-file programs, with an
unchanged IR. The second, breaking, made the pair the only spelling: a
`workload` in a program's `fn main()` is refused, every example and tutorial
program is split, and pyserq takes the workload too.

## Before and after

### Step 1: pairing (#639)

`examples/multi-turn/vllm.sq`, `vllm_chat.sq`, `single-turn/vllm_single_turn.sq`
and `subagent/vllm_subagents.sq` are one engine under four workloads, written
four times; `tests/workloads.rs` holds the four engines' text equal. After,
`examples/paired/deployments/vllm.sq` is the engine once, and
`examples/paired/workloads/{multi_turn,chat,single_turn,subagents}.sq` the
four workloads:

```
serq run examples/paired/deployments/vllm.sq --workload examples/paired/workloads/chat.sq --horizon 200
```

reports what `serq run examples/multi-turn/vllm_chat.sq --horizon 200` did
as JSON, byte for byte.

### Step 2: the program is the deployment

Before, `examples/multi-turn/vllm.sq` (origin/main at 1b0a47b) is the engine
and its sessions in one `fn main()`:

```serq
use "std/args";

let B = 8192;  // max_num_batched_tokens
// … bs, a, omega, beta, c0
let p = 0.9;  // continue after a turn

fn main() {
  let max_seqs = args.number("max_seqs", 16);  // max_num_seqs
  let blocks = args.number("blocks", 10000);  // num_gpu_blocks (less the null block)
  let chunk_cap = args.number("chunk_cap", inf);  // long_prefill_token_threshold (inf: none)
  let Lambda = args.number("Lambda", 0.3);  // sessions per second
  let Z = args.number("Z", 3.0);  // tool time (s)

  device gpu { … }
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  engine vllm on gpu { … }
  pool reqs on vllm { queue fifo; }

  workload {
    arrive poisson(Lambda);
    init { set K = 0; }
    turn {
      set n = ceil(K == 0 ? ~uniform(1000, 3000) : ~exp(500));
      set o = floor(~exp(200)) + 1;
      set more = ~bernoulli(p);
    }
    send (input: K + n, output: o);
    session { turn; while (more) { set K = K + n + o; tool (~exp(Z)); turn; } }
  }

  server (input, hidden output) {
    vllm (input, output);
  }
}
```

```
$ serq run examples/multi-turn/vllm.sq --horizon 200
run: horizon 200 end 200 warmup 0 seed 1 events 95861 arrivals 63 ended 53 turns 522 mean live 6.574
```

After, the same file is the deployment (its `let`s and inputs are the
engine's; `p`, `Lambda` and `Z` left with the workload), and
`examples/workloads/chat.sq` the workload, which `vllm_lmcache.sq` runs too
(`examples/pairs.json`):

```serq
// examples/workloads/chat.sq
use "std/args";

let p = 0.9;  // continue after a turn
let Lambda = args.number("Lambda", 0.3);  // sessions per second
let Z = args.number("Z", 3.0);  // tool time (s)

workload {
  arrive poisson(Lambda);
  init { set K = 0; }
  turn { … }
  send (input: K + n, output: o);
  session { … }
}
```

```
$ serq run examples/multi-turn/vllm.sq --horizon 200
examples/multi-turn/vllm.sq: 57:3: the server needs a workload to run: give the workload's file, `serq run FILE --workload W.sq` (pyserq: `workload=`)
57 |   server (input, hidden output) {
   |   ^
$ serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 200
run: horizon 200 end 200 warmup 0 seed 1 events 95861 arrivals 63 ended 53 turns 522 mean live 6.574
```

and the Before file, unchanged, is refused:

```
$ serq run vllm.sq   # origin/main's examples/multi-turn/vllm.sq
vllm.sq: 59:3: a workload is a file of its own, given when the program runs: `serq run FILE --workload W.sq` (pyserq: `workload=`)
59 |   workload {
   |   ^
help: move `workload { … }`, with the `let`s, `def`s, stages and pools only it reads, to a file of its own
```

## The rules

- A program is the deployment: a `fn main()` with its devices, engines,
  pools, stages and `server`, and no `workload`. A `workload { … }` in it is
  a compile error, and so is a server run with no workload given.
- The workload file has no `fn main()`. It declares its `use`s, constants
  and inputs (`let`, `args.number` among them), definitions (`def`), the
  stages it runs on itself, its own pools (`pool NAME { … }`, no `on`: a
  session cap, say) and exactly one `workload { … }`; anything else
  (`engine`, `server`, `device`, …) is the deployment file's and refused.
- Each file reads its own names. No constant or definition of the
  workload file is read by the deployment, wherever (the server, a pool's
  capacity, an engine's schedule, a constant), and none of the deployment's
  by the workload; both may declare `p`, each its own. The workload runs on
  and holds its own stages and pools (and the `tool` it needs none for),
  the server none of them. What crosses is the signature.
- The inputs of both are the program's: `--Lambda 0.5` sets the workload's.
- `--workload W` is required by `run`, `check`, `ir`, `draw` and `target`
  for a `.sq` deployment; an IR (`.json`) is a whole program already and
  refuses it. pyserq: `compile(path, workload=W)` or
  `workload_source=TEXT` (one of the two), `draw` likewise.
- A message names its file, at its own line (`chat.sq: 3:5: …`).
- The examples: deployments keep their paths, workloads are in
  `examples/workloads/*.sq` and `docs/tutorial/programs/workloads/*.sq`,
  and `examples/pairs.json` maps each program to its workload (paths from
  the repository root). One workload may serve several deployments
  (`chat.sq`: `vllm.sq`, `vllm_lmcache.sq`; `dai_periodic.sq`: both of Dai's
  engines). Snippet markers (`--8<-- [start:NAME]`) moved with their text.

## How

The tool lexes both files and puts the workload file's tokens at the end
of the deployment's `fn main()` body (its `use`s before the deployment's),
then parses one program. At the end, each declaration keeps the index it has
in the one file the pair would make, so a program split into a pair keeps
its IR (the oracle's and the claims' among them). Each file's constants and definitions are
renamed apart inside the pair (`w·Z`, `d·B`), so a name of the other file
resolves to nothing wherever it is read, in an expression, a capacity or a
family's size, and a message says whose it is in place of "unknown"; the
names a message shows are as written. Stages and pools keep their names (a
report and a figure show them): the linker checks, by the side running or
holding one, that it is its file's (`Linker::paired_stage`). The workload's lines are numbered after
the deployment's, in one text (`Paired::combined`), so every position the
parser, the linker and the IR's check record, and each one a message quotes
(`declared at 6:5`), maps back to its file (`Paired::render`), and the
outline splits into the two files' (`Outline::split`, `serq::outline_paired`).
Step 2 adds the two refusals in the linker and the parser, the workload
file's `pool`, `serq::paired_workload` (a program's workload from
`examples/pairs.json`) and pyserq's `workload=` / `workload_source=`.

## Decisions

- **An in-file workload refused, not a default that `--workload`
  overrides.** A default would leave two spellings of one program, the
  workload inside and the workload beside, and a reader of a deployment
  would not know which sessions it ran. One spelling: the program is the
  deployment, the workload is given when it runs.
- **Errors, not warnings.** serQ has no warnings by design (a program links
  or it does not; its lints are errors, `docs/language.md`, Lints), and the user asked for errors.
  Both the in-file workload and the server with no workload are refused,
  each with the command that fixes it.
- **A workload's own pool.** `examples/multi-turn/replica.sq` caps live
  sessions (`pool live { cap 24; }`, held for a session's conversation):
  that cap is the client's, not the replica's, so it moved with the
  workload. A workload pool has no `on`: the workload has no device.
- **`drop kv` moved to the server, with `final`.** Tutorials 04 and 05 and
  `replica.sq` ended a session with `drop kv;` in the workload. The KV pool
  is the deployment's, which the workload no longer holds; the workload
  says what it knows, that this is the session's last request
  (`send (…, final: …)`), and the server drops its prefix
  (`if (final) { drop kv; }`). This moves the three programs' numbers:
  whether the session continues is drawn in `turn` (`set more =
  ~bernoulli(p)`), before the request, where it was drawn after it, so
  the draws come in another order; and in `replica.sq` a session that ends
  after its last tool call drops its prefix when the last request ends,
  before that call, not after it.
- **`--workload` required for `check` and `draw` too.** A figure starts at
  the arrival, and a check links the server against the attributes it is
  sent: without the workload neither has its subject.
- **pyserq takes it.** Step 1 left pairs to the CLI and the library; with
  no single-file program left, `compile` and `draw` take `workload=` (a
  path) or `workload_source=` (text), and refuse both, and a workload with
  a `.json` IR.

## Self-critique

- **`use "./workload.sq"` in a third file.** Imports would make a program a
  set of files with module namespaces (`chat.Lambda`): a module system for
  one pairing. The tool pairs two files instead; serQ's programs stay one
  program.
- **A top-level `workload` in every program** (step 1). Step 1 kept the
  single-file spelling to avoid a second migration; step 2 is that
  migration, made because two spellings of one meaning is what serQ
  refuses elsewhere (`docs/design/one-admission.md`).
- **Checking each read across.** A first version refused a token of the
  other file's name (it refused `hbm (k)`, a parameter, beside a workload's
  `let k`), then the linker's reads of constants by side: a pool's capacity,
  a family's size and a `def` slipped past both. Renaming each file's names
  apart closes every place at once, since nothing resolves a name of the
  other file; messages take the renaming off.
- **Refusing a name both declare.** With each file its own namespace there
  is nothing to refuse: the chat workload and the engine may both say `p`.
- **`serq fmt` on half a pair.** A half links only with its other half;
  `fmt` parses it for its syntax beside an empty other half
  (`parser::parse_half`), and `serq check D --workload W` checks the rest.
- **`pairs.json` is the examples' index, not the language's.** A
  deployment does not name its workload; the CLI never reads `pairs.json`.
  It is how the tests, `scripts/check_rust.sh`, the generators and the
  docs find an example's workload.
- **A workload file checked alone** (`serq check examples/workloads/chat.sq`)
  is read as a program and refused with a message about `args.number`
  rather than "a workload file needs its deployment".

## Validation

Step 1, measured 2026-10-14 against 9a49115, `serq run … --horizon 200
--json`: each of the four pairs reports what its single-file example does, as JSON
byte for byte (`tests/paired.rs`).

Step 2, measured 2026-10-14 on 1b0a47b plus this change, against the
single-file programs at 9a49115: every program of `examples/pairs.json`
(34 examples, 5 tutorial programs) compiled and run with its workload,

```
serq ir P --workload W --horizon 30
serq run P --workload W --horizon 30 --json
```

compared with `serq ir P --horizon 30` and `serq run P --horizon 30 --json`
of the program before the split, JSON without `serq_version`. 71 of the 78
are identical. The 7 that differ are the ones this change means to move:

| program | IR | report | why |
|---|---|---|---|
| `docs/tutorial/programs/04-cache.sq` | differs | differs | `drop kv` moved to the server, `final` sent |
| `docs/tutorial/programs/05-engine.sq` | differs | differs | the same |
| `examples/multi-turn/replica.sq` | differs | differs | the same, and its `live` pool, now the workload's, declared after the deployment's |
| `examples/replay/vllm_replay.sq` | the trace path only | identical | the trace is read beside the workload file: `data/short_base.csv` is now `../replay/data/short_base.csv` |

`make oracle-ir` and `make claims-ir` leave `tools/oracle/*.ir.json`,
`tools/claims/*.ir.json` and the Lean files they generate unchanged, and
`make draw-golden` moves no figure in `docs/assets/`. The tutorial pages
of chapters 4, 5 and 6, which run 04 and 05, were measured again with
their own commands. `scripts/check_rust.sh` links and draws every program
with its workload from `examples/pairs.json` (`scripts/pairs.py`), and
every deployment of `examples/paired/` under every workload there.
