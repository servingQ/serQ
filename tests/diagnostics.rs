//! The error codes an editor recognises a refusal by (#645): the table in
//! `docs/reference/diagnostics.md` is a copy of `Code::ALL`, held equal
//! here, both ways.

use std::collections::BTreeSet;
use std::path::Path;

use serq::frontend::diagnostic::Code;

#[test]
fn the_documented_codes_are_the_codes() {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/diagnostics.md"),
    )
    .unwrap();
    let documented: BTreeSet<&str> = doc
        .lines()
        .filter_map(|l| l.strip_prefix("| `E-"))
        .map(|l| &l[..l.find('`').unwrap()])
        .collect();
    let codes: BTreeSet<&str> = Code::ALL
        .iter()
        .map(|c| c.name().strip_prefix("E-").unwrap())
        .collect();
    assert_eq!(documented, codes);
}
