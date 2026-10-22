//! A KV relation the program declares but does not run: the refusal names
//! the relation and what to write, not what the linker made of it (a NIC
//! the relation was meant to use). Each case is an example with one thing
//! taken out, as found (#645). And a push dispatched in order, which is no
//! refusal: the reader's `join` waits for a leg only where one is forked
//! (#653 refused it, from the `join` the linker added).

use serq::Overrides;
use std::path::{Path, PathBuf};

mod common;

/// An example's deployment, and the workload file `examples/pairs.json`
/// pairs it with.
fn example(path: &str) -> (String, PathBuf) {
    let workload = serq::paired_workload(Path::new(path)).expect("a pair");
    (std::fs::read_to_string(path).unwrap(), workload)
}

fn refused(src: &str, workload: &Path) -> String {
    let w = std::fs::read_to_string(workload).unwrap();
    match common::inspected(src, &w, &Overrides::default()) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted:\n{src}"),
    }
}

/// Run a deployment under the workload file `workload`, its trace read
/// beside that file.
fn run(src: &str, workload: &Path, ov: &Overrides) -> serq::Report {
    let w = std::fs::read_to_string(workload).unwrap();
    serq::run_paired(src, &w, common::NAMES, ov, workload.parent()).unwrap()
}

/// `vllm_nixl_push.sq` with its reader's connector taken out.
const CONNECTOR: &str =
    "      connector P { load (request.preempted ? 0 : request.input - request.hit); }\n";

/// `vllm_nixl_push.sq` written with no `fork` anywhere: the decode request
/// after the prefill's answer, as the example's `concurrent = 0` does. It
/// links, and runs as `concurrent = 0` does, where the `fork` it never
/// reaches is still written.
#[test]
fn a_push_dispatched_in_order_runs_as_concurrent_0() {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let forked = "      if (concurrent) {
        fork { P.prefill (prompt); }     // asyncio.create_task: the prefill leg runs beside
      } else {
        P.prefill (prompt);              // the sidecar's order: the prefill's answer first
      }";
    let one = "let concurrent = 1;";
    assert!(src.contains(forked) && src.contains(one));
    let in_order = src.replace(forked, "      P.prefill (prompt);");
    let ov = common::horizon(300.0);
    let unforked = run(&in_order, &workload, &ov);
    let sequential = run(&src.replace(one, "let concurrent = 0;"), &workload, &ov);
    assert!(unforked.stages_named("D").iter().any(|d| d.completed > 0));
    assert_eq!(unforked.text(), sequential.text());
}

/// A push whose reader copies with its entry's `transfer … from P`, no
/// connector, and forks nothing: it linked and ran before #653, and does
/// again (`tests/queues.rs` `a_pull_relation_is_the_flat_read`, pushed).
#[test]
fn a_push_read_by_transfer_without_a_fork_runs() {
    let src = common::main_source(
        "
      let x0 = 0.5;
      queue gw : gateway { route {
        P[i].prefill (prompt);
        D[j].decode (prompt) from P[i];
      } }
      queue P[2] : prefill {
        pool kv { cap 1000; }
        serve fifo;
        nic ps(100);
        prefill (prompt) { hold kv (cost(kv, prompt)) { run (cost(P, prompt)); } cache (cost(kv, prompt)) lease kv (inf); }
      }
      queue D[2] : agg, decode {
        device gpu { kv cap 1000; } pool kv on gpu { block 16; }
        engine on gpu { tokens cap inf; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1); } }
        nic ps(200);
        decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; } }
        decode (prompt) from P {
          hold kv (cost(kv, prompt)) {
            transfer (prompt) from P to kv (prompt - 1);
            run D decode (cost(D, o - 1)) growing kv;
          }
        }
      }
      use \"std/kvt\"; use \"std/share\"; kvt.nixl_push (P, D, latency = x0, share = share.maxmin);
      server (prompt, hidden o, i, j) { gw.route(); }
      ",
    );
    let workload = "workload { arrive batch(1); init { set prompt = 32; set o = 4; set i = 1; set j = 0; } send (prompt, o, i, j); session { turn; end; } }";
    let r = common::run(&src, workload, &common::horizon(100.0)).unwrap();
    assert!(r.text().contains("response"), "{}", r.text());
}

#[test]
fn a_push_whose_reader_has_no_connector_names_the_connector() {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let parked = "      observe parked = D.load_done - D.admitted;   // how long the decoder's blocks waited for the KV\n";
    assert!(src.contains(CONNECTOR) && src.contains(parked));
    let e = refused(&src.replace(CONNECTOR, "").replace(parked, ""), &workload);
    // not the `fork` the program wrote, which the missing `join` left alone
    assert!(!e.contains("never joins"), "{e}");
    assert!(e.contains("`D`'s engine has no `connector P`"), "{e}");
    assert!(e.contains(": error[E-copy-no-connector]: "), "{e}");
    // what a request loads is the program's to say: a help, no fix-it that
    // would write one program's policy into another (criterion 2)
    assert!(
        e.contains("help: in `D`'s engine, write `connector P { load (…); }`"),
        "{e}"
    );
    assert!(!e.contains("fix-it"), "{e}");
}

/// A push toggled back to a pull, its source's call still forked: the
/// reader's load waits for the source's send in either mode, so it runs,
/// the same numbers as the push (only who posts the copy differs, which the
/// figure draws). `E-pull-forked` refused it while a pull's reader waited
/// for nothing (#657).
#[test]
fn a_pull_whose_source_is_forked_runs_as_the_push() {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let push = "kvt.nixl_push (P, D,";
    assert!(src.contains(push));
    let ov = common::horizon(300.0);
    let pull = run(&src.replace(push, "kvt.nixl_pull (P, D,"), &workload, &ov);
    assert_eq!(pull.text(), run(&src, &workload, &ov).text());
}

