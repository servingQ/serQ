# Case study: prefill/decode disaggregation over NIXL

`examples/pd-disaggregation/llmd_nixl_pull.sq` is llm-d's prefill/decode split on vLLM: the router
that decides which pod prefills and which decodes, the sidecar that sends
the prompt to one and the decode request to the other, and the two
schedulers that hand the KV over with the NIXL connector. Every line is
checked against the source: llm-d at `8a2f37d`, its router
(`llm-d-inference-scheduler`) at `13eebdb`, vLLM at `0c87a197` (`ref/vllm`;
the vLLM citations are hashed by `make check`, the llm-d ones are not).

It is a specification checked against the code, not yet against a
scheduler oracle. What the program cannot say without two new statements,
and what it still cannot say, is in [The KV transfer](design/pd-transfer.md).

![llm-d prefill/decode over NIXL as a queueing network](assets/llmd_nixl_pull.deployment.svg)

The prompt's KV is in the prefiller's pool through the transfer and in the
decoder's from the transfer on, which is why the two enclosures cross at the
read, and the read holds the prefiller's NIC and the decoder's at once, which
is the bracket around `egress[i]` and `ingress[j]`.

## The deployment

llm-d's P/D guide (`guides/pd-disaggregation`, llm-d `8a2f37d`) deploys
this:

| Piece | Configuration | Where |
|---|---|---|
| model servers | 8 prefill pods (`vllm serve`, TP 1) and 2 decode pods (TP 4), gpt-oss-120b, `--block-size 128`, `--kv-transfer-config '{"kv_connector":"NixlConnector","kv_role":"kv_producer" / "kv_consumer","kv_connector_extra_config":{"backends":["UCX"]}}'`, `VLLM_NIXL_SIDE_CHANNEL_HOST` the pod IP | `modelserver/gpu/vllm/base/patch-prefill.yaml`, `patch-decode.yaml` |
| router (EPP) | `disagg-profile-handler` with `always-disagg-pd-decider` (every request is prefilled remotely); decode profile `decode-filter` → `active-request-scorer` → `max-score-picker` (the least busy decoder); prefill profile `prefill-filter` → `prefix-cache-affinity-filter` (`approx-prefix-cache-producer`, a cache-warm prefiller first) → `token-load-scorer` → `max-score-picker`; one EPP replica | `router/pd-disaggregation.values.yaml` |
| sidecar | `llm-d-routing-sidecar` on each decode pod, connector `nixlv2`: prefill leg (`max_tokens = 1`, `do_remote_decode`), wait, decode leg with the prefiller's block ids; serial | `pkg/sidecar/proxy/connector_nixlv2.go` (router `13eebdb`) |
| alternative decider | `prefix-based-pd-decider` with `nonCachedTokens`, `promptTokens`: remote prefill only when the uncached suffix on the chosen decoder is long enough | `docs/disaggregation.md`, `profilehandler/disagg/README.md` |

The examples below run on one prefiller or two, one decoder or two, with Qwen3-8B
and block 16. What differs from the guide is written next to each number.

## In serQ

```serq title="examples/pd-disaggregation/llmd_nixl_pull.sq"
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq"
```

### Line by line

#### The router and the sidecar, against llm-d

| llm-d | serQ | Where |
|---|---|---|
| the endpoint picker runs the decode profile first and picks a decode pod; the guide's decode profile scores by active requests (the least busy) | `choose j in ND by (holders(D[j].kv) + queued(D[j].kv))` | `disagg_profile_handler.go:316-334`; `guides/pd-disaggregation/router/pd-disaggregation.values.yaml` |
| the decider: a remote prefill when the prompt's uncached suffix on the chosen decode pod is at least `nonCachedTokens`, and the prompt at least `promptTokens`; the cached part is the router's own estimate of the pod's prefix cache | `set hitD = min(cachedin(D[j].kv), reusable(prompt, bs)); set remote = prompt >= minp && prompt - hitD >= thr;` | `prefix_based_pd_decider.go:266-303`; `disagg_profile_handler.go:353-366`. The guide runs `always-disagg-pd-decider`, which is `thr = 1` |
| the prefill profile: pods that have the prefix (`prefix-cache-affinity-filter`), then the least loaded (`token-load-scorer`) | `choose i in NP by (cachedin(P[i].kv) > 0 ? 0 : 1, work(P[i]) + queued(P[i].reqs))` | the same values file |
| the decode pod's sidecar sends the prompt to the prefiller with `max_tokens = 1` and `do_remote_decode`, waits for the answer, then sends the decode request to its own engine with the prefiller's block ids | `P[i].prefill (prompt);` then `D[j].decode (prompt) from P[i];` — the prefiller's entry, its blocks leased at its end, then the decoder's | `connector_nixlv2.go:69-232` (the prefill leg, `CapSingleToken` at 145), `261-379` (the decode leg) |
| no prefill header: the request goes to the decode pod's engine as it is | the `else` branch, `D[j].decode (prompt);`: vLLM's engine on one device (`examples/multi-turn/vllm.sq`) | `dispatch.go:196-213` |
| the two legs in parallel, so the decoder allocates while the prefiller works | not written: a session waits at one pool at a time ([The KV transfer](design/pd-transfer.md)) | `connector_nixlv2.go:60-67` (MoRI-IO write mode only); vLLM's push-mode proxy, `disagg_proxy_pushconnector_demo.py:227-270` |

