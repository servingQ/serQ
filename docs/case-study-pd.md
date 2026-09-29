# Case study: prefill/decode disaggregation over NIXL

`programs/llmd_pd.seq` is llm-d's prefill/decode split on vLLM: the router
that decides which pod prefills and which decodes, the sidecar that sends
the prompt to one and the decode request to the other, and the two
schedulers that hand the KV over with the NIXL connector. Every line is
checked against the source: llm-d at `8a2f37d`, its router
(`llm-d-inference-scheduler`) at `13eebdb`, vLLM at `0c87a197` (`ref/vllm`;
the vLLM citations are hashed by `make check`, the llm-d ones are not).

It is a specification checked against the code, not yet against a
scheduler oracle. What the program cannot say without two new statements,
and what it still cannot say, is in [The KV transfer](design/pd-transfer.md).

![llm-d prefill/decode over NIXL as a queueing network](assets/llmd_pd.deployment.svg)

The prompt's KV is in the prefiller's pool through the transfer and in the
decoder's from the transfer on, which is why the two enclosures cross at the
link.

## The deployment

llm-d's P/D guide (`guides/pd-disaggregation`, llm-d `8a2f37d`) deploys
this:

| Piece | Configuration | Where |
|---|---|---|
| model servers | 8 prefill pods (`vllm serve`, TP 1) and 2 decode pods (TP 4), gpt-oss-120b, `--block-size 128`, `--kv-transfer-config '{"kv_connector":"NixlConnector","kv_role":"kv_producer" / "kv_consumer","kv_connector_extra_config":{"backends":["UCX"]}}'`, `VLLM_NIXL_SIDE_CHANNEL_HOST` the pod IP | `modelserver/gpu/vllm/base/patch-prefill.yaml`, `patch-decode.yaml` |
| router (EPP) | `disagg-profile-handler` with `always-disagg-pd-decider` (every request is prefilled remotely); decode profile `decode-filter` → `active-request-scorer` → `max-score-picker` (the least busy decoder); prefill profile `prefill-filter` → `prefix-cache-affinity-filter` (`approx-prefix-cache-producer`, a cache-warm prefiller first) → `token-load-scorer` → `max-score-picker`; one EPP replica | `router/pd-disaggregation.values.yaml` |
| sidecar | `llm-d-routing-sidecar` on each decode pod, connector `nixlv2`: prefill leg (`max_tokens = 1`, `do_remote_decode`), wait, decode leg with the prefiller's block ids; serial | `pkg/sidecar/proxy/connector_nixlv2.go` (router `13eebdb`) |
| alternative decider | `prefix-based-pd-decider` with `nonCachedTokens`, `promptTokens`: remote prefill only when the uncached suffix on the chosen decoder is long enough | `docs/disaggregation.md`, `profilehandler/disagg/README.md` |

