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

Read live state. They may appear at any moment (a queue key, a step stage's
`cost`, an eviction key), but not in `const` position. All return a number.

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

`price` is `missPrice` with the stage's measured estimates. For a miss that
lengthens a hit service `s_hit` by `ds`, with `s_miss = s_hit + ds`:

```
Φ = ds + λ̂ (s_miss² − s_hit²) / (2 (1 − ρ̂)) + λ̂ Ŵ ds / (1 − ρ̂)
```

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
| `blocksize(p: pool)` | the `block` of pool `p`. The linker folds it to a number, so it may appear wherever a number may, except in `const` position (a `let`, a pool's `cap` or `block`). A pool without `block` is a link error. |

A definition that takes a pool reads its block size from it:
`reusable(known, blocksize(kv))` in `lib/vllm.seq`.

## Where an observable may be read

Observables read state that eviction and admission change, so a `set` that
reads one and reaches a hold's header is a link error: the header is read at
admission and the `set` was read before the session queued. Use
[`at admission`](statements.md#hold).
