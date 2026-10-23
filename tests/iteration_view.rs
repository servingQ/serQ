//! The iteration view (`serq::view::iterations`): an engine of a run,
//! iteration by iteration, drawn from the run's log and nothing else.

mod common;

use std::path::{Path, PathBuf};

use serq::engine::log::{EventKind, Log, Who};
use serq::engine::state::Status;
use serq::frontend::ast::RunMode;
use serq::run_ir_logged;
use serq::view::figure::{BoxStyle, Figure, Item, Mark, Rect};
use serq::view::iterations::{self, CELL_MAX, CELL_MIN, CELL_W, ROW_H, STRIP_H};

/// The tutorial's engine (chapter 5, the program of chapter 6's cliff).
const CLIFF: &str = "docs/tutorial/programs/05-engine.sq";

/// The crate's directory, which the programs' paths are relative to.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The log of `program` (relative to the crate, with the workload
/// `examples/pairs.json` gives it) run for `horizon` seconds at seed 1,
/// with `inputs` given as `--name value` would.
fn logged(program: &str, horizon: f64, inputs: &[(&str, f64)]) -> Log {
    let mut ov = common::horizon(horizon);
    for &(name, x) in inputs {
        ov.set_num(name, x).unwrap();
    }
    let path = root().join(program);
    let workload = serq::paired_workload(&path).expect("a pair");
    let p = serq::load_paired(&path, &workload, &ov).unwrap_or_else(|e| panic!("{program}: {e}"));
    let r = run_ir_logged(&p, workload.parent(), usize::MAX).unwrap();
    r.log.expect("a logged run")
}

/// The figure's frames, top down: the requests, the tokens, the memory.
fn frames(f: &Figure) -> Vec<Rect> {
    f.boxes(BoxStyle::Frame)
}

/// The column of an x in the grid `grid` (a frame, padded by 1).
fn column(grid: &Rect, x: f64) -> usize {
    ((x - (grid.x + 1.0)) / CELL_W).floor() as usize
}

/// The row of a y in the grid.
fn row(grid: &Rect, y: f64) -> usize {
    ((y - (grid.y + 1.0)) / ROW_H).floor() as usize
}

/// The boxes of `style` inside `frame`.
fn inside(f: &Figure, style: BoxStyle, frame: &Rect) -> Vec<Rect> {
    f.boxes(style)
        .into_iter()
        .filter(|r| frame.contains(r))
        .collect()
}

/// The text of the row labels, by row: the session (or `s/leg`) each
/// row is.
fn row_labels(f: &Figure, grid: &Rect) -> Vec<(usize, String)> {
    f.items
        .iter()
        .filter_map(|i| match i {
            Item::Text { at, text, .. }
                if at.x < grid.x && at.y > grid.y && at.y < grid.bottom() =>
            {
                Some((row(grid, at.y), text.clone()))
            }
            _ => None,
        })
        .collect()
}

/// A resident's row label, written out here to hold the figure's to it:
/// `34`, or `34/0` for a leg of session 34.
fn label(w: Who) -> String {
    match w.leg {
        Some(l) => format!("{}/{l}", w.serial),
        None => w.serial.to_string(),
    }
}

/// The engine's iterations from `from`, at most `n`, as the log has them:
/// `(t, budget, [(who, tokens, mode)], evicted rose)`, the mode the
/// session's last run's.
#[allow(clippy::type_complexity)]
fn iterations_of(
    log: &Log,
    from: f64,
    n: usize,
) -> Vec<(f64, f64, Vec<(Who, f64, RunMode)>, bool)> {
    let mut mode = std::collections::HashMap::new();
    let mut evicted = 0.0;
    let mut out = vec![];
    for e in &log.events {
        match &e.kind {
            EventKind::Run { who, mode: m, .. } => {
                mode.insert(*who, *m);
            }
            EventKind::Iter {
                budget,
                reqs,
                each,
                memory,
                ..
            } => {
                let rose = memory.is_some_and(|m| m.evicted > evicted);
                if let Some(m) = memory {
                    evicted = m.evicted;
                }
                if e.t >= from && out.len() < n {
                    let cells = reqs
                        .iter()
                        .zip(each)
                        .map(|(w, &k)| (*w, k, mode[w]))
                        .collect();
                    out.push((e.t, *budget, cells, rose));
                }
            }
            _ => {}
        }
    }
    out
}

