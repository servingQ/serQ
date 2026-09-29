# Serving systems: declarations, implementations, workloads

Status: design proposal with executable semantic witnesses. The `def`, `impl`,
`bind`, `fn`, `component`, `connect` and typed-interface notation below
is a sketch, not accepted seQ syntax. The separate example fragments at the
end run through the existing parser. No module loader or new IR node is
implemented by this document.

## Decision

**`def` declares topology and semantics; `impl` binds configuration values
and named functions to its slots; a workload knows the request contract.**

Instance counts belong to the model. Two prefillers and four decoders are
a different topology from one prefiller and one decoder. An implementation
cannot change counts, add an edge or replace a component's protocol.

The composition is an open model, closed by supplying an implementation:
`elaborate(model, implementation, workload, run settings) -> closed IR`.
The model describes a family of processes parameterised by declared
configuration and expression slots; it does not fix numerical behaviour
until those slots are filled. The resulting IR remains the executable and
verifiable definition of one program.

This supersedes the placement of the gateway name in the workload in
[Explicit gateways](explicit-gateways.md). Explicit binding was the right
requirement, but `request gw;` put that binding on the wrong side.

## Who owns what

| Concern | Declaration / model | Implementation |
|---|---|---|
| Topology | Components, **instance counts**, ports, entry point, connections, one-to-one/shared bindings | Cannot change it |
| Protocol | Request/response fields, visible information, state transitions, return and error paths | Cannot add hidden protocol steps |
| Admission | Wait vs reject, ordering, acquisition/release scope, re-evaluation moment | Capacity, thresholds, service-time formula if declared |
| Routing | Eligible ports and workflows, decision points, allowed observations, selection/reselection moment | Required router implementation when there is a choice; declared thresholds and weights |
| Engine scheduling | Step mechanism, admission order, preemption/recovery, memory ownership | Pool capacities, block sizes, request limits |
| Budget | Result type, valid input state, evaluation moment, validity constraints | Value or pure expression |
| Cost | Result type, valid input state, evaluation moment, validity constraints | Constants, calibrated formula, explicit table/profile |
| Workload | Separate client process using the request contract | Its own arrival/trace/turn parameters, not serving topology |
| Experiment | Binding of a workload to a serving model and implementation | Seed, horizon, warmup; separate from hardware calibration |

A literal `1` meaning "one request occupies one permit" is semantic, not
a hardware constant. The number of permits is configuration. Similarly,
the last-token recomputation rule is semantic; block size and available KV
capacity are configuration. A FIFO policy or a recovery rule does not move
to implementation merely because it currently appears beside a number.

Some policies may deliberately be parameters. In that case the declaration
must expose a policy contract with allowed inputs and effects. Routing is
one such contract: an implementation chooses among declared alternatives,
but cannot introduce an edge, change the workflow or move the decision
across admission. A generic configuration dictionary must not silently
acquire the power to replace the model's semantics.

## Definitions, implementations and functions

The outer boundary is `def` / `impl`. `def` names a reusable component or
composed serving model. It owns topology, instance counts, resource and
protocol semantics, and the signatures of required configuration and
functions. `impl` names the definition it completes and binds those slots
to external configuration values and named functions. These replace the
earlier sketch's `model` and `implementation` keywords; they are not extra
spellings.

A definition does not create an instance: a `component` declaration does,
with the count fixed by its containing model. An `impl` cannot add an
instance or edge, replace the admission protocol or change the evaluation
moment. In particular, `def` is more than a field layout: the process that
acquires a permit and releases it on completion remains part of the model.

The declaration/implementation boundary applies to functions too. A
`fn` signature ending in `;` declares an implementation slot in `def`.
A module-level `fn` supplies the reusable computation; a `bind` in `impl`
connects it to the slot. Importing a function does not bind it automatically.
An `impl` is a static binding table, not another process body.

Signature excerpts, not complete component definitions or runnable syntax:

