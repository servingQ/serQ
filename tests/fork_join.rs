//! `fork { … }` and `join;`: a leg of the request runs beside the session
//! (vLLM's push proxy sends the prefill and the decode request at once),
//! and the session waits for its legs where it joins.

mod common;

use serq::{Overrides, check_source, run_source};

fn run(src: &str) -> serq::Report {
    run_source(&common::main_source(src), &Overrides::default(), None)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn fails(src: &str) -> String {
    match run_source(&common::main_source(src), &Overrides::default(), None) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("ran:\n{src}"),
    }
}

fn refused(src: &str, needle: &str) {
    let e = match check_source(&common::main_source(src), &Overrides::default()) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted:\n{src}"),
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

/// The leg runs 5 s while the session runs 3: the session passes the
/// `join` at 5, when the leg ends, not at 8.
#[test]
fn a_leg_runs_beside_the_session_and_join_waits_for_it() {
    let r = run(r#"
        stage svc : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          fork { run svc (5); observe leg_done = now; }
          run svc (3);
          observe before_join = now;
          join;
          observe after_join = now;
        }
        run { horizon 100; }
"#);
    assert_eq!(samples(&r, "leg_done"), [5.0]);
    assert_eq!(samples(&r, "before_join"), [3.0]);
    assert_eq!(samples(&r, "after_join"), [5.0]);
}

/// A `join` after the legs have ended does not wait.
#[test]
fn a_join_after_the_legs_end_passes() {
    let r = run(r#"
        stage svc : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          fork { run svc (1); }
          run svc (3);
          join;
          observe after_join = now;
        }
        run { horizon 100; }
"#);
    assert_eq!(samples(&r, "after_join"), [3.0]);
}

/// A leg reads the session's attributes as they were at the fork, and its
/// `set`s change its own copy: the session does not see them.
#[test]
fn a_leg_sets_its_own_copy() {
    let r = run(r#"
        stage svc : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          set x = 1;
          fork { observe leg_reads = x; set x = 7; run svc (1); observe leg_after = x; }
          set x = 2;
          join;
          observe session_x = x;
        }
        run { horizon 100; }
"#);
    assert_eq!(samples(&r, "leg_reads"), [1.0]);
    assert_eq!(samples(&r, "leg_after"), [7.0]);
    assert_eq!(samples(&r, "session_x"), [2.0]);
}

/// A leg is part of the request, not a session: it is no arrival and no
/// end in the run's counts.
#[test]
fn a_leg_is_not_a_session() {
    let r = run(r#"
        stage svc : delay;
        workload { arrive batch(2);
          session { request;
            end;

          }
        }
        server {
          fork { run svc (1); }
          fork { run svc (2); }
          join;
        }
        run { horizon 100; }
"#);
    assert_eq!(r.arrivals, 2);
    assert_eq!(r.ended, 2);
}

/// What a leg leases passes to its session when the leg ends: the
/// session's `release` takes it, and the pool is free for the next.
#[test]
fn a_legs_lease_passes_to_its_session() {
    let r = run(r#"
        pool kv { cap 10; }
        stage svc : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          fork { hold kv (10) { run svc (1); } lease kv (inf); }
          join;
          observe leased = free(kv);
          run svc (2);
          release kv;
          observe released = free(kv);
        }
        run { horizon 100; }
"#);
    assert_eq!(samples(&r, "leased"), [0.0]);
    assert_eq!(samples(&r, "released"), [10.0]);
}

/// The session's cached prefix is the request's: a leg's hold consumes and
/// keeps it as the session's own would.
#[test]
fn a_leg_shares_the_sessions_cache() {
    let r = run(r#"
        pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          hold kv (8) { run svc (1); } cache (8);
          fork { hold kv (8) { observe hit = cached; run svc (1); } cache (8); }
          join;
        }
        run { horizon 100; }
"#);
    assert_eq!(samples(&r, "hit"), [8.0]);
}

/// The push this was written for: the decoder's blocks are taken while the
/// prefill runs, and the write starts when the prefill ends.
#[test]
fn the_decode_leg_holds_its_blocks_during_the_prefill() {
    let src = |concurrent: u32| {
        format!(
            r#"
        queue gw : gateway {{
          route {{
            set t0 = now;
            branch ({concurrent}) {{ fork {{ P.prefill (prompt); }} }} else {{ P.prefill (prompt); }}
            D.decode (prompt) from P;
            observe parked_at = D.parked - t0;
            observe written_at = D.written - t0;
          }}
        }}
        queue P : prefill {{
          pool kv {{ cap 100; }}
          serve fifo;
          nic ps(10);
          prefill (p) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }}
        }}
        queue D : decode {{
          pool kv {{ cap 100; }}
          serve step {{ cost 1; memory kv; }}
          nic ps(10);
          decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
          decode (p) from src {{
            hold kv (p) {{
              mark parked;
              join;
              transfer (p) from src to kv (p);
              mark written;
            }}
          }}
        }}
        P push D latency 1 share maxmin;
        workload {{ arrive batch(1); init {{ set prompt = 10; }} session {{ request gw; end; }} }}
        run {{ horizon 100; }}
    "#
        )
    };
    // the prefill takes 10 s, the prefiller's wait 1 s, the copy 10 tokens at 10/s
    let r = run(&src(1));
    assert_eq!(samples(&r, "parked_at"), [0.0]);
    assert_eq!(samples(&r, "written_at"), [12.0]);
    let r = run(&src(0));
    assert_eq!(samples(&r, "parked_at"), [10.0]);
    assert_eq!(samples(&r, "written_at"), [12.0]);
}

