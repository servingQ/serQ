//! A pool's prefix cache: a trie of keyed segments
//! (`docs/design/shared-prefix-cache.md`).
//!
//! A hold's sequence is a chain of segments, each a key and a length
//! (`prefix (k₁: n₁, …)`, or `(serial: ∞)` without the clause). A node
//! holds a run of one segment, `[off, off + len)`; a segment's runs follow
//! one another as `Cont` children, and the next segment hangs off the run
//! that ends where its segment ends, under its key. Two chains share the
//! nodes of their equal leading keys. With every chain `(serial: ∞)` the
//! trie is a forest of one-node trees, one per session: the map from serial
//! to entry the interpreter kept before.
//!
//! A node's units are cached while no hold reads through it and allocated
//! while one does (it is locked); the interpreter moves them between its
//! pool's `cached` and `used` with the deltas these functions return. A
//! hold keeps the ids of the nodes it locked: a locked node is never split,
//! extended, trimmed, cut off or evicted, so the ids stay its own.
//!
//! A prefix a session has locked, and no other session reads, is the
//! session's consumed entry: its other holds do not see it (a hold nested
//! in one that consumed the prefix finds none, #230), and a write of the
//! session's chain there displaces it, unreachable, until its hold lets it
//! go.

use std::collections::HashMap;

const EPS: f64 = 1e-9;

/// The edge from a node to a child: the next segment's key, or the next
/// run of the same segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Edge {
    Key(u64),
    Cont,
}

fn key(k: f64) -> Edge {
    // -0 and 0 are one key
    Edge::Key(if k == 0.0 {
        0.0f64.to_bits()
    } else {
        k.to_bits()
    })
}

#[derive(Clone, Debug)]
pub(super) struct Node {
    parent: Option<usize>,
    seg: usize,
    off: f64,
    pub len: f64,
    children: Vec<(Edge, usize)>,
    /// The serials of the holds that read through it, one per hold.
    lockers: Vec<u64>,
    pub last: f64,
    pub seq: u64,
    pub snap: Vec<f64>,
    /// The serial of the session that wrote it last.
    pub owner: u64,
    /// Cut off by a `reuse` bound: no chain reaches it, and it stays cached,
    /// unusable, until it is evicted. Its tag names it to a spill tier.
    pub dead: Option<u64>,
    /// Displaced by a write of its session while that session held it: no
    /// chain reaches it, and it goes when the hold lets it go.
    displaced: bool,
    live: bool,
}

impl Node {
    fn fresh(seg: usize, off: f64, len: f64, owner: u64, snap: &[f64], now: f64, seq: u64) -> Self {
        Node {
            parent: None,
            seg,
            off,
            len,
            children: vec![],
            lockers: vec![],
            last: now,
            seq,
            snap: snap.to_vec(),
            owner,
            dead: None,
            displaced: false,
            live: true,
        }
    }

    pub fn locked(&self) -> bool {
        !self.lockers.is_empty()
    }

    /// Locked by `viewer`'s holds alone: its consumed entry, which it does
    /// not see.
    fn hidden(&self, viewer: Option<u64>) -> bool {
        viewer.is_some_and(|v| self.locked() && self.lockers.iter().all(|&s| s == v))
    }
}

/// One node a walk passed, and how many of its units, from its start.
#[derive(Clone, Copy, Debug)]
struct Step {
    node: usize,
    take: f64,
}

/// Where a walk stopped: the steps, the units matched, and the (segment,
/// offset) it stopped at.
struct Walk {
    steps: Vec<Step>,
    total: f64,
    seg: usize,
    off: f64,
}

#[derive(Clone, Debug)]
pub(super) struct Trie {
    nodes: Vec<Node>,
    free: Vec<usize>,
    /// The dead node a spill of a dead node of another pool extends.
    dead_spills: HashMap<u64, usize>,
}

impl Default for Trie {
    fn default() -> Self {
        Trie {
            nodes: vec![Node::fresh(usize::MAX, 0.0, 0.0, 0, &[], 0.0, 0)],
            free: vec![],
            dead_spills: HashMap::new(),
        }
    }
}

