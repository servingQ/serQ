# Design

The design record of seQ. `docs/language.md` describes the language as it
is and `docs/ir.md` the IR as it is; this directory records **why they have
the shape they have, what is pushing them to change, and what the next shape
is**.

## The IR is a living thing

The IR is never finished. It evolves with the requirements and keeps looking
for a better solution. That is a working rule, not a slogan, and it unfolds
into five items.

**1. Selection pressure comes from verification.** What asks the IR to change
is not taste but what a check caught: the fifteen defects of
`docs/review.md` §3, a path no oracle exercised, a statement Lean cannot
write, a regime where the simulator is slow. A requirement is written in
that form. "This program could produce this defect" is the requirement;
"this concept makes that class impossible" is the solution.

**2. Variation is an RFC; selection is done by the three consumers.** A
proposal is an issue (labels `rfc`, `design`), and the test it must pass is
what the IR's three consumers gain. The interpreter: a wrong answer that is
now impossible, or speed. Lean: a statement that can now be written, or an
undefined behaviour that is gone. The oracle: a path that now agrees. And a
fourth, the reader: a rule that disappears. A variation that answers none of
the four is the charm of a paradigm, not the power of a tool. The verdict
table of the [frontend sketch](frontend.md) applies that test.

**3. The price is a handshake.** One IR version moves `serving-queue-theory`'s
generator, the seven oracle IR files and the Lean fragment together. The
version policy that identifies meaning rather than shape (#21) sets that
price. The same shape with a different meaning is the most expensive change,
because an old reader is silently wrong.

**4. A decision keeps its reason.** A decision can be reversed only if its
"why" is recorded, so a rejected idea is recorded too, with its reason, in
the self-critique section of the document that rejected it. That is so the
same proposal is not argued twice.

**5. Nothing goes extinct.** Committed IR files must always be regenerable
(`make oracle-ir`). An old program must compile through the new frontend to
the same report (`tests/ir.rs`), and a change for which that is not possible
says what changed in its release note.

## How it evolves

```
what a check caught ──▶ RFC issue (Before/After, picture books)
                                │
   judged by the three consumers and the reader ◀──┘
                                │
   adopted ──▶ IR version (handshake) ──▶ docs/ir.md, docs/language.md updated
   rejected ──▶ the design document's self-critique, with the reason
```

The design criteria themselves (unambiguity, intention-revealing, policy in
the program, checkability) are in `CLAUDE.md` and #8 and are not repeated
here. This directory is the record of **applying** them.

## Documents

| Document | What | Status |
|---|---|---|
| [Philosophy](philosophy.md) | What seQ is mathematically, and the principles by which its vocabulary grows | discussion of 2026-09-28 |
| [Frontend](frontend.md) | Model / instance split, effects and handlers, the admission block, trait vocabulary, units. With two self-critiques and a verdict table | sketch, before issues |
| [IR v4](ir-v4.md) | Making the implicit static: moments, ownership, declared orders, integer ticks, the session automaton, step coalescing, progress checks | RFC #41 |
| [Subagents](subagents.md) | A session that spawns sessions is not a tool call: endogenous arrivals, hold-and-wait, cross-session cache, `Spawn`/`Join` | review, for v4b |
| [The KV transfer](pd-transfer.md) | Prefill/decode disaggregation as llm-d and the NIXL connector do it, in pull and push mode; `release`, `load`, `transfer … from … to …`; the reservation push mode still wants | IR v5, with `programs/llmd_pd.seq` |
| [Queues](queue.md) | gateway, prefill, link and decode as roles of one `queue` that owns its pools, its stage and the entries holding a request's admission, allocation and service; what an entry may read; sugar over IR v5 | RFC #72, with `programs/llmd_pd.seq` |
| [Explicit gateways](explicit-gateways.md) | Historical named-request design; superseded because the workload must not name the serving entry | PR #87 follow-up |
| [Serving composition](serving-composition.md) | `def` declares topology, instance counts and semantics; `impl` binds typed config and named `fn` policies for budget, cost and routing; gateway admission and routing through explicit ports | design with executable witnesses |

A new design document adds a row to this table.
