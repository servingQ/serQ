//! The session view: one session's path, at full fidelity.
//!
//! The deployment view (`crate::deployment`) quotients the session program
//! to its stages. This one keeps every statement. Its subject is the thing a
//! flowchart cannot show: `hold` is a *scope*, so it is drawn as a band — a
//! region of a pool's column occupied over a span of the program — and the
//! units a scope leaves cached are a tail that outlives the band.
//!
//! Vertical is position in the program, not time. Band widths are nominal:
//! a single session's allocation against a pool of 160 000 units would be
//! invisible, and magnitude is not what this view is for. What the geometry
//! does carry is *when* a width is decided — a solid edge for a width the
//! constants fix, a dashed one for a width evaluated at admission.

use crate::figure::{Anchor, BoxStyle, EdgeStyle, Figure, Item, Rect, TextSize, pt};
use crate::ir::{CExpr, CStmt, Program, RunMode};

const ROW_H: f64 = 26.0;
const COL_W: f64 = 168.0;
const COL_GAP: f64 = 14.0;
const BAND_INSET: f64 = 10.0;
const CACHE_H: f64 = 10.0;
const RAIL_W: f64 = 13.0;
const SPINE_W: f64 = 330.0;
/// Longest label a spine row shows before it is elided.
const SPINE_CHARS: usize = 54;
const MARGIN: f64 = 26.0;
const HEAD_H: f64 = 46.0;

/// How deeply `branch` and `loop` nest in the session program. The spine has to start
/// clear of the rails, and a rail drawn over the statement column is worse
/// than a wide margin.
fn control_depth(p: &Program, block: usize) -> usize {
    let Some(stmts) = p.blocks.get(block) else {
        return 0;
    };
    let mut max = 0;
    for s in stmts {
        let d = match s {
            CStmt::Branch(_, t, e) => 1 + control_depth(p, *t).max(control_depth(p, *e)),
            CStmt::Loop(b) => 1 + control_depth(p, *b),
            CStmt::Hold { body, .. } => control_depth(p, *body),
            _ => 0,
        };
        max = max.max(d);
    }
    max
}

/// Pools that any `hold` in the program acquires, in declaration order.
fn held_pools(p: &Program) -> Vec<usize> {
    let mut v: Vec<usize> = vec![];
    for block in &p.blocks {
        for s in block {
            if let CStmt::Hold { pools, .. } = s {
                for (r, _, _) in pools {
                    if !v.contains(&r.base) {
                        v.push(r.base);
                    }
                }
            }
        }
    }
    v.sort_unstable();
    v
}

/// Shorten to `max` characters. A label that reaches the next column is
/// worse than one that stops.
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{keep}~")
}

struct Draw<'a> {
    p: &'a Program,
    f: Figure,
    /// The bands themselves: the deepest layer.
    bands: Vec<Item>,
    /// Wedges and column labels: over the bands, under the spine.
    under: Vec<Item>,
    cols: Vec<usize>,
    /// Left edge of the spine, after the control rails.
    spine_x: f64,
    y: f64,
    depth: usize,
    show_set: bool,
    /// Tails still open at the bottom of the figure, by pool.
    open_tails: Vec<(usize, f64)>,
    /// Rows at which a `release` gave a pool back before its hold ended, by
    /// pool, with the control depth it was written at: the hold's band for
    /// that pool stops there when the release is on every path (written at
    /// the hold's own depth), and only the marker says it otherwise.
    released: Vec<(usize, f64, usize)>,
    /// Per hold body, the pools its `cache` clause can leave units in.
    cache_targets: std::collections::BTreeMap<usize, Vec<usize>>,
}

impl Draw<'_> {
    fn band_box(&mut self, rect: Rect, style: BoxStyle, round: f64) {
        self.bands.push(Item::Box { rect, style, round });
    }
    fn under_note(&mut self, at: crate::figure::Point, text: String) {
        self.under.push(Item::Text {
            at,
            text,
            anchor: Anchor::Start,
            size: TextSize::Small,
            dim: true,
        });
    }
}

