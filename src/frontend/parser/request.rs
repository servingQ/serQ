//! An engine's request (`docs/design/engine-request.md`): what the engine
//! does with one request, called from the server as `vllm (prompt, o);`.
//!
//! ```text
//! engine vllm on gpu {
//!   request (prompt, output) {
//!     set kept = prompt;                      // when the call is reached
//!     at admission { known = max(kept, computed + 1); }
//!     set kept = known;                       // while admitted
//!     prefill (known - cached) growing kv;
//!     mark prefilled;
//!     decode (output - 1) growing kv;
//!     cache (prompt + output);                // what it leaves cached
//!   }
//!   schedule { …; admit waiting … taking reqs (1), kv (known) reserve (known); }
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
    /// What the braces hold, and the position of its first token.
    pub body: Vec<Token>,
    pub body_at: usize,
}

/// One pool of a `taking`: `POOL (units) [reserve (units)]`.
struct TakenPool {
    pool: Token,
    units: Vec<Token>,
    reserve: Option<Vec<Token>>,
}

/// `taking P (u) [reserve (r)], … [reuse (e)]` on `admit waiting`.
struct Taking {
    /// The position of `taking`.
    at: usize,
    pools: Vec<TakenPool>,
    reuse: Option<Vec<Token>>,
}

/// `NAME = e;` in `at admission { … }`.
struct Binding {
    name: Token,
    value: Vec<Token>,
}

/// A request's items in the order they happen: the statements when the
/// call is reached, the admission, the statements while admitted, and
/// what the release leaves cached.
struct RequestItems<'a> {
    before: &'a [Token],
    /// `at admission { … }`: its position and bindings.
    admission: Option<(usize, Vec<Binding>)>,
    after: &'a [Token],
    /// `cache (e);`: its position and `e`.
    cache: Option<(usize, Vec<Token>)>,
    /// `prefix (k: n, …);`: its position and the segments.
    prefix: Option<(usize, Vec<Token>)>,
}

/// An engine's request's own names, which the rest of the program does not
/// read or set.
pub(super) struct RequestNames {
    pub engine: String,
    pub at: usize,
    pub names: Vec<String>,
}

/// A token written by the lowering, placed at `like` for the errors.
fn made(tok: Tok, like: &Token) -> Token {
    Token {
        text: tok.to_string(),
        tok,
        line: like.line,
        col: like.col,
        leading: vec![],
    }
}

fn word(w: &str, like: &Token) -> Token {
    made(Tok::Ident(w.into()), like)
}

fn is_word(t: Option<&Token>, w: &str) -> bool {
    matches!(t, Some(Token { tok: Tok::Ident(x), .. }) if x == w)
}

