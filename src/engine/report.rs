//! What a run reports: `observe` statistics, per-stage and per-pool
//! time averages and counters. Printed as text or JSON.

use std::fmt::Write as _;

use crate::engine::stats::{CI_MIN_SAMPLES, Estimate};

#[derive(Clone, Debug)]
pub struct ObserveReport {
    pub name: String,
    pub count: u64,
    pub mean: f64,
    pub cv2: f64,
    /// Batch-means 95 % CI (below 40 samples, mean NaN and half-width +inf).
    pub ci: Estimate,
    pub p99: f64,
    pub samples: Vec<f64>,
    /// `(time, session serial, turn number)` of every sample.
    pub records: Vec<(f64, u64, u32)>,
    /// The observed expression is a test (its outermost operator a
    /// comparison, `&&`, `||` or `!`): 0 or 1 by construction
    /// (`Program::observe_is_test`).
    pub is_test: bool,
}

impl ObserveReport {
    /// A test that never held: 0 over `CI_MIN_SAMPLES` or more samples. A
    /// `hit` that was 0 all run long is a program to look at before its
    /// means are (#230 was found that way, late), and the table alone does
    /// not show it (cv2 is NaN for a constant 0). A test that always held
    /// is not noted: a routing policy that hits by construction is a
    /// correct program. The note is a line of text, not an error: one
    /// program of the corpus earns it by design
    /// (`tests/report.rs::the_corpus_earns_one_note`).
    pub fn never_held(&self) -> bool {
        self.is_test
            && self.samples.len() >= CI_MIN_SAMPLES
            && self.samples.iter().all(|&x| x == 0.0)
    }
}

/// A `gauge`: the time average of a function of the deployment's state
/// over `[warmup, end]`, and the extremes it held.
#[derive(Clone, Debug)]
pub struct GaugeReport {
    pub name: String,
    pub mean: f64,
    /// Batch-means 95 % CI over 20 equal windows of the measured span.
    pub ci: Estimate,
    /// The least and greatest value held for a positive time after warm-up.
    pub min: f64,
    pub max: f64,
    /// `(time, value)` change points, the value from that time on, from
    /// time 0 (before warm-up included).
    pub points: Vec<(f64, f64)>,
}

#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(get_all, frozen, name = "Stage", module = "pyserq")
)]
pub struct StageReport {
    pub name: String,
    /// The member's index in a stage array (`rep[1]`), `None` for a single
    /// stage.
    pub index: Option<u32>,
    /// Time-average jobs present (waiting and in service).
    pub mean_number: f64,
    /// Fraction of time with at least one job present; on a shared stage,
    /// the time-average capacity its flows carry (`Σ rate / φ`).
    pub utilization: f64,
    pub completed: u64,
    pub throughput: f64,
    pub mean_wait: f64,
    pub mean_service: f64,
    /// Step stages: iterations run (0 otherwise).
    pub iterations: u64,
    /// Step stages: the fraction of the measured time an iteration of
    /// prefill only, of decodes only, or of both was running (0 otherwise;
    /// the rest of the time the engine was idle).
    pub prefill_only: f64,
    pub decode_only: f64,
    pub mixed: f64,
    /// Step stages: time-average decodes in the running iteration (0 while
    /// none runs).
    pub mean_decodes: f64,
    /// Step stages: the decodes of an iteration that carried any, and its
    /// duration, averaged over such iterations started after warm-up (NaN
    /// otherwise): the batch a decode is in and the step it waits for.
    pub mean_decode_batch: f64,
    pub mean_decode_step: f64,
    /// Step stages: the gaps between a turn's successive tokens (a session's
    /// tokens with the same `turn_no`) that end on this stage after warm-up,
    /// wherever the earlier token was: their mean, and their median and
    /// 99th percentile within 0.5 % (NaN without any). A turn's tokens are
    /// its decodes' and its prefills' ends; a prefill's end is the next
    /// token after a decode, or after a preemption on the previous token's
    /// stage, and its gap holds the preemption; otherwise it is the first,
    /// and replaces any before it (a decoder's recompute replaces a
    /// prefiller's dropped token). A turn's gaps add up to its last token
    /// less its first.
    pub mean_itl: f64,
    pub itl_p50: f64,
    pub itl_p99: f64,
    /// Step stages: the run ended with the stage holding residents or a
    /// waiting queue it serves, and its last try at an iteration scheduled
    /// nothing (a body or `serve only` that served and admitted nobody).
    pub idle_with_work: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(get_all, frozen, name = "Pool", module = "pyserq")
)]
pub struct PoolReport {
    pub name: String,
    /// The member's index in a pool array (`kvD[1]`), `None` for a single
    /// pool.
    pub index: Option<u32>,
    pub mean_used: f64,
    pub mean_cached: f64,
    pub mean_queue: f64,
    pub mean_holders: f64,
    pub mean_wait: f64,
    pub admissions: u64,
    pub evicted_entries: u64,
    pub evicted_units: f64,
    pub preemptions: u64,
    pub spills: u64,
    pub rejected: u64,
    /// Sessions preempted a second time without progress past their
    /// previous preemption: a livelock the run would otherwise hide.
    pub stuck: u64,
    /// The run ended with the head of a queue asking this pool for more
    /// than its cap: the queue's pool and what the head asks. A hold whose
    /// units or `reserve` read the deployment's state is not rejected when
    /// it joins the queue, and waits (#364).
    pub over_cap: Option<(String, f64)>,
}