/// The other half of the connector's rule: what it loads, the engine's
/// `taking` takes. `vllm_nixl_push.sq` with `kv` taken out of the decoder's
/// `taking` is refused at `D`, with the pool to take and no amount.
#[test]
fn a_connector_whose_engine_does_not_take_its_memory_names_the_taking() {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let taking =
        "          taking kv (request.known) reserve (request.known),\n                 reqs";
    assert!(src.contains(taking));
    let e = refused(&src.replace(taking, "          taking reqs"), &workload);
    assert!(
        e.contains("error[E-connector-untaken]: engine `D`'s connector"),
        "{e}"
    );
    assert!(
        e.contains(" 104:9: error[E-connector-untaken]"),
        "at `D`: {e}"
    );
    assert!(!e.contains("link error"), "{e}");
    assert!(
        e.contains("help: take `kv` in the `taking` of `D`'s `admit waiting`: `kv (…)`"),
        "{e}"
    );
    assert!(!e.contains("fix-it"), "{e}");
}

#[test]
fn a_pull_whose_reader_has_no_connector_names_the_connector() {
    let (src, workload) = example("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let lease = "        observe send_held = D.load_done - t1;\n";
    assert!(src.contains(CONNECTOR) && src.contains(lease));
    let e = refused(&src.replace(CONNECTOR, "").replace(lease, ""), &workload);
    // not "declare `kvt.nixl_pull`", which the program declares
    assert!(!e.contains("carries no copy"), "{e}");
    assert!(
        e.contains("`kvt.nixl_pull (P, D)` copies the KV `P` leases to `D`"),
        "{e}"
    );
    assert!(e.contains("`D`'s engine has no `connector P`"), "{e}");
}

/// The other way round (#645 A7): a connector whose queue copies no KV
/// from the queue it names. `vllm_nixl_push.sq` without its relation keeps
/// `connector P`, and the help names both relations, which is a choice.
#[test]
fn a_connector_no_relation_copies_for_names_the_relation() {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let relation = "  kvt.nixl_push (P, D, latency = x0, share = share.maxmin);\n";
    assert!(src.contains(relation));
    let e = refused(&src.replace(relation, ""), &workload);
    assert!(
        e.contains(
            "error[E-connector-no-copy]: engine `D`'s `connector P`: queue `D` reads KV from no `P`"
        ),
        "{e}"
    );
    assert!(
        e.contains("`kvt.nixl_pull (P, D, …);` or `kvt.nixl_push (P, D, …);`"),
        "{e}"
    );
    assert!(!e.contains("fix-it"), "{e}");
}

/// `vllm_nixl_push.sq` with `edits`, each `(from, to)` written once.
fn push_with(edits: &[(&str, &str)]) -> (String, PathBuf) {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let src = edits.iter().fold(src, |s, (a, b)| {
        assert!(s.contains(a), "no {a:?}");
        s.replacen(a, b, 1)
    });
    (src, workload)
}

/// A reader with no entry `… from` its source, the relation the right way
/// round (#645 A6): its code, and no fix-it, the entry's body being the
/// program's.
#[test]
fn a_reader_without_its_entry_from_the_source_has_no_fix() {
    let (src, w) = push_with(&[
        (
            "      D.decode (prompt) from P;",
            "      D.decode (prompt);",
        ),
        ("  queue D : decode {", "  queue D : agg {"),
        ("    decode (prompt) from P {", "    decode (prompt) {"),
    ]);
    let e = refused(&src, &w);
    assert!(
        e.contains("error[E-copy-no-entry]: `kvt.nixl_push (P, D)`"),
        "{e}"
    );
    assert!(!e.contains("fix-it") && !e.contains("comes first"), "{e}");
}

/// A swapped copy of a relation written the right way: the swap would be
/// the same relation twice (E-reader-two-sources), so no E-copy-swapped
/// and no fix-it.
#[test]
fn a_swapped_second_relation_is_not_swapped() {
    let rel = "  kvt.nixl_push (P, D, latency = x0, share = share.maxmin);\n";
    let two = format!("{rel}  kvt.nixl_push (D, P, share = share.maxmin);\n");
    let (src, w) = push_with(&[(rel, &two)]);
    let e = refused(&src, &w);
    assert!(e.contains("error[E-copy-no-entry]"), "{e}");
    assert!(!e.contains("fix-it"), "{e}");
}

/// The reader's engine with an L2's connector and a relation: which
/// connector the engine keeps is a choice, so a help that names both ways
/// and no fix-it.
#[test]
fn a_reader_whose_connector_is_an_l2s_has_no_fix() {
    let l2 = "  queue lmc : l2 {\n    device host { kv cap 1e6; }\n    pool kv on host { block bs; \
              evict lru; }\n    serve ps(1);\n  }\n\n  queue D : decode {";
    let (src, w) = push_with(&[
        ("  queue D : decode {", l2),
        (
            "      connector P { load (",
            "      connector lmc { per_token (1e-6); load (",
        ),
    ]);
    let e = refused(&src, &w);
    assert!(e.contains("error[E-copy-no-connector]"), "{e}");
    assert!(
        e.contains(
            "help: an engine has one connector: replace `connector lmc` with `connector P { load \
             (…); }`, or keep it and give `D` an entry that `transfer`s the KV `from P`"
        ),
        "{e}"
    );
    assert!(!e.contains("fix-it"), "{e}");
}
