# The stage algebra

A stage that is not a step engine is its throughput: φ(n), the work it does
per unit time when n jobs are present. Two stages combine in two ways a
deployment uses, pooling and holding at once, and each is an operation on
φ. This document states the operations and their laws, says which forms of
the language they already are, and why they are not operators.
`tests/stage_algebra.rs` checks the laws.

## A stage is φ

A `ps` stage serves each of its `present` jobs at `φ(present)/present`
([Stage](../api/stage.md#ps)), and its capacity expression may read
`present`, so φ is written in the program:

| Stage | φ(n), n ≥ 1 |
|---|---|
| `ps(c)` | c |
| `delay` | n, `ps(present)` |
| `fifo(c)` | min(n, c), `ps(min(present, c))`, in throughput |

`fifo(c)` and `ps(min(present, c))` do the same total work at every n; they
serve it in different orders, so they agree on the law of the number
present when the work is exponential and not on one job's sojourn. A
`step` stage serves in iterations, not at a rate, and has no φ: the algebra
is about the other three kinds.

## Pooling: ⊕

Two servers behind one waiting line serve n jobs at the best split of the
n between them:

> (φ₁ ⊕ φ₂)(n) = max over k = 0..n of φ₁(k) + φ₂(n − k)

the sup-convolution. On nondecreasing φ it is commutative and associative,
with the stage that does no work as its unit, and not idempotent: two
servers do more than one.

- `fifo(c₁) ⊕ fifo(c₂) = fifo(c₁ + c₂)`.
- `delay ⊕ delay = delay`.
- `ps(a) ⊕ ps(b)` is max(a, b) at n = 1 and a + b from n = 2:
  `ps(present == 1 ? max(a, b) : a + b)`. Finite sums of constant `ps`
  stages are `ps` stages again.

**Partition** is the other sum: each server keeps its own line, `stage
E[N]` and a `choose` that routes to one. Its state is the vector of what
each holds, not one φ, and the route is a policy the program writes. The
gap between the two sums is resource pooling, M/M/2 against two M/M/1.

## Holding at once: ⊗

A run that holds several stages, `run S₁, S₂ (X)` (`Run.also`), advances at
the rate its `share` gives it on each ([Bandwidth sharing](bandwidth-sharing.md)).
When every job holds both, under `share bottleneck`:

> (φ₁ ⊗ φ₂)(n) = min(φ₁(n), φ₂(n))

commutative, associative and idempotent, with the stage of infinite
throughput as its unit. Idempotence is why the IR's rule that a run names
a stage array once loses nothing: the rule is there because an index known
only when the run starts could name one stage twice
(`tests/shared_stages.rs`), and a stage held twice is the stage held once.

⊗ is not closed over stages. When some jobs hold S₁ alone and others S₁
and S₂, the rates are a max-min or bottleneck allocation over the flows,
and no φ of S₁ and S₂ alone gives them. Holding at once is a property of
the job's route, so the IR has it on `Run`, with `share` once per program,
and not as a stage combinator.

## Distributivity fails

> φ ⊗ (ψ ⊕ ψ) ≠ (φ ⊗ ψ) ⊕ (φ ⊗ ψ)

With φ = ψ = `ps(1)` and two jobs, the left is min(1, 2) = 1 and the right
2. The right copies φ: two jobs that share one link do not each get a link
by pooling what lies behind it. The pair (⊕, ⊗) is two commutative monoids
and not a semiring, because a stage is a resource and is not duplicated.

## Sequence

`run S₁ (w₁); run S₂ (w₂);` is a composition of the process, not of the
stages. In distribution it is a product: `fifo` with exponential work,
`ps` and `delay` are the BCMP stations of types 1, 2 and 3, and an open
network of them has the product of its stations' marginals as its
stationary law. A step engine, a `hold` that binds, or a run with `also`
leaves the product form, and those are what a program runs the simulator
for.

## Placement: where an exact algebra could live

⊕ and ⊗ depend on a policy because φ is a performance. What a job holds
does not: it is a map from the job's coordinate to stage indices, and a
map composes exactly. A family `E[N]` is a shape, an index expression is
the map, and `CRef {base, count, index}` already flattens any such map to
one array, so a (shape, stride) placement would be parse-time sugar. A
stride of 0 is sharing: several coordinates hold one stage, the φ of
§Distributivity left uncopied.

vLLM's read of the KV across tensor-parallel ranks is such a map. A
decoder of TP `tpD` reading a prefiller of TP `tpP ≤ tpD`: rank `r` reads
from prefiller rank `r * tpP // tpD` (`nixl/tp_mapping.py:98`) the head
chunk `r % (tpD / tpP)` (`nixl/tp_mapping.py:149`), so the prefiller-rank
mode has stride 0 inside each group of `tpD / tpP`. Under MLA the cache is
replicated and every decoder rank of the group reads all of it from that
rank (`nixl/base_worker.py:2170-2171`).

When every read fans out the same way, and every rank has a link of its
own (a property of the machine, not of vLLM), the map reduces to one
capacity per side, in tokens of the read per second:

| | prefiller side | decoder side |
|---|---|---|
| GQA, `tpD / tpP` dividing the heads | `tpP · BwP` | `tpD · BwD` |
| MLA | `BwP · tpP / tpD` | `BwD` |

A program can write that today, `nic ps(tpD * BwD);`. In
`examples/pd-disaggregation/llmd_nixl_pull.sq` with every prompt
prefilled remotely (`--set thr=1`), a TP-4 decoder's ingress (`BwD`
times 4) leaves the mean TTFT at 0.0311 s (λ = 0.6, seed 1) while the
ingress utilisation falls from 0.024 to 0.006, and at a tenth of the
bandwidth it moves from 0.1709 to 0.1708: the links do not bind in that
deployment.

A placement layout would earn its place where no capacity says the
sharing: a link shared by several pods (a stride-0 mode across a family,
pods of one node behind one NIC), where the reads of different pods
contend, or reads that fan out differently from request to request. No
program in the repository is one, so the layout waits for one that moves a
number.

## Checks

| Law | Test |
|---|---|
| `fifo(1) ⊕ fifo(1) = fifo(2)`: `fifo(2)` and `ps(min(present, 2))` have one mean sojourn, Erlang C's 16/7 at λ = 1.5 | `pooling_two_servers_is_a_server_of_two` |
| one server of capacity 2 is not two servers pooled: a lone job ends at 1/2 and 1 | `one_fast_server_is_not_two_servers` |
| distributivity fails: a shared `A` holds two jobs to 2, a copied one lets each end at 1, under either `share` | `a_shared_stage_is_not_copied` |
| a stage array named twice in a run does not link, so ⊗'s idempotence is never in question | `a_run_names_each_stage_array_once` (`tests/shared_stages.rs`) |

## Self-critique

**⊕ and ⊗ as operators were considered and rejected.** `stage dec : ps(a)
+ ps(b);` or `run egress ⊗ ingress (X);`:

- *Criterion 0.* `+` has two readings, one server of capacity a + b
  (`ps(a + b)`) and two servers pooled (the sup-convolution), and they part
  when a job is alone. `examples/pd-disaggregation/pd_ps.sq` declares its
  split as `ps(N * f)` and calls it N f engines; the two readings agree at
  the committed N f = 1 and differ by up to the factor N f above it (#243).
  Either meaning chosen for `+` reads wrong to someone.
- *Criterion 2.* The sup-convolution is a dispatch rule, the best split,
  the faster server first; the minimum is `share bottleneck`. An operator
  would fix both, and `share maxmin` exists so that a program can state
  the opposite of the second.
- *Criterion 1.* What the operators would replace reads better as it is:
  `fifo(4)` against four `fifo`s summed, `stage E[N]` with its `choose`,
  `run S₁, S₂ (X)`. The stage the programs are about, `step`, is outside
  the algebra.
- *Cost.* ⊕ on `ps` rewrites to a capacity expression at parse time and
  costs nothing, but ⊕ on `fifo` stages of different rates has no kernel
  form, and ⊗ as a combinator is not closed: both would be IR.

The algebra earns its place as laws a check holds the forms to, which is
criterion 3, and not as notation.
