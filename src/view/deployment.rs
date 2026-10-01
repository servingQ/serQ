//! The deployment view: a serQ program as a queueing network.
//!
//! Pools and stages are declared, but the arrows between them are not: the
//! flow is a property of the session program. This module projects it onto the
//! stages — the statements disappear, the `Run`s become stations, and the
//! `Hold`s become the boundaries drawn around them — and lays the result out.
//!
//! See `tests/draw.rs`.

use std::collections::BTreeMap;

use crate::ir::{CArg, CArrival, CExpr, CRef, CStageKind, CStmt, Program, RunMode};
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
        let across = self
            .flows
            .iter()
            .any(|g| g.contains(&node) && self.spans(g));
        if across {
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
}

struct Walker<'a> {
    p: &'a Program,
    net: Net,
    /// Ends the next station will be reached from, with the label of the path.
    frontier: Vec<(At, Option<String>)>,
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
    /// Per loop being probed, the ends its body first reaches.
    probes: Vec<Vec<End>>,
    /// The decision node of each loop body that has one, by body block: a
    /// loop walked twice (inside another) has one decision, not two.
    decisions: Vec<(usize, usize)>,
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
                        n.work = self.p.show_expr(&work);
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
                    for (r, units, _) in &pools {
                        let encloses = !matches!(units, CExpr::Num(x) if *x == 0.0);
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
                    // A body that decides before its first station (several
                    // first stations, or an `end` before any) is a router at
                    // the top of every turn: one decision node the body
                    // starts from and every pass returns to. A pass from a
                    // mark at the body's start finds out what it reaches
                    // first - a station, the one it was at included, or the
                    // exit - and is undone whole.
                    let saved = (
                        self.net.clone(),
                        self.frontier.clone(),
                        self.holds.clone(),
                        self.pending.clone(),
                        self.arm.clone(),
                        self.chosen.clone(),
                        self.instance_of_slot.clone(),
                        self.last_flow,
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
                        self.attach(d);
                    } else if entries.len() > 1 {
                        // named by the `choose`s it makes before any station:
                        // the router's name is the gateway's, which is the
                        // parser's and not the IR's
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
                        self.attach(d);
                    } else {
                        // The second pass starts where the first ended, so
                        // every way back into the body is drawn, from every
                        // arm, with the guard of the arm it takes; edges
                        // already there are not drawn twice.
                        self.walk(body);
                        self.walk(body);
                    }
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
                CStmt::Turn | CStmt::Set(..) | CStmt::Observe(..) => {}
                CStmt::Grow(..) | CStmt::Drop(..) => {}
            }
        }
    }
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
            CStmt::Hold { body, .. } | CStmt::Loop(body) if leading_chooses(p, *body, out) => {
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
        frontier: vec![(At::End(End::Arrival), None)],
        holds: vec![],
        next_hold: 0,
        pending: vec![],
        arm: None,
        chosen: vec![],
        instance_of_slot: vec![],
        last_flow: None,
        probes: vec![],
        decisions: vec![],
    };
    w.walk(p.session);
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
    let drawn: Vec<Vec<usize>> = (0..net.nodes.len()).map(|i| net.drawn_pools(i)).collect();
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
    let row_y = MARGIN
        + 34.0
        + (max_depth as f64 + 1.0) * DEPTH_PAD
        + if boxed { INST_HEAD + INST_PAD } else { 0.0 };
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
        rects.push(Rect::new(
            x + (slot - STATION_W) / 2.0,
            row_y,
            STATION_W,
            STATION_H,
        ));
        x += slot;
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
        let inner = rects[g.first].union(&rects[g.last]);
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
                Rect::new(s.x, s.y - 20.0, s.w, s.h + 38.0)
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
                        pt(ax, ra.bottom()),
                        pt(ax, y),
                        pt(bx, y),
                        pt(bx, rb.bottom()),
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
                    f.note(pt(ra.right() + 6.0, ra.centre().y - 8.0), l, Anchor::Start);
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
