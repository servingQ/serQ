# A serving specification language

serQ is a **serving specification language**: a program specifies an LLM
serving system, and that one specification is run three ways. The
interpreter simulates it, Lean proves things about it, and a target runs it
in place of an engine's scheduler. The first two exist. The third is where
the language is headed, and it decides what the language should look like
now.

The precedent is P4, the language for programming packet switches. A P4
program specifies a switch's pipeline. A reference software target (bmv2)
runs it, a formal semantics (Petr4) reasons about it, and a compiler
backend runs it on a switch ASIC in place of the fixed-function pipeline
the vendor shipped. serQ sits in the same position for a serving engine. A
serving program states its policy (admission, serving order, eviction)
where the engine today hard-codes it, and the program can say the opposite
(criterion 2).

## The circuit and the testbench

What a program specifies splits as a hardware design does:

| | in a serQ program | in a hardware design |
|---|---|---|
| the system | `server`, pools, stages: what one request runs, which is what `serq draw` draws (the deployment view) | the circuit, and its schematic |
| the environment | `workload`: arrivals, turns, the client's decision to go on | the testbench |
| what the system cannot see | `hidden` attributes (`hidden o;`: the scheduler knows `max_tokens`, not the length) | signals outside the design's ports |
| what the target supplies | a stage's `cost`, a `delay` stage | a hard IP block's behavioural model |

A target runs the system and replaces the environment with real clients.
The testbench is never synthesised, and that is why the deployment view
draws the server's side only (#265).

## What P4 gives

Five things in P4's design have a counterpart serQ either lacks or keeps
implicit. Each one below says where serQ stands today and what the design
is.

### 1. An architecture separates the target's facts from the program's policy

P4_16's core language knows nothing about a target. An *architecture*
(`v1model`, `psa`, a vendor's) declares the programmable blocks a program
fills in, the fixed-function parts between them, and the externs the
target provides. A program is written against one architecture.

serQ has one architecture, the continuous-batching step engine, and it is
built into the core: `CStage::Step` with `budget`, `chunk`, `serve`, `only`,
`cost` and `memory`, and the iteration it runs, are language semantics
(`docs/language.md` §3). A vendor's engine is written with the same stage,
and a fact about the vendor's hardware becomes an option of the language:
RBLN runs one prefill or a decode-only batch, and `examples/vendors/rbln.sq`
says so with `serve exclusive prefill`.

The design: an architecture declares the programmable blocks of an engine
(admission, selection, eviction, budget, serving order, a subset to serve),
which of them a target fixes, and the externs. A program names the
architecture it is written against. This sharpens criterion 2's test (can a
program state the opposite?), which a construct fails today for one of
two reasons. Either it is policy the language took from the program, or it
is a fact of the target. The first moves into the program, the second into
the architecture. **Policy is written in the program; a constraint is
written in the architecture.**

### 2. Moments are the signatures of the blocks

In P4 a control block declares in its signature the metadata it receives,
and the target supplies that metadata and nothing else. serQ's Moments
table (`docs/ir.md`, Moments) is the same structure without the name. Each
moment (`Select`, `Evict`, `Budget`, `Step`, `Serve`) is a programmable
block, and its context variables are the block's intrinsic metadata.
`Program::validate` already refuses an expression that reads a variable its
moment does not supply.

The design: the Moments table belongs to the architecture, so a target that
does not expose a quantity (an engine that cannot report `kv_prefill`)
removes it from that block's signature, and a program that reads it does
not link against that target. `hidden` is the same rule for user metadata.
An attribute the environment sets and the system cannot observe may be read
only where the target, not the scheduler, decides. A run's work is decided
by the model emitting its end token, and a hold's `cache` is read at
release, once the run has revealed the length. **A program is
synthesisable when every block reads only its signature.** That is a check,
not a semantics, so the linker can make it.

### 3. Externs: the time model belongs to the target

A P4 extern (a counter, a hash, a checksum unit) has a declared interface
and a target-supplied implementation. bmv2 has a software one, an ASIC a
hardware one, and the program cannot tell them apart.

A stage's `cost` is serQ's main extern. `cost c0 + max(omega + beta *
(kv_decode + kv_prefill), tokens * a)` is not the program's meaning. It is
the behavioural model of the forward pass, and a target replaces it with
the forward pass. A `delay` stage (a tool call) and a link's transfer time
are externs in the same sense.

