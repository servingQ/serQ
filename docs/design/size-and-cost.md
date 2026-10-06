# Request sizes and resource costs

A workload specifies demand. A server interprets demand using its device,
allocation policy and current state. Request demand is not a service time or
an allocation, even when an identity conversion is a useful model.

## Contract

Workload assignments infer `Size`; the server can read but cannot assign
those slots. Server bookkeeping assignments infer `Value`. An explicit
`cost(resource, expression)` produces `Cost(resource)`; aliases and permitted
arithmetic retain its resource. `Run` consumes a stage cost, and `Hold`,
`Grow` and `Load` consume pool costs. No numerical value is implicitly a
cost in these primitive statements. A cost cannot be converted into another
resource's cost by wrapping it again.

The serving vocabulary (`prefill`, `decode`, `tool`, `transfer`) is the
named resource conversion followed by the primitive operation. A step
stage's work remains tokens and its iteration cost remains seconds. This
change does not reinterpret FIFO/PS work, which remains work in seconds at
rate one. A resource family has one cost type; choosing its member remains
the indexed primitive operation. A common cache/reuse amount, or a flow
across several stages, names all affected resources in its conversion.

`Size` expresses ownership, not dimensional analysis. Workload metadata,
continuation probabilities and thinking intervals remain expressible.
Demand is immutable *to the server*, not across a client's successive turns.
The server may calculate remaining work, chunk sizes and bounded output,
but it cannot replace the original demand slot. A server result can be a
separate bookkeeping value that the client uses after the request.

The client's session may execute costs of its own resources, such as a
thinking/tool stage or a conversation-wide hold. It cannot create or read
costs of resources that a server runs, acquires, grows or loads. Workload `init` and
`turn` never construct costs. Client release/drop at session termination
continues to describe the lifetime of its cached state. Resource lifetime
and permission to assign a request attribute are distinct.

## Before and after

Before, from `docs/tutorial/programs/02-memory.sq` at `fa794c5`:

```serq
    turn {
      set s = ~exp(S);
      set c = ceil(~uniform(1, 5));   // units this request needs
    }
```

The server used `hold mem (c)` and `run svc (s)`. The current runnable
replacement draws normalized work and a number of items in the workload.
The server declares its seconds per work unit and memory units per item:

```serq
    set s = cost(svc, S * work_size);
    set c = cost(mem, items);
    set t0 = now;
    hold mem (c) {
      observe admit_wait = now - t0;
      run svc (s);
    }
```

The distributions stay in the same workload stream. Changing a conversion
coefficient changes the deployment's cost, not the sampled demand.
`tests/size_cost.rs` uses a demand of three items and servers costing one
and two seconds per item: completion must be at three and six seconds.

The server boundary was previously erased by source expansion. A completed
source `turn;` now expands to a drawing `Turn` and the server statements, and retains the executing side for every IR statement, including nested bodies.
No request event, call frame, attribute reset or new random stream is added.

## IR and consumers

IR 12 adds `CExpr::Cost`, mandatory `attr_types` and a mandatory `sides`
table aligned with every statement in `blocks`. Numeric execution of a
conversion evaluates its argument once, at the original moment and on the
original random stream. The tables are validation data; the interpreter
continues to execute the existing statements. JSON cannot omit the tables
or skip type validation. A cost slot needs an assignment on every path
before use; its initial numerical zero is not a conversion.

Existing evaluation moments and resource ownership checks remain in force.
A conversion does not make a draw in an admission header legal, nor does it
hide a hidden-attribute dependency, a negative amount, or a zero-time loop.
Cost slots in a cache clause must be initialized at hold entry, since
release, rejection and preemption can read it before the body completes.
The expression visitors must traverse the conversion. Readers that already
know the target resource, such as a figure's stage label, display its
underlying amount.

The latest tag at implementation start, `v0.1.3`, uses IR 11. New expression
readers and the mandatory type contract are included in the untagged IR 12 alongside `While`; they do not bump it again. The Lean
expression translator explicitly erases conversions after accepting their
underlying expression. This preserves the executable fragment; the Lean
execution theorems do not claim to prove the Rust type checker sound.

## Self-critique

- A name-based ban on `s`, `memory`, or arithmetic cannot identify resource
  costs and would reject legitimate client data. Rejected.
- `cost` as an effect on a whole function would control resource access but
  would not distinguish a raw input amount from a resource-specific cost.
  Keep conversions and slot ownership in the IR.
- A new runtime `Request` node would require defining call-local lifetimes,
  fork inheritance and cache identity. Those are not needed for this
  contract. Retain the flat executable blocks and their statement sides.
- A single unindexed cost type would allow a pool cost to run a stage, or
  one pool's units to be assigned to another. Costs name resource families.
- A cost is not a `(time, memory)` pair evaluated at request arrival.
  Admission, iterations and incremental growth keep their own moments.
- This does not determine the physical meaning of an arbitrary number.
  A user can supply a normalized service requirement as a size and choose
  an identity conversion. The checkable promise is an explicit resource
  interpretation, not inference of what an author intended a number to mean.
- This is not a general request/response isolation mechanism, a dimensional
  unit system, a new hidden-information analysis, or an RNG redesign.
  Those require separate evidence and separate changes.

## Review and corrections

Two independent subagent reviews covered the six repository review questions.
They confirmed the narrower flat-IR design and identified concrete gaps:

- Cache validation assumed normal scope exit. An early `release` can read
  the clause before a later assignment, so cost slots used by it must be
  ready at entry. The regression uses release before the assignment.
- Lease validation checked an expression's internal type but not its
  consumer. Pool costs are now rejected as lease seconds.
