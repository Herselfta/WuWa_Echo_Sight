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

#[cfg(test)]
mod tests {
    use super::{bayes_interval, expected_tier_adjacency_ratios_for_stat, wilson_interval};

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
