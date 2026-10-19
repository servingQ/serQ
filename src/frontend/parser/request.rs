//! An engine's request (`docs/design/scheduler-worker.md`, step 3): the
//! server calls the engine, `vllm (prompt, o);`, and the engine keeps the
//! request's state as vLLM keeps a `Request`. What its admission takes is
//! written once, on the scheduler's `admit waiting`:
//!
//! ```text
//! engine vllm on gpu {
//!   scheduler {
//!     …
//!     admit waiting while (running.preempted == 0)
//!       taking reqs (1), kv (min(request.known, request.hit + budget_left(vllm)))
//!         reserve (request.known)
//!       reuse (floor(request.known - 1, blocksize(kv)))
//!       cache (request.input + request.output);
//!   }
//! }
//! ```
//!
//! and lowers to the engine's `ir::CRequest`; the call is `ir::CStmt::Request`.

use super::*;

/// `taking …` as written on one `admit waiting`: its tokens, which every
/// `admit waiting` of the engine repeats, and what they say.
pub(super) type Taking = (Vec<Token>, RequestSpec);

impl Parser {
    /// `taking …` on `admit waiting`, if it is next: the request's
    /// admission, read up to the `;` the statement ends with.
    pub(super) fn taking_src(&mut self) -> PResult<()> {
        let at = self.pos;
        if !self.eat_kw("taking") {
            self.takings.push((at, None));
            return Ok(());
        }
        let start = self.pos;
        let spec = self.taking()?;
        let toks = self.toks[start..self.pos].to_vec();
        self.takings.push((at, Some((toks, spec))));
        Ok(())
    }

