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

/// The codes, each written once: its variant, its name and what it means.
macro_rules! codes {
    ($($(#[doc = $doc:literal])* $variant:ident => $name:literal,)*) => {
        /// A rule whose refusal an editor recognises by its code, not its
        /// wording (#645): the message's first line starts `error[E-…]: `.
        /// The one list of codes.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Code {
            $($(#[doc = $doc])* $variant,)*
        }

        impl Code {
            pub const ALL: &[Code] = &[$(Code::$variant),*];

            pub fn name(self) -> &'static str {
                match self {
                    $(Code::$variant => $name,)*
                }
            }
        }
    };
}

codes! {
    /// A pull's source called in a leg nothing joins.
    PullForked => "E-pull-forked",
    /// A KV relation whose reader has no connector to load what it copies.
    CopyNoConnector => "E-copy-no-connector",
    /// A connector that loads into a memory the engine's `taking` does not
    /// take.
    ConnectorUntaken => "E-connector-untaken",
    /// A queue's NIC that no copy runs on.
    NicUnused => "E-nic-unused",
    /// A name the language uses for itself, given to something the program
    /// declares.
    ReservedWord => "E-reserved-word",
    /// Two pools on the device an engine runs on, which holds one: the engine's KV.
    DeviceTwoPools => "E-device-two-pools",
    /// A pool on a device or engine that declares no capacity of its name.
    PoolNoCapacity => "E-pool-no-capacity",
    /// A KV relation whose reader has no entry that takes the KV from its
    /// source.
    CopyNoEntry => "E-copy-no-entry",
    /// A KV relation written reader first: the source has the entry `…
    /// from` the other.
    CopySwapped => "E-copy-swapped",
    /// An engine's connector that names another queue than the one its
    /// queue's relation reads from.
    ConnectorMisnamed => "E-connector-misnamed",
    /// A second KV relation into one reader.
    ReaderTwoSources => "E-reader-two-sources",
    /// An engine's connector to a queue its queue copies no KV from.
    ConnectorNoCopy => "E-connector-no-copy",
}

impl Code {
    /// The message with its code: `error[E-…]: message`.
    pub fn message(self, message: impl std::fmt::Display) -> String {
        format!("error[{}]: {message}", self.name())
    }

    /// The code a rendered message's first line carries (`L:C: error[E-…]: …`).
    pub fn of(message: &str) -> Option<Code> {
        let first = message.lines().next()?;
        let name = first.split_once("error[")?.1.split_once("]: ")?.0;
        Code::ALL.iter().copied().find(|c| c.name() == name)
    }
}

/// A byte range of the text, and what to write there instead.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Edit {
    pub range: crate::frontend::outline::Range,
    pub text: String,
}

/// A code's fix-it (`docs/reference/diagnostics.md`) as edits of the text.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Fix {
    pub label: String,
    pub edits: Vec<Edit>,
}

/// Why a text does not compile, for an editor (`serq::diagnose`): the
/// message `serq check` prints, its code, its place, and its fix-it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Diagnostic {
    pub code: Option<&'static str>,
    pub message: String,
    /// A pair's file the place is in, `deployment` or `workload`; null for
    /// one text, or a refusal with no place.
    pub file: Option<&'static str>,
    pub line: Option<usize>,
    pub col: Option<usize>,
    pub range: Option<crate::frontend::outline::Range>,
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a diagnostic serializes")
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
