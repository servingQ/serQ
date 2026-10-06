# pyserq.Rng

```python
pyserq.Rng(seed: int)
```

Create the generator used by the interpreter: rand 0.9's `StdRng`, seeded
with `seed_from_u64`. Each method advances this object's state.

## Parameters

| Parameter | Type | Description |
|---|---|---|
| `seed` | `int` | From 0 through `2**64 - 1`. The same seed and sequence of method calls reproduce the same values for this implementation. |

Negative or out-of-range seeds raise `OverflowError`; wrong types raise
`TypeError`.

## Methods

| Method | Returns | Range |
|---|---|---|
| [`next_u32()`](#next_u32) | `int` | 0 through `2**32 - 1`. |
| [`next_u64()`](#next_u64) | `int` | 0 through `2**64 - 1`. |
| [`random_f64()`](#random_f64) | `float` | `[0, 1)`. |
| [`range_u32(low, high)`](#range_u32) | `int` | `[low, high]`, both ends included. |
| [`range_u64(low, high)`](#range_u64) | `int` | `[low, high]`, both ends included. |
| [`range_f64(low, high)`](#range_f64) | `float` | `[low, high]`, both ends included. |

## next_u32

```python
Rng.next_u32(self) -> int
```

Return the next unsigned 32-bit random integer. No arguments.

## next_u64

```python
Rng.next_u64(self) -> int
```

Return the next unsigned 64-bit random integer. No arguments.

## random_f64

```python
Rng.random_f64(self) -> float
```

Return a uniform floating-point draw from `[0, 1)`. No arguments.

## range_u32

```python
Rng.range_u32(self, low: int, high: int) -> int
```

Draw uniformly between inclusive integer bounds. Both bounds must be
between 0 and `2**32 - 1`.

**Raises:** `ValueError` if `low > high`, `OverflowError` if a bound is
outside the unsigned range, or `TypeError` for the wrong type.

## range_u64

```python
Rng.range_u64(self, low: int, high: int) -> int
```

As `range_u32`, with bounds between 0 and `2**64 - 1`.

## range_f64

```python
Rng.range_f64(self, low: float, high: float) -> float
```

Draw uniformly between inclusive floating-point bounds.

**Raises:** `ValueError` if bounds are reversed, NaN, infinite, or their
difference is not finite; `TypeError` for the wrong type.

## Examples

```python
import pyserq

a, b = pyserq.Rng(10), pyserq.Rng(10)
assert [a.next_u64() for _ in range(3)] == [b.next_u64() for _ in range(3)]
assert 0.0 <= a.random_f64() < 1.0
assert a.range_u32(7, 7) == 7
```

A run seeded `s` uses `Rng(s)` for its arrival stream. Its other draws use
separate streams; one `Rng(s)` does not reproduce every draw in the run.
See [distributions](../api/distributions.md) for workload stream behavior.

## See also

[`compile(seed=...)`](compile.md), [distributions](../api/distributions.md).
