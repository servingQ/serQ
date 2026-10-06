//! `granule g` on a step stage: a prefill gets the whole of what it has
//! left or a multiple of `g` (TensorRT-LLM schedules a context whole when
//! chunking is off, and rounds a chunk down to a block when it is on;
//! `microBatchScheduler.cpp` L228-L263 at bf414e37).

mod common;

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
    let r = run_source(
        &common::main_source(&prog(granule)),
        &Overrides::default(),
        None,
    )
    .unwrap();
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
        let e = compile_source(&common::main_source(&prog(g)), &Overrides::default()).unwrap_err();
        assert!(e.contains("above 0"), "{g}: {e}");
    }
}

/// A granule the stage could never give does not link: beside `exclusive
/// prefill` a refused prefill blocks every decode for ever (the review of
/// #373), and above a constant `chunk` a prompt longer than it
/// never gets a token (TensorRT-LLM refuses a chunk below its unit).
#[test]
fn a_granule_that_could_never_be_given_does_not_link() {
    for (opts, message) in [
        ("serve exclusive prefill; granule inf;", "exclusive prefill"),
        ("chunk 4; granule inf;", "chunk"),
        ("chunk 3; granule 4;", "chunk"),
    ] {
        let src = prog(opts);
        let e = compile_source(&common::main_source(&src), &Overrides::default()).unwrap_err();
        assert!(e.contains(message), "{opts}: {e}");
    }
    // above the budget is allowed: a prompt that fits the budget is run
    // whole, and a longer one is the workload's (it waits, `idle:`)
    assert!(
        compile_source(
            &common::main_source(&prog("granule 16;")),
            &Overrides::default()
        )
        .is_ok()
    );
}

/// The chunk caps first and the granule rounds what it leaves: a prefill of
/// 10 under `chunk 6; granule 4;` gets 4 (6 rounded down), then the 6 left,
/// whole. Its first token is at the second iteration's end.
#[test]
fn the_chunk_caps_and_the_granule_rounds() {
    let src = r#"
        pool reqs { cap 4; admit via engine; }
        stage engine : step { budget 8; chunk 6; granule 4; cost 1; }
        workload { arrive batch(1); }
        session {
          set t0 = now;
          hold reqs (1) { prefill on engine (10); observe ttft = now - t0; }
          end;
        }
        run { horizon 20; warmup 0; seed 1; }
        "#;
    let r = run_source(&common::main_source(src), &Overrides::default(), None).unwrap();
    // 4 at the first, 6 at the second (the whole remainder, not capped
    // below the chunk): done at 2
    assert_eq!(r.observe("ttft").unwrap().samples, [2.0]);
}

/// A prefill the granule refuses ends the iteration's admissions, as
/// TensorRT-LLM's scan stops at the first context that does not fit. Five
/// six-token prompts on a budget of eight, each allocating the chunk the
/// budget leaves: the first is admitted and served 6, the second admitted
/// and refused the 2 left, and no third follows in that iteration (before,
/// all five were admitted at 0, four holding a chunk they were refused).
#[test]
fn a_refused_prefill_ends_the_admissions() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        pool kv { cap 100; }
        stage engine : step { budget 8; granule inf; cost 1; memory kv; }
        workload { arrive batch(5); }
        session {
          hold reqs (1), kv (min(6, left)) at admission (left = budget_left(engine)) {
            observe admitted = now;
            prefill on engine (6) growing kv;
          }
          end;
        }
        run { horizon 20; warmup 0; seed 1; }
        "#;
    let r = run_source(&common::main_source(src), &Overrides::default(), None).unwrap();
    let admitted = &r.observe("admitted").unwrap().samples;
    assert_eq!(
        admitted.iter().filter(|&&t| t == 0.0).count(),
        2,
        "{admitted:?}\n{}",
        r.text()
    );
}
