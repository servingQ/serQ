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

/// A constant guard strictly between 0 and 1 is not a test; it was meant as
/// a draw, and the link error names the spelling for that.
#[test]
fn a_constant_probability_guard_is_rejected() {
    let src = "stage tool : delay;
        workload { arrive poisson(1); turn { set Z = ~exp(3); } }
        session { turn; loop { branch (0.8) { run tool (Z); turn; } else { end; } } }
        run { horizon 100; }";
    let e = check(src).expect_err("rejected");
    assert!(e.contains("`branch (0.8)` is not a test"), "{e}");
    assert!(e.contains("branch with (0.8)"), "{e}");
    // a constant that is neither 0 nor 1 and not a probability either
    let e = check(&src.replace("branch (0.8)", "branch (2)")).expect_err("rejected");
    assert!(
        e.contains("`branch (2)` is not a test: a guard is 0 or 1."),
        "{e}"
    );
    assert!(!e.contains("branch with"), "{e}");
    check(&src.replace("branch (0.8)", "branch (1)")).expect("0 and 1 are tests");
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
    assert!(
        e.starts_with("session: "),
        "names the block by its role: {e}"
    );
    assert!(e.contains("`ntok` is read in a session statement"), "{e}");
    assert!(e.contains("exists only in a step stage's cost"), "{e}");
    // every other moment refuses what it does not supply, and says where it
    // was read and where it exists
    let engine = |pool: &str, stage: &str, session: &str| {
        format!(
            "let bs = 16;
            pool kv {{ cap 1e5; block bs; {pool} }}
            pool reqs {{ cap 8; }}
            stage engine : step {{ budget 512; cost 1e-3; memory kv; {stage} }}
            stage svc : ps (4);
            workload {{ arrive poisson(0.3); init {{ set K = 0; }}
                       turn {{ set m = ~exp(500); set o = ~exp(200) + 1; }} }}
            run {{ horizon 500; }}
            session {{ turn; {session} enter reqs (1), kv (m) {{ prefill (m) growing kv; }} end; }}"
        )
    };
    // (the prompt is `m`, not `n`: a session attribute named `n` would
    // shadow the ps capacity variable, which is the case below)
    for (src, where_read, exists) in [
        (
            engine("evict by (ntok);", "", ""),
            "pool `kv`: `ntok` is read in an eviction key",
            "a step stage's cost",
        ),
        (
            engine("queue by (age);", "", ""),
            "pool `kv`: `age` is read in a hold's header (`hold`, `enter`, `admit if … where`) or a queue key, read at admission",
            "an eviction key",
        ),
        (
            engine("", "", "hold kv (size) { run svc (1); }"),
            "session: `size` is read in a hold's header (`hold`, `enter`, `admit if … where`) or a queue key, read at admission",
            "an eviction key",
        ),
        (
            engine("", "", "hold kv (1) { run svc (n); }"),
            "session: `n` is read in a session statement",
            "a ps stage's capacity",
        ),
        (
            engine("", "budget ntok + 512;", ""),
            "stage `engine`: `ntok` is read in a step stage's budget or chunk",
            "a step stage's cost",
        ),
        (
            engine("", "chunk attn;", ""),
            "stage `engine`: `attn` is read in a step stage's budget or chunk",
            "a step stage's cost",
        ),
        (
            engine("", "cost age;", ""),
            "stage `engine`: `age` is read in a step stage's cost",
            "an eviction key",
        ),
        (
            engine("", "", "set x = init_age; set y = age;"),
            "init: `init_age`",
            "",
        ),
    ] {
        if where_read.starts_with("init:") {
            // an `init` statement is named as such
            let src = engine("", "", "").replace("init { set K = 0; }", "init { set K = age; }");
            let e = check(&src).expect_err("rejected");
            assert!(e.starts_with("init: `age`"), "{e}");
            continue;
        }
        let e = match check(&src) {
            Err(e) => e,
            Ok(()) => panic!("linked, but should have said `{where_read}`"),
        };
        assert!(e.contains(where_read), "want `{where_read}` in: {e}");
        assert!(e.contains(exists), "want `{exists}` in: {e}");
    }
    // what the table allows still links: `age`, `size` and a pool index in
    // an eviction key, `n` in a ps capacity, the residents' variables in a
    // budget and a chunk, `ntok` in a cost, `now` at every moment
    let src =
        "pool kv[2] { cap 1e5; evict by (age, size + used(kv[size > 1e9 ? 1 : 0]) * 0, now * 0); }
        stage svc : ps (min(n, 4) + now * 0);
        stage engine : step { budget 512 + nres + ndec + kvb * 0 + kvp * 0 + now * 0;
                              chunk ndec > 0 ? 64 : 128;
                              cost 1e-3 * ntok + npre * 0 + attn * 0 + now * 0; memory kv; }
        workload { arrive poisson(0.3); turn { set n = ~exp(500); } }
        session { turn; set t = now; hold kv[0] (n) { run svc (n); } end; }
        run { horizon 500; }";
    check(src).expect("links");
}

