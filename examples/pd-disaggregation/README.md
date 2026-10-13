# Prefill/decode examples

`llmd_nixl_pull.sq` describes a serving deployment. `vllm_nixl_push.sq`
is vLLM's push proxy in front of one prefill and one decode instance on the
A6000 testbed, replaying the short-context trace: the proxy's two legs as
`fork` and `join`, and the copy as `kvt.nixl_push (P, D, …)`
(`docs/design/push-mode.md`).

`pd_open.sq` and `pd_tandem.sq` are research comparison fixtures for
serving-queue-theory's queueing models. The open model uses Poisson arrivals;
the saturated model circulates a closed population. They remain packaged
here so the downstream Rust adapter can load the exact program belonging
to its pinned serQ revision.

`mode = 0` selects the aggregate queue; `mode = 1` selects the prefill,
link and decode tandem. Both modes use the same work draws. A deployment
drawing shows all declared queues, including inactive ones: it depicts the
comparison fixture, not a single active serving deployment.

`pd_batching.sq` runs the same requests on N engines colocated or split
into NP prefill and ND decode engines, with a free transfer, to see what
the split does to the time per output token at equal throughput (#208);
`pd_ps.sq` is the processor-sharing idealisation it is compared to.
`tools/pd_batching/` sweeps both and keeps the results.

`pd_two_fabrics.sq` puts the prefill and decode GPUs on two nodes: a
decoder on its prefiller's node reads the KV over NVLink, one on the other
node over RDMA, and the two fabrics divide their ports by two policies,
`share.bottleneck` and `share.maxmin`, written in the kernel's
`transfer on a, b (w, share = …)`. Neither policy is measured; the A6000
testbed moved its KV inside one node and cannot tell them apart.
