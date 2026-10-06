//! `gauge NAME = e;`: the time average of a function of the deployment's
//! state, and `max j in n (e)` / `min` / `sum`, which the linker writes out.

mod common;

use serq::{Overrides, compile_source, run_source};

const DEPLOYMENT: &str = "
        pool kv[2] { cap 100; }
        stage svc[2] : fifo;
        workload { arrive poisson(1.5);
          session { turn;
            end;

          }
        }
        server {
          choose j in 2 by (holders(kv[j]));
          set n = ~uniform(1, 20);
          hold kv[j] (n) { run svc[j] (~exp(0.5)); }
        }

";

fn run(gauges: &str) -> serq::Report {
    run_source(
        &common::main_source(&format!("{DEPLOYMENT}{gauges}")),
        &Overrides {
            warmup: Some(100.0),
            seed: Some(3),
            ..common::horizon(4000.0)
        },
        None,
    )
    .unwrap()
}

fn link_error(src: &str) -> String {
    compile_source(
        &common::main_source(src),
        &Overrides {
            warmup: Some(100.0),
            seed: Some(3),
            ..common::horizon(4000.0)
        },
    )
    .unwrap_err()
    .to_string()
}

/// A gauge of a pool's `used` is the pool's time-average `used`: the two
/// integrate the same piecewise-constant signal over the same span.
#[test]
fn a_gauge_of_used_is_the_pools_time_average() {
    let r = run("gauge u0 = used(kv[0]); gauge h1 = holders(kv[1]);");
    let g = r.gauge("u0").unwrap();
    let pool = &r.pools[0];
    assert!(
        (g.mean - pool.mean_used).abs() <= 1e-9 * pool.mean_used,
        "{} vs {}",
        g.mean,
        pool.mean_used
    );
    assert!(g.ci.lo() <= g.mean && g.mean <= g.ci.hi());
    let h = r.gauge("h1").unwrap();
    assert!((h.mean - r.pools[1].mean_holders).abs() <= 1e-9 * r.pools[1].mean_holders);
    assert_eq!(h.min, 0.0);
    assert!(h.max >= 1.0 && h.max.fract() == 0.0);
}

/// The time average of a maximum is not the maximum of the time averages:
/// what the instances hold at the same moment is what a gauge sees.
#[test]
fn the_spread_is_read_at_one_moment() {
    let r = run("
        gauge hot = max k in 2 (used(kv[k]));
        gauge total = sum k in 2 (used(kv[k]));
        gauge spread = max k in 2 (used(kv[k])) - min k in 2 (used(kv[k]));
    ");
    let (a, b) = (r.pools[0].mean_used, r.pools[1].mean_used);
    let hot = r.gauge("hot").unwrap().mean;
    let total = r.gauge("total").unwrap().mean;
    assert!((total - (a + b)).abs() <= 1e-9 * (a + b));
    assert!(
        hot > a.max(b),
        "E[max] {hot} should exceed max E {}",
        a.max(b)
    );
    assert!(r.gauge("spread").unwrap().mean > (a - b).abs());
}

/// `max k in 2 (e)` is `max(e[k := 0], e[k := 1])`: the IR is the same.
#[test]
fn an_aggregate_is_written_out() {
    let ir = |g: &str| {
        let p = compile_source(
            &common::main_source(&format!("{DEPLOYMENT}{g}")),
            &Overrides {
                warmup: Some(100.0),
                seed: Some(3),
                ..common::horizon(4000.0)
            },
        )
        .unwrap();
        serde_json::to_value(&p.gauges).unwrap()
    };
    assert_eq!(
        ir("let N = 2; gauge x = max k in N (used(kv[k]) + k);"),
        ir("gauge x = max(used(kv[0]) + 0, used(kv[1]) + 1);")
    );
    assert_eq!(
        ir("gauge x = sum k in 2 (holders(kv[k]));"),
        ir("gauge x = holders(kv[0]) + holders(kv[1]);")
    );
}

/// A `let` the linker folds is a constant everywhere, an array size
/// included: the parser, which sizes arrays, folds an aggregate as the
/// linker writes it out (#274).
#[test]
fn an_aggregate_let_sizes_an_array() {
    for (n, size) in [
        ("sum j in 2 (1)", 2),
        ("max j in 3 (j)", 2),
        ("min j in 2 (3 - j)", 2),
    ] {
        let src = format!(
            "let N = {n};
        stage s[N] : delay;
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ run s[0] (N);
        }}
        "
        );
        let p = compile_source(&common::main_source(&src), &common::horizon(10.0))
            .unwrap_or_else(|e| panic!("{n}: {e}"));
        assert_eq!(p.stages.len(), size, "{n}");
    }
    // nested aggregates share the program's budget of terms, in the parser
    // as in the linker: the parser folded 64³ terms, and a nest of three
    // counts of 4096 ran for minutes before the linker could say so
    let e = link_error(
        "let N = sum i in 64 (sum j in 64 (sum k in 64 (0))) + 1;
        workload { session { turn; end;
        } }
        server {
        } ",
    );
    assert!(e.contains("terms, at most 4096"), "{e}");
}

