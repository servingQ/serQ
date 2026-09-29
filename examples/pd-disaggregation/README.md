# Prefill/decode examples

`llmd_pd.seq` and `lecture_pd.seq` describe serving deployments.

`pd_open.seq` and `pd_tandem.seq` are research comparison fixtures for
serving-queue-theory's queueing models. The open model uses Poisson arrivals;
the saturated model circulates a closed population. They remain packaged
here so the downstream Rust adapter can load the exact program belonging
to its pinned seQ revision.

`mode = 0` selects the aggregate queue; `mode = 1` selects the prefill,
link and decode tandem. Both modes use the same work draws. A deployment
drawing shows all declared queues, including inactive ones: it depicts the
comparison fixture, not a single active serving deployment.
