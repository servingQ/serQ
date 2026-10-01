//! The laws of `docs/design/stage-algebra.md`: a stage as its throughput
//! φ(present), pooling as the sup-convolution of two φ, holding at once as
//! their pointwise minimum. The language has no operator for either; these
//! tests check that the forms it has obey the laws, and the one law that
//! fails (distributivity) fails the way a shared stage should.

use serq::{Overrides, run_source};

fn run(src: &str) -> serq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
}

fn at(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn close(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{got:?} vs {want:?}");
    }
}

/// `fifo(1) ⊕ fifo(1) = fifo(2)`, written as the throughput it pools to,
/// `ps(min(present, 2))`. With exponential work the number present is the
/// same Markov chain at both, M/M/2, so the mean sojourn is the same and
/// is Erlang C's: ρ = 3/4, P(wait) = 2ρ²/(1 + ρ) = 9/14, and
/// W = 1 + P(wait)/(2 − λ) = 1 + 9/7 = 16/7. The orders differ, so the
/// sojourns of one request do not. One seed draws the same arrivals and
/// work for both, so the two means are closer to each other (4e-3 over
/// seeds 1–3 and 11) than either is to 16/7 (the 95 % interval is ±0.04
/// to ±0.08).
#[test]
fn pooling_two_servers_is_a_server_of_two() {
    let w = |kind: &str| {
        let r = run(&format!(
            "stage svc : {kind};
             workload {{ arrive poisson(1.5); }}
             session {{ set t0 = now; run svc (~exp(1)); observe sojourn = now - t0; end; }}
             run {{ horizon 200_000; warmup 1_000; seed 11; }}"
        ));
        mean(&at(&r, "sojourn"))
    };
    let (fifo, ps) = (w("fifo(2)"), w("ps(min(present, 2))"));
    assert!((fifo - ps).abs() < 0.02, "fifo(2) {fifo}, ps {ps}");
    for got in [fifo, ps] {
        assert!((got - 16.0 / 7.0).abs() < 0.1, "{got} vs 16/7");
    }
}

/// `ps(1) + ps(1)` has two readings, and they part when a job is alone.
/// One server of capacity 2, `ps(2)`, gives a lone job the whole 2; two
/// servers pooled, `ps(min(present, 2))`, give it one of them. With two
/// jobs present they agree.
#[test]
fn one_fast_server_is_not_two_servers() {
    let ends = |kind: &str, jobs: u32| {
        let r = run(&format!(
            "stage svc : {kind};
             workload {{ arrive batch({jobs}); }}
             session {{ run svc (1); observe ended = now; end; }}
             run {{ horizon 10; warmup 0; }}"
        ));
        at(&r, "ended")
    };
    close(&ends("ps(2)", 1), &[0.5]);
    close(&ends("ps(min(present, 2))", 1), &[1.0]);
    close(&ends("ps(2)", 2), &[1.0, 1.0]);
    close(&ends("ps(min(present, 2))", 2), &[1.0, 1.0]);
}

/// φ ⊗ (ψ ⊕ ψ) ≠ (φ ⊗ ψ) ⊕ (φ ⊗ ψ): distributing would copy φ. Two jobs
/// of work 1 from t = 0. Left, both hold one `A` of capacity 1 and the
/// pooled `B` of capacity 2 (`ps(1) ⊕ ps(1)` while both are present, which
/// is until they end together): `A` is the bottleneck at 1/2 each, so both
/// end at 2. Right, each holds its own `A[j]` and `B[j]`, both of
/// capacity 1, and ends at 1. Under either `share`.
#[test]
fn a_shared_stage_is_not_copied() {
    for share in ["maxmin", "bottleneck"] {
        let left = run(&format!(
            "stage A : ps(1); stage B : ps(2);
             share {share};
             workload {{ arrive batch(2); }}
             session {{ run A, B (1); observe ended = now; end; }}
             run {{ horizon 10; warmup 0; }}"
        ));
        close(&at(&left, "ended"), &[2.0, 2.0]);
        let right = run(&format!(
            "stage A[2] : ps(1); stage B[2] : ps(1);
             share {share};
             workload {{ arrive batch(2); }}
             session {{ set j = serial; run A[j], B[j] (1); observe ended = now; end; }}
             run {{ horizon 10; warmup 0; }}"
        ));
        close(&at(&right, "ended"), &[1.0, 1.0]);
    }
}