```text
def Router {
  config: RouterConfig;
  fn choose_decode(req: Request, candidates: DecodeCandidates,
                   cfg: RouterConfig) -> DecodeTarget;
}

def Decode {
  config: EngineConfig;
  fn budget(state: BudgetState, cfg: EngineConfig) -> Tokens;
  fn cost(batch: StepBatch, cfg: EngineConfig) -> Seconds;
}

// Policy module: these can be imported and reused by other components.
fn token_budget(state: BudgetState, cfg: EngineConfig) -> Tokens =
  cfg.max_batch_tokens;
fn affine_step_cost(batch: StepBatch, cfg: EngineConfig) -> Seconds =
  cfg.base_seconds + cfg.seconds_per_token * batch.ntok;

// Binding excerpt; profile.decode is an immutable EngineConfig value.
impl Decode {
  bind config = profile.decode;
  bind budget = token_budget;
  bind cost = affine_step_cost;
}
```

`EngineConfig` is a declared record schema for capacities, block size,
token budget and timing coefficients. It has no instance-count field.
Its coefficients carry the indicated units; they are not builtin
measurements. A constant budget is still computed by a function at the
declared budget moment. The model passes its bound `config` and the
appropriate state view when invoking each hook; passing config is
explicit in the function contract, not an ambient global lookup.

At composition time, `bind config` resolves and validates an immutable
value, while `bind cost` resolves a function symbol and checks its
signature. It does not invoke the function. Runtime calls then receive the
dynamic batch and bound config. Different instances can reuse the same
function with different config values, without runtime function pointers,
closures or an implicit default implementation.

The composition explicitly selects one root `impl` for the serving
definition, with one binding per required instantiated slot. Alternative
profile modules may supply different implementations; imports do not
merge them or resolve conflicts by file order. The standalone `impl Decode`
above and the root `impl PDServing` below are separate examples, not two
layers to apply cumulatively. Generic traits, inheritance and overlapping
implementation rules are not needed for this boundary.

The complete definitions also declare their ports, protocol, resources
and evaluation moments. `Gateway` contains the router and admission
controller definitions; the router's functions return choices, while the
gateway's declared process performs admission, dispatch and completion.
The signature excerpt does not replace that process with an arbitrary
callback. Each routing decision still has a separate required slot: a
`choose_decode` body cannot fill `choose_prefill` or the local/remote slot.

The first `fn` mechanism should be deliberately small:

- Functions return values and have no process effects. They cannot acquire
  a pool, run a stage, wait, mutate session state, sample or alter topology.
  Such operations remain in the component's declared process. A router's
  simulated service time is charged by that process, not by executing its
  selection function in the host language.
- Inputs are explicit, read-only views. A budget receives pre-selection
  state, cost receives the selected batch, and routing receives visible
  request fields and candidate metrics exposed by its contract. No ambient
  access to hidden output length, global queues or mutable client state is
  gained by moving an expression into a function. Immutable implementation
  constants and explicitly imported pure helpers can be in lexical scope.
- A call evaluates at its enclosing hook's declared moment. Function
  parameters denote argument values at that call, not expressions that
  can be replayed after a wait. Helper calls must retain the same visibility
  and moment restrictions transitively.
- A router must return a member of the supplied candidate set. Role type
  alone is insufficient: an unrelated decoder is still an invalid result.
  The frontend must prove membership for a supported selection form or
  provide a checked lowering; this is not established by the scalar cost
  witnesses in this PR.
- Start with named, statically resolved calls and expression bodies. Reject
  recursion and call cycles; closures and functions stored in runtime data
  are outside this proposal. Preserve both the definition and call-site
  locations in diagnostics.

For example, `fn budget(state) = cost(batch)` must not gain access to a
batch that has not been selected yet. Missing required bindings, wrong
signatures, duplicate bindings and unknown component paths are composition
errors. Filling a budget or router slot cannot change instance counts.

Pure scalar helpers are candidates for expansion into existing expressions
with hygienic bindings. The frontend must establish that equivalence with
tests before claiming no IR change. Typed state views, candidate sets and
target-valued routing functions need their own lowering design; introducing
`fn` does not automatically make those types supported by today's IR.

## Topology is a set of bindings, not a linear execution schedule

Proposed declaration notation:

```text
def PDServing {
  component gw     : Gateway;
  component P[2]   : Prefill;
  component D[2]   : Decode;
  component nic[2] : Transfer;

  entry gw.request;

  connect gw.prefillers -> P;
  connect gw.decoders   -> D;
  connect D[*].transfer -> nic[*];
}
```

**These lines are where connections are defined.** `gw.prefillers` and
`gw.decoders` are required ports declared by `Gateway`; they are not global
names discovered inside its code. A call in `Gateway` may use these ports,
its own resources and its request, but cannot reach `P`, `D` or another
component by a global name. Port resolution substitutes the explicit
bindings during elaboration.

