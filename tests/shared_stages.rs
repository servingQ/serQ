//! Deterministic checks of a run over several stages: a flow holds every
//! stage it names at once, at the rate the program's `share` gives it. The
//! expected values are derived by hand in `docs/design/bandwidth-sharing.md`
//! §Expected values; each test's comment repeats the derivation.

use seq::{Overrides, check_source, run_source};

fn run(src: &str) -> seq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
}

fn at(r: &seq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

fn close(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{got:?} vs {want:?}");
    }
}

/// Two decoders read one prompt each from the same prefiller, a second's
/// worth of bytes, both from t = 1. Each read holds the prefiller's egress
/// and its own decoder's ingress, all of capacity 1: the egress is the
/// bottleneck at 1/2 each, so both reads end at 1 + 1 / (1/2) = 3, under
/// either policy. On the decoder's link alone (the program before) they
/// would end at 2.
#[test]
fn two_reads_share_the_senders_link() {
    for share in ["maxmin", "bottleneck"] {
        let src = format!(
            "pool kvP {{ cap 100; }} pool kvD[2] {{ cap 100; }}
             stage P : delay; stage egress : ps(1); stage ingress[2] : ps(1); stage D[2] : delay;
             share {share};
             workload {{ arrive batch(2); }}
             session {{
               set j = serial;
               hold kvP (10) {{ run P (1); }} lease kvP (inf);
               hold kvD[j] (10) {{
                 transfer on egress, ingress[j] (1) from kvP to kvD[j] (10);
                 observe transferred = now;
                 run D[j] (1);
               }}
               end;
             }}
             run {{ horizon 10; }}"
        );
        close(&at(&run(&src), "transferred"), &[3.0, 3.0]);
    }
}

fn three(share: &str, horizon: f64) -> seq::Report {
    run(&format!(
        "stage A : ps(1); stage B : ps(2);
         share {share};
         workload {{ arrive batch(3); }}
         session {{
           branch (serial == 0) {{ run A, B (1); observe f1 = now; }}
           branch (serial == 1) {{ run A (1); observe f2 = now; }}
           branch (serial == 2) {{ run B (1); observe f3 = now; }}
           end;
         }}
         run {{ horizon {horizon}; }}"
    ))
}

/// Three flows of work 1 from t = 0: `A` (capacity 1) carries f1 and f2,
/// `B` (capacity 2) carries f1 and f3, f2 and f3 being single-stage runs on
/// shared stages. Max-min: `A` fills first at 1/2 each for f1 and f2; f3
/// takes the rest of `B`, 3/2, and ends at 2/3; f1 and f2 stay at 1/2 (f1
/// is still held at `A`) and end at 2.
#[test]
fn maxmin_gives_the_rest_to_the_flow_that_can_use_it() {
    let r = three("maxmin", 5.0);
    close(&at(&r, "f1"), &[2.0]);
    close(&at(&r, "f2"), &[2.0]);
    close(&at(&r, "f3"), &[2.0 / 3.0]);
}

/// The same flows under `bottleneck`: f1 = min(1/2, 2/2) = 1/2, f2 = 1/2,
/// f3 = 2/2 = 1. f3 ends at 1; then f1 = min(1/2, 2/1) = 1/2 still, and f1
/// and f2 end at 2. The policies differ in f3 only: the 1/2 of `B` that f1
/// does not use is not given to f3.
#[test]
fn bottleneck_leaves_what_a_flow_cannot_use() {
    let r = three("bottleneck", 5.0);
    close(&at(&r, "f1"), &[2.0]);
    close(&at(&r, "f2"), &[2.0]);
    close(&at(&r, "f3"), &[1.0]);
}

/// A shared stage's utilisation is the capacity its flows carry. Until
/// t = 2/3, `B` carries f1 (1/2) and f3 (3/2 under max-min, 1 under
/// bottleneck) of its 2: 2/2 and 3/4.
#[test]
fn a_shared_stage_reports_the_capacity_it_carries() {
    for (share, want) in [("maxmin", 1.0), ("bottleneck", 0.75)] {
        let r = three(share, 0.5);
        let u = r.stage("B").unwrap().utilization;
        assert!((u - want).abs() < 1e-9, "{share}: {u}");
    }
}

/// A stage that no run holds with another serves as `ps` always has: the
/// report of a program without `also` does not move (the examples' reports
/// and the oracle IR files check the same, more widely).
#[test]
fn a_ps_stage_nobody_shares_is_unchanged() {
    let src = "stage A : ps(1);
               workload { arrive batch(2); }
               session { run A (1); observe done = now; end; }
               run { horizon 5; }";
    close(&at(&run(src), "done"), &[2.0, 2.0]);
}

