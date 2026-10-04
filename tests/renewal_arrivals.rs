use serq::{Overrides, run_source};

#[test]
fn deterministic_renewal_arrivals_follow_the_supplied_gap() {
    let src = r#"
        stage svc : fifo;
        workload { arrive renewal(2); }
        session {
          set t0 = now;
          run svc (~det(1));
          observe response = now - t0;
          end;
        }
        run { horizon 10; warmup 0; seed 7; }
    "#;
    let report = run_source(src, &Overrides::default(), None).unwrap();
    assert_eq!(report.arrivals, 5);
    let response = report.observe("response").unwrap();
    assert_eq!(response.samples, vec![1.0; 4]);
}

#[test]
fn hyperexponential_renewal_arrivals_have_the_configured_mean_rate() {
    let src = r#"
        stage svc : fifo;
        workload { arrive renewal(~h2(1, 4)); }
        session { run svc (0); end; }
        run { horizon 20_000; warmup 0; seed 19; }
    "#;
    let report = run_source(src, &Overrides::default(), None).unwrap();
    let rate = report.arrivals as f64 / 20_000.0;
    assert!((rate - 1.0).abs() < 0.06, "observed arrival rate {rate}");
}

#[test]
fn open_arrival_limit_drains_within_the_horizon() {
    let src = r#"
        stage svc : fifo;
        workload { arrive poisson(1000); }
        session {
          run svc (~det(2));
          observe service = 2;
          end;
        }
        run { horizon 10; arrivals 2; warmup 0; seed 7; }
    "#;
    let report = run_source(src, &Overrides::default(), None).unwrap();
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
            "loop { run svc (1); }",
            "horizon 10; arrivals 3;",
            "failed to drain",
        ),
        (
            "run svc (1); end;",
            "horizon 10; arrivals 1000;",
            "requested arrivals",
        ),
        (
            "run svc (1); end;",
            "horizon 1000; warmup 500; arrivals 5;",
            "before or at warmup",
        ),
        (
            "run svc (1); end;",
            "horizon 20; warmup 3; arrivals 1;",
            "before or at warmup",
        ),
    ] {
        let src = format!(
            "stage svc : fifo; workload {{ arrive renewal(2); }} session {{ {session} }} run {{ {run} }}"
        );
        let error = run_source(&src, &Overrides::default(), None).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn finite_arrivals_can_finish_exactly_at_the_deadline() {
    // Arrivals at 2, 4, 6 and one second of service finish at 3, 5, 7.
    let src = "stage svc : fifo; workload { arrive renewal(2); } session { run svc (1); end; } run { horizon 7; arrivals 3; }";
    let report = run_source(src, &Overrides::default(), None).unwrap();
    assert_eq!(report.arrivals, 3);
    assert_eq!(report.ended, 3);
    assert_eq!(report.end, 7.0);
}

#[test]
fn poisson_retains_its_initial_arrival_and_renewal_waits_for_a_gap() {
    let run = |arrival: &str| {
        run_source(
            &format!("workload {{ arrive {arrival}; }} session {{ observe arrival = now; end; }} run {{ horizon 10; seed 1; }}"),
            &Overrides::default(), None,
        ).unwrap()
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
        let src = format!("workload {{ arrive renewal({gap}); }} run {{ horizon 10; }}");
        assert!(serq::compile_source(&src, &Overrides::default()).is_err());
    }
    let mut program = serq::compile_source(
        "workload { arrive renewal(2); } run { horizon 10; arrivals 3; }",
        &Overrides::default(),
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
    let src = |gap: &str| {
        format!(
            "let g = 0.5;
             stage svc : delay;
             workload {{ arrive renewal({gap}); }}
             session {{ run svc (1); end; }}
             run {{ horizon 10; }}"
        )
    };
    for gap in ["0", "-1", "g - 2 * g"] {
        let e = serq::compile_source(&src(gap), &Overrides::default()).unwrap_err();
        assert!(
            e.contains("an interarrival time must be positive"),
            "{gap}: {e}"
        );
    }
    let e = run_source(&src("~uniform(-1, 1)"), &Overrides::default(), None).unwrap_err();
    assert!(
        e.contains("`arrive renewal(~uniform(-1, 1))`: the interarrival time is -"),
        "{e}"
    );
    // IR that bypasses the text: a literal gap is the IR's to refuse
    let mut p = serq::compile_source(&src("2"), &Overrides::default()).unwrap();
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
        let src = format!(
            "let lam = 1; stage svc : delay;
             workload {{ arrive poisson({rate}); }}
             session {{ run svc (1); end; }}
             run {{ horizon 10; }}"
        );
        let e = serq::compile_source(&src, &Overrides::default()).unwrap_err();
        assert!(e.contains(said), "{rate}: {e}");
    }
}
