//! The deployment view: a seQ program as a queueing network.
//!
//! Pools and stages are declared, but the arrows between them are not: the
//! flow is a property of the session program. This module projects it onto the
//! stages — the statements disappear, the `Run`s become stations, and the
//! `Hold`s become the boundaries drawn around them — and lays the result out.
//!
//! See `tests/draw.rs`.

use std::collections::BTreeMap;

use crate::ir::{CArg, CArrival, CExpr, CStageKind, CStmt, Program};
use crate::view::figure::{
    Anchor, BoxStyle, EdgeStyle, Figure, Item, Rect, StationKind, TextSize, pt,
};

// --- the projection ---------------------------------------------------------

/// Where an edge starts or ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Sessions entering the system.
    Arrival,
    /// A station, by index into `Net::nodes`.
    Node(usize),
    /// A session ending.
    Exit,
}

/// A station: one stage the session reaches.
#[derive(Clone, Debug)]
pub struct Node {
    pub stage: usize,
    /// `engine`, `rep[4]`.
    pub label: String,
    pub kind: StationKind,
    /// The discipline written inside the glyph: `FIFO`, `PS`, `step`.
    pub inner: String,
    /// A line under the glyph: the capacity, the budget, a `choose` key.
    pub note: Option<String>,
    /// Pools held around every visit to this stage, outermost first.
    pub pools: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: End,
    pub to: End,
    pub label: Option<String>,
    /// A back edge: it returns to somewhere already drawn.
    pub back: bool,
}

/// The session program, projected onto the stages.
#[derive(Clone, Debug, Default)]
pub struct Net {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// How sessions arrive, as a label.
    pub arrival: String,
    /// Pools any `hold` caches a prefix in.
    pub cached: Vec<usize>,
}

impl Net {
    /// The node for a stage, if the session reaches it.
    pub fn node_of(&self, stage: usize) -> Option<usize> {
        self.nodes.iter().position(|n| n.stage == stage)
    }
    /// Whether an edge exists between two ends, in that direction.
    pub fn has_edge(&self, from: End, to: End) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to)
    }
}

struct Walker<'a> {
    p: &'a Program,
    net: Net,
    /// Ends the next station will be reached from, with the label of the path.
    frontier: Vec<(End, Option<String>)>,
    /// Pools held right now, outermost first, each with the hold that took
    /// it (a `release` takes one off before its hold ends).
    holds: Vec<(usize, usize)>,
    next_hold: usize,
    /// `choose`s that have not yet found the station they select, as
    /// (attribute slot, label). A `choose` names an index, so it belongs to
    /// the station whose reference reads that attribute - `rep[j]`, not
    /// whatever station happens to come next.
    pending: Vec<(usize, String)>,
    /// The guard of the branch arm being walked, for the first edge it takes.
    arm: Option<String>,
}

