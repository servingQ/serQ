//! An engine's request (`docs/design/engine-request.md`): what the engine
//! does with one request, called from the server as `vllm (prompt, o);`.
//!
//! ```text
//! engine vllm on gpu {
//!   request (prompt, output) {
//!     state kept = prompt;                    // set when the call is reached
//!     known = max(kept, computed + 1);        // an admission binding
//!     set kept = known;                       // the statements while admitted
//!     prefill (known - cached) growing kv;
//!     mark prefilled;
//!     decode (output - 1) growing kv;
//!   }
//!   schedule { …; admit waiting … taking reqs (1), kv (known) reserve (known); }
//!   release cache (prompt + output);
//! }
//! ```
//!
//! It is parse-time sugar. Once the engine is read, the request is written
//! out as the kernel text a server writes, a queue entry of the engine
//!
//! ```text
//! request (prompt, output) {
//!   set kept = prompt;
//!   hold reqs (cost(reqs, 1)), kv (cost(kv, known)) reserve (cost(kv, known))
//!        at admission (known = max(kept, computed + 1)) {
//!     set kept = known;
//!     run vllm prefill (cost(vllm, known - cached)) growing kv;
//!     mark prefilled;
//!     run vllm decode (cost(vllm, output - 1)) growing kv;
//!   } cache (cost(reqs, kv, prompt + output));
//! }
//! ```
//!
//! and read by `entry`, so a call expands as a queue's does: the parameters
//! substituted, `mark m` written `set vllm.m = now`. Unlike a queue's entry,
//! the request keeps its names (`kept`, not `vllm.kept`) and the engine's
//! pools are the program's: the IR is the one the server wrote.

use super::*;

/// `request ( NAME, … ) { … }` as written, read once the engine is.
pub(super) struct RequestSrc {
    /// The position of `request`.
    pub at: usize,
    /// `( … )`, the parentheses included.
    pub params: Vec<Token>,
    /// What the braces hold.
    pub body: Vec<Token>,
}

/// `taking P (u) [reserve (r)], … [reuse (e)]`, each expression's tokens.
struct Taking {
    pools: Vec<(Token, Vec<Token>, Option<Vec<Token>>)>,
    reuse: Option<Vec<Token>>,
}

/// A token written by the lowering, placed at `like` for the errors.
fn made(tok: Tok, like: &Token) -> Token {
    Token {
        text: tok.to_string(),
        tok,
        line: like.line,
        col: like.col,
        file: like.file,
        leading: vec![],
    }
}

fn word(w: &str, like: &Token) -> Token {
    made(Tok::Ident(w.into()), like)
}

