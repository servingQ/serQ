//! The deployment view: a serQ program as a queueing network.
//!
//! Pools and stages are declared, but the arrows between them are not: the
//! flow is a property of the session program. This module projects it onto the
//! stages — the statements disappear, the `Run`s become stations, and the
//! `Hold`s become the boundaries drawn around them — and lays the result out.
//!
//! See `tests/draw.rs`.

use std::collections::BTreeMap;

use crate::ir::{CArrival, CExpr, CRef, CStageKind, CStmt, CtxVar, Program, RunMode, UnOp};
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

/// Where the walk is: an end of the net, or the start of a loop body whose
/// first stations the walk is looking for. The walker's own; a `Net` has
/// only `End`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    End(End),
    Probe(usize),
}

/// A station: one stage the session reaches, or a decision before any
/// (`kind: Decision`, with no stage).
#[derive(Clone, Debug)]
pub struct Node {
    pub stage: Option<usize>,
    /// `engine`, `rep[4]`.
    pub label: String,
    pub kind: StationKind,
    /// The discipline written inside the glyph: `FIFO`, `PS`, `step`.
    pub inner: String,
    /// A line under the glyph: the capacity, the budget, a `choose` key.
    pub note: Option<String>,
    /// Pools held around every visit to this stage, outermost first.
    pub pools: Vec<usize>,
    /// The work of the first visit, as written (`x0`).
    pub work: String,
    /// The modes the session runs it in.
    pub modes: Vec<RunMode>,
}

impl FlowNote {
    fn note_from(&mut self, pool: String) {
        match &self.from {
            Some(p) if *p != pool => self.mixed(),
            _ if !self.several => self.from = Some(pool),
            _ => {}
        }
    }
    fn note_to(&mut self, pool: String) {
        match &self.to {
            Some(p) if *p != pool => self.mixed(),
            _ if !self.several => self.to = Some(pool),
            _ => {}
        }
    }
    fn mixed(&mut self) {
        self.several = true;
        self.from = None;
        self.to = None;
    }
}

/// An instance: what the session addresses through one `choose`. A router
/// that picks `i` picks a pod, and every stage and pool the session then
/// indexes by `i` is that pod's (`P[i]`, `egress[i]`, `kvP[i]`).
#[derive(Clone, Debug)]
pub struct Instance {
    /// `choose i of NP`.
    pub label: String,
    /// Its stations, by node.
    pub nodes: Vec<usize>,
    /// Its pools.
    pub pools: Vec<usize>,
}

/// What a run over several stages moves and waits for, when it is a
/// transfer: the pool it releases and the one it loads, and the delays the
/// session passes just before it (folded in: `setup (x0)`).
#[derive(Clone, Debug, Default)]
pub struct FlowNote {
    pub from: Option<String>,
    pub to: Option<String>,
    pub latency: Vec<String>,
    /// Two runs over these stations moved different pools: the figure
    /// says neither.
    pub several: bool,
}

/// One `hold` the walk entered: the pool its request waits in, the pools it
/// occupies, and the stations reached while it held them.
#[derive(Clone, Debug)]
pub struct HoldSpan {
    /// The hold's first pool: a hold of several pools waits once, in this
    /// pool's queue (`interp.rs` `enqueue_hold`, `first_pool`).
    pub queue: usize,
    /// The stations reached while it held a pool, as (pool, node). A pool
    /// of no units occupies nothing and has none; a `release` ends one pool's
    /// before the others', and a leased pool's runs on past the hold.
    pub visits: Vec<(usize, usize)>,
}

impl HoldSpan {
    /// The stations reached while it held `pool`.
    pub fn nodes_of(&self, pool: usize) -> impl Iterator<Item = usize> + '_ {
        self.visits
            .iter()
            .filter(move |&&(q, _)| q == pool)
            .map(|&(_, n)| n)
    }
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
    /// The stations a run over several stages holds at once, by node.
    pub flows: Vec<Vec<usize>>,
    /// Per flow, what it moves and waits for.
    pub flow_notes: Vec<FlowNote>,
    /// The instances, in the order the session reaches them.
    pub instances: Vec<Instance>,
    /// Every `hold` the walk entered, by the order it entered them.
    pub holds: Vec<HoldSpan>,
}

impl Net {
    /// The node for a stage, if the session reaches it.
    pub fn node_of(&self, stage: usize) -> Option<usize> {
        self.nodes.iter().position(|n| n.stage == Some(stage))
    }
    /// Whether an edge exists between two ends, in that direction.
    pub fn has_edge(&self, from: End, to: End) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to)
    }
    /// Whether a run over several stages crosses from one instance to
    /// another: two of its stations in two different instances.
    /// It does when its stations, in the order named, are one instance's
    /// and then another's (`egress[i], ingress[j]`); any other mixture
    /// (`a[i], b[j], c[i]`) is bracketed where it stands.
    pub fn spans(&self, flow: &[usize]) -> bool {
        let mut runs: Vec<Option<usize>> = vec![];
        for &k in flow {
            let o = self.instance_of(k);
            if runs.last() != Some(&o) {
                runs.push(o);
            }
        }
        matches!(runs.as_slice(), [Some(a), Some(b)] if a != b)
    }

    /// The instance a station is in, if any.
    pub fn instance_of(&self, node: usize) -> Option<usize> {
        self.instances.iter().position(|g| g.nodes.contains(&node))
    }
    /// The pools drawn around a station: those held there, less a pool of
    /// another instance, and none at a station of a transfer between two
    /// instances. A transfer holds the sender's pool and the receiver's at
    /// both NICs; the figure says so on the link between the instances
    /// (`kvP[i] → kvD[j]`), not with boxes that cross.
    pub fn drawn_pools(&self, node: usize) -> Vec<usize> {
        let here = self.instance_of(node);
        if self.across(node) {
            return vec![];
        }
        self.nodes[node]
            .pools
            .iter()
            .copied()
            .filter(
                |&q| match self.instances.iter().position(|g| g.pools.contains(&q)) {
                    Some(owner) => here.is_none_or(|h| h == owner),
                    None => true,
                },
            )
            .collect()
    }

    /// Whether a station is one of a transfer between two instances.
    fn across(&self, node: usize) -> bool {
        self.flows
            .iter()
            .any(|g| g.contains(&node) && self.spans(g))
    }

    /// The pools drawn inside a station's own box: drawn at it, and every
    /// hold that takes the pool there reaches no other station. A request
    /// holds them only while it is there, so the figure draws them as that
    /// station's. A transfer between instances does not count: it holds the
    /// sender's pool and the receiver's, and its arrow says so
    /// (`P.kv[i] → D.kv[j]`), so the prefiller's KV is the prefiller's.
    pub fn resident_pools(&self, node: usize) -> Vec<usize> {
        if self.nodes[node].stage.is_none() {
            return vec![];
        }
        self.drawn_pools(node)
            .into_iter()
            .filter(|&q| {
                self.holds
                    .iter()
                    .filter(|h| h.nodes_of(q).any(|n| n == node))
                    .all(|h| h.nodes_of(q).all(|n| n == node || self.across(n)))
            })
            .collect()
    }

    /// Whether `pool`'s enclosure over the stations `first..=last` draws a
    /// queue: a hold that waits in `pool` reaches one of them.
    fn waits_in_enclosure(&self, pool: usize, first: usize, last: usize) -> bool {
        self.holds
            .iter()
            .any(|h| h.queue == pool && h.visits.iter().any(|(_, n)| (first..=last).contains(n)))
    }
}

