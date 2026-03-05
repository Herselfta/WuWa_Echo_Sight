use std::collections::{HashMap, HashSet};

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use statrs::distribution::{Beta, ContinuousCDF, Normal};

// Baseline tier distributions are approximate priors distilled from
// docs/echo-system-baseline.md (non-official, strategy-context only).
const CRIT_LOW_BIASED_8: [f64; 8] = [0.30, 0.25, 0.20, 0.12, 0.07, 0.04, 0.015, 0.005];
const MID_BIASED_8: [f64; 8] = [0.05, 0.12, 0.18, 0.20, 0.20, 0.17, 0.05, 0.03];
const MID_BIASED_4: [f64; 4] = [0.08, 0.44, 0.40, 0.08];

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

/// Blend baseline uniform prior and observed frequencies into per-stat probabilities.
///
/// - `baseline_blend`: 1.0 means fully baseline-uniform; 0.0 means fully data-driven.
/// - `smoothing_alpha`: Laplace smoothing strength for observed frequencies.
pub fn blended_stat_probabilities(
    stat_keys: &[String],
    count_map: &HashMap<String, i64>,
    baseline_blend: f64,
    smoothing_alpha: f64,
) -> HashMap<String, f64> {
    if stat_keys.is_empty() {
        return HashMap::new();
    }

    let k = stat_keys.len() as f64;
    let blend = baseline_blend.clamp(0.0, 1.0);
    let alpha = smoothing_alpha.max(0.0);
    let baseline_p = 1.0 / k;

    let observed_total: f64 = stat_keys
        .iter()
        .map(|key| (*count_map.get(key).unwrap_or(&0)).max(0) as f64)
        .sum();
    let observed_denom = observed_total + alpha * k;

    stat_keys
        .iter()
        .map(|key| {
            let count = (*count_map.get(key).unwrap_or(&0)).max(0) as f64;
            let observed_p = if observed_denom > 0.0 {
                (count + alpha) / observed_denom
            } else {
                baseline_p
            };
            let blended = blend * baseline_p + (1.0 - blend) * observed_p;
            (key.clone(), blended.max(1e-12))
        })
        .collect()
}

pub fn tier_baseline_probs_for_stat(stat_key: &str) -> Option<&'static [f64]> {
    match stat_key {
        "crit_rate" | "crit_dmg" => Some(&CRIT_LOW_BIASED_8),
        "atk_flat" | "def_flat" => Some(&MID_BIASED_4),
        "atk_pct" | "def_pct" | "hp_pct" | "hp_flat" | "energy_regen" | "basic_dmg"
        | "heavy_dmg" | "skill_dmg" | "liberation_dmg" => Some(&MID_BIASED_8),
        _ => None,
    }
}

/// Returns expected ratios for `(stop, step, jump)` under independent redraws
/// from the baseline tier distribution of this stat.
pub fn expected_tier_adjacency_ratios_for_stat(stat_key: &str) -> Option<(f64, f64, f64)> {
    tier_baseline_probs_for_stat(stat_key).and_then(expected_tier_adjacency_ratios)
}

fn expected_tier_adjacency_ratios(probs: &[f64]) -> Option<(f64, f64, f64)> {
    if probs.len() < 2 {
        return None;
    }
    let sum: f64 = probs.iter().sum();
    if sum <= 0.0 {
        return None;
    }

    let mut norm = vec![0.0; probs.len()];
    for (idx, p) in probs.iter().enumerate() {
        norm[idx] = *p / sum;
    }

    let mut stop = 0.0f64;
    let mut step = 0.0f64;
    for (idx, p) in norm.iter().enumerate() {
        stop += p * p;
        if idx > 0 {
            step += p * norm[idx - 1];
        }
        if idx + 1 < norm.len() {
            step += p * norm[idx + 1];
        }
    }
    let jump = (1.0 - stop - step).max(0.0);
    Some((stop, step, jump))
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

    use super::{
        bayes_interval, blended_stat_probabilities, expected_tier_adjacency_ratios_for_stat,
        monte_carlo_final_prob, wilson_interval,
    };

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

    #[test]
    fn blended_stat_probabilities_are_normalized() {
        let stat_keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let counts = HashMap::from([
            ("a".to_string(), 10_i64),
            ("b".to_string(), 20_i64),
            ("c".to_string(), 30_i64),
        ]);
        let probs = blended_stat_probabilities(&stat_keys, &counts, 0.65, 1.0);
        let s: f64 = probs.values().sum();
        assert!((s - 1.0).abs() < 1e-9);
        for p in probs.values() {
            assert!(*p > 0.0);
        }
    }

    #[test]
    fn tier_adjacency_baseline_ratios_are_valid() {
        let (stop, step, jump) = expected_tier_adjacency_ratios_for_stat("crit_rate")
            .expect("crit_rate should have baseline");
        assert!(stop > 0.0 && step > 0.0 && jump > 0.0);
        assert!((stop + step + jump - 1.0).abs() < 1e-9);

        let (stop4, step4, jump4) = expected_tier_adjacency_ratios_for_stat("atk_flat")
            .expect("atk_flat should have baseline");
        assert!((stop4 + step4 + jump4 - 1.0).abs() < 1e-9);
    }
}
