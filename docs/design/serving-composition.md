# Serving systems: declarations, implementations, workloads

Status: design proposal with executable semantic witnesses. The `model`,
`implementation`, `component`, `connect` and typed-interface notation below
is a sketch, not accepted seQ syntax. The separate example fragments at the
end run through the existing parser. No module loader or new IR node is
implemented by this document.

## Decision

**A model declares topology and semantics; an implementation fills its
configuration and expression slots; a workload knows the request contract.**

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

## Topology is a set of bindings, not a linear execution schedule

Proposed declaration notation:

```text
model PDServing {
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
    d = router.choose(decoders, req);
    if router.needs_remote(req, d) {
      p = router.choose(prefillers, req);
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

Example implementation sketch for the fixed topology above:

```text
implementation Measured of PDServing {
  gw.admission.capacity = 64;
  gw.router.choose(decoders, req, state) = least_work(state);
  gw.router.choose(prefillers, req, state) = least_work(state);
  gw.router.needs_remote(req, d) = req.prompt > remote_threshold;
  gw.router.cost(req) = route_base + route_per_token * req.prompt;
  gw.router.remote_threshold = 512;

  P[*].kv.capacity = 128000;
  P[*].kv.block = 16;
  P[*].budget(state) = 512;
  P[*].cost(batch) = c0 + a * batch.ntok;

  D[*].kv.capacity = 160000;
  D[*].kv.block = 16;
  D[*].budget(state) = 512;
  D[*].cost(batch) = d0 + b * batch.ntok;

  nic[*].cost(copy) = x0 + copy.tokens / bandwidth;
}
```

The coefficient names must be supplied by this implementation's own
configuration; the numbers and formula shapes above are illustrative,
not measured claims. There is no `replicas` or `instances` setting here.
Per-instance overrides may specialise existing paths such as `D[1]`;
`D[2]`, an unknown key or a missing required slot must fail elaboration.
Changing from 2P2D to 4P8D requires another declared topology (or an explicit
model-level structural variant), never an implementation override.

Budget and cost are expression bodies, not values computed while loading
configuration. A budget sees the declared pre-selection state. A step cost
sees the selected batch. `ntok` can appear in a cost expression but not in a
budget expression that determines that very batch. The existing
`ir::Moment` checks provide part of this contract. Scope, units, legal
sampling and evaluation frequency must be specified for every exposed
hook; moving its body to another file must not change any of them.

The initial implementation can support pure expression hooks, including
router selection. `least_work` above is a proposed policy definition, not
a builtin that the parser already accepts. Its state view and tie-breaking
must be specified by the selected implementation. No arbitrary
host-language plugin, I/O callback or opaque hardware object is needed;
these would hide the process from IR validation and the oracle consumers.

## Imports name definitions

Only after interfaces and model definitions exist should a program say,
for example, `import { Gateway, Prefill, Decode } from "std/serving"`.
Those exports need to contain the entry/port signatures, information
visibility and behaviour contracts that elaboration actually uses.

Language syntax (`model`, resource scopes, expressions and connections)
remains syntax. Components and their model definitions are importable;
workload definitions and implementation profiles can also have their own
modules. A role import must not merely toggle a hardcoded keyword table.
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
helper; they do not implement `implementation { ... }` syntax. The tests
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
3. Every implementation slot is supplied, correctly typed and valid for
   its evaluation moment; unknown slots and conflicting overrides fail.
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
diagnostics. Then add model parameter/expression signatures
and implementation checking, preserving counts in the model. Add imports
when there are real model/interface definitions to export. Handle new
request outcomes and concurrency with their own semantic tests and IR
decision, rather than attaching them to the syntax change.