`D[*].transfer -> nic[*]` is an elementwise binding, requiring equal counts.
A shared NIC would instead be one declared instance with an explicitly
shared binding. Neither a missing index nor equal array sizes should
silently choose between shared and elementwise ownership.

The remote workflow can be drawn as `gw -> P -> D`, and the local workflow
as `gw -> D`. Those are **derived execution paths**. They are not another
source definition. In the current `llmd_pd.seq` program the gateway calls
both P and D; P does not call D. The `from P[i]` dependency identifies the
KV source for the decoder. A diagram should distinguish call connections,
KV transfer and the admission scope instead of assigning all three one
unlabelled arrow.

The topology graph shows possible dependencies. A path view also reads the
component's branches, preserves guards and order, and need not enumerate
unbounded loops. Drawing must not claim every topological path is a valid
request execution.

## A gateway is optional; an unresolved choice is an error

A serving declaration need not include a gateway. A direct decoder or a
fixed prefill-then-decode flow is a valid system. If the system includes a
gateway, that gateway is its entry point: a declaration cannot include it
and silently enter a downstream engine instead. An unused gateway defined
in another module is not a member of that system. The initial contract has
one system entry; multiple ingress gateways require an explicit outer
composition rather than an inferred default.

Check choices **at each declared decision point**, after resolving ports
and expanding the model's instance counts. Count eligible destinations,
not all instances in the program:

- A binding to `D[0]` has one candidate even when `D` has four instances.
- A binding to all four instances has four candidates.
- Two separately named singleton decoders also give two candidates.
- Prefill and decode can themselves be alternatives at one decision:
  `gw -> P` versus `gw -> D` requires routing even with one P and one D.
- A fixed `P -> D` sequence is two steps, not two choices. Selecting among
  multiple P or multiple D instances within either step is a choice.
- A declared choice between a remote `P -> D` workflow and a local `D`
  workflow requires routing even when both use the same singleton decoder.

Connections specify eligible dependencies; the component's declared flow
specifies which are successive steps and which are alternatives. Edges to
both P and D alone cannot resolve that distinction. This must be explicit
in the flow contract, not guessed from role names or graph traversal.

For every reachable decision with more than one candidate, require a
router implementation **in the entry gateway** that covers that decision.
A gateway with only admission control does not satisfy this requirement.
Having a router for decoder selection does not satisfy an unresolved
prefiller or local/remote choice. With no gateway, the diagnostic asks for
both a gateway and its router. There is no automatic first-member, random,
round-robin or least-work policy. With a singleton candidate, forwarding
is mechanical; a router is optional, as is admission control.

The checker runs when the declaration and implementation are composed,
before lowering or simulation. A declaration may leave its router hook
open; an executable composition may not leave a required hook unfilled.
Dynamic conditions or a runtime mask do not prove a singleton at compile
time. Any such selection belongs to the router contract. Duplicate target
bindings and empty candidate sets are wiring errors, not ways to avoid
the router requirement. The router must return an eligible destination or
declared workflow; selecting a disconnected target is invalid too.

Required diagnostics, **specimens for the future frontend**, not output
currently emitted by the parser:

```text
error: ambiguous decode routing at gw.decoders
  candidates: D[0], D[1]
  help: implement a router in gateway `gw` for this decision
```

```text
error: ambiguous serving entry: P.prefill or D.decode
  help: add a gateway as the server entry point and implement its router
```

The diagnostic must point to the candidate binding, name the decision and
available destinations, and point to the missing router implementation
slot. Large families can be reported as `D[0..31] (32 candidates)`.

Acceptance cases for that frontend:

| Declared flow / binding | Gateway / router | Result |
|---|---|---|
| Direct `D[0]`, with other unconnected replicas | Neither | Valid |
| Fixed singleton `P -> D` | Neither | Valid |
| Gateway forwards to one D | Gateway, no router | Valid |
| Gateway performs admission, forwards to one D | Gateway, no router | Valid |
| Choice among `P[2]` or among `D[2]` | Gateway, no router | Error: implement the gateway router |
| Choice between singleton P and singleton D | Gateway, no router | Same error |
| Choice between remote and local workflows | Gateway, no router | Same error |
| Several candidates, no gateway | Neither | Error: add gateway and router |
| Router covers D selection but P selection is unresolved | Gateway, partial router | Error names the P decision |
| Every choice has a router implementation | Gateway and router | Valid, subject to the remaining type/protocol checks |
| Gateway is a system member but entry bypasses it | Gateway | Entry-point error |

