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
