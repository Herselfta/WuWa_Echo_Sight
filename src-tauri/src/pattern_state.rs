use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

const STAT_CATEGORIES: &[(&str, &str)] = &[
    ("atk_flat", "atk"),
    ("atk_pct", "atk"),
    ("def_flat", "def"),
    ("def_pct", "def"),
    ("hp_flat", "hp"),
    ("hp_pct", "hp"),
    ("crit_rate", "crit"),
    ("crit_dmg", "crit"),
    ("energy_regen", "utility"),
    ("basic_dmg", "dmg_bonus"),
    ("heavy_dmg", "dmg_bonus"),
    ("skill_dmg", "dmg_bonus"),
    ("liberation_dmg", "dmg_bonus"),
];

pub const ZONE_LABELS: &[(&str, &[&str])] = &[
    ("攻防区", &["atk", "def"]),
    ("攻生区", &["atk", "hp"]),
    ("防生区", &["def", "hp"]),
    ("伤害加成区", &["dmg_bonus"]),
    ("共鸣区", &["dmg_bonus", "utility"]),
    ("爆区", &["crit"]),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternEventLite {
    pub stat_key: String,
    pub tier_index: i64,
    pub analysis_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceStateFeatures {
    pub active_stat_count_recent8: i64,
    pub active_stat_count_recent12: i64,
    pub active_stat_bucket: String,
    pub zone_candidate: String,
    pub zone_confidence: f64,
    pub out_of_zone_streak: i64,
    pub crit_signal: String,
    pub tier_signal: String,
    pub regime_stage: String,
    pub regime_shift_score: f64,
    pub dominant_category_recent4: String,
    pub dominant_category_recent8: String,
    pub current_category_run_len: i64,
    pub reversion_top_stats: Vec<String>,
    #[serde(skip)]
    pub reversion_score_by_stat: HashMap<String, f64>,
}

fn dominant_category(events: &[PatternEventLite]) -> (String, f64) {
    if events.is_empty() {
        return ("mixed".to_string(), 0.0);
    }
    let mut counts = HashMap::<String, i64>::new();
    for event in events {
        let category = stat_category(&event.stat_key).to_string();
        *counts.entry(category).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(category, count)| (category, count as f64 / events.len() as f64))
        .unwrap_or_else(|| ("mixed".to_string(), 0.0))
}

pub fn stat_category(stat_key: &str) -> &'static str {
    STAT_CATEGORIES
        .iter()
        .find(|(key, _)| *key == stat_key)
        .map(|(_, category)| *category)
        .unwrap_or("unknown")
}

pub fn infer_zone(category: &str) -> Vec<&'static str> {
    ZONE_LABELS
        .iter()
        .filter(|(_, categories)| categories.contains(&category))
        .map(|(label, _)| *label)
        .collect()
}

pub fn stat_matches_zone(stat_key: &str, zone_label: &str) -> bool {
    let category = stat_category(stat_key);
    infer_zone(category).into_iter().any(|zone| zone == zone_label)
}