#### The two schedulers, against vLLM

| vLLM | serQ | Where |
|---|---|---|
| a prompt of `max_model_len` tokens or more is refused before it is scheduled; a generation stops at `max_model_len` tokens; no KV cache smaller than one request of `max_model_len` is started | `branch (K + n >= max_model_len) { end; }` before `request gw;`; `set o = min(o, max_model_len - prompt)` in the route; `max_model_len = 16384` below every pool | `input_processor.py:512-536`; `sched/utils.py:114-120`; `kv_cache_utils.py:864-900`, called at `kv_cache_utils.py:2742` |
| the prefiller admits like any vLLM engine: a slot, the blocks of the first chunk, room for the whole prompt, the prefix hit looked up when the scheduler takes the request | `hold reqs (1), kv (min(prompt, hit + budget_left(P))) reserve (prompt) at admission (hit = …)` in `P`'s `prefill` entry | the waiting loop, `scheduler.py:868-1128`; [the vLLM case study](case-study-vllm.md) |
| the prefiller computes the prompt in chunks and samples one token, which the sidecar discards | `prefill (prompt - c) growing kv` | `scheduler.py:624-823`; the truncation for Mamba and MTP only, `nixl/base_scheduler.py:409-436` |
| the request finishes on the prefiller: its slot is freed, its blocks are not — `request_finished` returns `delay_free_blocks` and a lease of `kv_lease_duration` (30 s), renewed by the decoder's heartbeats while the request waits | `} cache (prompt) lease kv (inf);` — the scope ends, the slot goes, the blocks stay the session's | `nixl/pull_scheduler.py:191-292`; `_free_request`, `scheduler.py:2628-2657`; the renewal, `nixl/base_scheduler.py:199-238`, `nixl/base_worker.py:3010-3030` |
| the prefiller keeps every computed full block of the prompt cached once the lease ends | `cache (prompt)` on that hold, applied when the lease ends | `_connector_finished`, `scheduler.py:2929-2982`; `kv_cache_manager.py:610-619` |
| the decoder's scheduler looks at its waiting queue only at a step with budget left and a running slot free | `admit via D` on both of the decoder's pools; `reqs (0) reserve (1)` — a slot must be free, none is taken | `scheduler.py:872-879` |
| the decoder's local prefix hit, then the connector: for a remote prefill every prompt token beyond the local hit is external and loaded asynchronously | `kv (known) reserve (known)` with `reuse (reusable(known, bs))` in `D`'s `decode … from` entry; `c = cached` is the local hit | `scheduler.py:932-954`; `nixl/pull_scheduler.py:34-66` |
| blocks are allocated for the whole prompt, and the request is parked, `WAITING_FOR_REMOTE_KVS`, holding them and no slot; one transfer per request | the hold on `kv`; `transferred` is `do_remote_prefill`, spent | `scheduler.py:1199-1226, 1264-1294`; `nixl/pull_scheduler.py:108-189` |
| the worker reads the blocks from the prefiller over NIXL (pull mode: a NIXL READ issued by the decoder) | `run setup (x0); transfer on egress[src], ingress[self] (prompt - c) from src to kv (prompt - 1 - c)`: a fixed wait, then the bytes out of the prefiller's NIC and into the decoder's at once, the two shared max-min fairly (`share maxmin`, a model, not a measurement: [Bandwidth sharing](design/bandwidth-sharing.md)) | `nixl/pull_scheduler.py:168-177`; the worker's `_read_blocks`, `nixl/pull_worker.py:392-575` |
| the read done, the blocks are cached, the last prompt token is marked uncomputed (its logits are needed), and the request is back in the waiting queue, served before new arrivals | the `load D.kv[j] (prompt - 1 - c)` the transfer stands for; `hold reqs (1)`, with `reqs` declared before `kv` | `_update_waiting_for_remote_kv`, `scheduler.py:3032-3077`; `_try_promote_blocked_waiting_request`, `scheduler.py:3079-3092`; `scheduler.py:2383-2385` |
| the prefiller frees the leased blocks when the read completes | the `release P.kv[i]` the transfer stands for takes the lease | `_update_from_kv_xfer_finished`, `scheduler.py:3113-3138` |
| the decoder recomputes the last prompt token and decodes; a request preempted afterwards is rescheduled without a second transfer, prefilling locally what it lost | `prefill (known - c) growing kv; decode (o - 1 - (known - prompt)) growing kv;` with `known` from `computed` | `scheduler.py:1560-1561`; `nixl/pull_scheduler.py:187-189` |
| a parked request is in no `running` list and is not preempted; nor is the prefiller's finished one | `preempt lifo` takes the last admitted *resident* of the engine | `scheduler.py:742-813` |
| the decoder keeps the prompt's and the output's full blocks cached | `} cache (prompt + o)` | `kv_cache_manager.py:602-606` |

