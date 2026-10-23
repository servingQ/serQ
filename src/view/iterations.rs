//! The iteration view: one engine of a run, iteration by iteration, read
//! from the run's log (`engine::log`). Where the deployment view is a
//! function of the program and runs nothing, this one is a function of a
//! run that already ran (`serq run --log`), and runs nothing either.
//!
//! Across, the engine's iterations, one column each. Down, the requests in
//! them, one row per session (a leg of one: `34/0`), in the order they
//! first appear. A cell is what its request took of the iteration: its
//! colour the mode of its run (prefill or decode), its height its tokens
//! (`iter.each`) against the iteration's `tokens cap` (`iter.budget`), at
//! least a hairline. A cross is a preemption in the victim's row, at the
//! engine's first iteration after it. Under the rows, per column, the
//! tokens the iteration took against its cap, and the engine's memory pool
//! (`used`, then `cached`) against the pool's `cap`, which the log's header
//! gives; a triangle over a column is an eviction since the engine's
//! previous iteration (`iter.evicted` rose).

use std::collections::HashMap;

use crate::engine::log::{EventKind, Log, Memory, Who};
use crate::engine::state::Status;
use crate::frontend::ast::RunMode;
use crate::view::figure::{
    Anchor, BoxStyle, EdgeStyle, Figure, Item, Mark, Point, Rect, TextSize, pt,
};

/// How many iterations a figure draws unless told.
pub const ITERATIONS: usize = 120;

/// A column's width, in points.
pub const CELL_W: f64 = 5.0;
/// A row's height, in points.
pub const ROW_H: f64 = 9.0;
/// The tallest a cell is: a row less the gap between rows; a cell of all
/// the iteration's `tokens cap`.
pub const CELL_MAX: f64 = ROW_H - 1.5;
/// The least a cell is, so that a request with one token shows.
pub const CELL_MIN: f64 = 1.5;
/// A strip's height under the rows.
pub const STRIP_H: f64 = 36.0;
/// The left of the grid: the rows' labels stand left of it.
const LEFT: f64 = 56.0;
/// The top of the grid, under the title and its note.
const TOP: f64 = 52.0;
/// A time tick every so many columns.
const TICK_EVERY: usize = 20;
/// A mark's size.
const MARK: f64 = 4.5;

