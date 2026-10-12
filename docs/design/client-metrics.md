# What the client sees

Every turn is one request from a virtual client, and the report says what
that client saw of it: its time to first token, its time per output token
and its response time, with no program text (#501). They were written by
hand in every program, each slightly differently, and a P/D program could
not write them where the engine is, because more than one engine serves
one request.

## The problem

`examples/multi-turn/vllm.sq`'s server measured both itself:

```serq
  server {
    set t0 = now;
    set prompt = K + n;
    …
    hold … {
      …
      run vllm prefill (cost(vllm, known - c)) growing kv;
      branch (known == prompt) { observe ttft = now - t0; }   // not a resumed request's re-prefill
      run vllm decode (cost(vllm, o - 1 - (known - prompt))) growing kv;
    } cache (cost(reqs, kv, prompt + o));
    observe response = now - t0;
  }
```

and `examples/pd-disaggregation/llmd_nixl_pull.sq` the same through a mark
on two entries of the decoder:

```serq
      observe ttft = D.first_token - t0;
      …
        branch (known == prompt) { mark first_token; }   // not a resumed request's re-prefill
```

The first token is "the end of the first attempt's prefill" in one, a
`branch` on `known == prompt` in another, a `sent` flag in the tutorial's
fifth chapter, a mark the gateway reads in the P/D programs. #501's step 1
would have moved the vLLM programs' to a mark too, and that moved the
`ttft` line of their reports (`engine-request.md`, *How a step is
checked*). What the client sees is the same thing in every program; it is
not the engine's to say, nor the program's to rewrite.

## The definitions

The IR has what they need: a turn is the `Turn` statement and the server
statements spliced after it, which `sides` marks `Server`; a run's mode
says whether it decodes; and the interpreter commits each output token, a
decode's at every iteration and a prefill's at its end.

| Metric | Sample | Recorded |
|---|---|---|
| `ttft` | first token − start | when the first token is known (below) |
| `tpot` | (answer − first token) / tokens after the first | when the turn is answered |
| `response` | answer − start | when the turn is answered |

- **start**: the session executes the turn's `Turn`.
- **answered**: the session's frame reaches the first statement after the
  `Turn` on the workload's side, or the end of the `Turn`'s block: the
  server's statements are done.
- **first token**: the token the prefill sampled that ended last before
  the turn's first decode token, or that decode token when no prefill
  ended before it; when the turn commits no decode token, the token its
  last prefill sampled (a `max_tokens = 1` request). Tokens of the session
  and of its legs count, a leg's only while the session is on the turn the
  leg was forked in. A P/D prefiller's prefill ends before the decoder's,
  so the decoder's is the first, as llm-d's sidecar (`max_tokens = 1`)
  makes it.
- **recorded**: when a decode run starts at the instant a prefill of the
  turn ended, the usual case, the first token is known then; otherwise at
  the first decode token, or when the turn is answered for one with no
  decode token.
- **tokens after the first**: the tokens the interpreter commits for the
  turn after its first: a decode's at each iteration, and a prefill's at
  its end, which after the first token is a resumed request's re-prefill
  sampling the next token (vLLM keeps the outputs of a preempted request
  and samples on, `scheduler.py:1560-1561`).

A turn that commits no token, a station of `fifo`, `ps` or `delay` stages
(`examples/single-turn/mg1.sq`, the tutorial's first chapters, the
papers), has a `response` and no `ttft` or `tpot`; one with a single
output token has a `ttft` and no `tpot`.

**vLLM.** The engine's request statistics at the pinned revision: the
time to first token is the time since arrival when a request's first
output comes back while it is prefilling (`metrics/stats.py:472-474`),
once (`output_processor.py:692-702`), so a request of one output token has
one; that output's time is its first-token time
(`metrics/stats.py:499-505`); the end-to-end latency is the time since
arrival when it finishes (`metrics/stats.py:541`). The client is vLLM's
benchmark (`benchmarks/serve.py`), which these follow where the two
differ: its time per output token is the latency less the time to first
token over the output tokens less one, and a request of one output token
has none (`benchmarks/serve.py:625-628`), where the engine's divides the
first-to-last-token interval and records 0 (`metrics/stats.py:552,
558-563`). The answer and the last token are the same instant in every
program here; a server that runs a stage after its decode charges it to
`tpot`, as a client would see it. The start is the client's `turn`, where
the engine's is the frontend's arrival; serQ has no frontend between them
unless the program writes one (`vllm_nixl_push.sq`'s `run front`). The
benchmark counts only the requests that succeed
(`benchmarks/serve.py:604-605, 633`), and so does the client here (*A
refused turn*, below).

**Where it is reported.** A section of its own, `client`, beside the
program's `observe` table, in the text report, in the JSON (`client.ttft`,
…), in `--dump` (`client/ttft.csv`) and in pyserq (`report.client`). Its
numbers are printed to four significant digits of the mean, since a time
per token is a fraction of a millisecond. A run in which no turn was
answered after the warm-up says so in a note. A program's `observe ttft`
keeps its name and its own table, but a program that measures something
else names it otherwise (`routing.sq`'s `later_response`).

**The warm-up's edges.** A sample counts if it is recorded after the
warm-up, so a turn's `ttft` (recorded at its first token) and its
`response` (at its answer) can fall on two sides of it: a turn whose
first token comes before the warm-up and whose answer after has a
`response` and no `ttft`. At the horizon, a turn with its first token and
no answer has a `ttft` and no `response`. Counts of the three differ by
these turns.

## What changed in the programs

The IR and the run do not change: the interpreter derives the metrics from
the statements it already executes, so `IR_VERSION` stays and the oracles,
the Lean model and the draws are untouched. Programs whose hand-written
`ttft` or `response` meant exactly the client's lose them, and with them a
`set t0` or a `mark first_token` nothing else reads. Each was checked
sample by sample: before (main's `serq` and text) and after (this one's),
seeds 1 to 3 at horizons 60 (no warm-up), 200 and 1 000, the program's
`observe` dump against the client's, and every other line of the report.

| Program | Removed | Before against after |
|---|---|---|
| `multi-turn/vllm.sq`, `vllm_chat.sq`, `single-turn/vllm_single_turn.sq`, `subagent/vllm_subagents.sq`, `engines/sglang.sq`, `tensorrt_llm.sq`, `tgi.sq`, `multi-turn/replica.sq`, `vendors/rbln.sq` | `ttft`, `response` | the same samples, at the same times |
| `multi-turn/closed.sq`, `single-turn/fastertransformer.sq`, `mg1.sq`, `ps.sq`, `vendors/ascend.sq`, `pd-disaggregation/pd_open.sq`, the tutorial's five programs | `response` (and `ttft` in the fifth) | the same samples, at the same times |
| `pd-disaggregation/llmd_nixl_pull.sq` | `ttft` and its marks, `response` (the refusal now `end;`) | `response` the same samples, at the same times; `ttft` the same value for every turn, recorded at the first token, where the gateway recorded it at the end, so a turn at either edge counts on the other side (at the warm-up's start, or one with its first token and no answer at the horizon) |
| `pd-disaggregation/vllm_nixl_push.sq` | `ttft`, `response` and its mark | `response` the same; `ttft` as llm-d's |

Kept, because they measure something else, renamed where the name was the
client's:

- `separate_phases.sq`'s, now `gated_ttft` and `gated_response`: from the
  end of its `gate`, B's arrival at the engine, where the client's start
  at the `turn`. And B is prefill-only, a token the client counts at the
  prefill's end: for A and B, client `ttft` [1, 2] and `response` [4, 2]
  against the program's [1, 1] and [4, 1].
- `routing.sq`'s, now `later_response`: not a session's first turn.
- `pd_tandem.sq`'s, now `cycle`: one per pass of a server loop that never
  ends its turn, a job of a closed tandem; the client answers no turn and
  the report says so.
- `pd_batching.sq`'s `ttft`, `tpot` and `response`: the same values (its
  `tpot` divides by `o - 1`, the client's count), its `ttft` recorded at
  the turn's end; kept because `tools/pd_batching/` reads them from the
  reports it archives.
- `pd_ps.sq`'s `tpot`: its stages are `ps`, with no decode run.
- `kong_svf.sq`'s `response`: its claim aggregates it (`total(response)`).
- `replay/vllm_replay.sq`'s: the prefix-cache oracle and its Lean theorem
  read them (`tests/vllm_cache.rs`, `scripts/gen_lean_oracle.py`).
- the papers' (`dai_*.sq`, `bari_rad.sq`): the Lean claims' programs,
  left with their claims.

## A refused turn

vLLM refuses a prompt of `max_model_len` tokens or more before it
schedules it (`input_processor.py:512-536`), and its benchmark client
does not count the failed request. A server says the same with `end;`:
the request is refused and the session ends, and a turn whose session
ends among its server statements is not answered, so the client records
nothing of it. `llmd_nixl_pull.sq` writes it as its comment says:

```serq
  server {
    observe too_long = K + n >= max_model_len;
    branch (K + n >= max_model_len) { end; }
    gw.route();
  }
```

A server's `end` is refused inside a `hold` (the request would end
holding what it was admitted with, before its release and `cache`),
inside a `fork` (a leg runs beside the request and is not the session),
and in a queue's entry (a station does not decide whether the session
goes on). The IR had `End` already; its validation stops refusing it on
the server's side, a weaker check of an untagged IR version (13), recorded
for the coming tag.

## Self-critique

**The engine's mark (#501, step 1).** A request's `mark first_token`,
read by the server as `vllm.first_token`, says when the engine produced
the first token. It is the engine's view, and in P/D two engines serve a
request, each with its own; and it moved the reports, because the server
recorded the observation after the hold. Rejected: the first token is the
client's.

**Refusing `observe ttft`.** The names `ttft`, `tpot` and `response` could
be the language's, refused as an `observe` name (#483, #487). A program
that measures something else under the name would have to rename it,
which a reader of the report would want anyway, and one that measures the
same would have to delete it. Rejected for a section of its own: a reader
sees in the section's title whose number it is; the programs here that
measured something else are renamed.

**A refused turn answered at once (c).** The server's `branch (accepted)
{ gw.route(); }` answered a refused turn with nothing run, a `response` of
0 the client counted (9 to 67 turns a run at the defaults), which no
vLLM client records. Rejected.

**A turn that runs nothing is not answered (b).** It would read the
refusal from the absence of work, a rule the language would hold and the
program not say (criterion 2): a server that answers at once on purpose,
a cache hit served by the router, would lose its turns. Rejected for
`end;`, which the program writes.

**The first token at the start of the first decode run.** It is the
prefill's end in a turn that prefills, and it is wrong where no prefill
ended then: a decode alone (`kong_svf.sq`) would have its first token
before any iteration, and its decode tokens all after it. The committed
tokens say it in every case.

**`tpot` to the last token.** vLLM's engine divides the first-to-last
token interval (`metrics/stats.py:552`); its benchmark client divides the
latency less the time to first token (`benchmarks/serve.py:626-627`). The
two agree when the answer is the last token, as in every program here;
the client's is what a turn's client sees, so the report follows it.

**Counting decode work.** The tokens after the first could be the work of
the turn's decode runs. A request preempted while decoding resumes with a
prefill that samples a token and a decode one shorter
(`examples/multi-turn/vllm.sq`'s `known = max(kept, computed + 1)`), so
the decode work would be one short per preemption. The committed tokens
count it, as vLLM's `num_generation_tokens` does; `pd_batching.sq`'s
`tpot`, which divides by `o - 1`, agrees sample by sample.
