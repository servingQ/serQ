# IR v4: making the implicit static

The body of RFC [#41](https://github.com/vrvrv/seQ/issues/41). The issue is
where the discussion happens and this document follows its outcome. Every
After is a sketch and was not compiled. Every Before is copied from the
repository.

## In one line

Four things the IR settles implicitly (the moment an expression is
evaluated, the owner of a cached unit, the order that breaks ties, the order
of events at the same instant) become explicit and static, and one
optimisation the static property licenses (step coalescing) comes with it.

## Why

Rust's ownership was not a feature. It made something dynamic and implicit
(who frees this, and when) static, and then the compiler could refuse bugs
and drop the GC at the same time. To make the same move we need our list of
"C bugs", and it is in `docs/review.md` §3.

The fifteen defects two rounds of verification caught fall into five
classes, four of them implicit things.

| Class | Item in `docs/review.md` §3 | What was implicit |
|---|---|---|
| time of evaluation | the first chunk over-reserved → `budget_left`; admission at zero budget → `admit via`; #23 | when an expression is evaluated |
| cache lifetime | an ended session's prefix survived; `end` dropped → keep; `drop` removed blocks vLLM keeps; blocks of generated tokens | who owns a cached unit |
| order | two prefills finishing in one step swapped places; LRU ties broken by session number | what breaks a tie |
| same instant | an iteration between two arrivals; a zero-work run resident forever | the order of events at one time |
| names | a `let` shadowed by an attribute; `cached` of which pool | the name space (outside this RFC) |
| **recovery** | after a preemption during decode the body restarts from the top (found while writing this RFC, §5) | what a preemption preserves |

The PS virtual-clock bug and the withdrawn bistability claim are an
interpreter bug and a statistics problem; the IR cannot fix them, and they
are left out.

## 1. Every expression belongs to one moment of evaluation

Today `CtxVar` is documented as "meaningful only where the semantics supplies
them", and that is what an undefined behaviour is: `budget_left` read in a
`@session` statement is silently 0.

There are four moments: `@session` (the session reaches the statement),
`@admit(P)` (P's scheduler decides), `@step(S)` (S starts an iteration),
`@evict(P)`. A context variable is legal only under its moment, and a
`@session` value entering an `@admit` expression is a snapshot, visibly so
in the IR.

**Changed in self-review.** The chat version said "tag every expression",
but the moment is in fact fixed by the position in the IR: a hold's header
is `@admit`, an `evict` key is `@evict`, `budget` and `cost` are `@step`, a
statement is `@session`. So no tag is needed on the expression;
`Program::validate` checks a table of allowed variables per position. The
one new node is the admission block. The cost dropped a great deal.

**Before** (`src/ir.rs:56-57`, `159-170`)

```rust
/// Context variables: meaningful only where the semantics supplies them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CtxVar {
```

```rust
    Hold {
        pools: Vec<(CRef, CExpr, Option<CExpr>)>,
        reuse: Option<CExpr>,
        body: BlockId,
        cache: Option<CExpr>,
    },
```

**After** (sketch)

```rust
/// Where an expression is evaluated. `validate` rejects a context variable
/// outside its moment: `BudgetLeft` only at `Admit`/`Step`, `Age`/`Size`/`Last` only at `Evict`.
pub enum Moment { Session, Admit(usize), Step(usize), Evict(usize) }

    Hold {
        /// Evaluated once, at `Moment::Admit`, in the session's environment; `Let`s bind once.
        at_admission: Vec<AdmitStmt>,          // Let(slot, CExpr) | Need(CRef, CExpr) | Take(CRef, CExpr) | Reuse(CRef, CExpr)
        body: BlockId,
        keep: Vec<(CRef, CExpr)>,               // Moment::Session, at scope exit
    },
```

What it catches: the whole first row of the table, one linker lint, and the
`~` ban on `at admission` bindings (a `Let` inside the block binds once, so
the ban is unnecessary). The licence condition of §6 is checked from the
same table.

**One more thing the same table checks: the scheduler does not peek at the
answer.** Today `o` is drawn at `turn`, before the request, and nothing stops
a hold header, an evict key or a budget from reading it; `reserve (prompt + o)`
is legal. vLLM knows only the bound `max_tokens` and learns the length at
EOS. The programs keep that discipline by hand with `reserve (prompt)`. An
attribute marked "not readable at a scheduler moment" is checked by the same
per-position table.

```rust
pub struct Program { …, pub hidden: Vec<usize> /* attr slots legal at Session only */, … }
```

A bound, when needed, is a separate attribute the program declares
(`max_out`). That the drawn `o` and the declared bound are different things
is what the theorem of §7 needs.

**Outcome.** The moment table landed as `ir::Moment` and the per-position
check in `Program::validate` (#44), and `hidden` as `Program.hidden` (#51).
The admission block node was not added: `at admission (x = e)` and
`admit if … fit where x = e` are parser substitutions into the hold's header,
so `CStmt::Hold` is unchanged and the check reads the substituted
expressions. No program declares `max_out` yet.

## 2. A unit always has exactly one owner

Every unit of a pool is in one of three states: owned by a hold (allocated),
owned by the cache (kept, remembering one key eligible to borrow it back),
or free. The only transitions are moves. Acquire moves free or cached units
to a hold (`reuse` is borrowing back), release moves `keep` units to the
cache and the rest to free, evict and drop move cached units to free.

Then `allocated + cached + free = cap` is an equation, assertable after
every event, and in Lean a conservation law. A dead entry gets a definition:
a unit the cache owns with no eligible borrower. Whether `end` keeps or
frees a prefix becomes an explicit move in the IR rather than an
interpreter default.

**Changed in self-review.** For a pool with infinite `cap` (the default)
free is infinite and the equation is vacuous; assert it on finite pools
only. Making the eligible borrower a key rather than a session number puts a
future content-addressed cache (a shared system prompt, §9 limitation 2) on
the same state machine. Whether the equation makes the Lean proof easier
than the inequality is to be checked on `SeqLang.Step.invariant` first, and
not adopted before.

**Before** (`docs/language.md` §3)

```
The invariant `allocated + cached ≤ cap` holds in every reachable configuration
(`SeqLang.Step.invariant`). `end` releases every hold but *keeps* the
session's cached prefixes
```

**After** (sketch)

```rust
pub struct CPool { …, pub on_end: EndMove /* Keep | Free */, … }
// interp, debug build: after every event, for every finite pool
debug_assert_eq!(p.allocated + p.cached + p.free, p.cap);
```

**Outcome.** `on_end` was not added: `drop P; end;` already says the
opposite of the default, so a pool option would be a second spelling of one
thing (criterion 0). The equality invariant waits for the Lean model to
state it; the invariant the spec states today is `allocated + cached ≤ cap`.
The `Owned` type is open.

## 3. Every order is a declared key and a named sequence number breaks ties

Every ordered collection in the IR (a pool's queue, the residents of a step
stage, the eviction order, the preemption victim, the ties of `choose`) is
defined by a key tuple whose last element must be the sequence number of a
named event: `arrived`, `admitted`, `released`. That makes it a total order
and there is no implementation order left.

**Before** (`src/interp.rs:141-142`, present only as a comment)

```rust
    /// Release order (breaks ties in `last`: entries released at the same
    /// instant age in the order they were released).
```

**After** (sketch)

```rust
pub enum Seq { Arrived, Admitted, Released }
pub struct COrder { pub keys: Vec<CExpr>, pub tie: Seq }   // validate: `tie` required
pub struct CPool { …, pub evict: COrder, pub queue: COrder, pub victim: COrder, … }
pub struct CStep { …, pub serve: COrder, … }               // replaces decode_first + exclusive_prefill
```

With `serve` an expression, #19's option B and the two criterion-2
violations of #8 (`exclusive prefill`, `decode first`) close together.

**Outcome.** `serve` became an expression (#50). `COrder.tie` was not
added: each collection has one tie, so the field would carry no
information. The rules are stated in `docs/language.md` §3 (Ties).

## 4. Time is superdense over integer ticks

An instant is `(t, n)`: `t` an integer tick, `n` the order within the same
`t`. The settle rule becomes "finish every `n` before `t` advances", and
"arrivals before an iteration start" is a declared priority. A zero-work
`run` is defined by the IR as a no-op.

**Added in self-review.** The chat version only had superdense time, but on
`time: f64` "the same instant" depends on rounding: whether the trace
spacings `i·spacing` are exactly equal, and macOS and Linux have already
differed by one ulp in a staleness diff. Integer ticks (ns) are the same kind
of clock as the Lean fragment's ℕ and the platform difference disappears.
The f64 fits of the cost model are quantised to ticks at evaluation; 1 ns is
nothing next to an A100 step of 14 ms.

**Before** (`src/interp.rs:33`, `src/ir.rs` `Program`)

```rust
    time: f64,
```

```rust
    pub horizon: f64,
    pub warmup: f64,
```

**After** (sketch)

```rust
pub struct Instant { pub tick: u64, pub sub: u32 }   // ns; `sub` orders same-tick events
pub struct Program { …, pub tick_ns: u64, pub horizon: u64, pub warmup: u64, … }
```

## 5. A session is an automaton, not a stack

Today a session's state is "a stack of block frames", and Lean has not
formalised the session-level semantics (§9 limitation 4). The program has no
procedure calls and no recursion, so that stack is fully determined by one
program counter. Compiling the session process to an automaton whose states
are the points where it can block (acquire, run, turn) makes `loop` and
`branch` jumps, preemption "a jump to the acquire state", and the session's
state a `pc` plus the attribute vector.

**The first thing 5 has to fix: a preemption resumes, it does not restart.**
Today `preempt()` unwinds the victim's frames to the hold statement and puts
the hold back at the head of the queue. The body runs again from the top:
`prefill (prompt - c)` and then `decode (o - 1)`, all of it. The decode run's
progress g was in the frame that was unwound, so it is gone. vLLM only sets
`num_computed_tokens = 0`; the generated tokens stay on the request. On
rescheduling it recomputes prompt + g as a prefill and decodes o − g − 1
more.

| | seQ today | vLLM |
|---|---|---|
| decodes after re-admission | o − 1 again | o − g − 1 |
| prefill recomputed | prompt − c | prompt + g − c |
| hit ceiling | `hitmax = floor((prompt−1)/bs)·bs`; the blocks of g are dead entries | prompt + g − 1 |

`docs/review.md` §2 says preemption is "abort the scope and re-execute the
statement, which is exactly `_preempt_request`". That is right during
prefill and wrong during decode. The Lean model has the same rule.

**No oracle has exercised this path.** The preemption counts of
`tools/oracle/*.out.json`: `chunked` 0, `hol` 0, `longchunk` 0, `mixed` 0,
`seqcap` 0, `preempt` 1. That one is a self-preemption during prefill on
10 usable blocks, before a token was produced. `programs/vllm_replay.seq`
run at spacings of 3.5 s and 2.5 s reports `preempt 0` in the `kv` row of
both, with KV use of 793 and 1 865 of 128 160 tokens. The collapse at 2.5 s
is the `reqs` queue waiting 35.8 s, not memory pressure. The 3 321-request
agreement never tested preemption recovery.

**Before** (`src/interp.rs:1463-1477`, `scheduler.py:1560-1561`)

```rust
    fn preempt(&mut self, victim: usize, pl: usize) {
        …
        self.detach(victim);
        // unwind holds inner to `hi` (nested holds), then `hi` itself
        while self.sessions[victim].holds.len() > hi {
            let h = self.sessions[victim].holds.pop().unwrap();
            self.release_hold(victim, &h);
            // pop frames down to and including that hold's frame
            while let Some(f) = self.sessions[victim].frames.pop() {
                if f.kind == FrameKind::Hold && f.block == h.body {
                    break;
                }
            }
        }
```

```python
        request.status = RequestStatus.PREEMPTED
        request.num_computed_tokens = 0
```

**After** (sketch, on the automaton)

```rust
// preemption: jump to `Scope.reentry` with the attribute vector intact.
// A run's position counter lives in the vector (slot `done` of the innermost run),
// so the program can say what vLLM does:
//   prefill (prompt + done - c) growing kv;   // recompute the KV of every token it has
//   decode  (o - 1 - done)      growing kv;   // generate only what is left
```

In today's IR the same program is possible if a run's position is exposed as
a session attribute, at the price of one more exception next to the
frame-unwinding rule ("this attribute is not unwound"). On the automaton it
is not an exception but the definition.

An oracle scenario that takes this path is needed: raise `out` in the
`preempt` scenario so that memory runs out during decode. Independently of
this RFC, the bug in today's interpreter is the subject of a bug issue.

**Confirmed in self-review.** The soundness argument is "there are no
procedures". The list of open holds is read statically from the scopes
enclosing `pc`, so a scope tree sits next to the state table. `End` is a
transition that closes every scope at any depth.

**Before** (`src/ir.rs:269`)

```rust
    pub blocks: Vec<Vec<CStmt>>,
```

**After** (sketch)

```rust
pub struct Program { …, pub states: Vec<State>, pub scopes: Vec<Scope>, pub entry: StateId, … }
pub struct State { pub scope: ScopeId, pub kind: StateKind, pub next: StateId }
pub enum StateKind { Turn, Acquire(HoldSpec), Run(RunSpec), Set(..), Observe(..), Branch(CExpr, StateId), Choose(..), Exit(ScopeId), End }
pub struct Scope { pub parent: Option<ScopeId>, pub hold: Option<HoldSpec>, pub reentry: StateId /* preemption target */ }
```

**Outcome.** The first item landed by the exception path this section
called second-best: a preempted hold is re-executed with `computed`, the
position it had reached, and the vLLM programs resume from it (#49). No
oracle scenario preempts during decode yet, and the Lean fragment does not
set `computed` (#63). The automaton itself, `states` and `scopes`, is open.

## 6. Step coalescing

While the resident composition does not change, iterations repeat
deterministically. The number `k` of iterations to the next event has a
closed form, and `k` of them can be applied at once. The 5 000 iterations
per simulated second that §9 names as the bottleneck of a fluid budget
shrink here.

The licence is static and checked from the table of §1: `cost` and `budget`
depend only on the step shape (`ntok`, `ndec`, `kvb`, …), read neither `now`
nor the online estimates (`est_*`, `price`), and are piecewise affine in the
variables that change linearly per iteration (`kvb` grows by `ndec`). Then
the cost of `k` iterations closes as an arithmetic series. A stage that does
not meet the condition runs one iteration at a time, as today.

**Lowered in self-review.** While the waiting queue is non-empty and budget
is left, an admission is possible in every iteration, so `k = 1`. There is
no gain in vLLM's collapsed regime; the gain is large in the paper's fluid
replica, where the queue is mostly empty. A block boundary of growth can be
skipped if the free blocks cover `k` iterations, else the next boundary is
the event. And it is exact only on top of 5: a resident's state has to be a
`pc` and a position counter for the state after `k` iterations to be written
in closed form.

## 7. Progress checks and the deadlock theorem

Simulation today reveals a deadlock on one instance but does not detect it.
`docs/language.md` §7's "both deadlock at the same step on 1 000 blocks" is
the example. A self-preemption is the livelock grow fails → self is the
victim → re-queue → re-admit → grow fails, and iterations keep running so
time passes and the run goes quietly to the horizon. There is no line in the
report.

**The cheap half (no IR change).** A session preempted twice without progress
is written as `stuck` in the report and `seq-lang check` fails. "Without
progress" means 5's position counter is equal, so it is exact on top of 5;
in today's IR it is approximated by zero tokens between re-admissions.

**The expensive half (a sentence writable only on top of 5).** The target
theorem:

> Under `preempt lifo` and `reserve`, if every session has
> `cap ≥ prompt + max_out`, every admitted session completes.

The argument is induction on admission order. The earliest-admitted holder is
never the victim while other holders exist, and alone it fits by hypothesis.
It is the argument vLLM makes when it asserts `max_model_len ≤ blocks·bs` at
start-up. Without the session-level semantics as a Lean relation (§9
limitation 4) the sentence cannot be written, and its `max_out` is the
"declared bound" of §1. The drawn `o ~ exp(200)` is unbounded, so without a
bound the theorem is false.

An exhaustive check on a finite instance with arrival order left
nondeterministic (P, TLA+ style) is also possible on the automaton, but
outside this RFC.

**Subagents demand one more hypothesis.** If a parent session waits for child
sessions (`Join`) while holding units of a finite pool, the induction that
the earliest-admitted holder progresses breaks. The theorem gains "no join
while holding a finite pool", and if the linker rejects a `Join` inside a
`Hold` the hypothesis is structure. `Spawn`/`Join` themselves are in
`docs/design/subagents.md` and belong to v4b.

**Outcome.** The cheap half landed as the `stuck` count: a session
preempted again without passing the position of its previous preemption is
counted per pool and written in the report (#43). It does not fail a run
yet (#64). The theorem is open.

## Value and cost

| Concept | Class removed | Lean | Simulator | Cost |
|---|---|---|---|---|
| 1 moments (position + table + admission block) | time of evaluation | the UB of context variables is gone | pure-expression caching, licence for 6 | one node, generator change |
| 2 unit ownership | cache lifetime | an equation as conservation law (to be checked) | assertion after every event | pool code rewrite |
| 3 declared orders | order | order is a function of the trace | none | small |
| 4 integer ticks + superdense | same instant | the same kind of clock as ℕ | platform differences vanish | time type replaced |
| 5 automaton | recovery (preemption resumes) | session semantics formalisable, the theorem of 7 writable | no frames, state exploration | IR redesign |
| 6 coalescing | none | none | one or two orders of magnitude, regime-dependent | only on top of 5 |

## Stages

**v4a** (cheap; semantics change, small shape change): 3, 4, the table and
admission block and `hidden` of 1, the `on_end` move of 2, the `stuck`
report of 7. #21's "same shape, different meaning → bump plus release note"
applies here. The preemption-recovery bug of 5 is fixed before v4a in
today's interpreter by exposing the run position as an attribute, with one
oracle scenario added. Of v4a, the admission block node and the `on_end`
move were not added (§1, §2 **Outcome**); 4 is open.

**v4b** (redesign): 5, then 6 on top, and the theorem of 7. The equation of
2 goes into either, after the Lean-side check.

At either stage `gen_seq_oracle.py` is a rewrite rather than a migration and
the seven `tools/oracle/*.ir.json` are regenerated. v4b needs
`SeqExec.lean` to read the automaton form. What that buys is that twelve of
the fifteen defects in the table above become impossible as a class in the
next program, and that the preemption recovery no oracle ever exercised is
right by definition.

The frontend (model/instance split, trait vocabulary, unit labels) is
independent of this RFC and goes to separate issues. This RFC is about the
IR only.
