//! A `Figure` as a standalone SVG file.
//!
//! The writer decides no coordinate; it only paints what the layout placed.
//! Colours are given for both colour schemes so that a figure dropped into a
//! dark page stays legible, and the font is monospaced so that
//! `TextSize::char_width` is the truth rather than an estimate.

use std::fmt::Write as _;

use crate::view::figure::{Anchor, BoxStyle, EdgeStyle, Figure, Item, StationKind};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn n(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

fn box_class(s: BoxStyle) -> &'static str {
    match s {
        BoxStyle::Solid => "solid",
        BoxStyle::Cached => "cached",
        BoxStyle::Enclosure => "enclosure",
        BoxStyle::Flow => "rail",
        BoxStyle::Instance => "instance",
        BoxStyle::Frame => "frame",
    }
}

fn edge_class(s: EdgeStyle) -> &'static str {
    match s {
        EdgeStyle::Flow => "flow",
        EdgeStyle::Back => "back",
        EdgeStyle::Relation => "relation",
    }
}

/// One painted role: the attributes it gets in light mode, and the overrides
/// a dark viewer applies.
///
/// The colours are written as **presentation attributes**, not as CSS custom
/// properties, because `librsvg`, `cairosvg` and every other non-browser
/// renderer ignores `var()` and paints the result black. CSS beats a
/// presentation attribute, so the media query below still wins in a browser
/// and the file converts correctly everywhere else.
struct Paint {
    class: &'static str,
    light: &'static str,
    dark: &'static str,
}

const INK: &str = "#1d2021";
const DIM: &str = "#6b7280";

const PAINTS: &[Paint] = &[
    Paint {
        class: "bg",
        light: "fill=\"#ffffff\"",
        dark: "fill:#14171a",
    },
    Paint {
        class: "solid",
        light: "fill=\"#eef2f5\" stroke=\"#4b5563\" stroke-width=\"1.1\"",
        dark: "fill:#232a31;stroke:#b6bfcc",
    },
    Paint {
        class: "cached",
        light: "fill=\"#e6e2da\" stroke=\"none\"",
        dark: "fill:#33302a",
    },
    Paint {
        class: "instance",
        light: "fill=\"#f5f7f9\" stroke=\"#4b5563\" stroke-width=\"1.2\"",
        dark: "fill:#1a1f24;stroke:#b6bfcc",
    },
    Paint {
        class: "frame",
        light: "fill=\"none\" stroke=\"#4b5563\" stroke-width=\"1.1\"",
        dark: "stroke:#b6bfcc",
    },
    Paint {
        class: "enclosure",
        light: "fill=\"none\" stroke=\"#6b7280\" stroke-width=\"1\" stroke-dasharray=\"6 4\"",
        dark: "stroke:#9aa4b2",
    },
    Paint {
        class: "station",
        light: "fill=\"#d8e6e4\" stroke=\"#4b5563\" stroke-width=\"1.3\"",
        dark: "fill:#26403d;stroke:#b6bfcc",
    },
    Paint {
        class: "cell",
        light: "fill=\"#ffffff\" stroke=\"#4b5563\" stroke-width=\".8\"",
        dark: "fill:#14171a;stroke:#b6bfcc",
    },
    Paint {
        class: "rail",
        light: "fill=\"none\" stroke=\"#4b5563\" stroke-width=\"1\"",
        dark: "stroke:#b6bfcc",
    },
    Paint {
        class: "flow",
        light: "fill=\"none\" stroke=\"#4b5563\" stroke-width=\"1.3\"",
        dark: "stroke:#b6bfcc",
    },
    Paint {
        class: "back",
        light: "fill=\"none\" stroke=\"#4b5563\" stroke-width=\"1.1\" stroke-dasharray=\"4 3\"",
        dark: "stroke:#b6bfcc",
    },
    Paint {
        class: "relation",
        light: "fill=\"none\" stroke=\"#6b7280\" stroke-width=\"1\" stroke-dasharray=\"2 3\"",
        dark: "stroke:#9aa4b2",
    },
];

