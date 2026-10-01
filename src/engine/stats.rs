//! Running statistics (mirrors `libqueuingsim::stats`).

#[derive(Clone, Debug, Default)]
pub struct Welford {
    n: u64,
    mean: f64,
    m2: f64,
    sum_sq: f64,
}

impl Welford {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, x: f64) {
        self.n += 1;
        let d = x - self.mean;
        self.mean += d / self.n as f64;
        self.m2 += d * (x - self.mean);
        self.sum_sq += x * x;
    }

    pub fn count(&self) -> u64 {
        self.n
    }

    /// Number of observations (alias retained for queueing clients).
    pub fn n(&self) -> u64 {
        self.n
    }

    pub fn mean(&self) -> f64 {
        if self.n == 0 { f64::NAN } else { self.mean }
    }

    pub fn variance(&self) -> f64 {
        if self.n < 2 {
            f64::NAN
        } else {
            self.m2 / (self.n - 1) as f64
        }
    }

    /// Population variance `E[(X - E X)^2]`.
    ///
    /// `variance` is the sample variance used for confidence intervals;
    /// queueing formulas usually need this population moment instead.
    pub fn population_variance(&self) -> f64 {
        if self.n == 0 {
            f64::NAN
        } else {
            self.m2 / self.n as f64
        }
    }

    pub fn second_moment(&self) -> f64 {
        if self.n == 0 {
            f64::NAN
        } else {
            self.sum_sq / self.n as f64
        }
    }

    pub fn cv2(&self) -> f64 {
        self.variance() / (self.mean() * self.mean())
    }
}

/// Time average of a piecewise-constant signal.
#[derive(Clone, Debug)]
pub struct TimeAverage {
    value: f64,
    last: f64,
    integral: f64,
    start: f64,
}

impl TimeAverage {
    pub fn new(t0: f64, value: f64) -> Self {
        Self {
            value,
            last: t0,
            integral: 0.0,
            start: t0,
        }
    }

    pub fn set(&mut self, now: f64, value: f64) {
        self.integral += self.value * (now - self.last);
        self.last = now;
        self.value = value;
    }

    pub fn add(&mut self, now: f64, delta: f64) {
        let v = self.value + delta;
        self.set(now, v);
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn reset(&mut self, now: f64) {
        self.integral = 0.0;
        self.last = now;
        self.start = now;
    }

    pub fn mean(&self, now: f64) -> f64 {
        let span = now - self.start;
        if span <= 0.0 {
            return self.value;
        }
        (self.integral + self.value * (now - self.last)) / span
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub mean: f64,
    pub half_width: f64,
}

impl Estimate {
    pub fn lo(&self) -> f64 {
        self.mean - self.half_width
    }

    pub fn hi(&self) -> f64 {
        self.mean + self.half_width
    }

    /// Whether `x` lies in the interval widened by `rel` of `|x|`.
    pub fn agrees_with(&self, x: f64, rel: f64) -> bool {
        let slack = rel * x.abs();
        self.lo() - slack <= x && x <= self.hi() + slack
    }

    pub fn nan() -> Self {
        Estimate {
            mean: f64::NAN,
            half_width: f64::INFINITY,
        }
    }
}

impl std::fmt::Display for Estimate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.4} ± {:.4}", self.mean, self.half_width)
    }
}

/// Batch-means 95 % confidence interval (t-quantile for 19 degrees of
/// freedom when `batches = 20`; 2.093).
pub fn batch_means(xs: &[f64], batches: usize) -> Estimate {
    let size = xs.len() / batches;
    if batches < 2 || size < 1 {
        return Estimate::nan();
    }
    let means: Vec<f64> = xs
        .chunks_exact(size)
        .take(batches)
        .map(|c| c.iter().sum::<f64>() / size as f64)
        .collect();
    let k = means.len() as f64;
    let m = means.iter().sum::<f64>() / k;
    let var = means.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (k - 1.0);
    let t = if batches == 20 { 2.093 } else { 2.0 };
    Estimate {
        mean: m,
        half_width: t * (var / k).sqrt(),
    }
}