/// Two requests whose legs wait for each other: A's decode leg holds the
/// decoder's memory waiting for A's prefill, which waits for the
/// prefiller's memory, which B's lease holds until B's decode leg, which
/// waits for the decoder's memory. The run says so instead of ending as if
/// the requests were merely slow.
#[test]
fn legs_that_wait_for_each_other_fail_the_run() {
    let e = fails(
        r#"
        queue gw : gateway {
          route {
            fork { P.prefill (prompt); }
            run gate (serial == 1 ? 1 : 0);
            D.decode (prompt) from P;
          }
        }
        queue P : prefill {
          pool kv { cap 10; }
          serve fifo;
          nic ps(100);
          prefill (p) { hold kv (p) { run (p); } cache (p) lease kv (inf); }
        }
        queue D : decode {
          pool kv { cap 10; }
          serve step { cost 1; memory kv; }
          nic ps(100);
          decode (p) { hold kv (p) { prefill (p) growing kv; } }
          decode (p) from src { hold kv (p) { join; transfer (p) from src to kv (p); } }
        }
        P push D share maxmin;
        stage gate : delay;
        workload {
          arrive batch(2);
          init { set prompt = 8; }
          session { run gate (serial == 0 ? 0.5 : 0); request gw; end; }
        }
        run { horizon 100; }
    "#,
    );
    assert!(e.contains("wait for each other"), "{e}");
    assert!(e.contains("session 0 waits at a `join`"), "{e}");
    assert!(e.contains("a leg of session 0 waits at `P.kv`"), "{e}");
    assert!(e.contains("session 1 waits at `D.kv`"), "{e}");
}

/// A request one of whose holds can never fit is refused whole: a refused
/// leg ends its session, and a leg of a refused session runs on and gives
/// back what it leases.
#[test]
fn a_refused_leg_or_session_ends_the_request() {
    let src = |leg: u32, session: u32| {
        format!(
            r#"
        pool a {{ cap 10; }}
        pool b {{ cap 10; }}
        stage svc : delay;
        workload {{ arrive batch(1);
          session {{ request;
            end;

          }}
        }}
        server {{
          set na = {leg};
          set nb = {session};
          fork {{ hold a (na) {{ run svc (2); }} lease a (inf); }}
          run svc (1);
          hold b (nb) {{ join; release a; }}
          observe done = now;
        }}
        run {{ horizon 100; }}
"#
        )
    };
    let r = run(&src(5, 5));
    assert_eq!(samples(&r, "done"), [2.0]);
    // the leg refused: the session ends at the refusal
    let r = run(&src(50, 5));
    assert_eq!(r.ended, 1);
    assert!(samples(&r, "done").is_empty());
    // the session refused at 1: its leg runs on and ends its lease
    let r = run(&src(5, 50));
    assert_eq!(r.ended, 1);
    assert!(samples(&r, "done").is_empty());
    assert_eq!(r.pool("a").unwrap().admissions, 1);
}

/// A session that ends while a leg it forked runs is an error: the
/// program forgot a `join`.
#[test]
fn a_session_may_not_end_before_its_legs() {
    let e = fails(
        r#"
        stage svc : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { set x = 0; fork { run svc (5); } run svc (1); branch (x) { join; }
        }
        run { horizon 100; }
"#,
    );
    assert!(e.contains("while a leg it forked runs"), "{e}");
}

