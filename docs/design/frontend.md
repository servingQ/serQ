# Frontend design: model, instance, claims

A sketch of 2026-09-28. Its purpose is to separate "expressing a serving
system semantically" from "injecting a configuration into that expression
and running it", and it is independent of [IR v4](ir-v4.md). The kernel
process language is today's IR, so `link(model, instance)` yields today's
closed IR. The two self-critiques at the end are half of this document, and
their verdicts set the order in which the pieces go to issues.

## Four decisions

**1. A program is three documents: model, instance, claims.** The model
writes structure only. Every number except a structural constant (one
request) is a `param` with a type and a unit. The instance is a valuation of
one model's parameter signature; the trace, the cost model, the seeds and
the horizon belong there. Claims split into those about the model (hold for
every instance; Lean) and those about an instance (agree with measurements
or the oracle; `make check`). The model is a term with free variables, the
instance a valuation, the meaning `⟦M⟧ : Instance(M) → Process`: ML's functor
and signature.

**2. A session is a coroutine that performs effects; the deployment is a set
of handlers.** A session does eight things: `acquire`, `grow`, `release`,
`run`, `sample`, `observe`, `turn`, `now`. Pools handle acquire/grow/release,
stages handle run, the workload handles turn, the instance handles sample
and observe. Policies are handler parameters, so they live in the program.

**3. Time of evaluation is a block, not a substitution.** A block the handler
runs once at admission. A value read outside enters the block only as a
snapshot. vLLM's "look up when the scheduler takes me" and H-pin's "look up
on arrival" are both expressible and differ visibly.

**4. A resource is a scope and an affine value.** `acquire … as h { … }` is a
scope; `h : Held<kv>` is an affine value usable only inside it; `grow` takes
`h`.

Two small ones on top. Units are types (`tokens`, `s`, `count`), block
rounding is `kv.blocks(p - 1)`. The serving vocabulary is a trait
(`impl Prefill, Decode`).

## vLLM as three documents

```
model vllm {
  param block_size : tokens;
  param blocks     : count;                 // num_gpu_blocks
  param max_seqs   : count;                 // max_num_seqs
  param budget     : tokens;                // max_num_batched_tokens
  param chunk_cap  : tokens = unbounded;    // long_prefill_token_threshold
  param step_cost  : fn(Step) -> s;         // measured on the device
  param Prompt0, Prompt, Out : dist<tokens>;
  param Think : dist<s>;
  param Continue : dist<bool>;
  param Arrival : process;

  resource kv : Pool<tokens> {
    cap     = blocks * block_size;
    grain   = block_size;
    evict   = order by (released);          // LRU, tail first per block
    on_full = preempt (latest admitted);    // vLLM running[-1]; or `wait`
    admit   = via engine, head only;        // or `first fit`
  }
  resource reqs : Pool<count> { cap = max_seqs; }

  stage engine : Step {
    budget = budget;
    chunk  = chunk_cap;
    cost   = step_cost;
    serve  = residents by (admitted);       // or by (mode == decode ? 0 : 1, admitted)
    memory = kv;
  } impl Prefill, Decode;
  stage tool : Delay impl Tool;

  workload {
    arrive Arrival;
    state K : tokens = 0;
    turn { n ~ (K == 0 ? Prompt0 : Prompt); o ~ Out; more ~ Continue; }
  }

  process session {
    turn;
    loop {
      let t0 = now;
      let prompt = K + n;
      acquire reqs (1), kv at admission {
        let hit = min(kv.cached, kv.blocks(prompt - 1));   // read when the scheduler takes me
        need  prompt;                                       // scheduler_reserve_full_isl
        take  min(prompt, hit + engine.budget_left);
        reuse hit;
      } as h {
        observe hit = kv.cached > 0;
        prefill (prompt - kv.cached) growing h;
        observe ttft = now - t0;
        decode (o - 1) growing h;
      } keep (prompt + o);
      observe response = now - t0;
      K := prompt + o;
      if more { tool ~Think; turn; } else { end; }
    }
  }
}
```

H-pin is moving the one line `let hit = …` out of the `at admission` block.
Today the same change is the deletion of `admit via`, and the intent is not
visible.

