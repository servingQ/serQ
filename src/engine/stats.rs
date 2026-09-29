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