/// A `claim`: what the run found of it on the path it ran.
#[derive(Clone, Debug)]
pub struct ClaimReport {
    pub name: String,
    pub kind: crate::ir::ClaimKind,
    pub result: ClaimResult,
    /// Iterations the claim was read at (an iteration claim), or 1 when a
    /// claim `at end` was read.
    pub checked: u64,
    /// Of those, the ones it read as 0.
    pub failures: u64,
    /// The first failure of an `every` claim or one `at end` (the end), the
    /// first witness of a `some` claim.
    pub first: Option<f64>,
    /// Why the claim was not read: the sessions live at the end, or the
    /// session that failed its `given`.
    pub note: Option<String>,
}

/// What a run says of a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimResult {
    /// Every iteration read, or the end, satisfied it.
    Holds,
    /// Some iteration read, or the end, did not.
    Fails,
    /// A `some` claim: an iteration satisfied it.
    Witnessed,
    /// A `some` claim: no iteration read satisfied it.
    NotWitnessed,
    /// A claim `at end` with sessions live at the end: the run did not
    /// reach the end the claim is about.
    NotEvaluated,
    /// A session failed the claim's `given`.
    OutOfScope,
}

impl ClaimResult {
    /// The JSON spelling.
    pub fn name(self) -> &'static str {
        match self {
            ClaimResult::Holds => "holds",
            ClaimResult::Fails => "fails",
            ClaimResult::Witnessed => "witnessed",
            ClaimResult::NotWitnessed => "not_witnessed",
            ClaimResult::NotEvaluated => "not_evaluated",
            ClaimResult::OutOfScope => "out_of_scope",
        }
    }
}

impl ClaimReport {
    /// The JSON spelling of the kind.
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            crate::ir::ClaimKind::EveryIteration(_) => "every_iteration",
            crate::ir::ClaimKind::SomeIteration(_) => "some_iteration",
            crate::ir::ClaimKind::AtEnd => "at_end",
        }
    }

    /// The result as the text report says it.
    fn said(&self) -> String {
        let iterations = |n: u64| format!("{n} iteration{}", if n == 1 { "" } else { "s" });
        let at = |t: Option<f64>| t.map_or(String::new(), |t| format!(" at {t:.4}"));
        let at_end = matches!(self.kind, crate::ir::ClaimKind::AtEnd);
        match self.result {
            ClaimResult::Holds if at_end => "holds".into(),
            ClaimResult::Holds => format!("holds ({})", iterations(self.checked)),
            ClaimResult::Fails if at_end => "fails".into(),
            ClaimResult::Fails => format!(
                "fails{} ({} of {})",
                at(self.first),
                self.failures,
                iterations(self.checked)
            ),
            ClaimResult::Witnessed => {
                format!("witnessed{} ({})", at(self.first), iterations(self.checked))
            }
            ClaimResult::NotWitnessed => format!("not witnessed ({})", iterations(self.checked)),
            ClaimResult::NotEvaluated => {
                format!("not evaluated: {}", self.note.as_deref().unwrap_or(""))
            }
            ClaimResult::OutOfScope => {
                format!("out of scope: {}", self.note.as_deref().unwrap_or(""))
            }
        }
    }
}

