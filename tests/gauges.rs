//! `gauge NAME = e;`: the time average of a function of the deployment's
//! state, and `max j in n (e)` / `min` / `sum`, which the linker writes out.

mod common;

use serq::Overrides;

const DEPLOYMENT: &str = "
        pool kv[2] { cap 100; }
        stage svc[2] : fifo;
        server {
          choose j in 2 by (holders(kv[j]));
          set n = ~uniform(1, 20);
          hold kv[j] (cost(kv, n)) { run svc[j] (cost(svc, ~exp(0.5))); }
        }

";

/// The sessions `DEPLOYMENT` serves.
const WORKLOAD: &str = "
        workload { arrive poisson(1.5);
          session { turn;
            end;

          }
        }
";

/// One session of one turn, the workload of the programs that are not
/// `DEPLOYMENT`.
const ONE: &str = "workload { session { turn; end; } }";

fn options() -> Overrides {
    Overrides {
        warmup: Some(100.0),
        seed: Some(3),
        ..common::horizon(4000.0)
    }
}

fn run(gauges: &str) -> serq::Report {
    common::run(
        &common::main_source(&format!("{DEPLOYMENT}{gauges}")),
        WORKLOAD,
        &options(),
    )
    .unwrap()
}

fn link_error(src: &str, workload: &str) -> String {
    common::compile(&common::main_source(src), workload, &options())
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
        let p = common::compile(
            &common::main_source(&format!("{DEPLOYMENT}{g}")),
            WORKLOAD,
            &options(),
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
    // Keep the left fold in the IR: balancing sums changes floating-point
    // rounding, and reordering terms can change which draw an operand reads.
    for (aggregate, expanded) in [
        ("sum", "(0 + 1) + 2"),
        ("min", "min(min(0, 1), 2)"),
        ("max", "max(max(0, 1), 2)"),
    ] {
        assert_eq!(
            ir(&format!("gauge x = {aggregate} k in 3 (k);")),
            ir(&format!("gauge x = {expanded};")),
        );
    }
    // The same order applies during constant folding: (1e16 - 1e16) + 1
    // is 1, whereas 1e16 + (-1e16 + 1) rounds to 0.
    assert_eq!(
        ir("let N = sum k in 3 (k == 0 ? 1e16 : k == 1 ? -1e16 : 1); gauge x = N;"),
        ir("gauge x = 1;"),
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
        server {{ run s[0] (cost(s, N));
        }}
        "
        );
        let workload = "workload { arrive batch(1);
          session { turn; end;
          }
        }";
        let p = common::compile(&common::main_source(&src), workload, &common::horizon(10.0))
            .unwrap_or_else(|e| panic!("{n}: {e}"));
        assert_eq!(p.stages.len(), size, "{n}");
    }
    // nested aggregates share the program's budget of terms, in the parser
    // as in the linker: the parser folded 64³ terms, and a nest of three
    // counts of 4096 ran for minutes before the linker could say so
    let e = link_error(
        "let N = sum i in 64 (sum j in 64 (sum k in 64 (0))) + 1;
        server {
        } ",
        ONE,
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
        let e = link_error(&format!("{DEPLOYMENT}{gauge}"), WORKLOAD);
        assert!(e.contains(want), "{gauge}: {e}");
    }
}

#[test]
fn a_gauge_has_a_name_of_its_own() {
    let e = link_error(&format!("{DEPLOYMENT} gauge x = 1; gauge x = 2;"), WORKLOAD);
    assert!(e.contains("declared twice"), "{e}");
    // the text refuses the gauge as it refuses any two declarations of one
    // name (#486), and IR that bypasses the text is refused too
    // (the observation was the session's; the server's is in the same file
    // as the gauge, and declared before it)
    let src = DEPLOYMENT.replace(
        "(cost(svc, ~exp(0.5))); }",
        "(cost(svc, ~exp(0.5))); }\n          observe x = now;",
    );
    let e = link_error(&format!("{src} gauge x = 1;"), WORKLOAD);
    assert!(
        e.contains("`x` is an observation at") && e.contains("a gauge named `x`"),
        "{e}"
    );
    let mut p = common::compile(
        &common::main_source(&format!("{src} gauge y = 1;")),
        WORKLOAD,
        &common::horizon(10.0),
    )
    .unwrap();
    p.gauges[0].name = "x".into();
    let e = p.validate().unwrap_err();
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
        let e = link_error(&format!("{DEPLOYMENT}{gauge}"), WORKLOAD);
        assert!(e.contains(want), "{gauge}: {e}");
    }
}