fn err(src: &str) -> String {
    check_source(src, &Overrides::default()).expect_err("rejected")
}

const HEAD: &str = "stage A : ps(1); stage B : ps(2); stage C : fifo; stage E[2] : ps(1);
                    workload { arrive batch(1); }";

#[test]
fn a_run_over_several_stages_needs_the_programs_share() {
    let e = err(&format!(
        "{HEAD} session {{ run A, B (1); end; }} run {{ horizon 1; }}"
    ));
    assert!(e.contains("needs the program's `share`"), "{e}");
    let e = err(&format!(
        "{HEAD} share maxmin; session {{ run A (1); end; }} run {{ horizon 1; }}"
    ));
    assert!(
        e.contains("`share` without a run over several stages"),
        "{e}"
    );
}

#[test]
fn a_shared_stage_is_ps_of_a_constant() {
    let e = err(&format!(
        "{HEAD} share maxmin; session {{ run A, C (1); end; }} run {{ horizon 1; }}"
    ));
    assert!(e.contains("stage `C`"), "{e}");
    assert!(e.contains("constant"), "{e}");
    let e = err(
        "stage A : ps(1); stage N : ps(min(present, 4)); share maxmin;
         workload { arrive batch(1); }
         session { run A, N (1); end; } run { horizon 1; }",
    );
    assert!(e.contains("stage `N`"), "{e}");
}

/// An index is known only when the run starts, so `E[0], E[1]` could be
/// one stage twice: one array appears once in a run.
#[test]
fn a_run_names_each_stage_array_once() {
    let e = err(&format!(
        "{HEAD} share maxmin; session {{ run E[0], E[1] (1); end; }} run {{ horizon 1; }}"
    ));
    assert!(e.contains("named twice"), "{e}");
}

/// `transfer on a, b (w) from P to Q (n)` is the kernel's
/// `run a, b (w); load Q (n); release P;`.
#[test]
fn transfer_on_several_stages_is_sugar() {
    let head = "pool p { cap 10; } pool q { cap 10; }
                stage a : ps(1); stage b : ps(1); share maxmin;
                workload { arrive batch(1); }";
    let ir = |body: &str| {
        let src = format!(
            "{head} session {{ hold q (1) {{ hold p (1) {{ {body} }} }} end; }} run {{ horizon 5; }}"
        );
        seq::compile_source(&src, &Overrides::default())
            .unwrap()
            .to_json()
    };
    assert_eq!(
        ir("transfer on a, b (1) from p to q (1);"),
        ir("run a, b (1); load q (1); release p;")
    );
}

/// A flow counts once at every stage it held: `A` saw f1 and f2 end, `B`
/// saw f1 and f3.
#[test]
fn a_flow_is_counted_at_each_of_its_stages() {
    let r = three("maxmin", 5.0);
    assert_eq!(r.stage("A").unwrap().completed, 2);
    assert_eq!(r.stage("B").unwrap().completed, 2);
}

/// The same seed gives the same report, digit for digit: nothing a shared
/// stage reports may depend on the order a hash map keeps its flows in.
#[test]
fn a_seed_reproduces_a_run_with_flows() {
    let src = "stage E[3] : ps(1); stage F : ps(2); stage I[4] : ps(1);
               share maxmin;
               workload { arrive poisson(6.5); }
               session {
                 choose i in 3 by (~uniform(0, 1));
                 choose j in 4 by (~uniform(0, 1));
                 run E[i], F, I[j] (~exp(0.3));
                 observe left = work(F);
                 end;
               }
               run { horizon 200; warmup 20; seed 3; }";
    let a = run(src).json();
    for _ in 0..4 {
        assert_eq!(run(src).json(), a);
    }
}

#[test]
fn a_shared_stage_has_a_capacity_above_zero() {
    let e = err("stage A : ps(1); stage Z : ps(0); share maxmin;
         workload { arrive batch(1); }
         session { run A, Z (1); end; } run { horizon 1; }");
    assert!(e.contains("above 0"), "{e}");
}

#[test]
fn share_is_given_once() {
    let e = err("stage A : ps(1); stage B : ps(1);
         share maxmin;
         share bottleneck;
         workload { arrive batch(1); }
         session { run A, B (1); end; } run { horizon 1; }");
    assert!(
        e.contains("`share` is given twice: the first is on line 2"),
        "{e}"
    );
}
