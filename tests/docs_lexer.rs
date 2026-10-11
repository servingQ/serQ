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

/// The parser's sources: `parser.rs` and its submodules (`parser/device.rs`
/// reads the engine form), each without its tests, which quote words that are
/// not keywords.
fn parser_sources() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/frontend/parser");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            format!("src/frontend/parser/{name}")
        })
        .collect();
    assert!(
        !files.is_empty(),
        "no parser submodule in {}",
        dir.display()
    );
    files.sort();
    files.insert(0, "src/frontend/parser.rs".to_string());
    files
        .iter()
        .map(|f| {
            let all = read(f);
            all[..all.find("#[cfg(test)]").unwrap_or(all.len())].to_string()
        })
        .collect()
}

/// Every word the parser matches as a keyword.
fn parser_keywords() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for src in parser_sources() {
        keywords_in(&src, &mut out);
    }
    out
}

fn keywords_in(src: &str, out: &mut BTreeSet<String>) {
    for pat in ["eat_kw(\"", "is_kw(\"", "expect_kw(\""] {
        let mut rest = src;
        while let Some(i) = rest.find(pat) {
            rest = &rest[i + pat.len()..];
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
}

const LEXER: &str = "docs/hooks/serq_lexer.py";

/// The words of the lexer's tuple `NAME = (…)`, comments left out.
fn lexer_list(name: &str) -> BTreeSet<String> {
    let text = read(LEXER);
    let head = format!("\n{name} = (");
    let at = text
        .find(&head)
        .unwrap_or_else(|| panic!("{LEXER} has no `{name} = (`"));
    let body = &text[at + head.len()..];
    let body = &body[..body
        .find("\n)")
        .or_else(|| body.find(')'))
        .expect("a closing paren")];
    let code: String = body
        .lines()
        .map(|l| l.split_once('#').map_or(l, |(c, _)| c))
        .collect::<Vec<_>>()
        .join("\n");
    code.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Every word the lexer colours by name.
fn coloured() -> BTreeSet<String> {
    [
        "STRUCTURE",
        "STATEMENTS",
        "OPTIONS",
        "BUILTINS",
        "AGGREGATES",
    ]
    .into_iter()
    .flat_map(lexer_list)
    .collect()
}

/// Words the lexer is not expected to list, with the reason.
fn exempt(word: &str) -> bool {
    matches!(
        word,
        // kept only to tell an old program what its keyword became
        "fits"
            // a step stage's retired options, kept only to say what an
            // engine writes instead (`parser/device.rs`)
            | "budget" | "chunk" | "memory"
            // a distribution, which the `~name` rule colours
            | "exp" | "det" | "uniform" | "erlang" | "h2" | "bernoulli"
            // an engine's lists, read as `running.count`, which the
            // ENGINE_VALUES rule colours whole
            | "running"
    ) || word.len() < 2
}

/// Every word the parser matches, and every word of `parser::KEYWORDS`, is
/// in one of the lexer's lists.
#[test]
fn the_docs_lexer_knows_every_keyword() {
    let parser = parser_keywords();
    assert!(
        parser.len() > 40,
        "keyword extraction found only {}",
        parser.len()
    );
    let listed = coloured();
    let missing: BTreeSet<&str> = parser
        .iter()
        .map(String::as_str)
        .chain(serq::frontend::parser::KEYWORDS)
        .filter(|w| !exempt(w) && !listed.contains(*w))
        .collect();
    assert!(missing.is_empty(), "{LEXER} is missing {missing:?}");
}

/// A `def` and its parameters may not be keywords, which the parser checks
/// against its own list: that list is every word it matches, but for the
/// names a program reads, which a body's reads must still see.
#[test]
fn the_parser_keyword_list_is_every_keyword() {
    let listed: BTreeSet<&str> = serq::frontend::parser::KEYWORDS.into_iter().collect();
    let missing: Vec<String> = parser_keywords()
        .into_iter()
        .filter(|w| w.len() >= 2 && !listed.contains(w.as_str()))
        // `running.count`, `waiting.count`, and the context variable `tokens`
        // of `tokens cap`: a value, not only a keyword
        .filter(|w| !matches!(w.as_str(), "running" | "waiting" | "tokens"))
        .collect();
    assert!(
        missing.is_empty(),
        "parser::KEYWORDS is missing {missing:?}"
    );
}

/// An engine's values (`running.count`, `waiting.count`, `batch.tokens`)
/// are what an engine reads, coloured whole: the lexer's list is the
/// parser's `ENGINE_VALUES`, no more and no fewer.
#[test]
fn the_docs_lexer_knows_every_engine_value() {
    let parser: BTreeSet<String> = serq::frontend::parser::ENGINE_VALUES
        .iter()
        .map(|(v, ..)| v.to_string())
        .collect();
    assert_eq!(
        lexer_list("ENGINE_VALUES"),
        parser,
        "{LEXER}: ENGINE_VALUES"
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
/// what a program reads, or, for arithmetic, listed to be left plain. Since
/// #231 a `set`, `choose` or `let` may not take one, but an `observe`, a
/// stage, a pool or a `def` still may: a name a shipped program declares as
/// its own stays plain until #460 refuses that.
#[test]
fn the_docs_lexer_knows_every_name_the_language_supplies() {
    use serq::frontend::link::{AGGREGATES, BUILTIN_ATTRS, CONTEXT_VARS, FOLDED, FUNCTIONS};
    let supplied = CONTEXT_VARS
        .iter()
        .map(|(n, _)| *n)
        // examples/vendors/ascend.sq: `observe admitted = now;` (#460)
        .filter(|n| *n != "admitted")
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

    let mut listed = coloured();
    listed.extend(lexer_list("ARITHMETIC"));
    let missing: Vec<&str> = supplied.filter(|w| !listed.contains(*w)).collect();
    assert!(missing.is_empty(), "{LEXER} does not colour {missing:?}");
}