### Writing xPyD

The deployment is queues: `NP` prefill instances and `ND` decode instances,
each with its own KV pool, request-slot pool, engine and NIC, and one
gateway selected by `request gw;` in the workload's session.

```
queue gw : gateway { route { … } }                 // the router and the sidecar
queue egress[NP] : link { serve ps(BwP); }         // a prefiller's NIC
queue ingress[ND] : link { serve ps(BwD); }        // a decoder's NIC
queue P[NP] : prefill { pool reqs { … admit via P; } pool kv { … } serve step { … memory kv; } prefill (prompt) { … } }
queue D[ND] : decode  { pool reqs { … admit via D; } pool kv { … admit via D; } serve step { … memory kv; } decode (prompt) { … } decode (prompt) from src { … } }
stage setup : delay;
stage tool : delay;
```

A queue's pools are its members': `P[i].kv` is the KV of the i-th
prefiller, `admit via P` inside `P` means the member's own scheduler serves
the pool, and `memory kv` on its stage counts the member's own blocks as its
residents' memory (`kv_decode`, `kv_prefill`). The family's size is the
`let` the router chooses over (`queue D[ND]`, `choose j in ND`), so the
number is written once. A link with only a `serve` is a NIC whose cost is
its bandwidth; the links are declared above the pods, whose entries name
them.

The router is the gateway's two `choose`s, and the instance it picked is
the queue the request is handed to: `P[i].prefill (prompt)`, then
`D[j].decode (prompt) from P[i]`. Inside an entry nothing is indexed — `kv`,
`reqs`, `P` are the member's own — and `self` is the member's index where
another family has to be matched (`ingress[self]`). `from P[i]` is the pool
`P`'s entry leases; read as an index, the `from` name is the source member's
(`egress[src]`, the prefiller's NIC). The transfer's `load` and `release`
name the pool as the holds did, index included. The admission, the
allocation and the two runs are the entries' and appear once each: the
router's body has no `hold`.

Going from 2P2D to 4P8D is the two `let`s; the program does not change
otherwise, which is the point of writing the router as `choose` over a
family rather than as a branch per instance (`examples/multi-turn/routing.sq`
still has the branch-per-policy shape the design notes call a smell).

### The two modes

**Pull** (`NixlConnector`, `kv_role` producer and consumer): the decoder
reads. The prefiller must be done before the decoder can be told where the
blocks are, so the sidecar's dispatch is serial: prefill leg, answer, decode
leg. The program above is this mode, on the llm-d guide's deployment
(`guides/pd-disaggregation/modelserver/gpu/vllm/base/patch-decode.yaml`).

**Push** (`NixlPushConnector`): the decoder allocates and registers its
block ids with the prefiller over a NIXL notification
(`nixl/push_scheduler.py:128-205`); the prefiller writes the KV when it has
both a finished request and its registration (`nixl/push_scheduler.py:207-294`)
and frees the lease when the write completes (`nixl/push_scheduler.py:342-357`).
The decoder needs nothing from the prefiller's answer, so the two legs can
be sent at once, and vLLM's push-mode proxy does
(`disagg_proxy_pushconnector_demo.py:227-270`): the decoder allocates *during*
the prefill and the write starts the moment it ends. Through the llm-d
sidecar the dispatch is serial for NIXL (parallel for MoRI-IO only,
`connector_nixlv2.go:60-67`), and then the two modes have the same
lifecycle: the same holds in the same order, the copy moved by the
prefiller's worker instead of the decoder's, one notification more.

#### The two modes in the program

Pull is the program as written. The prefiller's scope ends in a lease, the
decoder is admitted, and the decoder's worker reads over both NICs:

```
P[i].prefill (prompt);              // hold reqs (1), kv (…) … { prefill (…) growing kv; } cache (prompt) lease kv (inf);
D[j].decode (prompt) from P[i];     // hold kv (known) reserve (known), reqs (0) reserve (1) … {
                                    //   run setup (x0);
                                    //   transfer on egress[src], ingress[self] (prompt - c) from src to kv (prompt - 1 - c);   // D READs; the lease ends
                                    //   hold reqs (1) { … }
                                    // } cache (prompt + o);
```

Push through the llm-d sidecar (serial dispatch) is the same three lines
with one difference a reader can see: the copy is the prefiller's WRITE
(`nixl/push_worker.py:714-722`), which crosses the same two NICs, and it
starts one notification after the decoder's admission (the registration,
`nixl/push_scheduler.py:128-205`):

