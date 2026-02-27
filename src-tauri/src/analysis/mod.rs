use std::collections::{HashMap, HashSet};

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use statrs::distribution::{Beta, ContinuousCDF, Normal};

pub fn wilson_interval(success: i64, total: i64, confidence: f64) -> (f64, f64) {
    if total <= 0 {
        return (0.0, 0.0);
    }
    let n = total as f64;
    let phat = success as f64 / n;
    let z = normal_quantile(0.5 + confidence / 2.0);
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = (phat + z2 / (2.0 * n)) / denom;
    let margin = (z / denom) * ((phat * (1.0 - phat) / n + z2 / (4.0 * n * n)).sqrt());
    ((center - margin).max(0.0), (center + margin).min(1.0))
}

pub fn bayes_interval(success: i64, total: i64, confidence: f64) -> (f64, f64, f64) {
    let alpha = success as f64 + 0.5;
    let beta = (total - success).max(0) as f64 + 0.5;
    let dist = Beta::new(alpha, beta).expect("beta params should always be positive");
    let tail = (1.0 - confidence) / 2.0;
    let low = dist.inverse_cdf(tail);
    let high = dist.inverse_cdf(1.0 - tail);
    let mean = alpha / (alpha + beta);
    (mean, low, high)
}

pub fn normal_quantile(p: f64) -> f64 {
    let normal = Normal::new(0.0, 1.0).expect("std normal should initialize");
    normal.inverse_cdf(p.clamp(1e-12, 1.0 - 1e-12))
}

pub fn monte_carlo_final_prob(
    all_stats: &[String],
    owned_stats: &HashSet<String>,
    target_stat: &str,
    weight_map: &HashMap<String, f64>,
    remaining_draws: i64,
    iterations: usize,
    seed: u64,
) -> f64 {
    if owned_stats.contains(target_stat) {
        return 1.0;
    }
    if remaining_draws <= 0 {
        return 0.0;
    }

    let candidates: Vec<String> = all_stats
        .iter()
        .filter(|s| !owned_stats.contains(*s))
        .cloned()
        .collect();
    if !candidates.iter().any(|s| s == target_stat) {
        return 0.0;
    }

    let draws = remaining_draws.min(candidates.len() as i64) as usize;
    if draws == 0 {
        return 0.0;
    }

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut hits = 0usize;

    for _ in 0..iterations {
        let mut local = candidates.clone();
        let mut hit = false;

        for _ in 0..draws {
            if local.is_empty() {
                break;
            }
            let total_weight: f64 = local
                .iter()
                .map(|k| *weight_map.get(k).unwrap_or(&1.0))
                .sum();
            if total_weight <= 0.0 {
                break;
            }

            let mut r = rng.gen::<f64>() * total_weight;
            let mut chosen_idx = 0usize;
            for (idx, key) in local.iter().enumerate() {
                r -= *weight_map.get(key).unwrap_or(&1.0);
                if r <= 0.0 {
                    chosen_idx = idx;
                    break;
                }
                chosen_idx = idx;
            }

            let chosen = local.swap_remove(chosen_idx);
            if chosen == target_stat {
                hit = true;
                break;
            }
        }

        if hit {
            hits += 1;
        }
    }

    hits as f64 / iterations as f64
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{bayes_interval, monte_carlo_final_prob, wilson_interval};

    #[test]
    fn intervals_are_in_unit_range() {
        let (low, high) = wilson_interval(20, 100, 0.95);
        assert!(low >= 0.0 && high <= 1.0);
        assert!(low <= high);

        let (mean, bayes_low, bayes_high) = bayes_interval(20, 100, 0.95);
        assert!(mean >= 0.0 && mean <= 1.0);
        assert!(bayes_low >= 0.0 && bayes_high <= 1.0);
        assert!(bayes_low <= bayes_high);
    }

    #[test]
    fn monte_carlo_returns_zero_when_target_not_available() {
        let all_stats = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let owned: HashSet<String> = ["a".to_string(), "b".to_string(), "c".to_string()]
            .into_iter()
            .collect();
        let weights = HashMap::from([
            ("a".to_string(), 1.0),
            ("b".to_string(), 1.0),
            ("c".to_string(), 1.0),
        ]);

        let p = monte_carlo_final_prob(&all_stats, &owned, "a", &weights, 2, 1_000, 7);
        assert_eq!(p, 1.0);

        let p_missing = monte_carlo_final_prob(&all_stats, &owned, "d", &weights, 2, 1_000, 7);
        assert_eq!(p_missing, 0.0);
    }
}
