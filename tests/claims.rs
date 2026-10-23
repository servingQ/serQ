//! `claim NAME [given (e)] : every iteration of ENGINE (e);`, `… some
//! iteration of ENGINE (e);`, `… at end (e);`: propositions about every
//! path, which the interpreter checks on the path it runs.

mod common;

use serq::engine::report::ClaimResult;
use serq::{Overrides, Program};

/// One engine at critical load: a request is 290 prompt tokens and 990
/// decodes, an iteration of up to 128 tokens costs `c + a`, and a request
/// arrives every `10 (c + a)`, the time its 1280 tokens take at a full batch.
const ENGINE: &str = "
let bmax = 128;
let c = 1128;
let a = 3547;
let b0 = 128;
device gpu { }
engine llm on gpu {
  tokens cap bmax;
  scheduler { SCHEDULE }
  worker { forward (c + a * ceil(batch.tokens / b0)); }
}
server (t0) { llm (290, 991); observe response = now - t0;
}
claim work_conserving: every iteration of llm (demand < bmax || batch.tokens == bmax);
claim token_rate: every iteration of llm (served * (c + a * ceil(bmax / b0)) <= bmax * now);
claim starved: some iteration of llm (demand >= bmax && batch.tokens < bmax);
claim mean_ok: at end (total(response) <= 1000000 * count(response));

";

/// `ENGINE`'s workload: a request every `10 (c + a)`.
const ENGINE_WORKLOAD: &str = "
workload { arrive renewal(46750); init { set t0 = now; } send (t0);
  session { turn; end;
  }
}
";

fn engine(schedule: &str) -> String {
    ENGINE.replace("SCHEDULE", schedule)
}

/// The schedule that advances the residents' decodes first.
const DECODE_FIRST: &str =
    "advance running decode first; admit waiting while (running.preempted == 0);";

/// Run the deployment `src` with the workload's file `workload`.
fn run(src: &str, workload: &str, options: &Overrides) -> serq::Report {
    common::run(&common::main_source(src), workload, options).unwrap()
}

/// The error of linking `src`, a `BATCH` deployment, with `BATCH`'s workload.
fn link_error(src: &str) -> String {
    common::compile(
        &common::main_source(src),
        BATCH_WORKLOAD,
        &common::horizon(1000000.0),
    )
    .unwrap_err()
}

fn result(r: &serq::Report, name: &str) -> ClaimResult {
    r.claim(name).unwrap().result
}

