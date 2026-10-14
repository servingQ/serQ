//! A run's `latency` (IR 14): the wait before its work, at its stages and
//! taking none of their capacity. The expected values are derived by hand
//! from the semantics (`docs/ir.md`, `Run`): the stage report counts the
//! wait as the run's `wait`, the work as its `service`, and the job as
//! present during both.

mod common;

use serq::{Overrides, compile_source, run_source};

fn run(src: &str) -> serq::Report {
    run_source(
        &common::main_source(src),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(100.0)
        },
        None,
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Two jobs of work 1 on a `ps(1)` stage, started together, one with a
/// wait of 0.5: B alone runs at rate 1 for 0.5 s (0.5 done), then both at
/// 1/2, so B ends at 0.5 + 0.5/0.5 = 1.5 and A, with 0.5 left when B
/// ends, at 2.0. The stage saw two runs: waits 0.5 and 0 (mean 0.25),
/// services 1.5 and 1.5. The same on two shared stages under max-min,
/// whose rates are the same numbers: the waiting flow takes no share.
#[test]
fn a_waiting_job_takes_no_share_until_its_wait_is_over() {
    for (stages, run_a, run_b) in [
        (
            "stage nic : ps(1);",
            "run nic (cost(nic, 1), latency = 0.5);",
            "run nic (cost(nic, 1));",
        ),
        (
            "stage a : ps(1); stage b : ps(1); use \"std/share\";",
            "run a, b (cost(a, b, 1), share = share.maxmin, latency = 0.5);",
            "run a, b (cost(a, b, 1), share = share.maxmin);",
        ),
    ] {
        let src = format!(
            "{stages}
            workload {{ arrive batch(1); session {{ turn; end; }} }}
            server {{
              fork {{ {run_b} observe b_done = now; }}
              {run_a}
              observe a_done = now;
              join;
            }}"
        );
        let r = run(&src);
        assert_eq!(samples(&r, "b_done"), [1.5], "{}", r.text());
        assert_eq!(samples(&r, "a_done"), [2.0], "{}", r.text());
        let s = r.stages.iter().find(|s| s.completed > 0).unwrap();
        assert_eq!(s.completed, 2, "{}", r.text());
        assert!(close(s.mean_wait, 0.25), "{}", r.text());
        assert!(close(s.mean_service, 1.5), "{}", r.text());
        // present for 0..2 (A) and 0..1.5 (B): 3.5 job-seconds over the
        // measured 100 s
        assert!(close(s.mean_number, 3.5 / 100.0), "{}", r.text());
    }
}

/// On a `fifo(1)` stage the wait comes before the queue: B takes the server
/// at 0 and ends at 1; A's wait ends at 0.5, it queues, starts at 1 and
/// ends at 2. Waits 1.0 (its latency and then the queue) and 0, services 1
/// and 1.
#[test]
fn a_fifo_job_queues_after_its_wait() {
    let r = run("stage f : fifo;
        workload { arrive batch(1); session { turn; end; } }
        server {
          fork { run f (cost(f, 1)); observe b_done = now; }
          run f (cost(f, 1), latency = 0.5);
          observe a_done = now;
          join;
        }");
    assert_eq!(samples(&r, "b_done"), [1.0], "{}", r.text());
    assert_eq!(samples(&r, "a_done"), [2.0], "{}", r.text());
    let s = r.stage("f").unwrap();
    assert_eq!(s.completed, 2);
    assert!(close(s.mean_wait, 0.5), "{}", r.text());
    assert!(close(s.mean_service, 1.0), "{}", r.text());
}

/// A run of no work and a wait (a copy of zero bytes after its latency) is
/// a run served for nothing: done once the wait is over, counted with the
/// wait as its `wait` and 0 as its `service`, on one stage and on two (PR
/// #612's review found the wait landing in `service` on one).
#[test]
fn a_run_of_nothing_after_a_wait_is_a_wait_not_a_service() {
    for (stages, stmt) in [
        (
            "stage nic : ps(1);",
            "run nic (cost(nic, 0), latency = 0.5);",
        ),
        ("stage f : fifo;", "run f (cost(f, 0), latency = 0.5);"),
        ("stage d : delay;", "run d (cost(d, 0), latency = 0.5);"),
        (
            "stage a : ps(1); stage b : ps(1); use \"std/share\";",
            "run a, b (cost(a, b, 0), share = share.maxmin, latency = 0.5);",
        ),
    ] {
        let src = format!(
            "{stages}
            workload {{ arrive batch(1); session {{ turn; end; }} }}
            server {{ {stmt} observe done = now; }}"
        );
        let r = run(&src);
        assert_eq!(samples(&r, "done"), [0.5], "{}", r.text());
        for s in r.stages.iter().filter(|s| s.completed > 0) {
            assert_eq!(s.completed, 1, "{}", r.text());
            assert!(close(s.mean_wait, 0.5), "{}", r.text());
            assert_eq!(s.mean_service, 0.0, "{}", r.text());
        }
        assert!(r.stages.iter().any(|s| s.completed > 0), "{}", r.text());
    }
}

/// The clause is the kernel's spelling of the wait: `transfer on L (n,
/// latency = x)` and a link's `serve … latency x` give one IR, and writing
/// both is refused.
#[test]
fn a_transfer_latency_is_the_links_or_its_own_not_both() {
    let program = |serve: &str, clause: &str| {
        format!(
            "pool kvP {{ cap 100; }} pool kvD {{ cap 100; }}
            queue L : link {{ serve ps(100){serve}; }}
            workload {{ arrive batch(1); session {{ turn; end; }} }}
            server {{
              hold kvP (cost(kvP, 10)) {{ }} lease kvP (inf);
              hold kvD (cost(kvD, 10)) {{ transfer on L (10{clause}) from kvP to kvD (10); }}
            }}"
        )
    };
    let ov = common::horizon(10.0);
    let on_link = compile_source(&common::main_source(&program(" latency 0.5", "")), &ov).unwrap();
    let on_run =
        compile_source(&common::main_source(&program("", ", latency = 0.5")), &ov).unwrap();
    assert_eq!(on_link.to_json(), on_run.to_json());
    assert!(on_link.blocks.iter().flatten().any(|st| matches!(
        st,
        serq::ir::CStmt::Run {
            latency: Some(serq::ir::CExpr::Num(x)),
            ..
        } if *x == 0.5
    )));
    let e = compile_source(
        &common::main_source(&program(" latency 0.5", ", latency = 0.5")),
        &ov,
    )
    .expect_err("refused");
    assert!(e.contains("the wait is written once"), "{e}");
}