/// Every cell is a request of its iteration's batch, in that request's
/// row: as many cells as the batch has requests, each coloured by its
/// run's mode and as tall as its tokens' share of the `tokens cap` (a
/// hairline at least); and the tokens strip's bar is the iteration's
/// tokens against the cap.
#[test]
fn cells_are_the_batch() {
    let log = logged(CLIFF, 60.0, &[("Lambda", 2.0)]);
    let (from, n) = (40.0, 120);
    let f = iterations::figure(&log, None, from, n).unwrap();
    let want = iterations_of(&log, from, n);
    assert_eq!(want.len(), n);
    let fr = frames(&f);
    let (grid, strip) = (fr[0], fr[1]);
    let labels = row_labels(&f, &grid);
    let who_at = |r: usize| {
        labels
            .iter()
            .find(|(k, _)| *k == r)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| panic!("row {r} has no label"))
    };
    let mut decodes = 0;
    for (c, (t, budget, cells, _)) in want.iter().enumerate() {
        let mut got: Vec<(String, BoxStyle, f64)> = vec![];
        for style in [BoxStyle::Prefill, BoxStyle::Decode] {
            for r in inside(&f, style, &grid) {
                if column(&grid, r.x) == c {
                    got.push((who_at(row(&grid, r.y)), style, r.h));
                }
            }
        }
        let mut expect: Vec<(String, BoxStyle, f64)> = cells
            .iter()
            .map(|&(w, k, mode)| {
                let style = if mode == RunMode::Decode {
                    decodes += 1;
                    BoxStyle::Decode
                } else {
                    BoxStyle::Prefill
                };
                (
                    label(w),
                    style,
                    (CELL_MAX * k / budget).clamp(CELL_MIN, CELL_MAX),
                )
            })
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        expect.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got, expect, "the iteration at t = {t}");
        // the strip: prefill under decode, together the tokens' share
        let bar: f64 = [BoxStyle::Prefill, BoxStyle::Decode]
            .into_iter()
            .flat_map(|s| inside(&f, s, &strip))
            .filter(|r| column(&grid, r.x) == c)
            .map(|r| r.h)
            .sum();
        let tokens: f64 = cells.iter().map(|x| x.1).sum();
        assert!(
            (bar - STRIP_H * tokens / budget).abs() < 1e-9,
            "t = {t}: {bar} for {tokens} of {budget}"
        );
    }
    assert!(decodes > 0);
}

/// A preemption is a cross in its victim's row, at the engine's first
/// iteration after it: on the cliff's program at 2.0 sessions/s, the first
/// request preempted (vLLM's `preempt lifo` when the blocks run out).
#[test]
fn a_preemption_is_marked_in_its_victims_row() {
    let log = logged(CLIFF, 100.0, &[("Lambda", 2.0)]);
    let (t, victim) = log
        .events
        .iter()
        .find_map(|e| match e.kind {
            EventKind::Status {
                who,
                status: Status::Preempted,
                ..
            } => Some((e.t, who)),
            _ => None,
        })
        .expect("a preemption by t = 100");
    let f = iterations::figure(&log, None, t, 20).unwrap();
    let grid = frames(&f)[0];
    let marks: Vec<_> = f
        .marks()
        .into_iter()
        .filter(|(_, m)| *m == Mark::Preempt)
        .filter(|(at, _)| grid.contains(&Rect::new(at.x, at.y, 0.0, 0.0)))
        .collect();
    let (at, _) = marks
        .iter()
        .find(|(at, _)| column(&grid, at.x) == 0)
        .unwrap_or_else(|| panic!("no cross in the first column: {marks:?}"));
    let labels = row_labels(&f, &grid);
    let r = row(&grid, at.y);
    assert!(
        labels.contains(&(r, label(victim))),
        "the cross is in row {r}, {victim:?} is not: {labels:?}"
    );
}