## A gateway contains admission control and routing

`Gateway` is a composite component, not necessarily one physical queue or
one service stage. Its declaration may contain an ingress waiting queue,
an admission controller with permits, and a router with its own service
resource. Those resources have different lifetimes.

- An inflight permit may cover the entire downstream request.
- A router CPU slot covers routing work only.
- A pending-queue slot covers waiting before admission.
- A rate-limit token is consumed/replenished by its declared mechanism;
  it is not a concurrency permit returned when the request ends.
- Decoder memory and running slots are downstream resources with their
  own admission rules; gateway acceptance does not imply decoder admission.

Their counts and relationships are declared. Their capacities and timing
functions are implementation slots. Router and admission-controller
contracts are independent: one does not stand in for the other. An
idealised router can have a zero-cost implementation without erasing its
selection policy. A gateway with a single fixed path can omit a router.

Illustrative control flow, not a new mandatory gateway algorithm:

```text
Gateway.request(req) {
  with admission.acquire(req) as permit {
    router.process(req);
    d = router.choose_decode(req, decoders, router.config);
    if router.needs_remote(req, d, router.config) {
      p = router.choose_prefill(req, prefillers, router.config);
      kv = prefillers[p].prefill(req);
      return decoders[d].decode(req, from = kv);
    }
    return decoders[d].decode(req);
  }
}
```

The body defines the allowed workflows and decision **sequence** over
ports. The system definition binds those ports to concrete components.
The implementation supplies the router's selection policy at those hooks.
No separate list of local/remote paths repeats the branch. The router can
choose D before P, as the current example does, without implying that D
executes first. A model whose flow includes both branches requires the
local/remote policy as well as any non-singleton endpoint selection policy.

Here the permit ends after downstream completion, not when the router
finishes dispatch. Another model may route first to choose a tenant or
destination-specific admission controller. The language must not bake in
"admission always precedes routing". If a request waits after selection,
the model must say whether the selection is retained or repeated; moving
the same expression across the wait can change behaviour.

In the current IR, nested holds can preserve an outer gateway permit while
an inner decoder hold is replayed. KV leases require their existing
source/destination lifetime rules. The `kv` in this sketch denotes that
request's source lease, not a copy of KV memory or a new runtime object
type promised by this RFC. Existing serial transfers can lower to the
current indexed pool reference and `run; load; release` sequence.

## Workload isolation includes data, not just names

The workload must not name a gateway, replica or NIC. It constructs a
request, submits it through the experiment's serving binding, and handles
the response. It owns session history, arrival/think-time distributions and
follow-up behaviour.

The request contract exposes data such as prompt length and a declared
output limit. Actual future generated length is simulation input, not
knowledge available to a router or admission controller. Currently
`hidden` protects scheduler expressions, while a gateway body has broad
session access; a port/interface implementation must close that gap rather
than infer isolation from the absence of `P` or `D` names in the workload.

The existing example also computes `prompt = K + n` in the gateway and
reads `prompt` back in the workload. A full interface migration should have
the client construct the prompt field and receive explicit response fields,
instead of letting the serving model reach arbitrary client variables.

For rejection, the serving side returns a rejected **request outcome**;
the client decides whether to retry, back off or end the session. Ending
the entire session is not an equivalent implementation of request rejection.
Pending timeouts, cancellation and exactly-once cleanup need explicit
contracts before they can be claimed to work. This RFC's executable witness
covers wait-based admission and successful synchronous completion only.

## Implementation slots have contracts

Example binding sketch for the fixed topology above. `profile` is an
imported immutable configuration record; function names refer to explicitly
defined or imported policies with signatures matching their target slots:

```text
impl PDServing {
  bind gw.admission.config = profile.gateway.admission;
  bind gw.router.config = profile.gateway.router;
  bind gw.router.choose_decode = least_work_decode;
  bind gw.router.choose_prefill = least_work_prefill;
  bind gw.router.needs_remote = remote_over_threshold;
  bind gw.router.cost = affine_route_cost;

  bind P[*].config = profile.prefill;
  bind P[*].budget = token_budget;
  bind P[*].cost = affine_step_cost;

  bind D[*].config = profile.decode;
  bind D[*].budget = token_budget;
  bind D[*].cost = affine_step_cost;

  bind nic[*].config = profile.transfer;
  bind nic[*].cost = transfer_cost;
}
```

