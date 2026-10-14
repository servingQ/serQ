mod common;
use serq::Overrides;

#[test]
fn deterministic_renewal_arrivals_follow_the_supplied_gap() {
    let src = r#"
        stage svc : fifo;
        server {
          set t0 = now;
          run svc (cost(svc, ~det(1)));
          observe response = now - t0;
        }

"#;
    let workload = r#"
        workload { arrive renewal(2);
          session { turn;
            end;

          }
        }
"#;
    let report = common::run(
        &common::main_source(src),
        workload,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(7),
            ..common::horizon(10.0)
        },
    )
    .unwrap();
    assert_eq!(report.arrivals, 5);
    let response = report.observe("response").unwrap();
    assert_eq!(response.samples, vec![1.0; 4]);
}

#[test]
fn hyperexponential_renewal_arrivals_have_the_configured_mean_rate() {
    let src = r#"
        stage svc : fifo;
        server { run svc (cost(svc, 0));
        }

"#;
    let workload = r#"
        workload { arrive renewal(~h2(1, 4));
          session { turn; end;
          }
        }
"#;
    let report = common::run(
        &common::main_source(src),
        workload,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(19),
            ..common::horizon(20_000.0)
        },
    )
    .unwrap();
    let rate = report.arrivals as f64 / 20_000.0;
    assert!((rate - 1.0).abs() < 0.06, "observed arrival rate {rate}");
}

#[test]
fn open_arrival_limit_drains_within_the_horizon() {
    let src = r#"
        stage svc : fifo;
        server {
          run svc (cost(svc, ~det(2)));
          observe service = 2;
        }

"#;
    let workload = r#"
        workload { arrive poisson(1000);
          session { turn;
            end;

          }
        }
"#;
    let report = common::run(
        &common::main_source(src),
        workload,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(7),
            arrivals: Some(2),
            ..common::horizon(10.0)
        },
    )
    .unwrap();
    assert_eq!(report.arrivals, 2);
    assert_eq!(report.ended, 2);
    // FIFO jobs take two seconds each; the first Poisson arrival is at zero.
    assert_eq!(report.end, 4.0);
    assert_eq!(report.horizon, 10.0);
    assert_eq!(report.stages[0].throughput, 0.5);
    let json: serde_json::Value = serde_json::from_str(&report.json()).unwrap();
    assert_eq!(json["horizon"], 10.0);
    assert_eq!(json["end"], 4.0);
    assert_eq!(report.observe("service").unwrap().samples.len(), 2);
}