    /// `P (u) [reserve (r)], … [reuse (e)] [prefix (k: n, …)] [cache (e)]
    /// [lease P (t)]`: the pools the admission takes and how much, what it
    /// may reuse of the cache, what its sequence is made of, what its
    /// release leaves cached, and the pool it keeps past its end for `t`
    /// seconds at most, as a hold's `lease`. The pools are checked where the
    /// engine ends, with its capacities.
    fn taking(&mut self) -> PResult<RequestSpec> {
        let mut pools = vec![];
        loop {
            let span = Some(self.span());
            let name = self.ident()?;
            if ["reuse", "prefix", "cache", "reserve"].contains(&name.as_str()) {
                self.pos -= 1;
                return self.err(
                    "`taking` names the pools the admission takes first: `taking reqs (1), kv \
                     (request.known)`",
                );
            }
            let pool = Ref {
                span,
                name,
                index: None,
            };
            let units = self.engine_expr("taking", |p| p.paren_expr())?;
            let reserve = if self.eat_kw("reserve") {
                Some(self.engine_expr("taking", |p| p.paren_expr())?)
            } else {
                None
            };
            pools.push((pool, units, reserve));
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        let reuse = if self.eat_kw("reuse") {
            Some(self.engine_expr("taking", |p| p.paren_expr())?)
        } else {
            None
        };
        let prefix = if self.eat_kw("prefix") {
            self.expect(&Tok::LParen)?;
            let mut segs = vec![];
            loop {
                let k = self.engine_expr("taking", |p| p.expr())?;
                self.expect(&Tok::Colon)?;
                segs.push((k, self.engine_expr("taking", |p| p.expr())?));
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen)?;
            Some(segs)
        } else {
            None
        };
        let cache = if self.eat_kw("cache") {
            Some(self.engine_expr("cache", |p| p.paren_expr())?)
        } else {
            None
        };
        let lease = if self.eat_kw("lease") {
            let span = Some(self.span());
            let name = self.ident()?;
            let pool = Ref {
                span,
                name,
                index: None,
            };
            Some((pool, self.engine_expr("lease", |p| p.paren_expr())?))
        } else {
            None
        };
        Ok(RequestSpec {
            pools,
            reuse,
            prefix,
            cache,
            lease,
        })
    }

    /// Engine `engine` is read: its request's admission, the `taking` every
    /// `admit waiting` writes, each pool one of `pools` (its capacities and
    /// its device's). No `taking`: the request takes nothing.
    pub(super) fn engine_request(
        &mut self,
        engine: &str,
        pools: &[String],
    ) -> PResult<RequestSpec> {
        let takings = std::mem::take(&mut self.takings);
        let Some((at, (first, spec))) = takings
            .iter()
            .find_map(|(at, t)| t.as_ref().map(|t| (*at, t)))
        else {
            return Ok(RequestSpec::default());
        };
        let same = |t: &Option<Taking>| {
            t.as_ref().is_some_and(|(toks, _)| {
                toks.len() == first.len() && toks.iter().zip(first).all(|(a, b)| a.tok == b.tok)
            })
        };
        if let Some((other, _)) = takings.iter().find(|(_, t)| !same(t)) {
            return self.err_at(
                *other,
                format!(
                    "every `admit waiting` of engine `{engine}` admits its requests: write the \
                     same `taking` on each"
                ),
            );
        }
        for (k, (r, _, _)) in spec.pools.iter().enumerate() {
            if !pools.contains(&r.name) {
                return self.err_at(
                    at,
                    format!(
                        "engine `{engine}`'s `taking` names `{}`, which is no capacity of the \
                         engine or its device: it takes {}",
                        r.name,
                        pools
                            .iter()
                            .map(|q| format!("`{q}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
            if spec.pools[..k].iter().any(|(q, _, _)| q.name == r.name) {
                return self.err_at(at, format!("`taking` names `{}` twice", r.name));
            }
        }
        if let Some((r, _)) = &spec.lease
            && !spec.pools.iter().any(|(q, _, _)| q.name == r.name)
        {
            return self.err_at(
                at,
                format!(
                    "engine `{engine}`'s `lease {}`: its `taking` does not take `{}`, so the \
                     request has none of it to keep",
                    r.name, r.name
                ),
            );
        }
        Ok(spec.clone())
    }

    /// `E (input, output);` in a server, or in an entry of queue `E` for its
    /// own engine: a request to engine `E`, `input` tokens of prompt and
    /// `output` to generate.
    pub(super) fn engine_call(&mut self) -> PResult<Stmt> {
        let at = self.pos;
        let span = Some(self.span());
        let name = self.ident()?;
        let own = self.in_queue.is_some_and(|qi| self.queues[qi].name == name);
        if !own && self.queues.iter().any(|q| q.name == name) {
            return self.err_at(
                at,
                format!(
                    "`{name} (…)` is a request to queue `{name}`'s engine, which its entries make: \
                     call one of them (`{name}.VERB (…);`)"
                ),
            );
        }
        if !own && self.side != Side::Server {
            return self.err_at(
                at,
                format!("`{name} (…)` is a request to engine `{name}`, which a `server` makes"),
            );
        }
        self.expect(&Tok::LParen)?;
        let input = self.expr()?;
        self.expect(&Tok::Comma)?;
        let output = self.expr()?;
        if *self.peek() != Tok::RParen {
            return self.err(format!(
                "`{name} (input, output)`: a request is its prompt's tokens and the tokens it \
                 generates; what else the engine reads of it, it reads as the session's \
                 attributes"
            ));
        }
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::Semi)?;
        Ok(Stmt::EngineRequest {
            stage: Ref {
                span,
                name,
                index: None,
            },
            input,
            output,
        })
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
            self.admitted_by(po)
        };
        let engines = Engines {
            admitter: &admitter,
            is_engine: &|stage: &str| self.engines.iter().any(|e| e.name == stage),
            // a queue's engine is run through the queue's entries
            is_queue: &|stage: &str| self.queues.iter().any(|q| q.name == stage),
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
                if gateway && e.verb == queue::GATEWAY_VERB {
                    blocks.push(("the gateway's `route`".into(), e.at, &e.body, None));
                } else {
                    let who = format!("`{}.{}`", q.name, e.verb);
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
            format!("call the engine, `{e} (input, output);`, which keeps the request's state")
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
                         in its own queue's entries; {}",
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
                            ": write the admission as `taking {} (…)` on `{e}`'s `admit waiting`, \
                             and call the engine, `{e} (input, output);`",
                            p.name
                        )
                    };
                    (
                        p.span,
                        format!("{who} holds `{}`, which engine `{e}` admits{to}", p.name),
                    )
                })
                .or_else(|| engine_use(body, en, who, own)),
            Stmt::Branch(_, a, b) => {
                engine_use(a, en, who, own).or_else(|| engine_use(b, en, who, own))
            }
            Stmt::Loop(b) | Stmt::While(_, b) | Stmt::Fork(b) => engine_use(b, en, who, own),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}
