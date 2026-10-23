//! A `Figure` as a standalone SVG file.
//!
//! The writer decides no coordinate; it only paints what the layout placed.
//! Colours are given for both colour schemes so that a figure dropped into a
//! dark page stays legible, and the font is monospaced so that
//! `TextSize::char_width` is the truth rather than an estimate.

use std::fmt::Write as _;

use crate::view::figure::{
    Anchor, BoxStyle, CLIENT, EdgeRole, EdgeStyle, Figure, Item, Mark, StationKind,
};

/// Text or an attribute value, escaped.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
        BoxStyle::Server => "server",
        BoxStyle::Prefill => "prefill",
        BoxStyle::Decode => "decode",
        BoxStyle::Used => "used",
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
        class: "server",
        light: "fill=\"none\" stroke=\"#4b5563\" stroke-width=\"1.4\"",
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
        class: "poster",
        light: "fill=\"#4b5563\" stroke=\"none\"",
        dark: "fill:#b6bfcc",
    },
    Paint {
        class: "relation",
        light: "fill=\"none\" stroke=\"#6b7280\" stroke-width=\"1\" stroke-dasharray=\"2 3\"",
        dark: "stroke:#9aa4b2",
    },
];

/// The roles only a run's figure paints (the iteration view): a request's
/// tokens by its mode, in serq-lar's colours for a prefill and a decode so
/// that its replay and the figure agree; a pool's allocated units; the
/// marks of an event. Their dark overrides are written only in a figure
/// that has one, so that a deployment figure's file stays as it was.
const RUN_PAINTS: &[Paint] = &[
    Paint {
        class: "prefill",
        light: "fill=\"#e8782a\" stroke=\"none\"",
        dark: "fill:#f39248",
    },
    Paint {
        class: "decode",
        light: "fill=\"#2f6fde\" stroke=\"none\"",
        dark: "fill:#5b8ff0",
    },
    Paint {
        class: "used",
        light: "fill=\"#8a94a3\" stroke=\"none\"",
        dark: "fill:#6b7585",
    },
    Paint {
        class: "preempt",
        light: "fill=\"none\" stroke=\"#d6453d\" stroke-width=\"1.4\"",
        dark: "stroke:#f0655c",
    },
    Paint {
        class: "evict",
        light: "fill=\"#d6453d\" stroke=\"none\"",
        dark: "fill:#f0655c",
    },
];