/// Version of `Report::json`'s shape: the names of its fields, which
/// consumers read by name (serving-queue-theory, pyserq). Bump it on a
/// renamed, removed or retyped field, by the rules of `docs/ir.md`
/// (Stability); `tests/report.rs` holds the shape.
pub const REPORT_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct Report {
    /// Configured execution deadline.
    pub horizon: f64,
    /// Actual end of execution; rates use `end - warmup`.
    pub end: f64,
    pub warmup: f64,
    pub seed: u64,
    pub events: u64,
    pub arrivals: u64,
    pub ended: u64,
    /// `turn` commands executed after warm-up.
    pub turns: u64,
    pub mean_live: f64,
    pub observes: Vec<ObserveReport>,
    pub gauges: Vec<GaugeReport>,
    /// The program's claims, in order; empty (and absent from the JSON)
    /// when it has none.
    pub claims: Vec<ClaimReport>,
    pub stages: Vec<StageReport>,
    pub pools: Vec<PoolReport>,
}

impl Report {
    /// Write every observation as `<name>.csv` with columns
    /// `time,session,turn,value` into `dir`, and every gauge's change points
    /// as `gauge/<name>.csv` with columns `time,value`.
    pub fn dump(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for o in &self.observes {
            let mut s = String::from("time,session,turn,value\n");
            for (rec, v) in o.records.iter().zip(&o.samples) {
                s.push_str(&format!("{},{},{},{}\n", rec.0, rec.1, rec.2, v));
            }
            std::fs::write(dir.join(format!("{}.csv", o.name)), s)?;
        }
        if !self.gauges.is_empty() {
            let gdir = dir.join("gauge");
            std::fs::create_dir_all(&gdir)?;
            for g in &self.gauges {
                let mut s = String::from("time,value\n");
                for (t, v) in &g.points {
                    s.push_str(&format!("{t},{v}\n"));
                }
                std::fs::write(gdir.join(format!("{}.csv", g.name)), s)?;
            }
        }
        Ok(())
    }

    pub fn observe(&self, name: &str) -> Option<&ObserveReport> {
        self.observes.iter().find(|o| o.name == name)
    }

    pub fn gauge(&self, name: &str) -> Option<&GaugeReport> {
        self.gauges.iter().find(|g| g.name == name)
    }

    pub fn claim(&self, name: &str) -> Option<&ClaimReport> {
        self.claims.iter().find(|c| c.name == name)
    }

    pub fn stage(&self, name: &str) -> Option<&StageReport> {
        self.stages.iter().find(|s| s.name == name)
    }

    /// All array members of a stage, in index order.
    pub fn stages_named(&self, name: &str) -> Vec<&StageReport> {
        self.stages.iter().filter(|s| s.name == name).collect()
    }

    pub fn pool(&self, name: &str) -> Option<&PoolReport> {
        self.pools.iter().find(|p| p.name == name)
    }