```
decode (prompt) from src {             // D's entry
  hold kv (known) reserve (known), reqs (0) reserve (1) … {
    run setup (x0 + x_reg);
    transfer on egress[src], ingress[self] (prompt - c) from src to kv (prompt - 1 - c);
    hold reqs (1) { … }
  } cache (prompt + o);
}
```

Which NIC saturates is not the mode's: it is the ratio of prefillers to
decoders, where the router sends the reads (the prefill profile prefers the
pod that has the prefix, so one prefiller's egress carries most of them),
and the sharing policy.

Push with the two legs dispatched at once (vLLM's own push proxy) is the
form the language cannot yet write: the decoder's admission would have to
be requested when the request arrives, while the session is still queued
at the prefiller, so the write can start the moment the prefill ends. A
session waits at one pool at a time, so the program above writes the
decoder's admission after the prefill. The difference is bounded: under
decoder memory pressure both forms lease at the prefiller, since the write
cannot start before the decoder has allocated; without it, the concurrent
form takes the decoder's blocks a prefill earlier and saves one decoder
step of latency. The reservation that would write it exactly is sketched in
[The KV transfer](design/pd-transfer.md):

```
book kvD[j] (prompt) reserve (prompt);                      // join the decoder's queue now, not written yet
hold reqsP[i] (1), kvP[i] (…) … { … } cache (prompt) lease kvP[i] (inf);
hold kvD[j] { transfer on egress[i], ingress[j] (…) from kvP[i] to kvD[j] (…); … }   // open the booking, waiting if it is not granted
```

**Not modelled**: the lease's expiry and the decoder's heartbeats (the
lease is granted at `nixl/pull_scheduler.py:248-269`, reaped at
`nixl/base_worker.py:2982-3008`, extended by heartbeats the decoder tracks
from `nixl/base_scheduler.py:199-238`, sends at `nixl/base_worker.py:3141-3170`
and the prefiller applies at `nixl/base_worker.py:3010-3030`; they matter
only when a decoder dies);
bidirectional transfer for multi-turn (`bidirectional_kv_xfer`, the
decoder's blocks read back by the prefiller); the host buffer on
accelerators NIXL cannot read directly; tensor-parallel fan-out of the
read; cross-session prefix sharing on either side (per session here, as in
`vllm.sq`); the router's approximate prefix cache as a data structure of
its own (its estimate is taken to be the pod's cache).

## What the program predicts

Two decode pods of 160 000 tokens each, two prefill pods, sessions of
one to several turns (a prompt of 1 000–3 000 new tokens on a growing
context, 200 output tokens, a 3 s tool call between turns, `p = 0.9`), a
`max_model_len` of 16 384 tokens, the A100-shaped step cost of
`examples/multi-turn/vllm.sq`, a 200 000 token/s NIC on every pod. Every number
below is the median over seeds 1–20 of a run of 10 000 s after 1 000 s
of warm-up (`--seed N --horizon 10000 --warmup 1000 --set …`), with the
range across the seeds where it says more than the median. About 1 % of
turns, and so 11 % of sessions, reach `max_model_len` and end there.

**The decider.** The guide's `always-disagg-pd-decider` sends every prompt
to a prefiller; the `prefix-based-pd-decider` keeps a follow-up turn whose
context the decoder already has on the decoder. At 0.6 sessions per
second:

| `thr` (nonCachedTokens) | remote prefills | mean TTFT, median (range) | mean response |
|---|---|---|---|
| 1 (always) | 100 % | 29.5 ms (28.3–30.5) | 72.1 ms |
| 512 | 59 % | 28.7 ms (28.0–29.1) | 71.3 ms |
| 2 048 | 22 % | 29.0 ms (28.5–29.6) | 71.8 ms |
| never (decode only) | 0 % | 29.4 ms (28.2–30.4) | 73.8 ms |

Disaggregating everything costs a transfer per request, disaggregating
nothing costs every decoder a prefill in its decode steps, and at this
load the two cost about the same: the four settings lie within a
millisecond. The decider's `thr = 512` sits below both: seed for seed it
is 0.3–1.4 ms under always-disaggregate and 0.2–1.6 ms under decode only,
in all twenty. With the guide's decode profile (the least busy pod,
no prefix affinity) a follow-up turn often lands on the pod that does not
have its context, and the prefiller — which does have it, through the
affinity filter — prefills the new tokens only: 59 % of requests are
remote at `thr = 512` although every first turn is a miss.

**The lease.** A finished prefill's blocks stay allocated on the prefiller
until the decoder has read them, and the decoder reads only once its own
scheduler has room for the whole prompt. Shrinking the decoders' memory
lengthens the lease and grows what the prefillers hold (`thr = 512`; the
prefill pool is 160 000 tokens; a decoder is never smaller than
`max_model_len`):

| decoder memory, tokens per pod | Λ, sessions/s | remote prefills | lease, mean | prefiller memory allocated, mean per pod | prefiller queue, mean | mean TTFT |
|---|---|---|---|---|---|---|
| 160 000 | 0.6 | 59 % | 13 ms | 435 | 0.01 | 28.7 ms |
| 32 000 | 0.6 | 86 % | 33 ms | 944 | 0.01 | 49.1 ms |
| 17 600 | 0.6 | 92 % | 37 ms | 1 079 | 0.01 | 54.7 ms |
| 160 000 | 1.2 | 72 % | 25 ms | 1 650 | 0.03 | 47.8 ms |
| 32 000 | 1.2 | 93 % | 42 ms | 2 728 | 0.04 | 70.7 ms |
| 17 600 | 1.2 | 96 % | 49 ms | 3 046 | 0.04 | 78.4 ms |

Smaller decoders send *more* prompts to the prefillers — they cache less,
so the decider's uncached suffix is longer — and each of those prompts
waits longer for the decoder's room, so the prefillers hold more memory:
2.2–2.5 times at 0.6 sessions per second, 1.7–1.8 times at 1.2. The reads
now share the prefillers' NICs too, and the prefill profile sends most of
them to one prefiller, the one that has the prefix: at 1.2 sessions per
second with the smallest decoders that costs 5 ms of TTFT and 6 ms of lease
over a model that shares the decoders' NICs only. At these
loads that memory is under 2 % of the pool and no prefiller queues; the
deployment pays for the small decoders in TTFT, not yet in admissions.

The decoder's shortage shows up as memory *on the prefiller*, which is the
coupling a store-and-forward program gets backwards: its prefiller holds
through the transfer and lets go before the session queues for the decoder,
so a decoder with no room costs the prefiller nothing.
`tests/pd_semantics.rs` has the deterministic version: six requests, a
decoder with room for one, a prefiller with room for two; the
store-and-forward program prefills all six at once, the NIXL program stops
after the decoder is full and the two leases have taken the prefiller.

## Why you should believe it

The claim is weaker than the [vLLM case study](case-study-vllm.md)'s, and
the difference is the point of saying so.

1. **The source, by line.** The tables above name the function for every
   statement. The vLLM ranges are hashed against `ref/vllm` at `0c87a197`
   by `make check` (`scripts/check_citations.py`), so a range that moves
   fails the build; the llm-d ranges are pinned by commit in the text only.
2. **The statements, deterministically.** `tests/pd_semantics.rs` checks
   `lease`, `release`, `load` and `transfer … from … to …` on pools of ten
   units: a leased pool stays allocated after its scope and is free the
   moment the link run ends, an untaken lease ends at its bound and keeps
   its cache, a re-executed hold releases nothing twice, the prefiller
   stops when the decoder is full.
3. **No machine.** No run of a real deployment is compared with the
   program here: the numbers under "What the program predicts" are the
   program's, not measurements.
4. **No scheduler oracle yet.** `tools/vllm_oracle.py` drives one real
   scheduler with a fake model runner. The oracle this program wants drives
   two — a producer and a consumer with a fake NIXL connector that reports a
   read complete after a chosen number of steps — and compares, per request,
   the step the decoder parks it, the step the prefiller frees its blocks
   and the decoder's first-token step. The one-block cache disagreement of
   the pull run is the first question for it.

---

See also: [The KV transfer](design/pd-transfer.md), [How serQ is checked](validation.md).
