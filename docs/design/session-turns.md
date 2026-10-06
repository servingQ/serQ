# Sessions describe completed turns

A workload describes arrivals and how a conversation grows. Requiring the
single-turn client to call a server and terminate exposes the frontend's
inlining mechanism. A destination chosen inside each client statement also
prevents using the same workload with a different serving implementation.

## Before and After

Before, copied from `docs/tutorial/01-a-queue.md` at `adc75ea`:

```serq
workload {
  arrive poisson(Lambda);
  turn { set s = ~exp(S); }
  session { turn; request; end; }
}
```

After, from `docs/tutorial/programs/01-queue.sq`:

```serq
workload {
  arrive poisson(Lambda);
  turn { set s = ~exp(S); }
}
```

An omitted session means one complete turn. An explicit `session { turn; }`
means exactly the same thing. A source `turn;` draws the next attributes,
submits them to the program's server, and waits for the response before
continuing. Reaching the end of the session ends it.

Before, copied from `examples/multi-turn/vllm.sq` at `adc75ea`:

```serq
    session {
      turn;
      loop {
        request;
        set K = prompt + o;
        branch (more) { tool (~exp(Z)); turn; } else { end; }
      }
    }
```

After, from the same program:

```serq
    session {
      turn;
      while (more) {
        set K = K + n + o;
        tool (~exp(Z));
        turn;
      }
    }
```

The client accumulates its own input and output attributes; it no longer
reads the server's `prompt` temporary. It only needs that accumulation when
another turn follows. The continuation probability remains in the program's
turn block. A trace instead supplies `more` from its remaining turns.

Named gateway selection moves into `server { gw.route(); }`. The server can
choose among gateways, using ordinary branches and entry calls. Neither a
single gateway declaration nor its position registers a default server.
Source `request` is rejected with migration guidance; queue entry calls in a
workload session are rejected. There is one serving boundary, at a turn.

The PD example's prompt-length check runs inside the server after the turn
has been drawn and before its gateway is called. A refused prompt sets
`more = 0`, so the client terminates without running a serving stage.

## IR and verification

The source `turn;` lowers to the existing IR `Turn` followed by the expanded
server statements. The IR `Turn` still only draws attributes. No new server
field, dispatch instruction, scope, or random stream is needed. `Program` is
still the executable definition of the composed deployment.

`While(guard, body)` is the one new IR variant. It tests before each pass and
continues after the loop on zero. Only 0 and 1 are guards: validation rejects an invalid constant, and
the interpreter rejects an invalid computed value. The node is necessary for a loop
that can finish inside an enclosing hold or another loop: `Loop` has no
normal exit and `End` ends the entire session, so that pair cannot express
this behavior. IR 11 is tagged in v0.1.3; this change therefore uses IR 12.

The frontend appends the existing `End` at the outer session boundary;
users do not need to spell this kernel termination instruction.

The Lean route has `whileLoop guard body continuation`. It reuses the
sequence frame to retest after the body, and its continuation is a subprogram
in the reachability invariant. The oracle generator translates `While`;
the prefix-cache oracle now exercises it. Translation requires a guard whose
range is guaranteed to be 0/1; other guards are outside the Lean fragment. Oracle request times and cached
prefix expectations remain the checked-in upstream answers.

The claim fragment has no trace or turn block and does not track the turn
counter. Its translator omits empty `Turn` steps only after refusing any
read of `turn_no` anywhere in the IR. `Exec.exec_empty_turn` proves that
such a step just advances the continuation under the generated workload
family. The oracle translator keeps `Turn`, including all replay turns.

The regression tests derive their expected times by adding request and
think durations. They cover an omitted session, response-dependent input,
nested loops, zero passes, statements after a loop and scoped release.
`Program::validate` checks progress for direct IR as well as linked text.
The existing Lean regression corpus and oracle IR are regenerated.

## Why the old form was burdensome