    /// All array members of a pool, in index order.
    pub fn pools_named(&self, name: &str) -> Vec<&PoolReport> {
        self.pools.iter().filter(|p| p.name == name).collect()
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "run: horizon {} end {} warmup {} seed {} events {} arrivals {} ended {} turns {} mean live {:.3}",
            self.horizon,
            self.end,
            self.warmup,
            self.seed,
            self.events,
            self.arrivals,
            self.ended,
            self.turns,
            self.mean_live
        );
        if !self.observes.is_empty() {
            let rows = self.observes.iter().map(|o| {
                vec![
                    o.name.clone(),
                    o.count.to_string(),
                    format!("{:.4}", o.mean),
                    format!("±{:.4}", o.ci.half_width),
                    format!("{:.3}", o.cv2),
                    format!("{:.4}", o.p99),
                ]
            });
            table(
                &mut s,
                &["observe", "count", "mean", "95% CI", "cv2", "p99"],
                rows,
            );
            for o in self.observes.iter().filter(|o| o.never_held()) {
                let _ = writeln!(
                    s,
                    "note: observe {} is constant 0 over {} samples",
                    o.name, o.count
                );
            }
        }
        if !self.gauges.is_empty() {
            let rows = self.gauges.iter().map(|g| {
                vec![
                    g.name.clone(),
                    format!("{:.4}", g.mean),
                    format!("±{:.4}", g.ci.half_width),
                    format!("{:.4}", g.min),
                    format!("{:.4}", g.max),
                ]
            });
            table(&mut s, &["gauge", "mean", "95% CI", "min", "max"], rows);
        }
        if !self.claims.is_empty() {
            let rows = self.claims.iter().map(|c| {
                let kind = match c.kind {
                    crate::ir::ClaimKind::EveryIteration(_) => "every iteration",
                    crate::ir::ClaimKind::SomeIteration(_) => "some iteration",
                    crate::ir::ClaimKind::AtEnd => "at end",
                };
                vec![c.name.clone(), kind.into(), c.said()]
            });
            table_aligned(&mut s, &["claim", "kind", "result"], rows, true);
        }
        if !self.stages.is_empty() {
            let rows = self.stages.iter().map(|st| {
                vec![
                    label(&st.name, st.index),
                    format!("{:.3}", st.mean_number),
                    format!("{:.3}", st.utilization),
                    st.completed.to_string(),
                    format!("{:.4}", st.throughput),
                    format!("{:.4}", st.mean_wait),
                    format!("{:.4}", st.mean_service),
                    st.iterations.to_string(),
                ]
            });
            table(
                &mut s,
                &[
                    "stage", "number", "util", "done", "thru", "wait", "service", "iters",
                ],
                rows,
            );
        }
        if self.stages.iter().any(|st| st.iterations > 0) {
            let rows = self.stages.iter().filter(|st| st.iterations > 0).map(|st| {
                vec![
                    label(&st.name, st.index),
                    format!("{:.3}", st.prefill_only),
                    format!("{:.3}", st.decode_only),
                    format!("{:.3}", st.mixed),
                    format!("{:.3}", 1.0 - st.prefill_only - st.decode_only - st.mixed),
                    format!("{:.3}", st.mean_decodes),
                    format!("{:.3}", st.mean_decode_batch),
                    format!("{:.6}", st.mean_decode_step),
                    format!("{:.6}", st.itl_p50),
                    format!("{:.6}", st.itl_p99),
                ]
            });
            table(
                &mut s,
                &[
                    "step",
                    "prefill only",
                    "decode only",
                    "mixed",
                    "idle",
                    "decodes",
                    "decode batch",
                    "decode step",
                    "itl p50",
                    "itl p99",
                ],
                rows,
            );
        }
        if !self.pools.is_empty() {
            let rows = self.pools.iter().map(|p| {
                vec![
                    label(&p.name, p.index),
                    format!("{:.1}", p.mean_used),
                    format!("{:.1}", p.mean_cached),
                    format!("{:.3}", p.mean_queue),
                    format!("{:.3}", p.mean_holders),
                    format!("{:.4}", p.mean_wait),
                    p.admissions.to_string(),
                    p.evicted_entries.to_string(),
                    format!("{:.0}", p.evicted_units),
                    p.preemptions.to_string(),
                    p.spills.to_string(),
                    p.rejected.to_string(),
                    p.stuck.to_string(),
                ]
            });
            table(
                &mut s,
                &[
                    "pool", "used", "cached", "queue", "holders", "wait", "admits", "evict(n)",
                    "evict(u)", "preempt", "spill", "rej", "stuck",
                ],
                rows,
            );
            for p in &self.pools {
                if p.stuck > 0 {
                    let _ = writeln!(
                        s,
                        "stuck: {} session(s) preempted again at pool `{}` without passing the position of their previous preemption",
                        p.stuck,
                        label(&p.name, p.index)
                    );
                }
                if p.rejected > 0 {
                    let _ = writeln!(
                        s,
                        "rej: {} session(s) ended at pool `{}` asking for more than its cap",
                        p.rejected,
                        label(&p.name, p.index)
                    );
                }
                if let Some((queue, need)) = &p.over_cap {
                    let _ = writeln!(
                        s,
                        "over: the head of pool `{queue}`'s queue asks `{}` for {need}, above \
                         its cap, when the run ends, and waits (its hold reads the \
                         deployment's state, so joining the queue did not reject it)",
                        label(&p.name, p.index)
                    );
                }
            }
        }
        for st in self.stages.iter().filter(|st| st.idle_with_work) {
            let _ = writeln!(
                s,
                "idle: stage `{}` ended with residents or waiting requests, its last iteration \
                 scheduling nothing (a body or `serve only` that serves and admits nobody waits \
                 for an event)",
                label(&st.name, st.index)
            );
        }
        s
    }

    pub fn json(&self) -> String {
        fn f(x: f64) -> String {
            if x.is_finite() {
                format!("{x}")
            } else {
                "null".into()
            }
        }
        fn index(i: Option<u32>) -> String {
            i.map_or("null".into(), |i| i.to_string())
        }
        let mut s = String::from("{");
        // the interpreter that ran, for a record kept next to the IR
        let _ = write!(s, "\"serq_version\":\"{}\",", crate::VERSION);
        let _ = write!(
            s,
            "\"horizon\":{},\"end\":{},\"warmup\":{},\"seed\":{},\"events\":{},\"arrivals\":{},\"ended\":{},\"turns\":{},\"mean_live\":{}",
            f(self.horizon),
            f(self.end),
            f(self.warmup),
            self.seed,
            self.events,
            self.arrivals,
            self.ended,
            self.turns,
            f(self.mean_live)
        );
        s.push_str(",\"observes\":{");
        for (i, o) in self.observes.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "\"{}\":{{\"count\":{},\"mean\":{},\"ci\":{},\"cv2\":{},\"p99\":{}}}",
                o.name,
                o.count,
                f(o.mean),
                f(o.ci.half_width),
                f(o.cv2),
                f(o.p99)
            );
        }
        s.push_str("},\"gauges\":{");
        for (i, g) in self.gauges.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "\"{}\":{{\"mean\":{},\"ci\":{},\"min\":{},\"max\":{}}}",
                g.name,
                f(g.mean),
                f(g.ci.half_width),
                f(g.min),
                f(g.max)
            );
        }
        s.push('}');
        if !self.claims.is_empty() {
            s.push_str(",\"claims\":[");
            for (i, c) in self.claims.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                let _ = write!(
                    s,
                    "{{\"name\":{},\"kind\":\"{}\",\"result\":\"{}\",\"checked\":{},\"failures\":{},\"first\":{},\"note\":{}}}",
                    json_string(&c.name),
                    c.kind_name(),
                    c.result.name(),
                    c.checked,
                    c.failures,
                    c.first.map_or("null".into(), f),
                    c.note.as_deref().map_or("null".into(), json_string)
                );
            }
            s.push(']');
        }
        s.push_str(",\"stages\":[");
        for (i, st) in self.stages.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":\"{}\",\"index\":{},\"mean_number\":{},\"utilization\":{},\"completed\":{},\"throughput\":{},\"mean_wait\":{},\"mean_service\":{},\"iterations\":{},\"prefill_only\":{},\"decode_only\":{},\"mixed\":{},\"mean_decodes\":{},\"mean_decode_batch\":{},\"mean_decode_step\":{},\"mean_itl\":{},\"itl_p50\":{},\"itl_p99\":{},\"idle_with_work\":{}}}",
                st.name,
                index(st.index),
                f(st.mean_number),
                f(st.utilization),
                st.completed,
                f(st.throughput),
                f(st.mean_wait),
                f(st.mean_service),
                st.iterations,
                f(st.prefill_only),
                f(st.decode_only),
                f(st.mixed),
                f(st.mean_decodes),
                f(st.mean_decode_batch),
                f(st.mean_decode_step),
                f(st.mean_itl),
                f(st.itl_p50),
                f(st.itl_p99),
                st.idle_with_work
            );
        }
        s.push_str("],\"pools\":[");
        for (i, p) in self.pools.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":\"{}\",\"index\":{},\"mean_used\":{},\"mean_cached\":{},\"mean_queue\":{},\"mean_holders\":{},\"mean_wait\":{},\"admissions\":{},\"evicted_entries\":{},\"evicted_units\":{},\"preemptions\":{},\"spills\":{},\"rejected\":{},\"stuck\":{},\"over_cap\":{}}}",
                p.name,
                index(p.index),
                f(p.mean_used),
                f(p.mean_cached),
                f(p.mean_queue),
                f(p.mean_holders),
                f(p.mean_wait),
                p.admissions,
                p.evicted_entries,
                f(p.evicted_units),
                p.preemptions,
                p.spills,
                p.rejected,
                p.stuck,
                match &p.over_cap {
                    Some((queue, need)) =>
                        format!("{{\"queue\":\"{queue}\",\"need\":{}}}", f(*need)),
                    None => "null".into(),
                }
            );
        }
        s.push_str("]}");
        s
    }
}

