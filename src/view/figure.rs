//! The geometry a view produces and a writer consumes.
//!
//! Both views (`session` for the session program, `deployment` for the queueing
//! network) produce a `Figure`; both writers (`svg`, `tikz`) consume one.
//! Nothing in here knows about seQ, and nothing in a writer decides a
//! coordinate: a figure is the test surface, which is why the tests assert on
//! rectangles rather than on bytes.
//!
//! Coordinates are points, x to the right and y **downwards**, origin at the
//! figure's top left. The TikZ writer flips y on the way out.

/// A point in figure coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub const fn pt(x: f64, y: f64) -> Point {
    Point { x, y }
}

/// An axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn centre(&self) -> Point {
        pt(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    /// Grow by `d` on every side.
    pub fn pad(&self, d: f64) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
    /// The smallest rectangle containing both.
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }
    pub fn contains(&self, o: &Rect) -> bool {
        self.x <= o.x && self.y <= o.y && self.right() >= o.right() && self.bottom() >= o.bottom()
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// How a box is painted, and what that says about the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxStyle {
    /// A width fixed by the program's constants.
    Solid,
    /// A width decided when the session is admitted.
    Admission,
    /// Units that stay cached after the scope ends.
    Cached,
    /// `reserve (r)`: what must be free for the admission.
    Reserve,
    /// An enclosure: a pool's instance boundary.
    Enclosure,
    /// A filled body: a run box, a station's rectangle.
    Body,
}

/// How a connector is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    /// The path a session takes.
    Flow,
    /// A loop's back edge, or a feedback path.
    Back,
    /// A relation that is not a path: `admit via`, `memory`.
    Relation,
    /// A run growing a hold.
    Grow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// Text size classes, in points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    Title,
    Normal,
    Small,
}

impl TextSize {
    pub fn points(self) -> f64 {
        match self {
            TextSize::Title => 13.0,
            TextSize::Normal => 10.0,
            TextSize::Small => 8.0,
        }
    }
    /// Width of one character, for layout. The writers set a monospaced
    /// family so that this estimate is the truth rather than a guess.
    pub fn char_width(self) -> f64 {
        self.points() * 0.6
    }
    pub fn width_of(self, s: &str) -> f64 {
        s.chars().count() as f64 * self.char_width()
    }
}

/// The shape a stage is drawn as, which is its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StationKind {
    /// `fifo(c)`: a circle.
    Fifo,
    /// `ps(phi)`: a circle.
    Ps,
    /// `delay`: infinitely many servers.
    Delay,
    /// `step`: an iterating engine. The lecture's notation has no glyph for
    /// this one; it is a budget bar rather than a queueing station.
    Step,
}

/// One drawable.
#[derive(Clone, Debug)]
pub enum Item {
    Box {
        rect: Rect,
        style: BoxStyle,
        /// Corner radius in points; 0 for square corners.
        round: f64,
    },
    /// A band that changes width: a `growing` run.
    Poly { pts: Vec<Point>, style: BoxStyle },
    /// A queueing station.
    Station {
        rect: Rect,
        kind: StationKind,
        /// `FIFO`, `PS`, the budget for a step engine.
        text: String,
    },
    /// The queueing-theory queue glyph: `cells` boxes between two rails.
    Queue { rect: Rect, cells: usize },
    /// A pool's capacity as a grid of units.
    Slots {
        rect: Rect,
        cols: usize,
        rows: usize,
    },
    Edge {
        pts: Vec<Point>,
        style: EdgeStyle,
        arrow: bool,
    },
    Text {
        at: Point,
        text: String,
        anchor: Anchor,
        size: TextSize,
        /// Drawn in a lighter colour: a note rather than a name.
        dim: bool,
    },
}

/// A laid-out figure.
#[derive(Clone, Debug, Default)]
pub struct Figure {
    pub width: f64,
    pub height: f64,
    pub title: String,
    pub items: Vec<Item>,
}

impl Figure {
    pub fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    pub fn text(&mut self, at: Point, text: impl Into<String>, anchor: Anchor, size: TextSize) {
        self.push(Item::Text {
            at,
            text: text.into(),
            anchor,
            size,
            dim: false,
        });
    }

    pub fn note(&mut self, at: Point, text: impl Into<String>, anchor: Anchor) {
        self.push(Item::Text {
            at,
            text: text.into(),
            anchor,
            size: TextSize::Small,
            dim: true,
        });
    }

    pub fn boxed(&mut self, rect: Rect, style: BoxStyle, round: f64) {
        self.push(Item::Box { rect, style, round });
    }

    pub fn edge(&mut self, pts: Vec<Point>, style: EdgeStyle) {
        self.push(Item::Edge {
            pts,
            style,
            arrow: true,
        });
    }

    /// Every box of a given style, for tests.
    pub fn boxes(&self, style: BoxStyle) -> Vec<Rect> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Box { rect, style: s, .. } if *s == style => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// Every station, for tests.
    pub fn stations(&self) -> Vec<(Rect, StationKind)> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Station { rect, kind, .. } => Some((*rect, *kind)),
                _ => None,
            })
            .collect()
    }

    /// Set `width`/`height` from the extent of the items, plus a margin.
    pub fn fit(&mut self, margin: f64) {
        let mut max = pt(0.0, 0.0);
        let mut visit = |p: Point| {
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        };
        for i in &self.items {
            match i {
                Item::Box { rect, .. }
                | Item::Station { rect, .. }
                | Item::Queue { rect, .. }
                | Item::Slots { rect, .. } => visit(pt(rect.right(), rect.bottom())),
                Item::Poly { pts, .. } | Item::Edge { pts, .. } => {
                    pts.iter().for_each(|p| visit(*p))
                }
                Item::Text {
                    at,
                    text,
                    size,
                    anchor,
                    ..
                } => {
                    let w = size.width_of(text);
                    let right = match anchor {
                        Anchor::Start => at.x + w,
                        Anchor::Middle => at.x + w / 2.0,
                        Anchor::End => at.x,
                    };
                    visit(pt(right, at.y + size.points()));
                }
            }
        }
        self.width = max.x + margin;
        self.height = max.y + margin;
    }
}
