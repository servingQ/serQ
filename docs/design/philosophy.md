# Philosophy: what it is mathematically, how it grows linguistically

A record of the discussion of 2026-09-28. serQ had already half chosen its
philosophy; this document gives it a name. In one line:

> Mathematically, **a stochastic process over resources whose definition is
> the object of theorems**. Linguistically, **sentences with a fixed subject,
> and a vocabulary that grows only by definitional extension**.

## Mathematics: an invariant should be structure, not a check

**Sessions are processes, pools are resources, stages are servers.** The
semantics is a stochastic timed transition system. A session block is a
sequential process and pools and stages are shared resources, which puts serQ
where the stochastic process algebras (PEPA, Modest) are. "Commands take no
time and only flow does" is then the computational rule that separates
instantaneous from timed transitions.

**`hold` is the archetype.** Folding admit and free into one scope makes
release automatic, as in RAII and the scoped ownership of separation
logic. The question for a new IR node is: which property does this node
guarantee structurally? Without an answer it stays sugar.

**Time of evaluation is a type.** What is most often wrong in this language is
not a value but *when* it is evaluated. `set` is read when the session is
ready, a hold's header at admission, and the two look alike, which is how
the vLLM program went wrong. `at admission`, one lint and the rule "a
binding may not draw" are three pieces of one problem: every expression has
a moment, and a draw or a read of live state is an effect bound to that
moment. Pushed to the end it is a type system with two tenses; [IR v4](ir-v4.md)
§1 is its first step.

**Randomness is an explicit effect and a program is a deterministic function
of its seeds.** The streams are four, `~` marks a draw, #13 removed the
implicit one. Replaying the real scheduler on the same clock and searching
for the first step that differs is impossible without this property; it is
what made the oracle possible.

**The Lean fragment is the centre; the rest is a fluid extension.** The pool
and step-engine fragment on an ℕ clock is what the theorems are about; fifo,
ps and delay on f64 time are fluid approximations. A new IR node enters the
fragment or is marked fluid-only. The fragment should grow towards the whole,
not the whole away from the fragment.

**From simulation to analysis.** A program is today the simulator's input and
the closed forms are checked outside. Pushed to the end, checkability means
the program states a claim and the tool turns it into a theorem or a
statistical test. That `mg1.sq` is an M/G/1 is decidable from its
structure. Expensive, but the best kind of answer to "why change the IR".

## Language: a fixed subject, definitional extension, polysemy by position only

**One subject per block.** The subject of the deployment is the system, of
the workload the environment, of the session one session. `admit` was wrong
because the scheduler's verb entered the session's sentence; `admit via` as
a pool option is right (#31). The one question for a keyword: is this word's
subject the block's subject?

**Vocabulary grows only by definitional extension.** The kernel (`hold`,
`run`) is the language of resources, the serving vocabulary (`enter`,
`prefill`, `keep`) the language of the domain, and the latter is defined by a
parser rewrite. That is how mathematics introduces a symbol as an
abbreviation: a conservative extension, no new theorems, hence free. If it
is definable by a rewrite it is vocabulary; otherwise it is kernel, and the
kernel pays only in checkability. `CLAUDE.md`'s cost budget is this
principle.

**Position may resolve polysemy; meaning may not.** Criterion 0's "one
construct, one meaning" and #19's "`decode` in four places" do not conflict.
Only two meanings in the same position are forbidden; `branch (p)` was that
violation.

**A domain name wins, the subject wins more.** `reserve` is good because it is
upstream's word (`scheduler_reserve_full_isl`) and the session is its
subject. The order is: grammatical subject, upstream name, invented name.

**Process imperative, policy declarative.** The session block is sequential
and should be; a request's life is a sequence. Policies are expressions
(`evict by`, `budget`, `cost`, `choose by`). Crossing that line is a smell:
the five-way policy switch of `routing.sq` is one program carrying five
deployments, and by the principle that the IR is data, a sweep is an IR edit,
not a branch in the program.

## What this philosophy asks for now

- Close #31 with the subject rule and write it into the spec in one line.
- Of the four criterion-2 violations, fold `exclusive prefill` and
  `decode first` into one order clause (#19 B, [IR v4](ir-v4.md) §3); take the
  `price` builtin out into a program expression; make HOL blocking a queue
  option.
- Write the "moment" principle into spec §3 as one paragraph.

*The one-subject-per-block rule chose `enter` and `admit if` for one construct;
[One admission](one-admission.md) retired both for the kernel's `hold`, and says
why in its self-critique.*
