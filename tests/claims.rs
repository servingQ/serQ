//! `claim NAME [given (e)] : every iteration of S (e);`, `… some iteration of
//! S (e);`, `… at end (e);`: propositions about every path, which the
//! interpreter checks on the path it runs.

use serq::engine::report::ClaimResult;
use serq::{Overrides, Program, compile_source, run_source};

/// One engine at critical load: a request is 290 prompt tokens and 990
/// decodes, an iteration of up to 128 tokens costs `c + a`, and a request
/// arrives every `10 (c + a)`, the time its 1280 tokens take at a full batch.
const ENGINE: &str = "
let bmax = 128;
let c = 1128;
let a = 3547;
let b0 = 128;
stage engine : step { budget bmax; cost c + a * ceil(tokens / b0); SERVE }
workload { arrive renewal(46750); init { set t0 = now; } }
session { run engine prefill (290); run engine decode (990); observe response = now - t0; end; }
claim work_conserving: every iteration of engine (demand < bmax || tokens == bmax);
claim token_rate: every iteration of engine (served * (c + a * ceil(bmax / b0)) <= bmax * now);
claim starved: some iteration of engine (demand >= bmax && tokens < bmax);
claim mean_ok: at end (total(response) <= 1000000 * count(response));
run { horizon 1000000; warmup 0; seed 1; }
";

fn engine(serve: &str) -> String {
    ENGINE.replace("SERVE", serve)
}

fn run(src: &str) -> serq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
}

fn link_error(src: &str) -> String {
    compile_source(src, &Overrides::default()).unwrap_err()
}

fn result(r: &serq::Report, name: &str) -> ClaimResult {
    r.claim(name).unwrap().result
}

/// Under `serve decode first` the engine takes what it can: every
/// iteration whose residents could take a full batch schedules one, and
/// the tokens it served never outrun a full batch per iteration's cost.
#[test]
fn a_work_conserving_engine_holds_its_claims() {
    let r = run(&engine("serve decode first;"));
    let iterations = r.stage("engine").unwrap().iterations;
    assert!(iterations > 100, "{iterations}");
    for name in ["work_conserving", "token_rate"] {
        let c = r.claim(name).unwrap();
        assert_eq!(c.result, ClaimResult::Holds, "{name}: {c:?}");
        assert_eq!(c.checked, iterations);
        assert_eq!((c.failures, c.first), (0, None));
    }
    assert_eq!(result(&r, "starved"), ClaimResult::NotWitnessed);
    // the run ends with requests in flight: the end the claim is about
    // was not reached
    let end = r.claim("mean_ok").unwrap();
    assert_eq!(end.result, ClaimResult::NotEvaluated);
    assert!(
        end.note.as_deref().unwrap().ends_with("live at the end"),
        "{end:?}"
    );
    let text = r.text();
    assert!(
        text.contains("work_conserving  every iteration  holds ("),
        "{text}"
    );
    assert!(text.contains("not evaluated: "), "{text}");
}

/// `serve only` a prefill when no decode is resident and decodes alone
/// otherwise: an iteration of decodes leaves the newcomers' prompts out,
/// so the engine is not work conserving and an iteration starves.
#[test]
fn an_engine_that_serves_one_kind_fails_them() {
    let r = run(&engine("serve only (decoders > 0 ? decoding : !decoding);"));
    let w = r.claim("work_conserving").unwrap();
    assert_eq!(w.result, ClaimResult::Fails);
    assert!(w.failures > 0 && w.failures <= w.checked);
    let s = r.claim("starved").unwrap();
    assert_eq!(s.result, ClaimResult::Witnessed);
    // the first starved iteration is the first that fails conservation
    assert_eq!(s.first, w.first);
    assert_eq!(result(&r, "token_rate"), ClaimResult::Holds);
    let j: serde_json::Value = serde_json::from_str(&r.json()).unwrap();
    let w = &j["claims"][0];
    assert_eq!(w["name"], "work_conserving");
    assert_eq!(w["kind"], "every_iteration");
    assert_eq!(w["result"], "fails");
    assert!(w["first"].is_number() && w["note"].is_null());
    assert_eq!(j["claims"][2]["result"], "witnessed");
    assert_eq!(j["claims"][3]["result"], "not_evaluated");
}

const BATCH: &str = "
stage engine : step { budget 4; cost 1; }
workload { arrive batch(3); init { set n = serial == 0 ? 3 : serial; } }
session { run engine prefill (n); observe x = n; end; }
run { horizon 100; }
";