The [testbed](#measured-and-predicted) below stands in one node for it:
one or two prefillers and decoders, Qwen3-8B, block 16, and vLLM's own
NIXL proxies in place of the sidecar (the same serial protocol for pull;
the two legs at once for push). What differs from the guide is written next
to each number.

## In seQ

```seq title="programs/llmd_pd.seq"
--8<-- "programs/llmd_pd.seq"
```

### Line by line

#### The router and the sidecar, against llm-d

| llm-d | seQ | Where |
|---|---|---|
| the endpoint picker runs the decode profile first and picks a decode pod; the guide's decode profile scores by active requests (the least busy) | `choose j in ND by (holders(D[j].kv) + queued(D[j].kv))` | `disagg_profile_handler.go:316-334`; `guides/pd-disaggregation/router/pd-disaggregation.values.yaml` |
| the decider: a remote prefill when the prompt's uncached suffix on the chosen decode pod is at least `nonCachedTokens`, and the prompt at least `promptTokens`; the cached part is the router's own estimate of the pod's prefix cache | `set hitD = min(cachedin(D[j].kv), floor((prompt - 1) / bs) * bs); set remote = prompt >= minp && prompt - hitD >= thr;` | `prefix_based_pd_decider.go:266-303`; `disagg_profile_handler.go:353-366`. The guide runs `always-disagg-pd-decider`, which is `thr = 1` |
| the prefill profile: pods that have the prefix (`prefix-cache-affinity-filter`), then the least loaded (`token-load-scorer`) | `choose i in NP by ((cachedin(P[i].kv) > 0 ? 0 : 1) * 1e9 + work(P[i]) + queued(P[i].reqs))` | the same values file |
| the decode pod's sidecar sends the prompt to the prefiller with `max_tokens = 1` and `do_remote_decode`, waits for the answer, then sends the decode request to its own engine with the prefiller's block ids | `P[i].prefill (prompt);` then `D[j].decode (prompt) from P[i];` — the prefiller's entry, its blocks leased at its end, then the decoder's | `connector_nixlv2.go:69-232` (the prefill leg, `CapSingleToken` at 145), `261-379` (the decode leg) |
| no prefill header: the request goes to the decode pod's engine as it is | the `else` branch, `D[j].decode (prompt);`: vLLM's engine on one device (`programs/vllm.seq`) | `dispatch.go:196-213` |
| the two legs in parallel, so the decoder allocates while the prefiller works | not written: a session waits at one pool at a time ([The KV transfer](design/pd-transfer.md)) | `connector_nixlv2.go:60-67` (MoRI-IO write mode only); vLLM's push-mode proxy, `disagg_proxy_pushconnector_demo.py:227-270` |

#### The two schedulers, against vLLM

| vLLM | seQ | Where |
|---|---|---|
| the prefiller admits like any vLLM engine: a slot, the blocks of the first chunk, room for the whole prompt, the prefix hit looked up when the scheduler takes the request | `admit if reqs (1), kv (min(prompt, hit + budget_left(P))) reserve (prompt) fit where hit = …` in `P`'s `prefill` entry | the waiting loop, `scheduler.py:868-1128`; [the vLLM case study](case-study-vllm.md) |
| the prefiller computes the prompt in chunks and samples one token, which the sidecar discards | `prefill (prompt - c) growing kv` | `scheduler.py:624-823`; the truncation for Mamba and MTP only, `nixl/base_scheduler.py:409-436` |
| the request finishes on the prefiller: its slot is freed, its blocks are not — `request_finished` returns `delay_free_blocks` and a lease of `kv_lease_duration` (30 s), renewed by the decoder's heartbeats while the request waits | `} keep (prompt) lease kv (inf);` — the scope ends, the slot goes, the blocks stay the session's | `nixl/pull_scheduler.py:191-292`; `_free_request`, `scheduler.py:2628-2657`; the renewal, `nixl/base_scheduler.py:199-238`, `nixl/base_worker.py:3010-3030` |
| the prefiller keeps every computed full block of the prompt cached once the lease ends | `keep (prompt)` on that hold, applied when the lease ends | `_connector_finished`, `scheduler.py:2929-2982`; `kv_cache_manager.py:610-619` |
| the decoder's scheduler looks at its waiting queue only at a step with budget left and a running slot free | `admit via D` on both of the decoder's pools; `reqs (0) reserve (1)` — a slot must be free, none is taken | `scheduler.py:872-879` |
| the decoder's local prefix hit, then the connector: for a remote prefill every prompt token beyond the local hit is external and loaded asynchronously | `kv (known) reserve (known)` with `reuse (floor((known - 1) / bs) * bs)` in `D`'s `decode … from` entry; `c = cached` is the local hit | `scheduler.py:932-954`; `nixl/pull_scheduler.py:34-66` |
| blocks are allocated for the whole prompt, and the request is parked, `WAITING_FOR_REMOTE_KVS`, holding them and no slot; one transfer per request | the hold on `kv`; `transferred` is `do_remote_prefill`, spent | `scheduler.py:1199-1226, 1264-1294`; `nixl/pull_scheduler.py:108-189` |
| the worker reads the blocks from the prefiller over NIXL (pull mode: a NIXL READ issued by the decoder) | `nic[self].transfer (prompt - c) from src to kv (prompt - 1 - c)`: the decoder's NIC, whose `transfer (n)` entry takes `x0 + n / Bw` | `nixl/pull_scheduler.py:168-177`; the worker's `_read_blocks`, `nixl/pull_worker.py:392-575` |
| the read done, the blocks are cached, the last prompt token is marked uncomputed (its logits are needed), and the request is back in the waiting queue, served before new arrivals | the `load D.kv[j] (prompt - 1 - c)` the transfer stands for; `admit if reqs (1) fit`, with `reqs` declared before `kv` | `_update_waiting_for_remote_kv`, `scheduler.py:3032-3077`; `_try_promote_blocked_waiting_request`, `scheduler.py:3079-3092`; `scheduler.py:2383-2385` |
| the prefiller frees the leased blocks when the read completes | the `release P.kv[i]` the transfer stands for takes the lease | `_update_from_kv_xfer_finished`, `scheduler.py:3113-3138` |
| the decoder recomputes the last prompt token and decodes; a request preempted afterwards is rescheduled without a second transfer, prefilling locally what it lost | `prefill (known - c) growing kv; decode (o - 1 - (known - prompt)) growing kv;` with `known` from `computed` | `scheduler.py:1560-1561`; `nixl/pull_scheduler.py:187-189` |
| a parked request is in no `running` list and is not preempted; nor is the prefiller's finished one | `preempt lifo` takes the last admitted *resident* of the engine | `scheduler.py:742-813` |
| the decoder keeps the prompt's and the output's full blocks cached | `} keep (prompt + o)` | `kv_cache_manager.py:602-606` |

### Writing xPyD

The deployment is queues: `NP` prefill instances and `ND` decode instances,
each with its own KV pool, request-slot pool, engine and NIC, and one
gateway selected by `request gw;` in the workload's session.

```
queue gw : gateway { route { … } }                 // the router and the sidecar
queue P[NP] : prefill { pool reqs { … admit via P; } pool kv { … } serve step { … memory kv; } prefill (prompt) { … } }
queue D[ND] : decode  { pool reqs { … admit via D; } pool kv { … admit via D; } serve step { … memory kv; } decode (prompt) { … } decode (prompt) from src { … } }
queue nic[ND] : link  { serve ps(1); transfer (n) { run (x0 + n / Bw); } }
stage tool : delay;
```

A queue's pools are its members': `P[i].kv` is the KV of the i-th
prefiller, `admit via P` inside `P` means the member's own scheduler serves
the pool, and `memory kv` on its stage counts the member's own blocks as its
residents' memory (`kvb`, `kvp`). The family's size is the `let` the router
chooses over (`queue D[ND]`, `choose j in ND`), so the number is written
once.

The router is the gateway's two `choose`s, and the instance it picked is
the queue the request is handed to: `P[i].prefill (prompt)`, then
`D[j].decode (prompt) from P[i]`. Inside an entry nothing is indexed — `kv`,
`reqs`, `P` are the member's own — and `self` is the member's index where
another family has to be matched (`nic[self]`). `from P[i]` is the pool
`P`'s entry leases, and the transfer's `load` and `release` name it as the
holds did, index included, because the linker writes them. The admission,
the allocation and the two runs are the entries' and appear once each:
the router's body has no `admit if`.

Going from 2P2D to 4P8D is the two `let`s; the program does not change
otherwise, which is the point of writing the router as `choose` over a
family rather than as a branch per instance (`programs/routing.seq` still
has the branch-per-policy shape the design notes call a smell).

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
decoder is admitted, and the decoder's NIC does the copy:

```
P[i].prefill (prompt);              // admit if reqs (1), kv (…) fit … { prefill (…) growing kv; } keep (prompt) lease kv (inf);
D[j].decode (prompt) from P[i];     // admit if kv (known) reserve (known), reqs (0) reserve (1) fit … {
                                    //   nic[self].transfer (prompt - c) from src to kv (prompt - 1 - c);   // the decoder's NIC READs; the lease ends
                                    //   admit if reqs (1) fit { … }
                                    // } keep (prompt + o);
```

Push through the llm-d sidecar (serial dispatch) is the same three lines
with two differences a reader can see: the copy is the prefiller's WRITE,
so it runs on the prefiller's NIC, and it starts one notification after the
decoder's admission (the registration, `nixl/push_scheduler.py:128-205`):

```
stage linkP[2] : ps(1);                                   // the prefillers' NICs
…
admit if kvD[j] (known) reserve (known), reqsD[j] (0) reserve (1) fit … {
  transfer on linkP[i] (x0 + x_reg + (prompt - c) / Bw) from kvP[i] to kvD[j] (prompt - 1 - c);
  admit if reqsD[j] (1) fit { … }
} keep (prompt + o);
```

`transfer on linkP[i] (…) from kvP[i] to kvD[j] (…)` is the same kernel
statements — `run linkP[i]; load kvD[j]; release kvP[i]` — on the other
side's stage. Which NIC saturates under load is what the two programs
differ in, and a deployment with more prefillers than decoders (or the
reverse) will show it. Written with queues, the copy is still inside the
decoder's `decode … from src` entry, and that entry sees the source pool
and nothing else of the prefiller — not which member's NIC `src` belongs
to. Push mode wants the link named by the `from` (`nicP[src]`, or a link
the prefiller owns), which [the design document](design/queue.md) leaves
open.

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
admit if reqsP[i] (1), kvP[i] (…) fit … { … } keep (prompt) lease kvP[i] (inf);
enter kvD[j] { transfer on linkP[i] (…) from kvP[i] to kvD[j] (…); … }   // open the booking, waiting if it is not granted
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
`vllm.seq`); the router's approximate prefix cache as a data structure of
its own (its estimate is taken to be the pod's cache).

## Measured and predicted

### On the A6000 testbed

One Lambda Cloud `gpu_4x_a6000` node (four RTX A6000, 48 GB each, PCIe;
vLLM at `0c87a197`, NIXL 1.4.1 over UCX with `cuda_ipc,cuda_copy,tcp,sm`;
Qwen3-8B, block 16, budget 8 192, `max_num_seqs` 16 on a prefiller and 64
on a decoder, 192 000-token KV pools) ran the deployment of
`programs/llmd_pd_replay.seq`: one prefiller, one decoder, vLLM's own
proxy in front (`tests/v1/kv_connector/nixl_integration/toy_proxy_server.py`
for pull, `disagg_proxy_pushconnector_demo.py` for push), replaying the
short-context trace of the [vLLM case study](case-study-vllm.md) — 96
sessions sent 3 s apart, 957 requests, prompts of 2 000–30 000 token ids
sent as ids so that every length is the trace's — with the scripts and the
raw records in `tools/a6000/`.

**The constants, from lone requests.** Fourteen requests of 512–32 000
tokens through the proxy with nothing else in flight
(`tools/a6000/pull1x1_probe.jsonl`, fitted by `fit_probe.py`): a transfer
takes 10 ms + tokens / 99 800 s (13 of 14; one 32 000-token read took 1.5 s
and was left out), a prefill 119 µs per token + 7.06 ns per token per
context token, a decode step 24 ms, and the rest of a lone request's TTFT —
the two proxy hops, the API servers — 63 ms. Those eight numbers are the
`let`s of the program; nothing is fitted on the replay.

**Pull, request for request** (`tools/a6000/pull1x1_s3_rounds.jsonl`,
`compare.py`):

| | measured | `llmd_pd_replay.seq` |
|---|---|---|
| mean TTFT | 1.300 s | 1.088 s |
| median TTFT | 0.692 s | 0.562 s |
| p99 TTFT | 5.98 s | 5.11 s |
| first turns, mean | 1.284 s | 1.116 s |
| follow-up turns, mean | 1.302 s | 1.085 s |
| decoder's cached tokens on follow-ups, mean | 5 774 | 5 769 |
| follow-ups with the cached count exactly right | | 742 of 861 |

The cache agrees: the decoder's `cached_tokens` is the program's
`d_cached` on 742 of 861 follow-ups, and 113 of the other 119 are one
block (16 tokens) lower on the machine, which is the same question as the
vLLM case study's `keep (prompt + out - 1)` — whether the block a request's
last step fills is cached before the request is freed on the decode side of
a transfer — and belongs to the P/D oracle. The time is under-predicted
where the prefiller queues: requests measured under 0.5 s are predicted
within 0.14 s, those measured above 3 s are predicted 1–2 s short, and the
prefiller's `running` reached 10 with a 6-deep queue. The lone-request
constants say what one request costs; what several cost together on this
proxy — which forwards the prefill leg unstreamed and the decode leg
streamed from one Python process — is not in them, as the A100 replay's
two served-path constants were not in its step fit either. The prefiller's
KV usage averaged 11 % and peaked at 77 %; the leases are short at this
load (47 ms mean in the program).

**Push, the same trace** (`NixlPushConnector` on both engines, vLLM's
`disagg_proxy_pushconnector_demo.py`, `tools/a6000/push1x1_s3_rounds.jsonl`).
The lone requests are the pull run's within noise up to 4 096 tokens
(`tools/a6000/push1x1_probe.jsonl`: 0.09–0.86 s against 0.14–0.69 s) and
one to two seconds slower at 8 192 tokens and above, where some writes stall;
the first request paid 3.6 s for the handshake. Under the replay's load the
deployment collapsed: the prefiller's 1 944 writes averaged 1.03 s each
(`nixl_xfer_time_seconds` on the prefiller; 41 ms of it the submission),
requests parked on the decoder waiting for their write held the decoder's
KV at 94 % on average and 100 % at peak with 100 to 150 requests waiting on
each side, the mean TTFT was 95 s and the 96 sessions took 1 468 s against
435 s in pull mode. This is the commit's push connector as shipped
(`vllm/distributed/kv_transfer/kv_connector/v1/nixl/push_worker.py`, one
writer thread per rank) through a demo proxy, on one node over `cuda_ipc`;
it is a measurement of that, not of push mode in general.

The program says what such a write time does. With the pull constants and
`x0` set to the measured 1.03 s per transfer, `llmd_pd_replay.seq` gives a
mean TTFT of 73 s, a mean lease of 21 s, and 57 000 tokens of the decoder's
pool allocated on average (30 %; measured 94 %): the transfers form a
queue whose throughput is one per second against 2.2 requests per second,
so every arrival after the first minute waits behind it, holding its blocks
on both instances — the coupling of the lease table above, reached from the
transfer's side. The decoder's occupancy is under-predicted, and not by the
link: with the link a one-at-a-time `fifo` stage the program gives 70 s and
32 %. The gap is the dispatch. vLLM's push proxy sends the decode leg at
once, so the real decoder took every request as it arrived and parked it
with its blocks allocated while the prefiller was still queued and
prefilling — 100 to 150 of them at a time — whereas the program's decoder
is asked only after the prefill, the serial order a session can write. The
collapse is in the program; the decoder's share of it is the concurrent
dispatch the reservation of [The KV transfer](design/pd-transfer.md) would
write, measured.

Two things the runs settle about the two modes. Through a serial proxy the
lifecycle is the one program: the same trace, the same admissions, and the
cache agreeing request for request in pull mode. And the mode is a
*constant* of that program, not a construct: what changed between the runs
is `x0` (and, in an xPyD deployment, which NIC the transfer runs on), and
with the measured constant the program predicts the collapse the run
showed.


### What the program predicts at loads the testbed did not run

Two decode pods of 160 000 tokens each, two prefill pods, sessions of
one to several turns (a prompt of 1 000–3 000 new tokens on a growing
context, 200 output tokens, a 3 s tool call between turns, `p = 0.9`), the
A100-shaped step cost of `programs/vllm.seq`, a 200 000 token/s link.

**The decider.** The guide's `always-disagg-pd-decider` sends every prompt
to a prefiller; the `prefix-based-pd-decider` keeps a follow-up turn whose
context the decoder already has on the decoder. At 0.6 sessions per
second:

| `thr` (nonCachedTokens) | remote prefills | mean TTFT | mean response |
|---|---|---|---|
| 1 (always) | 100 % | 46.6 ms | 90.6 ms |
| 512 | 65 % | 40.6 ms | 84.4 ms |
| 2 048 | 30 % | 39.6 ms | 83.7 ms |
| never (decode only) | 0 % | 66.0 ms | 120.1 ms |

Disaggregating everything costs a transfer per request, disaggregating
nothing costs every decoder a prefill in its decode steps, and the decider
sits between the two. With the guide's decode profile (the least busy pod,
no prefix affinity) a follow-up turn often lands on the pod that does not
have its context, and the prefiller — which does have it, through the
affinity filter — prefills the new tokens only: 65 % of requests are remote
at `thr = 512` although every first turn is a miss.