The wrong assumption was that exposing a general client coroutine's
server-call and termination instructions would also be the clearest way
to describe a workload. The single-turn example showed the cost: three
instructions stated no policy. This is a notation problem, not an incorrect
interpreter answer. A validator could not reject that old program as unsafe.

A conditional loop *can* repeat without letting time pass. The existing
progress rule therefore applies to `While` too, with an error explaining
that every body path must run, hold a body that runs, or end. A nested
conditional loop can be skipped; it cannot alone certify outer progress.
The runtime's existing non-progress counters remain the backstop for
computed zero-duration work.

## Self-critique

- Making `session` an implicit after-response hook would remove an explicit
  first turn, but lose pre-first-turn timing and admission. Replay programs
  delay arrivals and a conversation can hold a slot across multiple turns;
  an explicit sequence preserves both.
- Keeping `request` as a synonym would leave two spellings for submission.
  It is removed instead. This is a source migration, including definitions
  that used to draw without submitting or name a gateway per request.
- Removing all `end` statements would remove early termination. Only the
  redundant final statement disappears; `end` remains an explicit early
  exit, and `loop` still expresses unconditional repetition.
- Lowering `while` to `loop` and `end` would end the entire session, skip its
  continuation, and mishandle enclosing scopes. A new node costs every IR
  consumer but gives normal loop completion one checkable meaning.
- The progress check is conservative: a human may know that an inner
  conditional loop runs at least once, but the current checker does not
  prove that from preceding assignments. Such an outer loop needs an
  independently progress-making statement on each path.
- The server and workload still share attributes. This change separates
  lifecycle and routing notation; it does not introduce an input/output
  type system or ownership of arbitrary attribute names.

## Generated figure

Moving the prompt-length check into the server makes its refusal visible in
`docs/assets/llmd_nixl_pull.deployment.svg`. Before, the only exit was from
the decoder (copied from `adc75ea`):

```svg
<path class="flow" fill="none" stroke="#4b5563" stroke-width="1.3" d="M1329.2 167 L1490 167" marker-end="url(#a)"/>
<text x="1494" y="170" text-anchor="start" font-size="8" fill="#6b7280" class="dim">out</text>
```

After regeneration, that exit remains and the router has the additional
refusal exit:

```svg
<path class="flow" fill="none" stroke="#4b5563" stroke-width="1.3" d="M225 193 L225 359 L1490 359" marker-end="url(#a)"/>
<text x="1494" y="362" text-anchor="start" font-size="8" fill="#6b7280" class="dim">out</text>
```

The drawing regression checks both exits and that thinking time remains
outside the serving path.

## Single-turn reporting

Programs that formerly submitted without a `turn;` now count that turn.
Before, copied from `docs/getting-started.md` at `adc75ea`:

```text
run: horizon 250000 end 250000 warmup 25000 seed 1 events 397862 arrivals 198931 ended 179177 turns 0 mean live 3.899
```

After, from `serq run examples/single-turn/mg1.sq --horizon 250000 --warmup 25000`:

```text
run: horizon 250000 end 250000 warmup 25000 seed 1 events 397862 arrivals 198931 ended 179177 turns 179176 mean live 3.899
```

The measured queueing observations are unchanged. The counter records turns
started after warm-up; completions can cross the measurement boundaries.


## Review: conditional-loop translation boundary

The initial Lean implementation reused `branch`'s nonzero test, assuming
that every generated `while` condition would be 0/1. The generator did not
check that assumption. Wrapping the `alone` oracle's session in
`Set(prompt, 2); While(Attr(prompt), body); End` made Rust report an invalid
guard while Lean executed the body and recorded completions.

The IR correctly allows a dynamic guard: its value is generally unknown
until execution, where Rust fails loudly. No additional source/IR rejection
is appropriate. The Lean generator now refuses guards without a 0/1 range
check. It accepts literal booleans, comparisons, logical expressions,
conditionals with boolean outcomes, and read-only trace `more` after checking
all session and turn presets. Any assignment to `more` is conservatively
refused; proving mutable attributes boolean would require dataflow analysis.

