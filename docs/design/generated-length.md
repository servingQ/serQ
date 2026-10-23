# What the scheduler knows of a generated length

A server's parameter is the scheduler's to read unless the signature says
`hidden`. That default is right for a prompt or a flag and wrong for the
length a model generates: a program that forgot `hidden` linked as a
scheduler that knows how long each request runs, and nothing said so. A
parameter that a generated length reads now has no default. The signature
writes `hidden` (the scheduler learns the length when the run ends) or
`visible` (it may read it before), and a bare one is refused (#61). The
IR does not change: `visible` is a parameter left out of `Program::hidden`,
as a bare one was.

## Before and after

`examples/multi-turn/vllm.sq:57-59`:

```serq
  // the scheduler knows max_tokens, not the length (sched/utils.py:98-119)
  server (input, hidden output) {
    vllm (input, output);
```

vLLM's scheduler knows `max_tokens` and learns the length when
`check_stop` sees EOS, a stop token or the cap (`sched/utils.py:98-119`).
With `reserve (request.known)` (`:41`) changed to `reserve (request.input
+ request.output)`, `hidden` refused the program; without `hidden` it
linked, a scheduler that reserves each request's whole sequence before it
runs. Measured on 1cb55ea:

```text
$ serq check v_hidden_reserve.sq --workload examples/workloads/chat.sq
v_hidden_reserve.sq: 59:18: link error: `output` is hidden from the scheduler, but the server's admission reads it before a run reveals it
$ serq check v_nohidden_reserve.sq --workload examples/workloads/chat.sq     # server (input, output)
OK: 2 pool(s), 2 stage(s), 18 attribute(s), 4 block(s)
```

Two shipped programs had forgotten it. The tutorial's fifth chapter
follows vLLM (`docs/tutorial/programs/05-engine.sq:33`) and wrote `server
(input, output, final)`; `examples/vendors/rbln.sq` wrote `server (prompt,
out)`. Their answers were right only because their admissions read
`request.output` in `cache` alone, at the release.

After, on this branch:

```text
$ serq check v_nohidden_reserve.sq --workload examples/workloads/chat.sq
v_nohidden_reserve.sq: 58:18: `output` is a length the model generates (`vllm (…)`'s output at 59:5 reads it), and the signature does not say whether the scheduler knows it: a bare parameter is the scheduler's to read, which makes a scheduler that knows each request's length before it runs
58 |   server (input, output) {
   |                  ^
help: `hidden output` if the scheduler learns the length when the run ends, as vLLM's, which knows `max_tokens`; `visible output` if it may read it before, as an admission that reserves the whole sequence does
$ serq check v_hidden_reserve.sq --workload examples/workloads/chat.sq
v_hidden_reserve.sq: 59:18: link error: `output` is hidden from the scheduler, but the server's admission reads it before a run reveals it
$ serq check v_visible_reserve.sq --workload examples/workloads/chat.sq      # server (input, visible output)
OK: 2 pool(s), 2 stage(s), 18 attribute(s), 4 block(s)
$ serq check v_visible_input.sq --workload examples/workloads/chat.sq        # server (visible input, hidden output)
v_visible_input.sq: 58:19: `visible input`: no length the model generates reads `input` beyond what the request's prompt reads, and a parameter the scheduler may read is written bare
help: `input`; `visible` is said only of a generated length, which says `hidden` or `visible`
$ serq check v_ctx_after.sq --workload examples/workloads/chat.sq          # set ctx = ctx + input; vllm (ctx, output); set ctx = ctx + output;
v_ctx_after.sq: 58:18: `output` is a length the model generates (`vllm (…)`'s output at 60:5 reads it), and the signature does not say whether the scheduler knows it: a bare parameter is the scheduler's to read, which makes a scheduler that knows each request's length before it runs
```

The same program without the reservation is refused the same way: the
word is asked for where the signature is written, not when a later edit
makes the scheduler read it.

## The rule

A *generated length* is the output of an engine's request (`E (n, o);`'s
second argument, which the model ends) or the work of a decode run (`run
E decode (w)`, the kernel spelling a queue's entry writes). Each parameter
that a generated length reads, and that the request's prompt does not, is
written `hidden` or `visible`. Every other parameter is bare or `hidden`,
as before.

- **A length's reads follow the server's `set`s and `choose`s, in any
  order.** The parser checks the server once the queues' entries are in
  place (an entry's parameters replaced by the arguments, its locals by
  `Q.x`), so `examples/pd-disaggregation/llmd_nixl_pull.sq`'s length, `set
  produced = min(output, max_model_len - prompt); D (prompt, produced);`,
  reads `output` through the entry's local. A `set` anywhere in the server
  counts: one after the call reaches the next turn's length, and one a loop
  runs later reaches its next pass. This may ask for a word that a walk in
  order would not, and `hidden` answers it.
- **The prompt is exempt, as it stands at the call.** What the input of `E
  (n, o);` reads needs no word: a prompt is no generated length, and the
  engine admits by it, so the linker refuses `hidden` there
  (`check(input, "admission", t)` in `src/frontend/link.rs`;
  `tests/hidden_server.rs`, `a_request_s_input_may_not_be_hidden`). A word
  with no alternative is not a choice. Without the exemption
  `llmd_nixl_pull.sq`, whose length reads `prompt` through `max_model_len
  - prompt`, would need `visible input`. The input is followed in the
  order the server runs (`held_by_prompts` in `src/frontend/parser.rs`):
  through the `set`s before the call, joined over an `if`'s arms and a
  loop's passes. The order matters here and not for the length, since
  what the prompt counts is excused: a multi-turn server that stores its
  context after the request, `set ctx = ctx + input; vllm (ctx, output);
  set ctx = ctx + output;`, would otherwise have `output` in the prompt,
  and the length it generates would pass bare (`v_ctx_after.sq` above,
  which linked when the prompt's `set`s counted in any order).
- **A decode run has no prompt to exempt.** It has no input argument: in
  the kernel the prompt is what the hold's header and the prefill run
  read, and either may read the length too (a header that holds `K + n +
  o`, as `replica.sq`'s does), so nothing tells the prompt from the length.
  Every parameter a decode run's work reads asks for a word. The two
  tests whose decode work subtracts what an earlier attempt produced, `o -
  1 - (known - prompt)` (`tests/engine_request.rs`, `KERNEL`;
  `tests/vllm_semantics.rs`, `preempted_twice_in_a_queue`), say `visible`
  of the prompt, which their headers read; only test programs decode a
  parameter (`tests/lean-regress/preempt_delay.sq` decodes a constant).
- **`visible` elsewhere is refused.** A parameter no generated length reads
  is the scheduler's when bare; `visible` there would be a second spelling
  of one meaning (CLAUDE.md, the `queue fifo;` rule), as `server ()` is
  refused for `server { … }`. So every parameter has one spelling: bare or
  `hidden` for an ordinary one, `hidden` or `visible` for a length.
- **`hidden visible o` is refused** as a contradiction.

The check is the parser's (`Parser::generated_lengths`, called in
`assemble` before the interface is lowered). It errors at the parameter in
the signature, where the word goes. The linker never sees `visible`.

## The programs

Every program that wrote a bare length now says which. The three the
issue did not list were found by the check itself.

| Program | Word | Evidence |
|---|---|---|
| `docs/tutorial/programs/05-engine.sq` | `hidden output` | vLLM's admission; `request.output` only in `cache` |
| `examples/vendors/rbln.sq` | `hidden out` | vllm-rbln's admission; `request.output` only in `cache` |
| `examples/papers/bari_rad.sq` | `hidden vd` | RAD decides by the batch (`running.decoding`), never by a request's output |
| `tests/lean-regress/chunk_expr.sq` | `hidden o` | the admission takes `request.input` |
| `tests/lean-regress/cost_ctx.sq` | `hidden o` | the admission reserves `request.input` |
| `examples/vendors/ascend.sq` | `visible out` | the admission takes `request.input + request.output - 1` (`:22`) |
| `examples/multi-turn/replica.sq` | `visible o` | the hold's header holds `K + n + o` (`:45`) |
| `examples/papers/kong_svf.sq` | `visible o` | SVF's queue key `s * o + …` and the peak `s + o` |

Each `hidden` program also links with `visible`; each `visible` one is
refused with `hidden`. The word is the program's statement of its
scheduler (criterion 2), and the language picks neither.

Of the test programs, those whose scheduler orders by `remaining`
(`tests/vllm_semantics.rs`, `serve_by_orders_residents_by_the_declared_keys`
and `a_resident_admitted_mid_iteration_is_served_once_under_serve_by`)
say `visible o`: shortest-remaining-first reads each request's length. The
two kernel programs say `visible` of the prompt their decode work reads
(above). The others say `hidden`.

## The IR

Unchanged. `visible` and a bare parameter both lower to a slot outside
`Program::hidden`; `hidden` is read only by `Program::validate`
(`src/ir.rs`, the check at `Session` and `Given`), so no number moves. The
five `hidden` programs gain the slot: three committed IR files
(`tools/claims/bari_rad.ir.json`, `tests/lean-regress/chunk_expr.ir.json`,
`cost_ctx.ir.json`) gain `hidden`, which neither Lean generator reads, and
their Lean statements and answers are the same. `IR_VERSION`,
`scripts/gen_lean_oracle.py` and the Lean model do not move. A stricter
check does not bump the version.

## Transition

`serving-queue-theory/programs` is pinned to v0.1.5 and writes bare
`server { … }`. When it adopts signatures it writes the word on each
length; a bare one is refused with the two choices in its help.

## What this does not cover

- **`remaining`.** An `advance running by (…)` key, it is the tokens a
  resident's run has left, and for a decode that is the generated length
  less what is done. A program with `hidden o` may still order by it, and
  so read the length before the run ends: `server (input, hidden output)`
  with `advance running by (remaining)` links, with #554's key check in
  place. The check here asks for a word at the signature; the hole is the
  linker's, a context variable that reads a hidden value, and is left to
  its own issue: #554 refuses a key that reads a name set from a hidden
  attribute, not a context variable the engine derives from one.
- **A run on a stage.** `run S (w)` without a mode, on a stage that is no
  engine, is no request and has no `request.output`: its work is the
  program's model of a service time, which a scheduler may read as written.
  The programs the issue named for reading a length on purpose through one
  stay bare: `examples/pd-disaggregation/pd_two_fabrics.sq` (the decode
  hold's `n + o`, then `run D[j] (cost(D, o * tok))`),
  `docs/tutorial/programs/04-cache.sq` (`hold kv (cost(kv, K + n + o))`
  around `run engine (…)`), `examples/multi-turn/routing.sq` (a replica
  chosen by a service time that reads `o`), and also
  `examples/pd-disaggregation/pd_ps.sq` and
  `docs/tutorial/programs/03-sessions.sq`.
- **A length the server draws.** `server (input) { set o =
  ceil(~exp(200)); E (input, o); }` reads no parameter, so no word can be
  written for it, and an admission that reserves `request.output` links.
  `hidden` is said of a parameter, and a value the server draws is none.

## Self-critique

- **The name `known`.** The issue's placeholder. `request.known` already
  names the tokens a request has (its input and what it generated), read
  in the same admissions (`vllm.sq:40-42`), so one word would carry two
  meanings in one program (criterion 0); and `known` is a `def` parameter
  in `examples/multi-turn/vllm_lmcache.sq:58`, which a keyword may not be.
  `visible` is the docs' own opposite of "hidden from the scheduler".
- **Every default flipped to hidden.** Every server's `input` would carry
  a word, and the prompt's could only be one (the engine admits by it).
- **A warning.** serQ has no warnings, by design (`src/frontend/lint.rs`,
  [the request interface](request-interface.md)'s self-critique).
- **An engine's output always hidden.** A scheduler that reserves the whole
  sequence (`ascend.sq`), holds it (`replica.sq`) or orders by it
  (`kong_svf.sq`) could not be written; the language would pick a policy
  (criterion 2).
- **A word on every parameter a length reads, the prompt's included.**
  `llmd_nixl_pull.sq` would write `visible input`, a word with no
  alternative.
- **One marked parameter per length suffices.** `E (n, o1 + o2)` with
  `hidden o1` would pass with `o2` bare, which is the failure again.
- **Direct reads only.** `llmd_nixl_pull.sq`'s `set produced = min(output,
  …)` would escape the check; a forgotten word one `set` away is still
  forgotten.
- **A length's reads in order.** A `set` after the call reaches the next
  turn's length, and one later in a loop the next pass's; read in any
  order, the length asks at worst for a word `hidden` answers.
- **The prompt's reads in any order.** The first version followed the
  prompt as it does a length, through a `set` anywhere. What the prompt
  counts is excused, so the direction is the opposite: a `set` after the
  call put the length in the prompt and let it pass bare, the failure
  again (`v_ctx_after.sq` linked, with an admission that reserved the
  length). The prompt is followed in order (`held_by_prompts`), which is
  not the linker's taint walk: it tracks which parameters a name holds,
  not what is hidden or revealed.
- **What the admission reads before the run needs no word.** Proposed in
  review to merge the two exemptions, the input of `E (n, o);` and, for a
  decode run, a hold's header, since `hidden` is refused in both. A header
  is the program's policy and not the request's prompt: `replica.sq`
  holds `K + n + o` in its header, so its `o` would pass bare, a scheduler
  that knows the length without saying so, which the issue lists as a
  program that must say it.
- **The word at the call, `llm (input, hidden output)`.** Hiding is the
  signature's ([the request interface](request-interface.md)): the server
  is the deployment's interface, and two calls could say different things
  of one parameter.
- **Inferring the word from what the scheduler reads.** That is what the
  bare default did: adding a `reserve (request.output)` would silently
  turn a scheduler that learns the length into one that knows it, the
  failure this closes.
- **A decode run left out.** Only test programs write the kernel's decode
  run, but the same rule written in one place and not the other is the
  next bug (CLAUDE.md).
- **A decode run's prompt is what a prefill run on its engine reads.** The
  first version exempted it, so that the two tests whose decode work
  subtracts `known - prompt` needed no word. A prefill's work may read a
  hidden value: with a prefill and a decode that both read `prompt`, `n`
  passed bare though `server (hidden n, hidden o)` links, a word with an
  alternative left unasked. The two tests say `visible`.
- **A diagnostic code.** Fixing the error is a choice between two words,
  so it gets a `help:` and no fix-it (`docs/reference/diagnostics.md`). A
  code can follow when an editor asks for one.