/// `serve` is said once per step stage, in one of three spellings, and the
/// two options it replaced are parse errors that name it.
#[test]
fn serve_is_one_order_said_once() {
    let step = |opts: &str| {
        format!(
            "pool kv {{ cap 1e5; }}
            stage engine : step {{ budget 512; cost 1; memory kv; {opts} }}
            workload {{ arrive batch(1); }}
            session {{ hold kv (1) {{ run engine prefill (1) growing kv; }} end; }}
            run {{ horizon 10; }}"
        )
    };
    for ok in [
        "",
        "serve admission;",
        "serve decode first;",
        "serve exclusive prefill;",
    ] {
        check(&step(ok)).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    let e = check(&step("serve decode first; serve admission;")).expect_err("twice");
    assert!(e.contains("`serve` twice"), "{e}");
    for (old, new) in [
        (
            "decode first;",
            "`decode first;` is now `serve decode first;`",
        ),
        (
            "exclusive prefill;",
            "`exclusive prefill;` is now `serve exclusive prefill;`",
        ),
    ] {
        let e = check(&step(old)).expect_err(old);
        assert!(e.contains(new), "the error names the new spelling: {e}");
    }
    let e = check(&step("serve shortest;")).expect_err("unknown");
    assert!(e.contains("`serve` takes"), "{e}");
}

/// `serve admission` is `by` with no keys (every resident ties, and ties are
/// admission order), so the IR knows one form; a serve key may read the
/// residents' variables and may not draw.
#[test]
fn serve_admission_is_by_with_no_keys_and_a_key_does_not_draw() {
    let step = |opts: &str| {
        format!(
            "pool kv {{ cap 1e5; }}
            stage engine : step {{ budget 512; cost 1; memory kv; {opts} }}
            workload {{ arrive batch(1); }}
            session {{ hold kv (1) {{ run engine prefill (1) growing kv; }} end; }}
            run {{ horizon 10; }}"
        )
    };
    let ir = |s: &str| {
        seq::compile_source(&step(s), &Overrides::default())
            .unwrap()
            .to_json()
    };
    assert_eq!(ir("serve admission;"), ir(""));
    assert!(ir("serve admission;").contains("\"By\": []"));
    check(&step("serve by (nres > 4 ? -remaining : admission);"))
        .expect("the residents' variables are keys");
    let e = check(&step("serve by (~uniform(0, 1));")).expect_err("a draw");
    assert!(e.contains("stage `engine`"), "{e}");
    assert!(e.contains("a serve key may not draw"), "{e}");
}

/// `hidden o;`: the scheduler does not know the output length (vLLM knows
/// `max_tokens`, scheduler.py:639, and learns the length when `check_stop`
/// sees EOS or the cap, sched/utils.py:98-119). A hidden attribute is read
/// in session statements and rejected at every scheduler moment.
#[test]
fn a_hidden_attribute_is_not_read_by_the_scheduler() {
    let wl = "let bs = 16;
        pool kv { cap 1e5; block bs; evict lru; }
        pool reqs { cap 8; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }
        workload { arrive poisson(0.3); hidden o; init { set K = 0; }
                   turn { set n = ~exp(500); set o = ~exp(200) + 1; } }
        run { horizon 500; }";
    // the body may read it
    let ok = format!(
        "{wl} session {{ turn; enter reqs (1), kv (n) {{ prefill (n) growing kv; decode (o - 1) growing kv; }} keep (n + o); end; }}"
    );
    check(&ok).expect("links");
    // a hold's header may not: the reservation is the scheduler's
    let bad = format!(
        "{wl} session {{ turn; enter reqs (1), kv (n) reserve (n + o) {{ prefill (n) growing kv; }} end; }}"
    );
    let e = check(&bad).expect_err("rejected");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    assert!(e.contains("read at admission"), "{e}");
    // the server-block spelling substitutes the binding into the header,
    // and the check sees the substituted expression
    let bad = format!(
        "{} server {{ admit if reqs (1), kv (n) reserve (need) fit where need = n + o {{ prefill (n) growing kv; }} }}",
        wl.replace(
            "init { set K = 0; }",
            "init { set K = 0; } session { turn; request; end; }"
        )
    );
    let e = check(&bad).expect_err("rejected");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    assert!(e.contains("read at admission"), "{e}");
    // nor a pool's queue key
    let bad = "let bs = 16;
        pool kv { cap 1e5; block bs; evict lru; queue by (o); }
        stage engine : step { budget 512; cost 1e-3; memory kv; }
        workload { arrive poisson(0.3); hidden o; turn { set n = ~exp(500); set o = ~exp(200) + 1; } }
        run { horizon 500; }
        session { turn; hold kv (n) { prefill (n) growing kv; } end; }";
    let e = check(bad).expect_err("rejected");
    assert!(e.contains("pool `kv`"), "{e}");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // nor a stage's budget
    let bad = "let bs = 16;
        pool kv { cap 1e5; block bs; evict lru; }
        stage engine : step { budget 512 + o; cost 1e-3; memory kv; }
        workload { arrive poisson(0.3); hidden o; turn { set n = ~exp(500); set o = ~exp(200) + 1; } }
        run { horizon 500; }
        session { turn; hold kv (n) { prefill (n) growing kv; } end; }";
    let e = check(bad).expect_err("rejected");
    assert!(e.contains("stage `engine`"), "{e}");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // a name nothing sets is not an attribute
    let bad = "pool kv { cap 1e5; }
        stage svc : fifo;
        workload { arrive poisson(0.3); hidden nothing; }
        run { horizon 500; }
        session { hold kv (1) { run svc (1); } end; }";
    let e = check(bad).expect_err("rejected");
    assert!(e.contains("hidden `nothing`"), "{e}");
    // what the scheduler sets cannot be hidden from it, and a name is hidden once
    let bad = format!("{wl} session {{ turn; hold kv (n) {{ prefill (n) growing kv; }} end; }}")
        .replace("hidden o;", "hidden computed;");
    let e = check(&bad).expect_err("rejected");
    assert!(
        e.contains("hidden `computed`: the scheduler sets it"),
        "{e}"
    );
    let bad = format!("{wl} session {{ turn; hold kv (n) {{ prefill (n) growing kv; }} end; }}")
        .replace("hidden o;", "hidden o, o;");
    let e = check(&bad).expect_err("rejected");
    assert!(e.contains("hidden `o` twice"), "{e}");
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
