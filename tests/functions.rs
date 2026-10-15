//! The functions a program may call, as an editor reads them
//! (`frontend::highlight::functions`, #645 B7): the table `ir::Fun` and
//! its summaries are the one list, and `docs/api/functions.md` a copy of
//! it, held equal here both ways: each function's row of the index and its
//! own section say its parameters, their kinds, its group and what it
//! returns, and each row of the index is a function, or a word another
//! table supplies (`blocksize`, `total`, …).

use std::path::Path;

use serq::frontend::highlight::{Function, functions};
use serq::frontend::link::{AGGREGATES, FOLDED};

fn doc() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/api/functions.md"))
        .unwrap()
}

/// `name(a: expr, b: pool)`, as the reference writes a signature.
fn signature(f: &Function) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.kind))
        .collect();
    format!("{}({})", f.name, params.join(", "))
}

/// One row of the index: its group (the section it is under), its
/// signature (an optional argument, the parser's sugar, left out), and
/// what it returns.
struct Row {
    group: String,
    name: String,
    signature: String,
    summary: String,
}

fn index() -> Vec<Row> {
    let doc = doc();
    let mut group = String::new();
    let mut rows = vec![];
    for line in doc.lines() {
        if line.starts_with("## ") && !line.starts_with("## Function index") {
            break;
        }
        // `**[Arithmetic](#arithmetic)**`
        if let Some(g) = line.strip_prefix("**[") {
            group = g[..g.find(']').unwrap()].to_lowercase();
            continue;
        }
        // | [`min(a: expr, b: expr)`](#min) | The smaller argument. |
        let Some(rest) = line.strip_prefix("| [`") else {
            continue;
        };
        let (sig, rest) = rest.split_once("`](#").unwrap();
        let summary = rest.split_once(") | ").unwrap().1;
        let summary = summary.strip_suffix(" |").unwrap().to_string();
        let signature = match sig.split_once(" [") {
            Some((head, _)) => format!("{head})"),
            None => sig.to_string(),
        };
        rows.push(Row {
            group: group.clone(),
            name: sig[..sig.find('(').unwrap()].to_string(),
            signature,
            summary,
        });
    }
    assert!(rows.len() > 20, "read {} rows", rows.len());
    rows
}

#[test]
fn every_function_is_its_row_of_the_index() {
    let rows = index();
    for f in functions() {
        let row = rows
            .iter()
            .find(|r| r.name == f.name)
            .unwrap_or_else(|| panic!("`{}` has no row in docs/api/functions.md", f.name));
        assert_eq!(row.signature, signature(&f));
        assert_eq!(row.group, f.group, "`{}`'s group", f.name);
        assert_eq!(row.summary, f.summary, "`{}`'s summary", f.name);
    }
}

/// Its own section: the signature first in its `text` block, and the
/// summary the first sentence of its `Returns`.
#[test]
fn every_function_is_its_section_of_the_reference() {
    let doc = doc();
    for f in functions() {
        let head = format!("{{#{}}}", f.name);
        let at = doc
            .find(&head)
            .unwrap_or_else(|| panic!("`{}` has no section", f.name));
        let section = &doc[at..];
        let block = section.split("```text\n").nth(1).unwrap();
        let first = block.lines().next().unwrap();
        assert_eq!(first, format!("{} -> number", signature(&f)));
        let returns = section
            .lines()
            .find_map(|l| l.strip_prefix("**Returns:** "))
            .unwrap();
        assert!(
            returns.starts_with(f.summary),
            "`{}`: `Returns` is\n  {returns}\nnot\n  {}",
            f.name,
            f.summary
        );
    }
}

#[test]
fn every_row_of_the_index_is_a_word_of_the_language() {
    let names: Vec<&str> = functions().iter().map(|f| f.name).collect();
    for r in index() {
        assert!(
            names.contains(&r.name.as_str())
                || FOLDED.contains(&r.name.as_str())
                || AGGREGATES.iter().any(|(a, _)| *a == r.name),
            "`{}` is in docs/api/functions.md and no table of the language",
            r.name
        );
    }
}