/// `prefix_total` sorts the values: 3, 1, 2 give 1 + (1 + 2) + (1 + 2 + 3).
#[test]
fn the_aggregates_of_the_observations() {
    let r = run(&format!(
        "{BATCH}
         claim t: at end (total(x) == 6 && count(x) == 3);
         claim m: at end (largest(x) == 3 && smallest(x) == 1);
         claim p: at end (prefix_total(x) == 10);
         claim q: at end (prefix_total(x) == 14);"
    ));
    for name in ["t", "m", "p"] {
        assert_eq!(result(&r, name), ClaimResult::Holds, "{name}");
    }
    let q = r.claim("q").unwrap();
    assert_eq!(q.result, ClaimResult::Fails);
    assert_eq!(q.first, Some(r.end));
    assert!(r.text().contains("q      at end  fails"), "{}", r.text());
    assert_eq!(serq::ir::Agg::PrefixTotal.of(&[3.0, 1.0, 2.0]), 10.0);
    assert_eq!(serq::ir::Agg::Largest.of(&[]), 0.0);
}

/// The aggregates are the whole run's: warm-up does not drop a value.
#[test]
fn the_aggregates_include_the_warmup() {
    let src = format!("{BATCH} claim t: at end (count(x) == 3);")
        .replace("run { horizon 100; }", "run { horizon 100; warmup 50; }");
    let r = run(&src);
    assert_eq!(r.observe("x").unwrap().count, 0);
    assert_eq!(result(&r, "t"), ClaimResult::Holds);
}

/// A session that fails `given` takes the claim out of scope: the path is
/// not one the claim is about.
#[test]
fn a_session_that_fails_given_puts_the_claim_out_of_scope() {
    let r = run(&format!(
        "{BATCH}
         claim small given (n <= 2): every iteration of engine (tokens <= 2);
         claim any given (n <= 3): every iteration of engine (tokens <= 4);"
    ));
    let small = r.claim("small").unwrap();
    assert_eq!(small.result, ClaimResult::OutOfScope);
    assert_eq!(small.note.as_deref(), Some("session 0 fails `given`"));
    assert!(
        r.text().contains("out of scope: session 0 fails `given`"),
        "{}",
        r.text()
    );
    assert_eq!(result(&r, "any"), ClaimResult::Holds);
}

/// `served` is the tokens of the earlier iterations, `demand` what the
/// residents could take now: three prompts of 3, 1, 2 under a budget of 4
/// are scheduled 4 then 2, with 6 then 2 demanded.
#[test]
fn served_and_demand_are_read_as_the_iteration_starts() {
    let r = run(&format!(
        "{BATCH}
         claim first: some iteration of engine (served == 0 && demand == 6 && tokens == 4);
         claim second: some iteration of engine (served == 4 && demand == 2 && tokens == 2);
         claim two: every iteration of engine (served < 6);"
    ));
    assert_eq!(result(&r, "first"), ClaimResult::Witnessed);
    assert_eq!(result(&r, "second"), ClaimResult::Witnessed);
    let two = r.claim("two").unwrap();
    assert_eq!((two.result, two.checked), (ClaimResult::Holds, 2));
}

/// Each moment reads what it supplies, and a claim's name is its own.
#[test]
fn a_claim_reads_only_what_its_moment_supplies() {
    let deployment = format!("{BATCH} stage tool : delay;");
    for (claim, want) in [
        (
            "claim d: at end (1); session_extra",
            "exists only in a claim over iterations",
        ),
        (
            "claim a: every iteration of engine (n > 0);",
            "`n` is a session attribute, and a claim over iterations",
        ),
        (
            "claim a: at end (n > 0);",
            "`n` is a session attribute, and a claim `at end`",
        ),
        (
            "claim t: at end (total(y) > 0);",
            "`total(y)`: `y` is not an `observe` of the program",
        ),
        (
            "claim t: every iteration of engine (total(x) > 0);",
            "read only by a claim `at end`",
        ),
        (
            "claim t: every iteration of tool (1);",
            "stage `tool` is not a `step` stage",
        ),
        (
            "claim t: at end (1); claim t: at end (0);",
            "claim `t`: declared twice",
        ),
        (
            "claim g given (now > 0): at end (1);",
            "a claim's `given` may not read `now`",
        ),
        (
            "claim g given (busy(engine) > 0): at end (1);",
            "`busy(…)` reads the deployment's state",
        ),
        (
            "claim d: every iteration of engine (~exp(1) > 0);",
            "a claim may not draw",
        ),
        (
            "claim w: every iteration of engine (work(engine) > 0);",
            "may not read `work(…)`",
        ),
        ("claim e: at end (tokens > 0);", "exists only in"),
        (
            "claim e: at end (busy(engine) == 0);",
            "`busy(…)` reads the deployment's state",
        ),
    ] {
        let src = if claim.contains("session_extra") {
            // `demand` in a session statement
            deployment.replace("observe x = n;", "observe x = demand;")
                + &claim.replace(" session_extra", "")
        } else {
            format!("{deployment}{claim}")
        };
        let e = link_error(&src);
        assert!(e.contains(want), "{claim}: {e}");
    }
    // `total(o)` in a session statement
    let e = link_error(&BATCH.replace("observe x = n;", "observe x = n; observe y = total(x);"));
    assert!(e.contains("read only by a claim `at end`"), "{e}");
}