struct Walker<'a> {
    p: &'a Program,
    net: Net,
    /// Ends the next station will be reached from, with the label of the path.
    frontier: Vec<(At, Option<String>)>,
    /// The ends of the legs forked and not yet joined: the station after
    /// the `join` is reached from them as well.
    forked: Vec<(At, Option<String>)>,
    /// Pools held right now, outermost first, each with the hold that took
    /// it (a `release` takes one off before its hold ends) and whether it
    /// encloses: a hold of no units only reserves, and occupies nothing.
    holds: Vec<(usize, usize, bool)>,
    next_hold: usize,
    /// `choose`s that have not yet found the station they select, as
    /// (attribute slot, label). A `choose` names an index, so it belongs to
    /// the station whose reference reads that attribute - `rep[j]`, not
    /// whatever station happens to come next.
    pending: Vec<(usize, String)>,
    /// The guard of the branch arm being walked, for the first edge it takes.
    arm: Option<String>,
    /// Every `choose` seen, as (attribute slot, label): the instances.
    chosen: Vec<(usize, String)>,
    /// The instance each chosen slot made, by slot.
    instance_of_slot: Vec<(usize, usize)>,
    /// The flow just run, whose `load` and `release` follow it.
    last_flow: Option<usize>,
    /// Per block being probed, the ends it first reaches.
    probes: Vec<Vec<End>>,
    /// The decision node of each loop body that has one, by body block: a
    /// loop walked twice (inside another) has one decision, not two.
    decisions: Vec<(usize, usize)>,
    /// Attributes this path has set to a constant, by slot. A guard that
    /// reads only these is decided, and only its arm is drawn.
    known: Vec<(usize, f64)>,
}

