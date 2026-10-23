//! A deployment file and a workload file, paired by the tool (`serq run
//! DEPLOYMENT --workload WORKLOAD`, `serq::compile_paired`): the same
//! deployment under another workload, the same workload on another
//! deployment (`docs/design/paired-workload.md`).
//!
//! The deployment file is a program without its `workload`; the workload
//! file has no `fn main()`: its `use`s, its constants and inputs (`let`), its
//! definitions (`def`), the stages it runs on itself (`stage user : delay;`)
//! and `workload { … }`. The two meet only at the server's signature.
//!
//! The parser reads the pair as one program: the workload file's tokens go
//! at the end of `fn main()`'s body (its `use`s before the deployment's), so
//! its inputs are the program's inputs and each declaration keeps its index. Each file's constants and
//! definitions are its own: the pair renames them apart (`w·Z`, `d·B`), so a
//! name of the other file resolves to nothing, wherever it is read, and both
//! may declare `p`. Messages show the names as written. Positions stay each
//! file's: the workload's lines are numbered after the deployment's, as in the
//! text `combined`, and `locate` takes a line back to its file.

use crate::frontend::diagnostic::Span;
use crate::frontend::lexer::{Tok, Token, lex};
use crate::frontend::parser::ParseError;

/// What the pair calls a workload file's constant or definition inside.
pub const WORKLOAD: &str = "w·";
/// What the pair calls a deployment file's constant or definition inside.
pub const DEPLOYMENT: &str = "d·";

/// Which of the pair a position is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    Deployment,
    Workload,
}

/// The pair, and the one text positions are numbered in.
pub struct Paired<'a> {
    pub deployment: &'a str,
    pub workload: &'a str,
    /// The names the files are shown by in a message (their paths).
    pub names: (&'a str, &'a str),
    /// The deployment's text, a line break, the workload's: what the
    /// parser's positions (and the outline's byte offsets) are into.
    pub combined: String,
    /// The lines of `combined` before the workload's first.
    lines: usize,
    /// The byte offset of the workload's text in `combined`.
    pub at: usize,
}

/// The tokens the parser reads for a pair, and what it needs to know of them.
pub struct Spliced {
    pub toks: Vec<Token>,
    /// The last line of the deployment's in the pair's text: a token on a
    /// later line is the workload file's, wherever the parser's expansions
    /// (a `def`'s use) move it in the token stream.
    pub split: usize,
    /// The stages and pools the workload file declares: what it runs on
    /// and holds itself (a closed loop's user, a session cap).
    pub stages: Vec<String>,
    /// The definitions of the deployment file and of the workload file, as
    /// written: an override names one (`--def service=…`).
    pub defs: (Vec<String>, Vec<String>),
    /// Each file's own constants and definitions, the deployment's and the
    /// workload's: an override's body reads its definition's file's (#703).
    pub own: Own,
}

/// Each file's own constants and definitions: the deployment's, the workload's.
pub type Own = (Vec<(Token, String)>, Vec<(Token, String)>);

/// The tokens of an override of the definition `name` (as the pair
/// renamed it), its file's names renamed as that file's are, so that it
/// reads them as the definition it replaces would (`--def size_of=a * x`,
/// #703).
pub fn rename_override(own: &Own, name: &str, body: &mut [Token]) {
    if name.starts_with(DEPLOYMENT) {
        rename(body, &own.0, DEPLOYMENT);
    } else if name.starts_with(WORKLOAD) {
        rename(body, &own.1, WORKLOAD);
    }
}

impl<'a> Paired<'a> {
    pub fn new(deployment: &'a str, workload: &'a str, names: (&'a str, &'a str)) -> Self {
        let mut combined = deployment.to_string();
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
        let lines = combined.matches('\n').count();
        let at = combined.len();
        combined.push_str(workload);
        Paired {
            deployment,
            workload,
            names,
            combined,
            lines,
            at,
        }
    }

