//! Percentile summaries. A measurement is never reported as an average alone.

/// A percentile summary over a non-empty sample set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub samples: usize,
    pub min: f64,
    pub max: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

/// Sorts samples and computes a linear-interpolated percentile.
///
/// The interpolation follows the common `(n - 1) * p / 100` definition, so
/// p50 of two samples is their mean and p50 of 1..=100 is 50.5. Returning
/// `None` for an empty sample set keeps unavailable measurements explicit.
pub fn summarize(samples: &mut [f64]) -> Option<Stats> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    Some(Stats {
        samples: samples.len(),
        min: samples[0],
        max: samples[samples.len() - 1],
        p50: percentile(samples, 50.0),
        p95: percentile(samples, 95.0),
        p99: percentile(samples, 99.0),
    })
}

/// Computes one percentile from an already sorted, non-empty slice.
pub fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let percentile = percentile.clamp(0.0, 100.0);
    let position = (sorted.len() - 1) as f64 * percentile / 100.0;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = position - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(values: &[f64]) -> Stats {
        let mut values = values.to_vec();
        summarize(&mut values).expect("non-empty samples")
    }

    #[test]
    fn small_sample_percentiles_are_exact() {
        let stats = summary(&[10.0, 20.0]);
        assert_eq!(stats.samples, 2);
        assert_eq!(stats.p50, 15.0);
        assert_eq!(stats.p95, 19.5);
        assert!((stats.p99 - 19.9).abs() < 1e-9);
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 20.0);

        let single = summary(&[7.0]);
        assert_eq!(single.p50, 7.0);
        assert_eq!(single.p95, 7.0);
        assert_eq!(single.p99, 7.0);
    }

    #[test]
    fn large_sample_percentiles_use_interpolation() {
        let values: Vec<f64> = (1..=100).map(|value| value as f64).collect();
        let stats = summary(&values);
        assert_eq!(stats.samples, 100);
        assert_eq!(stats.p50, 50.5);
        assert!((stats.p95 - 95.05).abs() < 1e-9);
        assert!((stats.p99 - 99.01).abs() < 1e-9);
    }

    #[test]
    fn unsorted_input_is_sorted_and_duplicates_survive() {
        let stats = summary(&[30.0, 10.0, 20.0, 20.0]);
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 30.0);
        assert_eq!(stats.p50, 20.0);
        assert_eq!(stats.samples, 4);
    }

    #[test]
    fn empty_input_is_unavailable() {
        let mut empty: Vec<f64> = Vec::new();
        assert!(summarize(&mut empty).is_none());
    }
}