impl Walker<'_> {
    /// The pending `choose` this index expression selects with, if any.
    fn claim_choose(&mut self, index: &CExpr) -> Option<String> {
        let i = self
            .pending
            .iter()
            .position(|(v, _)| reads_attr(index, *v))?;
        Some(self.pending.remove(i).1)
    }

    fn attach(&mut self, node: usize) {
        let arm = self.arm.take();
        let frontier = std::mem::take(&mut self.frontier);
        for (from, label) in frontier {
            // Two runs at the same stage in a row are two visits, not a flow
            // between stations; there is nothing to draw.
            if from == End::Node(node) {
                continue;
            }
            let label = label.or_else(|| arm.clone());
            // A back edge is one that returns to a station already passed.
            let back = matches!(from, End::Node(i) if i >= node);
            self.push_edge(Edge {
                from,
                to: End::Node(node),
                label,
                back,
            });
        }
        self.frontier = vec![(End::Node(node), None)];
    }

    /// Add an edge unless the same one is already there. Programs written as
    /// a chain of guards (`routing.seq`'s five policies) reach the same
    /// station down many paths; the picture wants one arrow.
    fn push_edge(&mut self, e: Edge) {
        if let Some(existing) = self
            .net
            .edges
            .iter_mut()
            .find(|x| x.from == e.from && x.to == e.to)
        {
            // Keep the shorter label: it is the one that reads.
            match (&existing.label, &e.label) {
                (Some(a), Some(b)) if b.len() < a.len() => existing.label = e.label,
                (None, Some(_)) => existing.label = e.label,
                _ => {}
            }
            existing.back &= e.back;
            return;
        }
        self.net.edges.push(e);
    }

    fn visit_stage(&mut self, stage: usize, label: String, note: Option<String>) {
        let idx = match self.net.node_of(stage) {
            Some(i) => {
                // Seen before: it belongs only to the pools held every time.
                let held = &self.holds;
                self.net.nodes[i]
                    .pools
                    .retain(|p| held.iter().any(|&(q, _)| q == *p));
                i
            }
            None => {
                let (kind, inner, kind_note) = station_of(self.p, stage);
                self.net.nodes.push(Node {
                    stage,
                    label,
                    kind,
                    inner,
                    note: kind_note,
                    pools: self.holds.iter().fold(vec![], |mut v, &(q, _)| {
                        // nested holds of one pool are one enclosure
                        if !v.contains(&q) {
                            v.push(q);
                        }
                        v
                    }),
                });
                self.net.nodes.len() - 1
            }
        };
        if let Some(n) = note
            && self.net.nodes[idx].note.is_none()
        {
            self.net.nodes[idx].note = Some(n);
        }
        self.attach(idx);
    }

    fn walk(&mut self, block: usize) {
        let Some(stmts) = self.p.blocks.get(block) else {
            return;
        };
        for s in stmts.clone() {
            match s {
                CStmt::Run { stage, .. } => {
                    let label = self.p.show_stage_ref(&stage);
                    let note = stage.index.as_ref().and_then(|i| self.claim_choose(i));
                    self.visit_stage(stage.base, label, note);
                }
                CStmt::Hold {
                    pools, body, lease, ..
                } => {
                    let id = self.next_hold;
                    self.next_hold += 1;
                    for (r, _, _) in &pools {
                        self.holds.push((r.base, id));
                    }
                    self.walk(body);
                    // by hold, not by depth: a `release` inside may have
                    // taken an outer hold's pool off the stack already
                    let leased = lease
                        .as_ref()
                        .filter(|(r, _)| self.holds.iter().any(|&(q, h)| q == r.base && h == id));
                    self.holds.retain(|&(_, h)| h != id);
                    // a leased pool stays held past the scope, until the
                    // `release` that takes it
                    if let Some((r, _)) = leased {
                        self.holds.push((r.base, id));
                    }
                }
                CStmt::Release(r) => {
                    // the pool leaves the enclosure here: the stations after
                    // this one are not inside it
                    if let Some(i) = self.holds.iter().rposition(|&(p, _)| p == r.base) {
                        self.holds.remove(i);
                    }
                }
                CStmt::Branch(c, t, e) => {
                    // The guard labels the first edge the arm takes. An arm
                    // with no station of its own contributes no label, which
                    // is what keeps a chain of guards from multiplying out.
                    let saved = self.frontier.clone();
                    let outer = self.arm.take();
                    // each arm starts from the holds at the branch; a
                    // `release` in one arm does not reach the other, and
                    // after the branch a pool is held only where both arms
                    // still hold it
                    let held = self.holds.clone();
                    self.frontier = saved.clone();
                    self.arm = Some(self.p.show_guard(&c));
                    self.walk(t);
                    let then_out = std::mem::take(&mut self.frontier);
                    let then_held = std::mem::replace(&mut self.holds, held);
                    self.frontier = saved;
                    self.arm = Some("else".into());
                    self.walk(e);
                    self.holds.retain(|h| then_held.contains(h));
                    let mut out = then_out;
                    out.append(&mut self.frontier);
                    dedupe(&mut out);
                    self.frontier = out;
                    self.arm = outer;
                }
                CStmt::Loop(body) => {
                    // The second pass starts where the first ended, so every
                    // way back into the body is drawn, from every arm, with
                    // the guard of the arm it takes; edges already there are
                    // not drawn twice.
                    self.walk(body);
                    self.walk(body);
                    self.frontier.clear();
                    // A loop is left only by `end`, which already recorded it.
                }
                CStmt::End => {
                    let arm = self.arm.take();
                    let frontier = std::mem::take(&mut self.frontier);
                    for (from, label) in frontier {
                        let label = label.or_else(|| arm.clone());
                        self.push_edge(Edge {
                            from,
                            to: End::Exit,
                            label,
                            back: false,
                        });
                    }
                    self.arm = arm;
                }
                CStmt::Choose { var, count, .. } => {
                    let name = self.p.attrs.get(var).map_or("?", String::as_str);
                    let label = format!("choose {name} of {}", self.p.show_expr(&count));
                    self.pending.retain(|(v, _)| *v != var);
                    self.pending.push((var, label));
                }
                CStmt::Turn | CStmt::Set(..) | CStmt::Observe(..) => {}
                CStmt::Grow(..) | CStmt::Drop(..) | CStmt::Load(..) => {}
            }
        }
    }
}