    /// The file a line of `combined` is in, and its line there.
    pub fn locate(&self, line: usize) -> (File, usize) {
        if line > self.lines {
            (File::Workload, line - self.lines)
        } else {
            (File::Deployment, line)
        }
    }

    /// A file's name and text.
    fn file(&self, f: File) -> (&str, &str) {
        match f {
            File::Deployment => (self.names.0, self.deployment),
            File::Workload => (self.names.1, self.workload),
        }
    }

    /// A message at a position of `combined`, shown in its own file: the
    /// names as written, a name of the other file said to be the other's,
    /// and each position it quotes (`declared at 6:5`) in its own file.
    pub fn render(&self, span: Option<Span>, message: &str) -> String {
        let span = span.filter(|s| s.line > 0);
        let here = span.map(|s| self.locate(s.line).0);
        let message = self
            .crossed(here, message)
            .unwrap_or_else(|| self.places(here, message));
        let message = written(&message);
        let Some(span) = span else {
            return format!("{} + {}: {message}", self.names.0, self.names.1);
        };
        let (file, line) = self.locate(span.line);
        let (name, text) = self.file(file);
        format!("{name}: {}", Span { line, ..span }.render(text, &message))
    }

    /// A message about a name the reading file does not have and the other
    /// file declares: the reason, in place of "unknown".
    fn crossed(&self, here: Option<File>, message: &str) -> Option<String> {
        let here = here?;
        if !message.contains("unknown") && !message.contains("is not a constant") {
            return None;
        }
        let (other, whose) = match here {
            File::Deployment => (File::Workload, "workload"),
            File::Workload => (File::Deployment, "deployment"),
        };
        // the first name quoted, as the reading file wrote it
        let name = message.split('`').nth(1)?;
        let depth = if other == File::Workload { 0 } else { 1 };
        let toks = lex(self.file(other).1).ok()?;
        let declared = names(&toks, &["let", "def"], depth)
            .iter()
            .any(|(_, n)| n == name);
        let link = if message.starts_with("link error: ") {
            "link error: "
        } else {
            ""
        };
        declared.then(|| {
            format!(
                "{link}`{name}` is the {whose} file's: the workload and the deployment read \
                 their own names, and meet only at the server's signature (`send`, `reply`)"
            )
        })
    }

    /// Each `L:C` a message quotes after "at", in its own file, named when it
    /// is in the other file than the message's (`w.sq:1:5`).
    fn places(&self, here: Option<File>, message: &str) -> String {
        let digits = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let mut out = String::new();
        let mut rest = message;
        while let Some(k) = rest.find("at ") {
            out.push_str(&rest[..k + 3]);
            rest = &rest[k + 3..];
            let l = digits(rest);
            if l == 0 || !rest[l..].starts_with(':') {
                continue;
            }
            let c = digits(&rest[l + 1..]);
            if c == 0 {
                continue;
            }
            let (file, own) = self.locate(rest[..l].parse().unwrap_or(0));
            let col = &rest[l + 1..l + 1 + c];
            if Some(file) == here {
                out.push_str(&format!("{own}:{col}"));
            } else {
                out.push_str(&format!("{}:{own}:{col}", self.file(file).0));
            }
            rest = &rest[l + 1 + c..];
        }
        out.push_str(rest);
        out
    }