impl Trie {
    pub fn node(&self, id: usize) -> &Node {
        &self.nodes[id]
    }

    fn child(&self, n: usize, e: Edge) -> Option<usize> {
        self.nodes[n]
            .children
            .iter()
            .find(|(x, _)| *x == e)
            .map(|&(_, c)| c)
    }

    fn alloc(&mut self, node: Node) -> usize {
        match self.free.pop() {
            Some(i) => {
                self.nodes[i] = node;
                i
            }
            None => {
                self.nodes.push(node);
                self.nodes.len() - 1
            }
        }
    }

    fn add_child(&mut self, parent: usize, e: Edge, node: Node) -> usize {
        let id = self.alloc(Node {
            parent: Some(parent),
            ..node
        });
        self.nodes[parent].children.push((e, id));
        id
    }

    fn unlink(&mut self, id: usize) {
        if let Some(p) = self.nodes[id].parent.take() {
            self.nodes[p].children.retain(|&(_, c)| c != id);
        }
    }

    fn remove(&mut self, id: usize) {
        debug_assert!(self.nodes[id].children.is_empty() && !self.nodes[id].locked());
        self.unlink(id);
        if self.nodes[id].dead.is_some() {
            self.dead_spills.retain(|_, n| *n != id);
        }
        let n = &mut self.nodes[id];
        n.live = false;
        n.len = 0.0;
        self.free.push(id);
    }

    /// Walk `chain` from the root for at most `limit` units, as `viewer`
    /// sees it (`None`: every node).
    fn walk(&self, chain: &[(f64, f64)], limit: f64, viewer: Option<u64>) -> Walk {
        let mut w = Walk {
            steps: vec![],
            total: 0.0,
            seg: 0,
            off: 0.0,
        };
        let mut at = 0;
        for (i, &(k, n)) in chain.iter().enumerate() {
            w.seg = i;
            w.off = 0.0;
            let mut next = self.child(at, key(k));
            loop {
                let Some(cur) = next.filter(|&c| !self.nodes[c].hidden(viewer)) else {
                    return w;
                };
                let node = &self.nodes[cur];
                let end = node.off + node.len;
                let take = (end.min(n) - node.off).min(limit - w.total);
                if take <= EPS {
                    return w;
                }
                w.steps.push(Step { node: cur, take });
                w.total += take;
                w.off = node.off + take;
                if take < node.len - EPS {
                    // stopped inside the node: the limit, or a segment that
                    // ends before this run does (no next segment hangs here)
                    return w;
                }
                if end >= n - EPS {
                    at = cur;
                    break;
                }
                if w.total >= limit - EPS {
                    return w;
                }
                next = self.child(cur, Edge::Cont);
            }
            if w.total >= limit - EPS {
                // the segment is complete; the walk stands at its end
                w.seg = i + 1;
                w.off = 0.0;
                return w;
            }
        }
        w.seg = chain.len();
        w.off = 0.0;
        w
    }

    /// The node the chain continues into where the walk stopped, if the
    /// walk stopped at a node's end.
    fn next_on(&self, chain: &[(f64, f64)], w: &Walk) -> Option<usize> {
        match w.steps.last() {
            None => self.child(0, key(chain.first()?.0)),
            Some(s) if s.take < self.nodes[s.node].len - EPS => None,
            Some(s) if w.seg < chain.len() && w.off > EPS => self.child(s.node, Edge::Cont),
            Some(s) => self.child(s.node, key(chain.get(w.seg)?.0)),
        }
    }

    /// The units of `chain` cached or read through, as `viewer` sees them.
    pub fn matched(&self, chain: &[(f64, f64)], viewer: u64) -> f64 {
        self.walk(chain, f64::INFINITY, Some(viewer)).total
    }

