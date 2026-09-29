use seq::{Overrides, run_source};

#[test]
fn deterministic_renewal_arrivals_follow_the_supplied_gap() {
    let src = r#"
        stage svc : fifo;
        workload { arrive renewal(~det(2)); }
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
fn open_arrival_limit_drains_the_last_sessions_past_the_horizon() {
    let src = r#"
        stage svc : fifo;
        workload { arrive poisson(1000); }
        session {
          run svc (~det(2));
          observe service = 2;
          end;
        }
        run { horizon 1; arrivals 2; warmup 0; seed 7; }
    "#;
    let report = run_source(src, &Overrides::default(), None).unwrap();
    assert_eq!(report.arrivals, 2);
    assert_eq!(report.ended, 2);
    assert!(report.horizon > 1.0, "drained through {}", report.horizon);
    assert_eq!(report.observe("service").unwrap().samples.len(), 2);
}
