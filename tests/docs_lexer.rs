//! The docs lexer and the parser say the same words.
//!
//! `docs/hooks/serq_lexer.py` carries a list of serQ's keywords, and the parser
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
    let all = read("src/frontend/parser.rs");
    // the tests quote words that are not keywords
    let src = all[..all.find("#[cfg(test)]").unwrap_or(all.len())].to_string();
    let mut out = BTreeSet::new();
    for (pat, skip) in [("eat_kw(\"", 8), ("is_kw(\"", 7), ("expect_kw(\"", 11)] {
        let mut rest = src.as_str();
        while let Some(i) = rest.find(pat) {
            rest = &rest[i + skip..];
            if let Some(j) = rest.find('"') {
                out.insert(rest[..j].to_string());
            }
        }
    }
    // statement and option heads are matched as `"word" =>`, or as
    // alternatives `"a" | "b" =>`
    for line in src.lines() {
        let t = line.trim();
        let Some((arm, _)) = t.split_once("=>") else {
            continue;
        };
        let words: Vec<&str> = arm.split('|').map(str::trim).collect();
        if words
            .iter()
            .all(|w| w.len() > 2 && w.starts_with('"') && w.ends_with('"'))
        {
            for w in words {
                let w = &w[1..w.len() - 1];
                if w.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !w.is_empty() {
                    out.insert(w.to_string());
                }
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

    let file = "docs/hooks/serq_lexer.py";
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
    let file = "docs/hooks/serq_lexer.py";
    assert!(read(file).contains("admit"), "{file} dropped `admit via`");
}

/// A `def` and its parameters may not be keywords, which the parser checks
/// against its own list: that list is every word it matches.
#[test]
fn the_parser_keyword_list_is_every_keyword() {
    let listed: BTreeSet<String> = serq::frontend::parser::KEYWORDS
        .iter()
        .map(|w| w.to_string())
        .collect();
    let missing: Vec<String> = parser_keywords()
        .into_iter()
        .filter(|w| w.len() >= 2 && !listed.contains(w))
        .collect();
    assert!(
        missing.is_empty(),
        "parser::KEYWORDS is missing {missing:?}"
    );
}

/// A `def` may not be named like a function: `link::FUNCTIONS` is every
/// function a call may name, the IR's table (`Fun::names`) by which the
/// linker resolves a call and `Program::validate` checks one.
#[test]
fn the_function_list_is_every_function_the_linker_resolves() {
    let resolved: BTreeSet<&str> = serq::ir::Fun::names().collect();
    let listed: BTreeSet<&str> = serq::frontend::link::FUNCTIONS.iter().copied().collect();
    assert_eq!(resolved, listed);
}

/// The names the language supplies - context variables, functions, a run's
/// aggregates, folded calls and the attributes it sets - are coloured as
/// what a program reads. Since #231 none of them can be a program's own, so
/// the lexer has no reason to leave one plain.
#[test]
fn the_docs_lexer_knows_every_name_the_language_supplies() {
    use serq::frontend::link::{AGGREGATES, BUILTIN_ATTRS, CONTEXT_VARS, FOLDED, FUNCTIONS};
    let supplied = CONTEXT_VARS
        .iter()
        .map(|(n, _)| *n)
        .chain(FUNCTIONS)
        .chain(AGGREGATES.iter().map(|(n, _)| *n))
        .chain(FOLDED)
        // `new` and `out` stay plain: a program also names an `observe` so
        .chain(
            BUILTIN_ATTRS
                .into_iter()
                .filter(|a| !matches!(*a, "new" | "out")),
        )
        .chain(["inf"]);

    let file = "docs/hooks/serq_lexer.py";
    let text = read(file);
    let quoted: BTreeSet<&str> = text.split('"').skip(1).step_by(2).collect();
    let missing: Vec<&str> = supplied.filter(|w| !quoted.contains(w)).collect();
    assert!(missing.is_empty(), "{file} does not colour {missing:?}");
}
