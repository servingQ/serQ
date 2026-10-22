# The price of a miss is the program's

`price(s, s_hit, ds)` is gone from the language. It was one paper's
formula, `missPrice`, the added cost of a cache miss at a stage, made a
builtin. A program could not write another price in its place, and the
test of criterion 2 (#8) is whether a program can state the opposite: the
language may supply a mechanism a program selects and parameterises, not
a rule the program would otherwise write. The mechanism here is the
stage's online estimates, `est_lambda(s)`, `est_rho(s)` and `est_wait(s)`,
and they stay. The formula over them is a rule, and a program now writes
it as a `def`.

The IR loses the function `Fun::Price`, so `IR_VERSION` goes from 15
(tagged, `v0.1.7-alpha.2`) to 16. No program of this repository called
it (`tools/unused.json` listed it). Two programs of the companion
repository `serving-queue-theory` do, and they keep their calls (below).

## Before and after

The function, `src/ir.rs:540-541` and `:4214` before this change:

```rust
    /// online price of a miss at a stage: `price(stage, s_hit, ds)`
    Price,
```

```rust
            Fun::Price => f("price", &[S, E, E], &["s", "s_hit", "ds"], St),
```

Its formula, `src/engine/stats.rs:382-389`:

```rust
    /// Price of a miss that lengthens a hit service `s_h` by `ds`
    /// (`missPrice` with the measured wait):
    /// `Φ = ds + λ(s_m² - s_h²)/(2(1-ρ)) + λ W ds/(1-ρ)`.
    pub fn price(&self, s_h: f64, ds: f64) -> f64 {
        let (lam, rho, w) = self.estimates();
        let s_m = s_h + ds;
        ds + lam * (s_m * s_m - s_h * s_h) / (2.0 * (1.0 - rho)) + lam * w * ds / (1.0 - rho)
    }
```

A call, `serving-queue-theory`'s `programs/replay_vllm.sq:81`:

```text
    : (waiting ? 1 : p) * price(vllm, c0, a * size + b * size * size / 2)
```

After, the same formula is the program's, as `docs/api/functions.md`
shows it beside `est_wait`:

```serq
// the price of a miss that lengthens a hit service s_hit by ds at stage s
def miss_price(s, s_hit, ds) {
  ds + est_lambda(s) * ((s_hit + ds) * (s_hit + ds) - s_hit * s_hit) / (2 * (1 - est_rho(s)))
    + est_lambda(s) * est_wait(s) * ds / (1 - est_rho(s))
}
pool kv on gpu { block bs; evict by (waiting, miss_price(vllm, c0 + a, a * size) / size); preempt lifo; }
```

A call of the builtin is refused with what to write:

```text
link error: unknown function `price`: `price(s, s_hit, ds)` is gone: a miss's price is the program's, a `def` over `est_lambda(s)`, `est_rho(s)` and `est_wait(s)`
```

The message is an entry of `RETIRED_FUNCTIONS` (`src/frontend/link.rs`),
beside `pow`. A call is told; the name is free for anything else.

## The same run

`examples/multi-turn/vllm.sq` with its KV pool's `evict lru` replaced by
`evict by (waiting, price(vllm, c0 + a, a * size) / size)` was run before
the change (`1cb55ea`), and with the `def` above in its place after it:

```text
serq run X.sq --workload examples/workloads/chat.sq --horizon 600 --blocks 800 --json
```

The two JSON reports are the same byte for byte: 896 entries evicted from
`kv` (5401488 units). The pool's own `evict lru` evicts 848, and
`evict by (waiting, a)`, the price replaced by a constant, 847, so the
key the estimates give is live, not a tie. The same `def` named `price`, the
companion's spelling, gives the same report, and so do a one-line `def`
over `vllm` alone and the three-line one above. In the IR the call
`Price` becomes `EstLambda` twice, `EstRho` twice and `EstWait` once;
the `def` leaves no trace of its name.

The sameness is by construction, not luck. The `def` performs the
builtin's floating-point operations in the builtin's order: `s_hit + ds`
is computed twice and gives the same value both times, `x * x` is written
out (`^` is `powf`, which need not round as a product does), and each
`est_*` reads the same `estimates()` the builtin read. The eviction key
stays one whose entries are keyed once for a pass that evicts several
(`static_key`, `src/engine/interp.rs`), which already counted the three
estimates among what such a key may read.

`tests/engine_device.rs`,
`a_miss_price_is_a_definition_over_the_estimates`, keeps this: the
builtin's call is refused with the message, a `def price(s, s_hit, ds)`
links and its IR reads the estimates 2, 2 and 1 times, and the `def`
named `miss_price` gives the same IR.

## What went

`Fun::Price`, its row in `FUNS` and its entry in `Fun::entry`
(`src/ir.rs`); its evaluation in `Interp::call` and its place in
`static_key` (`src/engine/interp.rs`); `PriceEstimator::price`
(`src/engine/stats.rs`; the estimator stays, its comment now says what
reads it); its place among the functions whose value may be fractional
(`src/ir/whole.rs`); its summary in `src/frontend/highlight.rs`; its row
and section in `docs/api/functions.md`, its line in
`docs/reference/cheatsheet.md` and in `docs/language.md`'s limitations;
`price` in `docs/hooks/serq_lexer.py`'s `BUILTINS`. `tools/unused.json`
drops `Price` from `Fun`, and `tools/metrics.json` counts one function
fewer.

## IR 16

A removed variant bumps a tagged version (`docs/ir.md` §Stability). The
Lean generators move only their pin: `scripts/gen_lean_oracle.py` reads
16 and still reads 7 to 15, since a call was never in the fragment (it
ends at `Fragment("call …")`) and no oracle program had one, so a 15
file and a 16 file translate alike; `scripts/gen_lean_claims.py` pins
16. `lean/` does not change. The oracle, claims, Lean regression and
golden queue IR move their `version` field and nothing else.

## The companion's transition

`serving-queue-theory` pins a released serQ, so its programs do not break
now. `programs/open_vllm.sq` (three calls) and `programs/replay_vllm.sq`
(one) take the change when they move the pin to a release that has it,
by one line each above `fn main()`, where a definition that reads none
of main's names goes (`docs/writing-programs.md`):

```text
def price(s, s_hit, ds) { ds + est_lambda(s) * ((s_hit + ds) * (s_hit + ds) - s_hit * s_hit) / (2 * (1 - est_rho(s))) + est_lambda(s) * est_wait(s) * ds / (1 - est_rho(s)) }
```

Their calls stay as written, because a retired function's name is free.
From then on they are the programs that use `est_lambda`, `est_rho` and
`est_wait`, which `tools/unused.json` lists today: `scripts/unused.py`
does not read the companion's programs.

## Self-critique

**A library, `use "std/price"`, that defines `price`.** It keeps the
spelling for every program. But the language would still supply the
rule, now as a second spelling of a `def` over `est_*`, in frontend code
that nothing verifies. The two programs that call it write one line
instead.

**Keeping the builtin and adding a way to write another formula.** The
`def` already is that way. A builtin beside it is a second meaning for
the same expression and the rule criterion 2 refuses.

**Refusing the word `price` outright.** A reserved word would say "gone"
to a `def price` as well as to a call, and the companion's migration
would have to rename every call site. `RETIRED_FUNCTIONS` tells only a
call, as it does for `pow`.

**An example that evicts by price, so that `est_*` leave
`tools/unused.json`.** No example here runs the paper's priced policy:
`vllm.sq` is vLLM's LRU, and `replica.sq` checks itself against
libqueuingsim's shortest-first eviction. A program written so that a
construct counts as used is the path only tests reach under another
name. The companion's programs are the users.

**Removing `est_*` too, since no program here uses them.** They are
measurements a program selects and parameterises, a mechanism criterion
2 allows; the formula over them was the rule. The companion needs them.

**Renaming `PriceEstimator`.** It mirrors libqueuingsim's estimator of
the same name, which its comment cites, and
`docs/design/stochastic-model.md` refers to it. A rename moves no
meaning.

**`^` in the documented `def`.** `(s_hit + ds) ^ 2` reads better than the
product, but `^` is `powf`, and the report would no longer be the
builtin's byte for byte; the product keeps the experiment exact.

**Dated records.** `whole-tokens.md`, `stochastic-model.md` and
`ir-v4.md` name `price` among the functions of their day. They record
the code as it was when written and stay; `philosophy.md`'s list of
things to do is told that this one is done.
