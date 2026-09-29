# Serving vocabulary

Names for the parts of a request's life. Each form is sugar: the parser rewrites
it to the [kernel statement](statements.md) it stands for, so the AST, the IR
and the interpreter know nothing of it.

| Form | Kernel |
|---|---|
| [`enter P (u) … { body } keep (ℓ);`](#enter) | `hold P (u) … { body } cache (ℓ);` |
| [`admit if P (u) … fit where … { body } keep (ℓ);`](#admit-if) | `hold`, in a `server` block |
| [`prefill W;`](#prefill-decode-tool) | `run prefill (W);` or `run E prefill (T);` |
| [`decode W;`](#prefill-decode-tool) | `run decode (W);` or `run E decode (T);` |
| [`tool Z;`](#prefill-decode-tool) | `run tool (Z);` |
| [`transfer (X) from P to Q (n);`](#transfer-from-to) | `run link (X); load Q (n); release P;` |

## `enter`

```seq
enter POOL (units) [reserve (r)] [, POOL (units) [reserve (r)]]*
      [reuse (ρ)] [at admission (NAME = expr, …)]
      block [keep (ℓ)] [lease POOL (t)];
```

The `hold` of a `session` block, said from the session's side: the scheduler
admits, the session enters. `keep` is `cache`. Arguments as
[`hold`](statements.md#hold).

## `admit if`

```seq
admit if POOL (units) [reserve (r)] [, POOL (units) [reserve (r)]]* fit
         [reuse (ρ)] [where NAME = expr, …]
         block [keep (ℓ)] [lease POOL (t)];
```

The same hold, written by the scheduler in a `server` block. The pools listed
are the ones that must have room (`used + r ≤ cap`), the units are what the
admission takes, and `fit` is the whole condition. The condition is not an
expression on purpose: a free predicate would let a program admit on 10 units
and take 20. Where the test differs from the allocation, `reserve` says so.
`where` is `at admission`.

Only in a `server` block; `enter` only in a `session` block.

## `prefill`, `decode`, `tool`

```seq
prefill [ '[' j ']' | on STAGE ] work [growing POOL];
decode  [ '[' j ']' | on STAGE ] work [growing POOL];
tool    [ '[' j ']' | on STAGE ] work;
```

| Argument | Type | Description |
|---|---|---|
| `[j]` | `expr` | Index into the role's stage array: `prefill[j] W;`. |
| `on STAGE` | `stage` | Names the stage explicitly: `prefill on P2 (W);`. |
| `work` | `expr` | `W`: clock time on a `fifo`, `ps` or `delay` stage. `T`: tokens on a `step` engine. |
| `growing` | `pool` | `prefill` and `decode` on a step engine only. Passes through to the `run`; a form never adds it. |

**Which stage.** Without `[j]` or `on`, the form finds its stage among those
declared above it: the stage named for the role (`prefill`, `link` or
`transfer`, `decode`, `tool`); failing that, for `prefill` and `decode`, the
`step` engine. Exactly one must qualify: with none or several the parser stops
at the form. On a step engine the run gets the role's mode; elsewhere it is
plain. `transfer` and `tool` on a step engine are link errors, so they take no
`growing`, which needs one. `transfer` finds its stage by the same rule and
always says where the KV goes ([below](#transfer-from-to)); a link that only
takes time is `run link (X);`.

## `transfer … from … to`

```seq
transfer [ '[' j ']' | on STAGE ] (X) from P to Q (n);
```

| Argument | Type | Description |
|---|---|---|
| `X` | `expr` | Time the link takes. |
| `P` | `pool` | The session's lease (or hold) the KV comes from. Given back at the end. |
| `Q` | `pool` | The session's hold the KV arrives in. |
| `n` | `expr` | Tokens counted as computed at `Q`. |

`run link (X); load Q (n); release P;`. The KV of a prefill/decode split lives
in two pools whose lifetimes overlap without nesting: the decode instance
allocates before the prefill instance frees.

### Example

From `examples/pd-disaggregation/llmd_pd.seq`, the prefiller's lease and the
decoder's read:

```seq
admit if kvD (prompt) reserve (prompt), reqsD (0) reserve (1) fit … {
  transfer (x0 + (prompt - c) / Bw) from kvP to kvD (prompt - 1 - c);
  …
} keep (prompt + o);
```
