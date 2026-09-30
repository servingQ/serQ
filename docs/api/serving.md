# Serving vocabulary

Names for the parts of a request's life. An admission is the kernel's
[`hold … at admission (…) { … } cache (…)`](statements.md#hold); these name what
the request does once it is in.

| Form | The request… | Kernel |
|---|---|---|
| [`prefill W;`](#prefill-decode-tool) | computes its prompt's KV | `run prefill (W);` or `run E prefill (T);` |
| [`decode W;`](#prefill-decode-tool) | generates its output, a token per iteration | `run decode (W);` or `run E decode (T);` |
| [`tool Z;`](#prefill-decode-tool) | waits outside the engine (a tool call, a person reading) | `run tool (Z);` |
| [`transfer (X) from P to Q (n);`](#transfer-from-to) | has its KV moved to another instance | `run link (X); load Q (n); release P;` |

Each form is sugar: the parser rewrites it to the [kernel statement](statements.md)
in the last column, so the AST, the IR and the interpreter know nothing of it.

## `prefill`, `decode`, `tool`

```serq
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

```serq
transfer [ '[' j ']' | on STAGE [, STAGE]* ] (X) from P to Q (n);
```

| Argument | Type | Description |
|---|---|---|
| `X` | `expr` | The link's work, in its stages' unit: time at a `ps(1)`, tokens at a `ps` of tokens per second. |
| `P` | `pool` | The session's lease (or hold) the KV comes from. Given back at the end. |
| `Q` | `pool` | The session's hold the KV arrives in. |
| `n` | `expr` | Tokens counted as computed at `Q`. |

`run link (X); load Q (n); release P;`. The KV of a prefill/decode split lives
in two pools whose lifetimes overlap without nesting: the decode instance
allocates before the prefill instance frees. `on a, b` names several stages
the read holds at once, the sender's link and the receiver's
([`run`](statements.md#run)): `transfer on egress[i], ingress[j] (X) from P to Q (n);`
is `run egress[i], ingress[j] (X); load Q (n); release P;`.

### Example

From `examples/pd-disaggregation/llmd_nixl_pull.serq`, the decoder's read of
the prefiller's leased blocks, over the prefiller's NIC and its own:

```serq
hold kvD[j] (known) reserve (known), reqsD[j] (0) reserve (1) … {
  run setup (x0);
  transfer on egress[i], ingress[j] (prompt - c) from kvP[i] to kvD[j] (prompt - 1 - c);
  …
} cache (prompt + o);
```