/// `class="..."` plus the light-mode presentation attributes for a role.
fn paint(class: &str) -> String {
    let p = PAINTS
        .iter()
        .chain(RUN_PAINTS)
        .find(|p| p.class == class)
        .expect("a known paint role");
    format!(r#"class="{class}" {}"#, p.light)
}

/// The dark-mode overrides, and the font every label is set in.
fn stylesheet(f: &Figure) -> String {
    let mut s = String::from(
        "\n  text { font-family: ui-monospace, \"SF Mono\", Menlo, Consolas, monospace; }\n",
    );
    let _ = writeln!(s, "  @media (prefers-color-scheme: dark) {{");
    let _ = writeln!(
        s,
        "    text {{ fill: #e8e6e3; }} text.dim {{ fill: #9aa4b2; }}"
    );
    let run: &[Paint] = if f.of_a_run() { RUN_PAINTS } else { &[] };
    for p in PAINTS.iter().chain(run) {
        let _ = writeln!(s, "    .{} {{ {} }}", p.class, p.dark);
    }
    s.push_str("  }\n");
    s
}

/// Render a figure as a complete SVG document.
/// A copy's `data-copy` (`pull` or `push`) and `data-poster` (its node).
fn copy_tag(copy: &Option<crate::view::figure::CopyMark>) -> String {
    match copy {
        Some(c) => format!(
            r#" data-copy="{}" data-poster="{}""#,
            c.word(),
            esc(&c.poster)
        ),
        None => String::new(),
    }
}

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
    let _ = writeln!(s, "<style>{}</style>", stylesheet(f));
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
            Item::Box {
                rect,
                style,
                round,
                nodes,
                copy,
            } => {
                // a bracket names the nodes one job holds at once, which no
                // arrow joins: a page walking a request treats them as one
                // place; an instance's panel names the stations in it
                let tag = if *style == BoxStyle::Server {
                    r#" data-role="server""#.to_string()
                } else if nodes.is_empty() {
                    String::new()
                } else {
                    let role = match style {
                        BoxStyle::Flow => "bracket",
                        BoxStyle::Instance => "instance",
                        BoxStyle::Solid
                        | BoxStyle::Cached
                        | BoxStyle::Enclosure
                        | BoxStyle::Frame
                        | BoxStyle::Server
                        | BoxStyle::Prefill
                        | BoxStyle::Decode
                        | BoxStyle::Used => {
                            unreachable!("only a bracket or a panel names nodes")
                        }
                    };
                    format!(
                        r#" data-role="{role}" data-nodes="{}"{}"#,
                        esc(&nodes.join(" ")),
                        copy_tag(copy)
                    )
                };
                let _ = writeln!(
                    s,
                    r#"<rect {}{tag} x="{}" y="{}" width="{}" height="{}" rx="{}"/>"#,
                    paint(box_class(*style)),
                    n(rect.x),
                    n(rect.y),
                    n(rect.w),
                    n(rect.h),
                    n(*round)
                );
            }
            Item::Station {
                rect,
                kind,
                text,
                outline,
                stage,
                node,
            } => {
                let c = rect.centre();
                // the station's shapes in a group that names it as the arrows
                // do and, when it is a stage, by what the program calls it, so
                // that a page can find the station (and, in an engine, its
                // budget bar)
                match stage {
                    Some(name) => {
                        let _ = writeln!(
                            s,
                            r#"<g data-node="{}" data-stage="{}">"#,
                            esc(node),
                            esc(name)
                        );
                    }
                    // a node that is no stage says what it is
                    None => {
                        let role = match kind {
                            StationKind::Decision => r#" data-role="decision""#,
                            StationKind::Delay => r#" data-role="tick""#,
                            StationKind::Gateway => r#" data-role="gateway""#,
                            StationKind::Client => r#" data-role="client""#,
                            // a queueing station is a stage's: one drawn
                            // without is named alone
                            StationKind::Fifo | StationKind::Ps | StationKind::Step => "",
                        };
                        let _ = writeln!(s, r#"<g data-node="{}"{role}>"#, esc(node));
                    }
                }
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
                        if let Some(pts) = outline.map(|o| crate::view::figure::density(o, *rect)) {
                            let d: Vec<String> = pts
                                .iter()
                                .map(|q| format!("{} {}", n(q.x), n(q.y)))
                                .collect();
                            let _ =
                                writeln!(s, r#"<path {} d="M {}"/>"#, paint("rail"), d.join(" L "));
                        } else {
                            // a duration that is no distribution: infinitely
                            // many servers, as the lecture draws a delay
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
                    }
                    StationKind::Client => {
                        let _ = writeln!(
                            s,
                            r#"<rect {} x="{}" y="{}" width="{}" height="{}" rx="12"/>"#,
                            paint("station"),
                            n(rect.x),
                            n(rect.y),
                            n(rect.w),
                            n(rect.h)
                        );
                    }
                    StationKind::Gateway => {
                        let pts: Vec<String> = crate::view::figure::hexagon(*rect)
                            .iter()
                            .map(|q| format!("{},{}", n(q.x), n(q.y)))
                            .collect();
                        let _ = writeln!(
                            s,
                            r#"<polygon {} points="{}"/>"#,
                            paint("station"),
                            pts.join(" ")
                        );
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
                            r#"<rect {} data-role="budget" x="{}" y="{}" width="{}" height="{}"/>"#,
                            paint("solid"),
                            n(bar.x),
                            n(bar.y),
                            n(bar.w * 0.62),
                            n(bar.h)
                        );
                    }
                }
                if !text.is_empty() {
                    // under the budget bar, or the density
                    let dy = match kind {
                        StationKind::Step => -4.0,
                        StationKind::Delay => 18.0,
                        _ => 3.5,
                    };
                    let _ = writeln!(
                        s,
                        r#"<text x="{}" y="{}" text-anchor="middle" font-size="10" fill="{INK}">{}</text>"#,
                        n(c.x),
                        n(c.y + dy),
                        esc(text)
                    );
                }
                s.push_str("</g>\n");
            }
            Item::Queue { rect, cells, pool } => {
                let _ = writeln!(s, r#"<g data-pool="{}" data-role="queue">"#, esc(pool));
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
                s.push_str("</g>\n");
            }
            Item::Drum { rect, pool } => {
                let _ = writeln!(s, r#"<g data-pool="{}" data-role="drum">"#, esc(pool));
                // the side and the near half of the bottom, then the lid
                let (rx, ry) = (rect.w / 2.0, (rect.h / 5.0).min(4.0));
                let _ = writeln!(
                    s,
                    r#"<path {} d="M {} {} L {} {} A {} {} 0 0 0 {} {} L {} {} Z"/>"#,
                    paint("cell"),
                    n(rect.x),
                    n(rect.y + ry),
                    n(rect.x),
                    n(rect.bottom() - ry),
                    n(rx),
                    n(ry),
                    n(rect.right()),
                    n(rect.bottom() - ry),
                    n(rect.right()),
                    n(rect.y + ry)
                );
                let _ = writeln!(
                    s,
                    r#"<ellipse {} cx="{}" cy="{}" rx="{}" ry="{}"/>"#,
                    paint("cell"),
                    n(rect.centre().x),
                    n(rect.y + ry),
                    n(rx),
                    n(ry)
                );
                s.push_str("</g>\n");
            }
            Item::Edge {
                pts,
                style,
                arrow,
                role,
                copy,
            } => {
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
                let r = match role {
                    Some(EdgeRole::Arrival { to }) => format!(
                        r#" data-role="arrival" data-from="{CLIENT}" data-to="{}""#,
                        esc(to)
                    ),
                    Some(EdgeRole::Exit { from }) => format!(
                        r#" data-role="exit" data-from="{}" data-to="{CLIENT}""#,
                        esc(from)
                    ),
                    Some(EdgeRole::Flow { from, to }) => format!(
                        r#" data-role="flow" data-from="{}" data-to="{}""#,
                        esc(from),
                        esc(to)
                    ),
                    Some(EdgeRole::Reply { from, to }) => format!(
                        r#" data-role="reply" data-from="{}" data-to="{}""#,
                        esc(from),
                        esc(to)
                    ),
                    None => String::new(),
                };
                let c = copy_tag(copy);
                let _ = writeln!(
                    s,
                    r#"<path {}{r}{c} d="{d}"{m}/>"#,
                    paint(edge_class(*style))
                );
            }
            Item::Dot { at, r } => {
                let _ = writeln!(
                    s,
                    r#"<circle {} cx="{}" cy="{}" r="{}"/>"#,
                    paint("poster"),
                    n(at.x),
                    n(at.y),
                    n(*r)
                );
            }
            Item::Mark { at, size, mark } => {
                let h = size / 2.0;
                let (class, d) = match mark {
                    // a cross over the victim's cell
                    Mark::Preempt => (
                        "preempt",
                        format!(
                            "M {} {} L {} {} M {} {} L {} {}",
                            n(at.x - h),
                            n(at.y - h),
                            n(at.x + h),
                            n(at.y + h),
                            n(at.x - h),
                            n(at.y + h),
                            n(at.x + h),
                            n(at.y - h)
                        ),
                    ),
                    // a triangle pointing down, over its column
                    Mark::Evict => (
                        "evict",
                        format!(
                            "M {} {} L {} {} L {} {} z",
                            n(at.x - h),
                            n(at.y - h),
                            n(at.x + h),
                            n(at.y - h),
                            n(at.x),
                            n(at.y + h)
                        ),
                    ),
                };
                let _ = writeln!(s, r#"<path {} d="{d}"/>"#, paint(class));
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