    /// The tokens the parser reads (`Spliced`), each file's constants and
    /// definitions renamed apart.
    pub fn tokens(&self) -> Result<Spliced, String> {
        let at_line =
            |line: usize, col: usize, msg: &str| self.render(Some(Span { line, col, len: 1 }), msg);
        let mut dep = lex(self.deployment).map_err(|e| at_line(e.line, e.col, &e.msg))?;
        // the deployment's end is its own, not the workload's first line
        for t in &mut dep {
            t.line = t.line.min(self.lines);
        }
        let mut wl = lex(self.workload).map_err(|e| at_line(e.line + self.lines, e.col, &e.msg))?;
        // its trivia are numbered as its tokens are, the comments after its
        // last token among them: the parser finds the comment that ends a
        // `let`'s line by its line (`Program::consts`)
        for t in &mut wl {
            t.line += self.lines;
            for tr in &mut t.leading {
                tr.line += self.lines;
            }
        }
        let tail = wl
            .iter_mut()
            .filter(|t| t.tok == Tok::Eof)
            .flat_map(|t| std::mem::take(&mut t.leading))
            .collect::<Vec<_>>();
        wl.retain(|t| t.tok != Tok::Eof);
        // the workload file's `use`s, which belong before `fn main()`
        let mut uses = vec![];
        let mut rest = vec![];
        let mut i = 0;
        while i < wl.len() {
            if matches!(&wl[i].tok, Tok::Ident(w) if w == "use") {
                let end = wl[i..]
                    .iter()
                    .position(|t| t.tok == Tok::Semi)
                    .map_or(wl.len(), |k| i + k + 1);
                uses.extend(wl[i..end].iter().cloned());
                i = end;
            } else {
                rest.push(wl[i].clone());
                i += 1;
            }
        }
        let declares = rest
            .iter()
            .any(|t| matches!(&t.tok, Tok::Ident(w) if w == "workload"));
        if !declares {
            return Err(at_line(
                self.lines + 1,
                1,
                "a workload file declares `workload { … }`",
            ));
        }
        if let Some(t) = rest
            .iter()
            .find(|t| matches!(&t.tok, Tok::Ident(w) if w == "fn"))
        {
            return Err(at_line(
                t.line,
                t.col,
                "a workload file has no `fn main()`: its constants, inputs, definitions and \
                 stages, and `workload { … }`, are the program's when the tool pairs it",
            ));
        }
        // `fn main() {` of the deployment file
        let main = dep.windows(5).position(|w| {
            matches!(&w[0].tok, Tok::Ident(f) if f == "fn")
                && matches!(&w[1].tok, Tok::Ident(m) if m == "main")
                && w[2].tok == Tok::LParen
                && w[3].tok == Tok::RParen
                && w[4].tok == Tok::LBrace
        });
        let Some(main) = main else {
            return Err(at_line(
                1,
                1,
                "a deployment file declares `fn main() { … }`: its engines, pools and `server`",
            ));
        };
        // each file's own constants and definitions: the workload's at its
        // top level, the deployment's at its top level and in `fn main()`
        // a word of the language keeps its spelling, so the parser refuses
        // it as it would in one file (`def min`), and a `let` of one that
        // both files declare (`let cap`), which would be one constant
        let named = |toks: &[Token], depth| {
            let mut own = names(toks, &["let", "def"], depth);
            own.retain(|(_, n)| !word(n));
            own
        };
        let wl_own = named(&rest, 0);
        let dep_own = named(&dep, 1);
        let defs = (
            plain(names(&dep, &["def"], 1)),
            plain(names(&rest, &["def"], 0)),
        );
        let stages = plain(names(&rest, &["stage", "pool"], 0));
        rename(&mut uses, &wl_own, WORKLOAD);
        rename(&mut rest, &wl_own, WORKLOAD);
        rename(&mut dep, &dep_own, DEPLOYMENT);
        // the `}` that closes `fn main()`: the workload goes before it, after
        // the deployment's items, so each declaration keeps the index it has
        // in the one file the pair would make (the oracle's and the claims'
        // IR among them); an unclosed `fn main()` is the parser's to say
        let mut depth = 0usize;
        let close = dep[main + 4..]
            .iter()
            .position(|t| {
                match t.tok {
                    Tok::LBrace => depth += 1,
                    Tok::RBrace => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map_or(dep.len() - 1, |k| main + 4 + k);
        let mut toks = uses;
        toks.extend(dep[..close].iter().cloned());
        toks.extend(rest);
        let at = toks.len();
        toks.extend(dep[close..].iter().cloned());
        toks[at].leading.splice(0..0, tail);
        Ok(Spliced {
            toks,
            split: self.lines,
            stages,
            defs,
            own: (dep_own, wl_own),
        })
    }
}

/// A word of the language (supplied, retired or a built-in attribute) or a
/// std module's name:
/// never a file's own name.
fn word(name: &str) -> bool {
    use crate::frontend::link::{BUILTIN_ATTRS, SCHEDULER_ATTRS};
    use crate::frontend::parser::{is_reserved, retired};
    crate::frontend::parser::STD_MODULES.contains(&name)
        || BUILTIN_ATTRS.contains(&name)
        || SCHEDULER_ATTRS.contains(&name)
        || is_reserved(name)
        || retired(name).is_some()
        || crate::frontend::link::takes_supplied(name, "a definition").is_some()
}

/// A name as the program wrote it: the pair's renaming taken off.
pub fn written(s: &str) -> String {
    s.replace(WORKLOAD, "").replace(DEPLOYMENT, "")
}

fn plain(names: Vec<(Token, String)>) -> Vec<String> {
    names.into_iter().map(|(_, n)| n).collect()
}

/// Rename each of `own`'s names in `toks` to `prefix` and the name, but a
/// field after a `.` (`Q.x`).
fn rename(toks: &mut [Token], own: &[(Token, String)], prefix: &str) {
    for k in 0..toks.len() {
        let field = k > 0 && toks[k - 1].tok == Tok::Dot;
        // the word a clause begins with, followed by its value
        // (`pool live { cap cap; }`): an option, not the constant
        let clause = (k == 0 || matches!(toks[k - 1].tok, Tok::LBrace | Tok::Semi))
            && toks
                .get(k + 1)
                .is_some_and(|t| matches!(t.tok, Tok::Ident(_) | Tok::Num(_)));
        if let Tok::Ident(n) = &toks[k].tok
            && !field
            && !clause
            && own.iter().any(|(_, o)| o == n)
        {
            toks[k].tok = Tok::Ident(format!("{prefix}{n}"));
        }
    }
}

/// The names `toks` declare after one of `keywords`, at a block depth of at
/// most `depth` (0: the top level; 1: and `fn main()`'s body).
fn names(toks: &[Token], keywords: &[&str], depth: usize) -> Vec<(Token, String)> {
    let mut out = vec![];
    let mut level = 0usize;
    for (k, t) in toks.iter().enumerate() {
        match &t.tok {
            Tok::LBrace => level += 1,
            Tok::RBrace => level = level.saturating_sub(1),
            Tok::Ident(w) if keywords.contains(&w.as_str()) && level <= depth => {
                if let Some(Token {
                    tok: Tok::Ident(n), ..
                }) = toks.get(k + 1)
                {
                    out.push((toks[k + 1].clone(), n.clone()));
                }
            }
            _ => {}
        }
    }
    out
}

/// Which half of a pair `src` is, if it is one: a workload file (a
/// `workload` and no `fn main()`) or a deployment file (`fn main()` and no
/// `workload`).
pub fn half(src: &str) -> Option<File> {
    let toks = lex(src).ok()?;
    let says = |w: &str| {
        toks.iter()
            .any(|t| matches!(&t.tok, Tok::Ident(x) if x == w))
    };
    match (says("workload"), says("main")) {
        (true, false) => Some(File::Workload),
        (false, true) => Some(File::Deployment),
        _ => None,
    }
}

/// A parse error at a position of the pair, shown in its own file.
pub fn parse_error(p: &Paired, e: &ParseError) -> String {
    if e.line == 0 {
        return written(&e.msg);
    }
    p.render(
        Some(Span {
            line: e.line,
            col: e.col,
            len: 1,
        }),
        &e.msg,
    )
}