#[test]
fn finite_arrivals_reject_incomplete_runs_and_empty_measurement_intervals() {
    for (session, run, expected) in [
        (
            "loop { run svc (cost(svc, 1)); }",
            "horizon 10; arrivals 3;",
            "failed to drain",
        ),
        (
            "run svc (cost(svc, 1)); end;",
            "horizon 10; arrivals 1000;",
            "requested arrivals",
        ),
        (
            "run svc (cost(svc, 1)); end;",
            "horizon 1000; warmup 500; arrivals 5;",
            "before or at warmup",
        ),
        (
            "run svc (cost(svc, 1)); end;",
            "horizon 20; warmup 3; arrivals 1;",
            "before or at warmup",
        ),
    ] {
        let workload = format!(
            "stage svc : fifo; workload {{ arrive renewal(2); session {{ turn; {session} }} }}"
        );
        let error = {
            let mut options = common::horizon(10.0);
            options.instance(&format!("run {{ {run} }}")).unwrap();
            common::run(&common::main_source("server {}"), &workload, &options).unwrap_err()
        };
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn finite_arrivals_can_finish_exactly_at_the_deadline() {
    // Arrivals at 2, 4, 6 and one second of service finish at 3, 5, 7.
    let src = "stage svc : fifo; server { run svc (cost(svc, 1));\n} ";
    let workload = "workload { arrive renewal(2); \n  session { turn; end; \n  }\n}";
    let report = common::run(
        &common::main_source(src),
        workload,
        &Overrides {
            arrivals: Some(3),
            ..common::horizon(7.0)
        },
    )
    .unwrap();
    assert_eq!(report.arrivals, 3);
    assert_eq!(report.ended, 3);
    assert_eq!(report.end, 7.0);
}

#[test]
fn poisson_retains_its_initial_arrival_and_renewal_waits_for_a_gap() {
    let run = |arrival: &str| {
        common::run(
            &common::main_source("server { observe arrival = now;\n} "),
            &format!("workload {{ arrive {arrival}; \n  session {{ turn; end; \n  }}\n}}"),
            &Overrides {
                seed: Some(1),
                ..common::horizon(10.0)
            },
        )
        .unwrap()
    };
    let poisson = run("poisson(2)");
    let renewal = run("renewal(~exp(0.5))");
    let poisson_times = &poisson.observe("arrival").unwrap().samples;
    let renewal_times = &renewal.observe("arrival").unwrap().samples;
    assert_eq!(poisson_times[0], 0.0);
    assert_eq!(&poisson_times[1..], renewal_times);
}

#[test]
fn renewal_validation_and_ir_version_prevent_ambiguous_inputs() {
    for gap in ["now", "serial"] {
        let workload = format!("workload {{ arrive renewal({gap}); }}");
        assert!(
            common::compile(
                &common::main_source("server {} "),
                &workload,
                &common::horizon(10.0)
            )
            .is_err()
        );
    }
    let mut program = common::compile(
        &common::main_source("server {} "),
        "workload { arrive renewal(2); }",
        &common::horizon(10.0),
    )
    .unwrap();
    program.version = 5;
    assert!(
        program
            .validate()
            .unwrap_err()
            .contains(&format!("this interpreter reads {}", serq::ir::IR_VERSION))
    );
}

/// A gap that is not a positive time: a constant one does not link, and a
/// drawn one is a program error in the run. Both used to panic the
/// interpreter (#269).
#[test]
fn a_renewal_gap_must_be_positive() {
    let src = common::main_source(
        "stage svc : delay;
        server { run svc (cost(svc, 1));
        }
        ",
    );
    let workload = |gap: &str| {
        format!(
            "let g = 0.5;
        workload {{ arrive renewal({gap});
          session {{ turn; end;
          }}
        }}"
        )
    };
    let ov = common::horizon(10.0);
    for gap in ["0", "-1", "g - 2 * g"] {
        let e = common::compile(&src, &workload(gap), &ov).unwrap_err();
        assert!(
            e.contains("an interarrival time must be positive"),
            "{gap}: {e}"
        );
    }
    let e = common::run(&src, &workload("~uniform(-1, 1)"), &ov).unwrap_err();
    assert!(
        e.contains("`arrive renewal(~uniform(-1, 1))`: the interarrival time is -"),
        "{e}"
    );
    // IR that bypasses the text: a literal gap is the IR's to refuse
    let mut p = common::compile(&src, &workload("2"), &ov).unwrap();
    p.arrival = serq::ir::CArrival::Renewal(serq::ir::CExpr::Num(0.0));
    let e = p.validate().unwrap_err();
    assert!(e.contains("an interarrival time must be positive"), "{e}");
}

/// A poisson rate that is not a positive number draws gaps that are not
/// positive times: `poisson(-1)` ran backwards and never ended (#286).
#[test]
fn a_poisson_rate_must_be_positive() {
    for (rate, said) in [
        ("-1", "the rate is -1;"),
        ("0", "the rate is 0;"),
        ("inf", "the rate is inf;"),
        ("lam - 1", "the rate is 0;"),
        ("0/0", "the poisson rate is NaN"),
    ] {
        let src = "stage svc : delay;
        server { run svc (cost(svc, 1));
        }
        ";
        let workload = format!(
            "let lam = 1;
        workload {{ arrive poisson({rate});
          session {{ turn; end;
          }}
        }}"
        );
        let e = common::compile(&common::main_source(src), &workload, &common::horizon(10.0))
            .unwrap_err();
        assert!(e.contains(said), "{rate}: {e}");
    }
}

/// A count is a whole number in range: a cast made `closed(-1)` no sessions,
/// `batch(0.5)` none, `arrivals 2.5` two, and `batch(1e30)` a run that never
/// started (#289).
#[test]
fn a_count_is_a_whole_number_in_range() {
    for (workload, said) in [
        ("closed(-1)", "the closed population is -1"),
        ("closed(0)", "the closed population is 0"),
        ("closed(2.5)", "the closed population is 2.5"),
        ("batch(0)", "the batch size is 0"),
        ("batch(0.5)", "the batch size is 0.5"),
        (
            "batch(1e30)",
            "the batch size is 1000000000000000000000000000000",
        ),
    ] {
        let src = "stage svc : delay;
             server { run svc (cost(svc, 1)); }";
        let w = format!("workload {{ arrive {workload}; session {{ turn; end; }} }}");
        let e = common::compile(&common::main_source(src), &w, &common::horizon(10.0)).unwrap_err();
        assert!(
            e.contains(said) && e.contains("a count is a whole number"),
            "{workload}: {e}"
        );
    }
    // Invocation integer settings are typed in Rust/Python and checked when
    // parsing an external instance. Preserve the former in-model bad values.
    for (settings, said) in [
        ("seed -1;", "run option `seed` is not a number"),
        ("seed 2.5;", "seed 2.5 is not an unsigned integer"),
        ("arrivals 1e30;", "is not a positive integer"),
        ("arrivals 2.5;", "arrivals 2.5 is not a positive integer"),
        ("arrivals -1;", "run option `arrivals` is not a number"),
    ] {
        let e = Overrides::default()
            .instance(&format!("run {{ {settings} }}"))
            .unwrap_err();
        assert!(e.contains(said), "{settings}: {e}");
    }
    // IR that bypasses the text meets the same bound
    let mut p = common::compile(
        &common::main_source(
            "stage svc : delay;
        server { run svc (cost(svc, 1));
        } ",
        ),
        "workload { arrive batch(1);
          session { turn; end;
          }
        }",
        &common::horizon(10.0),
    )
    .unwrap();
    for n in [0, serq::ir::MAX_SESSIONS + 1] {
        p.arrival = serq::ir::CArrival::Batch(n);
        let e = p.validate().unwrap_err();
        assert!(
            e.contains("a closed population or a batch is from 1 to"),
            "{n}: {e}"
        );
    }
}
