//! `granule g` on a step stage: a prefill gets the whole of what it has
//! left or a multiple of `g` (TensorRT-LLM schedules a context whole when
//! chunking is off, and rounds a chunk down to a block when it is on;
//! `microBatchScheduler.cpp` L228-L263 at bf414e37).

use serq::{Overrides, compile_source, run_source};

/// Two six-token prompts on a budget of eight; an iteration costs 1, and 1
/// more per token above 6. Any amount: the first iteration serves 6 + 2
/// (cost 3), the second the other 4 (cost 1): first tokens at 3 and 4.
/// `granule inf` or `granule 4`: 2 is not the whole and no multiple of 4,
/// so the first iteration serves 6 alone (cost 1), the second the other 6
/// (cost 1): first tokens at 1 and 2.
fn prog(granule: &str) -> String {
    format!(
        r#"
        pool reqs {{ cap 4; admit via engine; }}
        stage engine : step {{ budget 8; cost 1 + max(0, tokens - 6); {granule} }}
        workload {{ arrive batch(2); }}
        session {{
          set t0 = now;
          hold reqs (1) {{
            prefill on engine (6);
            observe ttft = now - t0;
          }}
          end;
        }}
        run {{ horizon 20; warmup 0; seed 1; }}
        "#
    )
}

fn ttft(granule: &str) -> Vec<f64> {
    let r = run_source(&prog(granule), &Overrides::default(), None).unwrap();
    r.observe("ttft").unwrap().samples.clone()
}

#[test]
fn a_prefill_takes_its_whole_remainder_or_a_multiple_of_the_granule() {
    assert_eq!(ttft(""), [3.0, 4.0]);
    assert_eq!(ttft("granule 1;"), [3.0, 4.0]);
    assert_eq!(ttft("granule inf;"), [1.0, 2.0]);
    assert_eq!(ttft("granule 4;"), [1.0, 2.0]);
    // a multiple of 2 fits the 2 left
    assert_eq!(ttft("granule 2;"), [3.0, 4.0]);
}

#[test]
fn a_granule_is_above_zero() {
    for g in ["granule 0;", "granule -1;"] {
        let e = compile_source(&prog(g), &Overrides::default()).unwrap_err();
        assert!(e.contains("above 0"), "{g}: {e}");
    }
}