/// A triangle over a column is an eviction since the engine's previous
/// iteration: exactly the columns where the memory's `evicted` rose, the
/// first one's against the iteration before the window.
#[test]
fn an_eviction_is_marked_where_the_counter_rose() {
    let log = logged(CLIFF, 60.0, &[("Lambda", 2.0)]);
    let (from, n) = (50.0, 120);
    let want: Vec<usize> = iterations_of(&log, from, n)
        .iter()
        .enumerate()
        .filter(|(_, x)| x.3)
        .map(|(c, _)| c)
        .collect();
    assert!(!want.is_empty(), "the cache is evicted at 2.0 sessions/s");
    assert!(want.len() < n, "not every iteration evicts");
    let f = iterations::figure(&log, None, from, n).unwrap();
    let grid = frames(&f)[0];
    let kv = frames(&f)[2];
    let mut got: Vec<usize> = f
        .marks()
        .into_iter()
        .filter(|(at, m)| *m == Mark::Evict && at.y < kv.y && at.y > grid.bottom())
        .map(|(at, _)| column(&grid, at.x))
        .collect();
    got.sort();
    assert_eq!(got, want);
}

/// Every bar of a strip stays in its frame: the tokens never pass the
/// `tokens cap`, and the memory's allocated and cached units never its
/// pool's cap (the pools' invariant, which the strip makes visible).
#[test]
fn the_strips_stay_inside_their_caps() {
    for lambda in [1.5, 2.0] {
        let log = logged(CLIFF, 60.0, &[("Lambda", lambda)]);
        let f = iterations::figure(&log, None, 30.0, 400).unwrap();
        let fr = frames(&f);
        assert_eq!(fr.len(), 3, "rows, tokens and memory");
        let (strip, kv) = (fr[1], fr[2]);
        // the legend's swatches stand under the strips
        let bars = |style| -> Vec<Rect> {
            f.boxes(style)
                .into_iter()
                .filter(|r| r.y < kv.bottom())
                .collect()
        };
        for style in [BoxStyle::Used, BoxStyle::Cached] {
            let all = bars(style);
            assert!(!all.is_empty());
            for r in all {
                assert!(
                    kv.pad(1e-9).contains(&r),
                    "{lambda}: {style:?} {r:?} out of {kv:?}"
                );
            }
        }
        for style in [BoxStyle::Prefill, BoxStyle::Decode] {
            for r in bars(style).into_iter().filter(|r| r.y > fr[0].bottom()) {
                assert!(
                    strip.pad(1e-9).contains(&r),
                    "{lambda}: {r:?} out of {strip:?}"
                );
            }
        }
    }
}

/// With several engines (a P/D deployment's prefillers and decoders) the
/// figure is of one, named as the log's header names it.
#[test]
fn an_engine_is_named_when_there_are_several() {
    let log = logged("examples/pd-disaggregation/llmd_nixl_pull.sq", 30.0, &[]);
    let e = iterations::figure(&log, None, 0.0, 20).unwrap_err();
    assert!(
        e.contains("several engines") && e.contains("P[0]") && e.contains("D[0]"),
        "{e}"
    );
    let f = iterations::figure(&log, Some("D[0]"), 0.0, 20).unwrap();
    assert!(f.title.starts_with("D[0]:"), "{}", f.title);
    let e = iterations::figure(&log, Some("D"), 0.0, 20).unwrap_err();
    assert!(e.contains("no stage `D`"), "{e}");
}

