//! `queue … first fit` (#65, `docs/design/first-fit.md`): a waiting hold
//! that does not fit every pool it names is passed over, and the next in
//! the queue's order that fits is admitted. Without it the first that does
//! not fit stops the admission (head-of-line blocking, vLLM's `if
//! new_blocks is None: break`, scheduler.py:1228-1235). The pool's own
//! admission, an engine's iteration and a tick alike (`tests/queue_ticks.rs`
//! for the tick).

mod common;

/// `n` sessions arrive at t = 0, each one turn.
fn sessions(n: usize) -> String {
    format!("workload {{ arrive batch({n}); session {{ turn; }} }}\n")
}

fn run(n: usize, src: &str, horizon: f64) -> serq::Report {
    common::run(
        &common::main_source(src),
        &sessions(n),
        &common::horizon(horizon),
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name)
        .unwrap_or_else(|| panic!("no `{name}`: {}", r.text()))
        .samples
        .clone()
}

/// Serials 0, 1 and 2 ask 3, 2 and 1 of 4 units at t = 0; serial 0 runs
/// 10 s, the others 1 s. Serial 1 does not fit the 1 unit serial 0 leaves.
/// Behind it, serial 2 waits to t = 10; under `first fit` it goes in at
/// t = 0, and serial 1 still at t = 10.
#[test]
fn a_waiter_that_does_not_fit_is_passed_over() {
    let prog = |queue: &str| {
        format!(
            r#"
        stage svc : fifo(4);
        pool mem {{ cap 4; {queue} }}
        server {{
          hold mem (cost(mem, 3 - serial)) {{
            observe who = serial;
            observe at = now;
            run svc (cost(svc, serial == 0 ? 10 : 1));
          }}
        }}
"#
        )
    };
    let blocked = run(3, &prog("queue fifo;"), 50.0);
    assert_eq!(samples(&blocked, "who"), [0.0, 1.0, 2.0]);
    assert_eq!(samples(&blocked, "at"), [0.0, 10.0, 10.0]);
    let passed = run(3, &prog("queue fifo first fit;"), 50.0);
    assert_eq!(samples(&passed, "who"), [0.0, 2.0, 1.0]);
    assert_eq!(samples(&passed, "at"), [0.0, 0.0, 10.0]);
}

/// The queue's keys order the waiters `first fit` tries, read again at
/// each attempt. Serial 3 takes 3 of 4 units for 10 s. In the order of
/// `-serial`, serial 2 asks 2 and is passed over, serial 1 asks 1 and goes
/// in; at t = 1, when serial 1 leaves, serial 0 goes in before serial 2,
/// which waits for serial 3. Without `first fit` serial 2 waits for
/// serial 3 and the others behind it.
#[test]
fn first_fit_tries_the_waiters_in_the_keys_order() {
    let prog = |fit: &str| {
        format!(
            r#"
        stage svc : fifo(4);
        pool mem {{ cap 4; queue by (-serial){fit}; }}
        server {{
          hold mem (cost(mem, serial == 3 ? 3 : (serial == 2 ? 2 : 1))) {{
            observe who = serial;
            observe at = now;
            run svc (cost(svc, serial == 3 ? 10 : 1));
          }}
        }}
"#
        )
    };
    let passed = run(4, &prog(" first fit"), 50.0);
    assert_eq!(samples(&passed, "who"), [3.0, 1.0, 0.0, 2.0]);
    assert_eq!(samples(&passed, "at"), [0.0, 0.0, 1.0, 10.0]);
    let blocked = run(4, &prog(""), 50.0);
    assert_eq!(samples(&blocked, "who"), [3.0, 2.0, 1.0, 0.0]);
    assert_eq!(samples(&blocked, "at"), [0.0, 10.0, 10.0, 10.0]);
}

/// An engine's iteration admits the first waiter that fits too. Of 64 KV
/// units, serial 0 takes 41 for 10 tokens; serial 1 asks 41 and does not
/// fit until serial 0 ends; serial 2 asks 9. Behind serial 1 it waits to
/// t = 10, admitted with it; under `first fit` it goes in at t = 0 and
/// ends at t = 1.
#[test]
fn an_engine_admits_the_first_that_fits() {
    let prog = |queue: &str| {
        format!(
            r#"
        device gpu {{ kv cap 64; }}
        engine llm on gpu {{
          tokens cap 128;
          scheduler {{ advance running; admit waiting taking kv (request.input + 1); }}
          worker {{ forward (1); }}
        }}
        pool kv on llm.gpu {{ {queue} }}
        server {{
          llm (serial == 2 ? 8 : 40, serial == 0 ? 10 : 1);
          observe admitted_at = llm.admitted;
          observe done = now;
        }}
"#
        )
    };
    let blocked = run(3, &prog("queue fifo;"), 50.0);
    assert_eq!(samples(&blocked, "admitted_at"), [0.0, 10.0, 10.0]);
    assert_eq!(samples(&blocked, "done"), [10.0, 11.0, 11.0]);
    let passed = run(3, &prog("queue fifo first fit;"), 50.0);
    assert_eq!(samples(&passed, "admitted_at"), [0.0, 0.0, 10.0]);
    assert_eq!(samples(&passed, "done"), [1.0, 10.0, 11.0]);
}