impl<'a> Draw<'a> {
    fn col_x(&self, pool: usize) -> Option<f64> {
        let i = self.cols.iter().position(|&c| c == pool)?;
        Some(self.spine_x + SPINE_W + COL_GAP + i as f64 * (COL_W + COL_GAP))
    }

    fn rail_x(&self) -> f64 {
        MARGIN + self.depth as f64 * RAIL_W
    }

    fn row(&mut self) -> f64 {
        let y = self.y;
        self.y += ROW_H;
        y
    }

    fn marker(&mut self, glyph: &str, text: String) {
        let y = self.row();
        let x = self.spine_x + 4.0;
        self.f
            .text(pt(x, y + 12.0), glyph, Anchor::Start, TextSize::Normal);
        self.f.text(
            pt(x + 14.0, y + 12.0),
            elide(&text, SPINE_CHARS),
            Anchor::Start,
            TextSize::Small,
        );
    }

    fn static_style(e: &CExpr) -> BoxStyle {
        match e {
            CExpr::Num(_) => BoxStyle::Solid,
            _ => BoxStyle::Admission,
        }
    }

    fn walk(&mut self, block: usize) {
        let Some(stmts) = self.p.blocks.get(block) else {
            return;
        };
        for s in stmts.clone() {
            self.stmt(&s);
        }
    }