#[test]
fn a_gauge_reads_only_the_deployment() {
    for (gauge, want) in [
        ("gauge x = n;", "a gauge has no session"),
        ("gauge x = ~exp(1);", "a gauge may not draw"),
        ("gauge x = cachedin(kv[0]);", "a gauge has no session"),
        ("gauge x = tokens;", "exists only in"),
        ("gauge x = now;", "may not read `now`"),
        ("gauge x = work(svc[0]);", "may not read `work"),
    ] {
        let e = link_error(&format!("{DEPLOYMENT}{gauge}"));
        assert!(e.contains(want), "{gauge}: {e}");
    }
}

#[test]
fn a_gauge_has_a_name_of_its_own() {
    let e = link_error(&format!("{DEPLOYMENT} gauge x = 1; gauge x = 2;"));
    assert!(e.contains("declared twice"), "{e}");
    let src = DEPLOYMENT.replace("end;", "observe x = now; end;");
    let e = link_error(&format!("{src} gauge x = 1;"));
    assert!(e.contains("also an `observe`"), "{e}");
}

#[test]
fn an_aggregates_index_and_count_are_its_own() {
    for (gauge, want) in [
        // `j` is the session's `choose`
        (
            "gauge x = max j in 2 (used(kv[j]));",
            "is also a session attribute",
        ),
        (
            "let k = 1; gauge x = max k in 2 (used(kv[k]));",
            "is also a `let` constant",
        ),
        (
            "let x = sum k in 2 (k); let k = 1; gauge y = x;",
            "is also a `let` constant",
        ),
        (
            "let k = sum k in 2 (k); gauge y = k;",
            "is also a `let` constant",
        ),
        ("gauge x = max kv in 2 (1);", "is also a pool"),
        // whatever the order the names are declared in
        ("let x = sum svc in 2 (1); gauge y = x;", "is also a stage"),
        ("gauge x = sum k in 1.5 (1);", "must be a positive integer"),
        ("gauge x = sum k in 3 (used(kv[k]));", "out of range"),
        ("gauge x = sum k in 1e20 (1);", "at most 4096"),
        ("gauge x = sum k in 64 (sum i in 65 (1));", "at most 4096"),
        ("gauge x = used(kv[-1]);", "out of range"),
        ("let N = 2; gauge x = used(kv[N + 1 - 1]);", "out of range"),
        // a folded call is a number too
        (
            "pool b { cap 1; block 4; } gauge x = used(kv[blocksize(b)]);",
            "out of range",
        ),
        // the fold of the index spends no budget the linking spent already
        ("gauge x = used(kv[sum k in 2100 (1)]);", "out of range"),
    ] {
        let e = link_error(&format!("{DEPLOYMENT}{gauge}"));
        assert!(e.contains(want), "{gauge}: {e}");
    }
}

/// `--dump` writes a gauge's change points; their integral is the mean.
#[test]
fn the_dump_is_the_signal() {
    let r = run("gauge u = used(kv[0]);");
    let dir = std::env::temp_dir().join(format!("serq-gauge-{}", std::process::id()));
    r.dump(&dir).unwrap();
    let csv = std::fs::read_to_string(dir.join("gauge/u.csv")).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let pts: Vec<(f64, f64)> = csv
        .lines()
        .skip(1)
        .map(|l| {
            let (t, v) = l.split_once(',').unwrap();
            (t.parse().unwrap(), v.parse().unwrap())
        })
        .collect();
    assert!(pts.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 != w[1].1));
    let (from, to) = (r.warmup, r.end);
    let mut area = 0.0;
    for (k, &(t, v)) in pts.iter().enumerate() {
        let next = pts.get(k + 1).map_or(to, |p| p.0);
        let (a, b) = (t.max(from), next.min(to));
        if b > a {
            area += v * (b - a);
        }
    }
    let mean = area / (to - from);
    assert!((mean - r.gauge("u").unwrap().mean).abs() <= 1e-9 * mean);
}

