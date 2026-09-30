# Subagents: a session that spawns sessions

A review of 2026-09-28. The question was whether subagents are accounted for
in the IR design; the answer is **no**, and they are not a variant of a tool
call but a new kind of thing.

## What the IR does not have

Only the environment creates sessions. `CArrival` is `Poisson`, `Closed`,
`Batch` or `Sessions`, and the interpreter's `spawn` is what that arrival
process calls (`interp.rs:591`). None of the eleven statements of a session
block creates a session or waits for one. The tool call of `vllm.sq` is
`stage tool : delay`: an exogenous delay, independent of load.

A subagent is a parent agent sending requests to the same model and waiting
for the answers. From the parent's side it looks like a tool call. From the
system's side it is entirely different.

| | tool | subagent |
|---|---|---|
| cause of the delay | the outside world, independent of load | **the same engine's response time**, a function of load |
| arrivals | the environment makes them | **the system makes them itself** (endogenous arrivals) |
| prompt | unrelated to the parent's context | has the parent's context as its prefix (cross-session cache sharing) |
| the parent's resources | none (only cache after `end`) | the parent may hold a slot while it waits (hold-and-wait) |
| unit of statistics | the session | **the session tree** |

## Why it is new, in four points

**1. Load is endogenous.** The arrival rate is not λ but
λ · (1 + E[children] + E[grandchildren] + …), and the number of children
depends on the parent's output, so it is a branching process. The expected
total offspring must be finite, and the capacity condition sits on top. When
response time grows the parent waits longer, and meanwhile other parents
create more children. Lecture 5's cliff closes not through tool latency but
through the system itself. As an object of queueing theory it is a fork-join
queue with branching, and the paper's models do not have it.

**2. Hold-and-wait deadlock.** If the parent holds units of a finite pool (a
live-session cap, a `reqs` slot) while waiting for children that need the
same pool, the cap deadlocks. The theorem of [IR v4](ir-v4.md) §7 argues by
induction on admission order that the earliest-admitted session makes
progress; if that session is waiting for children it does not. The theorem
needs one more hypothesis: "no join while holding a finite pool", or "units
held during a join may be preempted by children". The first is the one a
linker can check: reject a `Join` inside a `Hold`, or require the pool to be
unbounded.

**3. Cross-session cache sharing becomes mandatory.** A child's prompt starts
with the parent's context. Today the cache is per session (§9 limitation 2),
so a child always misses; vLLM is content-addressed, so it hits. The moment
subagents are modelled this limitation decides the result. That is where the
"eligibility key" of [IR v4](ir-v4.md) §2 becomes real.

**4. Policies must read the tree.** A child a parent is waiting for is worth
serving first (the parent's latency is the sum), and a deep tree is worth
cutting. Queue keys and eviction keys must be able to read tree attributes
such as `depth`, `parent`, `siblings_left`, and by criterion 2 those are
expressions in the program.

## What goes into the IR

Two kernel statements and a few session attributes.

```rust
    /// Create `count` sessions running `session` (default: the same block),
    /// each initialised by `init` in the parent's environment. The children
    /// carry `parent`, `depth`, and start ready at the current instant.
    Spawn { count: CExpr, init: BlockId, session: Option<BlockId>, into: usize /* attr: handle */ },
    /// Block until every child of `handle` has ended.
    Join { handle: usize },
```

- In the automaton ([IR v4](ir-v4.md) §5) `Join` is the third blocking point,
  and the event that wakes it is a child's `End`. In the effect/handler table
  `spawn` and `join` are effects of the workload, since the workload is what
  creates arrivals.
- The moment rule ([IR v4](ir-v4.md) §1) is unchanged: a child's `init` starts
  from a snapshot evaluated at the parent's `@session`.
- In ownership ([IR v4](ir-v4.md) §2) a child borrowing the parent's cache
  needs a content-addressed eligibility key. A first step is the rule "the
  child inherits the parent's key", since the parent's prompt is a prefix of
  the child's.
- Statistics need tree-level observations: recording the root session
  number with each `observe` lets them be combined outside.
- The `stuck` check ([IR v4](ir-v4.md) §7) must include the state where a
  joining session's children are all blocked in a queue that is not moving.

## How a program reads

```
session {
  turn;
  loop {
    enter reqs (1), kv (…) { prefill …; decode …; } keep (…);
    branch (delegate) {
      spawn (k) into kids { set n = subtask_prompt; set depth = depth + 1; }
      join kids;                     // the "tool call" whose time is the engine's own
    } else {
      branch with (p) { tool Z; } else { end; }
    }
    turn;
  }
}
```

The point of the program above is that it spawns after the `enter` scope has
closed, so it does not `join` while holding `reqs`, and the linker can check
that.

## Cost and order

`Spawn` and `Join` are two IR nodes, hence a handshake, and the Lean
fragment fixes the number of sessions (`Exec.run … n sessions`), so
accepting dynamic creation means the fragment's state becomes a list of
sessions. That belongs in the same redesign as the automaton (v4b). Three
things can be done before it. First, approximate in today's language: route
the children's load through separate `arrive poisson` sessions and set the
parent's `tool` delay to those sessions' measured response time; the
feedback is cut, so the cliff is missed. Second, establish the stability
condition of the branching fork-join as a hand-written model in
`serving-queue-theory` first. Third, put the tree structure (parent request
id) into the trace so that a replay reproduces the order; only then is there
an oracle.
