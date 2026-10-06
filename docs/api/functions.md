# Functions

Every function takes a fixed number of arguments of named kinds
(`expr`, `pool`, `stage`) and returns a number. A call with the wrong count or
kind is a link error.

## Arithmetic

Pure: usable in `const` position.

| Signature | Returns |
|---|---|
| `min(a: expr, b: expr)` | the smaller |
| `max(a: expr, b: expr)` | the larger |
| `abs(x: expr)` | absolute value |
| `floor(x: expr)` | greatest integer ≤ `x` |
| `ceil(x: expr)` | least integer ≥ `x` |
| `sqrt(x: expr)` | square root |
| `exp(x: expr)` | `eˣ` |
| `ln(x: expr)` | natural logarithm |
| `pow(x: expr, y: expr)` | `xʸ` |

Operators: `+ - * / ^`, comparisons `< <= > >= == !=` giving 0 or 1, `&&`,
`||`, `!`, and `c ? a : b`. `^` is `pow`.

## Observables

These functions read live state and return a number. They cannot appear in
`const` expressions or a claim’s `given` or `at end` expressions. Other
restrictions depend on the construct: for example, `only` predicates cannot
read `work`. See [evaluation moments](context.md).

### Stage

| Signature | Returns |
|---|---|
| `queue(s: stage)` | jobs present at `s`, waiting or in service |
| `busy(s: stage)` | jobs in service (`fifo`: active servers; `step`: jobs in the current iteration) |
| `work(s: stage)` | unfinished work at `s`, in the stage's work unit |
| `est_lambda(s: stage)` | measured arrival rate `λ̂` at `s`, per clock unit |
| `est_rho(s: stage)` | measured utilisation `ρ̂` at `s` (capped below 1) |
| `est_wait(s: stage)` | measured mean wait `Ŵ` at `s` |
| `price(s: stage, s_hit: expr, ds: expr)` | online price of a miss at `s`, below |

For a miss that lengthens a hit service $s_{\mathrm{hit}}$ by $\Delta s$,
let $s_{\mathrm{miss}} = s_{\mathrm{hit}} + \Delta s$. Using the stage’s measured
arrival rate $\hat\lambda$, utilisation $\hat\rho$ and mean wait $\hat W$:

$$
\Phi = \Delta s
  + \frac{\hat\lambda(s_{\mathrm{miss}}^2-s_{\mathrm{hit}}^2)}{2(1-\hat\rho)}
  + \frac{\hat\lambda\hat W\Delta s}{1-\hat\rho}.
$$

### Pool

| Signature | Returns |
|---|---|
| `used(p: pool)` | units allocated |
| `free(p: pool)` | `cap − used` |
| `cachedin(p: pool)` | this session's own cached units in `p` (0 with no session) |
| `holders(p: pool)` | sessions holding units in `p` |
| `queued(p: pool)` | sessions waiting at `p` |

### Step stage

| Signature | Returns |
|---|---|
| `budget_left(s: stage)` | tokens the next iteration leaves after its residents. Meaningful in the header of a hold in a pool with `admit via s`; elsewhere it plans the next iteration. `s` must be a `step` stage. |

### Declarations

| Signature | Returns |
|---|---|
| `blocksize(p: pool)` | the `block` of pool `p`. The linker folds it to a number, so it may appear in any expression that is not in `const` position (a `let`, a pool's `cap` or `block`, an array size, `horizon`, a rate). A pool without `block` is a link error; an array is indexed as for any pool function. |

A definition that takes a pool reads its block size from it:
`reusable(known, blocksize(kv))` in `lib/vllm.sq`.

## Aggregates of the observations

Read by a [`claim`](program.md#claim) `at end` only, over every value the
run observed under `o`, warm-up included.

| Signature | Returns |
|---|---|
| `total(o: observe)` | the sum of the values |
| `count(o: observe)` | how many values |
| `largest(o: observe)` | the greatest value (0 when none) |
| `smallest(o: observe)` | the least value (0 when none) |
| `prefix_total(o: observe)` | `Σ_k (v_1 + … + v_k)` over the values sorted ascending: the least total completion time of jobs of those sizes served one at a time |

## Where an observable may be read

Observables read state that eviction and admission change, so a `set` that
reads one and reaches a hold's header is a link error: the header is read at
admission and the `set` was read before the session queued. Use
[`at admission`](statements.md#hold).