/// A window past the engine's last iteration is refused with where that
/// is, not drawn empty.
#[test]
fn a_window_past_the_end_is_refused() {
    let log = logged(CLIFF, 20.0, &[]);
    let e = iterations::figure(&log, None, 25.0, 20).unwrap_err();
    assert!(
        e.contains("no iteration from t = 25") && e.contains("its last is at t = "),
        "{e}"
    );
}

/// A log written before the iteration's per-request tokens (#728) is
/// refused with what to do, not drawn without them.
#[test]
fn a_log_without_per_request_tokens_is_refused() {
    let log = logged(CLIFF, 5.0, &[]);
    let mut j: serde_json::Value = serde_json::from_str(&log.json()).unwrap();
    for e in j["events"].as_array_mut().unwrap() {
        e.as_object_mut().unwrap().remove("each");
    }
    let e = Log::from_json(&j.to_string()).unwrap_err();
    assert!(
        e.contains("older serq") && e.contains("run the program again"),
        "{e}"
    );
}

/// Both writers paint a run's figure, with the colours a program's figure
/// does without.
#[test]
fn both_writers_paint_a_run() {
    let log = logged(CLIFF, 60.0, &[("Lambda", 2.0)]);
    let f = iterations::figure(&log, None, 50.0, 60).unwrap();
    let tikz = serq::view::render(&f, "tikz").unwrap();
    for word in [
        "\\begin{tikzpicture}",
        "seqprefill",
        "seqdecode",
        "seqused",
        "seqevict",
    ] {
        assert!(tikz.contains(word), "{word}");
    }
    let svg = serq::view::render(&f, "svg").unwrap();
    for word in [
        "class=\"prefill\"",
        "class=\"decode\"",
        "class=\"used\"",
        "class=\"evict\"",
        ".prefill {",
    ] {
        assert!(svg.contains(word), "{word}");
    }
    // a program's figure defines none of them
    let p = common::example("examples/multi-turn/vllm.sq", &common::horizon(10.0)).unwrap();
    let deployment = serq::view::deployment::figure(&p);
    assert!(!deployment.of_a_run());
    assert!(
        !serq::view::render(&deployment, "svg")
            .unwrap()
            .contains("prefill")
    );
    assert!(
        !serq::view::render(&deployment, "tikz")
            .unwrap()
            .contains("seqprefill")
    );
}

/// The figures the site shows: each `docs/assets/NAME.iterations.svg` is
/// what `serq draw --log` makes now of the run its row names, and `make
/// draw-golden` rewrites them. A figure without its row is an error.
#[test]
fn iteration_assets_are_current() {
    // (asset, program, Lambda, horizon, from, iterations): the commands of
    // docs/tutorial/06-the-cliff.md, at seed 1
    let table: [(&str, &str, f64, f64, f64, usize); 2] = [
        ("cliff-1.5", CLIFF, 1.5, 600.0, 591.0, 120),
        ("cliff-2.0", CLIFF, 2.0, 600.0, 591.0, 120),
    ];
    let assets = root().join("docs/assets");
    for entry in std::fs::read_dir(&assets).unwrap().flatten() {
        let name = entry.file_name().into_string().unwrap();
        if let Some(stem) = name.strip_suffix(".iterations.svg") {
            assert!(
                table.iter().any(|row| row.0 == stem),
                "docs/assets/{name} has no run in this table"
            );
        }
    }
    for (stem, program, lambda, horizon, from, n) in table {
        let log = logged(program, horizon, &[("Lambda", lambda)]);
        // what the CLI reads is what the run logged
        let log = Log::from_json(&log.json()).unwrap();
        let f = iterations::figure(&log, None, from, n).unwrap();
        let got = serq::view::svg::render(&f);
        let path: PathBuf = assets.join(format!("{stem}.iterations.svg"));
        if std::env::var("SERQ_BLESS").is_ok() {
            std::fs::write(&path, &got).unwrap();
        } else {
            let want = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}; run `make draw-golden`", path.display()));
            assert!(
                want == got,
                "docs/assets/{stem}.iterations.svg is not its run's figure; run `make draw-golden`"
            );
        }
    }
}