pub fn compute_reversion_scores(
    events: &[PatternEventLite],
    stat_keys: &[String],
) -> HashMap<String, f64> {
    let n = events.len();
    if n == 0 {
        return stat_keys
            .iter()
            .map(|stat_key| (stat_key.clone(), 0.0))
            .collect();
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut last_seen: HashMap<String, usize> = HashMap::new();
    for (idx, event) in events.iter().enumerate() {
        *counts.entry(event.stat_key.clone()).or_insert(0) += 1;
        last_seen.insert(event.stat_key.clone(), idx);
    }

    stat_keys
        .iter()
        .map(|stat_key| {
            let count = *counts.get(stat_key).unwrap_or(&0);
            if count == 0 {
                return (stat_key.clone(), 0.0);
            }
            let base_freq = count as f64 / n as f64;
            let expected_gap = if base_freq > 1e-9 { 1.0 / base_freq } else { n as f64 };
            let gap = last_seen
                .get(stat_key)
                .map(|idx| (n - 1).saturating_sub(*idx) as f64)
                .unwrap_or(n as f64);
            let score = ((gap + 1.0) / (expected_gap + 1.0)).ln().clamp(-0.35, 0.35);
            (stat_key.clone(), score)
        })
        .collect()
}

pub fn compute_sequence_state_features(
    events: &[PatternEventLite],
    stat_keys: &[String],
) -> SequenceStateFeatures {
    let recent8 = if events.len() > 8 {
        &events[events.len() - 8..]
    } else {
        events
    };
    let recent12 = if events.len() > 12 {
        &events[events.len() - 12..]
    } else {
        events
    };

    let active_stat_count_recent8 = recent8
        .iter()
        .map(|event| event.stat_key.clone())
        .collect::<HashSet<_>>()
        .len() as i64;
    let active_stat_count_recent12 = recent12
        .iter()
        .map(|event| event.stat_key.clone())
        .collect::<HashSet<_>>()
        .len() as i64;
    let active_stat_bucket = if active_stat_count_recent8 <= 2 {
        "low".to_string()
    } else if active_stat_count_recent8 <= 4 {
        "mid".to_string()
    } else {
        "high".to_string()
    };

    let mut zone_scores: HashMap<String, f64> = HashMap::new();
    let recent_len = recent8.len().max(1) as f64;
    for event in recent8 {
        let category = stat_category(&event.stat_key);
        for zone in infer_zone(category) {
            *zone_scores.entry(zone.to_string()).or_insert(0.0) += 1.0 / recent_len;
        }
    }
    let (best_zone, best_score) = zone_scores
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or_else(|| ("mixed".to_string(), 0.0));
    let zone_candidate = if best_score < 0.5 {
        "mixed".to_string()
    } else {
        best_zone
    };
    let zone_confidence = best_score.clamp(0.0, 1.0);

    let out_of_zone_streak = if zone_candidate == "mixed" {
        0
    } else {
        events
            .iter()
            .rev()
            .take_while(|event| !stat_matches_zone(&event.stat_key, &zone_candidate))
            .count() as i64
    };

    let crit_recent = recent8
        .iter()
        .filter(|event| stat_category(&event.stat_key) == "crit")
        .map(|event| event.stat_key.clone())
        .collect::<HashSet<_>>();
    let crit_signal = if crit_recent.len() >= 2 {
        "double".to_string()
    } else if crit_recent.len() == 1 {
        "single".to_string()
    } else {
        "none".to_string()
    };

    let recent4 = if events.len() > 4 {
        &events[events.len() - 4..]
    } else {
        events
    };
    let (dominant_category_recent4, dominant_share_recent4) = dominant_category(recent4);
    let (dominant_category_recent8, dominant_share_recent8) = dominant_category(recent8);
    let tail_category = events
        .last()
        .map(|event| stat_category(&event.stat_key).to_string())
        .unwrap_or_else(|| "mixed".to_string());
    let current_category_run_len = events
        .iter()
        .rev()
        .take_while(|event| stat_category(&event.stat_key) == tail_category.as_str())
        .count() as i64;

    let has_recent_extreme = events
        .iter()
        .rev()
        .take(6)
        .any(|event| event.tier_index == 1 || event.tier_index == 8);
    let tier_signal = if has_recent_extreme {
        "jump_or_extreme".to_string()
    } else {
        let mut prev_tier_by_stat: HashMap<&str, i64> = HashMap::new();
        let mut stable_count = 0i64;
        let mut step_count = 0i64;
        let mut jump_count = 0i64;
        for event in events.iter().rev().take(24).rev() {
            if let Some(prev_tier) = prev_tier_by_stat.get(event.stat_key.as_str()) {
                let diff = (event.tier_index - *prev_tier).abs();
                if diff == 0 {
                    stable_count += 1;
                } else if diff == 1 {
                    step_count += 1;
                } else {
                    jump_count += 1;
                }
            }
            prev_tier_by_stat.insert(event.stat_key.as_str(), event.tier_index);
        }
        if jump_count > step_count.max(stable_count) {
            "jump_or_extreme".to_string()
        } else if step_count > stable_count {
            "step".to_string()
        } else {
            "stable".to_string()
        }
    };

    let reversion_score_by_stat = compute_reversion_scores(events, stat_keys);
    let mut reversion_top_stats = reversion_score_by_stat
        .iter()
        .filter(|(_, score)| **score > 0.0)
        .map(|(stat_key, score)| (stat_key.clone(), *score))
        .collect::<Vec<_>>();
    reversion_top_stats.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let active_delta = (active_stat_count_recent8 - active_stat_count_recent12).abs() as f64;
    let dominant_shift = if dominant_category_recent4 != dominant_category_recent8
        && dominant_share_recent4 >= 0.5
    {
        0.55
    } else {
        0.0
    };
    let concentration_delta = (dominant_share_recent4 - dominant_share_recent8).abs();
    let run_boost = if current_category_run_len >= 3 {
        0.22
    } else if current_category_run_len == 2 {
        0.10
    } else {
        0.0
    };
    let zone_break = if zone_candidate != "mixed" && out_of_zone_streak >= 2 {
        0.25
    } else {
        0.0
    };
    let regime_shift_score = (dominant_shift
        + concentration_delta * 0.7
        + (active_delta / 4.0).min(1.0) * 0.25
        + run_boost
        + zone_break)
        .clamp(0.0, 1.5);
    let regime_stage = if regime_shift_score >= 0.85 {
        "new_regime".to_string()
    } else if regime_shift_score >= 0.42 {
        "transitioning".to_string()
    } else {
        "stable".to_string()
    };

    SequenceStateFeatures {
        active_stat_count_recent8,
        active_stat_count_recent12,
        active_stat_bucket,
        zone_candidate,
        zone_confidence,
        out_of_zone_streak,
        crit_signal,
        tier_signal,
        regime_stage,
        regime_shift_score,
        dominant_category_recent4,
        dominant_category_recent8,
        current_category_run_len,
        reversion_top_stats: reversion_top_stats
            .into_iter()
            .take(3)
            .map(|(stat_key, _)| stat_key)
            .collect(),
        reversion_score_by_stat,
    }
}