**The lease.** A finished prefill's blocks stay allocated on the prefiller
until the decoder has read them, and the decoder reads only once its own
scheduler has room for the whole prompt. Shrinking the decoders' memory
lengthens the lease and fills the prefillers (Λ = 0.6, `thr = 512`; the
prefill pool is 160 000 tokens):

| decoder memory, tokens per pod | Λ, sessions/s | remote prefills | lease, mean | prefiller memory allocated, mean of 160 000 (pod 1 / pod 2) | prefiller queue, mean | mean TTFT | sessions completed |
|---|---|---|---|---|---|---|---|
| 160 000 | 0.6 | 65 % | 21 ms | 1 800 / 400 | 0.0 | 41 ms | 1 059 |
| 32 000 | 0.6 | 91 % | 45 ms | 3 200 / 900 | 0.0 | 66 ms | 1 063 |
| 160 000 | 1.2 | 78 % | 43 ms | 6 900 / 4 400 | 0.1 | 100 ms | 2 160 |
| 32 000 | 1.2 | 95 % | 81 ms | 64 300 / 57 600 | 38.7 | 159 ms | 1 573 |

At 1.2 sessions per second the prefillers have room and compute to spare
(two of sixteen slots busy), and still their queues hold 39 requests: two
fifths of each prefiller's memory is leased to requests parked at a decoder
that has room for one or two of them. Smaller decoders also send *more*
prompts to the prefillers — they cache less, so the decider's uncached
suffix is longer — which is the other direction of the same coupling.

The decoder's shortage shows up as memory *on the prefiller*, which is the
coupling `programs/lecture_pd.seq` gets backwards: its prefiller holds
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
3. **The machine, request for request.** The A6000 runs above: the
   decoder's cached tokens agree with the program on 742 of 861 follow-ups
   in pull mode, the lone-request constants predict a lightly loaded
   prefiller's TTFT within 0.14 s and under-predict a queued one's by a
   third, and the measured push-mode write time reproduces the collapse.
   Two runs at one load, not a sweep.
4. **No scheduler oracle yet.** `tools/vllm_oracle.py` drives one real
   scheduler with a fake model runner. The oracle this program wants drives
   two — a producer and a consumer with a fake NIXL connector that reports a
   read complete after a chosen number of steps — and compares, per request,
   the step the decoder parks it, the step the prefiller frees its blocks
   and the decoder's first-token step. The one-block cache disagreement of
   the pull run is the first question for it.

---

See also: [The KV transfer](design/pd-transfer.md), [How seQ is checked](validation.md).
