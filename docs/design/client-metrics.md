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
says whether it decodes; and the interpreter commits each output token.

| Metric | Sample | Recorded |
|---|---|---|
| `ttft` | first token − start | at the first token |
| `tpot` | (last token − first token) / tokens after the first | when the turn is answered |
| `response` | answer − start | when the turn is answered |

- **start**: the session executes the turn's `Turn`.
- **answered**: the session's frame reaches the first statement after the
  `Turn` on the workload's side, or the end of the `Turn`'s block: the
  server's statements are done.
- **first token**: the turn's first `Decode` run starts, in the session or
  one of its legs. The prefill before it ends at that instant and sampled
  the token. A prefill-only engine, llm-d's prefiller with `max_tokens =
  1`, runs no decode, so the first token is the decoder's.
- **tokens after the first**: what the interpreter commits for the turn
  after its first token: a decode's token at each iteration, and a
  prefill's at its end, which after the first token is a resumed request's
  re-prefill sampling the next token (vLLM keeps the outputs of a preempted
  request and samples on, `scheduler.py:1560-1561`).

A turn that runs no decode, a station of `fifo`, `ps` or `delay` stages
(`examples/single-turn/mg1.sq`, the tutorial's first chapters, the
papers), has a `response` and no `ttft` or `tpot`; one with a single
output token has no `tpot`.

**vLLM.** The metrics are vLLM's request statistics at the pinned revision
(`vllm/v1/metrics/stats.py`): the time to first token is the time since
arrival when a request's first output comes back while it is prefilling
(`metrics/stats.py:472-474`), and that output's time is its first-token
time (`metrics/stats.py:499-505`); the end-to-end latency is the time since
arrival when it finishes (`metrics/stats.py:541`); and the mean time per
output token is the decode interval, from the first token to the last,
over the generated tokens less the first (`metrics/stats.py:552,
558-563`). Two differences: the start is the client's `turn`, where vLLM's
is the frontend's arrival; serQ has no frontend between them unless the
program writes one (`vllm_nixl_push.sq`'s `run front`). And a request with
one output token has no `tpot` sample, where vLLM records 0: 0 over 0
tokens is no time per token, and a 0 pulls the mean of the others down.

**Where it is reported.** A section of its own, `client`, beside the
program's `observe` table, in the text report, in the JSON (`client.ttft`,
…), in `--dump` (`client/ttft.csv`) and in pyserq (`report.client`). A
program's `observe ttft` keeps its name and its own table: it may measure
something else, as `routing.sq`'s `response` does.

## What changed in the programs

The IR and the run do not change: the interpreter derives the metrics from
the statements it already executes, so `IR_VERSION` stays and the oracles,
the Lean model and the draws are untouched. Programs whose hand-written
`ttft` or `response` meant exactly the client's lose them, and with them a
`set t0` or a `mark first_token` nothing else reads. Each was checked
sample by sample: before (main's `serq` and text) and after (this one's),
seeds 1 to 3 at horizons 200 and 1 000, the program's `observe` dump against
the client's.

| Program | Removed | Before against after |
|---|---|---|
| `multi-turn/vllm.sq`, `vllm_chat.sq`, `single-turn/vllm_single_turn.sq`, `subagent/vllm_subagents.sq`, `engines/sglang.sq`, `tensorrt_llm.sq`, `tgi.sq`, `multi-turn/replica.sq`, `single-turn/separate_phases.sq`, `vendors/rbln.sq` | `ttft`, `response` | the same samples, at the same times |
| `multi-turn/closed.sq`, `single-turn/fastertransformer.sq`, `mg1.sq`, `ps.sq`, `vendors/ascend.sq`, `pd-disaggregation/pd_open.sq`, the tutorial's five programs | `response` (and `ttft` in the fifth) | the same samples, at the same times |
| `pd-disaggregation/llmd_nixl_pull.sq` | `ttft` and its marks | the same value for every turn; recorded at the first token, where the gateway recorded it at the end, so a turn on the warm-up's edge counts on the other side (two turns in six runs) |
| `pd-disaggregation/vllm_nixl_push.sq` | `ttft`, `response` and its mark | `response` the same; `ttft` as llm-d's |

Every other line of every report is the same.

Kept, because they measure something else:

- `llmd_nixl_pull.sq`'s `response`: only an accepted turn's. A turn vLLM
  refuses (`too_long`) is answered at once, a client `response` of 0 (9 to
  67 turns a run).
- `pd_batching.sq`'s `ttft`, `tpot` and `response`: the same values (its
  `tpot` divides by `o - 1`, the client's count), its `ttft` recorded at the
  turn's end; kept because `tools/pd_batching/` reads them from the
  reports it archives.
- `routing.sq`'s `response`: not a session's first turn.
- `pd_tandem.sq`'s `response`: one per pass of a server loop that never
  ends its turn, a job of a closed tandem, not a turn.
- `pd_ps.sq`'s `tpot`: its stages are `ps`, with no decode run.
- `kong_svf.sq`'s `response`: its claim aggregates it (`total(response)`).
- `replay/vllm_replay.sq`'s: the prefix-cache oracle and its Lean theorem
  read them (`tests/vllm_cache.rs`, `scripts/gen_lean_oracle.py`).
- the papers' (`dai_*.sq`, `bari_rad.sq`): the Lean claims' programs,
  left with their claims.

## Self-critique

**The engine's mark (#501, step 1).** A request's `mark first_token`,
read by the server as `vllm.first_token`, says when the engine produced
the first token. It is the engine's view, and in P/D two engines serve a
request, each with its own; and it moved the reports, because the server
recorded the observation after the hold. Rejected: the first token is the
client's.

**Refusing `observe ttft`.** The names `ttft`, `tpot` and `response` could
be the language's, refused as an `observe` name (#483, #487). A program
that measures something else under the name, `routing.sq`'s `response`,
would have to rename it, and the report would have one table. Rejected:
a section of its own lets the program keep its names, and a reader sees in
the section's title whose number it is.

**The turn's end as the last token.** `tpot` could divide the time from the
first token to the turn's end. The two are the same instant in every
program here, but a server that runs a stage after its decode (a
detokenizer, a network hop) would charge that stage to every token. vLLM
divides to the last token (`metrics/stats.py:552`).

**Counting decode work.** The tokens after the first could be the work of
the turn's decode runs. A request preempted while decoding resumes with a
prefill that samples a token and a decode one shorter
(`examples/multi-turn/vllm.sq`'s `known = max(kept, computed + 1)`), so
the decode work would be one short per preemption. The committed tokens
count it, as vLLM's `num_generation_tokens` does; `pd_batching.sq`'s `tpot`,
which divides by `o - 1`, agrees sample by sample.

**The first token at the prefill's end.** The interpreter already marks a
prefill's end as a token (the inter-token gaps, `Interp::token`). In P/D a
prefiller's prefill ends first and samples a token the client never sees,
which the decoder recomputes; the turn's first decode run starts at the
end of the prefill that sampled the token the client does see, in every
program here. Choosing by the mode names that one without a rule about
which prefill counts.