    fn stmt(&mut self, s: &CStmt) {
        match s {
            CStmt::Turn => self.marker("*", "turn".into()),
            CStmt::Observe(k, e) => {
                let name = self.p.observes.get(*k).map_or("?", String::as_str);
                self.marker("o", format!("observe {name} = {}", self.p.show_expr(e)));
            }
            CStmt::Set(slot, e) => {
                if self.show_set {
                    let name = self.p.attrs.get(*slot).map_or("?", String::as_str);
                    self.marker(" ", format!("set {name} = {}", self.p.show_expr(e)));
                }
            }
            CStmt::Choose { var, count, key } => {
                let name = self.p.attrs.get(*var).map_or("?", String::as_str);
                self.marker(
                    "<",
                    format!(
                        "choose {name} in {} by ({})",
                        self.p.show_expr(count),
                        self.p.show_expr(key)
                    ),
                );
            }
            CStmt::End => self.marker("#", "end".into()),
            CStmt::Drop(r) => {
                let y = self.row();
                let name = self.p.show_pool_ref(r);
                self.f.text(
                    pt(self.spine_x + 4.0, y + 12.0),
                    "x",
                    Anchor::Start,
                    TextSize::Normal,
                );
                self.f.text(
                    pt(self.spine_x + 18.0, y + 12.0),
                    format!("drop {name}"),
                    Anchor::Start,
                    TextSize::Small,
                );
                if let Some(x) = self.col_x(r.base) {
                    self.open_tails.retain(|(pool, _)| *pool != r.base);
                    self.f.push(Item::Edge {
                        pts: vec![pt(x, y + 8.0), pt(x + COL_W, y + 8.0)],
                        style: EdgeStyle::Relation,
                        arrow: false,
                    });
                }
            }
            CStmt::Grow(r, d) => {
                self.marker(
                    "^",
                    format!("grow {} ({})", self.p.show_pool_ref(r), self.p.show_expr(d)),
                );
            }
            CStmt::Load(r, n) => {
                self.marker(
                    "+",
                    format!("load {} ({})", self.p.show_pool_ref(r), self.p.show_expr(n)),
                );
            }
            CStmt::Release(r) => {
                let y = self.y;
                self.marker("]", format!("release {}", self.p.show_pool_ref(r)));
                self.released.push((r.base, y, self.depth));
            }
            CStmt::Run {
                stage,
                mode,
                work,
                growing,
            } => {
                let y = self.row();
                let label = match mode {
                    RunMode::Plain => format!("run {}", self.p.show_stage_ref(stage)),
                    RunMode::Prefill => format!("run {} prefill", self.p.show_stage_ref(stage)),
                    RunMode::Decode => format!("run {} decode", self.p.show_stage_ref(stage)),
                };
                let r = Rect::new(self.spine_x, y + 2.0, SPINE_W - 6.0, ROW_H - 6.0);
                self.f.boxed(r, BoxStyle::Body, 2.0);
                self.f.text(
                    pt(r.x + 6.0, y + 15.0),
                    label,
                    Anchor::Start,
                    TextSize::Small,
                );
                self.f.note(
                    pt(r.x + 132.0, y + 15.0),
                    elide(&format!("({})", self.p.show_expr(work)), 30),
                    Anchor::Start,
                );
                if let Some(g) = growing
                    && let Some(x) = self.col_x(g.base)
                {
                    // the hold widens as the run advances
                    let (x0, x1) = (x + BAND_INSET, x + COL_W - BAND_INSET);
                    self.under.push(Item::Poly {
                        pts: vec![
                            pt(x0, y + 2.0),
                            pt(x0 + (x1 - x0) * 0.45, y + 2.0),
                            pt(x1, y + ROW_H - 4.0),
                            pt(x0, y + ROW_H - 4.0),
                        ],
                        style: BoxStyle::Body,
                    });
                    self.under_note(pt(x0 + 5.0, y + 17.0), "growing".into());
                }
            }
            CStmt::Hold {
                pools,
                reuse,
                body,
                cache,
            } => {
                let top = self.y;
                let depth = self.depth;
                if let Some(rho) = reuse {
                    self.marker("v", format!("reuse ({})", self.p.show_expr(rho)));
                }
                let names: Vec<String> = pools
                    .iter()
                    .map(|(r, u, _)| {
                        format!("{} ({})", self.p.show_pool_ref(r), self.p.show_expr(u))
                    })
                    .collect();
                self.marker("[", format!("hold {}", names.join(", ")));
                self.walk(*body);
                let bottom = self.y;
                self.marker("]", "release".into());
                let label_chars = (COL_W / TextSize::Small.char_width()) as usize - 2;
                let targets = self.cache_targets.get(body).cloned().unwrap_or_default();
                for (r, units, reserve) in pools {
                    let Some(x) = self.col_x(r.base) else {
                        continue;
                    };
                    // a `release` in the body, on every path, ended this
                    // pool's hold early; one inside a branch is a marker only
                    let end = self
                        .released
                        .iter()
                        .rposition(|&(q, y, d)| q == r.base && d == depth && y > top && y <= bottom)
                        .map(|i| self.released[i].1);
                    self.released
                        .retain(|&(q, y, _)| !(q == r.base && y > top && y <= bottom));
                    let bottom = end.unwrap_or(bottom);
                    if let Some(fx) = reserve {
                        let fr = Rect::new(x + 3.0, top - 3.0, COL_W - 6.0, bottom - top + 6.0);
                        self.band_box(fr, BoxStyle::Reserve, 2.0);
                        let t = elide(&format!("reserve ({})", self.p.show_expr(fx)), label_chars);
                        self.under_note(pt(fr.x + 2.0, fr.y - 4.0), t);
                    }
                    let band =
                        Rect::new(x + BAND_INSET, top, COL_W - 2.0 * BAND_INSET, bottom - top);
                    self.band_box(band, Self::static_style(units), 2.0);
                    let t = elide(&self.p.show_expr(units), label_chars);
                    self.under_note(pt(band.x + 4.0, top + 13.0), t);
                    // A hold with a `growing` run leaves units cached in that
                    // pool alone; one without leaves them in all of its pools.
                    if let Some(c) = cache
                        && targets.contains(&r.base)
                    {
                        let tail = Rect::new(band.x, bottom, band.w, CACHE_H);
                        self.band_box(tail, BoxStyle::Cached, 1.0);
                        let t = elide(&format!("cache ({})", self.p.show_expr(c)), label_chars);
                        self.under_note(pt(tail.x + 4.0, bottom + CACHE_H + 10.0), t);
                        self.open_tails.retain(|(pool, _)| *pool != r.base);
                        self.open_tails.push((r.base, bottom));
                    }
                }
            }
            CStmt::Branch(c, t, e) => {
                let x = self.rail_x();
                let top = self.y;
                self.marker("?", format!("branch ({})", self.p.show_guard(c)));
                self.depth += 1;
                self.walk(*t);
                self.depth -= 1;
                if !self.p.blocks.get(*e).map(Vec::is_empty).unwrap_or(true) {
                    self.marker(":", "else".into());
                    self.depth += 1;
                    self.walk(*e);
                    self.depth -= 1;
                }
                let bottom = self.y;
                self.f.push(Item::Edge {
                    pts: vec![pt(x + 4.0, top + 16.0), pt(x + 4.0, bottom - 6.0)],
                    style: EdgeStyle::Relation,
                    arrow: false,
                });
            }
            CStmt::Loop(body) => {
                let x = self.rail_x();
                let top = self.y;
                self.marker("@", "loop".into());
                self.depth += 1;
                self.walk(*body);
                self.depth -= 1;
                let bottom = self.y;
                // the bracket, and the back edge that carries the tails with it
                // One rail, drawn in the direction the session travels: out
                // at the bottom, up the left, back in at the top.
                self.f.push(Item::Edge {
                    pts: vec![
                        pt(x + 8.0, bottom + 4.0),
                        pt(x + 2.0, bottom + 4.0),
                        pt(x + 2.0, top + 10.0),
                        pt(x + 8.0, top + 10.0),
                    ],
                    style: EdgeStyle::Back,
                    arrow: true,
                });
                self.f.note(
                    pt(x + 12.0, bottom + 16.0),
                    "back to the top of the turn",
                    Anchor::Start,
                );
                self.y = bottom + ROW_H;
            }
        }
    }
}

