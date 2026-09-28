//! The two lints, and the corpus they must not fire on.
//!
//! Both come from bugs this repository shipped. Both are errors rather than
//! warnings because neither has a legitimate instance in `programs/` — a
//! warning nobody acts on is worse than no check — so a false positive here
//! is a real cost and `no_false_positives_on_the_corpus` is the test that
//! matters most.

use seq::{Overrides, compile_source, program_path};

fn check(src: &str) -> Result<(), String> {
    compile_source(src, &Overrides::default()).map(|_| ())
}

const ENGINE: &str = "let bs = 16;
    pool kv { cap 1e5; block bs; evict lru; }
    pool reqs { cap 8; }
    stage engine : step { budget 512; cost 1e-3; memory kv; }
    workload { arrive poisson(0.3); init { set K = 0; }
               turn { set n = ~exp(500); set o = ~exp(200) + 1; } }
    run { horizon 500; }";

/// `programs/vllm.seq` shipped with a prefix-cache lookup bound before the
/// session queued, under a comment citing the admission-time lookup. A hold's
/// header is read at admission; a `set` above it is not.
#[test]
fn a_stale_header_read_is_rejected() {
    let src = format!(
        "{ENGINE} session {{ turn; loop {{
            set prompt = K + n;
            set c = min(cachedin(kv), prompt - 1);
            enter reqs (1), kv (c + min(prompt - c, budget_left(engine)))
              {{ prefill (prompt - cached) growing kv; }} keep (prompt + o);
            set K = prompt + o; end; }} }}"
    );
    let e = check(&src).expect_err("rejected");
    assert!(e.contains("`c` reads cachedin(kv)"), "{e}");
    assert!(
        e.contains("hold on `kv`"),
        "names the pool that reads it: {e}"
    );
    assert!(
        e.contains("at admission"),
        "points at the clause for it: {e}"
    );
}

/// The same program with the clause links.
#[test]
fn at_admission_is_the_way_through() {
    let src = format!(
        "{ENGINE} session {{ turn; loop {{
            set prompt = K + n;
            enter reqs (1), kv (c + min(prompt - c, budget_left(engine)))
              at admission (c = min(cachedin(kv), prompt - 1))
              {{ prefill (prompt - cached) growing kv; }} keep (prompt + o);
            set K = prompt + o; end; }} }}"
    );
    check(&src).expect("the clause is the way to say it");
}

/// Reassigning before the hold means the stale value never reaches it.
#[test]
fn a_reassignment_clears_the_lint() {
    let src = format!(
        "{ENGINE} session {{ turn; loop {{
            set prompt = K + n;
            set c = min(cachedin(kv), prompt - 1);
            set c = 0;
            enter reqs (1), kv (c + prompt) {{ prefill (prompt) growing kv; }} keep (prompt);
            set K = prompt + o; end; }} }}"
    );
    check(&src).expect("the read no longer reaches the header");
}

/// A `set` inside the body runs after admission, which is the timing it would
/// have had anyway.
#[test]
fn a_read_inside_the_body_is_fine() {
    let src = format!(
        "{ENGINE} session {{ turn; loop {{
            set prompt = K + n;
            enter reqs (1), kv (prompt) {{
              set c = min(cachedin(kv), prompt - 1);
              observe hit = c; prefill (prompt) growing kv; }} keep (prompt);
            set K = prompt + o; end; }} }}"
    );
    check(&src).expect("after admission is not stale");
}

/// A guard strictly between 0 and 1 is a probability, and reads as a test.
#[test]
fn a_constant_probability_guard_is_rejected() {
    let src = "stage tool : delay;
        workload { arrive poisson(1); turn { set Z = ~exp(3); } }
        session { turn; loop { branch (0.8) { run tool (Z); turn; } else { end; } } }
        run { horizon 100; }";
    let e = check(src).expect_err("rejected");
    assert!(e.contains("`branch (0.8)` is a draw"), "{e}");
    assert!(e.contains("branch with (0.8)"), "{e}");
}

/// 0 and 1 are tests, not draws, and stay legal.
#[test]
fn zero_and_one_are_tests() {
    for g in ["0", "1"] {
        let src = format!(
            "stage tool : delay;
             workload {{ arrive poisson(1); turn {{ set Z = ~exp(3); }} }}
             session {{ turn; loop {{ branch ({g}) {{ run tool (Z); turn; }} else {{ end; }} }} }}
             run {{ horizon 100; }}"
        );
        check(&src).unwrap_or_else(|e| panic!("`branch ({g})` is a test: {e}"));
    }
}

/// A context variable outside the moment that supplies it. `ntok` is what a
/// step stage's budget and cost see for one iteration; in a session
/// statement it used to read as 0 and the program ran. The IR's validator
/// knows the table, and the linker now runs it on what it produces.
#[test]
fn a_context_variable_outside_its_moment_is_rejected() {
    let src = format!(
        "{ENGINE} session {{ turn; set x = ntok;
            enter reqs (1), kv (n) {{ prefill (n) growing kv; }} end; }}"
    );
    let e = check(&src).expect_err("rejected");
    assert!(e.contains("`ntok` is read in a session statement"), "{e}");
    assert!(e.contains("exists only in a step stage's budget"), "{e}");
    // and the other way: an eviction key is not a session statement
    let src = "let bs = 16;
        pool kv { cap 1e5; block bs; evict by (ntok); }
        pool reqs { cap 8; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }
        workload { arrive poisson(0.3); init { set K = 0; }
                   turn { set n = ~exp(500); set o = ~exp(200) + 1; } }
        run { horizon 500; }
        session { turn; enter reqs (1), kv (n) { prefill (n) growing kv; } end; }";
    let e = check(src).expect_err("rejected");
    assert!(e.contains("pool `kv`"), "{e}");
    assert!(e.contains("`ntok` is read in an eviction key"), "{e}");
    // what the table allows still links: `age` in an eviction key, `n` in a
    // ps capacity, `ntok` in a cost, `now` anywhere
    let src = "pool kv { cap 1e5; evict by (age, size); }
        stage svc : ps (min(n, 4));
        stage engine : step { budget 512; cost 1e-3 * ntok + now * 0; memory kv; }
        workload { arrive poisson(0.3); turn { set n = ~exp(500); } }
        session { turn; set t = now; hold kv (n) { run svc (n); } end; }
        run { horizon 500; }";
    check(src).expect("links");
}

/// The lints exist to be errors, which is only defensible if nothing real
/// trips them.
#[test]
fn no_false_positives_on_the_corpus() {
    for name in [
        "mg1",
        "ps",
        "closed",
        "agentic",
        "replica",
        "pd_tandem",
        "lecture_pd",
        "routing",
        "vllm",
        "vllm_request",
        "vllm_replay",
    ] {
        let src = std::fs::read_to_string(program_path(name)).unwrap();
        compile_source(&src, &Overrides::default())
            .unwrap_or_else(|e| panic!("{name} is a real program and must link: {e}"));
    }
}
