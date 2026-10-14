//! A deployment file and a workload file, paired by the tool (`serq run
//! DEPLOYMENT --workload WORKLOAD`, `serq::compile_paired`): the same
//! deployment under another workload, the same workload on another
//! deployment (`docs/design/paired-workload.md`).
//!
//! The deployment file is a program without its `workload`; the workload
//! file has no `fn main()`: its `use`s, its constants and inputs (`let`), its
//! definitions (`def`), the stages it runs on itself (`stage user : delay;`)
//! and `workload { … }`. The two meet only at the server's signature: each
//! reads its own names, and a name both declare is refused.
//!
//! The parser reads the pair as one program: the workload file's tokens go
//! at the head of `fn main()`'s body (its `use`s before the deployment's), so
//! its inputs are the program's inputs. Positions stay its own: its lines are
//! numbered after the deployment's, as in the text `combined`, and `locate`
//! takes a position back to its file.

use crate::frontend::diagnostic::Span;
use crate::frontend::lexer::{Tok, Token, lex};
use crate::frontend::parser::ParseError;

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

    /// A message at a position of `combined`, shown in its own file.
    pub fn render(&self, span: Option<Span>, message: &str) -> String {
        let Some(span) = span.filter(|s| s.line > 0) else {
            return message.to_string();
        };
        let (file, line) = self.locate(span.line);
        let (name, text) = match file {
            File::Deployment => (self.names.0, self.deployment),
            File::Workload => (self.names.1, self.workload),
        };
        let own = Span { line, ..span };
        format!("{name}: {}", own.render(text, message))
    }

    /// The tokens the parser reads, and the range of them that is the
    /// workload file's (its `use`s apart, which go first).
    pub fn tokens(&self) -> Result<(Vec<Token>, std::ops::Range<usize>), String> {
        let lexed = |src: &str, shift: usize| {
            lex(src)
                .map(|mut ts| {
                    for t in &mut ts {
                        t.line += shift;
                    }
                    ts
                })
                .map_err(|e| {
                    self.render(
                        Some(Span {
                            line: e.line + shift,
                            col: e.col,
                            len: 1,
                        }),
                        &e.msg,
                    )
                })
        };
        let dep = lexed(self.deployment, 0)?;
        let mut wl = lexed(self.workload, self.lines)?;
        wl.retain(|t| t.tok != Tok::Eof);
        let refuse = |t: &Token, msg: String| {
            self.render(
                Some(Span {
                    line: t.line,
                    col: t.col,
                    len: t.text.chars().count().max(1),
                }),
                &msg,
            )
        };
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
        if let Some(t) = rest
            .iter()
            .find(|t| matches!(&t.tok, Tok::Ident(w) if w == "fn"))
        {
            return Err(refuse(
                t,
                "a workload file has no `fn main()`: its constants, inputs, definitions and \
                 stages, and `workload { … }`, are the program's when the tool pairs it"
                    .into(),
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
            return Err(self.render(
                Some(Span {
                    line: 1,
                    col: 1,
                    len: 1,
                }),
                "a deployment file declares `fn main() { … }`: its engines, pools and `server`",
            ));
        };
        self.crossed(&dep, &rest)
            .map_err(|(t, msg)| refuse(&t, msg))?;
        let body = main + 5;
        let mut out = uses;
        out.extend(dep[..body].iter().cloned());
        let start = out.len();
        out.extend(rest);
        let end = out.len();
        out.extend(dep[body..].iter().cloned());
        Ok((out, start..end))
    }

    /// A name both files declare is refused: each reads its own, which the
    /// linker holds it to (`Linker::cross`).
    fn crossed(&self, dep: &[Token], wl: &[Token]) -> Result<(), (Token, String)> {
        let dep_names = declared(dep, &DEPLOYMENT_DECLS, false);
        for (t, n) in declared(wl, &WORKLOAD_DECLS, true) {
            if dep_names.iter().any(|(_, d)| *d == n) {
                return Err((
                    t,
                    format!(
                        "`{n}` is declared in both files; each reads its own: name one of them apart"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The names the workload file declares: its constants, inputs,
    /// definitions and stages.
    pub fn workload_names(&self) -> Result<Vec<String>, String> {
        let toks = lex(self.workload).map_err(|e| e.msg)?;
        Ok(declared(&toks, &WORKLOAD_DECLS, true)
            .into_iter()
            .map(|(_, n)| n)
            .collect())
    }
}

/// What a deployment file declares by name: its constants, definitions,
/// stages, pools, devices, engines and queues.
const DEPLOYMENT_DECLS: [&str; 7] = ["let", "def", "stage", "pool", "device", "engine", "queue"];

/// What a workload file declares by name.
const WORKLOAD_DECLS: [&str; 3] = ["let", "def", "stage"];

/// The names `toks` declare after one of `keywords`. `top`: only at the top
/// level (a workload file's), not inside a block.
fn declared(toks: &[Token], keywords: &[&str], top: bool) -> Vec<(Token, String)> {
    let mut out = vec![];
    let mut depth = 0usize;
    for (k, t) in toks.iter().enumerate() {
        match &t.tok {
            Tok::LBrace => depth += 1,
            Tok::RBrace => depth = depth.saturating_sub(1),
            Tok::Ident(w) if keywords.contains(&w.as_str()) && (!top || depth == 0) => {
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

/// Whether `src` is half of a pair: a workload file (a `workload` and no
/// `fn main()`) or a deployment file (`fn main()` and no `workload`).
pub fn half(src: &str) -> bool {
    let Ok(toks) = lex(src) else {
        return false;
    };
    let says = |w: &str| {
        toks.iter()
            .any(|t| matches!(&t.tok, Tok::Ident(x) if x == w))
    };
    says("workload") != says("main")
}

/// A parse error at a position of the pair, shown in its own file.
pub fn parse_error(p: &Paired, e: &ParseError) -> String {
    if e.line == 0 {
        return e.msg.clone();
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