/// The session figure of a program. `show_set` includes `set` statements, which
/// are computation rather than resource movement and are off by default.
pub fn figure(p: &Program, show_set: bool) -> Figure {
    let cols = held_pools(p);
    let mut d = Draw {
        p,
        f: Figure::default(),
        bands: vec![],
        under: vec![],
        cols: cols.clone(),
        spine_x: MARGIN + (control_depth(p, p.session) + 1) as f64 * RAIL_W,
        y: MARGIN + HEAD_H,
        depth: 0,
        show_set,
        open_tails: vec![],
        released: vec![],
        cache_targets: crate::deployment::cache_targets(p),
    };

    for (i, &pool) in cols.iter().enumerate() {
        let x = d.spine_x + SPINE_W + COL_GAP + i as f64 * (COL_W + COL_GAP);
        let name = &p.pools[pool].name;
        d.f.text(
            pt(x, MARGIN + 14.0),
            format!("pool {name}"),
            Anchor::Start,
            TextSize::Normal,
        );
        d.f.note(
            pt(x, MARGIN + 28.0),
            format!("cap {}", crate::ir::show_num(p.pools[pool].cap)),
            Anchor::Start,
        );
        d.f.push(Item::Edge {
            pts: vec![pt(x, MARGIN + 34.0), pt(x + COL_W, MARGIN + 34.0)],
            style: EdgeStyle::Relation,
            arrow: false,
        });
    }
    d.f.text(
        pt(MARGIN, MARGIN + 14.0),
        "session",
        Anchor::Start,
        TextSize::Normal,
    );

    d.walk(p.session);

    // Tails that no `drop` closed run past the end of the session.
    let bottom = d.y + 8.0;
    let tails = std::mem::take(&mut d.open_tails);
    for (pool, from) in tails {
        if let Some(x) = d.col_x(pool)
            && bottom > from + CACHE_H
        {
            d.band_box(
                Rect::new(
                    x + BAND_INSET,
                    from + CACHE_H,
                    COL_W - 2.0 * BAND_INSET,
                    bottom - from - CACHE_H,
                ),
                BoxStyle::Cached,
                1.0,
            );
        }
    }

    // The columns go under the spine, so that a band never hides a run box.
    let mut items = d.bands;
    items.extend(d.under);
    items.extend(d.f.items);
    let mut f = Figure {
        items,
        ..Figure::default()
    };
    f.fit(MARGIN);
    f
}

/// The pools a session figure gives a column to, for tests.
pub fn columns(p: &Program) -> Vec<usize> {
    held_pools(p)
}