impl Walker<'_> {
    /// The instance an index addresses: exactly a chosen attribute.
    fn instance(&mut self, index: Option<&CExpr>) -> Option<usize> {
        let Some(CExpr::Attr(slot)) = index else {
            return None;
        };
        let (_, label) = self.chosen.iter().find(|(s, _)| s == slot)?.clone();
        if let Some(&(_, k)) = self.instance_of_slot.iter().find(|(s, _)| s == slot) {
            return Some(k);
        }
        self.net.instances.push(Instance {
            label,
            nodes: vec![],
            pools: vec![],
        });
        let k = self.net.instances.len() - 1;
        self.instance_of_slot.push((*slot, k));
        Some(k)
    }

    /// Put a station in the instance its reference addresses; a station
    /// reached through two is the first's.
    fn place(&mut self, node: usize, r: &CRef) {
        if let Some(k) = self.instance(r.index.as_deref())
            && self.net.instance_of(node).is_none()
        {
            self.net.instances[k].nodes.push(node);
        }
    }

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
            let from = match from {
                At::Probe(k) => {
                    if !self.probes[k].contains(&End::Node(node)) {
                        self.probes[k].push(End::Node(node));
                    }
                    continue;
                }
                At::End(e) => e,
            };
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
        self.frontier = vec![(At::End(End::Node(node)), None)];
    }

    /// Add an edge unless the same one is already there. Programs written as
    /// a chain of guards (`routing.sq`'s five policies) reach the same
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
        let idx = self.station(stage, label, note);
        self.attach(idx);
    }

    /// The node of a stage the session reaches, made on the first visit.
    fn station(&mut self, stage: usize, label: String, note: Option<String>) -> usize {
        let idx = match self.net.node_of(stage) {
            Some(i) => {
                // Seen before: it belongs only to the pools held every time.
                let held = &self.holds;
                self.net.nodes[i]
                    .pools
                    .retain(|p| held.iter().any(|&(q, _, e)| e && q == *p));
                i
            }
            None => {
                let (kind, inner, kind_note) = station_of(self.p, stage);
                self.net.nodes.push(Node {
                    stage: Some(stage),
                    label,
                    kind,
                    inner,
                    note: kind_note,
                    work: String::new(),
                    modes: vec![],
                    pools: self.holds.iter().fold(vec![], |mut v, &(q, _, e)| {
                        // nested holds of one pool are one enclosure
                        if e && !v.contains(&q) {
                            v.push(q);
                        }
                        v
                    }),
                });
                self.net.nodes.len() - 1
            }
        };
        for &(q, id, e) in &self.holds {
            let visits = &mut self.net.holds[id].visits;
            if e && !visits.contains(&(q, idx)) {
                visits.push((q, idx));
            }
        }
        if let Some(n) = note
            && self.net.nodes[idx].note.is_none()
        {
            self.net.nodes[idx].note = Some(n);
        }
        idx
    }

    fn walk(&mut self, block: usize) {
        let Some(stmts) = self.p.blocks.get(block) else {
            return;
        };
        for s in stmts.clone() {
            match s {
                CStmt::Run {
                    stage,
                    also,
                    mode,
                    work,
                    ..
                } => {
                    self.last_flow = None;
                    let label = self.p.show_stage_ref(&stage);
                    let note = stage.index.as_ref().and_then(|i| self.claim_choose(i));
                    self.visit_stage(stage.base, label, note);
                    let node = self.net.node_of(stage.base).unwrap();
                    self.place(node, &stage);
                    let n = &mut self.net.nodes[node];
                    if n.modes.is_empty() {
                        n.work = self.p.show_expr(work.cost_value());
                        // a delay is its duration: written inside it, under
                        // the shape of its density, when it is a
                        // distribution or a constant and fits
                        let fits = TextSize::Normal.width_of(&n.work) <= STATION_W - 12.0;
                        if n.kind == StationKind::Delay
                            && fits
                            && crate::view::figure::density(&n.work, Rect::new(0.0, 0.0, 1.0, 1.0))
                                .is_some()
                        {
                            n.inner = n.work.clone();
                        }
                    }
                    if !n.modes.contains(&mode) {
                        n.modes.push(mode);
                    }
                    if !also.is_empty() {
                        // the other stages are held at once, not passed in
                        // turn: no arrow between them, and the session
                        // leaves from the last
                        let mut group = vec![self.net.node_of(stage.base).unwrap()];
                        for r in &also {
                            let label = self.p.show_stage_ref(r);
                            let note = r.index.as_ref().and_then(|i| self.claim_choose(i));
                            let k = self.station(r.base, label, note);
                            self.place(k, r);
                            group.push(k);
                        }
                        let last = *group.last().unwrap();
                        self.frontier = vec![(At::End(End::Node(last)), None)];
                        let k = match self.net.flows.iter().position(|g| *g == group) {
                            Some(k) => k,
                            None => {
                                self.net.flows.push(group);
                                self.net.flow_notes.push(FlowNote::default());
                                self.net.flows.len() - 1
                            }
                        };
                        self.last_flow = Some(k);
                    }
                }
                CStmt::Hold {
                    pools, body, lease, ..
                } => {
                    let id = self.next_hold;
                    self.next_hold += 1;
                    self.net.holds.push(HoldSpan {
                        queue: pools[0].0.base,
                        visits: vec![],
                    });
                    for (r, units, _) in &pools {
                        let encloses = !self.value(units).is_some_and(|(v, _)| v == 0.0);
                        self.holds.push((r.base, id, encloses));
                        if let Some(k) = self.instance(r.index.as_deref())
                            && !self.net.instances.iter().any(|g| g.pools.contains(&r.base))
                        {
                            self.net.instances[k].pools.push(r.base);
                        }
                    }
                    self.walk(body);
                    // by hold, not by depth: a `release` inside may have
                    // taken an outer hold's pool off the stack already
                    let leased = lease.as_ref().filter(|(r, _)| {
                        self.holds.iter().any(|&(q, h, _)| q == r.base && h == id)
                    });
                    let taken: Vec<_> = self
                        .holds
                        .iter()
                        .filter(|&&(_, h, _)| h == id)
                        .copied()
                        .collect();
                    self.holds.retain(|&(_, h, _)| h != id);
                    // a leased pool stays held past the scope, until the
                    // `release` that takes it
                    if let Some((r, _)) = leased {
                        let encloses = taken.iter().any(|&(q, _, e)| q == r.base && e);
                        self.holds.push((r.base, id, encloses));
                    }
                }
                CStmt::Release(r) => {
                    if let Some(k) = self.last_flow {
                        let pool = self.p.show_pool_ref(&r);
                        self.net.flow_notes[k].note_from(pool);
                    }
                    // the pool leaves the enclosure here: the stations after
                    // this one are not inside it
                    if let Some(i) = self.holds.iter().rposition(|&(p, _, _)| p == r.base) {
                        self.holds.remove(i);
                    }
                }
                CStmt::Branch(c, t, e) => {
                    // A guard this path has decided (`set transferred = 0;`
                    // before `branch (!transferred)`) takes one arm. The
                    // other may still run, when a hold is executed again
                    // after a preemption: one with a station of its own is
                    // drawn, as any arm. One with none only skips stations,
                    // and its edge would join the station before the guard
                    // to the one after, which the session never travels:
                    // the hold executed again starts where it was.
                    if let Some(v) = self.decided(&c) {
                        let (taken, other) = if v != 0.0 { (t, e) } else { (e, t) };
                        if !reaches_a_station(self.p, other) {
                            self.walk(taken);
                            continue;
                        }
                    }
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
                    let known = self.known.clone();
                    self.frontier = saved.clone();
                    self.arm = Some(self.p.show_guard(&c));
                    self.walk(t);
                    let then_out = std::mem::take(&mut self.frontier);
                    let then_held = std::mem::replace(&mut self.holds, held);
                    let then_known = std::mem::replace(&mut self.known, known);
                    self.frontier = saved;
                    self.arm = Some("else".into());
                    self.walk(e);
                    self.holds.retain(|h| then_held.contains(h));
                    // after the branch, what both arms leave the same
                    self.known.retain(|k| then_known.contains(k));
                    let mut out = then_out;
                    out.append(&mut self.frontier);
                    dedupe(&mut out);
                    self.frontier = out;
                    self.arm = outer;
                }
                CStmt::Fork(body) => {
                    // The leg starts where the session is and runs beside
                    // it: the session goes on from the same ends. A leg
                    // holds nothing of the session's; what it leases stays
                    // held after it, until the session's `release`.
                    let saved = self.frontier.clone();
                    let held = std::mem::take(&mut self.holds);
                    self.walk(body);
                    let leg = std::mem::replace(&mut self.frontier, saved);
                    let leased = std::mem::replace(&mut self.holds, held);
                    self.holds.extend(leased);
                    self.forked.extend(leg);
                }
                CStmt::Join => {
                    let mut ends = std::mem::take(&mut self.forked);
                    self.frontier.append(&mut ends);
                    dedupe(&mut self.frontier);
                }
                CStmt::While(_, body) => {
                    self.known.clear();
                    let before = self.frontier.clone();
                    let held = self.holds.clone();
                    self.enter(body, true);
                    self.frontier.extend(before);
                    dedupe(&mut self.frontier);
                    // The body may be skipped; its assignments and leases
                    // are not unconditional facts after the loop.
                    self.known.clear();
                    self.holds.retain(|h| held.contains(h));
                }
                CStmt::Loop(body) => {
                    // A full-session view (IR input or no unique request
                    // body) includes client loops. Draw their way back so
                    // no station is a dead end. Values set before the loop
                    // are known only on its first pass.
                    self.known.clear();
                    self.enter(body, true);
                    self.frontier.clear();
                    // A loop is left only by `end`, which already recorded it.
                }
                CStmt::End => {
                    let arm = self.arm.take();
                    let frontier = std::mem::take(&mut self.frontier);
                    for (from, label) in frontier {
                        let from = match from {
                            At::Probe(k) => {
                                if !self.probes[k].contains(&End::Exit) {
                                    self.probes[k].push(End::Exit);
                                }
                                continue;
                            }
                            At::End(e) => e,
                        };
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
                    self.known.retain(|&(s, _)| s != var);
                    self.pending.push((var, label.clone()));
                    if !self.chosen.iter().any(|(v, _)| *v == var) {
                        self.chosen.push((var, label));
                    }
                }
                CStmt::Load(r, _) => {
                    if let Some(k) = self.last_flow {
                        let pool = self.p.show_pool_ref(&r);
                        self.net.flow_notes[k].note_to(pool);
                    }
                }
                CStmt::Set(slot, e) => {
                    let v = self.value(&e).map(|(v, _)| v);
                    self.known.retain(|&(s, _)| s != slot);
                    if let Some(v) = v {
                        self.known.push((slot, v));
                    }
                }
                // a turn draws the attributes of the `turn` block again
                CStmt::Turn => self.known.clear(),
                CStmt::Observe(..) => {}
                CStmt::Grow(..) | CStmt::Drop(..) => {}
            }
        }
    }

    /// The value of an expression that reads only constants and attributes
    /// this path has set to constants, and whether it read an attribute. A
    /// guard that reads none is not decided: a guard on a constant
    /// (`mode == 0`) is the program's setting, which `--set` changes, and
    /// the figure draws every setting.
    fn value(&self, e: &CExpr) -> Option<(f64, bool)> {
        fn eval(e: &CExpr, known: &[(usize, f64)], read: &mut bool) -> Option<f64> {
            Some(match e {
                CExpr::Cost(_, x) => eval(x, known, read)?,
                CExpr::Num(x) => *x,
                CExpr::Attr(s) => {
                    *read = true;
                    known.iter().find(|(k, _)| k == s)?.1
                }
                CExpr::Unary(op, a) => {
                    let x = eval(a, known, read)?;
                    match op {
                        UnOp::Neg => -x,
                        UnOp::Not => f64::from(x == 0.0),
                    }
                }
                CExpr::Binary(op, a, b) => {
                    crate::frontend::link::binop(*op, eval(a, known, read)?, eval(b, known, read)?)
                }
                CExpr::Cond(c, a, b) => {
                    let (c, a, b) = (
                        eval(c, known, read)?,
                        eval(a, known, read)?,
                        eval(b, known, read)?,
                    );
                    if c != 0.0 { a } else { b }
                }
                CExpr::Ctx(_)
                | CExpr::Sample(..)
                | CExpr::Call(..)
                | CExpr::Agg(..)
                | CExpr::Reg(_) => {
                    return None;
                }
            })
        }
        let mut read = false;
        let v = eval(e, &self.known, &mut read)?;
        Some((v, read))
    }

    /// A guard this path decides: one `value` knows, that reads an attribute.
    fn decided(&self, e: &CExpr) -> Option<f64> {
        self.value(e).and_then(|(v, read)| read.then_some(v))
    }

    /// Walk a block that may decide before its first station: a loop's
    /// body, or the program the view draws. One that does (several first
    /// stations, or an `end` before any) is a router: one decision node the
    /// block starts from. A pass from a mark at the block's start finds out
    /// what it reaches first - a station, the one it was at included, or
    /// the exit - and is undone whole.
    ///
    /// A loop's body is entered `again`: its last stations lead back to the
    /// decision, or a second pass from where the first ended draws every
    /// way back to the station it starts at, from every arm, with the guard
    /// of the arm it takes; edges already there are not drawn twice.
    fn enter(&mut self, body: usize, again: bool) {
        let saved = (
            self.net.clone(),
            self.frontier.clone(),
            self.holds.clone(),
            self.pending.clone(),
            self.arm.clone(),
            self.chosen.clone(),
            self.instance_of_slot.clone(),
            self.last_flow,
            self.known.clone(),
            self.next_hold,
            self.decisions.clone(),
        );
        let k = self.probes.len();
        self.probes.push(vec![]);
        self.frontier = vec![(At::Probe(k), None)];
        self.walk(body);
        let entries = self.probes.pop().expect("pushed above");
        (
            self.net,
            self.frontier,
            self.holds,
            self.pending,
            self.arm,
            self.chosen,
            self.instance_of_slot,
            self.last_flow,
            self.known,
            self.next_hold,
            self.decisions,
        ) = saved;
        let known = self
            .decisions
            .iter()
            .find(|(b, _)| *b == body)
            .map(|&(_, d)| d);
        if let Some(d) = known {
            self.attach(d);
            self.walk(body);
            if again {
                self.attach(d);
            }
            return;
        }
        if entries.len() > 1 {
            // named by the `choose`s it makes before any station: the
            // router's name is the gateway's, which is the parser's and not
            // the IR's
            let mut slots = vec![];
            leading_chooses(self.p, body, &mut slots);
            let names: Vec<&str> = slots
                .iter()
                .map(|v| self.p.attrs.get(*v).map_or("?", String::as_str))
                .collect();
            let label = if names.is_empty() {
                String::new()
            } else {
                format!("choose {}", names.join(", "))
            };
            self.net.nodes.push(Node {
                stage: None,
                label,
                kind: StationKind::Decision,
                inner: String::new(),
                note: None,
                pools: vec![],
                work: String::new(),
                modes: vec![],
            });
            let d = self.net.nodes.len() - 1;
            self.decisions.push((body, d));
            self.attach(d);
            self.walk(body);
            if again {
                self.attach(d);
            }
            return;
        }
        self.walk(body);
        if again {
            // a later turn: what the first pass set may have changed
            self.known.clear();
            self.walk(body);
        }
    }
}

