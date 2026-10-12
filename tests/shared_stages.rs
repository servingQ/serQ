//! Deterministic checks of a run over several stages: a flow holds every
//! stage it names at once, at the rate the program's `share` gives it. The
//! expected values are derived by hand in `docs/design/bandwidth-sharing.md`
//! §Expected values; each test's comment repeats the derivation.

mod common;

use serq::{Overrides, check_source, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

fn at(r: &serq::Report, name: &str) -> Vec<f64> {
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
        use \"std/share\";
        workload {{ arrive batch(2);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set j = serial;
          hold kvP (cost(kvP, 10)) {{ run P (cost(P, 1)); }} lease kvP (inf);
          hold kvD[j] (cost(kvD, 10)) {{
            transfer on egress, ingress[j] (1, share = share.{share}) from kvP to kvD[j] (10);
            observe transferred = now;
            run D[j] (cost(D, 1));
          }}
        }}
        "
        );
        close(
            &at(&run(&src, &common::horizon(10.0)), "transferred"),
            &[3.0, 3.0],
        );
    }
}

fn three(share: &str, horizon: f64) -> serq::Report {
    run(
        &format!(
            "stage A : ps(1); stage B : ps(2);
        use \"std/share\";
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          branch (serial == 0) {{ run A, B (cost(A, B, 1), share = share.{share}); observe f1 = now; }}
          branch (serial == 1) {{ run A (cost(A, 1)); observe f2 = now; }}
          branch (serial == 2) {{ run B (cost(B, 1)); observe f3 = now; }}
        }}
        "
        ),
        &common::horizon(horizon),
    )
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
        workload { arrive batch(2);
          session { turn; end;
          }
        }
        server { run A (cost(A, 1)); observe done = now;
        }
        ";
    close(&at(&run(src, &common::horizon(5.0)), "done"), &[2.0, 2.0]);
}

fn err(src: &str) -> String {
    check_source(&common::main_source(src), &common::horizon(10.0)).expect_err("rejected")
}

const HEAD: &str = "stage A : ps(1); stage B : ps(2); stage C : fifo; stage E[2] : ps(1);
                    ";
const HEAD_WORKLOAD: &str = "arrive batch(1);";

fn server(body: &str) -> String {
    err(&format!(
        "{HEAD} use \"std/share\"; workload {{ {HEAD_WORKLOAD} session {{ turn; end; \n}} }}\nserver {{ {body}\n}} "
    ))
}

/// A job over several stages names how their flows divide them, and a job
/// on one stage names nothing: a policy divides what is held at once.
#[test]
fn a_run_over_several_stages_names_its_share() {
    let e = server("run A, B (cost(A, B, 1));");
    assert!(
        e.contains("`run A, B` holds several stages at once: name how their flows divide them"),
        "{e}"
    );
    let e = server("transfer on A, B (1) from kvP to kvD (1);");
    assert!(e.contains("`transfer on A, B` holds several stages"), "{e}");
    let e = server("run A (cost(A, 1), share = share.maxmin);");
    assert!(e.contains("`run A` holds one stage"), "{e}");
}

/// The policies are std/share's: read only with the import, and only the
/// ones it has (`Share::ALL`).
#[test]
fn a_policy_is_std_shares() {
    let e = err(&format!(
        "{HEAD} workload {{ {HEAD_WORKLOAD} session {{ turn; end; \n}} }}\nserver {{ run A, B (cost(A, B, 1), share = share.maxmin);\n}} "
    ));
    assert!(e.contains("`share.…` needs `use \"std/share\";`"), "{e}");
    let e = server("run A, B (cost(A, B, 1), share = share.fair);");
    assert!(
        e.contains("std/share has no policy `fair`; it has `share.maxmin` or `share.bottleneck`"),
        "{e}"
    );
    let e = server("run A, B (cost(A, B, 1), share = maxmin);");
    assert!(e.contains("a sharing policy is std/share's"), "{e}");
    let e = err(&format!(
        "{HEAD} use \"std/share\"; stage share : ps(1); workload {{ {HEAD_WORKLOAD} session {{ turn; end; \n}} }}\nserver {{ run A (cost(A, 1));\n}} "
    ));
    assert!(
        e.contains("`share` names the imported std/share module"),
        "{e}"
    );
}

#[test]
fn a_shared_stage_is_ps_of_a_constant() {
    let e = err(&format!(
        "{HEAD} use \"std/share\"; workload {{ {HEAD_WORKLOAD} session {{ turn; end; \n}} }}\nserver {{ run A, C (cost(A, C, 1), share = share.maxmin);\n}} "
    ));
    assert!(e.contains("stage `C`"), "{e}");
    assert!(e.contains("constant"), "{e}");
    let e = err(
        "stage A : ps(1); stage N : ps(min(present, 4)); use \"std/share\";
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { run A, N (cost(A, N, 1), share = share.maxmin);
        } ",
    );
    assert!(e.contains("stage `N`"), "{e}");
}

