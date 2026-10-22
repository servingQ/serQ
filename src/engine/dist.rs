//! Reusable probability distributions for simulator inputs.
//!
//! The language's `~exp`, `~erlang`, `~h2`, `~uniform`, and
//! `~bernoulli` expressions use these same sampling laws. Exact moments make
//! the laws useful to queueing analyses as well as to serQ programs.

use rand::Rng;

use crate::ir::DistKind;

#[derive(Clone, Debug, PartialEq)]
pub enum Dist {
    /// Always `x`.
    Deterministic(f64),
    /// Exponential with the given mean.
    Exponential { mean: f64 },
    /// Sum of `k` i.i.d. exponentials, total mean `mean` (CV² = 1/k).
    Erlang { k: u32, mean: f64 },
    /// With probability `p`, Exp(`mean1`); otherwise Exp(`mean2`).
    HyperExp { p: f64, mean1: f64, mean2: f64 },
    /// Uniform on `[lo, hi]`.
    Uniform { lo: f64, hi: f64 },
    /// Finite discrete distribution with matching values and probabilities.
    Discrete { values: Vec<f64>, probs: Vec<f64> },
    /// A hit/miss mixture with deterministic service times.
    HitMiss { p_hit: f64, hit: f64, miss: f64 },
    /// Bernoulli distribution, yielding 1 with probability `p` and 0 otherwise.
    Bernoulli { p: f64 },
}

impl Dist {
    pub fn exp(mean: f64) -> Self {
        Self::Exponential { mean }
    }

    /// Two-phase balanced hyperexponential with the requested mean and CV².
    pub fn hyperexp_balanced(mean: f64, cv2: f64) -> Self {
        assert!(cv2 >= 1.0, "hyperexponential needs CV² ≥ 1, got {cv2}");
        let p = 0.5 * (1.0 + ((cv2 - 1.0) / (cv2 + 1.0)).sqrt());
        Self::HyperExp {
            p,
            mean1: mean / (2.0 * p),
            mean2: mean / (2.0 * (1.0 - p)),
        }
    }

    pub fn discrete(values: Vec<f64>, probs: Vec<f64>) -> Self {
        assert_eq!(values.len(), probs.len());
        assert!(
            !values.is_empty(),
            "a discrete distribution cannot be empty"
        );
        let total: f64 = probs.iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "probabilities sum to {total}");
        assert!(probs.iter().all(|&p| p >= 0.0));
        Self::Discrete { values, probs }
    }

    pub fn bernoulli(p: f64) -> Self {
        Self::Bernoulli { p }
    }

    /// The law of a draw `~name(a…)` whose parameters are `a`, as the
    /// language reads them (`docs/api/distributions.md`): `~erlang`'s phase
    /// count rounded down and at least 1, `~h2`'s CV² at least 1.
    pub fn of(kind: DistKind, a: &[f64]) -> Self {
        match kind {
            DistKind::Det => Self::Deterministic(a[0]),
            DistKind::Exp => Self::exp(a[0]),
            DistKind::Uniform => Self::Uniform { lo: a[0], hi: a[1] },
            DistKind::Erlang => Self::Erlang {
                k: a[0].max(1.0) as u32,
                mean: a[1],
            },
            DistKind::H2 => Self::hyperexp_balanced(a[0], a[1].max(1.0)),
            DistKind::Bernoulli => Self::bernoulli(a[0]),
        }
    }

    /// Whether the law is an exponential: an Erlang of one phase is, and so
    /// is a hyperexponential of one phase or of two with one mean, as
    /// `~h2` of CV² 1 is.
    pub fn is_exponential(&self) -> bool {
        match self {
            Self::Exponential { .. } => true,
            Self::Erlang { k, .. } => *k <= 1,
            Self::HyperExp { p, mean1, mean2 } => mean1 == mean2 || *p == 0.0 || *p == 1.0,
            _ => false,
        }
    }

    pub fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        match self {
            Self::Deterministic(x) => *x,
            Self::Exponential { mean } => sample_exp(rng, *mean),
            Self::Erlang { k, mean } => {
                let k = (*k).max(1);
                let phase_mean = mean / k as f64;
                (0..k).map(|_| sample_exp(rng, phase_mean)).sum()
            }
            Self::HyperExp { p, mean1, mean2 } => {
                let mean = if rng.random::<f64>() < *p {
                    *mean1
                } else {
                    *mean2
                };
                sample_exp(rng, mean)
            }
            Self::Uniform { lo, hi } => lo + (hi - lo) * rng.random::<f64>(),
            Self::Discrete { values, probs } => {
                let u: f64 = rng.random();
                let mut acc = 0.0;
                for (value, probability) in values.iter().zip(probs) {
                    acc += probability;
                    if u < acc {
                        return *value;
                    }
                }
                *values.last().expect("validated non-empty distribution")
            }
            Self::HitMiss { p_hit, hit, miss } => {
                if rng.random::<f64>() < *p_hit {
                    *hit
                } else {
                    *miss
                }
            }
            Self::Bernoulli { p } => f64::from(rng.random::<f64>() < *p),
        }
    }

    pub fn mean(&self) -> f64 {
        match self {
            Self::Deterministic(x) => *x,
            Self::Exponential { mean } | Self::Erlang { mean, .. } => *mean,
            Self::HyperExp { p, mean1, mean2 } => p * mean1 + (1.0 - p) * mean2,
            Self::Uniform { lo, hi } => 0.5 * (lo + hi),
            Self::Discrete { values, probs } => values.iter().zip(probs).map(|(v, p)| p * v).sum(),
            Self::HitMiss { p_hit, hit, miss } => p_hit * hit + (1.0 - p_hit) * miss,
            Self::Bernoulli { p } => *p,
        }
    }

    /// `E[X²]`.
    pub fn second_moment(&self) -> f64 {
        match self {
            Self::Deterministic(x) => x * x,
            Self::Exponential { mean } => 2.0 * mean * mean,
            Self::Erlang { k, mean } => {
                let k = (*k).max(1) as f64;
                mean * mean * (1.0 + 1.0 / k)
            }
            Self::HyperExp { p, mean1, mean2 } => {
                2.0 * (p * mean1 * mean1 + (1.0 - p) * mean2 * mean2)
            }
            Self::Uniform { lo, hi } => (lo * lo + lo * hi + hi * hi) / 3.0,
            Self::Discrete { values, probs } => {
                values.iter().zip(probs).map(|(v, p)| p * v * v).sum()
            }
            Self::HitMiss { p_hit, hit, miss } => p_hit * hit * hit + (1.0 - p_hit) * miss * miss,
            Self::Bernoulli { p } => *p,
        }
    }

    pub fn variance(&self) -> f64 {
        self.second_moment() - self.mean().powi(2)
    }

    pub fn cv2(&self) -> f64 {
        self.variance() / self.mean().powi(2)
    }
}