/// Under `decode first` the engine takes what it can: every
/// iteration whose residents could take a full batch schedules one, and
/// the tokens it served never outrun a full batch per iteration's cost.
#[test]
fn a_work_conserving_engine_holds_its_claims() {
    let r = run(
        &engine(DECODE_FIRST),
        ENGINE_WORKLOAD,
        &common::horizon(1000000.0),
    );
    let iterations = r.stage("llm").unwrap().iterations;
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

/// `only` a prefill when no decode is resident and decodes alone
/// otherwise: an iteration of decodes leaves the newcomers' prompts out,
/// so the engine is not work conserving and an iteration starves.
#[test]
fn an_engine_that_serves_one_kind_fails_them() {
    let r = run(
        &engine(
            "advance running only (running.decoding > 0 ? decoding : !decoding);
             admit waiting only (running.decoding > 0 ? decoding : !decoding)
               while (running.preempted == 0);",
        ),
        ENGINE_WORKLOAD,
        &common::horizon(1000000.0),
    );
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
        device gpu { }
        engine llm on gpu {
          tokens cap 4;
          scheduler { advance running; admit waiting while (running.preempted == 0); }
          worker { forward (1); }
        }
        server (n) { llm (n, 1); observe x = n;
        }

";

/// `BATCH`'s workload: three requests at once, of 3, 1 and 2 tokens.
const BATCH_WORKLOAD: &str = "
        workload { arrive batch(3); init { set n = serial == 0 ? 3 : serial; } send (n);
          session { turn; end;
          }
        }
";

/// `prefix_total` sorts the values: 3, 1, 2 give 1 + (1 + 2) + (1 + 2 + 3).
#[test]
fn the_aggregates_of_the_observations() {
    let r = run(
        &format!(
            "{BATCH}
         claim t: at end (total(x) == 6 && count(x) == 3);
         claim m: at end (largest(x) == 3 && smallest(x) == 1);
         claim p: at end (prefix_total(x) == 10);
         claim q: at end (prefix_total(x) == 14);"
        ),
        BATCH_WORKLOAD,
        &common::horizon(100.0),
    );
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
    let src = format!("{BATCH} claim t: at end (count(x) == 3);");
    let r = run(
        &src,
        BATCH_WORKLOAD,
        &Overrides {
            warmup: Some(50.0),
            ..common::horizon(100.0)
        },
    );
    assert_eq!(r.observe("x").unwrap().count, 0);
    assert_eq!(result(&r, "t"), ClaimResult::Holds);
}

/// A session that fails `given` takes the claim out of scope: the path is
/// not one the claim is about.
#[test]
fn a_session_that_fails_given_puts_the_claim_out_of_scope() {
    let r = run(
        &format!(
            "{BATCH}
         claim small given (n <= 2): every iteration of llm (batch.tokens <= 2);
         claim any given (n <= 3): every iteration of llm (batch.tokens <= 4);"
        ),
        BATCH_WORKLOAD,
        &common::horizon(100.0),
    );
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
    let r = run(
        &format!(
            "{BATCH}
         claim first: some iteration of llm (served == 0 && demand == 6 && batch.tokens == 4);
         claim second: some iteration of llm (served == 4 && demand == 2 && batch.tokens == 2);
         claim two: every iteration of llm (served < 6);"
        ),
        BATCH_WORKLOAD,
        &common::horizon(100.0),
    );
    assert_eq!(result(&r, "first"), ClaimResult::Witnessed);
    assert_eq!(result(&r, "second"), ClaimResult::Witnessed);
    let two = r.claim("two").unwrap();
    assert_eq!((two.result, two.checked), (ClaimResult::Holds, 2));
}

/// `arrived` is the sessions started by the iteration's start, one arriving
/// at that instant included: a session of 3 tokens every 10 units, each
/// served in the iteration that starts at its arrival, so at 20 two have
/// arrived and `3 * arrived` is always what was served plus this batch.
#[test]
fn arrived_counts_the_sessions_started_by_the_iteration() {
    let r = run(
        "
        device gpu { }
        engine llm on gpu {
          tokens cap 4;
          scheduler { advance running; admit waiting while (running.preempted == 0); }
          worker { forward (1); }
        }
        server { llm (3, 1);
        }

        claim at_20: some iteration of llm (now == 20 && arrived == 2);
        claim balance: every iteration of llm (arrived * 3 == served + batch.tokens);
        claim clock: every iteration of llm (arrived * 10 == now);",
        "
        workload { arrive renewal(10);
          session { turn; end;
          }
        }",
        &common::horizon(55.0),
    );
    assert_eq!(result(&r, "at_20"), ClaimResult::Witnessed);
    let b = r.claim("balance").unwrap();
    assert_eq!((b.result, b.checked), (ClaimResult::Holds, 5));
    assert_eq!(result(&r, "clock"), ClaimResult::Holds);
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
            "claim a: every iteration of llm (n > 0);",
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
            "claim t: every iteration of llm (total(x) > 0);",
            "read only by a claim `at end`",
        ),
        (
            "claim t: every iteration of tool (1);",
            "stage `tool` is not an engine",
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
            "claim g given (busy(llm) > 0): at end (1);",
            "`busy(…)` reads the deployment's state",
        ),
        (
            "claim d: every iteration of llm (~exp(1) > 0);",
            "a claim may not draw",
        ),
        (
            "claim w: every iteration of llm (work(llm) > 0);",
            "may not read `work(…)`",
        ),
        ("claim e: at end (demand > 0);", "exists only in"),
        (
            "claim e: at end (busy(llm) == 0);",
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

/// A claim over an engine's iterations reads the engine's values as its
/// `worker` does, the batch, and `running.count`: each is the kernel's
/// variable a bare name gave before #437, so the IR is the same. Three
/// prompts of 3, 1 and 2 under a budget of 4: the first iteration schedules
/// 3 and 1, and all three are resident.
#[test]
fn a_claim_over_iterations_reads_its_engines_names() {
    use serq::ir::{CExpr, CtxVar};
    let src = format!(
        "{BATCH}
         claim w: every iteration of llm (batch.tokens + batch.prefilled + batch.decoding
           + batch.kv_decode + batch.kv_prefill + batch.attention + running.count >= 0);
         claim first: some iteration of llm (served == 0 && batch.tokens == 4
           && batch.prefilled == 4 && batch.decoding == 0 && running.count == 3);"
    );
    let options = common::horizon(100.0);
    let p = common::compile(&common::main_source(&src), BATCH_WORKLOAD, &options).unwrap();
    let read = std::cell::RefCell::new(vec![]);
    p.claims[0].expr.any(&|e| {
        if let CExpr::Ctx(v) = e {
            read.borrow_mut().push(*v);
        }
        false
    });
    assert_eq!(
        read.into_inner(),
        [
            CtxVar::Ntok,
            CtxVar::Npre,
            CtxVar::Ndec,
            CtxVar::Kvb,
            CtxVar::Kvp,
            CtxVar::Attn,
            CtxVar::Nres
        ]
    );
    let r = run(&src, BATCH_WORKLOAD, &options);
    let w = r.claim("w").unwrap();
    let iterations = r.stage("llm").unwrap().iterations;
    assert_eq!((w.result, w.checked), (ClaimResult::Holds, iterations));
    assert_eq!(result(&r, "first"), ClaimResult::Witnessed);
}

/// A claim over iterations names each value one way, the engine's: a bare
/// kernel name is refused with the engine's name for it, since `decoders`
/// is `running.decoding` in `tokens cap` and `batch.decoding` here (#437).
/// What the claim's moment does not hold is refused with why.
#[test]
fn a_claim_over_iterations_refuses_the_kernels_names() {
    for (claim, want) in [
        (
            "claim c: every iteration of llm (tokens <= 4);",
            "in a claim over an engine's iterations, `tokens` is `batch.tokens` here: one value, \
             one name",
        ),
        (
            "claim c: some iteration of llm (tokens == 4);",
            "in a claim over an engine's iterations, `tokens` is `batch.tokens` here",
        ),
        (
            "claim c: every iteration of llm (batch.tokens == 4 || decoders == residents);",
            "`decoders` is `batch.decoding` here",
        ),
        (
            "claim c: every iteration of llm (residents > 0);",
            "`residents` is `running.count` here",
        ),
        (
            "claim c: every iteration of llm (kv_decode >= 0);",
            "`kv_decode` is `batch.kv_decode` here",
        ),
        (
            "claim c: every iteration of llm (ntok <= 4);",
            "`ntok` is `batch.tokens` here",
        ),
        (
            "claim c: every iteration of llm (running.decoding >= 0);",
            "`running.decoding` counts the residents, in the batch or not, and the batch's is \
             `batch.decoding`",
        ),
        (
            "claim c: every iteration of llm (waiting.count >= 0);",
            "a claim reads a queue as `queued(P)`",
        ),
        (
            "claim c: every iteration of llm (running.preempted == 0);",
            "`running.preempted` is what the iteration's schedule did",
        ),
        (
            "claim c: every iteration of llm (preempted == 0);",
            "`preempted` is `running.preempted`, and a claim over an engine's iterations is read \
             once the batch is formed",
        ),
        (
            "claim c: every iteration of llm (request.input > 0);",
            "`request.input` is one request's, read in its engine's `taking` and `cache`",
        ),
        (
            "claim c: at end (batch.tokens > 0);",
            "`batch.…` is a value of an engine, read in its `scheduler` and `worker`, and in a \
             claim over its iterations",
        ),
        (
            "claim c given (running.count > 0): at end (1);",
            "`running.…` is a value of an engine, read in its `tokens cap`, `each at most`, \
             `only`, `by` and `scheduler`, and in a claim over its iterations",
        ),
    ] {
        let e = link_error(&format!("{BATCH}{claim}"));
        assert!(e.contains(want), "{claim}: {e}");
    }
}

/// Outside an engine and a claim over its iterations, a kernel's bare name
/// the engine's values replaced is answered with the engine's names, not
/// with the moments that read the kernel's: the message sent `claim c: at
/// end (tokens > 0);` to "a claim over iterations", which refuses `tokens`
/// since #437 (and to an engine's `worker`, which already refused it).
#[test]
fn a_kernel_name_outside_an_engine_says_the_engines() {
    for (decl, want) in [
        (
            "claim c: at end (tokens > 0);",
            "`tokens` is a value of an engine, read as `batch.tokens` in its `scheduler` and \
             `worker`, and in a claim over its iterations",
        ),
        (
            "claim c: at end (decoders > 0);",
            "`decoders` is a value of an engine, read as `running.decoding` in its `tokens cap`, \
             `each at most`, `only`, `by` and `scheduler`; or `batch.decoding` in its `worker`, \
             and in a claim over its iterations",
        ),
        (
            "claim c given (residents > 0): at end (1);",
            "`residents` is a value of an engine, read as `running.count`",
        ),
        (
            "claim c: at end (ntok > 0);",
            "`ntok` is a value of an engine, read as `batch.tokens`",
        ),
        (
            "let tokens = 3;",
            "`tokens` is a name the language supplies, read in an engine, as `batch.tokens` in \
             its `scheduler` and `worker`, and in a claim over its iterations; a `let` constant \
             named `tokens`",
        ),
    ] {
        let e = link_error(&format!("{BATCH}{decl}"));
        assert!(e.contains(want), "{decl}: {e}");
        assert!(!e.contains("exists only in"), "{decl}: {e}");
    }
}

/// An iteration claim may name a member of an engine array: a family of
/// queues, each with its engine, the server calling a member's entry (an
/// engine family has no request, so a server cannot run one directly).
#[test]
fn a_claim_over_a_member_of_an_array() {
    let src = "
        queue llm[2] : prefill {
          device gpu { }
          engine on gpu {
            tokens cap 4;
            scheduler { advance running; admit waiting while (running.preempted == 0); }
            worker { forward (1); }
          }
          prefill (prompt) { run llm prefill (cost(llm, prompt)); }
        }
        server { llm[serial].prefill (3);
        }
        claim one: every iteration of llm[1] (batch.tokens == 3);
        ";
    let workload = "
        workload { arrive batch(2);
          session { turn; end;
          }
        }";
    let p = common::compile(&common::main_source(src), workload, &common::horizon(100.0)).unwrap();
    assert_eq!(p.claims[0].kind, serq::ir::ClaimKind::EveryIteration(1));
    let r = run(src, workload, &common::horizon(100.0));
    let one = r.claim("one").unwrap();
    assert_eq!((one.result, one.checked), (ClaimResult::Holds, 1));
}

/// The claims survive the IR's JSON; a program without any has no `claims`
/// key, and neither has its report.
#[test]
fn the_ir_keeps_the_claims() {
    let src = format!(
        "{BATCH} claim small given (n <= 3): some iteration of llm (demand > served);
         claim p: at end (prefix_total(x) == 10);"
    );
    let batch = common::horizon(100.0);
    let p = common::compile(&common::main_source(&src), BATCH_WORKLOAD, &batch).unwrap();
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

    let none = common::compile(&common::main_source(BATCH), BATCH_WORKLOAD, &batch).unwrap();
    let j: serde_json::Value = serde_json::from_str(&none.to_json()).unwrap();
    assert!(j.get("claims").is_none());
    let r: serde_json::Value =
        serde_json::from_str(&run(BATCH, BATCH_WORKLOAD, &common::horizon(100.0)).json()).unwrap();
    assert!(r.get("claims").is_none());
    assert!(
        !run(BATCH, BATCH_WORKLOAD, &common::horizon(100.0))
            .text()
            .contains("claim")
    );
}

/// `serq fmt` prints a claim back as it was written.
#[test]
fn the_formatter_keeps_a_claim() {
    let src = engine(DECODE_FIRST);
    let formatted = serq::frontend::fmt::format(&common::main_source(&src)).unwrap();
    assert!(formatted.contains("claim work_conserving:"));
    let options = common::horizon(100.0);
    assert_eq!(
        common::compile(&formatted, ENGINE_WORKLOAD, &options)
            .unwrap()
            .to_json(),
        common::compile(&common::main_source(&src), ENGINE_WORKLOAD, &options)
            .unwrap()
            .to_json()
    );
}

/// The workload's file `examples/pairs.json` runs the paper program `name`
/// with.
fn paper_workload(name: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = serq::paired_workload(&root.join(format!("examples/papers/{name}.sq")))
        .unwrap_or_else(|| panic!("examples/pairs.json pairs no workload with {name}"));
    std::fs::read_to_string(path).unwrap()
}

/// Durations of the paper experiments whose complete IR is committed.
fn paper_horizon(name: &str) -> f64 {
    match name {
        "bari_rad" => 100_000_000.0,
        "dai_fastertransformer" | "dai_sarathi" => 5_000_000.0,
        "kong_svf" => 100_000.0,
        _ => panic!("specify the experiment horizon for {name}"),
    }
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
        let p: Program = common::compile(
            &common::main_source(&src),
            &paper_workload(&name),
            &common::horizon(paper_horizon(&name)),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
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
    let mut names: Vec<_> = std::fs::read_dir(root.join("examples/papers"))
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.extension()? == "sq").then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    for name in names {
        let src = std::fs::read_to_string(root.join(format!("examples/papers/{name}.sq"))).unwrap();
        let r = run(
            &src,
            &paper_workload(&name),
            &common::horizon(paper_horizon(&name)),
        );
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
