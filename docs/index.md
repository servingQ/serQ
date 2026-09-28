# seQ

**A serving deployment is a program.**

seQ is a small language in which an LLM serving system — its memory pools, its
engine, the path a session takes through them — is written down once, as a
program. That one program is then simulated, checked against the real system,
and reasoned about formally.

The name follows seL4: a system is specified once, and the specification is the
thing that gets verified.

```rust
pool kv    { cap blocks * bs; block bs; evict lru; preempt lifo; }
pool slots { cap max_seqs; admit via engine; }

stage engine : step {
  budget B;
  cost c0 + max(omega + beta * (kvb + kvp), ntok * a);
  memory kv;
}

session {
  turn;
  loop {
    set prompt = K + n;
    set c = min(cachedin(kv), floor((prompt - 1) / bs) * bs);
    hold slots (1), kv (c + min(prompt - c, budget_left(engine))) {
      run engine prefill (prompt - c) growing kv;
      observe ttft = now - t0;
      run engine decode (o - 1) growing kv;
    } cache (prompt + o);
    branch (more) { run tool (~exp(Z)); turn; } else { end; }
  }
}
```

That is most of `programs/vllm.seq`, and it **is** vLLM v1's engine: on six
deterministic scenarios and on a 333-session, 3 321-request trace, it gives the
real scheduler's answer for every request — every first-token time, every
cached-token count.

## The organising idea

A serving deployment does four things to a request:

1. makes it **wait for a resource**,
2. **runs** it on a stage,
3. **frees** the resource, possibly keeping a prefix cached,
4. **sends it somewhere next**.

seQ makes the first three one scoped statement — `hold p (u) { … } cache (ℓ)`
— and generalises "resource" so that KV memory, request slots, live-session
caps and offload tiers are all the same kind of object: a **pool**. The fourth
is ordinary control flow: `branch`, `loop`, `end`.

## One program, three uses

<div class="grid cards" markdown>

- **Simulation** — `seq-lang run prog.seq` executes the program as a
  discrete-event simulation and reports time averages, per-observation
  statistics and per-turn records.

- **Specification** — vLLM v1's engine is a 50-line program that reproduces the
  upstream scheduler request for request. Change one line and you have a
  different deployment to compare against.

- **Formal verification** — the same program is an inductive type in Lean with
  an operational semantics. The memory invariant of every pool is a theorem;
  the vLLM scheduler scenarios are theorems.

</div>

## Where to start

| If you want to | Go to |
|---|---|
| build it and run something | [Getting started](getting-started.md) |
| learn the language from scratch | [Tutorial](tutorial/index.md) — six chapters, each a runnable program |
| see a real system written in it | [Case study: vLLM v1](case-study-vllm.md) |
| look something up | [Cheatsheet](reference/cheatsheet.md), [CLI](reference/cli.md) |
| know why any of this should be believed | [How seQ is checked](validation.md) |

!!! note "The reference documents"
    [The language](language.md), [The IR](ir.md) and the [design
    review](review.md) are the specification-grade documents. They are complete
    and dense. The tutorial is the way in; those are what you read afterwards.
