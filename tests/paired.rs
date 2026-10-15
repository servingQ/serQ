//! A deployment file paired with a workload file by the tool
//! (`docs/design/paired-workload.md`): the workload file has no `fn
//! main()`; the two meet only at the server's signature, and each reads its
//! own names.

mod common;

use serq::{compile_paired, outline_paired};

const DEPLOYMENT: &str = "fn main() {
  stage svc : fifo;
  server (work) { run svc (cost(svc, work)); }
}
";

const WORKLOAD: &str = "let rate = 1;
workload {
  arrive poisson(rate);
  turn { set s = ~exp(1); }
  send (work: s);
}
";

fn names() -> (&'static str, &'static str) {
    ("d.sq", "w.sq")
}

fn refused(deployment: &str, workload: &str, fragments: &[&str]) {
    let err = compile_paired(deployment, workload, names(), &common::horizon(10.0)).unwrap_err();
    for f in fragments {
        assert!(err.contains(f), "missing {f:?} in\n{err}");
    }
}

#[test]
fn a_pair_compiles() {
    compile_paired(DEPLOYMENT, WORKLOAD, names(), &common::horizon(10.0)).unwrap();
}

/// A deployment's `def`, expanded where it is used, moves the workload's
/// tokens in the stream; they stay the workload's.
#[test]
fn an_expansion_keeps_the_workload_its_own() {
    let deployment = "def twice(x) { x * 2 + x * 0 + x * 0 + x * 0 }
fn main() {
  stage svc : fifo;
  server (work) { run svc (cost(svc, twice(work))); }
}
";
    compile_paired(
        deployment,
        "workload { arrive poisson(1); turn { set s = 1; } send (work: s); }\n",
        names(),
        &common::horizon(10.0),
    )
    .unwrap();
}

/// One deployment under several loads: `examples/multi-turn/vllm.sq` runs
/// under each workload `examples/pairs.json` lists for it, the chat it
/// starts with first, and each loads it as its own client.
#[test]
fn one_deployment_runs_under_each_of_its_workloads() {
    let deployment = std::path::Path::new("examples/multi-turn/vllm.sq");
    let workloads = serq::paired_workloads(deployment);
    let names: Vec<_> = workloads
        .iter()
        .map(|w| w.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["chat", "human_chat", "single_turn", "subagents"]);
    let ov = common::horizon(100.0);
    for w in &workloads {
        let p = serq::load_paired(deployment, w, &ov)
            .unwrap_or_else(|e| panic!("{}: {e}", w.display()));
        let r = serq::run_ir(&p, w.parent()).unwrap();
        assert!(r.turns > 0, "{}: no turn ran", w.display());
        assert!(p.stage_index("vllm").is_some(), "{}", w.display());
    }
}

#[test]
fn a_workload_file_has_no_main_and_declares_no_deployment() {
    refused(
        DEPLOYMENT,
        &format!("fn main() {{ }}\n{WORKLOAD}"),
        &["w.sq: 1:1:", "a workload file has no `fn main()`"],
    );
    refused(
        DEPLOYMENT,
        &format!("server (x) {{ }}\n{WORKLOAD}"),
        &["w.sq: 1:1:", "`server` is the deployment file's"],
    );
    // a pool of its own is the workload's to declare
    compile_paired(
        DEPLOYMENT,
        &format!("pool q {{ cap 1; }}\n{WORKLOAD}"),
        names(),
        &common::horizon(10.0),
    )
    .unwrap();
    refused(
        DEPLOYMENT,
        "let rate = 1;\n",
        &["w.sq: 1:1:", "a workload file declares `workload { … }`"],
    );
}

#[test]
fn a_paired_deployment_declares_no_workload() {
    let deployment = "fn main() {
  stage svc : fifo;
  workload { arrive poisson(1); }
  server (work) { run svc (cost(svc, work)); }
}
";
    refused(
        deployment,
        WORKLOAD,
        &["d.sq: 3:3:", "the workload is the workload file's"],
    );
}

/// Each file reads its own names; a message points into the file it is about.
#[test]
fn each_file_reads_its_own_names() {
    // the server reads the workload file's constant
    refused(
        "fn main() {\n  stage svc : fifo;\n  server (work) { run svc (cost(svc, work * rate)); }\n}\n",
        WORKLOAD,
        &["d.sq: 3:", "`rate` is the workload file's"],
    );
    // the workload reads the deployment file's constant
    let deployment = format!("let scale = 2;\n{DEPLOYMENT}");
    refused(
        &deployment,
        "workload {\n  arrive poisson(1);\n  turn { set s = ~exp(scale); }\n  send (work: s);\n}\n",
        &["w.sq: 3:", "`scale` is the deployment file's"],
    );
    // a constant of one reads one of the other
    refused(
        &deployment,
        "let rate = scale;\nworkload { arrive poisson(rate); turn { set s = 1; } send (work: s); }\n",
        &["w.sq: 1:", "`scale` is the deployment file's"],
    );
    // the arrival reads the deployment's
    refused(
        &deployment,
        "workload { arrive poisson(scale); turn { set s = 1; } send (work: s); }\n",
        &["w.sq: 1:", "`scale` is the deployment file's"],
    );
    // a declaration of the deployment reads the workload's constant
    refused(
        "fn main() {\n  pool q { cap rate; }\n  stage svc : fifo;\n  server (work) { hold q (cost(q, 1)) { run svc (cost(svc, work)); } }\n}\n",
        WORKLOAD,
        &["d.sq: 2:", "`rate` is the workload file's"],
    );
    // a definition of the other file
    refused(
        "def twice(x) { x * 2 }\nfn main() {\n  stage svc : fifo;\n  server (work) { run svc (cost(svc, work)); }\n}\n",
        "workload { arrive poisson(1); turn { set s = twice(1); } send (work: s); }\n",
        &["w.sq: 1:", "`twice` is the deployment file's"],
    );
}

