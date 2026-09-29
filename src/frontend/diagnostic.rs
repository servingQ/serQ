//! Source locations belong to the text frontend, never to the serialized IR.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    /// Length in Unicode scalar values, as in the lexer's columns.
    pub len: usize,
}

impl Span {
    pub fn render(self, source: &str, message: &str) -> String {
        let (first, rest) = message.split_once('\n').unwrap_or((message, ""));
        let line = source
            .split('\n')
            .nth(self.line.saturating_sub(1))
            .unwrap_or("");
        let width = self.line.to_string().len();
        let indent: String = line
            .chars()
            .take(self.col.saturating_sub(1))
            .map(|c| if c == '\t' { '\t' } else { ' ' })
            .collect();
        let mut out = format!(
            "{}:{}: {first}\n{:width$} | {line}\n{:width$} | {indent}{}",
            self.line,
            self.col,
            self.line,
            "",
            "^".repeat(self.len.max(1))
        );
        if !rest.is_empty() {
            out.push('\n');
            out.push_str(rest);
        }
        out
    }
}

/// Suggest only a unique, nearby name. Sort first so HashMap iteration cannot
/// decide which correction the user sees. Distance is over Unicode characters.
pub(crate) fn suggestion<'a>(
    name: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<String> {
    let mut candidates: Vec<_> = candidates.collect();
    candidates.sort_unstable();
    candidates.dedup();
    let input: Vec<_> = name.chars().collect();
    let mut scored: Vec<_> = candidates
        .into_iter()
        .map(|candidate| {
            let mut row: Vec<usize> = (0..=input.len()).collect();
            for (i, c) in candidate.chars().enumerate() {
                let mut diagonal = row[0];
                row[0] = i + 1;
                for (j, n) in input.iter().enumerate() {
                    let above = row[j + 1];
                    row[j + 1] = (row[j] + 1)
                        .min(above + 1)
                        .min(diagonal + usize::from(c != *n));
                    diagonal = above;
                }
            }
            (row[input.len()], candidate)
        })
        .collect();
    scored.sort_unstable();
    let &(distance, candidate) = scored.first()?;
    let limit = if input.len() < 4 { 1 } else { 2 };
    if distance <= limit && scored.get(1).is_none_or(|x| x.0 > distance) {
        Some(candidate.to_string())
    } else {
        None
    }
}
