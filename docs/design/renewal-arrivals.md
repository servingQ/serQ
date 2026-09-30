# Renewal arrivals and finite open runs

Issue [#108](https://github.com/vrvrv/serQ/issues/108) and
PR [#107](https://github.com/vrvrv/serQ/pull/107) add non-exponential
interarrival distributions and finite samples for queue validation.

## Contract

`renewal(expr)` evaluates a positive, finite gap on the arrival RNG stream,
including the gap from zero to the first arrival. Constants such as
`renewal(2)` are valid. Expressions cannot depend on a session or on the
state of the deployment. Arrival draws do not consume workload or session
randomness.

`arrivals N` requires exactly N arrivals, followed by completion of all
active sessions. The configured horizon is a deadline for both phases.
Insufficient arrivals, unfinished sessions, and completion before or at
warmup return errors rather than partial or empty reports. Completion
exactly at the horizon succeeds. Reports retain the configured `horizon`
and add `end`; the measurement interval is `end - warmup`.

IR 5 has shipped in release tags. `Renewal` and `arrivals` therefore open
IR 6, even though serde can default the latter. The oracle IR is regenerated
and the serving-queue-theory generator moves to 6. Renewal and finite open
runs are outside its explicit-session Lean fragment and are rejected.

## First-arrival decision and self-critique

Keep `Poisson(rate)` for this change. Its existing first arrival is at zero;
`Renewal(~exp(1 / rate))` first waits one exponential gap. This distinction
is now documented and tested. It preserves existing seeded workloads and
published simulation inputs.

We considered lowering Poisson to Renewal directly. That would remove the
initial session from every existing Poisson workload and change finite
samples and observations. Preserving those workloads with one IR variant
would require a separate first-arrival expression and corresponding source
syntax. That is a useful follow-up to the frontend's process design, but
this change does not invent that extra mechanism. The remaining limitation
is explicit: general renewal processes cannot request a first arrival at
zero. Retaining the two variants pays an IR cost for compatibility.

## PD comparison programs

Keep `pd_open.sq` and `pd_tandem.sq` packaged under `examples/` as research
comparison fixtures used by serving-queue-theory. They are queueing models,
not descriptions of an actual serving deployment. Their `mode` switch is
part of the paired experiment: both variants draw the same work demands.
A deployment drawing includes both topologies and should not be presented
as the active topology for one mode. The directory README makes this scope
explicit; a real deployment is illustrated by `llmd_pd.sq`.