/// `class="..."` plus the light-mode presentation attributes for a role.
fn paint(class: &str) -> String {
    let p = PAINTS
        .iter()
        .find(|p| p.class == class)
        .expect("a known paint role");
    format!(r#"class="{class}" {}"#, p.light)
}

/// The dark-mode overrides, and the font every label is set in.
fn stylesheet() -> String {
    let mut s = String::from(
        "\n  text { font-family: ui-monospace, \"SF Mono\", Menlo, Consolas, monospace; }\n",
    );
    let _ = writeln!(s, "  @media (prefers-color-scheme: dark) {{");
    let _ = writeln!(
        s,
        "    text {{ fill: #e8e6e3; }} text.dim {{ fill: #9aa4b2; }}"
    );
    for p in PAINTS {
        let _ = writeln!(s, "    .{} {{ {} }}", p.class, p.dark);
    }
    s.push_str("  }\n");
    s
}

/// Render a figure as a complete SVG document.
pub fn render(f: &Figure) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img">"#,
        w = n(f.width),
        h = n(f.height)
    );
    if !f.title.is_empty() {
        let _ = writeln!(s, "<title>{}</title>", esc(&f.title));
    }
    let _ = writeln!(s, "<style>{}</style>", stylesheet());
    let _ = writeln!(
        s,
        r##"<defs><marker id="a" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="context-stroke"/></marker></defs>"##
    );
    let _ = writeln!(
        s,
        r#"<rect {} x="0" y="0" width="{}" height="{}"/>"#,
        paint("bg"),
        n(f.width),
        n(f.height)
    );

    for item in &f.items {
        match item {
            Item::Box { rect, style, round } => {
                let _ = writeln!(
                    s,
                    r#"<rect {} x="{}" y="{}" width="{}" height="{}" rx="{}"/>"#,
                    paint(box_class(*style)),
                    n(rect.x),
                    n(rect.y),
                    n(rect.w),
                    n(rect.h),
                    n(*round)
                );
            }
            Item::Station { rect, kind, text } => {
                let c = rect.centre();
                match kind {
                    StationKind::Fifo | StationKind::Ps => {
                        let r = rect.h.min(rect.w) / 2.0;
                        let _ = writeln!(
                            s,
                            r#"<circle {} cx="{}" cy="{}" r="{}"/>"#,
                            paint("station"),
                            n(c.x),
                            n(c.y),
                            n(r)
                        );
                    }
                    StationKind::Delay => {
                        let _ = writeln!(
                            s,
                            r#"<rect {} x="{}" y="{}" width="{}" height="{}" rx="8"/>"#,
                            paint("station"),
                            n(rect.x),
                            n(rect.y),
                            n(rect.w),
                            n(rect.h)
                        );
                        for k in 0..5 {
                            let x = rect.x + rect.w * (k as f64 + 1.0) / 6.0;
                            let _ = writeln!(
                                s,
                                r#"<circle {} cx="{}" cy="{}" r="4"/>"#,
                                paint("cell"),
                                n(x),
                                n(c.y)
                            );
                        }
                    }
                    StationKind::Decision => {
                        let r = rect.h.min(rect.w) / 2.0;
                        let _ = writeln!(
                            s,
                            r#"<polygon {} points="{},{} {},{} {},{} {},{}"/>"#,
                            paint("station"),
                            n(c.x),
                            n(c.y - r),
                            n(c.x + r),
                            n(c.y),
                            n(c.x),
                            n(c.y + r),
                            n(c.x - r),
                            n(c.y)
                        );
                    }
                    StationKind::Step => {
                        let _ = writeln!(
                            s,
                            r#"<rect {} x="{}" y="{}" width="{}" height="{}" rx="3"/>"#,
                            paint("station"),
                            n(rect.x),
                            n(rect.y),
                            n(rect.w),
                            n(rect.h)
                        );
                        // the budget bar: an iteration's tokens, filling left to right
                        let bar = crate::view::figure::Rect::new(
                            rect.x + 8.0,
                            rect.bottom() - 13.0,
                            rect.w - 16.0,
                            6.0,
                        );
                        let _ = writeln!(
                            s,
                            r#"<rect {} x="{}" y="{}" width="{}" height="{}"/>"#,
                            paint("cell"),
                            n(bar.x),
                            n(bar.y),
                            n(bar.w),
                            n(bar.h)
                        );
                        let _ = writeln!(
                            s,
                            r#"<rect {} x="{}" y="{}" width="{}" height="{}"/>"#,
                            paint("solid"),
                            n(bar.x),
                            n(bar.y),
                            n(bar.w * 0.62),
                            n(bar.h)
                        );
                    }
                }
                if !text.is_empty() {
                    let dy = if *kind == StationKind::Step {
                        -4.0
                    } else {
                        3.5
                    };
                    let _ = writeln!(
                        s,
                        r#"<text x="{}" y="{}" text-anchor="middle" font-size="10" fill="{INK}">{}</text>"#,
                        n(c.x),
                        n(c.y + dy),
                        esc(text)
                    );
                }
            }
            Item::Queue { rect, cells } => {
                let _ = writeln!(
                    s,
                    r#"<path {} d="M{} {} L{} {} M{} {} L{} {}"/>"#,
                    paint("rail"),
                    n(rect.x),
                    n(rect.y),
                    n(rect.right()),
                    n(rect.y),
                    n(rect.x),
                    n(rect.bottom()),
                    n(rect.right()),
                    n(rect.bottom())
                );
                let cw = rect.w / (*cells as f64 + 0.5);
                for k in 0..*cells {
                    let x = rect.x + cw * (k as f64 + 0.25);
                    let _ = writeln!(
                        s,
                        r#"<rect {} x="{}" y="{}" width="{}" height="{}"/>"#,
                        paint("cell"),
                        n(x),
                        n(rect.y + 2.0),
                        n(cw * 0.7),
                        n(rect.h - 4.0)
                    );
                }
            }
            Item::Slots { rect, cols, rows } => {
                let cw = rect.w / *cols as f64;
                let ch = rect.h / *rows as f64;
                for r in 0..*rows {
                    for c in 0..*cols {
                        let _ = writeln!(
                            s,
                            r#"<rect {} x="{}" y="{}" width="{}" height="{}"/>"#,
                            paint("cell"),
                            n(rect.x + cw * c as f64),
                            n(rect.y + ch * r as f64),
                            n(cw * 0.82),
                            n(ch * 0.78)
                        );
                    }
                }
            }
            Item::Edge { pts, style, arrow } => {
                let mut d = String::new();
                for (i, p) in pts.iter().enumerate() {
                    let _ = write!(
                        d,
                        "{}{} {}",
                        if i == 0 { "M" } else { " L" },
                        n(p.x),
                        n(p.y)
                    );
                }
                let m = if *arrow {
                    r#" marker-end="url(#a)""#
                } else {
                    ""
                };
                let _ = writeln!(s, r#"<path {} d="{d}"{m}/>"#, paint(edge_class(*style)));
            }
            Item::Text {
                at,
                text,
                anchor,
                size,
                dim,
            } => {
                let a = match anchor {
                    Anchor::Start => "start",
                    Anchor::Middle => "middle",
                    Anchor::End => "end",
                };
                let (class, fill) = if *dim {
                    (r#" class="dim""#, DIM)
                } else {
                    ("", INK)
                };
                let _ = writeln!(
                    s,
                    r#"<text x="{}" y="{}" text-anchor="{a}" font-size="{}" fill="{fill}"{class}>{}</text>"#,
                    n(at.x),
                    n(at.y),
                    n(size.points()),
                    esc(text)
                );
            }
        }
    }
    s.push_str("</svg>\n");
    s
}
