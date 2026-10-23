//! The deployment view: a serQ program as a queueing network.
//!
//! Pools and stages are declared, but the arrows between them are not: the
//! flow is a property of the session program. This module projects it onto the
//! stages — the statements disappear, the `Run`s become stations, and the
//! `Hold`s become the boundaries drawn around them — and lays the result out.
//!
//! See `tests/draw.rs`.

use std::collections::BTreeMap;

use crate::engine::dist::Dist;
use crate::frontend::queue::GATEWAY_VERB;
use crate::ir::{
    CArrival, CExpr, CRef, CStageKind, CStmt, CtxVar, DistKind, Program, RunMode, Side, UnOp,
};
use crate::view::figure::{
    Anchor, BoxStyle, CLIENT, CopyMark, EdgeRole, EdgeStyle, Figure, Item, Outline, Point, Rect,
    StationKind, TextSize, pt,
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
    /// A delay's outline, read from its duration (`outline_of`).
    pub outline: Option<Outline>,
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
/// that picks `i` picks a pod, decided in this order:
/// - its stages and pools are those the session indexes by exactly `i` and
///   by nothing else (`P[i]`, `egress[i]`, `kvP[i]`; not `E` if `E[j]` or
///   `E[0]` is run too) (`Walker::settle_instances`);
/// - a pod of one station and nothing else is a choice, not a box
///   (`keep_instances`);
/// - a station whose flow the boxes would pull apart leaves its box
///   (`contiguous_instances`);
/// - its pools are drawn only inside its box (`Net::drawn_pools`).
#[derive(Clone, Debug)]
pub struct Instance {
    /// `choose i of NP`.
    pub label: String,
    /// Its stations, by node.
    pub nodes: Vec<usize>,
    /// Its pools.
    pub pools: Vec<usize>,
    /// The attribute its `choose` chose: two instances of one variable
    /// that share a station are one (`own`).
    pub slot: usize,
    /// Its stations are all a family's members (`P[i]`, `P.nic[i]`): the
    /// figure draws it once per member (`Stack`), so a flow out of it is
    /// one per member, whatever is at its other end.
    pub stacked: bool,
}

/// What a run over several stages moves and waits for, when it is a
/// transfer: the pool it releases and the one it loads, the run's
/// `latency`, written on it (`latency (x0)`), and who posts it.
#[derive(Clone, Debug, Default)]
pub struct FlowNote {
    pub from: Option<String>,
    pub to: Option<String>,
    pub latency: Vec<String>,
    /// The nodes the runs over these stations name as their poster
    /// (`CStmt::Run::poster`): a copy is drawn as a pull or a push when
    /// they name one.
    pub posters: Vec<usize>,
    /// Two runs over these stations moved different pools: the figure
    /// says neither.
    pub several: bool,
}

/// One `hold` the walk entered: the pool its request waits in, the pools it
/// occupies, and the stations reached while it held them.
#[derive(Clone, Debug)]
pub struct HoldSpan {
    /// The hold's first pool: a hold of several pools waits once, in this
    /// pool's queue (`interp.rs` `enqueue_hold`, `first_pool`). None for a
    /// lease a `grow` takes the pool by: a growth waits among the pool's
    /// growers, not in its queue.
    pub queue: Option<usize>,
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
    /// A reply: from a station the route ended at back to its gateway,
    /// which the request leaves by.
    pub reply: bool,
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
    /// Whether an edge a request takes exists between two ends, in that
    /// direction; a reply is not one (`Edge::reply`).
    pub fn has_edge(&self, from: End, to: End) -> bool {
        self.edges
            .iter()
            .any(|e| !e.reply && e.from == from && e.to == to)
    }
    /// What the SVG calls node `i`: its stage's name; for a gateway, the
    /// call the server makes (`gw.route`), which no stage is called (a
    /// gateway queue's engine is the stage `gw`); for a decision or a tick,
    /// which is no stage, its place among the nodes (`#0`), a name the
    /// arrows use within one figure: the place moves with the layout.
    pub fn name(&self, p: &Program, i: usize) -> String {
        let n = &self.nodes[i];
        match n.stage {
            Some(st) => p.stages[st].name.clone(),
            None if n.kind == StationKind::Gateway => {
                format!("{}.{GATEWAY_VERB}", n.label)
            }
            None => format!("#{i}"),
        }
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

    /// Whether a flow crosses an instance's box: between two instances
    /// (`spans`), or out of a stacked one to a station in any other place,
    /// none included, since a stacked box is drawn once per member (#667).
    /// A run from a station in no box into an unstacked one is no crossing
    /// (#197): that box is drawn once, and its pools with it.
    pub fn crosses(&self, flow: &[usize]) -> bool {
        self.spans(flow)
            || flow.iter().any(|&i| {
                self.instance_of(i)
                    .is_some_and(|k| self.instances[k].stacked)
                    && flow
                        .iter()
                        .any(|&j| self.instance_of(j) != self.instance_of(i))
            })
    }

    /// The instance a station is in, if any.
    pub fn instance_of(&self, node: usize) -> Option<usize> {
        self.instances.iter().position(|g| g.nodes.contains(&node))
    }
    /// The pools drawn around a station: those held there, less an
    /// instance's pool anywhere but in its box, and none at a station of a
    /// transfer between two instances. A transfer holds the sender's pool
    /// and the receiver's at both NICs; the figure says so on the link
    /// between the instances (`kvP[i] → kvD[j]`), not with boxes that cross.
    /// Nor a pool of no box at a station of a copy out of its stacked box
    /// (#667), whose enclosure would cross that box.
    pub fn drawn_pools(&self, node: usize) -> Vec<usize> {
        let here = self.instance_of(node);
        if self.across(node) {
            return vec![];
        }
        // a station of a copy out of its stacked box: a pool of no box (the
        // reader's, `D.kv`) would be an enclosure across the box (#667)
        let leaves = here.is_some_and(|k| self.instances[k].stacked)
            && self
                .flows
                .iter()
                .any(|g| g.contains(&node) && self.crosses(g));
        self.nodes[node]
            .pools
            .iter()
            .copied()
            .filter(
                |&q| match self.instances.iter().position(|g| g.pools.contains(&q)) {
                    // outside the box, its enclosure would cross the box
                    Some(owner) => here == Some(owner),
                    None => !leaves,
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
        self.holds.iter().any(|h| {
            h.queue == Some(pool) && h.visits.iter().any(|(_, n)| (first..=last).contains(n))
        })
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
    /// The session's ends at a `join` that opens a copy (a push: its legs
    /// end where the source posts the bytes): the copy's first station is
    /// reached from the legs, and the reader's, where the session's request
    /// waits for the bytes, from these.
    joined: Vec<(At, Option<String>)>,
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
    /// Pairs of instances of one variable found to share a station (`own`).
    merges: Vec<(usize, usize)>,
    /// Per station and per pool, the one instance every index addressed
    /// it through; `None` once two indices disagree or one is no chosen
    /// attribute, and then it is in no instance's box.
    node_owners: Vec<(usize, Option<usize>)>,
    pool_owners: Vec<(usize, Option<usize>)>,
    /// The flow just run, whose `load` and `release` follow it.
    last_flow: Option<usize>,
    /// Per block being probed, the ends it first reaches.
    probes: Vec<Vec<End>>,
    /// The decision node of each loop body that has one, by body block: a
    /// loop walked twice (inside another) has one decision, not two.
    decisions: Vec<(usize, usize)>,
    /// The node of each pool's queue tick a hold waits for, by pool.
    ticks: Vec<(usize, usize)>,
    /// The node of each gateway whose route the walk reached, by name.
    gateways: Vec<(String, usize)>,
    /// Attributes this path has set to a constant, by slot. A guard that
    /// reads only these is decided, and only its arm is drawn.
    known: Vec<(usize, f64)>,
    /// What this path last set each attribute to: a connector's block reads
    /// its request's source by one (`E.from`).
    set_to: Vec<(usize, CExpr)>,
    /// While a connector's save is walked, its request's call: the save's
    /// statements are its member's (`as_called`), its hold's body too.
    calling: Option<CRef>,
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
            slot: *slot,
            stacked: false,
        });
        let k = self.net.instances.len() - 1;
        self.instance_of_slot.push((*slot, k));
        Some(k)
    }

    /// Record the instance a station's reference addresses.
    fn place(&mut self, node: usize, r: &CRef) {
        if r.index.is_some() {
            let k = self.instance(r.index.as_deref());
            own(
                &mut self.node_owners,
                &self.net.instances,
                &mut self.merges,
                node,
                k,
            );
        }
    }

    /// Give each instance the stations and pools it alone addresses: a
    /// family two instances index (`E[i]`, then `E[j]`) is one node of the
    /// projection, and neither box holds it.
    fn settle_instances(&mut self) {
        // instances of one variable that share a station are one: each is
        // its representative's, the lowest of those merged with it (`own`)
        let mut rep: Vec<usize> = (0..self.net.instances.len()).collect();
        fn find(rep: &mut [usize], k: usize) -> usize {
            let mut r = k;
            while rep[r] != r {
                r = rep[r];
            }
            rep[k] = r;
            r
        }
        for &(a, b) in &self.merges {
            let (ra, rb) = (find(&mut rep, a), find(&mut rep, b));
            rep[ra.max(rb)] = ra.min(rb);
        }
        for owners in [&mut self.node_owners, &mut self.pool_owners] {
            for (_, o) in owners.iter_mut() {
                if let Some(k) = *o {
                    *o = Some(find(&mut rep, k));
                }
            }
        }
        let sole = |owners: &[(usize, Option<usize>)], k: usize| -> Vec<usize> {
            owners
                .iter()
                .filter(|(_, o)| *o == Some(k))
                .map(|&(x, _)| x)
                .collect()
        };
        for k in 0..self.net.instances.len() {
            let mut nodes = sole(&self.node_owners, k);
            nodes.sort_unstable();
            let p = self.p;
            let net = &self.net;
            let stacked = !nodes.is_empty()
                && nodes.iter().all(|&i| {
                    net.nodes[i]
                        .stage
                        .is_some_and(|s| p.stages[s].index.is_some())
                });
            self.net.instances[k].nodes = nodes;
            self.net.instances[k].pools = sole(&self.pool_owners, k);
            self.net.instances[k].stacked = stacked;
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
        let frontier = std::mem::take(&mut self.frontier);
        self.reach(frontier, node);
        self.frontier = vec![(At::End(End::Node(node)), None)];
    }

    /// The arrows from each of `frontier`'s ends to `node`.
    fn reach(&mut self, frontier: Vec<(At, Option<String>)>, node: usize) {
        let arm = self.arm.take();
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
                reply: false,
            });
        }
    }

    /// Add an edge unless the same one is already there. Programs written as
    /// a chain of guards (`routing.sq`'s five policies) reach the same
    /// station down many paths; the picture wants one arrow.
    fn push_edge(&mut self, e: Edge) {
        if let Some(existing) = self
            .net
            .edges
            .iter_mut()
            .find(|x| x.from == e.from && x.to == e.to && x.reply == e.reply)
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

    /// The node of the tick a hold on `pool`'s queue waits for (`queue …
    /// every (dt)`), made on the first visit: no stage's, a wait the pool
    /// encloses.
    fn tick_station(&mut self, pool: usize, dt: f64) -> usize {
        let idx = match self.ticks.iter().find(|&&(q, _)| q == pool) {
            Some(&(_, i)) => i,
            None => {
                self.net.nodes.push(Node {
                    stage: None,
                    label: "tick".into(),
                    kind: StationKind::Delay,
                    inner: String::new(),
                    note: Some(format!("every {dt}")),
                    pools: vec![],
                    work: String::new(),
                    modes: vec![],
                    outline: None,
                });
                let i = self.net.nodes.len() - 1;
                self.ticks.push((pool, i));
                i
            }
        };
        self.enclose(idx);
        idx
    }

    /// A node of no stage the walk reached inside the holds it is in (a
    /// tick, a gateway): the pools held now enclose it, as of its first
    /// visit, and each hold has reached it.
    fn enclose(&mut self, idx: usize) {
        let mut pools = vec![];
        for &(q, id, e) in &self.holds {
            if e && !pools.contains(&q) {
                pools.push(q);
            }
            let visits = &mut self.net.holds[id].visits;
            if e && !visits.contains(&(q, idx)) {
                visits.push((q, idx));
            }
        }
        if self.net.nodes[idx].pools.is_empty() {
            self.net.nodes[idx].pools = pools;
        }
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
                    outline: None,
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

    /// After a branch or a loop, the pools still held are those the other
    /// way through holds too, whichever hold took them: a lease grown in one
    /// arm and held anew in the other (ThunderAgent's router), or a lease
    /// each arm takes by a hold of its own (vLLM's push proxy, with and
    /// without a leg). A pool one way occupies and the other holds at no
    /// units encloses only where both do.
    fn keep_held(&mut self, other: &[(usize, usize, bool)]) {
        let mut kept = vec![];
        for &(q, id, e) in &self.holds {
            if let Some(&(_, _, f)) = other.iter().find(|&&(p, _, _)| p == q) {
                kept.push((q, id, e && f));
            }
        }
        self.holds = kept;
    }

    fn walk(&mut self, block: usize) {
        let Some(stmts) = self.p.blocks.get(block) else {
            return;
        };
        self.walk_in(stmts.clone(), Some(block));
    }

    /// The node of a gateway, made on the first route the walk reaches:
    /// no stage's, named after the gateway, the `choose`s its route makes
    /// before any station written in it.
    fn gateway(&mut self, r: &crate::ir::Route) -> usize {
        match self.gateways.iter().find(|(g, _)| *g == r.gateway) {
            Some(&(_, i)) => i,
            None => {
                let mut slots = vec![];
                leading_chooses_from(self.p, r.block, r.from, r.to, false, &mut slots);
                let names: Vec<&str> = slots
                    .iter()
                    .map(|v| self.p.attrs.get(*v).map_or("?", String::as_str))
                    .collect();
                // inside the glyph, clear of the arrows below it, when it
                // fits; `route` when the route chooses nothing first
                let chooses = (!names.is_empty()).then(|| format!("choose {}", names.join(", ")));
                let (inner, note) = match chooses {
                    Some(c) if fits_inside(&c) => (c, None),
                    Some(c) => (GATEWAY_VERB.into(), Some(c)),
                    None => (GATEWAY_VERB.into(), None),
                };
                self.net.nodes.push(Node {
                    stage: None,
                    label: r.gateway.clone(),
                    kind: StationKind::Gateway,
                    inner,
                    note,
                    pools: vec![],
                    work: String::new(),
                    modes: vec![],
                    outline: None,
                });
                let i = self.net.nodes.len() - 1;
                self.gateways.push((r.gateway.clone(), i));
                i
            }
        }
    }

    /// The routes that end and begin before statement `k` of the block
    /// being walked: the stations a route ended at reply to its gateway,
    /// which the request goes on from; a route starts at its gateway.
    fn routes_at(&mut self, routes: &[crate::ir::Route], k: usize) {
        for r in routes.iter().filter(|r| r.to == k) {
            let gw = self.gateway(r);
            for (from, _) in std::mem::take(&mut self.frontier) {
                if let At::End(from @ End::Node(n)) = from
                    && n != gw
                {
                    self.push_edge(Edge {
                        from,
                        to: End::Node(gw),
                        label: None,
                        back: true,
                        reply: true,
                    });
                }
            }
            self.frontier = vec![(At::End(End::Node(gw)), None)];
        }
        for r in routes.iter().filter(|r| r.from == k) {
            let gw = self.gateway(r);
            // inside the holds the route runs in, as a tick is
            self.enclose(gw);
            self.attach(gw);
        }
    }

    /// A connector's block as its request's call says it: the engine's own
    /// member the call's (`D.nic[j]`, not member 0's), and the source the
    /// member the entry set `E.from` to (`P.nic[i]`).
    fn at_call(
        &self,
        b: usize,
        call: &CRef,
        from: Option<usize>,
        source: Option<&CRef>,
    ) -> Vec<CStmt> {
        let from = from.and_then(|slot| {
            self.set_to
                .iter()
                .find(|(s, _)| *s == slot)
                .map(|(_, e)| e.clone())
        });
        let mut stmts = self.p.blocks[b].clone();
        // a read ends the source's send it reads, drawn as the `release` of
        // the source's pool it is
        stmts.extend(source.cloned().map(CStmt::Release));
        for s in &mut stmts {
            self.as_called(s, call, from.as_ref());
        }
        stmts
    }

    /// A statement of a connector's block as its request's call says it
    /// (`at_call`): its stages and pools of the engine's family the call's
    /// member, and a source's member the one `from` names.
    fn as_called(&self, s: &mut CStmt, call: &CRef, from: Option<&CExpr>) {
        let p = self.p;
        let fix = |r: &mut CRef, pools: bool| {
            let (name, index) = if pools {
                (&p.pools[r.base].name, p.pools[r.base].index)
            } else {
                (&p.stages[r.base].name, p.stages[r.base].index)
            };
            match r.index.as_deref() {
                // the source member the entry named
                Some(CExpr::Attr(_)) => {
                    if let Some(e) = from {
                        r.index = Some(Box::new(e.clone()));
                    }
                }
                // a member of the engine's family, the call's
                Some(CExpr::Num(_)) | None if call.index.is_some() => {
                    let k = match r.index.as_deref() {
                        Some(CExpr::Num(k)) => *k as usize,
                        _ => index.map_or(0, |k| k as usize),
                    };
                    if r.count == 1 && index.is_some() {
                        r.base -= k;
                        r.count = if pools {
                            p.pools.iter().filter(|q| &q.name == name).count()
                        } else {
                            p.stages.iter().filter(|q| &q.name == name).count()
                        };
                    }
                    if r.count == call.count {
                        r.index = call.index.clone();
                    }
                }
                _ => {}
            }
        };
        match s {
            CStmt::Run { stage, also, .. } => {
                fix(stage, false);
                for r in also {
                    fix(r, false);
                }
            }
            CStmt::Load(r, _) | CStmt::Release(r) | CStmt::Grow(r, _) => fix(r, true),
            // a save's room, in an L2's pool (#600)
            CStmt::Hold { pools, .. } => {
                for (r, _, _) in pools {
                    fix(r, true);
                }
            }
            _ => {}
        }
    }

    /// The legs forked so far end where a read of their send starts: at
    /// the source's side of the copy (`kvt.nixl_push`'s, the copy's run
    /// next), the session at the reader's.
    fn join_legs(&mut self) {
        let mut ends = std::mem::take(&mut self.forked);
        if !ends.is_empty() {
            let mut session = std::mem::replace(&mut self.frontier, ends);
            // a station a leg ends at reaches the copy by its NIC,
            // whichever way the session came by it
            session.retain(|(at, _)| !self.frontier.iter().any(|(leg, _)| leg == at));
            self.joined = session;
        } else {
            self.frontier.append(&mut ends);
            dedupe(&mut self.frontier);
        }
    }

    fn walk_stmts(&mut self, stmts: Vec<CStmt>) {
        self.walk_in(stmts, None);
    }

    /// Walk statements, those of `block` when they are a block's as the
    /// program has it, whose gateways' routes are found by it.
    fn walk_in(&mut self, stmts: Vec<CStmt>, block: Option<usize>) {
        let routes: Vec<crate::ir::Route> = block.map_or(vec![], |b| {
            self.p
                .routes
                .iter()
                .filter(|r| r.block == b)
                .cloned()
                .collect()
        });
        let n = stmts.len();
        for (k, mut s) in stmts.into_iter().enumerate() {
            self.routes_at(&routes, k);
            if let Some(call) = self.calling.clone() {
                self.as_called(&mut s, &call, None);
            }
            match s {
                CStmt::Run {
                    stage,
                    also,
                    mode,
                    work,
                    latency,
                    poster,
                    ..
                } => {
                    self.last_flow = None;
                    // a push: the legs reach the source's side of the copy,
                    // the session the reader's (below)
                    let session = std::mem::take(&mut self.joined);
                    let label = self.p.show_stage_ref(&stage);
                    let note = stage.index.as_ref().and_then(|i| self.claim_choose(i));
                    self.visit_stage(stage.base, label, note);
                    let node = self.net.node_of(stage.base).unwrap();
                    self.place(node, &stage);
                    let n = &mut self.net.nodes[node];
                    if n.modes.is_empty() {
                        n.work = self.p.show_expr(work.cost_value());
                        // a delay is its duration: the shape of its density
                        // when it is a distribution or a constant, and the
                        // duration written under it when it fits. The shape
                        // does not wait on the text: no `~erlang` or
                        // `~uniform` draw fits (`~erlang(1, 2)` is 13
                        // characters, a station holds 12)
                        let outline = outline_of(work.cost_value());
                        if n.kind == StationKind::Delay && outline.is_some() {
                            n.outline = outline;
                            if fits_inside(&n.work) {
                                n.inner = n.work.clone();
                            }
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
                        self.reach(session, last);
                        self.frontier = vec![(At::End(End::Node(last)), None)];
                        let k = match self.net.flows.iter().position(|g| *g == group) {
                            Some(k) => k,
                            None => {
                                self.net.flows.push(group);
                                self.net.flow_notes.push(FlowNote::default());
                                self.net.flows.len() - 1
                            }
                        };
                        // the run's wait is written on the transfer, not a
                        // station of its own
                        if let Some(l) = &latency {
                            let wait = format!("latency ({})", self.p.show_expr(l));
                            if !self.net.flow_notes[k].latency.contains(&wait) {
                                self.net.flow_notes[k].latency.push(wait);
                            }
                        }
                        // who posts the copy, when the run names its side
                        if let Some(node) = poster.and_then(|i| self.net.flows[k].get(i).copied())
                            && !self.net.flow_notes[k].posters.contains(&node)
                        {
                            self.net.flow_notes[k].posters.push(node);
                        }
                        self.last_flow = Some(k);
                    }
                }
                // an engine's request: the hold of what its engine takes,
                // around the engine's prefill and decode
                CStmt::Request { stage, output, .. } => {
                    let pools = match &self.p.stages[stage.base].kind {
                        CStageKind::Step(st) => st
                            .request
                            .as_ref()
                            .map(|r| r.pools.clone())
                            .unwrap_or_default(),
                        _ => vec![],
                    };
                    // its connector's load, then the engine, then its save,
                    // inside the request's hold
                    let connector = match &self.p.stages[stage.base].kind {
                        CStageKind::Step(st) => st.connector.clone(),
                        _ => None,
                    };
                    // a call from no source (`E.from` -1) loads nothing
                    let sourceless = connector.as_ref().and_then(|c| c.from).is_some_and(|slot| {
                        self.set_to
                            .iter()
                            .any(|(s, e)| *s == slot && matches!(e, CExpr::Num(v) if *v < 0.0))
                    });
                    let blocks = self.p.transfer_blocks.as_ref();
                    let load = connector
                        .as_ref()
                        .and_then(|c| {
                            let b = blocks.and_then(|t| t.load[stage.base])?;
                            let source = c.load.as_ref()?.source.clone();
                            Some((b, c.from, source))
                        })
                        .filter(|_| !sourceless);
                    // a pool it takes none of while it loads (its units 0 with
                    // `request.loaded` above 0) it takes once the load is done
                    // (`Interp::request_more`)
                    let after_load = |e: &CExpr| {
                        load.is_some()
                            && e.any(&|x| matches!(x, CExpr::Ctx(CtxVar::Loaded)))
                            && self.value(&with_loaded(e)).is_some_and(|(v, _)| v == 0.0)
                    };
                    let deferred: Vec<bool> = pools.iter().map(|(_, u, _)| after_load(u)).collect();
                    let id = self.next_hold;
                    let mut later = vec![];
                    if !pools.is_empty() {
                        self.next_hold += 1;
                        self.net.holds.push(HoldSpan {
                            queue: Some(pools[0].0.base),
                            visits: vec![],
                        });
                        for ((r, units, _), defer) in pools.iter().zip(deferred) {
                            if defer {
                                later.push(r.base);
                                continue;
                            }
                            let encloses = !self.value(units).is_some_and(|(v, _)| v == 0.0);
                            self.holds.push((r.base, id, encloses));
                        }
                    }
                    if let Some((b, from, source)) = load {
                        // a read waits for the send of the legs' source call
                        if source.is_some() && !self.forked.is_empty() {
                            self.join_legs();
                        }
                        let stmts = self.at_call(b, &stage, from, source.as_ref());
                        self.walk_stmts(stmts);
                    }
                    for q in later {
                        self.holds.push((q, id, true));
                    }
                    self.last_flow = None;
                    let label = self.p.show_stage_ref(&stage);
                    let note = stage.index.as_ref().and_then(|i| self.claim_choose(i));
                    self.visit_stage(stage.base, label, note);
                    let node = self.net.node_of(stage.base).unwrap();
                    self.place(node, &stage);
                    // a request for one token samples it at its prefill's
                    // end and decodes nothing (a prefiller's max_tokens = 1)
                    let decodes = !self.value(&output).is_some_and(|(v, _)| v <= 1.0);
                    let n = &mut self.net.nodes[node];
                    for mode in [RunMode::Prefill, RunMode::Decode] {
                        if (mode == RunMode::Prefill || decodes) && !n.modes.contains(&mode) {
                            n.modes.push(mode);
                        }
                    }
                    if let Some(b) = self
                        .p
                        .transfer_blocks
                        .as_ref()
                        .and_then(|t| t.save[stage.base])
                    {
                        // the save's hold and its body, as the call's
                        // member makes them: its own link (`E.nic[j]`,
                        // `kvt.l2_link`), not member 0's
                        let calling = self.calling.replace(stage.clone());
                        self.walk(b);
                        self.calling = calling;
                    }
                    if !pools.is_empty() {
                        self.holds.retain(|&(_, h, _)| h != id);
                    }
                    // a source's memory stays held past the request, while
                    // its send is registered, until the reader's load
                    let sent = match &self.p.stages[stage.base].kind {
                        CStageKind::Step(st) => crate::ir::sends(st).then_some(st.memory).flatten(),
                        _ => None,
                    };
                    if let Some(m) = sent {
                        self.holds.push((m, id, true));
                    }
                }
                CStmt::Hold {
                    pools, body, lease, ..
                } => {
                    let id = self.next_hold;
                    self.next_hold += 1;
                    self.net.holds.push(HoldSpan {
                        queue: Some(pools[0].0.base),
                        visits: vec![],
                    });
                    for (r, units, _) in &pools {
                        let encloses = !self.value(units).is_some_and(|(v, _)| v == 0.0);
                        self.holds.push((r.base, id, encloses));
                        if r.index.is_some() {
                            let k = self.instance(r.index.as_deref());
                            own(
                                &mut self.pool_owners,
                                &self.net.instances,
                                &mut self.merges,
                                r.base,
                                k,
                            );
                        }
                    }
                    // a hold that only waits for its queue's tick is the
                    // wait: a station of its own, inside the pool
                    let base = pools[0].0.base;
                    if let Some(dt) = self.p.pools[base].queue_every
                        && !reaches_a_station(self.p, body)
                    {
                        let idx = self.tick_station(base, dt);
                        self.attach(idx);
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
                    // before `if (!transferred)`) takes one arm. The
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
                    // and from the flow before it: a `load` that opens the
                    // else arm follows no transfer of the then arm
                    let flow = self.last_flow;
                    let known = self.known.clone();
                    self.frontier = saved.clone();
                    self.arm = Some(self.p.show_guard(&c));
                    self.walk(t);
                    let then_out = std::mem::take(&mut self.frontier);
                    let then_held = std::mem::replace(&mut self.holds, held);
                    let then_flow = std::mem::replace(&mut self.last_flow, flow);
                    let then_known = std::mem::replace(&mut self.known, known);
                    self.frontier = saved;
                    self.arm = Some("else".into());
                    self.walk(e);
                    self.keep_held(&then_held);
                    if self.last_flow != then_flow {
                        self.last_flow = None;
                    }
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
                    self.keep_held(&held);
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
                            reply: false,
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
                    // a choose is an instance of its own, even of a variable
                    // chosen before (`choose j` in each arm of an `if`, #685):
                    // what it indexes from here on is its, not the other's
                    self.chosen.retain(|(v, _)| *v != var);
                    self.chosen.push((var, label));
                    self.instance_of_slot.retain(|(s, _)| *s != var);
                }
                CStmt::Load(r, _) => {
                    if let Some(k) = self.last_flow {
                        let pool = self.p.show_pool_ref(&r);
                        self.net.flow_notes[k].note_to(pool);
                    }
                }
                CStmt::Set(slot, e) => {
                    self.set_to.retain(|(s, _)| *s != slot);
                    self.set_to.push((slot, e.clone()));
                    let v = self.value(&e).map(|(v, _)| v);
                    self.known.retain(|&(s, _)| s != slot);
                    if let Some(v) = v {
                        self.known.push((slot, v));
                    }
                }
                // a turn draws the attributes of the `turn` block again
                CStmt::Turn => self.known.clear(),
                CStmt::Observe(..) => {}
                // a `grow` or `shrink` of a pool held in no scope acts on
                // the session's lease of it: from here the request holds
                // that pool, whichever of the two an arm takes
                CStmt::Grow(r, _) | CStmt::Shrink(r, _)
                    if !self.holds.iter().any(|&(q, _, _)| q == r.base) =>
                {
                    let id = self.next_hold;
                    self.next_hold += 1;
                    self.net.holds.push(HoldSpan {
                        queue: None,
                        visits: vec![],
                    });
                    self.holds.push((r.base, id, true));
                    if r.index.is_some() {
                        let k = self.instance(r.index.as_deref());
                        own(
                            &mut self.pool_owners,
                            &self.net.instances,
                            &mut self.merges,
                            r.base,
                            k,
                        );
                    }
                }
                CStmt::Grow(..) | CStmt::Shrink(..) | CStmt::Drop(..) => {}
            }
        }
        // a route that ends with its block
        self.routes_at(&routes, n);
    }

    /// The value of an expression that reads only constants and attributes
    /// this path has set to constants, and whether it read an attribute. A
    /// guard that reads none is not decided: a guard on a constant
    /// (`mode == 0`) is the program's setting, which a supplied input changes, and
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
            self.merges.clone(),
            self.node_owners.clone(),
            self.pool_owners.clone(),
            self.last_flow,
            self.known.clone(),
            self.next_hold,
            self.decisions.clone(),
            self.ticks.clone(),
            self.gateways.clone(),
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
            self.merges,
            self.node_owners,
            self.pool_owners,
            self.last_flow,
            self.known,
            self.next_hold,
            self.decisions,
            self.ticks,
            self.gateways,
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
            // named by the `choose`s it makes before any station or
            // gateway, whose own are its
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
                outline: None,
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

/// Record that `x` is addressed through `owner`: a second, different one
/// leaves it in no box.
/// `x` is addressed through instance `owner`. Two instances of one
/// variable that address one station are one instance, merged in
/// `settle_instances`: a variable chosen in each arm of an `if` and read
/// after it, or a choose in a loop's body walked twice (#685). Two of two
/// variables (`E[i]`, then `E[j]`) leave it in neither box.
fn own(
    owners: &mut Vec<(usize, Option<usize>)>,
    instances: &[Instance],
    merges: &mut Vec<(usize, usize)>,
    x: usize,
    owner: Option<usize>,
) {
    match owners.iter_mut().find(|(y, _)| *y == x) {
        Some((_, Some(a)))
            if owner.is_some_and(|b| b != *a && instances[b].slot == instances[*a].slot) =>
        {
            merges.push((*a, owner.expect("checked above")));
        }
        Some((_, o)) if *o != owner => *o = None,
        Some(_) => {}
        None => owners.push((x, owner)),
    }
}

/// The attributes a block `choose`s before any station, down every path,
/// in order; whether every path reaches a station or ends. A gateway's
/// route is a station of its own, and what it chooses its own.
fn leading_chooses(p: &Program, block: usize, out: &mut Vec<usize>) -> bool {
    leading_chooses_from(p, block, 0, usize::MAX, true, out)
}

/// `leading_chooses` over statements `from..to` of a block; a route that
/// starts among them ends the search when `gateways` says so, and is
/// searched as any statements otherwise (the route's own, for its
/// gateway's label).
fn leading_chooses_from(
    p: &Program,
    block: usize,
    from: usize,
    to: usize,
    gateways: bool,
    out: &mut Vec<usize>,
) -> bool {
    let Some(stmts) = p.blocks.get(block) else {
        return false;
    };
    for (k, s) in stmts.iter().enumerate().take(to).skip(from) {
        if gateways && p.routes.iter().any(|r| r.block == block && r.from == k) {
            return true;
        }
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
                // a step stage is drawn in the words an engine is written in
                // (`docs/language.md`, the engine form), whichever form the
                // program used: its budget as `tokens cap` reads it (#411, #416)
                p.show_expr_with(&s.budget, crate::frontend::parser::tokens_cap_name)
            )),
        ),
    }
}

/// The outline of a duration's density: a draw, by its distribution, or a
/// constant. Anything else is drawn as infinitely many servers. A draw whose
/// parameters are numbers that make its law an exponential (`~erlang(1, m)`,
/// `~h2(m, 1)`) is drawn as one.
fn outline_of(duration: &CExpr) -> Option<Outline> {
    match duration {
        CExpr::Num(_) => Some(Outline::Spike),
        CExpr::Sample(d, a) if numbers(a).is_some_and(|a| Dist::of(*d, &a).is_exponential()) => {
            Some(Outline::Decaying)
        }
        CExpr::Sample(d, _) => match d {
            DistKind::Exp => Some(Outline::Decaying),
            DistKind::H2 => Some(Outline::TwoPhase),
            DistKind::Erlang => Some(Outline::Hump),
            DistKind::Uniform => Some(Outline::Flat),
            DistKind::Det => Some(Outline::Spike),
            // a draw of 0 or 1, not a duration
            DistKind::Bernoulli => None,
        },
        _ => None,
    }
}

/// The values of expressions that are all numbers.
fn numbers(es: &[CExpr]) -> Option<Vec<f64>> {
    es.iter()
        .map(|e| match e {
            CExpr::Num(x) => Some(*x),
            _ => None,
        })
        .collect()
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

/// A run of server statements in the session, at one `request`: the block
/// and its range, inside the holds the workload has around it, outermost
/// first.
struct Site {
    holds: Vec<CStmt>,
    block: usize,
    from: usize,
    to: usize,
}

/// The server's runs in a block, each inside the workload's holds around it.
/// A run ends at the first workload statement, where the interpreter ends
/// the client turn too (`ClientTurn`).
fn sites(p: &Program, block: usize, holds: &mut Vec<CStmt>, out: &mut Vec<Site>) {
    let (stmts, sides) = (&p.blocks[block], &p.sides[block]);
    let mut i = 0;
    while i < stmts.len() {
        if sides[i] == Side::Server {
            let from = i;
            while i < stmts.len() && sides[i] == Side::Server {
                i += 1;
            }
            out.push(Site {
                holds: holds.clone(),
                block,
                from,
                to: i,
            });
            continue;
        }
        match &stmts[i] {
            CStmt::Hold { body, .. } => {
                holds.push(stmts[i].clone());
                sites(p, *body, holds, out);
                holds.pop();
            }
            CStmt::Branch(_, a, b) => {
                sites(p, *a, holds, out);
                sites(p, *b, holds, out);
            }
            CStmt::Loop(b) | CStmt::While(_, b) | CStmt::Fork(b) => sites(p, *b, holds, out),
            _ => {}
        }
        i += 1;
    }
}

/// Two statement lists that run the same thing, block ids aside.
fn same_stmts(p: &Program, a: &[CStmt], b: &[CStmt]) -> bool {
    let same_block = |x: usize, y: usize| same_stmts(p, &p.blocks[x], &p.blocks[y]);
    // a statement with its blocks numbered 0, for comparing what is left
    let shape = |s: &CStmt| {
        let mut s = s.clone();
        match &mut s {
            CStmt::Hold { body, .. } => *body = 0,
            CStmt::Branch(_, t, e) => (*t, *e) = (0, 0),
            CStmt::Loop(b) | CStmt::While(_, b) | CStmt::Fork(b) => *b = 0,
            _ => {}
        }
        serde_json::to_value(&s).expect("a statement serialises")
    };
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            shape(x) == shape(y)
                && match (x, y) {
                    (CStmt::Hold { body: x, .. }, CStmt::Hold { body: y, .. })
                    | (CStmt::Loop(x), CStmt::Loop(y))
                    | (CStmt::While(_, x), CStmt::While(_, y))
                    | (CStmt::Fork(x), CStmt::Fork(y)) => same_block(*x, *y),
                    (CStmt::Branch(_, t, e), CStmt::Branch(_, u, f)) => {
                        same_block(*t, *u) && same_block(*e, *f)
                    }
                    _ => true,
                }
        })
}

/// Two sites run the same serving path inside holds of the same pools; the
/// units held do not change what the figure draws, so `hold live (1)` and
/// `hold live (2)` are one.
fn same_site(p: &Program, a: &Site, b: &Site) -> bool {
    let pools = |h: &CStmt| -> Vec<CRef> {
        match h {
            CStmt::Hold { pools, .. } => pools.iter().map(|(r, _, _)| r.clone()).collect(),
            _ => unreachable!("only holds are pushed"),
        }
    };
    a.holds.len() == b.holds.len()
        && a.holds
            .iter()
            .zip(&b.holds)
            .all(|(x, y)| pools(x) == pools(y))
        && same_stmts(
            p,
            &p.blocks[a.block][a.from..a.to],
            &p.blocks[b.block][b.from..b.to],
        )
}

/// What one request runs, in place of the session: the server's statements
/// (`Side::Server`) inside the holds the workload has around them. Whether
/// a session comes back, and when it ends, belongs to the workload. What the
/// server reads of the workload's attributes is set by nothing here, so a
/// server guard on one (`if (first)`) draws both arms. `None`, and the
/// session is drawn whole, when it runs no server statement or runs two
/// different things at its requests.
pub fn request(p: &Program) -> Option<Program> {
    let mut found = vec![];
    sites(p, p.session, &mut vec![], &mut found);
    let mut distinct: Vec<Site> = vec![];
    for s in found {
        if !distinct.iter().any(|d| same_site(p, d, &s)) {
            distinct.push(s);
        }
    }
    let [site] = &distinct[..] else {
        return None;
    };
    let mut q = p.clone();
    let mut block = q.blocks.len();
    q.blocks
        .push(p.blocks[site.block][site.from..site.to].to_vec());
    q.sides.push(vec![Side::Server; site.to - site.from]);
    // a route among the statements taken moves with them; one in a block
    // nested in them keeps its block
    for r in &mut q.routes {
        if r.block == site.block && site.from <= r.from && r.to <= site.to {
            (r.block, r.from, r.to) = (block, r.from - site.from, r.to - site.from);
        }
    }
    for h in site.holds.iter().rev() {
        let mut h = h.clone();
        if let CStmt::Hold { body, .. } = &mut h {
            *body = block;
        }
        block = q.blocks.len();
        q.blocks.push(vec![h]);
        q.sides.push(vec![Side::Workload]);
    }
    q.session = block;
    Some(q)
}

/// Project what one request runs (`request`) onto the stages, or the
/// session program when it has no one request.
pub fn project(p: &Program) -> Net {
    let owned = request(p);
    // the transfers' statements, which the walk follows (`Program::kernel`)
    let p = &owned.as_ref().unwrap_or(p).kernel();
    let mut w = Walker {
        p,
        net: Net {
            arrival: arrival_label(p),
            ..Net::default()
        },
        frontier: vec![(At::End(End::Arrival), None)],
        forked: vec![],
        joined: vec![],
        holds: vec![],
        next_hold: 0,
        pending: vec![],
        arm: None,
        chosen: vec![],
        instance_of_slot: vec![],
        merges: vec![],
        node_owners: vec![],
        pool_owners: vec![],
        last_flow: None,
        probes: vec![],
        decisions: vec![],
        ticks: vec![],
        gateways: vec![],
        known: vec![],
        set_to: vec![],
        calling: None,
    };
    w.enter(p.session, false);
    w.settle_instances();
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
                reply: false,
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
    decisions_first(&mut net);
    keep_instances(&mut net);
    adjacent_flows(&mut net);
    contiguous_instances(&mut net);
    // grouping may have moved a decision after the stations it sends to
    decisions_first(&mut net);
    net
}

/// A decision stands before the stations it sends sessions to.
fn decisions_first(net: &mut Net) {
    for d in 0..net.nodes.len() {
        // a gateway decides where its route goes, as a decision does
        if !matches!(
            net.nodes[d].kind,
            StationKind::Decision | StationKind::Gateway
        ) {
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

/// One station and nothing else is a choice, not an instance to box.
fn keep_instances(net: &mut Net) {
    net.instances
        .retain(|g| g.nodes.len() + g.pools.len() >= 2 && !g.nodes.is_empty());
}

/// Put each instance's stations side by side, where its first stands, so
/// that its box takes in no other station. Inside a box the stations a
/// transfer between two instances arrives at come first and those it leaves
/// from last, so that its stations stay side by side across the boundary.
/// A flow inside one instance, or in and out of one, that the boxes would
/// still pull apart takes its stations out of them, and is drawn where
/// `adjacent_flows` put it; a transfer between two instances stays an arrow
/// between their boxes, whichever way it points (a reply from `j` to `i`
/// after `i` sent to `j`).
fn contiguous_instances(net: &mut Net) {
    // The loop ends: boxing moves only boxed stations, so a flow it pulls
    // apart has a boxed station, and each pass takes at least one out.
    loop {
        if net.instances.is_empty() {
            return;
        }
        let order = instance_order(net);
        let mut at = vec![0; order.len()];
        for (k, &i) in order.iter().enumerate() {
            at[i] = k;
        }
        let together = |g: &[usize], pos: &dyn Fn(usize) -> usize| {
            g.windows(2).all(|w| pos(w[1]) == pos(w[0]) + 1)
        };
        let now = |i: usize| i;
        let boxed = |i: usize| at[i];
        let broken: Vec<usize> = net
            .flows
            .iter()
            .filter(|g| together(g, &now) && !together(g, &boxed) && !net.spans(g))
            .flatten()
            .copied()
            .collect();
        if broken.is_empty() {
            reorder(net, &order);
            return;
        }
        for g in &mut net.instances {
            g.nodes.retain(|k| !broken.contains(k));
        }
        keep_instances(net);
    }
}

/// The order `contiguous_instances` puts the stations in.
fn instance_order(net: &Net) -> Vec<usize> {
    // 0: a transfer into the instance arrives here; 2: one out of it leaves
    // from here; 1: neither, or both
    let side = |k: usize| {
        let (mut arrives, mut leaves) = (false, false);
        for g in net
            .flows
            .iter()
            .filter(|g| g.contains(&k) && net.crosses(g))
        {
            if net.instance_of(k) == net.instance_of(g[0]) {
                leaves = true;
            } else {
                arrives = true;
            }
        }
        match (arrives, leaves) {
            (true, false) => 0,
            (false, true) => 2,
            _ => 1,
        }
    };
    let n = net.nodes.len();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    for i in 0..n {
        if placed[i] {
            continue;
        }
        match net.instance_of(i) {
            Some(g) => {
                let mut nodes = net.instances[g].nodes.clone();
                nodes.sort_by_key(|&k| (side(k), k));
                for k in nodes {
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
    order
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
                // an engine's request caches in its memory pool, which grows
                // with it, or in every pool it takes when it holds no memory;
                // it has no body, so it is keyed past every block
                CStmt::Request { stage, .. } => {
                    if let CStageKind::Step(st) = &p.stages[stage.base].kind
                        && let Some(r) = &st.request
                        && r.cache.is_some()
                    {
                        let pools: Vec<usize> = r.pools.iter().map(|(q, _, _)| q.base).collect();
                        let targets = match st.memory {
                            Some(m) if pools.contains(&m) => vec![m],
                            _ => pools,
                        };
                        out.insert(p.blocks.len() + stage.base, targets);
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
    // a connector's save is a hold on its L2's pool that caches
    let p = &p.kernel();
    for b in (0..p.stages.len()).flat_map(|i| p.connector_blocks(i)) {
        walk(p, b, &mut vec![], &mut out);
    }
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
/// Characters of a pool's queue keys a label writes before it cuts them.
const QUEUE_KEYS_MAX: usize = 40;
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
/// The members of an instance drawn at most: past them the figure says how
/// many it left out.
const MEMBERS_MAX: usize = 8;
/// The room between two members' boxes, the lower one's title in it.
const MEMBER_GAP: f64 = 34.0;
/// The client's glyph, left of the server, and the server's margin around
/// what it holds.
const CLIENT_W: f64 = 64.0;
const SERVER_PAD: f64 = 16.0;

/// Whether a text is written inside a station, clear of its frame: 12
/// characters. A delay's duration and a gateway's `choose` read it.
fn fits_inside(t: &str) -> bool {
    TextSize::Normal.width_of(t) <= STATION_W - 12.0
}

/// An instance drawn once per member: `choose i in NP` picks one of `NP`
/// pods, and the figure draws the `NP`, its box and all in it again a
/// pitch further down for each, the arrows into it fanned out to every
/// member and those between two of them drawn from each member to each.
/// It is one when the instance's stations are the members of families
/// (`P[i]`, `P.nic[i]`), as many as the smallest family has.
struct Stack {
    /// The members drawn, and how many there are.
    count: usize,
    all: usize,
    /// The index the `choose` names (`i`), written `[i]` in the box.
    var: String,
    /// The names in the box that a member's own replaces (`P` by `P[1]`):
    /// its stations' and its pools'.
    names: Vec<String>,
}

impl Stack {
    fn of(p: &Program, net: &Net, g: &Instance) -> Option<Stack> {
        // stacked or not is `settle_instances`'s to say (`Instance.stacked`),
        // which `Net::crosses` reads too
        if !g.stacked {
            return None;
        }
        let family = |name: &str, index: Option<u32>, of: &mut dyn Iterator<Item = &str>| {
            index.map(|_| of.filter(|n| *n == name).count())
        };
        let mut all = usize::MAX;
        let mut names = vec![];
        for &i in &g.nodes {
            let st = &p.stages[net.nodes[i].stage?];
            all = all.min(family(
                &st.name,
                st.index,
                &mut p.stages.iter().map(|s| s.name.as_str()),
            )?);
            names.push(st.name.clone());
        }
        // its pools, and the members of a family its stations hold that a
        // transfer holds too (`D.kv`), drawn in its box all the same
        let held = g.nodes.iter().flat_map(|&i| &net.nodes[i].pools);
        for &q in g.pools.iter().chain(held) {
            let pool = &p.pools[q];
            if pool.index.is_some() && !names.contains(&pool.name) {
                names.push(pool.name.clone());
            }
        }
        let var = g
            .label
            .strip_prefix("choose ")?
            .split(" of ")
            .next()?
            .to_string();
        Some(Stack {
            count: all.min(MEMBERS_MAX),
            all,
            var,
            names,
        })
    }

    /// What member `m` calls `name`.
    fn name(&self, name: &str, m: usize) -> String {
        if self.names.iter().any(|n| n == name) {
            format!("{name}[{m}]")
        } else {
            name.to_string()
        }
    }

    /// `item`, drawn for member `m`, `dy` further down.
    fn member(&self, item: &Item, m: usize, dy: f64) -> Item {
        let down = |r: &Rect| Rect::new(r.x, r.y + dy, r.w, r.h);
        let name = |n: &String| self.name(n, m);
        let mut item = item.clone();
        match &mut item {
            Item::Box {
                rect, nodes, copy, ..
            } => {
                *rect = down(rect);
                *nodes = nodes.iter().map(name).collect();
                if let Some(c) = copy {
                    c.poster = name(&c.poster);
                }
            }
            Item::Station {
                rect, stage, node, ..
            } => {
                *rect = down(rect);
                *stage = stage.as_ref().map(name);
                *node = name(node);
            }
            Item::Queue { rect, pool, .. } | Item::Drum { rect, pool } => {
                *rect = down(rect);
                *pool = name(pool);
            }
            Item::Edge {
                pts, role, copy, ..
            } => {
                pts.iter_mut().for_each(|q| q.y += dy);
                match role {
                    Some(EdgeRole::Flow { from, to }) => {
                        *from = name(from);
                        *to = name(to);
                    }
                    Some(EdgeRole::Reply { from, to }) => {
                        *from = name(from);
                        *to = name(to);
                    }
                    Some(EdgeRole::Arrival { to }) => *to = name(to),
                    Some(EdgeRole::Exit { from }) => *from = name(from),
                    None => {}
                }
                if let Some(c) = copy {
                    c.poster = name(&c.poster);
                }
            }
            Item::Dot { at, .. } | Item::Mark { at, .. } => at.y += dy,
            Item::Text { at, text, .. } => {
                at.y += dy;
                *text = text.replace(&format!("[{}]", self.var), &format!("[{m}]"));
            }
        }
        item
    }
}

/// Whether an item is drawn wholly in `area`.
fn within(item: &Item, area: &Rect) -> bool {
    let at =
        |q: &Point| q.x >= area.x && q.x <= area.right() && q.y >= area.y && q.y <= area.bottom();
    match item {
        Item::Box { rect, .. }
        | Item::Station { rect, .. }
        | Item::Queue { rect, .. }
        | Item::Drum { rect, .. } => area.contains(rect),
        Item::Edge { pts, .. } => pts.iter().all(at),
        Item::Dot { at: q, .. } | Item::Mark { at: q, .. } | Item::Text { at: q, .. } => at(q),
    }
}

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
        + SERVER_PAD
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
    // a row's options on one line, its queue order on one of its own: a
    // `queue by` key can be as long as the station is wide
    let row_notes = |q: usize| -> Vec<String> {
        let (queue, rest): (Vec<String>, Vec<String>) = pool_notes(p, q, net.cached.contains(&q))
            .into_iter()
            .partition(|n| n.starts_with("queue by"));
        std::iter::once(rest.join(" · ")).chain(queue).collect()
    };
    let row_h = |q: usize| ROW_H + (row_notes(q).len() as f64 - 1.0) * NOTE_LINE;
    let row_w = |q: usize| {
        DRUM_W
            + 10.0
            + row_notes(q)
                .iter()
                .fold(TextSize::Normal.width_of(&pool_title(p, q)), |w, n| {
                    w.max(TextSize::Small.width_of(n))
                })
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
        FRAME_HEAD
            + rows_top(i)
            + resident[i].iter().map(|&q| row_h(q)).sum::<f64>()
            + cached as f64 * 16.0
            + 4.0
    };
    let spanning = |g: usize| net.crosses(&net.flows[g]);
    // who posts a copy: one poster for all the runs over these stations,
    // at the arrow's head (where the bytes arrive: a pull) or its tail (a
    // push); the arrow goes from the first stage the run names to the last
    let copy_mark = |g: usize| -> Option<CopyMark> {
        let group = &net.flows[g];
        match net.flow_notes[g].posters[..] {
            [node] => Some(CopyMark {
                pull: node != group[0],
                poster: net.name(p, node),
            }),
            _ => None,
        }
    };
    // a transfer between two instances is drawn in the gap between their
    // boxes, with what it moves above and what it waits for below
    let flow_texts = |g: usize| -> (Option<String>, Option<String>) {
        let n = &net.flow_notes[g];
        let mut moves = match (&n.from, &n.to) {
            (Some(a), Some(b)) => Some(format!("{a} → {b}")),
            _ => None,
        };
        let mut wait = (!n.latency.is_empty()).then(|| n.latency.join(" + "));
        // the word leads the first line written: what the copy moves, across
        // a gap between instances; else its wait, under its bracket
        if let Some(c) = copy_mark(g) {
            let word = c.word();
            match (&mut moves, &mut wait) {
                (Some(m), _) if spanning(g) => *m = format!("{word} · {m}"),
                (_, Some(w)) => *w = format!("{word} · {w}"),
                (_, None) => wait = Some(word.to_string()),
            }
        }
        (moves, wait)
    };
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

    // The client stands at the left, outside the server; the arrival arrow
    // from it has to be long enough for the label that sits over it, which
    // ends before the server's wall.
    let client = Rect::new(MARGIN, row_y, CLIENT_W, STATION_H);
    let arrival_text = format!("new sessions: {}", net.arrival);
    let lead = TextSize::Small.width_of(&arrival_text).max(70.0) + 14.0;
    let server_left = client.right() + lead;

    // Walk left to right, reserving room for the enclosures that open and
    // close around each station and for the pool glyphs inside them.
    let mut x = server_left + SERVER_PAD;
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

    // An instance whose stations are a family's members is drawn once per
    // member, the boxes stacked down from the row (`members`)
    let stacks: Vec<Option<Stack>> = net.instances.iter().map(|g| Stack::of(p, net, g)).collect();
    let stack_of = |i: usize| net.instance_of(i).and_then(|k| stacks[k].as_ref());
    // a copy across a stacked instance's box (`Net::crosses`): an arrow per
    // member, whatever is at its other end
    let stacked_copy =
        |g: usize| spanning(g) && net.flows[g].iter().any(|&i| stack_of(i).is_some());

    // A run over several stages: its stations bracketed as one job.
    for (g, group) in net.flows.iter().enumerate() {
        // a copy across a stacked instance's box is an arrow from each
        // member to each, drawn once the members are
        if stacked_copy(g) {
            continue;
        }
        let r = group
            .iter()
            .map(|&i| rects[i])
            .reduce(|a, b| a.union(&b))
            .expect("a flow holds a stage");
        let pad = 6.0;
        let copy = copy_mark(g);
        f.bracket(
            Rect::new(r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad),
            group.iter().map(|&i| net.name(p, i)).collect(),
            copy.clone(),
        );
        // the poster: a dot on the bracket over its station
        if let Some(&node) = net.flow_notes[g].posters.first()
            && copy.is_some()
        {
            f.push(Item::Dot {
                at: pt(rects[node].centre().x, r.y - pad),
                r: 3.0,
            });
        }
        let (moves, wait) = flow_texts(g);
        if spanning(g) {
            // the job crosses from one instance to the other: an arrow
            // through the gap, in the order the stages are named
            let (a, b) = (rects[group[0]], rects[*group.last().unwrap()]);
            let y = a.centre().y;
            f.copy_flow(
                vec![pt(a.right(), y), pt(b.x, y)],
                net.name(p, group[0]),
                net.name(p, *group.last().unwrap()),
                copy,
            );
            let mid = (a.right() + b.x) / 2.0;
            if let Some(t) = moves {
                f.note(pt(mid, y - 8.0), t, Anchor::Middle);
            }
            if let Some(t) = wait {
                f.note(pt(mid, y + 15.0), t, Anchor::Middle);
            }
        } else {
            // side by side under the bracket (`adjacent_flows`): an arrow
            // from each station to the next the run names, so that every
            // station of the job has an arrow in and one out, as any other
            // station has; a copy's mark goes on its one arrow
            for w in group.windows(2) {
                let (a, b) = (rects[w[0]], rects[w[1]]);
                let y = a.centre().y;
                let pts = if a.x < b.x {
                    vec![pt(a.right(), y), pt(b.x, y)]
                } else {
                    vec![pt(a.x, y), pt(b.right(), y)]
                };
                let mark = if group.len() == 2 { copy.clone() } else { None };
                f.copy_flow(pts, net.name(p, w[0]), net.name(p, w[1]), mark);
            }
            if let Some(t) = wait {
                f.note(pt(r.centre().x, r.bottom() + pad + 24.0), t, Anchor::Middle);
            }
        }
    }

    // Enclosures first, so stations and glyphs paint over them.
    let mut enclosures: Vec<(usize, usize, Rect)> = vec![];
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
        enclosures.push((g.first, g.last, r));

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
                pool: pool.name.clone(),
            });
        }
        f.push(Item::Drum {
            rect: Rect::new(gx + 40.0, gy - DRUM_H / 2.0, DRUM_W, DRUM_H),
            pool: pool.name.clone(),
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
                    role: None,
                    copy: None,
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
                pool: p.pools[q].name.clone(),
            });
            let tx = gx + DRUM_W + 10.0;
            f.text(
                pt(tx, y + 11.0),
                pool_title(p, q),
                Anchor::Start,
                TextSize::Normal,
            );
            for (k, line) in row_notes(q).into_iter().enumerate() {
                f.note(pt(tx, y + 23.0 + k as f64 * NOTE_LINE), line, Anchor::Start);
            }
            y += row_h(q);
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
            outline: n.outline,
            stage: n.stage.map(|st| p.stages[st].name.clone()),
            node: net.name(p, i),
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
    // enclosures that stand wholly in it, under everything else.
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
    let mut inst_boxes: Vec<Rect> = vec![];
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
        for (first, last, e) in &enclosures {
            if (*first..=*last).all(|i| g.nodes.contains(&i)) {
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
                // the stations it holds, by the names their arrows use: an
                // editor reads what is in the panel from the figure, not
                // from where things stand (#645 B1)
                nodes: g.nodes.iter().map(|&i| net.name(p, i)).collect(),
                copy: None,
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
        inst_boxes.push(r);
    }

    // The members: a stacked instance's box and all drawn in it again, a
    // pitch further down per member, named by the member's own (`P[1]`,
    // `P.kv[1]`); member 0 is the box drawn, its `choose` written once.
    let pitch = (0..stacks.len())
        .filter(|&k| stacks[k].is_some())
        .map(|k| inst_boxes[k].h + MEMBER_GAP)
        .fold(0.0, f64::max);
    let drawn = std::mem::take(&mut f.items);
    let (mut under, mut over) = (vec![], vec![]);
    let mut kept = drawn.clone();
    for (k, s) in stacks.iter().enumerate() {
        let Some(s) = s else { continue };
        let b = inst_boxes[k];
        let area = Rect::new(b.x, b.y - 20.0, b.w, b.h + 20.0);
        let choose = &net.instances[k].label;
        for (i, item) in drawn.iter().enumerate() {
            if !within(item, &area) {
                continue;
            }
            kept[i] = s.member(item, 0, 0.0);
            if matches!(item, Item::Text { text, .. } if text == choose) {
                continue;
            }
            for m in 1..s.count {
                match s.member(item, m, m as f64 * pitch) {
                    // under everything, as the first box is
                    i @ Item::Box {
                        style: BoxStyle::Instance,
                        ..
                    } => under.push(i),
                    i => over.push(i),
                }
            }
        }
        if s.all > s.count {
            let last = b.bottom() + (s.count - 1) as f64 * pitch;
            over.push(Item::Text {
                at: pt(b.x + 10.0, last + 14.0),
                text: format!("{} of {} members drawn", s.count, s.all),
                anchor: Anchor::Start,
                size: TextSize::Small,
                dim: true,
            });
        }
    }
    f.items = under.into_iter().chain(kept).chain(over).collect();
    let box_of = |i: usize| net.instance_of(i).map(|k| inst_boxes[k]);
    // a node's members, by index and name: one, itself, outside a stack
    let members_of = |i: usize| -> Vec<(usize, String)> {
        let name = net.name(p, i);
        match stack_of(i) {
            Some(s) => (0..s.count).map(|m| (m, s.name(&name, m))).collect(),
            None => vec![(0, name)],
        }
    };
    let dy = |m: usize| m as f64 * pitch;
    // the members an arrow joins: within one stacked instance each to
    // itself, else each to each
    let pairs = |a: usize, b: usize| -> Vec<((usize, String), (usize, String))> {
        let (ma, mb) = (members_of(a), members_of(b));
        if stack_of(a).is_some() && net.instance_of(a) == net.instance_of(b) {
            ma.into_iter().zip(mb).collect()
        } else {
            ma.iter()
                .flat_map(|x| mb.iter().map(move |y| (x.clone(), y.clone())))
                .collect()
        }
    };
    // From member `m`'s station `i`, at `x`, down to below the stack: a
    // member above the last leaves its box through the gap under it and
    // goes down beside the stack, on the side `right` says.
    let down = |i: usize, m: usize, x: f64, right: bool| -> Vec<Point> {
        let top = pt(x, frames[i].bottom() + dy(m));
        match (stack_of(i), box_of(i)) {
            (Some(s), Some(b)) if m + 1 < s.count => {
                let gy = b.bottom() + dy(m) + 8.0;
                let side = if right { b.right() + 8.0 } else { b.x - 8.0 };
                vec![top, pt(x, gy), pt(side, gy)]
            }
            _ => vec![top],
        }
    };
    // along a lane at `y`: from `a`'s member down, across, up into `b`'s
    let lane_path = |a: (usize, usize, f64), b: (usize, usize, f64), y: f64| -> Vec<Point> {
        let mut pts = down(a.0, a.1, a.2, b.2 > a.2);
        pts.push(pt(pts.last().unwrap().x, y));
        let mut up = down(b.0, b.1, b.2, a.2 > b.2);
        up.reverse();
        pts.push(pt(up[0].x, y));
        pts.extend(up);
        pts
    };
    // along the row, from `a`'s member to `b`'s: straight between two
    // stacks, else turning down (or up) by the stack's side
    let row_path = |a: (usize, usize), b: (usize, usize)| -> Vec<Point> {
        let (ra, rb) = (rects[a.0], rects[b.0]);
        let (ya, yb) = (ra.centre().y + dy(a.1), rb.centre().y + dy(b.1));
        if ya == yb || (stack_of(a.0).is_some() && stack_of(b.0).is_some()) {
            return vec![pt(ra.right(), ya), pt(rb.x, yb)];
        }
        let x = match (box_of(b.0), box_of(a.0)) {
            (Some(bb), _) if stack_of(b.0).is_some() => bb.x - 8.0,
            (_, Some(ba)) => ba.right() + 8.0,
            _ => (ra.right() + rb.x) / 2.0,
        };
        vec![pt(ra.right(), ya), pt(x, ya), pt(x, yb), pt(rb.x, yb)]
    };

    // a copy across a stacked instance's box: an arrow from each member to each,
    // what it moves and waits for written once, above the first
    for (g, group) in net.flows.iter().enumerate() {
        if !stacked_copy(g) {
            continue;
        }
        let (a, b) = (group[0], *group.last().unwrap());
        let copy = copy_mark(g);
        for ((ma, na), (mb, nb)) in pairs(a, b) {
            let mark = copy.clone().map(|c| CopyMark {
                poster: if c.poster == net.name(p, a) {
                    na.clone()
                } else if c.poster == net.name(p, b) {
                    nb.clone()
                } else {
                    c.poster
                },
                ..c
            });
            f.copy_flow(row_path((a, ma), (b, mb)), na, nb, mark);
        }
        // the poster: a dot over each member's station, with no bracket
        if let Some(&node) = net.flow_notes[g].posters.first()
            && copy.is_some()
        {
            for (m, _) in members_of(node) {
                let r = rects[node];
                f.push(Item::Dot {
                    at: pt(r.centre().x, r.y - 6.0 + dy(m)),
                    r: 3.0,
                });
            }
        }
        let (ra, rb) = (rects[a], rects[b]);
        let (mid, y) = ((ra.right() + rb.x) / 2.0, ra.centre().y);
        let (moves, wait) = flow_texts(g);
        let lines: Vec<String> = [moves, wait].into_iter().flatten().collect();
        for (k, t) in lines.iter().enumerate() {
            let above = (lines.len() - k) as f64;
            f.note(
                pt(mid, y - 8.0 - (above - 1.0) * NOTE_LINE),
                t.clone(),
                Anchor::Middle,
            );
        }
    }

    // One counter for every edge that needs a lane below the station row:
    // feedback, an early exit and a second entry point must not share one.
    let mut lanes = 0.0;
    let deep = below.iter().copied().fold(0.0, f64::max);
    // and clear of every frame, and of every enclosure around one
    let floor = frames
        .iter()
        .chain(enclosures.iter().map(|(_, _, r)| r))
        .map(|r| r.bottom())
        .fold(0.0, f64::max);
    // and below the members
    let base = (row_y + STATION_H + deep + BACK_DROP).max(floor + 14.0)
        + stacks
            .iter()
            .flatten()
            .map(|s| (s.count - 1) as f64 * pitch)
            .fold(0.0, f64::max);
    let lane = |n: &mut f64| {
        *n += 1.0;
        base + *n * 22.0
    };
    // The client: where a request comes from and its reply goes back to.
    // An arrival leaves it by its right side, or by its bottom for a lane;
    // an exit comes back into its bottom, right of where those leave.
    f.push(Item::Station {
        rect: client,
        kind: StationKind::Client,
        // what the reader reads; `CLIENT` is what a page addresses
        text: "client".into(),
        outline: None,
        stage: None,
        node: CLIENT.into(),
    });
    let (client_out, client_in) = (client.x + client.w * 0.3, client.x + client.w * 0.7);
    let mut arrivals = 0;
    for e in &net.edges {
        let label = e.label.clone();
        match (e.from, e.to) {
            (End::Arrival, End::Node(i)) => {
                let r = rects[i];
                let y = r.centre().y;
                arrivals += 1;
                if arrivals == 1 && i == 0 {
                    // The arrow leaves the client and the label rides above
                    // it, clear of the pool glyphs it passes; into a stack,
                    // it turns down beside it to each member.
                    for (m, to) in members_of(i) {
                        let pts = match box_of(i) {
                            Some(b) if m > 0 => vec![
                                pt(client.right(), y),
                                pt(b.x - 8.0, y),
                                pt(b.x - 8.0, y + dy(m)),
                                pt(r.x, y + dy(m)),
                            ],
                            _ => vec![pt(client.right(), y), pt(r.x, y)],
                        };
                        f.arrival(pts, to);
                    }
                    f.note(
                        pt(client.right() + 6.0, y - 10.0),
                        arrival_text.clone(),
                        Anchor::Start,
                    );
                } else {
                    // A session that opens with a branch has more than one entry
                    // station, and one may enter past the first station. An
                    // arrow along the row would run straight through the
                    // stations before it, so it takes a lane of its own.
                    if arrivals == 1 {
                        f.note(
                            pt(client.right() + 6.0, y - 10.0),
                            arrival_text.clone(),
                            Anchor::Start,
                        );
                    }
                    let ly = lane(&mut lanes);
                    let x = r.x + r.w * 0.25;
                    for (m, to) in members_of(i) {
                        let mut up = down(i, m, x, false);
                        up.reverse();
                        let mut pts = vec![
                            pt(client_out, client.bottom()),
                            pt(client_out, ly),
                            pt(up[0].x, ly),
                        ];
                        pts.extend(up);
                        f.arrival(pts, to);
                    }
                }
            }
            (End::Node(i), End::Exit) => {
                // The reply goes back to the client, through the server's
                // wall, along a lane of its own under the row: the last
                // station's leaves to the right and turns down, an earlier
                // one's drops from it rather than running a line through
                // the stations after it. A session that ends inside the
                // server has its guard written where it leaves.
                let r = rects[i];
                let y = lane(&mut lanes);
                let last = i + 1 == net.nodes.len();
                // clear of the row and of the panels around its last station
                let x = if last {
                    inst_boxes.iter().map(Rect::right).fold(row_right, f64::max) + 14.0
                } else {
                    r.x + r.w * 0.75
                };
                for (m, from) in members_of(i) {
                    let mut pts = if last {
                        let ym = r.centre().y + dy(m);
                        vec![pt(r.right(), ym), pt(x, ym)]
                    } else {
                        down(i, m, x, true)
                    };
                    pts.push(pt(pts.last().unwrap().x, y));
                    pts.push(pt(client_in, y));
                    pts.push(pt(client_in, client.bottom()));
                    f.exit(pts, from);
                }
                if let Some(l) = label {
                    let at = if last { x - 4.0 } else { x + 4.0 };
                    let anchor = if last { Anchor::End } else { Anchor::Start };
                    f.note(pt(at, y - 5.0), l, anchor);
                }
            }
            (End::Node(a), End::Node(b)) if e.reply => {
                // A reply: from the station the route ended at back to its
                // gateway, along a lane of its own, into the gateway's
                // underside left of where the request leaves it. It
                // leaves by a point no other arrow does (0.875) and a
                // stacked member by the stack's right, where the arrows
                // into the stack do not run
                let (ra, rb) = (rects[a], rects[b]);
                let y = lane(&mut lanes);
                let (ax, bx) = (ra.x + ra.w * 0.875, rb.x + rb.w * 0.375);
                for ((ma, na), (mb, nb)) in pairs(a, b) {
                    let mut pts = down(a, ma, ax, true);
                    pts.push(pt(pts.last().unwrap().x, y));
                    let mut up = down(b, mb, bx, false);
                    up.reverse();
                    pts.push(pt(up[0].x, y));
                    pts.extend(up);
                    f.reply(pts, na, nb);
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
                for ((ma, na), (mb, nb)) in pairs(a, b) {
                    f.flow(
                        lane_path((a, ma, ax), (b, mb, bx), y),
                        EdgeStyle::Flow,
                        na,
                        nb,
                    );
                }
                if let Some(l) = label {
                    f.note(
                        pt((ra.centre().x + rb.centre().x) / 2.0, y - 5.0),
                        l,
                        Anchor::Middle,
                    );
                }
            }
            (End::Node(a), End::Node(b)) if !e.back => {
                let ra = rects[a];
                for ((ma, na), (mb, nb)) in pairs(a, b) {
                    f.flow(row_path((a, ma), (b, mb)), EdgeStyle::Flow, na, nb);
                }
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
                for ((ma, na), (mb, nb)) in pairs(a, b) {
                    f.flow(
                        lane_path((a, ma, ax), (b, mb, bx), y),
                        EdgeStyle::Back,
                        na,
                        nb,
                    );
                }
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

    // The server: a frame around all but the client, under everything. The
    // arrows to and from the client cross its wall, and the lanes they take
    // under the row are inside it.
    let mut ext = Rect::new(server_left + SERVER_PAD, row_y, 0.0, 0.0);
    for item in &f.items {
        let r = match item {
            Item::Station { node, .. } if node == CLIENT => continue,
            Item::Box { rect, .. }
            | Item::Station { rect, .. }
            | Item::Queue { rect, .. }
            | Item::Drum { rect, .. } => *rect,
            Item::Edge { pts, .. } => {
                for q in pts.iter().filter(|q| q.x > server_left) {
                    ext = ext.union(&Rect::new(q.x, q.y, 0.0, 0.0));
                }
                continue;
            }
            Item::Dot { at, r } => Rect::new(at.x - r, at.y - r, 2.0 * r, 2.0 * r),
            Item::Mark { at, size, .. } => {
                Rect::new(at.x - size / 2.0, at.y - size / 2.0, *size, *size)
            }
            // the arrival's label stands outside, over its arrow
            Item::Text { at, .. } if at.x < server_left => continue,
            Item::Text {
                at,
                text,
                anchor,
                size,
                ..
            } => {
                let w = size.width_of(text);
                let x = match anchor {
                    Anchor::Start => at.x,
                    Anchor::Middle => at.x - w / 2.0,
                    Anchor::End => at.x - w,
                };
                Rect::new(x, at.y - size.points(), w, size.points())
            }
        };
        ext = ext.union(&r);
    }
    let server = Rect::new(
        server_left,
        ext.y - SERVER_PAD,
        ext.right() + SERVER_PAD - server_left,
        ext.h + 2.0 * SERVER_PAD,
    );
    f.items.insert(
        0,
        Item::Box {
            rect: server,
            style: BoxStyle::Server,
            round: 12.0,
            nodes: vec![],
            copy: None,
        },
    );
    f.text(
        pt(server.x + 10.0, server.y - 5.0),
        "server",
        Anchor::Start,
        TextSize::Normal,
    );

    f.fit(MARGIN);
    f
}

/// A pool rule's keys as the program reads them, comma-separated.
fn show_keys(p: &Program, keys: &[crate::ir::CExpr]) -> String {
    keys.iter()
        .map(|k| p.show_expr(k))
        .collect::<Vec<_>>()
        .join(", ")
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
    // the order the pool's waiting holds are let in; FIFO says nothing. A
    // key the IR writes out whole (a `def` inlined) is cut short: the
    // program says it in full
    if let Some(keys) = &pool.queue {
        let all = show_keys(p, keys);
        let shown = if all.chars().count() > QUEUE_KEYS_MAX {
            let cut: String = all.chars().take(QUEUE_KEYS_MAX).collect();
            // at a space, not inside a name or a number
            let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
            format!("{} …", cut.trim_end())
        } else {
            all
        };
        parts.push(format!("queue by ({shown})"));
    }
    if let Some(dt) = pool.queue_every {
        parts.push(format!("queue every ({dt})"));
    }
    // An eviction order says nothing about a pool nothing is cached in.
    if cached {
        match &pool.evict {
            crate::ir::CEvict::Lru => parts.push("evict lru".into()),
            crate::ir::CEvict::By(keys) => parts.push(format!("evict by ({})", show_keys(p, keys))),
        }
    }
    match &pool.preempt {
        crate::ir::Preempt::None => {}
        lifo if lifo.is_lifo() => parts.push("preempt lifo".into()),
        crate::ir::Preempt::By { keys, tail } => parts.push(format!(
            "preempt by ({}){}",
            show_keys(p, keys),
            if *tail { " requeue tail" } else { "" }
        )),
        crate::ir::Preempt::Leases {
            keys,
            every,
            suspend,
        } => parts.push(format!(
            "preempt leases by ({}){}{}",
            show_keys(p, keys),
            every.map_or(String::new(), |dt| format!(" every ({dt})")),
            suspend.as_ref().map_or(String::new(), |ks| format!(
                " suspend by ({})",
                show_keys(p, ks)
            ))
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

/// `figure`, refused when a station has no arrow in or no arrow out: every
/// engine, NIC and stage a request reaches has a way in and a way out
/// (#624). A program that links cannot have such a station (a stage
/// nothing runs does not link, and a request goes on from every station
/// it reaches), so this is the view failing the program, which `serq
/// check` and `serq draw` say rather than draw a station cut off.
pub fn checked_figure(p: &Program) -> Result<Figure, String> {
    let f = figure(p);
    match f.unjoined() {
        None => Ok(f),
        Some((node, which)) => Err(format!(
            "the deployment view draws station `{node}` with no arrow {which}, and every station \
             has an arrow in and one out\n\
             help: this is the view's fault, not the program's: report it with the program \
             (https://github.com/servingQ/serQ/issues)"
        )),
    }
}

/// `e` with `request.loaded` a positive number: what an admission takes
/// while its connector loads.
fn with_loaded(e: &CExpr) -> CExpr {
    let f = with_loaded;
    match e {
        CExpr::Ctx(CtxVar::Loaded) => CExpr::Num(1.0),
        CExpr::Cost(t, x) => CExpr::Cost(t.clone(), Box::new(f(x))),
        CExpr::Unary(o, x) => CExpr::Unary(*o, Box::new(f(x))),
        CExpr::Binary(o, a, b) => CExpr::Binary(*o, Box::new(f(a)), Box::new(f(b))),
        CExpr::Cond(c, a, b) => CExpr::Cond(Box::new(f(c)), Box::new(f(a)), Box::new(f(b))),
        other => other.clone(),
    }
}
