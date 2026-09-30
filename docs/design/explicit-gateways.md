# Explicit gateways

The workload must say where it sends a request. A `gateway` role describes
a queue's entry; declaring that role must not register a default server.

## Before and After

Before, copied from `programs/llmd_pd.seq` at `d0c1d70`:

```serq
    loop {
      request;
      set K = prompt + o;
      branch (more) { tool (~exp(Z)); turn; } else { end; }
    }
```

The parser registered the sole gateway's `route` as the anonymous server.
Nothing at the request site named `gw`; declaring another gateway failed.

After, from `examples/pd-disaggregation/llmd_nixl_pull.sq` (the same program, renamed since):

```serq
    loop {
      request gw;
      set K = prompt + o;
      branch (more) { tool (~exp(Z)); turn; } else { end; }
    }
```

The parser resolves `gw` after all declarations, checks that it is a queue
with the `gateway` role, and expands its `route` at that site. Gateway
declarations do not register a server. Several gateways can coexist, each
a single queue; the workload selects one at each request. A missing name
or a queue with another role is an error at the request target.

Bare `request;` still selects an explicit `server { ... }` block. It never
falls back to a gateway, even if only one exists. Named requests and a
separate anonymous server can coexist with distinct targets. Both request
forms remain restricted to the workload's session.

No IR node, version or execution rule changes. Requests expand into the
same kernel statements. `tests/queues.rs` compares the resulting IR with
flat programs, including nested requests, forward declarations, renamed
gateways, unused gateways and coexistence with an anonymous server.
`make check` runs `llmd_nixl_pull.sq`'s checks and its drawing goldens.

## Predefined vocabulary and imports

The current boundary is:

| Category | Examples | Defined by |
|---|---|---|
| Language syntax and mechanisms | `queue`, `pool`, `serve`, `request`, `mark`, `hold`, `run` | Parser and kernel semantics |
| Predefined roles and entry signatures | `gateway` / `route`, `prefill`, `decode`, `link` / `transfer` | `ROLES` in `src/queue.rs` |
| Built-in expressions and context | `min`, `now`, `cached`, `budget_left` | Expression semantics and evaluation context |
| Program declarations | `gw`, `P`, `D`, `nic`, `prompt`, `NP` | The program |

Imports would make the origin of reusable definitions visible. Roles are
a candidate for a standard serving library; a reusable vLLM engine is a
candidate for a model library once parameterised queue definitions exist.
Core syntax remains syntax. Context values such as `now` belong to the
moment in which an expression is evaluated, rather than a file import.

A possible future spelling is shown below as a design sketch, **not
accepted syntax**:

```text
import { gateway, prefill, decode, link } from "std/serving";
```

An import should resolve actual exported definitions. This requires a
role-definition mechanism, module namespaces, duplicate-name diagnostics
and a way to express the gateway entry's access to session attributes.
Reusable model imports also need parameters and instance ownership. The
current parser hardcodes these role properties; an import line alone
would only enable hidden parser behaviour and would not establish that
boundary. This change documents the boundary without introducing a
partial module system.

## Self-critique

Bare `request;` remains a shorthand for the existing anonymous server,
so explicit naming is complete for gateways rather than for all deployment
forms. Migrating `server` programs needs its own compatibility decision.

`request gw;` and an entry call both expand a body, but the request syntax
also checks the session-side gateway contract. The explicit target removes
default registration without adding a runtime dispatch mechanism.

The import direction does not yet make roles extensible. It records the
remaining hardcoded boundary and the mechanisms needed to move it into a
library; it is not a claim that modules have been implemented.
