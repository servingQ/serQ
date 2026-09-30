//! Every `.sq` file in the repository survives a trip through the lexer.
//!
//! `unlex(lex(src)) == src` is what makes a formatter possible: the lexer is
//! the only thing that reads the source, so if it keeps every comment and
//! every blank line, a tool that rewrites a program can put them back.

use serq::frontend::lexer::{lex, unlex};
use std::path::{Path, PathBuf};

fn seq_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            seq_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "sq") {
            out.push(p);
        }
    }
}

#[test]
fn every_program_round_trips_through_the_lexer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    seq_files(&root.join("examples"), &mut files);
    seq_files(&root.join("docs"), &mut files);
    seq_files(&root.join("tests"), &mut files);
    files.sort();
    assert!(files.len() >= 10, "found only {} programs", files.len());
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let toks = lex(&src).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert_eq!(
            unlex(&toks),
            src,
            "{} does not survive lex -> unlex",
            f.display()
        );
    }
}

/// The comments are not decoration: they carry the `scheduler.py:NNN`
/// citations `scripts/check_citations.py` verifies. What the lexer keeps has
/// to be every one of them, not most.
#[test]
fn the_programs_keep_their_citations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    seq_files(&root.join("examples"), &mut files);
    files.sort();
    let mut total = 0usize;
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let want = src.matches(".py:").count();
        let toks = lex(&src).unwrap();
        let got: usize = toks
            .iter()
            .flat_map(|t| &t.leading)
            .filter(|x| x.is_comment())
            .map(|x| x.text.matches(".py:").count())
            .sum();
        assert_eq!(got, want, "{}: citations lost", f.display());
        total += want;
    }
    assert!(total >= 5, "only {total} citations in examples/");
}