/// A leg is a part of the request: it may not turn, end, fork or join.
#[test]
fn a_leg_may_only_run_the_request() {
    for (stmt, what) in [
        ("turn;", "`turn`"),
        ("end;", "`end`"),
        ("fork { run svc (1); }", "fork"),
        ("join;", "`join`"),
    ] {
        refused(
            &format!(
                "stage svc : delay;
        workload {{ arrive batch(1); session {{
            fork {{ run svc (1); {stmt} }} join; request; end;
        }} }}
        server {{}}
        run {{ horizon 10; }}"
            ),
            &format!("a `fork`'s leg may not {what}"),
        );
    }
}

#[test]
fn a_join_needs_a_fork() {
    refused(
        "stage svc : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { run svc (1); join;
        }
        run { horizon 10; }",
        "a `join` in a program that forks no leg waits for nothing",
    );
}

/// A leg holds nothing of the session's: `release` in a leg of a pool the
/// session holds around the fork is refused.
#[test]
fn a_leg_does_not_act_on_the_sessions_holds() {
    refused(
        "pool kv { cap 10; }
        stage svc : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { hold kv (1) { fork { grow kv (1); } join; }
        }
        run { horizon 10; }",
        "`grow kv` outside a hold of `kv`",
    );
}

/// A leg's lease caches by the leg's attributes when it ends, after it has
/// passed to the session: `cache (n)` reads the `n` the leg set.
#[test]
fn a_legs_lease_caches_by_the_legs_attributes() {
    let r = run(r#"
        pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          set n = 0;
          fork { set n = 8; hold kv (8) { run svc (1); } cache (n) lease kv (inf); }
          join;
          release kv;
          hold kv (8) { observe hit = cached; } cache (0);
        }
        run { horizon 100; }
"#);
    assert_eq!(samples(&r, "hit"), [8.0]);
}

/// Waiting for a lease that expires is not waiting for each other: the
/// crossing legs of `legs_that_wait_for_each_other_fail_the_run` with a
/// finite lease, at a horizon before it expires, end as a slow run.
#[test]
fn a_lease_that_expires_is_not_a_deadlock() {
    let r = run(r#"
        queue gw : gateway {
          route {
            fork { P.prefill (prompt); }
            run gate (serial == 1 ? 1 : 0);
            D.decode (prompt) from P;
          }
        }
        queue P : prefill {
          pool kv { cap 10; }
          serve fifo;
          nic ps(100);
          prefill (p) { hold kv (p) { run (p); } cache (p) lease kv (50); }
        }
        queue D : decode {
          pool kv { cap 10; }
          serve step { cost 1; memory kv; }
          nic ps(100);
          decode (p) { hold kv (p) { prefill (p) growing kv; } }
          decode (p) from src { hold kv (p) { join; transfer (p) from src to kv (p); } }
        }
        P push D share maxmin;
        stage gate : delay;
        workload {
          arrive batch(2);
          init { set prompt = 8; }
          session { run gate (serial == 0 ? 0.5 : 0); request gw; end; }
        }
        run { horizon 30; }
    "#);
    assert_eq!(r.ended, 0);
}

/// A preempted hold runs again from its start: a `fork` inside one would
/// send a second leg.
#[test]
fn a_fork_in_a_hold_that_may_be_preempted_is_refused() {
    refused(
        "pool kv { cap 10; preempt lifo; }
        stage svc : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { hold kv (1) { fork { run svc (1); } join; }
        }
        run { horizon 10; }",
        "`fork` inside a hold of `kv`, which may preempt it",
    );
}

#[test]
fn a_fork_needs_a_join() {
    refused(
        "stage svc : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { fork { run svc (1); } run svc (2);
        }
        run { horizon 10; }",
        "a program that forks a leg and never joins",
    );
}

/// The queue that posts the copies waits before each at its own delay
/// stage: one relation per poster gives it a `latency`.
#[test]
fn a_queue_posts_the_copies_of_one_relation() {
    refused(
        "queue P : prefill { pool kv { cap 10; } serve fifo; nic ps(1);
          prefill (p) { hold kv (p) { run (p); } cache (p) lease kv (inf); } }
        queue D : decode { pool kv { cap 10; } serve step { cost 1; memory kv; } nic ps(1);
          decode (p) { hold kv (p) { prefill (p) growing kv; } }
          decode (p) from src { hold kv (p) { transfer (p) from src to kv (p); } } }
        queue E : decode { pool kv { cap 10; } serve step { cost 1; memory kv; } nic ps(1);
          decode (p) { hold kv (p) { prefill (p) growing kv; } }
          decode (p) from src { hold kv (p) { transfer (p) from src to kv (p); } } }
        P push D latency 1 share maxmin;
        P push E latency 1 share maxmin;
        workload { arrive batch(1);
          session { request; end;
          }
        } server {
        } run { horizon 10; }",
        "`P` waits before the copies of another relation already",
    );
}