- Erasing a family's index before checking it accepted unknown names.
  Indexed cost annotations now undergo normal name and static bounds checks
  before family projection. They never evaluate the index; actual resource
  operations select the member. This also preserves token-substituted defs.
- A serving form accepted a literal cost but rejected its alias because it
  recognized syntax. Serving forms now consistently convert ordinary
  quantities; neither form of already converted cost is accepted.

These are statically recognizable invalid programs, so the language rejects
them. Lean rounding translation also erases conversions below the rounding
operator, preserving the existing fragment rather than losing a valid case.

Requiring every cost annotation to be an unindexed source name was rejected:
a `def` parameter may be an indexed resource reference. A checked family
projection preserves that reusable program without a new runtime operation.

A final IR review found that classifying server resources from only `Hold`
and `Run` allowed client-created pool costs to reach a server `Grow` inside
a client hold. `Grow`, `Load` and a run's `growing` pool now participate in
the same classification. A client resource must not be a resource the
server consumes this way; no implicit cost handoff is permitted.

The release gate also exercised an existing large aggregate/index case.
The first fix separated conversion linking into a helper and made cost
validation iterative. It passed locally but still overflowed on Linux:
assuming a local pass guaranteed enough stack on other targets was wrong.
Aggregate expansion built a left-deep source tree, then recursively linked
or evaluated it, making stack usage proportional to the expanded count.
The linker now processes the separate terms before folding their results
left. It preserves the IR shape, floating-point grouping, evaluation order,
and the aggregate expansion budget without recursively linking that spine.

A regression reproduces the original 2100-term invalid index on a 1 MiB
thread stack and checks sum/min/max at the 4096-term limit. The test aborted
before the fix and now reports the expected range error. Three-term IR
comparisons and a rounding-sensitive constant sum check the left fold.
The program already has a static index check: this was an implementation
failure to reach it, not a missing language restriction. Increasing the
thread stack or balancing the sum was rejected: the former leaves the
linear stack dependency in place, while the latter changes rounding.

## Validation

`make check` passes release and debug tests, links and draws all 30 example
programs, and links the five tutorial programs. The 17 size/cost regressions
cover both source and direct IR. Oracle, scheduler-target, claim and Lean
regression IR files were regenerated. Their numeric oracle outputs and the
generated Lean statements are unchanged; deployment figure regeneration
also produced no changed figures.

`make lean` passes the oracle/claim freshness checks and build, with the
theorem audit using only the standard axioms. `mkdocs build --strict`
passes. The Python binding tests agree with the CLI at IR 12, and all 102
vLLM citations resolve against the pinned source.

## Integration with completed turns

The untagged IR 12 also contains `While` from the completed-turn frontend.
Its guard obeys the same scalar and resource-authority rules as a branch.
Cost initialization is checked before the guard and on a separate copy for
the body, so a zero-iteration path cannot acquire a cost initialized only
inside the loop. Server authority also covers the nested body. Regression
cases exercise all three boundaries.

The updated hidden-input analysis must treat `cost(resource[index], value)`
as evaluating only `value`. Counting the annotation's index as a definitely
read origin would incorrectly reveal that hidden input after a run. The
analysis now skips the resource annotations; the regression runs constant
work then attempts to branch on the hidden annotation index, which must be
rejected. This is a static visibility error, not a new execution policy.

The prefill/decode example returns a separate `accepted` server result when
a prompt is too long; the client's `while (more && accepted)` decides
whether to continue. This preserves the workload-owned `more` instead of
letting the server overwrite it.

## Explicit declarations and composite costs

The source can state its types directly:

```serq
Size items = 3;
Cost duration = cost(svc, 2 * items);
Cost processing = { mem: items, svc: 2 * items };
hold mem(processing.mem) { run svc(processing.svc); }
```

This excerpt uses the existing pool `mem` and stage `svc`. The runnable
second tutorial uses the same shape. A composite Cost contains independent
resource amounts. It differs from `CostTarget::Joint`, which applies one
common scalar amount to several resources, as needed by common cache/reuse
clauses or shared flows. Each composite field lowers to a resource-specific
scalar slot and conversion. Existing `set` inference remains available.
Explicit declarations are checked against inferred types, and conflicting
Size/Cost assignments remain invalid in source and JSON.

Declaration evaluates each field once in written order. Primitive statements
consume stored fields at their existing moments, and release/lease policy
remains in their scopes. The record does not acquire memory or schedule work
by itself. Plain FIFO/PS/delay work retains its existing seconds convention;
step-stage fields hold token work and iteration formulas determine elapsed
time. One Cost can describe
several pools and stages without treating those quantities as interchangeable.

Records use the existing flat session storage. Whole-record aliasing,
arithmetic, nested records and independent lexical lifetimes are not added.
Scalar field aliases retain the existing cost types. Record names cannot
also name scalars, constants or resources; repeated declarations retain the
same resource fields and order. Queue-local names are qualified by their
queue, while gateways keep the server's namespace. Member selection stays
at the consuming primitive, so resource field keys are unindexed families.

A review found that the initial parser map confused equal local record names
in different queues and accepted a record root that also named a scalar.
The wrong assumption was that dotted generated names alone gave the record
a namespace. Queue qualification and source root collision checks now reject
the ambiguity before execution. Another review found that an own local Cost
field in an admission header was reported as another queue's field. Dots no
longer imply external ownership in that diagnostic. The existing admission
rule still rejects body-local values; authors keep the conversion in the
header or pass an ordinary quantity. No runtime or IR restriction is needed.

Verification compares independent memory/time amounts, JSON round trips,
common pool scope release, declaration-time snapshots and written-order
sampling against an explicitly expanded scalar program. Type/side errors,
conditional initialization and namespace collisions have regressions.
