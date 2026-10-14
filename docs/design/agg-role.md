# The `agg` role

An engine that prefills and decodes a request itself plays `agg`; one that
decodes with the KV a prefiller leased plays `decode`. Issue #642:
parse-time names, no IR change.

## Before

`examples/multi-turn/thunderagent.sq` and
`examples/pd-disaggregation/llmd_nixl_pull.sq` at `ea630d1`:

```serq
  queue E[J] : decode {
    …
    decode (prompt) { … }
  }

  queue D[ND] : decode {
    …
    decode (prompt) { … }
    decode (prompt) from P { … }
  }
```

`decode` named two engines: ThunderAgent's replica, which prefills and
decodes on one engine, and a P/D decoder, which reads the KV a prefiller
leased. The role allowed either entry or both (#229), so the header said
nothing about which; a reader went down to the entries and looked for a
`from`.

## After

```serq
  queue E[J] : agg {
    …
    decode (prompt) { … }
  }

  queue D[ND] : agg, decode {
    …
    decode (prompt) { … }
    decode (prompt) from P { … }
  }
```

The programs' IR is byte-identical to `ea630d1`: a role is a name the
parser checks entries against, and nothing of it reaches the IR.

## The design

**A verb and a `from` name one role.** `decode (prompt)` is `agg`'s entry,
`decode (prompt) from Q` is `decode`'s. Each role has exactly one entry
now, so `RoleSig.from` is a `bool` and the "either or both" rule of #229
is gone: a pod that does either (llm-d's decode pod, which prefills a short
prompt itself) declares both roles.

**The verb stays `decode`.** The callers (`E[j].decode (prompt)`, `D[j].decode
(prompt) from P[i]`) do not change, and a call is still checked against
the entry it names.

**The entry is checked before the role.** A queue `: decode` with only
`decode (prompt)` is refused at the entry, naming the role it belongs to
(`` `decode (…)` is the `agg` role's entry; declare `queue E : agg` ``),
which is the likelier fix than adding the missing `from` entry.

## Self-critique

- *Keep `decode` for both and add `agg` as a narrower name.* Two spellings
  for an aggregated engine (`: decode` and `: agg`), against "one meaning,
  one spelling".
- *Call it `default`, or leave the aggregated engine without a role.* A
  name for what a UI picks first, not for what the engine does; and a role
  left out would make `decode (prompt)` the one entry with no role.
- *A verb of its own (`serve (prompt)`).* `serve` is a keyword, and every
  caller would change for a distinction the header already makes.