/// The tokens of the group that opens at `toks[i]`, without its brackets,
/// and the index after its close.
fn group(toks: &[Token], i: usize) -> Option<(Vec<Token>, usize)> {
    if toks.get(i)?.tok != Tok::LParen {
        return None;
    }
    let mut depth = 0usize;
    for (j, t) in toks.iter().enumerate().skip(i) {
        match t.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return Some((toks[i + 1..j].to_vec(), j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// The index of the first `;` at depth 0 from `i`.
fn item_end(toks: &[Token], i: usize) -> Option<usize> {
    let mut depth = 0isize;
    for (j, t) in toks.iter().enumerate().skip(i) {
        match t.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
            Tok::Semi if depth == 0 => return Some(j),
            _ => {}
        }
    }
    None
}

/// `cost ( P₁ , … , e )` over the pools taken.
fn cost(pools: &[&Token], e: &[Token], like: &Token) -> Vec<Token> {
    let mut out = vec![word("cost", like), made(Tok::LParen, like)];
    for p in pools {
        out.push((*p).clone());
        out.push(made(Tok::Comma, like));
    }
    out.extend(e.iter().cloned());
    out.push(made(Tok::RParen, like));
    out
}

/// `( e )`.
fn paren(mut e: Vec<Token>, like: &Token) -> Vec<Token> {
    e.insert(0, made(Tok::LParen, like));
    e.push(made(Tok::RParen, like));
    e
}

impl Parser {
    /// At `request` in engine `engine`: its parameters and body, kept as
    /// tokens until the engine's `taking` is read too. A queue's engine
    /// and an engine family have none: a queue's request is its entries,
    /// and a family's member would need an index at the call.
    pub(super) fn request_src(
        &mut self,
        engine: &str,
        device: &str,
        family: bool,
    ) -> PResult<RequestSrc> {
        let at = self.pos;
        self.advance();
        if let Some((q, _)) = device.split_once('.') {
            return self.err_at(
                at,
                format!(
                    "a `request` in queue `{q}`'s engine: a queue says what it does with a \
                     request in its entries"
                ),
            );
        }
        if family {
            return self.err_at(
                at,
                format!(
                    "a `request` in engine family `{engine}`: a call would have to pick a \
                     member, which a request does not take; declare one engine per device"
                ),
            );
        }
        let start = self.pos;
        let Some((_, end)) = group(&self.toks, start) else {
            return self.err(
                "expected `(` and the request's parameters: `request (prompt, output) { … }`",
            );
        };
        let params = self.toks[start..end].to_vec();
        self.pos = end;
        let open = self.pos;
        self.expect(&Tok::LBrace)?;
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek() {
                Tok::Eof => return self.err_at(open, "the request's `{` is not closed"),
                Tok::LBrace => depth += 1,
                Tok::RBrace => depth -= 1,
                _ => {}
            }
            self.advance();
        }
        let body = self.toks[open + 1..self.pos - 1].to_vec();
        Ok(RequestSrc { at, params, body })
    }

    /// `cache ( e ) ;` after `release`: `e`'s tokens.
    pub(super) fn release_src(&mut self) -> PResult<Vec<Token>> {
        self.expect_kw("cache")?;
        let Some((e, end)) = group(&self.toks, self.pos) else {
            return self.err(
                "expected `(`: `release cache (e);` says what a finished request leaves cached",
            );
        };
        if e.is_empty() {
            return self.err("`release cache ()` needs what the request leaves cached");
        }
        self.pos = end;
        self.expect(&Tok::Semi)?;
        Ok(e)
    }

    /// `taking …` on `admit waiting`, if it is next: its tokens up to `;`.
    pub(super) fn taking_src(&mut self) -> PResult<()> {
        let at = self.pos;
        if !self.eat_kw("taking") {
            self.takings.push((at, None));
            return Ok(());
        }
        let Some(end) = item_end(&self.toks, self.pos) else {
            return self.err("`taking …` ends with `;`");
        };
        let toks = self.toks[self.pos..end].to_vec();
        if toks.is_empty() {
            return self
                .err("`taking` names the pools the admission takes: `taking reqs (1), kv (n)`");
        }
        self.pos = end;
        self.takings.push((at, Some(toks)));
        Ok(())
    }

    /// `P (u) [reserve (r)], … [reuse (e)]`, from the tokens at `at`.
    fn taking(&self, at: usize, toks: &[Token]) -> PResult<Taking> {
        let bad = || {
            self.err_at(
                at,
                "`taking` is a list of `POOL (units) [reserve (units)]`, then an optional \
                 `reuse (units)`: `taking reqs (1), kv (n) reserve (m)`",
            )
        };
        let mut t = Taking {
            pools: vec![],
            reuse: None,
        };
        let mut i = 0;
        loop {
            let Some(p) = toks
                .get(i)
                .filter(|p| matches!(&p.tok, Tok::Ident(w) if w != "reuse"))
            else {
                return bad();
            };
            let Some((u, next)) = group(toks, i + 1) else {
                return bad();
            };
            i = next;
            let reserve = match toks.get(i) {
                Some(Token {
                    tok: Tok::Ident(w), ..
                }) if w == "reserve" => {
                    let Some((r, next)) = group(toks, i + 1) else {
                        return bad();
                    };
                    i = next;
                    Some(r)
                }
                _ => None,
            };
            if u.is_empty() || reserve.as_ref().is_some_and(|r| r.is_empty()) {
                return bad();
            }
            let Tok::Ident(name) = &p.tok else {
                unreachable!()
            };
            if t.pools.iter().any(|(q, ..)| q.tok == p.tok) {
                return self.err_at(at, format!("`taking` names `{name}` twice"));
            }
            t.pools.push((p.clone(), u, reserve));
            match toks.get(i) {
                Some(Token {
                    tok: Tok::Comma, ..
                }) => i += 1,
                _ => break,
            }
        }
        if let Some(Token {
            tok: Tok::Ident(w), ..
        }) = toks.get(i)
            && w == "reuse"
        {
            let Some((e, next)) = group(toks, i + 1) else {
                return bad();
            };
            t.reuse = Some(e);
            i = next;
        }
        if i != toks.len() {
            return bad();
        }
        Ok(t)
    }

    /// Engine `engine` is read: write its request out as the kernel a
    /// server writes, a queue entry of the engine, and read it.
    pub(super) fn engine_request(
        &mut self,
        engine: &str,
        request: Option<RequestSrc>,
        release: Option<(usize, Vec<Token>)>,
    ) -> PResult<()> {
        let takings = std::mem::take(&mut self.takings);
        let written: Vec<&(usize, Option<Vec<Token>>)> =
            takings.iter().filter(|(_, t)| t.is_some()).collect();
        let Some(request) = request else {
            if let Some((at, _)) = written.first() {
                return self.err_at(
                    *at,
                    format!(
                        "`taking` sizes the admission of engine `{engine}`'s request, and \
                         `{engine}` declares no `request (…) {{ … }}`"
                    ),
                );
            }
            if let Some((at, _)) = release {
                return self.err_at(
                    at,
                    format!(
                        "`release cache` says what engine `{engine}`'s request leaves cached, \
                         and `{engine}` declares no `request (…) {{ … }}`"
                    ),
                );
            }
            return Ok(());
        };
        // every admission is the request's: one sizing
        let taking = match written.first() {
            None => None,
            Some((at, Some(first))) => {
                let same = |t: &Option<Vec<Token>>| {
                    t.as_ref().is_some_and(|t| {
                        t.len() == first.len() && t.iter().zip(first).all(|(a, b)| a.tok == b.tok)
                    })
                };
                if let Some((other, _)) = takings.iter().find(|(_, t)| !same(t)) {
                    return self.err_at(
                        *other,
                        format!(
                            "every `admit waiting` of engine `{engine}` admits its request: \
                             write the same `taking` on each"
                        ),
                    );
                }
                Some(self.taking(*at, first)?)
            }
            Some((_, None)) => unreachable!("filtered"),
        };
        if taking.is_none()
            && let Some((at, _)) = release
        {
            return self.err_at(
                at,
                format!(
                    "`release cache` says what engine `{engine}`'s request leaves of what it took, \
                     and the request takes nothing: write `taking` on `admit waiting`"
                ),
            );
        }
        // `state`s, then bindings, then statements
        let body = &request.body;
        let mut states: Vec<(Token, Vec<Token>)> = vec![];
        let mut binds: Vec<(Token, Vec<Token>)> = vec![];
        let mut i = 0;
        while i < body.len() {
            let is_state = matches!(&body[i].tok, Tok::Ident(w) if w == "state");
            let is_bind = matches!(&body[i].tok, Tok::Ident(_))
                && body.get(i + 1).is_some_and(|t| t.tok == Tok::Assign);
            if !is_state && !is_bind {
                break;
            }
            let Some(end) = item_end(body, i) else {
                return self.err_at(request.at, "an item of the request ends with `;`");
            };
            if is_state {
                if !binds.is_empty() {
                    return self.err_at(
                        request.at,
                        "a request's `state` stands above its bindings: it is set when the call \
                         is reached, before the admission",
                    );
                }
                let name = body.get(i + 1).filter(|t| matches!(t.tok, Tok::Ident(_)));
                let (Some(name), true) =
                    (name, body.get(i + 2).is_some_and(|t| t.tok == Tok::Assign))
                else {
                    return self.err_at(request.at, "`state NAME = expr;` in a request");
                };
                states.push((name.clone(), body[i + 3..end].to_vec()));
            } else {
                binds.push((body[i].clone(), body[i + 2..end].to_vec()));
            }
            i = end + 1;
        }
        let stmts = &body[i..];
        let like = self.toks[request.at].clone();
        if taking.is_none()
            && let Some((name, _)) = binds.first()
        {
            return self.err_at(
                request.at,
                format!(
                    "`{}` is an admission binding, and engine `{engine}`'s request takes nothing \
                     at admission: write `taking` on `admit waiting`",
                    name.text
                ),
            );
        }
        // the kernel text
        let mut out = vec![like.clone()];
        out.extend(request.params.iter().cloned());
        out.push(made(Tok::LBrace, &like));
        for (name, e) in &states {
            out.push(word("set", name));
            out.push(name.clone());
            out.push(made(Tok::Assign, name));
            out.extend(e.iter().cloned());
            out.push(made(Tok::Semi, name));
        }
        match &taking {
            None => out.extend(stmts.iter().cloned()),
            Some(t) => {
                let pools: Vec<&Token> = t.pools.iter().map(|(p, ..)| p).collect();
                out.push(word("hold", &like));
                for (k, (p, u, reserve)) in t.pools.iter().enumerate() {
                    if k > 0 {
                        out.push(made(Tok::Comma, p));
                    }
                    out.push(p.clone());
                    out.extend(paren(cost(&[p], u, p), p));
                    if let Some(r) = reserve {
                        out.push(word("reserve", p));
                        out.extend(paren(cost(&[p], r, p), p));
                    }
                }
                if let Some(e) = &t.reuse {
                    out.push(word("reuse", &like));
                    out.extend(paren(cost(&pools, e, &like), &like));
                }
                if !binds.is_empty() {
                    out.push(word("at", &like));
                    out.push(word("admission", &like));
                    out.push(made(Tok::LParen, &like));
                    for (k, (name, e)) in binds.iter().enumerate() {
                        if k > 0 {
                            out.push(made(Tok::Comma, name));
                        }
                        out.push(name.clone());
                        out.push(made(Tok::Assign, name));
                        out.extend(e.iter().cloned());
                    }
                    out.push(made(Tok::RParen, &like));
                }
                out.push(made(Tok::LBrace, &like));
                out.extend(stmts.iter().cloned());
                out.push(made(Tok::RBrace, &like));
                if let Some((at, e)) = &release {
                    let at = self.toks[*at].clone();
                    out.push(word("cache", &at));
                    out.extend(paren(cost(&pools, e, &at), &at));
                }
                out.push(made(Tok::Semi, &like));
            }
        }
        out.push(made(Tok::RBrace, &like));
        // read it where the engine ends, as a queue's entry
        let at = self.pos;
        self.toks.splice(at..at, out);
        let pools = taking
            .as_ref()
            .map(|t| {
                t.pools
                    .iter()
                    .map(|(p, ..)| p.text.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.queues.push(QueueDecl {
            name: engine.to_string(),
            count: 1,
            family: false,
            roles: vec![],
            pools,
            has_stage: true,
            entries: vec![],
            leased: None,
            marks: vec![],
            latency: None,
            nic: false,
            takes: None,
            engine: true,
            at: request.at,
        });
        let qi = self.queues.len() - 1;
        self.in_request = Some(engine.to_string());
        let read = self.entry(qi, "request".into());
        self.in_request = None;
        read
    }

    /// `prefill (e) [growing P];` or `decode (…) …` in engine `engine`'s
    /// request: the engine's run in that mode.
    pub(super) fn request_run(&mut self, engine: &str) -> PResult<Stmt> {
        let span = Some(self.span());
        let mode = if self.eat_kw("prefill") {
            RunMode::Prefill
        } else {
            self.expect_kw("decode")?;
            RunMode::Decode
        };
        let work = self.paren_expr()?;
        let growing = if self.eat_kw("growing") {
            Some(self.own_pool("growing")?)
        } else {
            None
        };
        self.expect(&Tok::Semi)?;
        let stage = Ref {
            span,
            name: engine.to_string(),
            index: None,
        };
        Ok(Stmt::Run {
            work: Expr::cost(std::slice::from_ref(&stage), work),
            stage,
            mode,
            growing,
            also: vec![],
        })
    }

    /// `E (a, …);` in a server: engine `E`'s request, in its place.
    pub(super) fn engine_call(&mut self) -> PResult<Stmt> {
        let at = self.pos;
        let span = Some(self.span());
        let name = self.ident()?;
        if self.side != Side::Server {
            return self.err_at(
                at,
                format!("`{name} (…)` calls engine `{name}`'s request, which a `server` does"),
            );
        }
        if !self.queues.iter().any(|q| q.engine && q.name == name) {
            return self.err_at(
                at,
                format!(
                    "engine `{name}` has no `request (…) {{ … }}`: it says what the engine does \
                     with one request, and a call runs it"
                ),
            );
        }
        self.expect(&Tok::LParen)?;
        let mut args = vec![];
        if *self.peek() != Tok::RParen {
            args.push(self.expr()?);
            while self.eat(&Tok::Comma) {
                args.push(self.expr()?);
            }
        }
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::Semi)?;
        Ok(Stmt::Call {
            queue: Ref {
                span,
                name,
                index: None,
            },
            verb: "request".into(),
            args,
            from: None,
            to: None,
        })
    }

    /// A request's own names (`kept`, `known`) keep their names in the
    /// IR; the rest of the program reads what the request marks, not them.
    pub(super) fn check_request_names(&self, prog: &Program) -> PResult<()> {
        for (engine, at, locals) in &self.request_locals {
            let mut outside: Vec<&Stmt> = vec![];
            if let Some((_, s)) = &self.server {
                outside.extend(s);
            }
            if let Some((_, s)) = &self.wl_session {
                outside.extend(s);
            }
            if let Some(w) = &prog.workload {
                outside.extend(&w.init);
                outside.extend(&w.turn);
            }
            let mut assigned = vec![];
            for s in &outside {
                assigned_in(std::slice::from_ref(*s), &mut assigned);
            }
            let found = locals
                .iter()
                .find(|l| assigned.contains(l) || outside.iter().any(|s| stmt_reads(s, l)));
            if let Some(l) = found {
                return self.err_at(
                    *at,
                    format!(
                        "`{l}` is engine `{engine}`'s request's own, and the program reads or sets \
                         it outside the request\nhelp: name it otherwise there, or `mark` a moment \
                         the caller reads as `{engine}.NAME`"
                    ),
                );
            }
        }
        Ok(())
    }
}