`D[*]` fills the same declared slot on each existing instance; it does not
instantiate decoders. P and D use the same engine config schema and pure
functions here, with different bound values. Their protocols are still
defined by their respective component definitions.

For example, `profile.decode` could contain `kv_capacity = 160000`,
`kv_block = 16`, `max_batch_tokens = 512` and the two timing coefficients
read by `affine_step_cost`. These are illustrative values, not measured
claims. There is no `replicas` or `instances` setting. The record's fields,
types and constraints must match the declared schema; a generic map of
unvalidated settings cannot replace it.

Per-instance bindings may address existing paths such as `D[1]`. Choose
either a uniform `D[*].config` binding or disjoint indexed bindings for
that slot; combining a wildcard and an overlapping indexed binding is an
error, not a last-write-wins override. `D[2]`, an unknown config key or a
missing required slot must fail elaboration. Changing from 2P2D to 4P8D
requires another declared topology (or an explicit model-level structural
variant), never an implementation binding.

The routing requirement uses this same binding check. Two candidates at
`gw.decoders` require `bind gw.router.choose_decode = ...`; an admission
config alone, or a function merely imported into the module, does not
complete that slot. Zero-cost routing still needs an explicit selection
function whenever there is a choice.

Budget and cost are expression bodies, not values computed while loading
configuration. A budget sees the declared pre-selection state. A step cost
sees the selected batch. `ntok` can appear in a cost expression but not in a
budget expression that determines that very batch. The existing
`ir::Moment` checks provide part of this contract. Scope, units, legal
sampling and evaluation frequency must be specified for every exposed
hook; moving its body to another file must not change any of them.

The initial implementation can support pure expression hooks, including
router selection. The policy names above denote proposed definitions, not
builtins that the parser already accepts. Their state views and tie-breaking
must be specified by those definitions. No arbitrary
host-language plugin, I/O callback or opaque hardware object is needed;
these would hide the process from IR validation and the oracle consumers.

## Imports name definitions

Only after interfaces and model definitions exist should a program say,
for example, `import { Gateway, Prefill, Decode } from "std/serving"`.
Those exports need to contain the entry/port signatures, information
visibility and behaviour contracts that elaboration actually uses.

Language syntax (`def`, `impl`, `bind`, `fn`, resource scopes, expressions
and connections) remains syntax. Named component definitions, typed config
values and pure helper functions are importable; workload definitions and
implementation profiles can also have their own modules. A role import
must not merely toggle a hardcoded keyword table.
The full multi-file frontend must retain original filenames and spans in
diagnostics. The concatenation used by the tests below is only a witness,
not the proposed module system.

## Executable Before and After

Before, copied from `programs/llmd_pd.seq` at `6b0b0e7`:

```seq
      request gw;
```

The following After uses **existing supported syntax**:

```seq
server { gw.route(); }
// Inside the workload's session:
request;
```

It explicitly chooses the serving entry outside the workload. It does not
yet isolate gateway dependencies through ports; concrete P/D references
remain in the current gateway body. A regression test compares its entire
IR with the previous spelling, so moving the binding cannot change the
current simulation.

The deterministic witness is split into four source fragments:

- [Model](programs/gateway-model.seq): topology, fixed counts and
  admission/routing/lease behaviour.
- [Implementation](programs/gateway-implementation.seq): capacities and
  time constants.
- [Workload](programs/gateway-workload.seq): two clients and their requests.
- [Run](programs/gateway-run.seq): horizon.

`tests/gateway_composition.rs` concatenates them through the existing
frontend and checks results derived by hand:

| Case | Expected result |
|---|---|
| One inflight permit; remote then local | Admit at 0 and 8; complete at 8 and 12 |
| Two inflight permits, same single FIFO router | Admit both at 0; local completes at 5, remote at 8 |
| Bind the same workload directly to the decoder | Both complete at 3; gateway has no admissions or service |
| Change capacities and service constants only | Counts remain 1 P, 2 D, 2 NIC; complete at 7.5 and 10 |
| Five tokens, budget 2, cost `1 + 0.5 * ntok` | Batches 2,2,1; completion at 5.5 |
| Same declaration, budget 3, same cost expression | Batches 3,2; completion at 4.5 |
| Read `ntok` when computing that batch's budget | Rejected by IR moment validation |