/// An index is known only when the run starts, so `E[0], E[1]` could be
/// one stage twice: one array appears once in a run.
#[test]
fn a_run_names_each_stage_array_once() {
    let e = err(&format!(
        "{HEAD} use \"std/share\"; workload {{ {HEAD_WORKLOAD} session {{ turn; end; \n}} }}\nserver {{ run E[0], E[1] (cost(E, E, 1), share = share.maxmin);\n}} "
    ));
    assert!(e.contains("named twice"), "{e}");
}

/// `transfer on a, b (w, share = share.maxmin) from P to Q (n)` is the kernel's
/// `run a, b (w, share = share.maxmin); load Q (n); release P;`.
#[test]
fn transfer_on_several_stages_is_sugar() {
    let head = "pool p { cap 10; } pool q { cap 10; }
                stage a : ps(1); stage b : ps(1); use \"std/share\";
                ";
    let head_workload = "arrive batch(1);";
    let ir = |body: &str| {
        let src = format!(
            "{head} workload {{ {head_workload} session {{ turn; end; \n}} }}\nserver {{ hold q (cost(q, 1)) {{ hold p (cost(p, 1)) {{ {body} }} }}\n}} "
        );
        serq::compile_source(&common::main_source(&src), &common::horizon(5.0))
            .unwrap()
            .to_json()
    };
    assert_eq!(
        ir("transfer on a, b (1, share = share.maxmin) from p to q (1);"),
        ir("run a, b (cost(a, b, 1), share = share.maxmin); load q (cost(q, 1)); release p;")
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
        use \"std/share\";
        workload { arrive poisson(6.5);
          session { turn;
            end;

          }
        }
        server {
          choose i in 3 by (~uniform(0, 1));
          choose j in 4 by (~uniform(0, 1));
          run E[i], F, I[j] (cost(E, F, I, ~exp(0.3)), share = share.maxmin);
          observe left = work(F);
        }
        ";
    let a = run(
        src,
        &Overrides {
            warmup: Some(20.0),
            seed: Some(3),
            ..common::horizon(200.0)
        },
    )
    .json();
    for _ in 0..4 {
        assert_eq!(
            run(
                src,
                &Overrides {
                    warmup: Some(20.0),
                    seed: Some(3),
                    ..common::horizon(200.0)
                }
            )
            .json(),
            a
        );
    }
}

#[test]
fn a_shared_stage_has_a_capacity_above_zero() {
    let e = err("stage A : ps(1); stage Z : ps(0); use \"std/share\";
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { run A, Z (cost(A, Z, 1), share = share.maxmin);
        } ");
    assert!(e.contains("above 0"), "{e}");
}

/// The program's one `share` is gone: the old spelling says where the
/// policy is written now.
#[test]
fn the_programs_share_says_where_it_went() {
    let e = err("stage A : ps(1); stage B : ps(1);
        share maxmin;
        workload { arrive batch(1); session { turn; end; } }
        server { run A, B (cost(A, B, 1));
        } ");
    assert!(
        e.contains("`share maxmin;` is std/share's now, and each relation's"),
        "{e}"
    );
}

/// Two fabrics, two policies: `A`, `B` under max-min and `C`, `D` under
/// bottleneck, the flows of `three` on each. Each fabric's flows end as
/// `three` gives them under its policy alone (f3 at 2/3, g3 at 1): the
/// fabrics share no stage, so neither policy reads the other's flows.
#[test]
fn each_fabric_has_its_own_policy() {
    let r = run(
        "stage A : ps(1); stage B : ps(2); stage C : ps(1); stage D : ps(2);
        use \"std/share\";
        workload { arrive batch(6);
          session { turn;
            end;

          }
        }
        server {
          branch (serial == 0) { run A, B (cost(A, B, 1), share = share.maxmin); observe f1 = now; }
          branch (serial == 1) { run A (cost(A, 1)); observe f2 = now; }
          branch (serial == 2) { run B (cost(B, 1)); observe f3 = now; }
          branch (serial == 3) { run C, D (cost(C, D, 1), share = share.bottleneck); observe g1 = now; }
          branch (serial == 4) { run C (cost(C, 1)); observe g2 = now; }
          branch (serial == 5) { run D (cost(D, 1)); observe g3 = now; }
        }
        ",
        &common::horizon(5.0),
    );
    for (name, want) in [
        ("f1", 2.0),
        ("f2", 2.0),
        ("f3", 2.0 / 3.0),
        ("g1", 2.0),
        ("g2", 2.0),
        ("g3", 1.0),
    ] {
        close(&at(&r, name), &[want]);
    }
}

/// Stages runs join, however indirectly, are one fabric, solved together:
/// `A`-`B` and `B`-`C` join all three, and one fabric has one policy.
#[test]
fn a_fabric_has_one_policy() {
    let e = server(
        "branch (serial == 0) { run A, B (cost(A, B, 1), share = share.maxmin); }
         else { run B, E[0] (cost(B, E, 1), share = share.bottleneck); }",
    );
    assert!(
        e.contains(
            "runs join the stages `A`, `B`, `E`, so their flows are solved together, and the runs name `share.maxmin` and `share.bottleneck`: one fabric has one policy"
        ),
        "{e}"
    );
}