The boundary decides which theorems survive synthesis. On the ℕ clock of
the Lean fragment, the scheduler's decisions are a function of the state
and the order of events. Durations matter only through the order they
produce. A theorem about decisions given an event order therefore holds of
a target as well:

- the fill of an iteration's budget in serving order (`Serq/Fill.lean`);
- serving order (`Serq/Serve.lean`);
- the oracle scenarios, which fix the clock;
- the eviction results of `Serq/Regen.lean`.

A queueing result (a mean response time, a stability condition) depends on
the extern's model and holds of the simulation only.

### 4. Parameters and a control plane: the model/instance split

A P4 program declares the shape of its tables (the keys, the actions). The
control plane fills in their entries at run time through an interface
(P4Runtime) that the compiler emits with the program (P4Info). The program
is the model and the entries are the instance.

serQ already has half of this. Every top-level `let` is a parameter that
`--set name=expr` overrides at link time. What is missing is the instance as
a document: an experiment is today a command line, or a copied program with
its constants changed. The other missing half is the interface: nothing
emits a program's parameters, observations and gauges as data that an
operator's tooling could read.

The design: an *instance file* is a list of `let` bindings and an optional
`run` block (`serq run prog.sq --instance a100.sq`). The linker checks each
name against the program's constants, as `--set` does. An instance changes
values and never structure. It rewrites to the `--set`s it is equivalent to,
so the IR does not change. The interface, the program's parameters,
observations, gauges and hidden attributes, could be derived from the IR
at no cost to it; it waits for a reader (self-critique).

### 5. Targets: reference, proof, and a real one

| P4 | serQ |
|---|---|
| p4c frontend and midend | `.sq` to the IR (`src/frontend/`) |
| bmv2, the reference target | the interpreter (`src/engine/`) |
| Petr4, the formal semantics | the Lean package (`lean/`) |
| p4testgen, tests from the program | the vLLM oracle (`tools/oracle/`) |
| a backend for a switch ASIC | **none** |

The first real target, before any scheduler is replaced, is vLLM's own
scheduler taken as fixed-function hardware, as a P4 program can target a
switch whose pipeline it may only configure. The scheduler runs one policy,
and its configuration gives that policy its values. A program whose
policies are vLLM's compiles to that configuration (`serq target`). A program
with another policy is refused, and the refusal names the construct: `serve
by` keys, `exclusive prefill`, `only`, selection or eviction keys, a spill,
a queueing stage. The oracle closes the loop. Each scenario's program
compiles to the configuration the oracle drove the real scheduler with
(`tests/target.rs`). The scheduler so configured decides as the program
does (`tests/vllm_oracle.rs`). This target already says which programs are
vLLM, which an architecture declared in serQ (step 5) will have to say as well.

The programmable target begins with serving order. vLLM's running loop
visits `running` in list order and preempts `running[-1]`
(`scheduler.py:767, 799`), so in vLLM one list order is both the serving
order and the victim. serQ keeps the two apart: the victim is the latest
admission, whatever the serving order. vLLM's PRIORITY path picks the
victim apart from the list order (`scheduler.py:761-797`).
`tools/serq_vllm.py` takes that path. It sorts `running` by the program's
keys before each step, gives each request its admission number as its
priority, and numbers preempted requests so that the waiting queue stays
FCFS. The keys it can evaluate are what the scheduler observes of a
running request: `decoding` and `admission`. That is the Serve block's
signature on this target (§2). The scenarios of `tools/oracle/serve/` are
chosen so that each order gives a different answer from FCFS in the
interpreter, and a control in admission order checks the PRIORITY plumbing.
Decode-first is not among them: in this engine it is admission order
already (`Serq/Serve.lean`, `serve_eq_decode_first`), and no random case
found it differ.