/// An array size is folded by the parser, which knows only the `let`s above
/// it, and reaches the linker as a number; the linker asks its aggregates'
/// indices of the whole program, in the words it has for the same
/// aggregate in an expression (#499, #561). Until #561 a name the parser
/// did not know (a later `let`, an attribute, a pool, a stage) linked. Each
/// row sizes `DEPLOYMENT`'s own families, which it uses, at 2: with an
/// index of its own the program links (the control below).
#[test]
fn an_array_sizes_aggregate_index_is_its_own() {
    let svc = "stage svc[2] : fifo;";
    for (from, to, want) in [
        (
            svc,
            "let N = 4; stage svc[sum N in 2 (1)] : fifo;",
            "`sum N in`: `N` is also a `let` constant",
        ),
        // a `let` declared after the array
        (
            svc,
            "stage svc[sum N in 2 (1)] : fifo; let N = 4;",
            "`sum N in`: `N` is also a `let` constant",
        ),
        // `j` is the session's `choose`
        (
            svc,
            "stage svc[sum j in 2 (1)] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
        (
            svc,
            "stage svc[sum kv in 2 (1)] : fifo;",
            "`sum kv in`: `kv` is also a pool",
        ),
        // a pool family's size
        (
            "pool kv[2]",
            "pool kv[max svc in 2 (2)]",
            "`max svc in`: `svc` is also a stage",
        ),
        // an aggregate in the body of one with an index of its own
        (
            svc,
            "stage svc[sum k in 2 (sum j in 1 (1))] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
        // the branch the size does not take is the program's too
        (
            svc,
            "stage svc[1 ? 2 : sum j in 2 (1)] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
        // and so it is in a `let`, which the linker folds (#561 review)
        (
            svc,
            "let M = 1 ? 2 : sum j in 2 (1); stage svc[M] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
        (
            svc,
            "let N = 4; let M = sum N in 2 (1); stage svc[2] : fifo;",
            "`sum N in`: `N` is also a `let` constant",
        ),
        // an argument a `def` does not read is not evaluated either, and is
        // the program's too (#753 review): written, as a reference's index,
        // inside another unread argument, in a branch not taken
        (
            svc,
            "def one(x) { 2 } stage svc[one(sum svc in 2 (1))] : fifo;",
            "`sum svc in`: `svc` is also a stage",
        ),
        (
            svc,
            "def one(x) { 2 } let M = 1 ? 2 : one(kv[sum j in 2 (0)]); stage svc[M] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
        (
            svc,
            "def one(x) { 2 } stage svc[one(one(sum j in 2 (1)))] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
        (
            svc,
            "def one(x) { 2 } let M = 1 ? 2 : one(sum j in 2 (1)); stage svc[M] : fifo;",
            "`sum j in`: `j` is also a session attribute",
        ),
    ] {
        let src = DEPLOYMENT.replace(from, to);
        let e = link_error(&src, WORKLOAD);
        assert!(
            e.contains(want) && e.contains("give the index a name of its own"),
            "{to}: {e}"
        );
    }
    // an index of its own is no clash
    for to in [
        "stage svc[sum k in 2 (1)] : fifo;",
        "stage svc[1 ? 2 : sum k in 2 (1)] : fifo;",
        "let M = 1 ? 2 : sum k in 2 (1); stage svc[M] : fifo;",
        "def one(x) { 2 } stage svc[one(sum k in 2 (1))] : fifo;",
    ] {
        let own = DEPLOYMENT.replace(svc, to);
        assert!(
            common::compile(&common::main_source(&own), WORKLOAD, &options()).is_ok(),
            "{own}"
        );
    }
}

/// Aggregate expansion must reach the index diagnostic on a bounded stack.
/// 2100 units cannot index a two-member pool; min/max likewise produce 2.
#[test]
fn large_aggregate_indices_report_errors_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            for expression in [
                "sum k in 2100 (1)",
                "sum k in 4096 (1)",
                "min k in 4096 (2)",
                "max k in 4096 (2)",
            ] {
                let e = link_error(
                    &format!("{DEPLOYMENT} gauge x = used(kv[{expression}]);"),
                    WORKLOAD,
                );
                assert!(e.contains("out of range"), "{expression}: {e}");
            }
        })
        .unwrap()
        .join()
        .unwrap();
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
    let e = link_error(
        &format!("{DEPLOYMENT} gauge x = used(kv[holders(kv[0]) + 1]);"),
        WORKLOAD,
    );
    assert!(e.contains("index is a number in range"), "{e}");
    // a constant expression is folded to one
    let p = common::compile(
        &common::main_source(&format!(
            "{DEPLOYMENT} let N = 2; gauge x = used(kv[N - 1]);"
        )),
        WORKLOAD,
        &options(),
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
        server {
          run gate (cost(gate, serial == 0 ? 0 : 1));
          hold kv (cost(kv, 1)) { run gate (cost(gate, 1)); }
        }
        gauge n = holders(kv);
        ";
    let workload = "
        workload { arrive batch(2);
          session { turn;
            end;

          }
        }";
    let ov = common::horizon(3.0);
    let r = common::run(&common::main_source(src), workload, &ov).unwrap();
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
    let src = "device gpu { }
        engine e on gpu {
          tokens cap ~uniform(1, 2);
          scheduler { advance running; admit waiting while (running.preempted == 0); }
          worker { forward (1); }
        }
        server { e (1, 1);
        }
        gauge g = budget_left(e);";
    assert!(link_error(src, ONE).contains("may not read `budget_left"));
}

/// Nor does a step's budget: `budget_left` plans from the budget, so a
/// budget that reads it, its own engine's or another's, recursed until the
/// stack overflowed (#284).
#[test]
fn a_budget_does_not_read_budget_left() {
    for budget in ["128 + budget_left(e)", "128 + budget_left(f)"] {
        let src = format!(
            "device gpu {{ }}
        device tpu {{ }}
        engine e on gpu {{
          tokens cap {budget};
          scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
          worker {{ forward (1); }}
        }}
        engine f on tpu {{
          tokens cap 64;
          scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
          worker {{ forward (1); }}
        }}
        server {{ e (1, 1);
        }} "
        );
        let e = link_error(&src, ONE);
        assert!(
            e.contains("an engine's `tokens cap` or `each at most` may not read `budget_left"),
            "{budget}: {e}"
        );
    }
    // nor a chunk: an engine's `each at most` chooses among constants or a
    // `max` above one, and the condition that chooses may read the iteration
    let src = "device gpu { }
        engine e on gpu {
          tokens cap 128;
          scheduler {
            let c = budget_left(e) > 1 ? 4 : 8;
            advance running each at most (c);
            admit waiting while (running.preempted == 0) each at most (c);
          }
          worker { forward (1); }
        }
        server { e (1, 1);
        } ";
    let e = link_error(src, ONE);
    assert!(
        e.contains("an engine's `tokens cap` or `each at most` may not read `budget_left"),
        "{e}"
    );
    // the rule is the Budget moment's: a cost, a serve key and a hold's
    // header read it and the run ends
    let src = "device gpu { kv cap 64; }
        engine e on gpu {
          tokens cap 8;
          scheduler { advance running by (budget_left(e)); admit waiting while (running.preempted == 0); }
          worker { forward (1 + 0 * budget_left(e)); }
        }
        pool kv on gpu { }
        server { hold kv (cost(kv, min(8, budget_left(e)))) { e (4, 1); }
        }
        ";
    let workload = "workload { arrive batch(2);
          session { turn; end;
          }
        }";
    let ov = common::horizon(1.0);
    common::run(&common::main_source(src), workload, &ov).unwrap();
}

/// Inside a queue's entry an aggregate's index is the aggregate's, not a
/// session attribute the entry would read from outside, also where a bare
/// `k` argument parses as a reference.
#[test]
fn an_aggregate_in_a_queue_entry_reads_its_own_index() {
    let src = "queue engine : prefill {
          serve fifo;
          prefill (prompt) { run (cost(engine, sum k in 2 (max(k, 1)) * prompt)); }
        }
        queue gw : gateway { route { engine.prefill (1); } }
        server { gw.route(); }
        ";
    let workload = "workload { arrive batch(1); session { turn; end; } }";
    let ov = common::horizon(10.0);
    let r = common::run(&common::main_source(src), workload, &ov).unwrap();
    assert!((r.stage("engine").unwrap().mean_service - 2.0).abs() < 1e-12);
}