fn sample_exp<R: Rng + ?Sized>(rng: &mut R, mean: f64) -> f64 {
    // 1 - U is in (0, 1], so the logarithm stays finite.
    -mean * (1.0 - rng.random::<f64>()).ln()
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn moments_are_exact() {
        let cases = [
            (Dist::Deterministic(3.0), 3.0, 9.0),
            (Dist::exp(2.0), 2.0, 8.0),
            (Dist::Erlang { k: 4, mean: 2.0 }, 2.0, 5.0),
            (Dist::hyperexp_balanced(1.0, 4.0), 1.0, 5.0),
            (Dist::Uniform { lo: 1.0, hi: 3.0 }, 2.0, 13.0 / 3.0),
            (Dist::discrete(vec![1.0, 3.0], vec![0.25, 0.75]), 2.5, 7.0),
            (
                Dist::HitMiss {
                    p_hit: 0.8,
                    hit: 0.05,
                    miss: 0.5,
                },
                0.14,
                0.052,
            ),
            (Dist::bernoulli(0.25), 0.25, 0.25),
        ];
        for (d, mean, second_moment) in cases {
            assert!((d.mean() - mean).abs() < 1e-12, "{d:?}");
            assert!((d.second_moment() - second_moment).abs() < 1e-12, "{d:?}");
        }
    }

    #[test]
    fn seeded_sampling_matches_moments() {
        let dists = [
            Dist::exp(2.0),
            Dist::Erlang { k: 4, mean: 1.0 },
            Dist::hyperexp_balanced(1.0, 4.0),
            Dist::Uniform { lo: 1.0, hi: 3.0 },
            Dist::discrete(vec![100.0, 3700.0], vec![0.75, 0.25]),
            Dist::HitMiss {
                p_hit: 0.8,
                hit: 0.05,
                miss: 0.5,
            },
            Dist::bernoulli(0.25),
        ];
        for d in dists {
            let mut rng = StdRng::seed_from_u64(7);
            let mut sum = 0.0;
            let mut sum_sq = 0.0;
            let n = 400_000;
            for _ in 0..n {
                let x = d.sample(&mut rng);
                sum += x;
                sum_sq += x * x;
            }
            let mean = sum / n as f64;
            let second_moment = sum_sq / n as f64;
            assert!(
                (mean - d.mean()).abs() < 0.03 * d.mean().abs().max(1.0),
                "{d:?}"
            );
            assert!(
                (second_moment - d.second_moment()).abs() < 0.08 * d.second_moment().abs().max(1.0),
                "{d:?}: {second_moment} vs {}",
                d.second_moment()
            );
        }
    }
}
