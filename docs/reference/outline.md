# The outline of a text

Where each declaration of a program's text is written: for an editor that
changes the text in place, as [serq-lar](https://github.com/servingQ/serq-lar)
does from the figure. It finds the place here and leaves the meaning to
the compiler: the outline says where `queue D` is written, not what `D`
does.

In the API it is `serq::outline(&src)`, a `serq::frontend::outline::Outline`,
and `Outline::to_json()`. It parses and does not link, so a program with a
link error has an outline; a parse error is the diagnostic `serq check`
prints. Nothing in it is part of the program: it adds no syntax and no IR,
and the parser records it only when asked.

## Shape

Every range is a half-open range of bytes of the text, `[start, end]`
(UTF-8 bytes, not columns). A declaration's range runs from its first
word to its `;` or `}`; a `body` is what is between its braces.

| Field | Each item | Range |
|---|---|---|
| `main` | `{range, body}` | `fn main() { … }` |
| `uses` | `{path, range}` | `use "std/kvt";` |
| `decls` | `{kind, name, range}`, `kind` one of `let`, `stage`, `engine`, `pool`, `device` | a declaration outside a queue, in order |
| `queues` | `{name, roles, family, range, body, members}` | `queue P[NP] : prefill { … }`; `family` is the text between the brackets, or null |
| `queues[].members` | `{kind, name?, from?, range, body?}`, `kind` one of `device`, `engine`, `pool`, `nic`, `serve`, `entry` | in order; a device's or pool's `name`, an entry's verb as `name` and its `from Q` |
| `relations` | `{mode, source, reader, range, function}` | `kvt.nixl_pull (P, D, …);`; `mode` is `pull` or `push`, `function` the range of `nixl_pull` |
| `routes` | `{gateway, range, body}` | a gateway's `route { … }` |
| `server` | `{range, body}` | `server { … }` |
| `workload` | a range | `workload { … }` |
| `calls` | `{queue, index, verb, from, range, block, owner?}` | `D[j].decode (prompt) from P[i];` to its `;`; `index` the text between the brackets; `block` is `server`, `route` or `entry`, `owner` the queue whose route or entry it is in |

| `chooses` | `{var, count, keys, by, range, block, owner?}` | `choose i in NP by (work(P[i]), ~uniform(0, 1));` to its `;`; `count` the range of the count after `in`, `keys` the range of each key in order, `by` the key list with its parentheses; `block` and `owner` as a call's |

| `workload_items` | `{kind, name?, block, value, range}` | a statement of the workload, to its `;`, in order: `kind` its first word (`arrive`, `send`, `set`, `tool`, `turn`, …), `name` a `set`'s, `block` where it is (`workload` at its top, or `init`, `turn`, `session`), `value` what it says or null: an `arrive`'s process (`poisson(Lambda)`), a `set`'s right-hand side, what a `tool` or `send` takes between its parentheses. A statement holding a block (`while`, `if`) is not an item; the statements in it are |

A call, a `choose` or a workload statement a `def` expands to is written
in the `def`, and is not among the `calls`, the `chooses` or the
`workload_items`.

From `examples/pd-disaggregation/llmd_nixl_pull.sq`, a few items:

```json
{
  "relations": [{"mode": "pull", "source": "P", "reader": "D",
                 "range": [4275, 4332], "function": [4279, 4288]}],
  "calls": [{"queue": "P", "index": "i", "verb": "prefill", "from": null,
             "range": [1770, 1792], "block": "route", "owner": "gw"}, …]
}
```

`&src[4275..4332]` is `kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);`.
An editor that turns the pull into a push replaces `function`; one that
sends requests to another decoder writes a call after the last of
`calls`; one that changes how the router picks a decoder replaces a
`choose`'s `by`, or one of its `keys`; one that changes how often
sessions arrive replaces the `arrive` item's `value`.