/// An iteration claim may name a member of an engine array.
#[test]
fn a_claim_over_a_member_of_an_array() {
    let src = "
        stage engine[2] : step { budget 4; cost 1; }
        workload { arrive batch(2); }
        session { run engine[serial] prefill (3); end; }
        claim one: every iteration of engine[1] (tokens == 3);
        run { horizon 100; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
    assert_eq!(p.claims[0].kind, serq::ir::ClaimKind::EveryIteration(1));
    let r = run(src);
    let one = r.claim("one").unwrap();
    assert_eq!((one.result, one.checked), (ClaimResult::Holds, 1));
}

/// The claims survive the IR's JSON; a program without any has no `claims`
/// key, and neither has its report.
#[test]
fn the_ir_keeps_the_claims() {
    let src = format!(
        "{BATCH} claim small given (n <= 3): some iteration of engine (demand > served);
         claim p: at end (prefix_total(x) == 10);"
    );
    let p = compile_source(&src, &Overrides::default()).unwrap();
    assert_eq!(p.claims.len(), 2);
    let back = Program::from_json(&p.to_json()).unwrap();
    assert_eq!(back.claims, p.claims);
    let j: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
    assert_eq!(j["claims"][0]["kind"]["SomeIteration"], 0);
    assert_eq!(j["claims"][1]["kind"], "AtEnd");
    assert_eq!(
        j["claims"][1]["expr"]["Binary"][1]["Agg"],
        serde_json::json!(["PrefixTotal", 0])
    );
    assert_eq!(
        j["claims"][0]["expr"]["Binary"][2]["Ctx"],
        serde_json::json!("Served")
    );

    let none = compile_source(BATCH, &Overrides::default()).unwrap();
    let j: serde_json::Value = serde_json::from_str(&none.to_json()).unwrap();
    assert!(j.get("claims").is_none());
    let r: serde_json::Value = serde_json::from_str(&run(BATCH).json()).unwrap();
    assert!(r.get("claims").is_none());
    assert!(!run(BATCH).text().contains("claim"));
}

/// `serq fmt` prints a claim back as it was written.
#[test]
fn the_formatter_keeps_a_claim() {
    let src = engine("serve decode first;");
    assert_eq!(serq::frontend::fmt::format(&src).unwrap(), src);
}

/// The paper programs' IR, which `scripts/gen_lean_claims.py` reads to write
/// their claims as Lean statements (`lean/Serq/Claims.lean`), is the IR of
/// `examples/papers/*.sq` (`make claims-ir` rewrites it).
#[test]
fn claim_ir_files_are_current() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bless = std::env::var_os("SERQ_BLESS").is_some();
    let mut names: Vec<_> = std::fs::read_dir(root.join("examples/papers"))
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.extension()? == "sq").then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    assert!(!names.is_empty());
    for name in names {
        let src = std::fs::read_to_string(root.join(format!("examples/papers/{name}.sq"))).unwrap();
        let p: Program = compile_source(&src, &Overrides::default()).unwrap();
        let want = p.to_json() + "\n";
        let path = root.join(format!("tools/claims/{name}.ir.json"));
        if bless {
            std::fs::write(&path, &want).unwrap();
        } else {
            let have = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                have == want,
                "{} is stale: run `make claims-ir`",
                path.display()
            );
        }
    }
}

/// Every claim of the paper programs holds on the path the interpreter runs
/// (the Lean proofs say it holds on every path).
#[test]
fn paper_claims_hold() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in [
        "dai_sarathi",
        "dai_fastertransformer",
        "bari_rad",
        "kong_svf",
    ] {
        let src = std::fs::read_to_string(root.join(format!("examples/papers/{name}.sq"))).unwrap();
        let r = run(&src);
        for c in &r.claims {
            assert!(
                matches!(c.result, ClaimResult::Holds | ClaimResult::Witnessed),
                "{name}: claim {} is {:?}",
                c.name,
                c.result
            );
        }
    }
}
