# Prefill/decode examples

`llmd_nixl_pull.sq` describes a serving deployment.

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