/// Keep one entry per end: the arms of a guard that moved nobody all rejoin.
fn dedupe(frontier: &mut Vec<(End, Option<String>)>) {
    let mut seen: Vec<End> = vec![];
    frontier.retain(|(e, _)| {
        if seen.contains(e) {
            false
        } else {
            seen.push(*e);
            true
        }
    });
}

/// Does an expression read this attribute?
fn reads_attr(e: &CExpr, slot: usize) -> bool {
    match e {
        CExpr::Attr(s) => *s == slot,
        CExpr::Num(_) | CExpr::Ctx(_) => false,
        CExpr::Sample(_, a) => a.iter().any(|x| reads_attr(x, slot)),
        CExpr::Call(_, a) => a.iter().any(|x| match x {
            CArg::Expr(x) => reads_attr(x, slot),
            CArg::Pool(r) | CArg::Stage(r) => r.index.as_ref().is_some_and(|i| reads_attr(i, slot)),
        }),
        CExpr::Unary(_, a) => reads_attr(a, slot),
        CExpr::Binary(_, a, b) => reads_attr(a, slot) || reads_attr(b, slot),
        CExpr::Cond(c, a, b) => reads_attr(c, slot) || reads_attr(a, slot) || reads_attr(b, slot),
    }
}

fn station_of(p: &Program, stage: usize) -> (StationKind, String, Option<String>) {
    match &p.stages[stage].kind {
        CStageKind::Fifo(c) => {
            let note = (*c != 1).then(|| format!("{c} servers"));
            (StationKind::Fifo, "FIFO".into(), note)
        }
        CStageKind::Ps(phi) => (StationKind::Ps, "PS".into(), Some(p.show_expr(phi))),
        CStageKind::Delay => (StationKind::Delay, String::new(), Some("delay".into())),
        CStageKind::Step(s) => (
            StationKind::Step,
            "step".into(),
            Some(format!("budget {}", p.show_expr(&s.budget))),
        ),
    }
}

fn arrival_label(p: &Program) -> String {
    match &p.arrival {
        CArrival::Poisson(r) => format!("Poisson {}", crate::ir::show_num(*r)),
        CArrival::Renewal(e) => format!("renewal interarrival {}", p.show_expr(e)),
        CArrival::Closed(n) => format!("closed, {n} live"),
        CArrival::Batch(n) => format!("{n} at t=0"),
        CArrival::Sessions(s) => {
            let turns: usize = s.iter().map(|x| x.turns.len()).sum();
            if turns == 0 {
                format!("{} sessions", s.len())
            } else {
                format!("{} sessions, {turns} turns", s.len())
            }
        }
        CArrival::None => "no arrivals".into(),
    }
}

/// Project the session program onto the stages.
pub fn project(p: &Program) -> Net {
    let mut w = Walker {
        p,
        net: Net {
            arrival: arrival_label(p),
            ..Net::default()
        },
        frontier: vec![(End::Arrival, None)],
        holds: vec![],
        next_hold: 0,
        pending: vec![],
        arm: None,
    };
    w.walk(p.session);
    // Anything still on the frontier ran off the end of the session program.
    let frontier = std::mem::take(&mut w.frontier);
    for (from, label) in frontier {
        if from != End::Arrival {
            w.push_edge(Edge {
                from,
                to: End::Exit,
                label,
                back: false,
            });
        }
    }
    let mut cached: Vec<usize> = vec![];
    for pools in cache_targets(p).values() {
        for &pool in pools {
            if !cached.contains(&pool) {
                cached.push(pool);
            }
        }
    }
    w.net.cached = cached;
    w.net
}