/// A row's name: `kvD[0]` for an array member, `kv` otherwise.
fn label(name: &str, index: Option<u32>) -> String {
    match index {
        Some(i) => format!("{name}[{i}]"),
        None => name.to_string(),
    }
}

/// One section of the text report: each column as wide as its widest cell,
/// the name column left-aligned and the numbers right-aligned, a rule under
/// the header, a blank line above. The width is measured in `char`s, so `±` counts as one.
fn table(s: &mut String, header: &[&str], rows: impl Iterator<Item = Vec<String>>) {
    table_aligned(s, header, rows, false);
}

/// A string as JSON writes it.
fn json_string(x: &str) -> String {
    serde_json::to_string(x).expect("a string serialises")
}

/// `table`, every column left-aligned when `left` (a column of words).
fn table_aligned(
    s: &mut String,
    header: &[&str],
    rows: impl Iterator<Item = Vec<String>>,
    left: bool,
) {
    let rows: Vec<Vec<String>> = rows.collect();
    let width: Vec<usize> = (0..header.len())
        .map(|c| {
            rows.iter()
                .map(|r| r[c].chars().count())
                .chain([header[c].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    s.push('\n');
    let line = |s: &mut String, cells: &[&str]| {
        let mut l = String::new();
        for (c, (cell, w)) in cells.iter().zip(&width).enumerate() {
            if c == 0 {
                let _ = write!(l, "{cell:<w$}");
            } else if left {
                let _ = write!(l, "  {cell:<w$}");
            } else {
                let _ = write!(l, "  {cell:>w$}");
            }
        }
        let _ = writeln!(s, "{}", l.trim_end());
    };
    line(s, header);
    let rule: Vec<String> = width.iter().map(|w| "-".repeat(*w)).collect();
    line(s, &rule.iter().map(String::as_str).collect::<Vec<_>>());
    for r in &rows {
        line(s, &r.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_name_or_a_wide_number_keeps_the_columns_aligned() {
        let mut s = String::new();
        table(
            &mut s,
            &["observe", "count", "95% CI"],
            [
                vec!["prefill_tokens".into(), "821".into(), "±38.6706".into()],
                vec!["hit".into(), "1234567".into(), "±0.0221".into()],
            ]
            .into_iter(),
        );
        assert_eq!(
            s,
            "\n\
             observe           count    95% CI\n\
             --------------  -------  --------\n\
             prefill_tokens      821  ±38.6706\n\
             hit             1234567   ±0.0221\n"
        );
    }
}
