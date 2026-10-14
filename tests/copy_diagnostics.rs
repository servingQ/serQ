//! A KV relation the program declares but does not run: the refusal names
//! the relation and what to write, not what the linker made of it (a
//! `join` the program never wrote, a NIC the relation was meant to use).
//! Each case is an example with one thing taken out, as found (#645).

use serq::{Overrides, check_source};

fn example(path: &str) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn refused(src: &str) -> String {
    match check_source(src, &Overrides::default()) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted:\n{src}"),
    }
}

/// `vllm_nixl_push.sq` with its reader's connector taken out.
const CONNECTOR: &str =
    "      connector P { load (request.preempted ? 0 : request.input - request.hit); }\n";

#[test]
fn a_push_whose_source_is_not_forked_names_the_push() {
    let src = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let forked = "      if (concurrent) {
        fork { P.prefill (prompt); }     // asyncio.create_task: the prefill leg runs beside
      } else {
        P.prefill (prompt);              // the sidecar's order: the prefill's answer first
      }";
    assert!(src.contains(forked));
    let e = refused(&src.replace(forked, "      P.prefill (prompt);"));
    // not the `join` the connector adds, which the program never wrote
    assert!(!e.contains("forks no leg"), "{e}");
    assert!(
        e.contains("`kvt.nixl_push (P, D)`: in a push `P` posts the copy"),
        "{e}"
    );
    assert!(e.contains("help: call `P` in a leg"), "{e}");
    assert!(e.contains("`fork { P.prefill (…); }`"), "{e}");
}

#[test]
fn a_push_whose_reader_has_no_connector_names_the_connector() {
    let src = example("examples/pd-disaggregation/vllm_nixl_push.sq");
    let parked = "      observe parked = D.load_done - D.admitted;   // how long the decoder's blocks waited for the KV\n";
    assert!(src.contains(CONNECTOR) && src.contains(parked));
    let e = refused(&src.replace(CONNECTOR, "").replace(parked, ""));
    // not the `fork` the program wrote, which the missing `join` left alone
    assert!(!e.contains("never joins"), "{e}");
    assert!(e.contains("`D`'s engine has no `connector P`"), "{e}");
    assert!(
        e.contains("help: in `D`'s engine, write `connector P { load (…); }`"),
        "{e}"
    );
}

#[test]
fn a_pull_whose_reader_has_no_connector_names_the_connector() {
    let src = example("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let lease = "        observe lease = D.load_done - t1;\n";
    assert!(src.contains(CONNECTOR) && src.contains(lease));
    let e = refused(&src.replace(CONNECTOR, "").replace(lease, ""));
    // not "declare `kvt.nixl_pull`", which the program declares
    assert!(!e.contains("carries no copy"), "{e}");
    assert!(
        e.contains("`kvt.nixl_pull (P, D)` copies the KV `P` leases to `D`"),
        "{e}"
    );
    assert!(e.contains("`D`'s engine has no `connector P`"), "{e}");
}