/// For every `hold` with a `cache` clause, the pools that clause can leave
/// units in - keyed by the hold's body block, which is unique to it.
///
/// `release_hold` (`interp.rs`) caches `min(cache, computed)` per pool, where
/// `computed` is the allocation unless the hold grew, and then it is the
/// position the growing run reached, which only a grown pool has. `grow`
/// advances the *innermost* hold holding that pool, so a `growing` run deep
/// inside nested holds can belong to an outer one, and a hold may be grown in
/// more than one pool. `examples/multi-turn/replica.seq` is the case that makes this
/// visible: its `hold batch (1), kv (...)` has no `growing` at all and really
/// does keep a unit of `batch` cached.
pub(crate) fn cache_targets(p: &Program) -> BTreeMap<usize, Vec<usize>> {
    struct Frame {
        body: usize,
        pools: Vec<usize>,
        grown: Vec<usize>,
    }
    fn walk(
        p: &Program,
        block: usize,
        stack: &mut Vec<Frame>,
        out: &mut BTreeMap<usize, Vec<usize>>,
    ) {
        let Some(stmts) = p.blocks.get(block) else {
            return;
        };
        for s in stmts {
            match s {
                CStmt::Hold {
                    pools, body, cache, ..
                } => {
                    stack.push(Frame {
                        body: *body,
                        pools: pools.iter().map(|(r, _, _)| r.base).collect(),
                        grown: vec![],
                    });
                    walk(p, *body, stack, out);
                    let f = stack.pop().expect("pushed above");
                    if cache.is_some() {
                        let targets = if f.grown.is_empty() {
                            f.pools.clone()
                        } else {
                            f.grown.clone()
                        };
                        out.insert(f.body, targets);
                    }
                }
                CStmt::Run {
                    growing: Some(g), ..
                } => {
                    // the innermost hold that holds this pool is the one that grows
                    if let Some(f) = stack.iter_mut().rev().find(|f| f.pools.contains(&g.base))
                        && !f.grown.contains(&g.base)
                    {
                        f.grown.push(g.base);
                    }
                }
                // a load computes into the hold the way a growing run does
                CStmt::Grow(g, _) | CStmt::Load(g, _) => {
                    if let Some(f) = stack.iter_mut().rev().find(|f| f.pools.contains(&g.base))
                        && !f.grown.contains(&g.base)
                    {
                        f.grown.push(g.base);
                    }
                }
                CStmt::Branch(_, t, e) => {
                    walk(p, *t, stack, out);
                    walk(p, *e, stack, out);
                }
                CStmt::Loop(b) => walk(p, *b, stack, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(p, p.session, &mut vec![], &mut out);
    out
}

// --- layout -----------------------------------------------------------------

const STATION_W: f64 = 84.0;
const STATION_H: f64 = 52.0;
const GAP: f64 = 74.0;
const GLYPH_W: f64 = 78.0;
const PAD: f64 = 20.0;
const DEPTH_PAD: f64 = 15.0;
const MARGIN: f64 = 28.0;
const BACK_DROP: f64 = 26.0;
/// Extra room under an enclosure that carries a prefix-cache strip.
const CACHE_ROOM: f64 = 20.0;
/// Baseline-to-baseline for the stacked pool options.
const NOTE_LINE: f64 = 11.0;

/// A pool's extent over the station row, and the geometry the walk gives it.
struct Group {
    pool: usize,
    first: usize,
    last: usize,
    depth: usize,
    /// Left edge of the enclosure, set by the left-to-right walk.
    left: f64,
    /// Right edge of the enclosure.
    right: f64,
    /// Left edge of the column reserved for this pool's own glyphs.
    glyph_x: f64,
}

/// The enclosures: one per **maximal run of consecutive stations** a pool is
/// held across, not one per pool.
///
/// A pool held around two stations with an unheld one between them (two
/// separate `hold`s of the same pool) would otherwise get a box that swallows
/// the station in the middle, and the figure would assert a hold the program
/// does not make.
fn groups(net: &Net) -> Vec<Group> {
    let mut runs: Vec<(usize, usize, usize)> = vec![];
    let pools: Vec<usize> = {
        let mut v: Vec<usize> = net.nodes.iter().flat_map(|n| n.pools.clone()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    for pool in pools {
        let mut i = 0;
        while i < net.nodes.len() {
            if !net.nodes[i].pools.contains(&pool) {
                i += 1;
                continue;
            }
            let start = i;
            while i < net.nodes.len() && net.nodes[i].pools.contains(&pool) {
                i += 1;
            }
            runs.push((pool, start, i - 1));
        }
    }
    let mut gs: Vec<Group> = runs
        .into_iter()
        .map(|(pool, first, last)| Group {
            pool,
            first,
            last,
            depth: 0,
            left: 0.0,
            right: 0.0,
            glyph_x: 0.0,
        })
        .collect();
    // A group is nested inside every group whose span strictly contains it;
    // equal spans are ordered by how deep the hold that made them sits, which
    // the node's own pool order records.
    for i in 0..gs.len() {
        let (f, l, p) = (gs[i].first, gs[i].last, gs[i].pool);
        let mut d = 0;
        for (j, o) in gs.iter().enumerate() {
            if i == j {
                continue;
            }
            let contains = o.first <= f && o.last >= l;
            let strict = contains && (o.first < f || o.last > l);
            let same = contains && o.first == f && o.last == l;
            if strict {
                d += 1;
            } else if same {
                let order = net.nodes[f].pools.iter().position(|&x| x == p);
                let other = net.nodes[f].pools.iter().position(|&x| x == o.pool);
                if other < order {
                    d += 1;
                }
            }
        }
        gs[i].depth = d;
    }
    gs
}

/// Lay a projected network out as a figure.
pub fn layout(p: &Program, net: &Net) -> Figure {
    let mut f = Figure {
        title: String::new(),
        ..Figure::default()
    };
    let mut gs = groups(net);
    let max_depth = gs.iter().map(|g| g.depth).max().unwrap_or(0);
    let row_y = MARGIN + 34.0 + (max_depth as f64 + 1.0) * DEPTH_PAD;
    let cached_here = |g: &Group| net.cached.contains(&g.pool);
    // Room under the station row for the stacked pool options and the strip.
    let own_below = |g: &Group| {
        let notes = (pool_notes(p, g.pool, cached_here(g)).len() as f64 * NOTE_LINE + 24.0
            - STATION_H / 2.0
            - PAD)
            .max(0.0);
        notes + if cached_here(g) { CACHE_ROOM } else { 0.0 }
    };
    // An enclosure has to reach below everything nested in it, or the inner
    // box hangs out of the outer one. A deeper group sits `DEPTH_PAD` lower
    // per level, so its parent needs that much less of its own room.
    let mut below: Vec<f64> = gs.iter().map(&own_below).collect();
    let mut order: Vec<usize> = (0..gs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(gs[i].depth));
    for &i in &order {
        for j in 0..gs.len() {
            let (inner, outer) = (&gs[i], &gs[j]);
            let contains =
                outer.first <= inner.first && outer.last >= inner.last && outer.depth < inner.depth;
            if contains {
                let step = DEPTH_PAD * (inner.depth - outer.depth) as f64;
                below[j] = below[j].max(below[i] - step + 6.0);
            }
        }
    }

    // The arrival arrow has to be long enough for the label that sits over it.
    let arrival_text = format!("new sessions: {}", net.arrival);
    let lead = TextSize::Small.width_of(&arrival_text).max(70.0) + 14.0;

    // Walk left to right, reserving room for the enclosures that open and
    // close around each station and for the pool glyphs inside them.
    let mut x = MARGIN + lead;
    let mut rects: Vec<Rect> = Vec::with_capacity(net.nodes.len());
    for i in 0..net.nodes.len() {
        // Enclosures open outermost first, so that a nested pool's glyph
        // column sits beside its parent's rather than on top of it.
        let mut opening: Vec<usize> = (0..gs.len()).filter(|&k| gs[k].first == i).collect();
        opening.sort_by_key(|&k| gs[k].depth);
        for k in opening {
            let d = DEPTH_PAD * (max_depth - gs[k].depth) as f64;
            gs[k].left = x;
            gs[k].glyph_x = x + PAD + d;
            x += PAD + d + GLYPH_W;
        }
        rects.push(Rect::new(x, row_y, STATION_W, STATION_H));
        x += STATION_W;
        let mut closing: Vec<usize> = (0..gs.len()).filter(|&k| gs[k].last == i).collect();
        closing.sort_by_key(|&k| std::cmp::Reverse(gs[k].depth));
        for k in closing {
            x += PAD + DEPTH_PAD * (max_depth - gs[k].depth) as f64;
            gs[k].right = x;
        }
        if i + 1 < net.nodes.len() {
            x += GAP;
        }
    }
    let row_right = x;

    // Enclosures first, so stations and glyphs paint over them.
    for (gi, g) in gs.iter().enumerate() {
        let pool = &p.pools[g.pool];
        let d = (max_depth - g.depth) as f64 * DEPTH_PAD;
        let inner = rects[g.first].union(&rects[g.last]);
        let extra = below[gi];
        let r = Rect::new(
            g.left,
            inner.y - PAD - d,
            g.right - g.left,
            inner.h + 2.0 * (PAD + d) + extra,
        );
        f.boxed(r, BoxStyle::Enclosure, 8.0);

        // The queue glyph and the capacity, in the room reserved at the left.
        let gx = g.glyph_x;
        let gy = inner.centre().y;
        f.text(
            pt(gx, r.y - 5.0),
            format!("pool {}", pool.name),
            Anchor::Start,
            TextSize::Normal,
        );
        f.push(Item::Queue {
            rect: Rect::new(gx, gy - 11.0, 30.0, 22.0),
            cells: 3,
        });
        let (cols, rows) = slot_grid(pool.cap);
        f.push(Item::Slots {
            rect: Rect::new(gx + 38.0, gy - 11.0, 28.0, 22.0),
            cols,
            rows,
        });
        for (k, line) in pool_notes(p, g.pool, cached_here(g))
            .into_iter()
            .enumerate()
        {
            f.note(
                pt(gx, gy + 24.0 + k as f64 * NOTE_LINE),
                line,
                Anchor::Start,
            );
        }
        if cached_here(g) {
            let strip = Rect::new(gx, r.bottom() - 15.0, r.right() - gx - 10.0, 11.0);
            f.boxed(strip, BoxStyle::Cached, 2.0);
            f.note(
                pt(strip.centre().x, strip.y + 8.5),
                "prefix cache",
                Anchor::Middle,
            );
        }
        if let Some(via) = pool.admit_via {
            if let Some(n) = net.node_of(via) {
                f.push(Item::Edge {
                    pts: vec![
                        pt(gx + 15.0, gy - 13.0),
                        pt(rects[n].centre().x, rects[n].y - 6.0),
                    ],
                    style: EdgeStyle::Relation,
                    arrow: true,
                });
                f.note(pt(gx + 18.0, gy - 20.0), "admit via", Anchor::Start);
            }
        }
    }

    for (i, n) in net.nodes.iter().enumerate() {
        let r = rects[i];
        f.push(Item::Station {
            rect: r,
            kind: n.kind,
            text: n.inner.clone(),
        });
        f.text(
            pt(r.centre().x, r.y - 8.0),
            n.label.clone(),
            Anchor::Middle,
            TextSize::Normal,
        );
        if let Some(note) = &n.note {
            f.note(
                pt(r.centre().x, r.bottom() + 13.0),
                note.clone(),
                Anchor::Middle,
            );
        }
    }

    // One counter for every edge that needs a lane below the station row:
    // feedback, an early exit and a second entry point must not share one.
    let mut lanes = 0.0;
    let deep = below.iter().copied().fold(0.0, f64::max);
    let lane = |n: &mut f64| {
        *n += 1.0;
        row_y + STATION_H + deep + BACK_DROP + *n * 22.0
    };
    let mut arrivals = 0;
    for e in &net.edges {
        let label = e.label.clone();
        match (e.from, e.to) {
            (End::Arrival, End::Node(i)) => {
                let r = rects[i];
                let y = r.centre().y;
                arrivals += 1;
                if arrivals == 1 {
                    // The arrow starts at the margin and the label rides above
                    // it, clear of the pool glyphs it passes.
                    f.edge(vec![pt(MARGIN, y), pt(r.x, y)], EdgeStyle::Flow);
                    f.note(pt(MARGIN, y - 10.0), arrival_text.clone(), Anchor::Start);
                } else {
                    // A session that opens with a branch has more than one entry
                    // station. A second arrow along the row would run straight
                    // through the first one, so it takes a lane of its own.
                    let ly = lane(&mut lanes);
                    let x = r.x + r.w * 0.25;
                    f.push(Item::Edge {
                        pts: vec![pt(MARGIN, y), pt(MARGIN, ly), pt(x, ly), pt(x, r.bottom())],
                        style: EdgeStyle::Flow,
                        arrow: true,
                    });
                }
            }
            (End::Node(i), End::Exit) => {
                let r = rects[i];
                let text = label.map_or("ends".into(), |l| format!("ends ({l})"));
                if i + 1 == net.nodes.len() {
                    // The last station leaves to the right.
                    f.edge(
                        vec![
                            pt(r.right(), r.centre().y),
                            pt(row_right + 26.0, r.centre().y),
                        ],
                        EdgeStyle::Flow,
                    );
                    f.note(
                        pt(row_right + 30.0, r.centre().y + 3.0),
                        text,
                        Anchor::Start,
                    );
                } else {
                    // An earlier station's exit drops into a lane of its own
                    // rather than running a line through the stations after it.
                    let y = lane(&mut lanes);
                    f.push(Item::Edge {
                        pts: vec![
                            pt(r.x + r.w * 0.75, r.bottom()),
                            pt(r.x + r.w * 0.75, y),
                            pt(row_right + 26.0, y),
                        ],
                        style: EdgeStyle::Flow,
                        arrow: true,
                    });
                    f.note(pt(row_right + 30.0, y + 3.0), text, Anchor::Start);
                }
            }
            (End::Node(a), End::Node(b)) if !e.back => {
                let (ra, rb) = (rects[a], rects[b]);
                f.edge(
                    vec![pt(ra.right(), ra.centre().y), pt(rb.x, rb.centre().y)],
                    EdgeStyle::Flow,
                );
                if let Some(l) = label {
                    f.note(
                        pt((ra.right() + rb.x) / 2.0, ra.centre().y - 8.0),
                        l,
                        Anchor::Middle,
                    );
                }
            }
            (End::Node(a), End::Node(b)) => {
                let (ra, rb) = (rects[a], rects[b]);
                let y = lane(&mut lanes);
                let (ax, bx) = (ra.x + ra.w * 0.25, rb.x + rb.w * 0.25);
                f.push(Item::Edge {
                    pts: vec![
                        pt(ax, ra.bottom()),
                        pt(ax, y),
                        pt(bx, y),
                        pt(bx, rb.bottom()),
                    ],
                    style: EdgeStyle::Back,
                    arrow: true,
                });
                if let Some(l) = label {
                    f.note(
                        pt((ra.centre().x + rb.centre().x) / 2.0, y - 5.0),
                        l,
                        Anchor::Middle,
                    );
                }
            }
            _ => {}
        }
    }

    f.fit(MARGIN);
    f
}

/// A pool's options, one short line each. They are stacked in the glyph
/// column rather than run together beside the name: `evict by (queued, size)`
/// is wider than a station, and a note that reaches the next glyph is worse
/// than no note.
fn pool_notes(p: &Program, i: usize, cached: bool) -> Vec<String> {
    let pool = &p.pools[i];
    let mut parts = vec![format!("cap {}", crate::ir::show_num(pool.cap))];
    if let Some(b) = pool.block {
        parts.push(format!("block {}", crate::ir::show_num(b)));
    }
    // An eviction order says nothing about a pool nothing is cached in.
    if cached {
        match &pool.evict {
            crate::ir::CEvict::Lru => parts.push("evict lru".into()),
            crate::ir::CEvict::By(keys) => parts.push(format!(
                "evict by ({})",
                keys.iter()
                    .map(|k| p.show_expr(k))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    if pool.preempt == crate::ir::Preempt::Lifo {
        parts.push("preempt lifo".into());
    }
    if pool.admit_via.is_some() {
        parts.push("admit via".into());
    }
    parts
}

/// A readable grid for a capacity: exact cells when there are few enough.
fn slot_grid(cap: f64) -> (usize, usize) {
    if !cap.is_finite() || cap <= 0.0 {
        return (4, 2);
    }
    let n = cap.min(32.0) as usize;
    if cap <= 32.0 {
        let rows = if n > 8 { 2 } else { 1 };
        (n.div_ceil(rows), rows)
    } else {
        (4, 2)
    }
}

/// The deployment figure of a program.
pub fn figure(p: &Program) -> Figure {
    let net = project(p);
    layout(p, &net)
}