/// Each file runs on its own stages.
#[test]
fn each_file_runs_on_its_own_stages() {
    // the workload on a stage of the deployment
    refused(
        "fn main() {\n  stage svc : fifo;\n  stage wait : delay;\n  server (work) { run svc (cost(svc, work)); }\n}\n",
        "workload { arrive poisson(1); turn { set s = 1; } send (work: s);\n  session { turn; run wait (cost(wait, 1)); } }\n",
        &["w.sq: 2:", "stage `wait` is the deployment file's"],
    );
    // the server on a stage of the workload
    refused(
        "fn main() {\n  stage svc : fifo;\n  server (work) { run user (cost(user, work)); run svc (cost(svc, 1)); }\n}\n",
        "stage user : delay;\nworkload { arrive poisson(1); turn { set s = 1; } send (work: s); }\n",
        &["d.sq: 3:", "stage `user` is the workload file's"],
    );
}

/// Each file's names are its own: both may declare `p`, each reading its own.
#[test]
fn both_files_may_declare_a_name() {
    compile_paired(
        &format!("let rate = 2;\n{DEPLOYMENT}"),
        WORKLOAD,
        names(),
        &common::horizon(10.0),
    )
    .unwrap();
}

/// A position a message quotes is in its own file.
#[test]
fn a_quoted_position_is_in_its_file() {
    let err = compile_paired(
        "fn main() {\n  stage svc : fifo;\n  server (work) { run svc (cost(svc, work)); }\n  claim c : at end (rate > 0);\n}\n",
        WORKLOAD,
        names(),
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(!err.contains("6:5") && !err.contains("w·"), "{err}");
}

#[test]
fn an_empty_or_unclosed_file_is_its_own() {
    refused(
        DEPLOYMENT,
        "use \"std/args\";\n",
        &["w.sq: 1:1:", "a workload file declares `workload { … }`"],
    );
    refused(
        "fn main() {\n  stage svc : fifo;\n  server (work) { run svc (cost(svc, work)); }\n",
        WORKLOAD,
        &["d.sq: ", "unclosed `fn main()`"],
    );
}

/// `serq fmt` checks half of a pair for its syntax.
#[test]
fn a_half_is_formatted_only_if_it_parses() {
    serq::frontend::fmt::format(WORKLOAD).unwrap();
    serq::frontend::fmt::format(DEPLOYMENT).unwrap();
    serq::frontend::fmt::format("workload ;;; }}} {{ let let 1 2 3").unwrap_err();
    serq::frontend::fmt::format("fn main ( ) { stage svc : fifo").unwrap_err();
}

/// A local name of one file is no read of the other's constant.
#[test]
fn a_local_name_is_its_own() {
    let deployment = "fn main() {
  device gpu { compute (k) = k * 2; }
  engine e on gpu { tokens cap inf;
    scheduler { advance running; admit waiting while (running.preempted == 0); }
    worker { forward (compute(batch.tokens)); } }
  server (work) { e (work, 1); }
}
";
    let workload =
        "let k = 3;\nworkload { arrive poisson(1); turn { set s = k; } send (work: s); }\n";
    compile_paired(deployment, workload, names(), &common::horizon(10.0)).unwrap();
}

/// Each file's outline is in its own text.
#[test]
fn a_pair_has_two_outlines() {
    let (d, w) = outline_paired(DEPLOYMENT, WORKLOAD).unwrap();
    let server = d.server.unwrap();
    assert!(DEPLOYMENT[server.range.0..].starts_with("server (work)"));
    assert!(d.workload.is_none());
    let range = w.workload.unwrap();
    assert!(WORKLOAD[range.0..range.1].starts_with("workload {"));
    let decl = w.decls.iter().find(|x| x.name == "rate").unwrap();
    assert_eq!(&WORKLOAD[decl.range.0..decl.range.1], "let rate = 1;");
    // the workload's statements are the workload file's, in its own text
    assert!(d.workload_items.is_empty());
    let text = |r: serq::frontend::outline::Range| &WORKLOAD[r.0..r.1];
    let items: Vec<_> = w
        .workload_items
        .iter()
        .map(|i| (i.kind.as_str(), text(i.range), i.value.map(text)))
        .collect();
    assert_eq!(
        items,
        [
            ("arrive", "arrive poisson(rate);", Some("poisson(rate)")),
            ("set", "set s = ~exp(1);", Some("~exp(1)")),
            ("send", "send (work: s);", Some("work: s")),
        ]
    );
}
