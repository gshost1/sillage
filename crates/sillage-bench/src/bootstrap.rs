//! Seeded percentile bootstrap confidence intervals.

use rand::Rng;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;

use crate::Ci;

/// Arithmetic mean (`0.0` for an empty slice).
pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// Linear-interpolated percentile `p ∈ [0, 100]` of an ascending-sorted slice.
pub fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    match sorted.len() {
        0 => 0.0,
        1 => sorted[0],
        n => {
            let rank = (p.clamp(0.0, 100.0) / 100.0) * (n - 1) as f64;
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            let frac = rank - lo as f64;
            sorted[lo] + (sorted[hi] - sorted[lo]) * frac
        }
    }
}

/// Percentile of an unsorted slice.
pub fn percentile(xs: &[f64], p: f64) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    percentile_sorted(&v, p)
}

/// 95% percentile bootstrap CI of the mean: `n_resamples` resamples with
/// replacement drawn by `ChaCha8Rng(seed)`; `low`/`high` are the 2.5th and
/// 97.5th percentiles of the resampled means, `mean` the sample mean.
pub fn bootstrap_ci(samples: &[f64], n_resamples: usize, seed: u64) -> Ci {
    let m = mean(samples);
    if samples.is_empty() || n_resamples == 0 {
        return Ci {
            mean: m,
            low: m,
            high: m,
        };
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let n = samples.len();
    let mut means = Vec::with_capacity(n_resamples);
    for _ in 0..n_resamples {
        let mut acc = 0.0;
        for _ in 0..n {
            acc += samples[rng.random_range(0..n)];
        }
        means.push(acc / n as f64);
    }
    means.sort_by(f64::total_cmp);
    Ci {
        mean: m,
        low: percentile_sorted(&means, 2.5),
        high: percentile_sorted(&means, 97.5),
    }
}

/// Bootstrap CI over boolean hits (hit = 1.0).
pub fn bootstrap_hits(hits: &[bool], n_resamples: usize, seed: u64) -> Ci {
    let xs: Vec<f64> = hits.iter().map(|&h| if h { 1.0 } else { 0.0 }).collect();
    bootstrap_ci(&xs, n_resamples, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_distribution_has_zero_width() {
        let ci = bootstrap_ci(&[0.7; 40], 500, 7);
        for v in [ci.mean, ci.low, ci.high] {
            assert!((v - 0.7).abs() < 1e-9, "{v}");
        }
        assert_eq!(ci.low, ci.high);
    }

    #[test]
    fn bernoulli_half_ci_matches_normal_approx() {
        // 100 samples, p = 0.5 → SE = 0.05, 95% CI ≈ 0.5 ± 0.098.
        let hits: Vec<bool> = (0..100).map(|i| i % 2 == 0).collect();
        let ci = bootstrap_hits(&hits, 2000, 7);
        assert!((ci.mean - 0.5).abs() < 1e-12);
        assert!((ci.low - 0.402).abs() < 0.03, "low = {}", ci.low);
        assert!((ci.high - 0.598).abs() < 0.03, "high = {}", ci.high);
        assert!(ci.low < ci.mean && ci.mean < ci.high);
    }

    #[test]
    fn deterministic_per_seed() {
        let xs: Vec<f64> = (0..57).map(|i| (i * 37 % 11) as f64).collect();
        let a = bootstrap_ci(&xs, 300, 42);
        let b = bootstrap_ci(&xs, 300, 42);
        let c = bootstrap_ci(&xs, 300, 43);
        assert_eq!((a.low, a.high), (b.low, b.high));
        assert!((a.low, a.high) != (c.low, c.high));
    }

    #[test]
    fn degenerate_inputs() {
        let e = bootstrap_ci(&[], 100, 1);
        assert_eq!((e.mean, e.low, e.high), (0.0, 0.0, 0.0));
        let z = bootstrap_ci(&[1.0, 3.0], 0, 1);
        assert_eq!((z.mean, z.low, z.high), (2.0, 2.0, 2.0));
    }

    #[test]
    fn percentile_interpolates() {
        let xs = [4.0, 1.0, 3.0, 2.0, 5.0];
        assert_eq!(percentile(&xs, 50.0), 3.0);
        assert_eq!(percentile(&xs, 0.0), 1.0);
        assert_eq!(percentile(&xs, 100.0), 5.0);
        assert_eq!(percentile(&xs, 25.0), 2.0);
        assert!((percentile(&xs, 95.0) - 4.8).abs() < 1e-12);
    }
}