/// The stage the figure draws: `engine` by its name (`E`, or `E[0]` for a
/// member of an array), or the only stage that iterates.
fn engine_of(log: &Log, engine: Option<&str>) -> Result<usize, String> {
    let mut iterating = vec![false; log.stages.len()];
    for e in &log.events {
        if let EventKind::Iter { stage, .. } = e.kind {
            iterating[stage] = true;
        }
    }
    let engines: Vec<usize> = (0..log.stages.len()).filter(|&k| iterating[k]).collect();
    let names = |ks: &[usize]| {
        ks.iter()
            .map(|&k| log.stages[k].label())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match engine {
        None => match engines[..] {
            [k] => Ok(k),
            [] => Err("the log has no iteration of an engine".into()),
            _ => Err(format!(
                "the log has iterations of several engines ({}); name one with --engine",
                names(&engines)
            )),
        },
        Some(name) => {
            let named: Vec<usize> = (0..log.stages.len())
                .filter(|&k| log.stages[k].label() == name)
                .collect();
            match named[..] {
                [k] if iterating[k] => Ok(k),
                [_] => Err(format!(
                    "`{name}` has no iteration in the log; the engines that have: {}",
                    names(&engines)
                )),
                _ => Err(format!(
                    "the log has no stage `{name}`; its engines: {}",
                    names(&engines)
                )),
            }
        }
    }
}

/// One column: an iteration of the engine, with what the log says
/// happened since the engine's previous one.
struct Column {
    t: f64,
    tokens: f64,
    budget: f64,
    /// Each request of the batch, its tokens and its mode.
    cells: Vec<(Who, f64, RunMode)>,
    memory: Option<Memory>,
    /// Requests the engine preempted since its previous iteration.
    preempted: Vec<Who>,
    /// Its memory evicted cached units since its previous iteration.
    evicted: bool,
}

/// The engine's iterations from `from` on, at most `n`, and whether the
/// log had more of them.
fn columns(log: &Log, stage: usize, from: f64, n: usize) -> (Vec<Column>, bool) {
    let memory = log.stages[stage].memory;
    // a session's mode is its last run's: a run's mode is fixed from its
    // start and a session is in one run at a time (`tools/serq_iter_trace.py`
    // reads it so); a run outside any engine request has no `status`
    let mut mode: HashMap<Who, RunMode> = HashMap::new();
    let mut previous: Vec<Who> = vec![];
    let mut preempted: Vec<Who> = vec![];
    let mut evicted = 0.0;
    let mut out = vec![];
    for e in &log.events {
        match &e.kind {
            EventKind::Run { who, mode: m, .. } => {
                mode.insert(*who, *m);
            }
            // an engine request preempted from this engine, or a hold of
            // its memory preempted from a request in its last batch (an
            // engine whose entries take no request)
            EventKind::Status {
                who,
                stage: s,
                status: Status::Preempted,
                ..
            } if *s == stage => {
                if !preempted.contains(who) {
                    preempted.push(*who);
                }
            }
            EventKind::Preempt { who, pool } if Some(*pool) == memory => {
                if previous.contains(who) && !preempted.contains(who) {
                    preempted.push(*who);
                }
            }
            EventKind::Iter {
                stage: s,
                tokens,
                budget,
                reqs,
                each,
                memory: m,
                ..
            } if *s == stage => {
                let rose = m.is_some_and(|m| m.evicted > evicted);
                if let Some(m) = m {
                    evicted = m.evicted;
                }
                if e.t >= from {
                    if out.len() == n {
                        return (out, true);
                    }
                    out.push(Column {
                        t: e.t,
                        tokens: *tokens,
                        budget: *budget,
                        cells: reqs
                            .iter()
                            .zip(each)
                            .map(|(w, &k)| (*w, k, mode.get(w).copied().unwrap_or(RunMode::Plain)))
                            .collect(),
                        memory: *m,
                        preempted: std::mem::take(&mut preempted),
                        evicted: rose,
                    });
                } else {
                    preempted.clear();
                }
                previous.clone_from(reqs);
            }
            _ => {}
        }
    }
    (out, false)
}

/// `34`, or `34/0` for a leg of session 34.
fn who_label(w: Who) -> String {
    match w.leg {
        Some(l) => format!("{}/{l}", w.serial),
        None => w.serial.to_string(),
    }
}

/// A number as a label: a whole one without its fraction.
fn num(x: f64) -> String {
    if x.is_finite() && x == x.trunc() && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// What a column's tokens are measured against, and how the figure says so.
struct Scale {
    /// Per column: the tokens a full row and a full strip stand for.
    per: Vec<f64>,
    label: String,
}

/// Each column against its iteration's tokens cap, or, when a cap is not a
/// positive finite number, every column against the window's largest batch.
fn token_scale(cols: &[Column]) -> Scale {
    let budgets: Vec<f64> = cols.iter().map(|c| c.budget).collect();
    if budgets.iter().all(|b| b.is_finite() && *b > 0.0) {
        let (lo, hi) = budgets
            .iter()
            .fold((f64::INFINITY, 0.0f64), |(lo, hi), &b| {
                (lo.min(b), hi.max(b))
            });
        let label = if lo == hi {
            format!("tokens of tokens cap {}", num(lo))
        } else {
            format!(
                "tokens of each iteration's tokens cap ({} to {})",
                num(lo),
                num(hi)
            )
        };
        Scale {
            per: budgets,
            label,
        }
    } else {
        // no cap (or one that is not finite): the window's largest batch
        let most = cols.iter().map(|c| c.tokens).fold(1.0f64, f64::max);
        Scale {
            per: vec![most; cols.len()],
            label: format!(
                "tokens, no tokens cap: against the largest iteration, {} tokens",
                num(most)
            ),
        }
    }
}

/// The figure of `engine`'s iterations from `from` on, at most `n`.
pub fn figure(log: &Log, engine: Option<&str>, from: f64, n: usize) -> Result<Figure, String> {
    if n == 0 {
        return Err("a figure of no iteration".into());
    }
    let stage = engine_of(log, engine)?;
    let name = log.stages[stage].label();
    let (cols, more) = columns(log, stage, from, n);
    if cols.is_empty() {
        let last = log.events.iter().rev().find_map(|e| match e.kind {
            EventKind::Iter { stage: s, .. } if s == stage => Some(e.t),
            _ => None,
        });
        return Err(format!(
            "`{name}` has no iteration from t = {from}: its last is at t = {}",
            last.map_or("?".into(), |t| t.to_string())
        ));
    }

    // the rows: each request in a batch or preempted, as it first appears
    let mut rows: Vec<Who> = vec![];
    for c in &cols {
        for w in c.cells.iter().map(|x| &x.0).chain(&c.preempted) {
            if !rows.contains(w) {
                rows.push(*w);
            }
        }
    }
    let row_of: HashMap<Who, usize> = rows.iter().enumerate().map(|(k, w)| (*w, k)).collect();
    let tokens = token_scale(&cols);
    let mut f = Figure::default();
    let (first, last) = (cols[0].t, cols[cols.len() - 1].t);
    f.title = format!(
        "{name}: {} iterations, t = {first:.4} to {last:.4}",
        cols.len()
    );
    f.text(
        pt(LEFT, 18.0),
        f.title.clone(),
        Anchor::Start,
        TextSize::Title,
    );
    let mut note = if more || cols.len() == n {
        format!("from t = {from}, the first {} of the engine", cols.len())
    } else {
        format!(
            "from t = {from}: all the engine has, {} iterations",
            cols.len()
        )
    };
    if log.truncated {
        note.push_str(&format!(
            "; the log is truncated at {} events, past t = {} the run is not in it",
            log.events.len(),
            log.events.last().map_or(0.0, |e| e.t)
        ));
    }
    f.note(pt(LEFT, 32.0), note, Anchor::Start);

    let x = |c: usize| LEFT + c as f64 * CELL_W;
    let width = cols.len() as f64 * CELL_W;
    // the requests
    let grid = Rect::new(LEFT, TOP, width, rows.len() as f64 * ROW_H);
    f.note(pt(LEFT - 4.0, TOP - 4.0), "session", Anchor::End);
    f.boxed(grid.pad(1.0), BoxStyle::Frame, 0.0);
    for (k, w) in rows.iter().enumerate() {
        f.note(
            pt(LEFT - 4.0, TOP + (k + 1) as f64 * ROW_H - 1.5),
            who_label(*w),
            Anchor::End,
        );
    }
    for (c, col) in cols.iter().enumerate() {
        for &(w, k, mode) in &col.cells {
            let bottom = TOP + (row_of[&w] + 1) as f64 * ROW_H - 0.75;
            let h = (CELL_MAX * k / tokens.per[c]).clamp(CELL_MIN, CELL_MAX);
            let style = if mode == RunMode::Decode {
                BoxStyle::Decode
            } else {
                BoxStyle::Prefill
            };
            f.boxed(
                Rect::new(x(c) + 0.5, bottom - h, CELL_W - 1.0, h),
                style,
                0.0,
            );
        }
        for w in &col.preempted {
            let at = Point {
                x: x(c) + CELL_W / 2.0,
                y: TOP + (row_of[w] as f64 + 0.5) * ROW_H,
            };
            f.push(Item::Mark {
                at,
                size: MARK,
                mark: Mark::Preempt,
            });
        }
    }

    // the tokens each iteration took, prefill under decode
    let mut y = grid.bottom() + 22.0;
    f.note(pt(LEFT, y - 6.0), tokens.label.clone(), Anchor::Start);
    let strip = Rect::new(LEFT, y, width, STRIP_H);
    f.boxed(strip.pad(1.0), BoxStyle::Frame, 0.0);
    for (c, col) in cols.iter().enumerate() {
        let (mut p, mut d) = (0.0, 0.0);
        for &(_, k, mode) in &col.cells {
            if mode == RunMode::Decode {
                d += k;
            } else {
                p += k;
            }
        }
        stack(
            &mut f,
            x(c),
            strip.bottom(),
            &[(p, BoxStyle::Prefill), (d, BoxStyle::Decode)],
            tokens.per[c],
        );
    }
    y = strip.bottom();

    // the engine's memory: allocated under cached, against its cap
    if let Some(pool) = log.stages[stage].memory {
        let pool = &log.pools[pool];
        let (scale, label) = if pool.cap.is_finite() && pool.cap > 0.0 {
            (
                pool.cap,
                format!("{}: used + cached of cap {}", pool.label(), num(pool.cap)),
            )
        } else {
            let most = cols
                .iter()
                .filter_map(|c| c.memory.map(|m| m.used + m.cached))
                .fold(1.0f64, f64::max);
            (
                most,
                format!(
                    "{}: used + cached, no cap: against the most, {}",
                    pool.label(),
                    num(most)
                ),
            )
        };
        y += 30.0;
        f.note(pt(LEFT, y - 12.0), label, Anchor::Start);
        let strip = Rect::new(LEFT, y, width, STRIP_H);
        f.boxed(strip.pad(1.0), BoxStyle::Frame, 0.0);
        for (c, col) in cols.iter().enumerate() {
            if let Some(m) = col.memory {
                stack(
                    &mut f,
                    x(c),
                    strip.bottom(),
                    &[(m.used, BoxStyle::Used), (m.cached, BoxStyle::Cached)],
                    scale,
                );
            }
            if col.evicted {
                f.push(Item::Mark {
                    at: pt(x(c) + CELL_W / 2.0, strip.y - 1.0 - MARK / 2.0 - 1.0),
                    size: MARK,
                    mark: Mark::Evict,
                });
            }
        }
        y = strip.bottom();
    }

    // the time of every TICK_EVERY-th column, and of the last
    y += 4.0;
    let mut ticks: Vec<usize> = (0..cols.len()).step_by(TICK_EVERY).collect();
    if cols.len() - 1 - ticks[ticks.len() - 1] >= TICK_EVERY / 2 {
        ticks.push(cols.len() - 1);
    }
    for c in ticks {
        let cx = x(c) + CELL_W / 2.0;
        f.push(Item::Edge {
            pts: vec![pt(cx, y), pt(cx, y + 4.0)],
            style: EdgeStyle::Flow,
            arrow: false,
            role: None,
            copy: None,
        });
        // the first starts at its tick, clear of the axis' name
        let (at, anchor) = if c == 0 {
            (x(0), Anchor::Start)
        } else {
            (cx, Anchor::Middle)
        };
        f.note(pt(at, y + 13.0), format!("{:.4}", cols[c].t), anchor);
    }
    f.note(pt(LEFT - 4.0, y + 13.0), "t (s)", Anchor::End);

    // the legend
    y += 30.0;
    let mut lx = LEFT;
    let swatches = [
        (BoxStyle::Prefill, "prefill"),
        (BoxStyle::Decode, "decode"),
        (BoxStyle::Used, "used"),
        (BoxStyle::Cached, "cached"),
    ];
    for (style, word) in swatches {
        if matches!(style, BoxStyle::Used | BoxStyle::Cached) && log.stages[stage].memory.is_none()
        {
            continue;
        }
        f.boxed(Rect::new(lx, y - 7.0, 8.0, 8.0), style, 0.0);
        f.note(pt(lx + 12.0, y), word, Anchor::Start);
        lx += 12.0 + TextSize::Small.width_of(word) + 14.0;
    }
    let marks = [(Mark::Preempt, "preempted"), (Mark::Evict, "evicted")];
    for (mark, word) in marks {
        if mark == Mark::Evict && log.stages[stage].memory.is_none() {
            continue;
        }
        f.push(Item::Mark {
            at: pt(lx + 4.0, y - 3.0),
            size: MARK + 1.5,
            mark,
        });
        f.note(pt(lx + 12.0, y), word, Anchor::Start);
        lx += 12.0 + TextSize::Small.width_of(word) + 14.0;
    }
    f.fit(16.0);
    Ok(f)
}

/// A column's bar in a strip whose bottom is `bottom`: `parts` stacked
/// from the bottom, each as tall as its share of `scale`.
fn stack(f: &mut Figure, x: f64, bottom: f64, parts: &[(f64, BoxStyle)], scale: f64) {
    let mut y = bottom;
    for &(v, style) in parts {
        let h = STRIP_H * v / scale;
        if h > 0.0 {
            f.boxed(Rect::new(x + 0.5, y - h, CELL_W - 1.0, h), style, 0.0);
            y -= h;
        }
    }
}
