# Functions

Numeric functions take a fixed number of arguments. The `cost` constructor
additionally carries a resource type; all values execute as numbers (`f64`).
`expr`, `pool`, `stage` and `observe` in signatures describe argument kinds;
they are not part of the call syntax. Arguments are required and positional.
A wrong count, wrong kind, or unavailable evaluation context is a link error.
See [types](index.md#types) and [evaluation moments](context.md).

## Function index

**[Arithmetic](#arithmetic)**

| Function | Returns |
|---|---|
| [`min(a: expr, b: expr)`](#min) | The smaller argument. |
| [`max(a: expr, b: expr)`](#max) | The larger argument. |
| [`abs(x: expr)`](#abs) | The absolute value of `x`. |
| [`floor(x: expr)`](#floor) | The greatest integer less than or equal to `x`, represented as a number. |
| [`ceil(x: expr)`](#ceil) | The least integer greater than or equal to `x`, represented as a number. |
| [`sqrt(x: expr)`](#sqrt) | The square root of `x`. |
| [`exp(x: expr)`](#exp) | The exponential `eˣ`. |
| [`ln(x: expr)`](#ln) | The natural logarithm of `x`. |
| [`pow(x: expr, y: expr)`](#pow) | `x` raised to the power `y`; the operator `x ^ y` is equivalent. |

**[Stage](#stage)**

| Function | Returns |
|---|---|
| [`queue(s: stage)`](#queue) | Jobs present at `s`, including waiting and in-service jobs. |
| [`busy(s: stage)`](#busy) | Jobs in service: active servers for `fifo`, scheduled jobs in the current iteration for `step`. |
| [`work(s: stage)`](#work) | Unfinished work at `s`, in the stage's work unit. |
| [`est_lambda(s: stage)`](#est_lambda) | Measured arrival rate at `s`, per clock unit. |
| [`est_rho(s: stage)`](#est_rho) | Measured utilization at `s`, capped below 1. |
| [`est_wait(s: stage)`](#est_wait) | Measured mean queue wait at `s`, in clock units. |
| [`price(s: stage, s_hit: expr, ds: expr)`](#price) | An online estimate of the added cost of a cache miss, in clock units. |

**[Pool](#pool)**

| Function | Returns |
|---|---|
| [`used(p: pool)`](#used) | Units currently allocated in `p`. |
| [`free(p: pool)`](#free) | `cap - used`. Cached units are reclaimable and are not subtracted. |
| [`cachedin(p: pool)`](#cachedin) | This session's own cached units in `p`; 0 when evaluated without a session. |
| [`holders(p: pool)`](#holders) | Sessions holding units in `p`. |
| [`queued(p: pool)`](#queued) | Sessions waiting in `p`'s admission queue. |

**[Engine](#engine)**

| Function | Returns |
|---|---|
| [`budget_left(s: stage)`](#budget_left) | Tokens available to admissions after planning service for the current residents. |

**[Declarations](#declarations)**

| Function | Returns |
|---|---|
| [`blocksize(p: pool)`](#blocksize) | The allocation block size declared for `p`. |

**[Aggregates of the observations](#aggregates-of-the-observations)**

| Function | Returns |
|---|---|
| [`total(o: observe)`](#total) | Sum of the observed values; 0 when there are none. |
| [`count(o: observe)`](#count) | Number of observed values; 0 when there are none. |
| [`largest(o: observe)`](#largest) | Greatest observed value; 0 when there are none. |
| [`smallest(o: observe)`](#smallest) | Least observed value; 0 when there are none. |
| [`prefix_total(o: observe)`](#prefix_total) | Sum of prefix sums after sorting the observed values ascending; 0 when there are none. |

## Resource conversion

### `cost` {#cost}

```text
cost(resource, …, expression) -> Cost(resources)
```

Names one or more pool or stage families, followed by the quantity to
interpret. The result has the same numeric value, evaluated once at the
containing expression's original moment. It does not allocate, advance
time, resample, or capture an admission value early. The consuming
statement does the resource operation.

```serq
run svc (cost(svc, seconds_per_item * items));
hold mem (cost(mem, bytes_per_item * items)) { /* use the allocation */ }
```

One cost type belongs to an entire resource family. Prefer `cost(kv, n)`.
An indexed annotation such as `cost(kv[j], n)`, including a reference
substituted by a `def`, projects the same family type. Its names and
statically invalid indices are checked, and it cannot draw, but `j` is
not evaluated. The actual `hold kv[j]` or `run engine[j]` selects and
checks the member at its normal execution moment.

For common `cache`/`reuse` amounts across several pools, or work on several
stages together, name every affected family: `cost(kv, reqs, n)` or
`cost(egress, ingress, bytes)`. Order and duplicates do not change the type.
A cost of one resource cannot be used as another's or passed to `cost`
again. See [attribute types](attributes.md#sizes-values-and-costs).

Serving forms (`prefill`, `decode`, `tool`, `transfer`) perform this
conversion themselves and accept ordinary quantities. Pass an already
converted cost to the primitive `run`, `hold`, `grow` or `load` instead.
A stage's declarative `cost expr` specifies its iteration duration and is
already a resource formula; it takes ordinary quantities, not session
`Cost` values. The same applies to a spill's declarative work formula.

## Arithmetic

Pure functions, available in `const` positions. Numeric arguments are
expressions evaluated at the containing expression's moment. Operators are
`+ - * / ^`, comparisons `< <= > >= == !=` (0 or 1), `&&`, `||`, `!`, and
`c ? a : b`.

### `min` {#min}

```text
min(a: expr, b: expr) -> number
```

**Returns:** The smaller argument.

### `max` {#max}

```text
max(a: expr, b: expr) -> number
```

**Returns:** The larger argument.

### `abs` {#abs}

```text
abs(x: expr) -> number
```

**Returns:** The absolute value of `x`.

### `floor` {#floor}

```text
floor(x: expr) -> number
```

**Returns:** The greatest integer less than or equal to `x`, represented as a number.

### `ceil` {#ceil}

```text
ceil(x: expr) -> number
```

**Returns:** The least integer greater than or equal to `x`, represented as a number.

### `sqrt` {#sqrt}

```text
sqrt(x: expr) -> number
```

**Returns:** The square root of `x`.

For real-valued results, use `x >= 0`. A negative input produces NaN.

### `exp` {#exp}

```text
exp(x: expr) -> number
```

**Returns:** The exponential `eˣ`.

Large inputs can overflow to infinity.

### `ln` {#ln}

```text
ln(x: expr) -> number
```

**Returns:** The natural logarithm of `x`.

For a finite real result, use `x > 0`. Zero gives negative infinity; a negative input produces NaN.

### `pow` {#pow}

```text
pow(x: expr, y: expr) -> number
```

**Returns:** `x` raised to the power `y`; the operator `x ^ y` is equivalent.

Floating-point arithmetic applies; a result outside the real domain can be NaN.

## Observables

Stage, pool and step-stage observables read live state. They cannot appear
in `const` positions, a claim's `given`, or a claim `at end`. Construct-specific
restrictions still apply; see [evaluation moments](context.md).

### Stage

#### `queue` {#queue}

```text
queue(s: stage) -> number
```

**Returns:** Jobs present at `s`, including waiting and in-service jobs.

#### `busy` {#busy}

```text
busy(s: stage) -> number
```

**Returns:** Jobs in service: active servers for `fifo`, scheduled jobs in the current iteration for `step`.

#### `work` {#work}

```text
work(s: stage) -> number
```

**Returns:** Unfinished work at `s`, in the stage's work unit.

Not permitted in an engine's `only` predicate or a schedule's `branch` guard.

#### `est_lambda` {#est_lambda}

```text
est_lambda(s: stage) -> number
```

**Returns:** Measured arrival rate at `s`, per clock unit.

#### `est_rho` {#est_rho}

```text
est_rho(s: stage) -> number
```

**Returns:** Measured utilization at `s`, capped below 1.

#### `est_wait` {#est_wait}

```text
est_wait(s: stage) -> number
```

**Returns:** Measured mean queue wait at `s`, in clock units.

#### `price` {#price}

```text
price(s: stage, s_hit: expr, ds: expr) -> number
```

**Returns:** An online estimate of the added cost of a cache miss, in clock units.

For a miss that lengthens a hit service $s_{\mathrm{hit}}$ by $\Delta s$,
let $s_{\mathrm{miss}} = s_{\mathrm{hit}} + \Delta s$. Using the stage’s measured
arrival rate $\hat\lambda$, utilisation $\hat\rho$ and mean wait $\hat W$:

$$
\Phi = \Delta s
  + \frac{\hat\lambda(s_{\mathrm{miss}}^2-s_{\mathrm{hit}}^2)}{2(1-\hat\rho)}
  + \frac{\hat\lambda\hat W\Delta s}{1-\hat\rho}.
$$

**Parameters:** `s` names the measured stage; `s_hit` is hit service time, and `ds` is the extra service time of a miss.

### Pool

#### `used` {#used}

```text
used(p: pool) -> number
```

**Returns:** Units currently allocated in `p`.

#### `free` {#free}

```text
free(p: pool) -> number
```

**Returns:** `cap - used`. Cached units are reclaimable and are not subtracted.

#### `cachedin` {#cachedin}

```text
cachedin(p: pool) -> number
```

**Returns:** This session's own cached units in `p`; 0 when evaluated without a session.

Read at admission for a prefix lookup that accounts for eviction while the session waited. See [`hold`](statements.md#hold).

#### `holders` {#holders}

```text
holders(p: pool) -> number
```

**Returns:** Sessions holding units in `p`.

#### `queued` {#queued}

```text
queued(p: pool) -> number
```

**Returns:** Sessions waiting in `p`'s admission queue.

### Engine

#### `budget_left` {#budget_left}

```text
budget_left(s: stage) -> number
```

**Returns:** Tokens available to admissions after planning service for the running requests.

The argument must name an [engine](engine.md). In a hold header on a pool the engine admits (`pool reqs on s`, `pool kv on s.gpu`), this is the next iteration's admission budget. Elsewhere it plans the next iteration.

### Declarations

#### `blocksize` {#blocksize}

```text
blocksize(p: pool) -> number
```

**Returns:** The allocation block size declared for `p`.

The pool must declare `block`; otherwise linking fails. Although folded by the linker, this call is not allowed in a `const` position, such as a `let`, capacity or array size. A pool array requires an index. A pool-parameter definition can use `reusable(known, blocksize(kv))`.

## Aggregates of the observations

Only available in a [`claim`](program.md#claim) `at end`. `o` names an
`observe` declaration. These functions include values from warm-up, unlike
the report's measured observation samples.

### `total` {#total}

```text
total(o: observe) -> number
```

**Returns:** Sum of the observed values; 0 when there are none.

### `count` {#count}

```text
count(o: observe) -> number
```

**Returns:** Number of observed values; 0 when there are none.

### `largest` {#largest}

```text
largest(o: observe) -> number
```

**Returns:** Greatest observed value; 0 when there are none.

### `smallest` {#smallest}

```text
smallest(o: observe) -> number
```

**Returns:** Least observed value; 0 when there are none.

### `prefix_total` {#prefix_total}

```text
prefix_total(o: observe) -> number
```

**Returns:** Sum of prefix sums after sorting the observed values ascending; 0 when there are none.

`prefix_total(o) = Σ_k (v₁ + … + vₖ)` for values sorted ascending. For job sizes this is the least total completion time when jobs run one at a time.

## Examples

A complete program using arithmetic, live state and end-of-run aggregates:

```serq
fn main() {
  pool reqs { cap 2; }
  stage svc : fifo;
  workload {
    arrive batch(3);
  }
  server {
    set t0 = now;
    hold reqs (cost(reqs, 1)) {
      observe allocated = used(reqs);
      run svc (cost(svc, ceil(1.2)));
    }
    observe response = now - t0;
  }
  claim completed : at end (count(response) == 3);
  claim elapsed : at end (largest(response) == 6);
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## Where an observable may be read

Observables read state that eviction and admission change, so a `set` that
reads one and reaches a hold's header is a link error: the header is read at
admission and the `set` was read before the session queued. Use
[`at admission`](statements.md#hold).

## See also

[Context variables](context.md), [distributions](distributions.md),
[user-defined functions](program.md#def), [claims](program.md#claim).