/// An engine that serves several queues tries them in the order their
/// pools are declared, and the first head that does not fit stops the
/// iteration's admissions (`tests/pd_semantics.rs`,
/// `an_engine_serves_its_queues_in_declaration_order`). A queue that says
/// `first fit` none of whose waiters fits is passed over for the next:
/// session 1 waits for `a` until t = 50, and session 2, on `b`, goes in at
/// t = 2 as it queues, where behind `a`'s head it waits to t = 50.
#[test]
fn an_engine_passes_over_a_first_fit_queue_none_of_whose_waiters_fits() {
    let program = |a: &str| {
        format!(
            r#"
        queue llm : agg {{
          device gpu {{ }}
          engine on gpu {{
            a cap 1;
            b cap 1;
            tokens cap 1000;
            scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
            worker {{ forward (1); }}
          }}
          pool a on llm {{ {a} }}
          pool b on llm {{ }}
          decode (s) {{
            if (s == 0) {{ hold a (cost(a, 1)) {{ run llm decode (cost(llm, 50)); }} }}
            if (s == 1) {{ hold a (cost(a, 1)) {{ observe a_admitted = now; run llm decode (cost(llm, 1)); }} }}
            if (s == 2) {{ hold b (cost(b, 1)) {{ observe b_admitted = now; run llm decode (cost(llm, 1)); }} }}
          }}
        }}
        stage gate : delay;
        server {{
          run gate (cost(gate, serial));
          llm.decode (serial);
        }}
"#
        )
    };
    let blocked = run(3, &program(""), 200.0);
    assert_eq!(
        samples(&blocked, "b_admitted"),
        [50.0],
        "{}",
        blocked.text()
    );
    let passed = run(3, &program("queue fifo first fit;"), 200.0);
    assert_eq!(samples(&passed, "b_admitted"), [2.0], "{}", passed.text());
    assert_eq!(samples(&passed, "a_admitted"), [50.0], "{}", passed.text());
}

/// A preempted hold goes back to the head of its queue; under `first fit`
/// it is passed over like any other waiter that does not fit. Serials 0
/// and 1 take 4 and 5 of 10 units at t = 0; at t = 2 serial 0's growth by
/// 4 preempts serial 1 (`preempt lifo`), which asks 5 of the 2 left.
/// Serial 2 asks 2 at t = 3, which fits: behind serial 1 it waits for
/// serial 0 to end at t = 12; under `first fit` it goes in at t = 3.
#[test]
fn a_resumed_hold_that_does_not_fit_is_passed_over() {
    let prog = |queue: &str| {
        format!(
            r#"
        pool mem {{ cap 10; preempt lifo; {queue} }}
        stage d : delay;
        server {{
          if (serial == 2) {{ run d (cost(d, 3)); }}
          hold mem (cost(mem, serial == 0 ? 4 : (serial == 1 ? 5 : 2))) {{
            observe who = serial;
            observe at = now;
            if (serial == 0) {{ run d (cost(d, 2)); grow mem (cost(mem, 4)); run d (cost(d, 10)); }}
            else {{ run d (cost(d, serial == 1 ? 5 : 1)); }}
          }}
        }}
"#
        )
    };
    let blocked = run(3, &prog("queue fifo;"), 50.0);
    assert_eq!(blocked.pool("mem").unwrap().preemptions, 1);
    assert_eq!(samples(&blocked, "who"), [0.0, 1.0, 1.0, 2.0]);
    assert_eq!(samples(&blocked, "at"), [0.0, 0.0, 12.0, 12.0]);
    let passed = run(3, &prog("queue fifo first fit;"), 50.0);
    assert_eq!(passed.pool("mem").unwrap().preemptions, 1);
    assert_eq!(samples(&passed, "who"), [0.0, 1.0, 2.0, 1.0]);
    assert_eq!(samples(&passed, "at"), [0.0, 0.0, 3.0, 12.0]);
}

