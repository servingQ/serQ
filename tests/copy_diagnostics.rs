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

/// A push toggled back to a pull, its source's call still forked: a pull's
/// connector joins no leg, so the refusal names the fork, not a `join` the
/// program never had (#657).
#[test]
fn a_pull_whose_source_is_forked_names_the_fork() {
    let (src, workload) = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let push = "kvt.nixl_push (P, D,";
    assert!(src.contains(push));
    let e = refused(&src.replace(push, "kvt.nixl_pull (P, D,"), &workload);
    assert!(!e.contains("never joins"), "{e}");
    assert!(
        e.contains("error[E-pull-forked]: `kvt.nixl_pull (P, D)`"),
        "{e}"
    );
    assert!(
        e.contains(" 57:16: error[E-pull-forked]"),
        "at the call: {e}"
    );
    assert!(
        e.contains("help: fix-it: take the call `P.prefill (…);` out of its `fork { … }`\n"),
        "{e}"
    );
    // the other way out is a choice, its own help
    assert!(
        e.contains("help: or make the copy a push: `kvt.nixl_push (P, D, …)`"),
        "{e}"
    );
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
        e.contains(" 99:9: error[E-connector-untaken]"),
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
    let lease = "        observe lease = D.load_done - t1;\n";
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