The regression reproduces the review's complete IR wrapper and also covers
an overwritten `more`, including nested assignments and nonboolean presets.
The existing prefix-cache oracle remains in the fragment. Adding an error
state throughout the Lean machine was rejected for this fix: the current
fragment deliberately excludes runtime-error behavior, so the translation
boundary is the place to enforce this restriction.


## Re-review: loop-carried hidden values and arithmetic guards

The server's hidden-attribute analysis visited a loop body twice. That
assumed that a late assignment reaches every relevant decision on the next
pass. The counterexample `a = b; b = secret` reaches `while (a == 0)` only
on its third evaluation; longer chains require more passes. The analysis
now unions the origins arriving at the loop header until that finite set
stops growing, checking the guard and body on each pass. The same rule
applies to `Loop`. Joining paths retains all possible origins even when a hidden
attribute is reassigned from another hidden input. Possible origins prohibit
premature decisions; a separate intersection of origins present on every
path determines what a run can reveal. Revelation is tracked separately from
the value's dependencies, so repeated runs remain valid evidence at every
loop pass. These are statically
recognizable hidden reads, so the linker refuses them before execution.
Tests preserve the original two-hop counterexample, longer chains and the
legal case where a run has revealed the value before the decision.

The 0/1 translation check also confused a boolean result with an equivalent
boolean result. With `prompt = 1`, Rust evaluates `prompt - 2 < 0` as true,
but Lean's natural subtraction makes it false. The `alone` oracle wrapped
in this loop completed four requests in Rust and none in Lean; request work
and resource amounts remained nonnegative. This is a valid Rust program,
so rejecting it in the language would be wrong.

The Lean `While` boundary now refuses subtraction in the guard or any
assignment feeding its attributes, following aliases transitively. It uses
a visited set for cycles and checks assignments in every block, including
later loop passes. The regression covers the review's exact wrapper and a
two-hop alias of the difference. This is intentionally conservative: even
nonnegative differences are rejected here. Adding only a direct guard check
was rejected because moving the subtraction into an attribute would bypass
it; proving arithmetic ranges is beyond this translation fragment. Existing
`Branch` and general natural-arithmetic limits are unchanged. Neither fix
changes IR meaning or requires another version bump.


The first fix tried to union origins and reveal every member after a run.
That was rejected by a regression where only one branch assigns `a = b`:
a later run by `a` cannot establish that hidden `b` was read. The revised
analysis separates possible dependencies from guaranteed reads, including
conditional expressions and short-circuit operators. Tests cover both
branch directions, the rejected disclosure, and the allowed case where
both arms depend on the same hidden input. Aggregate bodies are not assumed
to execute; that conservative boundary avoids treating a zero-term
aggregate as evidence of a read.


## Re-review: division in guards

The `alone` session wrapped in `while (floor(prompt / 0) == 0)` completed
no requests in Rust but four in Lean. The assumption that boolean results
without subtraction preserve arithmetic meaning was wrong: Nat division
by zero is zero, while Rust produces infinity or NaN. This is a valid Rust
comparison, so no linker or IR validation rule should reject the program.

The translation boundary now requires every divisor in a `While` guard
and its transitive attribute assignments to fold to a positive natural
constant. It checks before constant folding, which could otherwise erase
`0 * floor(prompt / 0)`. Tests retain the review's exact wrapper, a dynamic
divisor, a two-hop alias, and the zero-product case, alongside accepted
literal and folded positive divisors. The prefix-cache oracle is unchanged.
This stricter translation check does not change IR meaning or its version.

Rejecting only literal zero was rejected: a preset or assignment can make
a dynamic divisor zero. Proving positivity of mutable attributes would
require range analysis beyond this fragment; even a dynamic divisor that
stays positive is conservatively rejected. The existing general arithmetic
and `Branch` limitations remain outside this focused `While` check.
