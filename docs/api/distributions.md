# Distributions

```seq
~name(args…)
```

A draw: an expression that samples a distribution each time it is evaluated. A
`const`, a `serve` key and an `at admission` binding may not draw. The result is a number, so `branch with (p)` is
`branch (~bernoulli(p))`.

| Signature | Parameters | Mean | Notes |
|---|---|---|---|
| `~exp(mean)` | `mean: expr` | `mean` | exponential |
| `~det(x)` | `x: expr` | `x` | the constant `x`, as a draw |
| `~uniform(lo, hi)` | `lo, hi: expr` | `(lo + hi) / 2` | continuous on `[lo, hi]` |
| `~erlang(k, mean)` | `k: expr`, `mean: expr` | `mean` | `k` phases, `k` rounded down and at least 1 |
| `~h2(mean, cv2)` | `mean: expr`, `cv2: expr` | `mean` | balanced two-phase hyperexponential; `cv2` is the squared coefficient of variation, and values below 1 are taken as 1 |
| `~bernoulli(p)` | `p: expr` | `p` | 1 with probability `p`, else 0 |

Each returns a number. An argument count other than the one above is a link error.

Draws in the workload, the session and eviction keys use separate random
streams, all seeded from `run`'s `seed`, so a change to one does not move the
others.