    /// Of the first `limit` units of `chain`, those another live hold reads
    /// through: allocated already, so an admission that reads them too
    /// does not ask for them again.
    pub fn locked_along(&self, chain: &[(f64, f64)], limit: f64, viewer: u64) -> f64 {
        self.walk(chain, limit, Some(viewer))
            .steps
            .iter()
            .filter(|s| self.nodes[s.node].locked())
            .map(|s| s.take)
            .sum()
    }

    /// Split `id` after `at` of its units: the rest becomes its `Cont` child,
    /// with its children and its age. Only a node no hold reads is split.
    fn split(&mut self, id: usize, at: f64) -> usize {
        debug_assert!(!self.nodes[id].locked());
        let n = self.nodes[id].clone();
        let children = std::mem::take(&mut self.nodes[id].children);
        self.nodes[id].len = at;
        let tail = self.add_child(
            id,
            Edge::Cont,
            Node {
                off: n.off + at,
                len: n.len - at,
                children: vec![],
                ..n
            },
        );
        for &(_, c) in &children {
            self.nodes[c].parent = Some(tail);
        }
        self.nodes[tail].children = children;
        tail
    }

    /// A hold of `viewer` reads through the first `limit` units of `chain`
    /// that it sees cached: lock them. A node the match stops inside is
    /// split if no hold reads it, else locked whole. Returns (the units
    /// matched, those that were cached and are now allocated, the nodes).
    pub fn lock(
        &mut self,
        chain: &[(f64, f64)],
        limit: f64,
        viewer: u64,
    ) -> (f64, f64, Vec<usize>) {
        let w = self.walk(chain, limit, Some(viewer));
        if let Some(last) = w.steps.last()
            && last.take < self.nodes[last.node].len - EPS
            && !self.nodes[last.node].locked()
        {
            self.split(last.node, last.take);
        }
        let mut moved = 0.0;
        let path: Vec<usize> = w.steps.iter().map(|s| s.node).collect();
        for &id in &path {
            let n = &mut self.nodes[id];
            if !n.locked() {
                moved += n.len;
            }
            n.lockers.push(viewer);
        }
        (w.total, moved, path)
    }

    /// A hold of `viewer` lets the nodes of `path` go. Returns (the units
    /// no hold reads any more and that are cached again, those that went
    /// with a displaced node).
    pub fn unlock(&mut self, path: &[usize], viewer: u64) -> (f64, f64) {
        let (mut back, mut gone) = (0.0, 0.0);
        for &id in path.iter().rev() {
            let n = &mut self.nodes[id];
            let i = n.lockers.iter().position(|&s| s == viewer);
            debug_assert!(
                n.live && i.is_some(),
                "unlock of a node the hold did not lock"
            );
            if let Some(i) = i {
                n.lockers.swap_remove(i);
            }
            if n.locked() {
                continue;
            }
            if n.displaced && n.children.is_empty() {
                gone += n.len;
                self.remove(id);
            } else {
                if n.displaced {
                    // what hung off it is no chain's any more
                    n.dead = Some(u64::MAX);
                }
                back += n.len;
            }
        }
        (back, gone)
    }

    /// `reuse` bounded the match at `m`: what the chain held past it is not
    /// this hold's sequence any more. Cut it off, cached and unusable, as a
    /// dead node with its age; unless a live hold reads it.
    pub fn cut(&mut self, chain: &[(f64, f64)], m: f64, tag: u64) {
        let w = self.walk(chain, m, None);
        let Some(next) = self.next_on(chain, &w) else {
            return;
        };
        if self.nodes[next].locked() {
            return;
        }
        self.unlink(next);
        let mut stack = vec![next];
        while let Some(n) = stack.pop() {
            self.nodes[n].dead = Some(tag);
            stack.extend(self.nodes[n].children.iter().map(|&(_, c)| c));
        }
    }

