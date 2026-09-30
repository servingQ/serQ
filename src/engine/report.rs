//! What a run reports: `observe` statistics, per-stage and per-pool
//! time averages and counters. Printed as text or JSON.

use std::fmt::Write as _;

use crate::engine::stats::Estimate;

#[derive(Clone, Debug)]
pub struct ObserveReport {
    pub name: String,
    pub count: u64,
    pub mean: f64,
    pub cv2: f64,
    /// Batch-means 95 % CI (NaN below 40 samples).
    pub ci: Estimate,
    pub p99: f64,
    pub samples: Vec<f64>,
    /// `(time, session serial, turn number)` of every sample.
    pub records: Vec<(f64, u64, u32)>,
}

#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(get_all, frozen, name = "Stage", module = "pyserq")
)]
pub struct StageReport {
    pub name: String,
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
}

#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(get_all, frozen, name = "Pool", module = "pyserq")
)]
pub struct PoolReport {
    pub name: String,
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
    pub stages: Vec<StageReport>,
    pub pools: Vec<PoolReport>,
}

impl Report {
    /// Write every observation as `observe/<name>.csv` with columns
    /// `time,session,turn,value` into `dir`.
    pub fn dump(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for o in &self.observes {
            let mut s = String::from("time,session,turn,value\n");
            for (rec, v) in o.records.iter().zip(&o.samples) {
                s.push_str(&format!("{},{},{},{}\n", rec.0, rec.1, rec.2, v));
            }
            std::fs::write(dir.join(format!("{}.csv", o.name)), s)?;
        }
        Ok(())
    }

    pub fn observe(&self, name: &str) -> Option<&ObserveReport> {
        self.observes.iter().find(|o| o.name == name)
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
        }
        if !self.stages.is_empty() {
            let rows = self.stages.iter().map(|st| {
                vec![
                    st.name.clone(),
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
        if !self.pools.is_empty() {
            let rows = self.pools.iter().map(|p| {
                vec![
                    p.name.clone(),
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
                        p.stuck, p.name
                    );
                }
            }
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
        let mut s = String::from("{");
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
        s.push_str("},\"stages\":[");
        for (i, st) in self.stages.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":\"{}\",\"mean_number\":{},\"utilization\":{},\"completed\":{},\"throughput\":{},\"mean_wait\":{},\"mean_service\":{},\"iterations\":{}}}",
                st.name,
                f(st.mean_number),
                f(st.utilization),
                st.completed,
                f(st.throughput),
                f(st.mean_wait),
                f(st.mean_service),
                st.iterations
            );
        }
        s.push_str("],\"pools\":[");
        for (i, p) in self.pools.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":\"{}\",\"mean_used\":{},\"mean_cached\":{},\"mean_queue\":{},\"mean_holders\":{},\"mean_wait\":{},\"admissions\":{},\"evicted_entries\":{},\"evicted_units\":{},\"preemptions\":{},\"spills\":{},\"rejected\":{},\"stuck\":{}}}",
                p.name,
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
                p.stuck
            );
        }
        s.push_str("]}");
        s
    }
}

/// One section of the text report: each column as wide as its widest cell,
/// the name column left-aligned and the numbers right-aligned, a rule under
/// the header, a blank line above. The width is measured in `char`s, so `±` counts as one.
fn table(s: &mut String, header: &[&str], rows: impl Iterator<Item = Vec<String>>) {
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