Run on vLLM at the pin (`0.30.1rc1.dev215+g0c87a197b`, CPU scheduler, the
oracle's fake model runner), the four scenarios agree with the interpreter,
request for request. The scheduler with no keys reproduces the six
committed scenarios, and in admission order it matches stock vLLM step for
step. Getting there found four bugs, each now held by
`tests/vllm_target_oracle.rs`:

- The scheduler ordered new waiting requests by `arrival_time`, which is
  when the request object was made, where FCFS is the order of
  `add_request`.
- The scheduler gave a preempted request its priority after
  `_preempt_request` had already requeued it (`scheduler.py:1580-1581`).
- The interpreter, under `serve by`, kept the tokens of a resident that a
  later growth preempted in the same iteration, and panicked costing it.
  It advanced the victim's computed position before the preemption. It
  also served the next resident after the grower preempted itself, where
  vLLM ends the running loop (`scheduler.py:779-813`). Admission order
  cannot reach any of the three: the victim is the last served.
- The vLLM programs capped every prefill at `chunk`. vLLM lifts the cap
  when one request is eligible (`scheduler.py:606-616`), and §7 cited those
  lines as if they were covered. The programs now say
  `chunk long_prefill(reqs, c)`, the Lean model has the rule
  (`Deployment.chunkLift`), and `serq target` refuses a constant cap.

The rest of the programmable target is vLLM's own engine with its
scheduler replaced.
`SchedulerConfig.scheduler_cls` takes a class or a qualified name
(`config/scheduler.py:178`). `get_scheduler_cls` loads it in place of
`Scheduler` (`config/scheduler.py:229-254`). The class implements
`SchedulerInterface` (`sched/interface.py:38`). Its `schedule` returns
the tokens to compute per request for one forward pass (`sched/interface.py:53-84`)
and `update_from_output` receives what the model runner produced
(`sched/interface.py:92-111`). The two map onto serQ's iteration: `schedule`
is the `Budget` and `Serve` blocks plus admission, and `update_from_output`
is the extern's return, which is where a hidden length is revealed one
token at a time. The model runner stays the extern. The oracle that today
compares the interpreter with vLLM's scheduler then compares a synthesised
scheduler with it, which is the equivalence check a backend needs.

## What is not taken from P4

- **A packet's stateless pass.** A P4 program processes one packet in a
  bounded pipeline; a serQ session is a long-lived process with state, and
  the workload is a stochastic process. The session block and the GSMP
  semantics ([the stochastic process](stochastic-model.md)) stay.
- **The ban on loops.** P4 forbids loops so that a packet is processed at
  line rate. The part of that kept here is narrower: the expressions a
  scheduler evaluates every iteration (serve keys, `only`, selection and
  eviction keys) are pure, do not draw, and cost a bounded number of
  operations per resident. The first two are rules already.
- **Syntax per target.** An architecture declares; it adds no syntax. One
  grammar, criterion 0.

## Order

Cheapest first, and the expensive step only after a target exists to test
it against.

| Step | What | IR |
|---|---|---|
| 0 | The position: README, the docs' front page, `docs/language.md` §1, this document | none |
| 1 | Instance files (`--instance`), rewritten to `--set` | none: link-time sugar |
| 2 | The synthesisability check for `hidden`: a hidden attribute is read by the target's decision only | none: a stricter check (`docs/ir.md` §Stability) |
| 3 | The interface: parameters, observations, gauges, hidden attributes and externs, as JSON derived from the IR | deferred until it has a reader |
| 4a | vLLM's scheduler as a fixed-function target (`serq target`): a program on its architecture compiles to the configuration that runs it, any other program is refused with the construct, and the oracle scenarios close the loop | none: a new consumer |
| 4b | A programmable target for the policies 4a refuses, checked by the oracle. First, serving order: `tools/serq_vllm.py` is vLLM's scheduler, and its running loop visits requests in the program's `serve by` order | none: a new consumer |
| 5 | Architectures declared in serQ: a target's provider writes its architecture (the blocks, what is fixed, what each block may read) as a program text, and the language checks a program against it with one mechanism | an RFC first |

Step 5 is where the language would learn of architectures, and it must
not learn of any one. The knowledge of vLLM in `src/target.rs` is a
backend's, as p4c's Tofino backend knows Tofino. What P4 keeps out of the
compiler is the architecture itself: `v1model.p4` and `tna.p4` are P4
source, written by the target's provider, and the compiler only matches a
program against what they declare. serQ's step 5 is the same: an
architecture is declared in serQ (for vLLM, beside `lib/vllm.sq`), and one
mechanism in the language checks a program against it. 4a and 4b are what
that declaration has to say for vLLM: the refusals in `src/target.rs`, and
the Serve block's signature (`decoding`, `admission`).

The full form is `Step` as a reference to a declared architecture, with
the Moments table as its signatures, and it stays the next shape. It would
move every consumer, and until a second architecture exists (4b against
real vLLM, or an RBLN target) its boundary is drawn from one example.

**Another kind: a gateway architecture.** ThunderAgent
([ThunderAgent-org/ThunderAgent](https://github.com/ThunderAgent-org/ThunderAgent)
at `7ddc861`, arXiv 2602.13692) is not an engine. It is a router in front of
several vLLM or SGLang backends, and it admits programs (agent sessions)
rather than requests. Its source:

- A program reserves its token count plus a 100-token buffer on one backend
  (`backend/state.py`, `remaining_capacity`). The reservation is held while
  the program reasons on the GPU and while it acts in a tool, with acting
  tokens weighted by `tool_coefficient`.
- A new program is placed on the backend with the fewest active tokens if
  the global queue is empty and it fits; otherwise it waits
  (`router.py`, `_select_backend_for_new_program`).
- Every 5 s a scheduler runs (`_scheduled_check`). An over-capacity backend
  pauses acting programs first, smallest first. A reasoning program is only
  marked, and is paused when it next becomes acting (`_pause_until_safe`).
  Paused programs resume in three priority groups: reasoning with a pending
  request, then new, then acting, each smallest first. They are placed
  across backends Best Fit Decreasing, and may move to another backend
  (`_greedy_resume`).

Measured against serQ today, half of this has a construct and half does
not:

| ThunderAgent | serQ |
|---|---|
| a gateway in front of engines, each a vLLM | a `gateway` queue with `route`, engines as a family, each on the `vllm` architecture |
| the least-loaded backend for a new program | `choose j in N by (…)` |
| a reservation held across a session's turns and tool calls | a `lease` outlives its hold until `release`, a time, or the session's end, but it is the KV, not a token count kept outside the engine |
| pausing an acting program: its reservation leaves while it is in a tool | none: a lease is neither evictable nor a preemption victim |
| a scheduler every 5 s | none: decisions happen at events (the stochastic-process document's candidate "shared iteration clock" is the nearest) |
| three resume groups, BFD across backends, migration on resume | queue keys order a pool's waiting holds; a resumed hold cannot move to another pool |
| token counts estimated from characters (ratio 5, momentum 0.2) | none: a program's attributes are exact |

ThunderAgent as an architecture is therefore an RFC, not a list of checks.
It needs a reservation the gateway owns that can be paused while the
session is in a tool, and a scheduler clock. Both are new semantics, and
both should meet the criteria on their own before an architecture names
them.

## Self-critique

- **Not "SSL".** The phrase is the category, but the acronym is Secure
  Sockets Layer's, and "SSL serving" finds TLS termination. The language is
  serQ, and the category is written out.
- **Not an HDL.** Hardware description languages fit the circuit and
  testbench picture, but for a serving engineer "description" reads as a
  manifest (Kubernetes, Helm, TOSCA), the opposite of a program whose
  policies execute. The HDL picture explains the structure (above). It is
  not the name.
- **Not a `param` keyword.** The [frontend sketch](frontend.md) declares
  parameters with `param` and types. Every top-level `let` is already
  overridable, so `param B = 8192;` next to `let B = 8192;` would be two
  spellings of one meaning (criterion 0). Making `let` a fixed definition
  instead would break every `--set` in the tutorial and the sweeps. Instance
  files reach the split without either. Units stay with #14.
- **The vLLM hook is not a public interface.** `get_scheduler_cls` warns
  that the interface "is not public and compatibility may not be
  maintained" (`config/scheduler.py:239-251`). A target built on it pins a
  vLLM revision, as `ref/vllm` does.
- **Not a named architecture in the language.** `architecture vllm;` was
  built: an optional IR field that `Program::validate` dispatched on by
  name to `src/target.rs`'s checks. It cost no version, but the language
  then knew vLLM, its policy hard-coded in the validator: criterion 2's
  failure, and the opposite of P4, whose architectures are source the
  compiler reads. A target's knowledge stays in its backend (`serq target`)
  until an architecture can be declared in serQ itself.
- **Decode-first is not a 4b scenario.** It was the first order tried. In
  this engine it equals admission order (`serve_eq_decode_first`), and 4000
  random cases found no difference, so it would test nothing the FCFS
  oracle does not.
- **Not yet the interface, nor an architecture without a backend.** Both
  were built and left out. `serq interface` emitted the parameters, reports
  and externs as JSON, but nothing reads it yet: P4Info is worth emitting
  because the control plane reads it. `architecture fastertransformer`
  wrote Dai et al.'s rule as checks, but FasterTransformer is a library
  that runs whatever batch its caller passes, so there is nothing to run
  the program on, and P4's architecture presupposes a target. It did show
  where architectures differ (the Serve block's signature, memory, the
  backend), which is what the full form of step 5 has to declare.
- **"Synthesisable" claims less than it sounds.** The step 2 check says a
  program does not read what the scheduler cannot see. It does not say a
  target exists that runs it, which only step 4 can show.