/// Statistics of a piecewise-constant signal over `[from, to]`, given as
/// change points `(t, v)` in time order (`v` holds from `t` to the next
/// point; a value before `from` holds into it): the time average with a
/// 95 % batch-means CI over `batches` equal windows, and the least and
/// greatest value held for a positive time.
pub fn time_stats(points: &[(f64, f64)], from: f64, to: f64, batches: usize) -> TimeStats {
    let span = to - from;
    let mut out = TimeStats {
        mean: f64::NAN,
        ci: Estimate::nan(),
        min: f64::NAN,
        max: f64::NAN,
    };
    if span.is_nan() || span <= 0.0 || points.is_empty() || batches == 0 {
        return out;
    }
    let width = span / batches as f64;
    let mut windows = vec![0.0; batches];
    let mut total = 0.0;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (k, &(t, v)) in points.iter().enumerate() {
        let next = points.get(k + 1).map_or(to, |p| p.0);
        let (a, b) = (t.max(from), next.min(to));
        if b <= a {
            continue;
        }
        lo = lo.min(v);
        hi = hi.max(v);
        total += v * (b - a);
        // add the segment's overlap with every window it can touch; the
        // window bounds are computed, not stepped to, so no rounding stops
        // the walk early
        let at = |x: f64| (((x - from) / width).max(0.0) as usize).min(batches - 1);
        for (w, acc) in windows
            .iter_mut()
            .enumerate()
            .take(at(b) + 2)
            .skip(at(a).saturating_sub(1))
        {
            let w_lo = if w == 0 {
                from
            } else {
                from + w as f64 * width
            };
            let w_hi = if w == batches - 1 {
                to
            } else {
                from + (w + 1) as f64 * width
            };
            let overlap = b.min(w_hi) - a.max(w_lo);
            if overlap > 0.0 {
                *acc += v * overlap;
            }
        }
    }
    out.mean = total / span;
    out.min = lo;
    out.max = hi;
    if batches >= 2 {
        let means: Vec<f64> = windows.iter().map(|x| x / width).collect();
        out.ci = batch_means(&means, batches);
        // the windows' average is the time average; keep the exact one
        out.ci.mean = out.mean;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeStats {
    pub mean: f64,
    pub ci: Estimate,
    pub min: f64,
    pub max: f64,
}

/// A distribution kept as counts in buckets of a constant relative width
/// (1 %), so that it costs the range of its values, not their number: a
/// quantile is its bucket's geometric middle, within 0.5 % of the values
/// in that bucket. The mean is exact.
#[derive(Clone, Debug, Default)]
pub struct LogHistogram {
    counts: std::collections::BTreeMap<i32, u64>,
    n: u64,
    sum: f64,
}

impl LogHistogram {
    const WIDTH: f64 = 0.01;

    pub fn push(&mut self, x: f64) {
        // zero and below share one bucket, below every other
        let k = if x > 0.0 {
            (x.ln() / Self::WIDTH.ln_1p()).floor() as i32
        } else {
            i32::MIN
        };
        *self.counts.entry(k).or_default() += 1;
        self.n += 1;
        self.sum += x;
    }

    pub fn mean(&self) -> f64 {
        if self.n == 0 {
            f64::NAN
        } else {
            self.sum / self.n as f64
        }
    }

    /// The bucket of the `ceil(q n)`-th smallest value, as `quantile` ranks.
    pub fn quantile(&self, q: f64) -> f64 {
        if self.n == 0 {
            return f64::NAN;
        }
        let rank = ((q * self.n as f64).ceil() as u64).clamp(1, self.n);
        let mut seen = 0;
        for (&k, &c) in &self.counts {
            seen += c;
            if seen >= rank {
                return if k == i32::MIN {
                    0.0
                } else {
                    ((k as f64 + 0.5) * Self::WIDTH.ln_1p()).exp()
                };
            }
        }
        unreachable!("the ranks add up to n")
    }
}

pub fn quantile(xs: &[f64], q: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let idx = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[idx]
}

/// Online estimates of the quantities in the price of a miss, as a
/// replica would observe them (mirrors `libqueuingsim`'s
/// `PriceEstimator`): exponentially weighted averages of the gap between
/// service starts (`λ̂` = 1 / mean gap), the service time and the queue
/// wait, with `ρ̂ = min(λ̂ ŝ, 0.99)`.
#[derive(Clone, Debug, Default)]
pub struct PriceEstimator {
    last_start: Option<f64>,
    gap: Option<f64>,
    service: f64,
    wait: f64,
    seen: u64,
}

impl PriceEstimator {
    pub const ALPHA: f64 = 0.01;
    pub const RHO_CAP: f64 = 0.99;

    fn ewma(old: f64, x: f64, first: bool) -> f64 {
        if first {
            x
        } else {
            (1.0 - Self::ALPHA) * old + Self::ALPHA * x
        }
    }

    pub fn observe(&mut self, start: f64, wait: f64, service: f64) {
        let first = self.seen == 0;
        self.service = Self::ewma(self.service, service, first);
        self.wait = Self::ewma(self.wait, wait, first);
        if let Some(t) = self.last_start {
            self.gap = Some(match self.gap {
                None => start - t,
                Some(g) => Self::ewma(g, start - t, false),
            });
        }
        self.last_start = Some(start);
        self.seen += 1;
    }

    /// `(λ̂, ρ̂, Ŵ)`.
    pub fn estimates(&self) -> (f64, f64, f64) {
        match self.gap {
            Some(g) if g > 0.0 => {
                let lam = 1.0 / g;
                (lam, (lam * self.service).min(Self::RHO_CAP), self.wait)
            }
            _ => (0.0, 0.0, self.wait),
        }
    }

    /// Price of a miss that lengthens a hit service `s_h` by `ds`
    /// (`missPrice` with the measured wait):
    /// `Φ = ds + λ(s_m² - s_h²)/(2(1-ρ)) + λ W ds/(1-ρ)`.
    pub fn price(&self, s_h: f64, ds: f64) -> f64 {
        let (lam, rho, w) = self.estimates();
        let s_m = s_h + ds;
        ds + lam * (s_m * s_m - s_h * s_h) / (2.0 * (1.0 - rho)) + lam * w * ds / (1.0 - rho)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_average_of_a_step() {
        let mut t = TimeAverage::new(0.0, 0.0);
        t.set(1.0, 2.0);
        t.set(3.0, 0.0);
        assert!((t.mean(4.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn time_stats_of_a_step() {
        // 0 on [0, 1), 2 on [1, 3), 5 from 3; measured on [1, 4]
        let ts = time_stats(&[(0.0, 0.0), (1.0, 2.0), (3.0, 5.0)], 1.0, 4.0, 20);
        assert!((ts.mean - 3.0).abs() < 1e-12);
        assert_eq!((ts.min, ts.max), (2.0, 5.0));
        // a value held for no time is not the extreme
        let ts = time_stats(&[(0.0, 1.0), (2.0, 9.0), (2.0, 1.0)], 0.0, 4.0, 20);
        assert_eq!(ts.max, 1.0);
        // a constant measured over a span whose windows do not divide it
        // exactly: every window gets its share, so the CI is zero (it was
        // 17 empty windows when a rounded bound stopped the walk)
        let ts = time_stats(&[(0.0, 3.0)], 0.0, 7.0, 20);
        assert!(ts.ci.half_width < 1e-12, "{}", ts.ci.half_width);
        assert!((ts.mean - 3.0).abs() < 1e-12);
        // no windows: no statistics, and no panic
        assert!(time_stats(&[(0.0, 1.0)], 0.0, 1.0, 0).mean.is_nan());
    }

    #[test]
    fn welford_moments() {
        let mut w = Welford::new();
        for x in [1.0, 2.0, 3.0, 4.0] {
            w.push(x);
        }
        assert!((w.mean() - 2.5).abs() < 1e-12);
        assert!((w.variance() - 5.0 / 3.0).abs() < 1e-12);
        assert_eq!(w.n(), w.count());
        assert!((w.population_variance() - 1.25).abs() < 1e-12);
    }
}