The last three budget/cost checks bind AST expression slots in a test
helper; they do not implement `def` / `impl` / `bind` / `fn` syntax. The tests
establish feasibility for these serial cases, not the correctness of a
future port type checker, loader, rejection path or cancellation protocol.
In particular, the existing parser has no declarative candidate bindings
or optional-router check: the acceptance matrix above is a specification
for that work, not a claim about these six tests.

## Static checks the new frontend must earn

1. Every required port is bound exactly once; target role and entry
   signatures match. No undeclared global component references survive.
2. Family counts and elementwise/shared mappings are fixed by the model.
   An implementation cannot mutate either.
3. Every required implementation slot has exactly one binding, correctly
   typed and valid for its evaluation moment. Config schemas reject
   unknown keys; functions must match the target signature. Overlapping
   wildcard/indexed bindings and unknown slots fail.
4. Workload and component names are in distinct scopes. Only request fields,
   response fields, declared observations and permitted context cross them.
5. Resource acquisition, leases and cleanup follow the declared protocol.
   Existence of a topology edge alone does not prove a request owns a lease.
6. Elaboration retains module and source provenance for errors and produces
   the same closed IR as a hand-written kernel program for supported cases.
7. Gateway membership fixes the entry point. Every reachable routing
   decision with multiple candidates has a gateway router implementation;
   missing or partially supplied routers produce the diagnostics above.

## Alternatives and self-critique

**A path list such as `gw -> P -> D | gw -> D`.** It omits endpoint selection,
resource lifetimes, request outcomes and the chosen KV source. Adding those
back creates a second process language, while copying the paths beside an
existing route body creates two authorities. Keep it as a derived view.

**Move routing and admission into the top-level system block.** The gateway
then becomes a label with its actual policies elsewhere. Keep behaviour in
the component, expressed through ports; place bindings in the topology.

**Only an entry plus a component list.** This leaves all dependencies hidden
in component bodies and does not check their wiring. Required ports are
what makes the composition explicit.

**An automatic router, or a mandatory gateway for every system.** A default
router silently chooses a policy; a mandatory gateway invents a component
for a direct, deterministic flow. Require the gateway and router when a
declared choice needs them, and allow explicit admission-only gateways.

**Put all knobs in implementation.** Some "knobs" change semantics: FIFO
versus priority, retaining versus reselecting a route, waiting versus
rejecting. Expose them as deliberate model contracts or change the model.
Instance counts are structural regardless of being integers.

**Use `def` and `fn` as interchangeable function keywords.** That adds two
spellings for one construct. The proposed split earns two names only if
`def` defines components and their processes, while `fn` computes values.
The outer definition/implementation boundary is `def` / `impl`; `fn`
declares the slots and defines the separately bound computations.

**Write function bodies inline in `impl`.** This mixes selecting a profile
with defining its policies. A binding table makes reuse and missing slots
explicit, and lets one named function run with different immutable config
values. Keep one way to fill a function slot: bind a named definition.

**Put the whole gateway in an implementation function.** An unrestricted
function could reorder admission, dispatch to undeclared destinations or
finish the permit early. Keep the process in the component definition;
bind implementation functions only at declared policy and numerical slots.

**All of this is sugar.** Only the already-supported sequential, scoped
behaviours have that evidence. Concurrent dispatch, queued timeout,
cancellation or request-result control flow may require kernel work and a
cross-repository IR handshake. Do not disguise them as an import feature.

**Interfaces remove every coupling.** They do not remove shared-resource
effects or upstream backpressure. They make the dependency and the visible
information explicit; physical coupling remains in the resource semantics.

## Implementation order

First move the entry binding out of the workload using the existing server
block and pin the semantics with the witnesses above. Next add explicit
ports and static wiring for the existing roles, including scalar, family
and elementwise bindings, optional gateway entry and required-router
diagnostics. Then add reusable `def` definitions, typed config schemas,
`fn` signatures and pure bodies, and `impl` binding validation, preserving
counts in the model. Add imports
when there are real model/interface definitions to export. Handle new
request outcomes and concurrency with their own semantic tests and IR
decision, rather than attaching them to the syntax change.