/// A waiter whose units read the deployment's state is not rejected when
/// it joins (#364). Behind a head, under head-of-line blocking, it waits
/// where the head is seen to; under `first fit` the others pass it and it
/// would wait to the end unseen, so the report names it. Serial 0 holds 2
/// of 4 units past the run's end; serial 1, the head, asks 3; serial 2
/// asks 5.
#[test]
fn a_waiter_over_the_cap_behind_the_head_is_named() {
    let prog = |queue: &str| {
        format!(
            r#"
        stage svc : fifo(4);
        pool mem {{ cap 4; {queue} }}
        server {{
          hold mem (cost(mem, serial == 0 ? 2 : (serial == 1 ? 3 : 5 + 0 * holders(mem)))) {{
            run svc (cost(svc, 100));
          }}
        }}
"#
        )
    };
    let passed = run(3, &prog("queue fifo first fit;"), 50.0);
    assert_eq!(
        passed.pool("mem").unwrap().over_cap,
        Some(("mem".to_string(), 5.0)),
        "{}",
        passed.text()
    );
    assert!(
        passed
            .text()
            .contains("over: a hold waiting in pool `mem`'s queue asks `mem` for 5"),
        "{}",
        passed.text()
    );
    // the head asks 3, under the cap: it is what the others wait behind
    let blocked = run(3, &prog("queue fifo;"), 50.0);
    assert!(
        blocked.pool("mem").unwrap().over_cap.is_none(),
        "{}",
        blocked.text()
    );
}

/// `first fit` is the tutorial's (`docs/tutorial/programs/02-first-fit.sq`),
/// which a queue key could imitate for that one pool: the waiters whose
/// items fit the free units first. The two run alike, report for report;
/// the key restates the hold's units, and would be wrong under `block`,
/// `reserve`, a cached prefix or a hold of several pools.
#[test]
fn the_tutorial_runs_as_the_key_that_imitates_it() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
    let program = read("docs/tutorial/programs/02-first-fit.sq");
    let workload = read("docs/tutorial/programs/workloads/02-memory.sq");
    let keyed = program.replace(
        "queue fifo first fit;",
        "queue by (items > free(mem) ? 1 : 0);",
    );
    assert_ne!(program, keyed);
    let mut ov = serq::Overrides {
        warmup: Some(500.0),
        seed: Some(1),
        ..common::horizon(5000.0)
    };
    ov.set("C", "6").unwrap();
    let report = |src: &str| common::run(src, &workload, &ov).unwrap();
    let (fit, key) = (report(&program), report(&keyed));
    assert_eq!(fit.json(), key.json());
    // and the clause changes the run it is written in
    let blocked = report(&read("docs/tutorial/programs/02-memory.sq"));
    assert!(
        fit.pool("mem").unwrap().mean_wait < blocked.pool("mem").unwrap().mean_wait,
        "{} against {}",
        fit.pool("mem").unwrap().mean_wait,
        blocked.pool("mem").unwrap().mean_wait
    );
}

/// `first fit` says which waiter is admitted, the tick's too, so it comes
/// before `every`; `first` alone is not a clause.
#[test]
fn first_fit_is_written_before_every() {
    let refused = |queue: &str, needle: &str| {
        let src = format!(
            "stage svc : fifo(1);
        pool mem {{ cap 4; {queue} }}
        server {{ hold mem (cost(mem, 1)) {{ run svc (cost(svc, 1)); }} }}"
        );
        let e = common::compile(
            &common::main_source(&src),
            &sessions(1),
            &common::horizon(10.0),
        )
        .unwrap_err();
        assert!(e.contains(needle), "{queue}: {e}");
    };
    refused(
        "queue fifo every (5) first fit;",
        "so it comes with the order, before `every`: `queue … first fit every (dt);`",
    );
    refused("queue fifo first;", "expected `fit`");
    // the forms that link, each with the field set
    for queue in [
        "queue fifo first fit;",
        "queue by (-serial) first fit;",
        "queue fifo first fit every (5);",
    ] {
        let src = format!(
            "stage svc : fifo(1);
        pool mem {{ cap 4; {queue} }}
        server {{ hold mem (cost(mem, 1)) {{ run svc (cost(svc, 1)); }} }}"
        );
        let p = common::compile(
            &common::main_source(&src),
            &sessions(1),
            &common::horizon(10.0),
        )
        .unwrap_or_else(|e| panic!("{queue}: {e}"));
        assert!(p.pools[0].first_fit, "{queue}");
    }
}

/// `fit` was a word #152 retired, `admit if … fit`. It comes back with one
/// meaning, a queue's: as a name or a hold's condition it is still refused,
/// and the message says where the word lives now.
#[test]
fn the_retired_fit_says_where_the_word_lives_now() {
    for (src, at) in [
        (
            "let fit = 1; stage svc : fifo(1);
        server { run svc (cost(svc, fit)); }",
            "a name",
        ),
        (
            "stage svc : fifo(1); pool mem { cap 4; }
        server { hold mem (cost(mem, 1)) fit { run svc (cost(svc, 1)); } }",
            "a hold's condition",
        ),
    ] {
        let e = common::compile(
            &common::main_source(src),
            &sessions(1),
            &common::horizon(10.0),
        )
        .unwrap_err();
        assert!(
            e.contains("`fit` as a condition is gone")
                && e.contains("the word is a queue's now, `queue … first fit`"),
            "{at}: {e}"
        );
    }
}