/// The tokens of the group that opens at `toks[i]` with `(` or `{`,
/// without its brackets, and the index after its close.
fn group(toks: &[Token], i: usize) -> Option<(Vec<Token>, usize)> {
    if !matches!(toks.get(i)?.tok, Tok::LParen | Tok::LBrace) {
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

/// The positions of `toks` at depth 0, with the token at each.
fn top_level(toks: &[Token]) -> Vec<usize> {
    let mut depth = 0isize;
    let mut out = vec![];
    for (j, t) in toks.iter().enumerate() {
        if depth == 0 {
            out.push(j);
        }
        match t.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
            _ => {}
        }
    }
    out
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
    /// At `request` in the engine on `device`: its parameters and body,
    /// kept as tokens until the engine's `taking` is read too. A queue's
    /// engine has none: a queue's request is its entries (an engine family,
    /// refused where it is declared, is a queue family).
    pub(super) fn request_src(&mut self, device: &str) -> PResult<RequestSrc> {
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
        let start = self.pos;
        if self.toks[start].tok != Tok::LParen {
            return self.err(
                "expected `(` and the request's parameters: `request (prompt, output) { … }`",
            );
        }
        let Some((_, end)) = group(&self.toks, start) else {
            return self.err("the request's `(` is not closed");
        };
        let params = self.toks[start..end].to_vec();
        self.pos = end;
        let open = self.pos;
        self.expect(&Tok::LBrace)?;
        let Some((body, close)) = group(&self.toks, open) else {
            return self.err_at(open, "the request's `{` is not closed");
        };
        self.pos = close;
        Ok(RequestSrc {
            at,
            params,
            body,
            body_at: open + 1,
        })
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

    /// The `taking` at `at`: `P (u) [reserve (r)], … [reuse (e)]`, each `P`
    /// one of `pools`, the engine's capacities and its device's.
    fn taking(&self, engine: &str, at: usize, toks: &[Token], pools: &[String]) -> PResult<Taking> {
        let bad = || {
            self.err_at(
                at,
                "`taking` is a list of `POOL (units) [reserve (units)]`, then an optional \
                 `reuse (units)`: `taking reqs (1), kv (n) reserve (m)`",
            )
        };
        let paren_group = |i: usize| {
            toks.get(i)
                .filter(|t| t.tok == Tok::LParen)
                .and_then(|_| group(toks, i))
        };
        let mut t = Taking {
            at,
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
            let Some((units, next)) = paren_group(i + 1) else {
                return bad();
            };
            i = next;
            let reserve = if is_word(toks.get(i), "reserve") {
                let Some((r, next)) = paren_group(i + 1) else {
                    return bad();
                };
                i = next;
                Some(r)
            } else {
                None
            };
            if units.is_empty() || reserve.as_ref().is_some_and(|r| r.is_empty()) {
                return bad();
            }
            if !pools.contains(&p.text) {
                return self.err_at(
                    at,
                    format!(
                        "engine `{engine}`'s `taking` names `{}`, which is no capacity of the \
                         engine or its device: it takes {}",
                        p.text,
                        pools
                            .iter()
                            .map(|q| format!("`{q}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
            if t.pools.iter().any(|q| q.pool.text == p.text) {
                return self.err_at(at, format!("`taking` names `{}` twice", p.text));
            }
            t.pools.push(TakenPool {
                pool: p.clone(),
                units,
                reserve,
            });
            if toks.get(i).is_some_and(|t| t.tok == Tok::Comma) {
                i += 1;
            } else {
                break;
            }
        }
        if is_word(toks.get(i), "reuse") {
            let Some((e, next)) = paren_group(i + 1) else {
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

    /// The request's items, in the order they happen.
    fn request_items<'a>(&self, r: &'a RequestSrc) -> PResult<RequestItems<'a>> {
        // `prefix (k: n, …);`, the first item: what the request's sequence
        // is made of, which the hold of its admission reads
        let mut prefix = None;
        let mut skip = 0;
        if is_word(r.body.first(), "prefix") && r.body.get(1).is_some_and(|t| t.tok == Tok::LParen)
        {
            let (segs, close) = group(&r.body, 1).expect("balanced");
            if r.body.get(close).is_none_or(|t| t.tok != Tok::Semi) || segs.is_empty() {
                return self.err_at(
                    r.body_at,
                    "a request's `prefix (k: n, …);` names its segments and ends with `;`",
                );
            }
            prefix = Some((r.body_at, segs));
            skip = close + 1;
        }
        // an item's first word: at the start, or after a `;` or a block
        if let Some(i) = top_level(&r.body).into_iter().find(|&i| {
            i >= skip
                && is_word(r.body.get(i), "prefix")
                && r.body.get(i + 1).is_some_and(|t| t.tok == Tok::LParen)
                && (i == 0 || matches!(r.body[i - 1].tok, Tok::Semi | Tok::RBrace))
        }) {
            return self.err_at(
                r.body_at + i,
                "a request's `prefix (k: n, …);` is its first item: what its sequence is made \
                 of, before anything it does",
            );
        }
        let body = &r.body[skip..];
        let body_at = r.body_at + skip;
        let starts = top_level(body);
        // `at admission { … }`: the admission, once
        let mut admission = None;
        let (mut before, mut after): (&[Token], &[Token]) = (&[], body);
        for &i in &starts {
            if !(is_word(body.get(i), "at")
                && is_word(body.get(i + 1), "admission")
                && body.get(i + 2).is_some_and(|t| t.tok == Tok::LBrace))
            {
                continue;
            }
            if admission.is_some() {
                return self.err_at(
                    body_at + i,
                    "a request is admitted once: one `at admission`",
                );
            }
            let (inner, close) = group(body, i + 2).expect("balanced");
            admission = Some((body_at + i, self.bindings_of(&inner, body_at + i)?));
            before = &body[..i];
            after = &body[close..];
        }
        // `cache (e);`, the last item, after a statement's `;` or a block's
        // `}`, though not a hold's: `} cache (…)` there is the hold's own
        let mut cache = None;
        let offset = body.len() - after.len();
        // where a statement starts: after a `;`, or after a `}` that no
        // `else`, `cache` or `lease` continues
        let starts_statement = |j: usize| {
            j == 0
                || body[j - 1].tok == Tok::Semi
                || body[j - 1].tok == Tok::RBrace
                    && !["else", "cache", "lease"]
                        .iter()
                        .any(|w| is_word(body.get(j), w))
        };
        let ends_a_statement = |i: usize| {
            i == 0
                || body[i - 1].tok == Tok::Semi
                || body[i - 1].tok == Tok::RBrace
                    && !starts
                        .iter()
                        .rev()
                        .find(|&&s| s < i && starts_statement(s))
                        .is_some_and(|&s| is_word(body.get(s), "hold"))
        };
        if let Some(&i) = starts
            .iter()
            .rev()
            .find(|&&i| i >= offset && is_word(body.get(i), "cache") && ends_a_statement(i))
            && body.get(i + 1).is_some_and(|t| t.tok == Tok::LParen)
        {
            let (e, close) = group(body, i + 1).expect("balanced");
            if close + 1 != body.len() || body.get(close).is_none_or(|t| t.tok != Tok::Semi) {
                return self.err_at(
                    body_at + i,
                    "a request's `cache (e);` is its last item: what the release leaves cached",
                );
            }
            if e.is_empty() {
                return self.err_at(
                    body_at + i,
                    "`cache ()` needs what the request leaves cached",
                );
            }
            cache = Some((body_at + i, e));
            after = &body[offset..i];
        }
        Ok(RequestItems {
            before,
            admission,
            after,
            cache,
            prefix,
        })
    }

    /// `NAME = e; …` inside `at admission { … }` at `at`.
    fn bindings_of(&self, toks: &[Token], at: usize) -> PResult<Vec<Binding>> {
        let mut out = vec![];
        let mut i = 0;
        while i < toks.len() {
            let named = matches!(toks[i].tok, Tok::Ident(_))
                && toks.get(i + 1).is_some_and(|t| t.tok == Tok::Assign);
            let Some(end) = item_end(toks, i).filter(|_| named) else {
                return self.err_at(
                    at,
                    "`at admission { … }` holds bindings, `NAME = expr;`, read when the request \
                     is admitted",
                );
            };
            out.push(Binding {
                name: toks[i].clone(),
                value: toks[i + 2..end].to_vec(),
            });
            i = end + 1;
        }
        Ok(out)
    }

    /// Engine `engine` is read: write its request out as the kernel a
    /// server writes, a queue entry of the engine, and read it. `pools`
    /// are the capacities it may take: its own and its device's;
    /// `registers` its `state`, which the request reads as the engine's.
    pub(super) fn engine_request(
        &mut self,
        engine: &str,
        request: Option<RequestSrc>,
        pools: &[String],
        registers: Vec<String>,
    ) -> PResult<()> {
        let takings = std::mem::take(&mut self.takings);
        let written: Vec<(usize, &Vec<Token>)> = takings
            .iter()
            .filter_map(|(at, t)| t.as_ref().map(|t| (*at, t)))
            .collect();
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
            return Ok(());
        };
        // every admission is the request's: one sizing
        let taking = match written.first() {
            None => None,
            Some((at, first)) => {
                let same = |t: &Option<Vec<Token>>| {
                    t.as_ref().is_some_and(|t| {
                        t.len() == first.len() && t.iter().zip(*first).all(|(a, b)| a.tok == b.tok)
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
                Some(self.taking(engine, *at, first, pools)?)
            }
        };
        let items = self.request_items(&request)?;
        if taking.is_none() {
            let without = |what: &str| {
                format!(
                    "{what} in engine `{engine}`'s request, which takes nothing at admission: \
                     write `taking` on `admit waiting`"
                )
            };
            if let Some((at, _)) = items.admission {
                return self.err_at(at, without("`at admission`"));
            }
            if let Some((at, _)) = items.cache {
                return self.err_at(at, without("`cache`"));
            }
            if let Some((at, _)) = items.prefix {
                return self.err_at(at, without("`prefix`"));
            }
        }
        // the engine's own run is `prefill (e)` here, its kernel spelling the
        // lowering's: one meaning, one spelling (#539)
        if let Some(k) = (0..request.body.len()).find(|&k| {
            is_word(request.body.get(k), "run")
                && is_word(request.body.get(k + 1), engine)
                && ["prefill", "decode"]
                    .iter()
                    .any(|m| is_word(request.body.get(k + 2), m))
        }) {
            let m = &request.body[k + 2].text;
            return self.err_at(
                request.body_at + k,
                format!(
                    "engine `{engine}`'s request runs its own {m} as `run {engine} {m} (cost({engine}, …))`: \
                     in a request it is written `{m} (…);`"
                ),
            );
        }
        // a parameter is substituted where it is read, so one named like a
        // register would turn the engine's register into the caller's value
        if let Some(p) = request.params.iter().find_map(|t| match &t.tok {
            Tok::Ident(n) if registers.contains(n) => Some(n),
            _ => None,
        }) {
            return self.err_at(
                request.at,
                format!(
                    "engine `{engine}`'s request names a parameter `{p}`, which is also its \
                     register: one name would read the caller's value in one place and the \
                     engine's in another\nhelp: name the parameter otherwise"
                ),
            );
        }
        let text = self.kernel_text(&request, &items, taking.as_ref());
        // read it where the engine ends, as a queue's entry
        let at = self.pos;
        self.toks.splice(at..at, text);
        self.queues.push(QueueDecl {
            name: engine.to_string(),
            count: 1,
            family: false,
            roles: vec![],
            pools: taking
                .as_ref()
                .map(|t| t.pools.iter().map(|p| p.pool.text.clone()).collect())
                .unwrap_or_default(),
            has_stage: true,
            entries: vec![],
            leased: None,
            marks: vec![],
            latency: None,
            nic: false,
            takes: None,
            engine: true,
            registers,
            header_at: taking.as_ref().map(|t| t.at),
            at: request.at,
        });
        let qi = self.queues.len() - 1;
        self.in_request = Some(engine.to_string());
        let read = self.entry(qi, "request".into());
        self.in_request = None;
        read
    }

    /// The kernel text the request stands for: the entry `request (…) { …
    /// }`, with the statements before the admission, then the hold of
    /// what `taking` takes, its bindings, its body and its cache.
    fn kernel_text(
        &self,
        request: &RequestSrc,
        items: &RequestItems,
        taking: Option<&Taking>,
    ) -> Vec<Token> {
        let like = self.toks[request.at].clone();
        let mut out = vec![like.clone()];
        out.extend(request.params.iter().cloned());
        out.push(made(Tok::LBrace, &like));
        out.extend(items.before.iter().cloned());
        let Some(t) = taking else {
            out.extend(items.after.iter().cloned());
            out.push(made(Tok::RBrace, &like));
            return out;
        };
        let at = self.toks[t.at].clone();
        let pools: Vec<&Token> = t.pools.iter().map(|p| &p.pool).collect();
        out.push(word("hold", &at));
        for (k, p) in t.pools.iter().enumerate() {
            if k > 0 {
                out.push(made(Tok::Comma, &p.pool));
            }
            out.push(p.pool.clone());
            out.extend(paren(cost(&[&p.pool], &p.units, &p.pool), &p.pool));
            if let Some(r) = &p.reserve {
                out.push(word("reserve", &p.pool));
                out.extend(paren(cost(&[&p.pool], r, &p.pool), &p.pool));
            }
        }
        if let Some((p, segs)) = &items.prefix {
            let p = self.toks[*p].clone();
            out.push(word("prefix", &p));
            out.push(made(Tok::LParen, &p));
            out.extend(segs.iter().cloned());
            out.push(made(Tok::RParen, &p));
        }
        if let Some(e) = &t.reuse {
            out.push(word("reuse", &at));
            out.extend(paren(cost(&pools, e, &at), &at));
        }
        if let Some((a, binds)) = &items.admission
            && !binds.is_empty()
        {
            let a = self.toks[*a].clone();
            out.push(word("at", &a));
            out.push(word("admission", &a));
            out.push(made(Tok::LParen, &a));
            for (k, b) in binds.iter().enumerate() {
                if k > 0 {
                    out.push(made(Tok::Comma, &b.name));
                }
                out.push(b.name.clone());
                out.push(made(Tok::Assign, &b.name));
                out.extend(b.value.iter().cloned());
            }
            out.push(made(Tok::RParen, &a));
        }
        out.push(made(Tok::LBrace, &like));
        out.extend(items.after.iter().cloned());
        out.push(made(Tok::RBrace, &like));
        if let Some((c, e)) = &items.cache {
            let c = self.toks[*c].clone();
            out.push(word("cache", &c));
            out.extend(paren(cost(&pools, e, &c), &c));
        }
        out.push(made(Tok::Semi, &like));
        out.push(made(Tok::RBrace, &like));
        out
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
    /// IR; the rest of the program, another engine's request included,
    /// reads what the request marks, not them.
    pub(super) fn check_request_names(&self, prog: &Program) -> PResult<()> {
        for r in &self.request_names {
            if let Some((other, l)) = self.request_names.iter().find_map(|o| {
                (o.engine != r.engine)
                    .then(|| r.names.iter().find(|l| o.names.contains(l)).map(|l| (o, l)))
                    .flatten()
            }) {
                return self.err_at(
                    r.at,
                    format!(
                        "`{l}` is engine `{}`'s request's own and engine `{}`'s too: one \
                         attribute would carry both, mixed when the requests run beside each \
                         other\nhelp: name it otherwise in one of them",
                        r.engine, other.engine
                    ),
                );
            }
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
            let found = r
                .names
                .iter()
                .find(|l| assigned.contains(l) || outside.iter().any(|s| stmt_reads(s, l)));
            if let Some(l) = found {
                return self.err_at(
                    r.at,
                    format!(
                        "`{l}` is engine `{}`'s request's own, and the program reads or sets \
                         it outside the request\nhelp: name it otherwise there, or `mark` a moment \
                         the caller reads as `{}.NAME`",
                        r.engine, r.engine
                    ),
                );
            }
        }
        Ok(())
    }

    /// An engine is run only through its request (#539): the server, a
    /// gateway's `route` and the session may not run an engine's prefill
    /// or decode, nor hold a pool an engine admits; a queue's entry and an
    /// engine's request run their own engine and no other. A pool no engine
    /// admits (a top-level pool, one `on DEVICE`) is held anywhere. Read
    /// before `assemble` writes the entries out, so who admits a pool is
    /// read from the `on` clauses, which `link_engines` turns into
    /// `admit_via` after it.
    pub(super) fn check_engine_runs(&self, prog: &Program) -> PResult<()> {
        let admitter = |pool: &str| -> Option<String> {
            let po = self
                .pools_on
                .iter()
                .find(|po| prog.pools[po.pool].name == pool)?;
            if self.engines.iter().any(|e| e.name == po.on.owner) {
                Some(po.on.owner.clone())
            } else {
                po.on.admitted_by.clone()
            }
        };
        let engines = Engines {
            admitter: &admitter,
            is_engine: &|stage: &str| self.engines.iter().any(|e| e.name == stage),
            // a queue's engine is run through the queue's entries
            is_queue: &|stage: &str| self.queues.iter().any(|q| !q.engine && q.name == stage),
        };
        let mut blocks: Vec<(String, usize, &[Stmt], Option<&str>)> = vec![];
        if let Some((at, s)) = &self.server {
            blocks.push(("the server".into(), *at, s, None));
        }
        if let Some((at, s)) = &self.wl_session {
            blocks.push(("the session".into(), *at, s, None));
        }
        for q in &self.queues {
            let gateway = q.roles.iter().any(|r| r == "gateway");
            for e in &q.entries {
                if gateway && e.verb == "route" {
                    blocks.push(("the gateway's `route`".into(), e.at, &e.body, None));
                } else {
                    let who = if q.engine {
                        format!("engine `{}`'s request", q.name)
                    } else {
                        format!("`{}.{}`", q.name, e.verb)
                    };
                    blocks.push((who, e.at, &e.body, Some(q.name.as_str())));
                }
            }
        }
        for (who, at, stmts, own) in blocks {
            if let Some((span, msg)) = engine_use(stmts, &engines, &who, own) {
                return match span {
                    Some(s) => Err(ParseError {
                        line: s.line,
                        col: s.col,
                        msg,
                    }),
                    None => self.err_at(at, msg),
                };
            }
        }
        Ok(())
    }
}

/// What `engine_use` asks of the program: who admits a pool, and which
/// stages are engines and which of those are queues'.
struct Engines<'a> {
    admitter: &'a dyn Fn(&str) -> Option<String>,
    is_engine: &'a dyn Fn(&str) -> bool,
    is_queue: &'a dyn Fn(&str) -> bool,
}

/// The first statement of `stmts` that runs an engine outside its own
/// request or queue: where it is and what to write instead. `own` is the
/// queue or the engine whose entry `stmts` is, which holds its own pools.
fn engine_use(
    stmts: &[Stmt],
    en: &Engines,
    who: &str,
    own: Option<&str>,
) -> Option<(Option<Span>, String)> {
    let instead = |e: &str| {
        if (en.is_queue)(e) {
            format!("call one of queue `{e}`'s entries (`{e}.VERB (…);`)")
        } else {
            format!(
                "write it in `{e}`'s `request (…) {{ … }}`, which the server calls as `{e} (…);`"
            )
        }
    };
    for s in stmts {
        let found = match s {
            Stmt::Run { stage, mode, .. }
                if *mode != RunMode::Plain
                    && (en.is_engine)(&stage.name)
                    && own != Some(stage.name.as_str()) =>
            {
                let (e, m) = (
                    &stage.name,
                    if *mode == RunMode::Prefill {
                        "prefill"
                    } else {
                        "decode"
                    },
                );
                Some((
                    stage.span,
                    format!(
                        "{who} runs engine `{e}`'s {m} (`run {e} {m} (…)`): an engine runs only \
                         in its own request or queue; {}",
                        instead(e)
                    ),
                ))
            }
            Stmt::Hold { pools, body, .. } => pools
                .iter()
                .filter(|_| own.is_none())
                .find_map(|(p, _, _)| (en.admitter)(&p.name).map(|e| (p, e)))
                .map(|(p, e)| {
                    let to = if (en.is_queue)(&e) {
                        format!("; {}", instead(&e))
                    } else {
                        format!(
                            ": write the admission as `taking {} (…)` on `{e}`'s `admit waiting`, and \
                             what the hold does in `{e}`'s `request (…) {{ … }}`, which {who} calls \
                             as `{e} (…);`",
                            p.name
                        )
                    };
                    (p.span, format!("{who} holds `{}`, which engine `{e}` admits{to}", p.name))
                })
                .or_else(|| engine_use(body, en, who, own)),
            Stmt::Branch(_, a, b) => engine_use(a, en, who, own).or_else(|| engine_use(b, en, who, own)),
            Stmt::Loop(b) | Stmt::While(_, b) | Stmt::Fork(b) => engine_use(b, en, who, own),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}