/// Does a block run at any station, at any depth?
fn reaches_a_station(p: &Program, block: usize) -> bool {
    p.blocks.get(block).is_some_and(|stmts| {
        stmts.iter().any(|s| match s {
            CStmt::Run { .. } => true,
            CStmt::Hold { body, .. }
            | CStmt::Loop(body)
            | CStmt::While(_, body)
            | CStmt::Fork(body) => reaches_a_station(p, *body),
            CStmt::Branch(_, a, b) => reaches_a_station(p, *a) || reaches_a_station(p, *b),
            _ => false,
        })
    })
}

/// The attributes a block `choose`s before any station, down every path,
/// in order; whether every path reaches a station or ends.
fn leading_chooses(p: &Program, block: usize, out: &mut Vec<usize>) -> bool {
    let Some(stmts) = p.blocks.get(block) else {
        return false;
    };
    for s in stmts {
        match s {
            CStmt::Choose { var, .. } => {
                if !out.contains(var) {
                    out.push(*var);
                }
            }
            CStmt::Run { .. } | CStmt::End => return true,
            CStmt::Branch(_, t, e) => {
                let a = leading_chooses(p, *t, out);
                let b = leading_chooses(p, *e, out);
                if a && b {
                    return true;
                }
            }
            // a guard that walks the body: the chooses in it are collected
            // whether or not it reaches a station
            CStmt::While(_, body) => {
                leading_chooses(p, *body, out);
            }
            CStmt::Hold { body, .. } | CStmt::Loop(body) | CStmt::Fork(body)
                if leading_chooses(p, *body, out) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Keep one entry per end: the arms of a guard that moved nobody all rejoin.
fn dedupe(frontier: &mut Vec<(At, Option<String>)>) {
    let mut seen: Vec<At> = vec![];
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
    e.any(&|x| matches!(x, CExpr::Attr(s) if *s == slot))
}

fn station_of(p: &Program, stage: usize) -> (StationKind, String, Option<String>) {
    match &p.stages[stage].kind {
        CStageKind::Fifo(c) => {
            let note = (*c != 1).then(|| format!("{c} servers"));
            (StationKind::Fifo, "FIFO".into(), note)
        }
        k if k.is_delay() => (StationKind::Delay, String::new(), Some("delay".into())),
        CStageKind::Ps(phi) => (StationKind::Ps, "PS".into(), Some(p.show_expr(phi))),
        CStageKind::Step(s) => (
            StationKind::Step,
            "engine".into(),
            Some(format!(
                "tokens cap {}",
                p.show_expr_with(&s.budget, list_names)
            )),
        ),
    }
}

/// A step stage is drawn in the words an engine is written in
/// (`docs/language.md`, the engine form), whichever form the program used:
/// the IR does not keep the form. Its budget is read as `tokens cap` reads
/// it, the residents' totals by their list (#411, #416).
fn list_names(v: CtxVar) -> &'static str {
    crate::frontend::parser::engine_name(v.name(), "tokens cap").unwrap_or(v.name())
}

/// The step stage whose `memory` pool `i` is.
fn memory_of(p: &Program, i: usize) -> Option<usize> {
    p.stages
        .iter()
        .position(|s| matches!(&s.kind, CStageKind::Step(spec) if spec.memory == Some(i)))
}

/// A pool's title, with what it is on as an engine names it: an engine's
/// memory is the pool on its device, which the IR does not name, and a pool
/// it admits otherwise is a pool on the engine. A memory the engine also
/// admits says so among its options (`pool_notes`).
fn pool_title(p: &Program, i: usize) -> String {
    let pool = &p.pools[i];
    match (memory_of(p, i), pool.admit_via) {
        (Some(s), _) => format!("pool {} on {}'s device", pool.name, p.stages[s].name),
        (None, Some(e)) => format!("pool {} on {}", pool.name, p.stages[e].name),
        (None, None) => format!("pool {}", pool.name),
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
        frontier: vec![(At::End(End::Arrival), None)],
        forked: vec![],
        holds: vec![],
        next_hold: 0,
        pending: vec![],
        arm: None,
        chosen: vec![],
        instance_of_slot: vec![],
        last_flow: None,
        probes: vec![],
        decisions: vec![],
        known: vec![],
    };
    w.enter(p.session, false);
    // Anything still on the frontier ran off the end of the session program.
    let frontier = std::mem::take(&mut w.frontier);
    for (from, label) in frontier {
        if let At::End(from) = from
            && from != End::Arrival
        {
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
    let mut net = w.net;
    fold_latencies(p, &mut net);
    decisions_first(&mut net);
    // one station and nothing else is a choice, not an instance to box
    net.instances
        .retain(|g| g.nodes.len() + g.pools.len() >= 2 && !g.nodes.is_empty());
    adjacent_flows(&mut net);
    contiguous_instances(&mut net);
    // grouping may have moved a decision after the stations it sends to
    decisions_first(&mut net);
    net
}

/// A link's latency (`serve ps(BwD) latency x0;`, which the parser writes
/// as a delay stage `ingress.latency` run before every transfer over
/// `ingress`) is the transfer's wait, not a station of the deployment. It
/// is written on the transfer (`ingress latency (x0)`) and its station
/// goes; the arrows into it go to the transfer's first station. Only the
/// name says so: any other delay before a transfer is a station.
fn fold_latencies(p: &Program, net: &mut Net) {
    loop {
        // a link's latency station, and the transfer over that link it
        // leads into
        let found = (0..net.nodes.len()).find_map(|k| {
            if net.nodes[k].kind != StationKind::Delay || net.flows.iter().any(|g| g.contains(&k)) {
                return None;
            }
            let link = p.stages[net.nodes[k].stage?]
                .name
                .strip_suffix(".latency")?;
            let outs: Vec<End> = net
                .edges
                .iter()
                .filter(|e| e.from == End::Node(k))
                .map(|e| e.to)
                .collect();
            let mut into: Vec<(usize, usize)> = vec![];
            for to in &outs {
                let End::Node(to) = *to else {
                    return None;
                };
                let flow = net.flows.iter().position(|g| {
                    g[0] == to
                        && g.iter().any(|&i| {
                            net.nodes[i]
                                .stage
                                .is_some_and(|st| p.stages[st].name == link)
                        })
                })?;
                into.push((to, flow));
            }
            // one transfer only: a latency waited before transfers down two
            // arms is one node of the projection, and folding it into both
            // would join each arm's way in to the other's transfer
            (into.len() == 1).then(|| (k, link.to_string(), into))
        });
        let Some((k, link, into)) = found else {
            return;
        };
        let wait = format!("{link} latency ({})", net.nodes[k].work);
        for &(_, flow) in &into {
            if !net.flow_notes[flow].latency.contains(&wait) {
                // folded from the transfer back: the one named first is
                // folded last and goes first
                net.flow_notes[flow].latency.insert(0, wait.clone());
            }
        }
        // the arrows into the delay go on to each transfer it leads into
        let mut edges = std::mem::take(&mut net.edges);
        edges.retain(|e| e.from != End::Node(k));
        let mut kept: Vec<Edge> = vec![];
        for e in edges {
            let targets: Vec<End> = if e.to == End::Node(k) {
                into.iter().map(|&(to, _)| End::Node(to)).collect()
            } else {
                vec![e.to]
            };
            for to in targets {
                match kept.iter_mut().find(|x| x.from == e.from && x.to == to) {
                    Some(x) => x.back &= e.back,
                    None => kept.push(Edge { to, ..e.clone() }),
                }
            }
        }
        net.edges = kept;
        remove_node(net, k);
    }
}

/// A decision stands before the stations it sends sessions to.
fn decisions_first(net: &mut Net) {
    for d in 0..net.nodes.len() {
        if net.nodes[d].kind != StationKind::Decision {
            continue;
        }
        let Some(first) = net
            .edges
            .iter()
            .filter(|e| e.from == End::Node(d))
            .filter_map(|e| match e.to {
                End::Node(k) if k != d => Some(k),
                _ => None,
            })
            .min()
        else {
            continue;
        };
        // before the instance its first station is in, not inside its box
        let first = match net.instance_of(first) {
            Some(g) => *net.instances[g]
                .nodes
                .iter()
                .min()
                .expect("an instance has a station"),
            None => first,
        };
        if first > d {
            continue;
        }
        let mut order: Vec<usize> = (0..net.nodes.len()).filter(|&k| k != d).collect();
        let at = order.iter().position(|&k| k == first).unwrap();
        order.insert(at, d);
        reorder(net, &order);
    }
}

/// Take a station out of the net, renumbering the ones after it.
fn remove_node(net: &mut Net, k: usize) {
    net.nodes.remove(k);
    let at = |i: usize| if i > k { i - 1 } else { i };
    for e in &mut net.edges {
        for end in [&mut e.from, &mut e.to] {
            if let End::Node(i) = *end {
                *end = End::Node(at(i));
            }
        }
    }
    for g in &mut net.flows {
        for i in g.iter_mut() {
            *i = at(*i);
        }
    }
    for g in &mut net.instances {
        g.nodes.retain(|&i| i != k);
        for i in g.nodes.iter_mut() {
            *i = at(*i);
        }
    }
    for h in &mut net.holds {
        h.visits.retain(|&(_, i)| i != k);
        for (_, i) in h.visits.iter_mut() {
            *i = at(*i);
        }
    }
}

/// Reorder the stations by a permutation: `order[k]` is the node that goes
/// to place `k`. Edges point the way the new order says.
fn reorder(net: &mut Net, order: &[usize]) {
    let n = net.nodes.len();
    if order.iter().enumerate().all(|(pos, &i)| pos == i) {
        return;
    }
    let mut pos = vec![0; n];
    for (k, &i) in order.iter().enumerate() {
        pos[i] = k;
    }
    let mut nodes: Vec<Option<Node>> = std::mem::take(&mut net.nodes)
        .into_iter()
        .map(Some)
        .collect();
    net.nodes = order.iter().map(|&i| nodes[i].take().unwrap()).collect();
    let at = |e: End| match e {
        End::Node(i) => End::Node(pos[i]),
        other => other,
    };
    for e in &mut net.edges {
        e.from = at(e.from);
        e.to = at(e.to);
        // the new order decides which way an arrow points, and so how it
        // is drawn: a return that now points right is drawn forward
        if let (End::Node(a), End::Node(b)) = (e.from, e.to) {
            e.back = a >= b;
        }
    }
    for g in &mut net.flows {
        for k in g.iter_mut() {
            *k = pos[*k];
        }
    }
    for g in &mut net.instances {
        for k in g.nodes.iter_mut() {
            *k = pos[*k];
        }
        g.nodes.sort_unstable();
    }
    for h in &mut net.holds {
        for (_, k) in h.visits.iter_mut() {
            *k = pos[*k];
        }
    }
}

/// Put the stations of each run over several stages side by side, in the
/// run's order, where the first of them stands in the row, so that the
/// bracket around them takes in no other station. An edge is then drawn by
/// the way it points in the new order. Two flows that share a station are
/// placed one after the other, so the second's bracket may still take in a
/// station of the first.
fn adjacent_flows(net: &mut Net) {
    if net.flows.is_empty() {
        return;
    }
    let n = net.nodes.len();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    for i in 0..n {
        if placed[i] {
            continue;
        }
        match net.flows.iter().find(|g| g.contains(&i)) {
            Some(g) => {
                for &k in g {
                    if !placed[k] {
                        placed[k] = true;
                        order.push(k);
                    }
                }
            }
            None => {
                placed[i] = true;
                order.push(i);
            }
        }
    }
    reorder(net, &order);
}

/// Put each instance's stations side by side, where its first stands, so
/// that its box takes in no other station.
fn contiguous_instances(net: &mut Net) {
    if net.instances.is_empty() {
        return;
    }
    let n = net.nodes.len();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    for i in 0..n {
        if placed[i] {
            continue;
        }
        match net.instance_of(i) {
            Some(g) => {
                for &k in &net.instances[g].nodes {
                    if !placed[k] {
                        placed[k] = true;
                        order.push(k);
                    }
                }
            }
            None => {
                placed[i] = true;
                order.push(i);
            }
        }
    }
    reorder(net, &order);
}

/// For every `hold` with a `cache` clause, the pools that clause can leave
/// units in - keyed by the hold's body block, which is unique to it.
///
/// `release_hold` (`interp.rs`) caches `min(cache, computed)` per pool, where
/// `computed` is the allocation unless the hold grew, and then it is the
/// position the growing run reached, which only a grown pool has. `grow`
/// advances the *innermost* hold holding that pool, so a `growing` run deep
/// inside nested holds can belong to an outer one, and a hold may be grown in
/// more than one pool. `examples/multi-turn/replica.sq` is the case that makes this
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
                CStmt::Loop(b) | CStmt::While(_, b) => walk(p, *b, stack, out),
                // a leg's holds are its own
                CStmt::Fork(b) => walk(p, *b, &mut vec![], out),
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
/// A framed station: the room above its glyph for its name, its margin,
/// and the height of one resident pool's row under it.
const FRAME_HEAD: f64 = 26.0;
const FRAME_PAD: f64 = 12.0;
const ROW_H: f64 = 32.0;
const QUEUE_W: f64 = 30.0;
/// A pool's drum.
const DRUM_W: f64 = 22.0;
const DRUM_H: f64 = 26.0;

/// An instance box's margin around what it holds, and the room for its title.
const INST_PAD: f64 = 12.0;
const INST_HEAD: f64 = 26.0;

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
    // a pool held at one station alone is drawn inside it, not boxed
    let drawn: Vec<Vec<usize>> = (0..net.nodes.len())
        .map(|i| {
            let resident = net.resident_pools(i);
            let mut v = net.drawn_pools(i);
            v.retain(|q| !resident.contains(q));
            v
        })
        .collect();
    let pools: Vec<usize> = {
        let mut v: Vec<usize> = drawn.iter().flatten().copied().collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    for pool in pools {
        let mut i = 0;
        while i < net.nodes.len() {
            if !drawn[i].contains(&pool) {
                i += 1;
                continue;
            }
            let start = i;
            while i < net.nodes.len() && drawn[i].contains(&pool) {
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
                let order = drawn[f].iter().position(|&x| x == p);
                let other = drawn[f].iter().position(|&x| x == o.pool);
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
    let boxed = !net.instances.is_empty();
    // per station, the pools drawn inside it
    let resident: Vec<Vec<usize>> = (0..net.nodes.len())
        .map(|i| net.resident_pools(i))
        .collect();
    let framed = |i: usize| !resident[i].is_empty();
    let row_y = MARGIN
        + 34.0
        + (max_depth as f64 + 1.0) * DEPTH_PAD
        + if boxed { INST_HEAD + INST_PAD } else { 0.0 }
        // a frame's name sits inside it, above the glyph: room for that
        // only where an enclosure or an instance box is drawn above it
        + if (0..net.nodes.len()).any(framed) && (!gs.is_empty() || boxed) {
            FRAME_HEAD
        } else {
            0.0
        };
    let row_notes = |q: usize| pool_notes(p, q, net.cached.contains(&q)).join(" · ");
    let row_w = |q: usize| {
        DRUM_W
            + 10.0
            + TextSize::Normal
                .width_of(&pool_title(p, q))
                .max(TextSize::Small.width_of(&row_notes(q)))
    };
    // under the glyph: its note, then a row per resident pool
    let rows_top = |i: usize| {
        STATION_H
            + if net.nodes[i].note.is_some() {
                24.0
            } else {
                10.0
            }
    };
    let frame_h = |i: usize| {
        let cached = resident[i]
            .iter()
            .filter(|q| net.cached.contains(q))
            .count();
        FRAME_HEAD + rows_top(i) + resident[i].len() as f64 * ROW_H + cached as f64 * 16.0 + 4.0
    };
    // a transfer between two instances is drawn in the gap between their
    // boxes, with what it moves above and what it waits for below
    let flow_texts = |g: usize| -> (Option<String>, Option<String>) {
        let n = &net.flow_notes[g];
        let moves = match (&n.from, &n.to) {
            (Some(a), Some(b)) => Some(format!("{a} → {b}")),
            _ => None,
        };
        let wait = (!n.latency.is_empty()).then(|| n.latency.join(" + "));
        (moves, wait)
    };
    let spanning = |g: usize| net.spans(&net.flows[g]);
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
    // a station's outline: its frame when it has one, its glyph otherwise
    let mut frames: Vec<Rect> = Vec::with_capacity(net.nodes.len());
    for i in 0..net.nodes.len() {
        let here = net.instance_of(i);
        let before = if i == 0 { None } else { net.instance_of(i - 1) };
        if here != before {
            // the boxes' margins, and for a transfer across the boundary the
            // width of what is written in the gap
            let mut room = INST_PAD
                * if i > 0 && here.is_some() && before.is_some() {
                    2.0
                } else {
                    1.0
                };
            if i > 0
                && let Some(g) = (0..net.flows.len())
                    .find(|&g| net.flows[g].contains(&(i - 1)) && net.flows[g].contains(&i))
            {
                let (moves, wait) = flow_texts(g);
                let w = [moves, wait]
                    .iter()
                    .flatten()
                    .map(|t| TextSize::Small.width_of(t) + 32.0)
                    .fold(0.0, f64::max);
                // the walls are `INST_PAD` outside the stations: the text
                // stands between them
                room += (w + 2.0 * INST_PAD - GAP).max(0.0);
            }
            x += room;
        }
        // Enclosures open outermost first, so that a nested pool's glyph
        // column sits beside its parent's rather than on top of it.
        let mut opening: Vec<usize> = (0..gs.len()).filter(|&k| gs[k].first == i).collect();
        opening.sort_by_key(|&k| gs[k].depth);
        for k in opening {
            let d = DEPTH_PAD * (max_depth - gs[k].depth) as f64;
            gs[k].left = x;
            gs[k].glyph_x = x + PAD + d;
            // the options stacked under the glyphs are as wide as their
            // longest line, and the column is as wide as they are
            let notes = pool_notes(p, gs[k].pool, cached_here(&gs[k]))
                .iter()
                .map(|l| TextSize::Small.width_of(l) + 8.0)
                .fold(GLYPH_W, f64::max);
            x += PAD + d + notes;
        }
        // A station's name above it and its note below it are centred on
        // it: one wider than the station gets the room, or it runs over
        // the pool options at its left and the station at its right.
        let n = &net.nodes[i];
        let slot = [
            STATION_W,
            TextSize::Normal.width_of(&n.label) + 8.0,
            n.note
                .as_deref()
                .map_or(0.0, |t| TextSize::Small.width_of(t) + 8.0),
        ]
        .into_iter()
        .fold(0.0, f64::max);
        if framed(i) {
            // the frame is as wide as its glyph or its widest row
            let rows = resident[i].iter().map(|&q| row_w(q)).fold(0.0, f64::max);
            let w = (slot + 2.0 * FRAME_PAD).max(rows + 2.0 * FRAME_PAD);
            rects.push(Rect::new(
                x + (w - STATION_W) / 2.0,
                row_y,
                STATION_W,
                STATION_H,
            ));
            frames.push(Rect::new(x, row_y - FRAME_HEAD, w, frame_h(i)));
            x += w;
        } else {
            rects.push(Rect::new(
                x + (slot - STATION_W) / 2.0,
                row_y,
                STATION_W,
                STATION_H,
            ));
            frames.push(rects[i]);
            x += slot;
        }
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

    // A run over several stages: its stations bracketed as one job.
    for (g, group) in net.flows.iter().enumerate() {
        let r = group
            .iter()
            .map(|&i| rects[i])
            .reduce(|a, b| a.union(&b))
            .expect("a flow holds a stage");
        let pad = 6.0;
        f.boxed(
            Rect::new(r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad),
            BoxStyle::Flow,
            4.0,
        );
        let (moves, wait) = flow_texts(g);
        if spanning(g) {
            // the job crosses from one instance to the other: an arrow
            // through the gap, in the order the stages are named
            let (a, b) = (rects[group[0]], rects[*group.last().unwrap()]);
            let y = a.centre().y;
            f.edge(vec![pt(a.right(), y), pt(b.x, y)], EdgeStyle::Flow);
            let mid = (a.right() + b.x) / 2.0;
            if let Some(t) = moves {
                f.note(pt(mid, y - 8.0), t, Anchor::Middle);
            }
            if let Some(t) = wait {
                f.note(pt(mid, y + 15.0), t, Anchor::Middle);
            }
        } else if let Some(t) = wait {
            f.note(pt(r.centre().x, r.bottom() + pad + 24.0), t, Anchor::Middle);
        }
    }

    // Enclosures first, so stations and glyphs paint over them.
    let mut enclosures: Vec<(usize, Rect)> = vec![];
    for (gi, g) in gs.iter().enumerate() {
        let pool = &p.pools[g.pool];
        let d = (max_depth - g.depth) as f64 * DEPTH_PAD;
        let inner = (g.first..=g.last)
            .map(|i| frames[i])
            .reduce(|a, b| a.union(&b))
            .unwrap();
        let extra = below[gi];
        let r = Rect::new(
            g.left,
            inner.y - PAD - d,
            g.right - g.left,
            inner.h + 2.0 * (PAD + d) + extra,
        );
        f.boxed(r, BoxStyle::Enclosure, 8.0);
        enclosures.push((g.first, r));

        // The queue glyph and the capacity, in the room reserved at the left.
        let gx = g.glyph_x;
        let gy = rects[g.first].centre().y;
        f.text(
            pt(gx, r.y - 5.0),
            pool_title(p, g.pool),
            Anchor::Start,
            TextSize::Normal,
        );
        // a hold of several pools waits in one queue, its first pool's
        if net.waits_in_enclosure(g.pool, g.first, g.last) {
            f.push(Item::Queue {
                rect: Rect::new(gx, gy - 11.0, QUEUE_W, 22.0),
                cells: 3,
            });
        }
        f.push(Item::Drum {
            rect: Rect::new(gx + 40.0, gy - DRUM_H / 2.0, DRUM_W, DRUM_H),
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
            // the option among the notes says which; the edge says where
            if let Some(n) = net.node_of(via) {
                f.push(Item::Edge {
                    pts: vec![
                        pt(gx + 15.0, gy - 13.0),
                        pt(rects[n].centre().x, rects[n].y - 6.0),
                    ],
                    style: EdgeStyle::Relation,
                    arrow: true,
                });
            }
        }
    }

    // A framed station: the pools held there alone, a row each under its
    // glyph.
    for i in (0..net.nodes.len()).filter(|&i| framed(i)) {
        let fr = frames[i];
        f.boxed(fr, BoxStyle::Frame, 8.0);
        let mut y = row_y + rows_top(i);
        for &q in &resident[i] {
            let gx = fr.x + FRAME_PAD;
            f.push(Item::Drum {
                rect: Rect::new(gx, y + 1.0, DRUM_W, DRUM_H),
            });
            let tx = gx + DRUM_W + 10.0;
            f.text(
                pt(tx, y + 11.0),
                pool_title(p, q),
                Anchor::Start,
                TextSize::Normal,
            );
            f.note(pt(tx, y + 23.0), row_notes(q), Anchor::Start);
            y += ROW_H;
            if net.cached.contains(&q) {
                let strip = Rect::new(gx, y - 4.0, fr.right() - FRAME_PAD - gx, 11.0);
                f.boxed(strip, BoxStyle::Cached, 2.0);
                f.note(
                    pt(strip.centre().x, strip.y + 8.5),
                    "prefix cache",
                    Anchor::Middle,
                );
                y += 16.0;
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

    // An instance's box: its stations with their names and notes, and the
    // enclosures that start at them, under everything else.
    let prefill_only = |k: usize| {
        net.instances[k]
            .nodes
            .iter()
            .any(|&i| net.nodes[i].kind == StationKind::Step)
            && net.instances[k].nodes.iter().all(|&i| {
                net.nodes[i].kind != StationKind::Step
                    || net.nodes[i].modes.iter().all(|m| *m == RunMode::Prefill)
            })
    };
    let any_prefill_only = (0..net.instances.len()).any(prefill_only);
    for (k, g) in net.instances.iter().enumerate() {
        let mut r = g
            .nodes
            .iter()
            .map(|&i| {
                let s = rects[i];
                if framed(i) {
                    frames[i]
                } else {
                    Rect::new(s.x, s.y - 20.0, s.w, s.h + 38.0)
                }
            })
            .reduce(|a, b| a.union(&b))
            .expect("an instance has a station");
        for (first, e) in &enclosures {
            if g.nodes.contains(first) {
                r = r.union(&e.pad(0.0));
            }
        }
        let r = Rect::new(
            r.x - INST_PAD,
            r.y - INST_PAD - 14.0,
            r.w + 2.0 * INST_PAD,
            r.h + 2.0 * INST_PAD + 14.0,
        );
        f.items.insert(
            0,
            Item::Box {
                rect: r,
                style: BoxStyle::Instance,
                round: 10.0,
            },
        );
        // named after its engine, and for prefill/decode by its part
        let engine = g
            .nodes
            .iter()
            .find(|&&i| net.nodes[i].kind == StationKind::Step)
            .unwrap_or(&g.nodes[0]);
        let role = if prefill_only(k) {
            "prefill "
        } else if any_prefill_only && net.nodes[*engine].modes.contains(&RunMode::Decode) {
            "decode "
        } else {
            ""
        };
        let title = format!("{role}instance {}", net.nodes[*engine].label);
        f.text(
            pt(r.x + 10.0, r.y - 7.0),
            title.clone(),
            Anchor::Start,
            TextSize::Normal,
        );
        f.note(
            pt(r.x + 22.0 + TextSize::Normal.width_of(&title), r.y - 7.0),
            g.label.clone(),
            Anchor::Start,
        );
    }

    // One counter for every edge that needs a lane below the station row:
    // feedback, an early exit and a second entry point must not share one.
    let mut lanes = 0.0;
    let deep = below.iter().copied().fold(0.0, f64::max);
    // and clear of every frame, and of every enclosure around one
    let floor = frames
        .iter()
        .chain(enclosures.iter().map(|(_, r)| r))
        .map(|r| r.bottom())
        .fold(0.0, f64::max);
    let base = (row_y + STATION_H + deep + BACK_DROP).max(floor + 14.0);
    let lane = |n: &mut f64| {
        *n += 1.0;
        base + *n * 22.0
    };
    let mut arrivals = 0;
    for e in &net.edges {
        let label = e.label.clone();
        match (e.from, e.to) {
            (End::Arrival, End::Node(i)) => {
                let r = rects[i];
                let y = r.centre().y;
                arrivals += 1;
                if arrivals == 1 && i == 0 {
                    // The arrow starts at the margin and the label rides above
                    // it, clear of the pool glyphs it passes.
                    f.edge(vec![pt(MARGIN, y), pt(r.x, y)], EdgeStyle::Flow);
                    f.note(pt(MARGIN, y - 10.0), arrival_text.clone(), Anchor::Start);
                } else {
                    // A session that opens with a branch has more than one entry
                    // station, and one may enter past the first station. An
                    // arrow along the row would run straight through the
                    // stations before it, so it takes a lane of its own.
                    if arrivals == 1 {
                        f.note(pt(MARGIN, y - 10.0), arrival_text.clone(), Anchor::Start);
                    }
                    let ly = lane(&mut lanes);
                    let x = r.x + r.w * 0.25;
                    f.push(Item::Edge {
                        pts: vec![
                            pt(MARGIN, y),
                            pt(MARGIN, ly),
                            pt(x, ly),
                            pt(x, frames[i].bottom()),
                        ],
                        style: EdgeStyle::Flow,
                        arrow: true,
                    });
                }
            }
            (End::Node(i), End::Exit) => {
                let r = rects[i];
                // a request leaving the deployment, or a session that ends
                // inside it: both go out, and only the second has a guard
                let text = label.map_or("out".into(), |l| format!("out ({l})"));
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
                            pt(r.x + r.w * 0.75, frames[i].bottom()),
                            pt(r.x + r.w * 0.75, y),
                            pt(row_right + 26.0, y),
                        ],
                        style: EdgeStyle::Flow,
                        arrow: true,
                    });
                    f.note(pt(row_right + 30.0, y + 3.0), text, Anchor::Start);
                }
            }
            (End::Node(a), End::Node(b)) if !e.back && b > a + 1 => {
                // Forward past the stations between: along the row it would run
                // through them, so it takes a lane below.
                let (ra, rb) = (rects[a], rects[b]);
                let y = lane(&mut lanes);
                // clear of the 0.25 a return leaves and enters by and the
                // 0.75 an early exit leaves by
                let (ax, bx) = (ra.x + ra.w * 0.625, rb.x + rb.w * 0.5);
                f.push(Item::Edge {
                    pts: vec![
                        pt(ax, frames[a].bottom()),
                        pt(ax, y),
                        pt(bx, y),
                        pt(bx, frames[b].bottom()),
                    ],
                    style: EdgeStyle::Flow,
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
            (End::Node(a), End::Node(b)) if !e.back => {
                let (ra, rb) = (rects[a], rects[b]);
                f.edge(
                    vec![pt(ra.right(), ra.centre().y), pt(rb.x, rb.centre().y)],
                    EdgeStyle::Flow,
                );
                if let Some(l) = label {
                    // by the station it leaves: the pool glyphs before the
                    // next one stand in the middle
                    f.note(
                        pt(frames[a].right() + 6.0, ra.centre().y - 8.0),
                        l,
                        Anchor::Start,
                    );
                }
            }
            (End::Node(a), End::Node(b)) => {
                let (ra, rb) = (rects[a], rects[b]);
                let y = lane(&mut lanes);
                let (ax, bx) = (ra.x + ra.w * 0.25, rb.x + rb.w * 0.25);
                f.push(Item::Edge {
                    pts: vec![
                        pt(ax, frames[a].bottom()),
                        pt(ax, y),
                        pt(bx, y),
                        pt(bx, frames[b].bottom()),
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
/// column rather than run together beside the name: `evict by (waiting, size)`
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
    match &pool.preempt {
        crate::ir::Preempt::None => {}
        lifo if lifo.is_lifo() => parts.push("preempt lifo".into()),
        crate::ir::Preempt::By { keys, tail } => parts.push(format!(
            "preempt by ({}){}",
            keys.iter()
                .map(|k| p.show_expr(k))
                .collect::<Vec<_>>()
                .join(", "),
            if *tail { " requeue tail" } else { "" }
        )),
    }
    // the title says `on S` only for a pool that is no memory
    if let (Some(_), Some(s)) = (memory_of(p, i), pool.admit_via) {
        parts.push(format!("admitted by {}", p.stages[s].name));
    }
    if pool.reserve_held {
        parts.push("reserve held".into());
    }
    parts
}

/// The deployment figure of a program.
pub fn figure(p: &Program) -> Figure {
    let net = project(p);
    layout(p, &net)
}
