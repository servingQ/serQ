# Distributions

```serq
~name(args…)
```

Sample a number each time the expression is evaluated. Arguments are required
numeric expressions. A wrong argument count is a link error. A draw is
allowed only at moments that permit sampling: `const` expressions and hold
headers cannot draw. See [evaluation moments](context.md).

## Distribution index

| Distribution | Parameters | Mean | Description |
|---|---|---|---|
| [`~exp(mean)`](#exp) | `mean: expr` | `mean` | Exponential. |
| [`~det(x)`](#det) | `x: expr` | `x` | Constant value. |
| [`~uniform(lo, hi)`](#uniform) | `lo, hi: expr` | `(lo + hi) / 2` | Continuous uniform. |
| [`~erlang(k, mean)`](#erlang) | `k, mean: expr` | `mean` | Sum of exponential phases. |
| [`~h2(mean, cv2)`](#h2) | `mean, cv2: expr` | `mean` | Balanced two-phase hyperexponential. |
| [`~bernoulli(p)`](#bernoulli) | `p: expr` | `p` | 0 or 1. |

## `exp` {#exp}

```text
~exp(mean: expr) -> number
```

**Parameters:** `mean` is the mean duration in the consumer's unit. Use a
positive finite mean for an exponential duration.

**Returns:** an exponential draw of mean `mean` and squared coefficient of
variation (CV²) 1. The parameter is a mean, not a rate.

## `det` {#det}

```text
~det(x: expr) -> number
```

**Parameters:** `x` is the value to return.

**Returns:** exactly `x`. It is still a draw expression for validation, so
it cannot be used where draws are forbidden.

## `uniform` {#uniform}

```text
~uniform(lo: expr, hi: expr) -> number
```

**Parameters:** lower and upper bounds `lo` and `hi`. Use finite ordered
bounds for a bounded duration.

**Returns:** `lo + (hi - lo) * U`, where `U` is uniform in `[0, 1)`.
Equal bounds return that value; floating-point rounding applies at the
endpoints. Mean: `(lo + hi) / 2`.

## `erlang` {#erlang}

```text
~erlang(k: expr, mean: expr) -> number
```

**Parameters:** `k` is the phase count, rounded down and taken to be at
least 1. `mean` is the total mean, not the mean of each phase; use a positive
finite value for a duration.

**Returns:** the sum of `k` independent exponential draws, each with mean
`mean / k`. Its CV² is `1 / k` after the phase-count conversion.

## `h2` {#h2}

```text
~h2(mean: expr, cv2: expr) -> number
```

**Parameters:** `mean` is the total mean; use a positive finite value for a
duration. `cv2` is the squared coefficient of variation, taken to be at least
1. For `cv2 = 1`, the law is exponential.

**Returns:** a draw from the balanced two-phase hyperexponential law.
For `v = max(cv2, 1)` and `p = (1 + sqrt((v - 1) / (v + 1))) / 2`, choose
an exponential of mean `mean / (2*p)` with probability `p`, otherwise one
of mean `mean / (2*(1-p))`.

## `bernoulli` {#bernoulli}

```text
~bernoulli(p: expr) -> number
```

**Parameters:** `p` is a probability in `[0, 1]`.

**Returns:** 1 with probability `p`, otherwise 0. The form
`branch with (p)` uses this draw; see [`branch`](statements.md#branch).

## Validation and reproducibility

The domains above describe the intended distributions, not a guarantee
that every invalid parameter is rejected at link time. The consuming
construct validates its own requirements: for example, a renewal gap must
be positive and finite when drawn or the run fails.

The run's `seed` initializes separate streams for arrivals, workload draws,
session statements and eviction keys. Workload draws are keyed by session
and turn: unchanged draw expressions with the same inputs reproduce the same
values across scheduling policies. See [workloads](../use-cases/workloads.md).

## Examples

A complete program with bursty arrivals and Erlang service times:

```serq
fn main() {
  stage svc : fifo;
  workload {
    arrive renewal(~h2(2, 4));
  }
  server {
    set t0 = now;
    run svc (~erlang(4, 1));
    observe latency = now - t0;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 1000 --seed 10
```

## See also

[Arrival processes](workload.md#arrive), [functions](functions.md),
[`pyserq.Rng`](../python/rng.md).
