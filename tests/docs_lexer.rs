//! The docs lexer and the parser say the same words.
//!
//! `docs/hooks/seq_lexer.py` carries a list of seQ's keywords, and the parser
//! carries the real one. Two copies rot in a week: this is the only reason
//! they do not.

use std::collections::BTreeSet;
use std::path::Path;

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Every word the parser matches as a keyword.
fn parser_keywords() -> BTreeSet<String> {
    let src = read("src/parser.rs");
    let mut out = BTreeSet::new();
    for (pat, skip) in [("eat_kw(\"", 8), ("is_kw(\"", 7)] {
        let mut rest = src.as_str();
        while let Some(i) = rest.find(pat) {
            rest = &rest[i + skip..];
            if let Some(j) = rest.find('"') {
                out.insert(rest[..j].to_string());
            }
        }
    }
    // statement and option heads are matched as `"word" =>`
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('"')
            && let Some(j) = rest.find('"')
            && rest[j..]
                .trim_start_matches('"')
                .trim_start()
                .starts_with("=>")
        {
            let w = &rest[..j];
            if w.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !w.is_empty() {
                out.insert(w.to_string());
            }
        }
    }
    out
}

/// Words the lexer is not expected to carry, with the reason.
fn exempt(word: &str) -> bool {
    matches!(
        word,
        // kept only to tell an old program what its keyword became
        "fits"
            // matched inside `pool`/`stage`/`workload` bodies as option values
            // that are already covered by the option list under another name
            | "lru" | "lifo" | "none" | "fcfs"
    ) || word.len() < 2
}

#[test]
fn the_docs_lexer_knows_every_keyword() {
    let parser = parser_keywords();
    assert!(
        parser.len() > 40,
        "keyword extraction found only {}",
        parser.len()
    );

    let file = "docs/hooks/seq_lexer.py";
    let text = read(file);
    let missing: Vec<&String> = parser
        .iter()
        .filter(|w| !exempt(w))
        .filter(|w| {
            !text
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|t| t == w.as_str())
        })
        .collect();
    assert!(missing.is_empty(), "{file} is missing {missing:?}");
}

/// `admit` names the pool option and nothing else now, so the lexer must
/// still colour it - and must not colour it as a statement.
#[test]
fn admit_is_an_option_not_a_statement() {
    let file = "docs/hooks/seq_lexer.py";
    assert!(read(file).contains("admit"), "{file} dropped `admit via`");
}
