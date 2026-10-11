# Select waiting requests on the admission clock

The FCFS configuration of tagged [vllm-ascend v0.27.1rc1](https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/core/short_request_first_scheduler.py#L148)
classifies requests into immediate, short and long lanes. At each selection,
immediate wins; an old enough long-lane head wins over shorts; otherwise
shorts win over longs. The tagged queue pins a peeked lane until its pop.
Elapsed waiting time is measured at selection, not captured at enqueue.

## Before and after

The existing IR v8 pool has `pub queue: Option<CExpr>`. Its interpreter
computes that key once in `enqueue_hold`, sorts the queue on insertion,
and subsequently tries its head. This source compiles in v8:

```serq
queue by (immediate ? 0 : prompt > 128 && now - queued >= 3 ? 1 : prompt <= 128 ? 2 : 3);
```

Here the session sets `queued = now` immediately before its hold. The long
request's key is fixed while `now == queued`, so aging cannot promote it.
A resident's `serve by` cannot change a waiting request's position.

IR v9 instead carries `pub queue: Option<Vec<CExpr>>`; the existing source
form accepts multiple keys and reevaluates them before every admission:

```serq
queue by (immediate ? 0 : prompt > 128 && waited >= 3 ? 1 : prompt <= 128 ? 2 : 3);
```

`waited` is supplied at `Moment::Select`, in simulation seconds since the
current hold entered the queue. It resets when that hold is re-enqueued.
The class expression is the program's policy: reverse the classes or remove
the aging term to specify a different policy. Several keys are compared
lexicographically; ties preserve enqueue order, so no numeric class offset
or serial-number bound is needed. A zero aging threshold can disable aging
with an explicit `max_wait > 0` guard, as in the executable example.

```serq title="examples/vendors/ascend.sq"
--8<-- "examples/vendors/ascend.sq"
```

## Selection and admission

One selection examines a snapshot of the waiting holds and their current
keys. It chooses an index, checks that same hold's reservation and admits
or blocks it; no second selection occurs between the check and removal.
No simulated time passes between these operations. This gives the
selection/check/commit behavior needed for the tagged peek/pop pair.

Both ordinary pool admission and `admit via` use the same selection function.
A stage-bound attempt exposes its current `budget_left(stage)` before
selection; after admitting one hold, the next attempt reevaluates the keys
against the updated budget and pool state. FIFO retains its order. A
preempted hold retains the existing prepend priority ahead of policy keys;
this generic recovery convention does not reproduce Ascend's lane-specific
prepend behavior in every configuration.

Only the selected hold is tested for fit. Failure blocks the queue; this
change does not introduce fit-first skipping. Keys may not draw and may not
read hidden attributes. A sampled prediction can be stored in a visible
session attribute. Admission is still triggered by the existing scheduler
attempts; no independent aging timer is created.

## Evidence and boundary

`tests/waiting_selection.rs` derives each expected order by hand. In the
aging regression a blocker holds the slot until 4; long 1 queues at 0,
shorts 2 and 3 at 1 and 2, immediate 4 and long 5 at 3. The threshold is 3.
IR v8's enqueue keys select `0,4,2,3,1,5`. Selection-time evaluation selects
`0,4,1,5,2,3`: long 1 is aged at 5, and long 5 becomes aged at 6.
Both admission paths and a JSON round trip run this case. Other tests cover
changing remaining budget within an iteration, lexicographic ties,
non-fitting selection, resumed priority and source/JSON validation.

The standalone example uses full-attention prefill/decode work with a
millisecond step clock. Its constants are illustrative. The tagged queue's actual classification/selection methods were executed
on a mocked monotonic clock and matched this example's admission order,
with aging enabled and disabled. The SDK, complete queue implementation
and scheduler were not executed. This is not an Ascend runtime test.
The policy models this FCFS aging rule. Priority-ordered lanes, dynamic
immediate predicates, job-level completion history, connector readiness,
remote recompute and hardware behavior have not been established.

## Compatibility and self-critique

The latest serQ tag `v0.1.0-rc6` carries IR v8. Selection timing and the
queue's JSON shape change, so this opens IR v9. Recompile source to obtain
v9 JSON. FIFO oracle files only change version; their expected answers and
the generated Lean programs stay unchanged. The companion generator reads
v9 FIFO programs and explicitly rejects non-FIFO queues. `Waited` and
selection policies are outside the Lean executable fragment.

Selection scans waiting requests and evaluates each key at every attempt.
This costs more than cached priority insertion; it supplies correctness for
time-dependent policy. Do not optimize it by caching dynamic keys. Shared
job predictors require separately justified state/update semantics; this
change does not claim to implement `BatchJobAwareRequestQueue`. Device
padding and shape IR remain deferred.
