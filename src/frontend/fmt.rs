//! Conservative source formatter. The lexer supplies the original spelling
//! and comment positions; the parser has already checked the source before
//! this module changes whitespace.

use crate::frontend::lexer::{Tok, Token, lex};
use crate::frontend::parser;

fn is_close(t: &Tok) -> bool {
    matches!(
        t,
        Tok::RParen | Tok::RBracket | Tok::Comma | Tok::Semi | Tok::Dot
    )
}

fn gap(prev: &Token, next: &Token) -> &'static str {
    let had_space = next.col > prev.col + prev.text.chars().count();
    if is_close(&next.tok)
        || matches!(
            prev.tok,
            Tok::LParen | Tok::LBracket | Tok::Dot | Tok::Tilde
        )
    {
        return "";
    }
    if matches!(next.tok, Tok::LBracket | Tok::Dot) {
        return "";
    }
    if matches!(next.tok, Tok::LParen) {
        return if had_space { " " } else { "" };
    }
    if matches!(
        prev.tok,
        Tok::Comma | Tok::Semi | Tok::Colon | Tok::LBrace | Tok::RBrace
    ) || matches!(next.tok, Tok::Assign | Tok::LBrace | Tok::RBrace)
        || matches!(prev.tok, Tok::Assign)
    {
        return " ";
    }
    // Preserve a tight unary minus and the spelling of expressions; a later
    // CST formatter can make wider layout decisions without guessing here.
    if had_space { " " } else { "" }
}

fn format_code(tokens: &[&Token]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i > 0 {
            out.push_str(gap(tokens[i - 1], token));
        }
        out.push_str(&token.text);
    }
    out
}

/// Format program text while keeping comments, blank lines, token spellings,
/// and line breaks. Formatting twice has the same result as formatting once.
pub fn format(src: &str) -> Result<String, String> {
    format_at(src, None)
}

/// Format the text of a program file in `base`, whose `use`s are read to
/// check the text parses.
pub fn format_at(src: &str, base: Option<&std::path::Path>) -> Result<String, String> {
    parser::parse_at(src, base).map_err(|e| e.render(src))?;
    let tokens = lex(src).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = src.lines().collect();
    let mut by_line: Vec<Vec<&Token>> = vec![vec![]; lines.len() + 1];
    let mut commented = vec![false; lines.len() + 1];
    for token in &tokens {
        if token.tok != Tok::Eof && token.line <= lines.len() {
            by_line[token.line].push(token);
        }
        for tr in &token.leading {
            if tr.is_comment() {
                let end = tr.line + tr.text.matches('\n').count();
                for marked in &mut commented[tr.line..=end.min(lines.len())] {
                    *marked = true;
                }
            }
        }
    }

    let mut out = String::new();
    let mut depth = 0usize;
    let mut admit_base: Option<usize> = None;
    let mut where_started = false;
    let mut previous_comma = false;
    let mut continuation = false;
    for (i, raw) in lines.iter().enumerate() {
        let line = i + 1;
        let row = &by_line[line];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        let closing = row.first().is_some_and(|t| t.tok == Tok::RBrace);
        let base = depth.saturating_sub(usize::from(closing)) * 2;
        let first = row.first().map(|t| t.text.as_str()).unwrap_or("");
        if first == "admit" {
            admit_base = Some(base);
            where_started = false;
        }
        let original_indent = raw.chars().take_while(|c| c.is_whitespace()).count();
        let indent = if row.is_empty() && commented[line] && original_indent > base {
            // An aligned continuation of a trailing comment (for example a
            // wrapped explanation under a `let`) belongs to that comment.
            original_indent
        } else if let Some(admit) = admit_base {
            if matches!(first, "where" | "reuse" | "reserve") {
                if first == "where" {
                    where_started = true;
                }
                admit + 6
            } else if where_started && previous_comma && !closing {
                admit + 12
            } else if continuation && !closing && first != "admit" {
                original_indent.max(base)
            } else {
                base
            }
        } else if continuation && !closing && !row.is_empty() {
            original_indent.max(base)
        } else {
            base
        };
        out.push_str(&" ".repeat(indent));
        if commented[line] || row.is_empty() {
            out.push_str(raw.trim_start().trim_end());
        } else {
            out.push_str(&format_code(row));
        }
        out.push('\n');

        for token in row {
            match token.tok {
                Tok::LBrace => depth += 1,
                Tok::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if row.iter().any(|t| t.tok == Tok::LBrace) {
            admit_base = None;
            where_started = false;
        }
        if let Some(last) = row.last() {
            previous_comma = last.tok == Tok::Comma;
            continuation = !matches!(last.tok, Tok::Semi | Tok::LBrace | Tok::RBrace)
                && !raw.trim_end().ends_with(';');
        }
    }
    // Keep an empty input empty; all nonempty formatted files end in a newline.
    if src.is_empty() {
        out.clear();
    }
    parser::parse_at(&out, base)
        .map_err(|e| format!("formatter produced invalid syntax: {}", e.render(&out)))?;
    let formatted = lex(&out).map_err(|e| e.to_string())?;
    let before: Vec<&Tok> = tokens.iter().map(|t| &t.tok).collect();
    let after: Vec<&Tok> = formatted.iter().map(|t| &t.tok).collect();
    if before != after {
        return Err("formatter changed the token stream".into());
    }
    Ok(out)
}