    /// A hold of `owner` ends with `keep` units of `chain` computed: they are
    /// cached along the chain, aged now. With `keep > 0` the chain past
    /// `keep` that only `owner` wrote, and no hold reads, goes (the
    /// session's entry is replaced); with `keep = 0` the nodes the hold read
    /// (`path`) go, and nothing else (a hold that keeps nothing consumed its
    /// prefix). Returns the change in cached units.
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &mut self,
        chain: &[(f64, f64)],
        keep: f64,
        path: &[usize],
        owner: u64,
        snap: &[f64],
        now: f64,
        seq: u64,
    ) -> f64 {
        let mut delta = 0.0;
        let gone = |n: &Node| n.live && !n.locked() && n.children.is_empty() && n.owner == owner;
        if keep <= EPS {
            for &id in path.iter().rev() {
                let n = &self.nodes[id];
                if gone(n) && !n.displaced && n.dead.is_none() {
                    delta -= n.len;
                    self.remove(id);
                }
            }
            return delta;
        }
        let w = self.walk(chain, f64::INFINITY, Some(owner));
        // the session's consumed entry stands where its chain goes on: the
        // write takes its place, and it stays its hold's until let go
        if let Some(next) = self.next_on(chain, &w)
            && self.nodes[next].hidden(Some(owner))
        {
            self.unlink(next);
            self.nodes[next].displaced = true;
        }
        // touch what lies within `keep`
        let mut start = 0.0;
        for s in &w.steps {
            if start >= keep - EPS {
                break;
            }
            let n = &mut self.nodes[s.node];
            n.last = now;
            n.seq = seq;
            n.owner = owner;
            n.snap = snap.to_vec();
            start += s.take;
        }
        // trim the session's own tail past `keep`, from the end
        let mut end = w.total;
        for s in w.steps.iter().rev() {
            let n = &self.nodes[s.node];
            let from = end - s.take;
            if end <= keep + EPS || !gone(n) || s.take < n.len - EPS {
                break;
            }
            if from >= keep - EPS {
                delta -= n.len;
                self.remove(s.node);
                end = from;
            } else {
                let cut = end - keep;
                self.nodes[s.node].len -= cut;
                delta -= cut;
                break;
            }
        }
        // extend past what the chain holds
        let mut have = w.total;
        if keep > have + EPS && w.seg < chain.len() {
            let mut at = w.steps.last().map_or(0, |s| s.node);
            let (mut seg, mut off) = (w.seg, w.off);
            if let Some(s) = w.steps.last()
                && s.take < self.nodes[s.node].len - EPS
            {
                if self.nodes[s.node].locked() {
                    // the segment ends inside a run another hold reads: the
                    // next one cannot hang there yet
                    return delta;
                }
                self.split(s.node, s.take);
            }
            while keep > have + EPS && seg < chain.len() {
                let (k, n) = chain[seg];
                let add = (n - off).min(keep - have);
                if add > EPS {
                    let node = &self.nodes[at];
                    let same = at != 0 && node.seg == seg && off > EPS;
                    if same && !node.locked() && node.children.is_empty() && node.dead.is_none() {
                        self.nodes[at].len += add;
                    } else {
                        let e = if same { Edge::Cont } else { key(k) };
                        at = self.add_child(
                            at,
                            e,
                            Node::fresh(seg, off, add, owner, snap, now, seq),
                        );
                    }
                    let n = &mut self.nodes[at];
                    n.last = now;
                    n.seq = seq;
                    n.owner = owner;
                    n.snap = snap.to_vec();
                    delta += add;
                    have += add;
                    off += add;
                }
                if off >= n - EPS {
                    seg += 1;
                    off = 0.0;
                }
            }
        }
        delta
    }

    /// The nodes eviction may take: cached leaves.
    pub fn leaves(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter_map(|(i, n)| {
            (i != 0 && n.live && !n.locked() && n.children.is_empty() && n.len > EPS).then_some(i)
        })
    }

    /// Evict `units` from the tail of leaf `id`. Returns (the units evicted,
    /// the parent if the leaf went and the parent is now an evictable leaf,
    /// whether the leaf went).
    pub fn evict_tail(&mut self, id: usize, units: f64) -> (f64, Option<usize>, bool) {
        let n = &mut self.nodes[id];
        let removed = units.min(n.len);
        n.len -= removed;
        if n.len > EPS {
            return (removed, None, false);
        }
        let parent = n.parent;
        self.remove(id);
        let leaf = parent.filter(|&p| {
            let n = &self.nodes[p];
            p != 0 && !n.locked() && n.children.is_empty() && n.len > EPS
        });
        (removed, leaf, true)
    }

    /// `drop P`: the session's own cached nodes go, leaves first. Returns
    /// the units dropped.
    pub fn drop_owner(&mut self, owner: u64) -> f64 {
        let mut gone = 0.0;
        loop {
            let found: Vec<usize> = (1..self.nodes.len())
                .filter(|&i| {
                    let n = &self.nodes[i];
                    n.live
                        && n.owner == owner
                        && n.dead.is_none()
                        && !n.displaced
                        && !n.locked()
                        && n.children.is_empty()
                })
                .collect();
            if found.is_empty() {
                return gone;
            }
            for i in found {
                gone += self.nodes[i].len;
                self.remove(i);
            }
        }
    }

    /// A spill from another pool: `units` of `owner`'s own entry (or of the
    /// dead node `dead`) arrive in this tier, which keeps them as that
    /// session's entry, extended (its snapshot as it was). Returns the units
    /// added.
    pub fn spill_in(
        &mut self,
        owner: u64,
        dead: Option<u64>,
        units: f64,
        snap: Vec<f64>,
        now: f64,
        seq: u64,
    ) -> f64 {
        let extend = |n: &mut Node| {
            n.len += units;
            n.last = now;
            n.seq = seq;
        };
        if let Some(t) = dead {
            match self.dead_spills.get(&t) {
                Some(&i) if self.nodes[i].live => extend(&mut self.nodes[i]),
                _ => {
                    let mut n = Node::fresh(0, 0.0, units, owner, &snap, now, seq);
                    n.dead = Some(t);
                    let i = self.alloc(n);
                    self.dead_spills.insert(t, i);
                }
            }
            return units;
        }
        // the session's run, to its end, as the session sees it
        let k = key(owner as f64);
        let mut cur = match self.child(0, k) {
            Some(c) if self.nodes[c].hidden(Some(owner)) => {
                self.unlink(c);
                self.nodes[c].displaced = true;
                None
            }
            c => c,
        };
        let Some(mut at) = cur.take() else {
            self.add_child(0, k, Node::fresh(0, 0.0, units, owner, &snap, now, seq));
            return units;
        };
        while let Some(c) = self.child(at, Edge::Cont) {
            at = c;
        }
        let n = &self.nodes[at];
        if !n.locked() && n.children.is_empty() {
            extend(&mut self.nodes[at]);
        } else {
            let off = n.off + n.len;
            self.add_child(
                at,
                Edge::Cont,
                Node::fresh(0, off, units, owner, &snap, now, seq),
            );
        }
        units
    }

    /// (Cached units, locked units counted once, locked units counted per
    /// hold that reads them), for the interpreter's conservation check.
    #[cfg(debug_assertions)]
    pub fn totals(&self) -> (f64, f64, f64) {
        let (mut cached, mut locked, mut read) = (0.0, 0.0, 0.0);
        for n in self.nodes.iter().skip(1).filter(|n| n.live) {
            if n.locked() {
                locked += n.len;
                read += n.lockers.len() as f64 * n.len;
            } else {
                cached += n.len;
            }
        }
        (cached, locked, read)
    }

    /// The units of the nodes of `path`.
    #[cfg(debug_assertions)]
    pub fn units(&self, path: &[usize]) -> f64 {
        path.iter().map(|&i| self.nodes[i].len).sum()
    }

    /// Live nodes, for a report of who holds the cache.
    pub fn live(&self) -> impl Iterator<Item = &Node> + '_ {
        self.nodes.iter().skip(1).filter(|n| n.live && n.len > EPS)
    }
}
