# Distributions

```serq
~name(args…)
```

A draw samples a distribution each time the expression is evaluated. It is
allowed only where the construct’s expression rules permit sampling; for
example, `const` expressions and hold headers cannot draw. The result is a
number: `branch with (p)` is `branch (~bernoulli(p))`.

| Signature | Parameters | Mean | Notes |
|---|---|---|---|
| `~exp(mean)` | `mean: expr` | `mean` | exponential |
| `~det(x)` | `x: expr` | `x` | the constant `x`, as a draw |
| `~uniform(lo, hi)` | `lo, hi: expr` | `(lo + hi) / 2` | continuous on `[lo, hi]` |
| `~erlang(k, mean)` | `k: expr`, `mean: expr` | `mean` | `k` phases, `k` rounded down and at least 1 |
| `~h2(mean, cv2)` | `mean: expr`, `cv2: expr` | `mean` | balanced two-phase hyperexponential; `cv2` is the squared coefficient of variation, and values below 1 are taken as 1 |
| `~bernoulli(p)` | `p: expr` | `p` | 1 with probability `p`, else 0 |

Each returns a number. An argument count other than the one above is a link error.

The run’s `seed` initializes separate streams for arrivals, workload draws,
session statements and eviction keys. Workload draws are keyed by session
and turn: unchanged draw expressions with the same inputs reproduce the same
values across scheduling policies. See [workloads](../use-cases/workloads.md).