```
instance a100_short of vllm {
  block_size = 16 tokens;  blocks = 8010;  max_seqs = 64;  budget = 512 tokens;
  step_cost = |st| 13.9 ms + 41 us * st.ndec + 0.138 us * st.kvb
                 + 51.5 us * st.npre + 4.02 ns * st.attn;
  trace "short.csv" ordered binds { n, o, think, more };
  Arrival = spaced 3.0 s;
  run { horizon 2000 s; warmup 200 s; seeds { arrival 1, workload 2, session 3, evict 4 } }
}

claims vllm {
  invariant kv: allocated + cached <= cap;                    // ∀ instance; Lean
  theorem   serve_is_decode_first when chunk_cap = unbounded; // Lean
}
claims a100_short {
  oracle "tools/oracle/*.json" on { ttft_step, last_step, preemptions };
  expect ttft.mean within 15% of "data/exp/gpu_seq/3.0s.csv";
}
```

## Semantics

An LTS with two kinds of transition: instantaneous (handling an effect) and
timed (a stage advancing work). Time does not pass while an instantaneous
transition is enabled (serQ's settle; maximal progress of timed automata).
Randomness is a per-stream effect, so a program is a deterministic function
of its seeds.

| Effect | Handler | The handler's policy expressions |
|---|---|---|
| `acquire`, `grow`, `release` | Pool | `evict`, `on_full`, `admit`, `grain` |
| `run` | Stage | `serve`, `budget`, `chunk`, `cost` |
| `turn` | Workload | distribution or trace |
| `sample`, `observe`, `now` | Instance | seeds, what is recorded, the clock |

## Self-critique 1: measured by "does a new theorem appear"

- **Effects and handlers are a metaphor, not a mechanism.** "The vLLM oracle
  is another handler" was said, but `vllm_replay_oracle.py` runs `schedule()`
  whole. Admission, serving order and preemption are decided together in one
  step, so they cannot be plugged in per effect. In Lean the handler
  composition machinery is more work, not less.
- **The model/instance split gives Lean less than claimed.** With symbolic
  parameters `decide` is gone and a person proves. The two example claims
  already exist as theorems of the semantics. What is new is "a safety
  property of this program for every instance", and that can only be written
  on the automaton, like the deadlock theorem of [IR v4](ir-v4.md) §7. The real
  gain on the simulator side is that sweeps and calibration become
  first-class.
- **The `at admission` block buys readability with an IR node.** No program
  draws inside it, so there is no new theorem.
- **Drop `Held<P>`.** The scope already gives the theorem, and an affine type
  is one more typing judgment to formalise in Lean.
- **Unit types have friction.** Cost expressions mix `s/token²` coefficients
  and policy expressions take `ln` of a token count. The labels of #14 come
  first.

## Self-critique 2: measured by "do rules disappear and is expression more direct"

This is the measure that matches criterion 1, and under it most of the
pieces survive. Simplicity is measured not in program lines but in the
number of rules a reader must know.

| Piece | What disappears | What must be learned | Verdict |
|---|---|---|---|
| admission block | the `at admission (x = e)` clause, the `~` ban, one lint, the mixed viewpoints of the header clauses (#31) | one block = one moment, three verbs (`need`, `take`, `reuse`) | adopt. If `~` stays forbidden it is definable by substitution, so the IR is unchanged |
| trait vocabulary | the "Which stage" resolution paragraph, the `prefill on P2` special form | one line, `impl Prefill` | adopt |
| unit labels and `kv.blocks(e)` | the comment convention next to constants, the block-rounding expression | three unit names | adopt, checking later |
| policies as expressions | `decode first` and `exclusive prefill`, implicit HOL blocking | a `serve = by (…)` expression with named defaults | adopt ([IR v4](ir-v4.md) §3) |
| model / instance | the `let` constant block, one program copied per experiment | a signature type language, three documents | conditional. The linker must reject a number inside the model; if the old form survives there are two spellings, against criterion 0 |
| claims | looking up "what was it checked against" in the §5 table | a claims grammar | conditional |
| effects and handlers | nothing (in programs) | nothing | as the structure of the document only: spec §3 becomes one table and one paragraph on time |
| `Held<P>` | nothing | one type | remove |

The kernel does not shrink. Its eleven statements are already small and they
are what Lean reads, so they stay. What this design simplifies is the
surface syntax and the spec.

## How to measure it

Move the twelve programs to the new syntax and count three numbers: lines
and comment lines per program, paragraphs that disappear from spec §2 and
§3, and whether `vllm.sq` and `vllm_replay.sq` become two instances of one
model. The last is even odds: replay's `front` stage is a structural
difference, so it has to sit in the model with a zero cost in one instance,
and if that is awkward the split is not finished.

## Order of issues

1. The admission block (sugar, no IR change). Closes #23 and half of #31.
2. Trait vocabulary (parser, no IR change). Replaces #5's resolution rule
   with a declaration.
3. Unit labels (the unchecked half of #14).
4. Model / instance split, as an RFC that states the condition forbidding the
   old form.
5. Claims, after 4.
