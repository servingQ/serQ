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
pub struct StageReport {
    pub name: String,
    /// Time-average jobs present (waiting and in service).
    pub mean_number: f64,
    /// Fraction of time with at least one job present.
    pub utilization: f64,
    pub completed: u64,
    pub throughput: f64,
    pub mean_wait: f64,
    pub mean_service: f64,
    /// Step stages: iterations run (0 otherwise).
    pub iterations: u64,
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub struct Report {
    pub horizon: f64,
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
            "run: horizon {} warmup {} seed {} events {} arrivals {} ended {} turns {} mean live {:.3}",
            self.horizon,
            self.warmup,
            self.seed,
            self.events,
            self.arrivals,
            self.ended,
            self.turns,
            self.mean_live
        );
        if !self.observes.is_empty() {
            let _ = writeln!(
                s,
                "observe        count        mean      95% CI      cv2       p99"
            );
            for o in &self.observes {
                let _ = writeln!(
                    s,
                    "  {:<12} {:>6} {:>11.4} ±{:<9.4} {:>7.3} {:>9.4}",
                    o.name, o.count, o.mean, o.ci.half_width, o.cv2, o.p99
                );
            }
        }
        if !self.stages.is_empty() {
            let _ = writeln!(
                s,
                "stage          number   util    done   thru      wait   service  iters"
            );
            for st in &self.stages {
                let _ = writeln!(
                    s,
                    "  {:<12} {:>7.3} {:>6.3} {:>7} {:>7.4} {:>9.4} {:>9.4} {:>6}",
                    st.name,
                    st.mean_number,
                    st.utilization,
                    st.completed,
                    st.throughput,
                    st.mean_wait,
                    st.mean_service,
                    st.iterations
                );
            }
        }
        if !self.pools.is_empty() {
            let _ = writeln!(
                s,
                "pool             used     cached  queue holders    wait  admits evict(n)  evict(u) preempt spill rej stuck"
            );
            for p in &self.pools {
                let _ = writeln!(
                    s,
                    "  {:<12} {:>9.1} {:>9.1} {:>6.3} {:>7.3} {:>7.4} {:>7} {:>8} {:>9.0} {:>7} {:>5} {:>3} {:>5}",
                    p.name,
                    p.mean_used,
                    p.mean_cached,
                    p.mean_queue,
                    p.mean_holders,
                    p.mean_wait,
                    p.admissions,
                    p.evicted_entries,
                    p.evicted_units,
                    p.preemptions,
                    p.spills,
                    p.rejected,
                    p.stuck
                );
            }
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
            "\"horizon\":{},\"warmup\":{},\"seed\":{},\"events\":{},\"arrivals\":{},\"ended\":{},\"turns\":{},\"mean_live\":{}",
            f(self.horizon),
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