/// A gauge's index is a number: one read from the state could leave the
/// array at run time, and a gauge must not be able to fail the run.
#[test]
fn a_gauges_index_is_a_number() {
    let e = link_error(&format!(
        "{DEPLOYMENT} gauge x = used(kv[holders(kv[0]) + 1]);"
    ));
    assert!(e.contains("index is a number in range"), "{e}");
    // a constant expression is folded to one
    let p = compile_source(
        &common::main_source(&format!(
            "{DEPLOYMENT} let N = 2; gauge x = used(kv[N - 1]);"
        )),
        &Overrides {
            warmup: Some(100.0),
            seed: Some(3),
            ..common::horizon(4000.0)
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&p.gauges[0].expr).unwrap()["Call"][1][0]["Pool"]["index"],
        serde_json::json!({"Num": 1.0})
    );
}

/// The gauges read the state an instant ends with: a session that takes
/// the pool and one that releases it at the same time leave one holder, and
/// the two the instant passes through are never a value.
#[test]
fn a_gauge_reads_the_end_of_an_instant() {
    let src = "
        pool kv { cap 10; }
        stage gate : delay;
        workload { arrive batch(2);
          session { turn;
            end;

          }
        }
        server {
          run gate (serial == 0 ? 0 : 1);
          hold kv (1) { run gate (1); }
        }
        gauge n = holders(kv);
        ";
    let r = run_source(&common::main_source(src), &common::horizon(3.0), None).unwrap();
    // [0, 1): session 0 holds; at 1 it releases and session 1 takes, and
    // in between the instant has two holders, which no gauge reads
    let n = r.gauge("n").unwrap();
    assert_eq!(n.max, 1.0);
    assert!(n.points.iter().all(|p| p.1 <= 1.0), "{:?}", n.points);
}

/// A gauge plans no iteration: `budget_left` would evaluate the budget,
/// which may draw, and the run would change.
#[test]
fn a_gauge_does_not_plan_an_iteration() {
    let src = "stage e : step { budget ~uniform(1, 2); cost 1; }
        workload { session { turn; end;
        } }
        server { run e prefill (1);
        }
        gauge g = budget_left(e);";
    assert!(link_error(src).contains("may not read `budget_left"));
}

/// Nor does a step's budget: `budget_left` plans from the budget, so a
/// budget that reads it, its own engine's or another's, recursed until the
/// stack overflowed (#284).
#[test]
fn a_budget_does_not_read_budget_left() {
    for (budget, chunk) in [
        ("128 + budget_left(e)", "0"),
        ("128", "budget_left(e)"),
        ("128 + budget_left(f)", "0"),
    ] {
        let src = format!(
            "stage e : step {{ budget {budget}; chunk {chunk}; cost 1; }}
        stage f : step {{ budget 64; cost 1; }}
        workload {{ session {{ turn; end;
        }} }}
        server {{ run e prefill (1);
        }} "
        );
        let e = link_error(&src);
        assert!(
            e.contains("a step's budget or chunk may not read `budget_left"),
            "{budget}; {chunk}: {e}"
        );
    }
    // the rule is the Budget moment's: a cost, a serve key and a hold's
    // header read it and the run ends
    let src = "pool kv { cap 64; }
        stage e : step { budget 8; cost 1 + 0 * budget_left(e); serve by (budget_left(e)); memory kv; }
        workload { arrive batch(2);
          session { turn; end;
          }
        }
        server { hold kv (min(8, budget_left(e))) { run e prefill (4); }
        }
        ";
    run_source(&common::main_source(src), &common::horizon(1.0), None).unwrap();
}

/// Inside a queue's entry an aggregate's index is the aggregate's, not a
/// session attribute the entry would read from outside, also where a bare
/// `k` argument parses as a reference.
#[test]
fn an_aggregate_in_a_queue_entry_reads_its_own_index() {
    let src = "queue engine : prefill {
          serve fifo;
          prefill (prompt) { run (sum k in 2 (max(k, 1)) * prompt); }
        }
        queue gw : gateway { route { engine.prefill (1); } }
        workload { arrive batch(1); session { turn; end; } } server { gw.route(); }
        ";
    let r = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap();
    assert!((r.stage("engine").unwrap().mean_service - 2.0).abs() < 1e-12);
}
