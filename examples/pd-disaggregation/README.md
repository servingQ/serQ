# Prefill/decode examples

`llmd_nixl_pull.serq` describes a serving deployment.

`pd_open.serq` and `pd_tandem.serq` are research comparison fixtures for
serving-queue-theory's queueing models. The open model uses Poisson arrivals;
the saturated model circulates a closed population. They remain packaged
here so the downstream Rust adapter can load the exact program belonging
to its pinned serQ revision.

`mode = 0` selects the aggregate queue; `mode = 1` selects the prefill,
link and decode tandem. Both modes use the same work draws. A deployment
drawing shows all declared queues, including inactive ones: it depicts the
comparison fixture, not a single active serving deployment.
