use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::db::{
    get_setting_f64, get_setting_i64, get_setting_string, now_rfc3339, open_connection, AppState,
};
use crate::domain::types::{
    AdaptiveNextSuggestion, DailyExactPatternRow, DailyPatternDecisionFilter,
    DailyPatternDecisionReport, DailyShapePatternRow, ManualCycleSuggestion,
    ManualGuessVerificationRow, ManualPatternSummary, PatternBacktestSummary,
    PatternBlendWeights,
};
use crate::stats::wilson_interval;

fn resolve_game_day(
    conn: &Connection,
    filter: &DailyPatternDecisionFilter,
) -> Result<String, String> {
    if let Some(game_day) = &filter.game_day {
        let trimmed = game_day.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let day: Option<String> = conn
        .query_row("SELECT MAX(game_day) FROM ordered_events", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("failed to resolve latest game_day: {e}"))?;
    Ok(day.unwrap_or_default())
}

fn list_enabled_stats(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT stat_key, display_name FROM stat_defs WHERE enabled = 1 ORDER BY rowid")
        .map_err(|e| format!("failed to prepare stat_defs query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("failed to query stat_defs: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect stat_defs: {e}"))
}

fn load_day_sequence(conn: &Connection, game_day: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT stat_key
             FROM ordered_events
             WHERE game_day = ?1
             ORDER BY analysis_seq ASC",
        )
        .map_err(|e| format!("failed to prepare day sequence query: {e}"))?;
    let rows = stmt
        .query_map([game_day], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query day sequence: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect day sequence: {e}"))
}

fn shape_signature(pattern: &[String]) -> String {
    let mut map: HashMap<&str, usize> = HashMap::new();
    let mut out = String::with_capacity(pattern.len());
    for token in pattern {
        let idx = if let Some(&v) = map.get(token.as_str()) {
            v
        } else {
            let v = map.len();
            map.insert(token.as_str(), v);
            v
        };
        let ch = if idx < 26 {
            (b'A' + idx as u8) as char
        } else {
            '?'
        };
        out.push(ch);
    }
    out
}

fn shape_signature_slice(pattern: &[&str]) -> String {
    let mut map: HashMap<&str, usize> = HashMap::new();
    let mut out = String::with_capacity(pattern.len());
    for token in pattern {
        let idx = if let Some(&v) = map.get(token) {
            v
        } else {
            let v = map.len();
            map.insert(*token, v);
            v
        };
        let ch = if idx < 26 {
            (b'A' + idx as u8) as char
        } else {
            '?'
        };
        out.push(ch);
    }
    out
}

fn ends_with_pattern(seq: &[String], suffix: &[String]) -> bool {
    if suffix.len() > seq.len() {
        return false;
    }
    let start = seq.len() - suffix.len();
    seq[start..] == *suffix
}

fn infer_next_stat_from_shape(shape: &str, suffix: &[String]) -> Option<String> {
    let chars = shape.chars().collect::<Vec<_>>();
    if chars.len() != suffix.len() + 1 {
        return None;
    }

    let mut sym_to_stat: HashMap<char, String> = HashMap::new();
    let mut stat_to_sym: HashMap<String, char> = HashMap::new();

    for (idx, symbol) in chars[..chars.len() - 1].iter().enumerate() {
        let stat = &suffix[idx];
        if let Some(mapped) = sym_to_stat.get(symbol) {
            if mapped != stat {
                return None;
            }
        } else {
            sym_to_stat.insert(*symbol, stat.clone());
        }

        if let Some(mapped_sym) = stat_to_sym.get(stat) {
            if mapped_sym != symbol {
                return None;
            }
        } else {
            stat_to_sym.insert(stat.clone(), *symbol);
        }
    }

    let next_symbol = chars[chars.len() - 1];
    sym_to_stat.get(&next_symbol).cloned()
}

fn canonicalize_shape(raw: &str) -> Option<String> {
    let letters = raw
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .collect::<Vec<_>>();
    if letters.len() < 2 || letters.len() > 16 {
        return None;
    }

    let mut raw_to_canonical: HashMap<char, char> = HashMap::new();
    let mut next_idx = 0usize;
    let mut out = String::with_capacity(letters.len());
    for ch in letters {
        let mapped = if let Some(&v) = raw_to_canonical.get(&ch) {
            v
        } else {
            if next_idx >= 26 {
                return None;
            }
            let v = (b'A' + next_idx as u8) as char;
            raw_to_canonical.insert(ch, v);
            next_idx += 1;
            v
        };
        out.push(mapped);
    }
    Some(out)
}

fn infer_hint_from_guess(seq: &[String], guess_shape: &str) -> Option<String> {
    let chars = guess_shape.chars().collect::<Vec<_>>();
    if chars.len() < 2 {
        return None;
    }

    for k in (1..chars.len()).rev() {
        if k > seq.len() {
            continue;
        }
        let probe = chars[..=k].iter().collect::<String>();
        let suffix = seq[seq.len() - k..].to_vec();
        if let Some(next_stat) = infer_next_stat_from_shape(&probe, &suffix) {
            return Some(next_stat);
        }
    }
    None
}

#[derive(Default)]
struct ShapeAggregate {
    support: i64,
    expected_count: f64,
    examples: Vec<(String, i64, f64)>,
}

#[derive(Clone)]
struct ComponentProbabilitySet {
    base_probs: HashMap<String, f64>,
    markov_probs: HashMap<String, f64>,
    motif_probs: HashMap<String, f64>,
    markov_active: bool,
    motif_active: bool,
}

#[derive(Clone, Copy)]
struct AdaptiveBlendSummary {
    base_weight: f64,
    markov_weight: f64,
    motif_weight: f64,
    sample_count: usize,
    top1_accuracy: f64,
    avg_true_prob: f64,
}

#[derive(Clone, Copy)]
struct BacktestConfig {
    analysis_window: usize,
    min_len: usize,
    max_len: usize,
    min_support: i64,
    max_order: usize,
    alpha: f64,
    motif_lambda: f64,
}

struct ReducedExactPattern {
    length: i64,
    pattern: Vec<String>,
    support: i64,
    window_count: i64,
    lift: f64,
}

struct ReducedShapePattern {
    length: i64,
    shape: String,
    support: i64,
    lift: f64,
}

fn normalize_probability_map(map: &mut HashMap<String, f64>) {
    let total = map.values().sum::<f64>();
    if total > 1e-12 {
        for value in map.values_mut() {
            *value /= total;
        }
    } else if !map.is_empty() {
        let uniform = 1.0 / map.len() as f64;
        for value in map.values_mut() {
            *value = uniform;
        }
    }
}

fn build_motif_probs_from_raw_boosts(
    stat_keys: &[String],
    base_probs: &HashMap<String, f64>,
    raw_boost_map: &HashMap<String, f64>,
    motif_lambda: f64,
) -> HashMap<String, f64> {
    let max_boost = raw_boost_map.values().copied().fold(0.0, f64::max);
    if max_boost <= 1e-9 {
        return base_probs.clone();
    }

    let mut motif_probs = HashMap::new();
    for stat_key in stat_keys {
        let base = *base_probs.get(stat_key).unwrap_or(&0.0);
        let raw_boost = *raw_boost_map.get(stat_key).unwrap_or(&0.0);
        let norm_boost = if max_boost > 1e-9 {
            raw_boost / max_boost
        } else {
            0.0
        };
        motif_probs.insert(stat_key.clone(), base * (1.0 + motif_lambda * norm_boost));
    }
    normalize_probability_map(&mut motif_probs);
    motif_probs
}

fn resolve_active_blend_weights(
    blend: &AdaptiveBlendSummary,
    markov_active: bool,
    motif_active: bool,
) -> (f64, f64, f64) {
    let base_weight = blend.base_weight.max(0.0);
    let markov_weight = if markov_active {
        blend.markov_weight.max(0.0)
    } else {
        0.0
    };
    let motif_weight = if motif_active {
        blend.motif_weight.max(0.0)
    } else {
        0.0
    };
    let total = base_weight + markov_weight + motif_weight;
    if total <= 1e-9 {
        return (1.0, 0.0, 0.0);
    }
    (
        base_weight / total,
        markov_weight / total,
        motif_weight / total,
    )
}

fn blend_component_probs(
    stat_keys: &[String],
    components: &ComponentProbabilitySet,
    blend: &AdaptiveBlendSummary,
) -> HashMap<String, f64> {
    let (base_weight, markov_weight, motif_weight) =
        resolve_active_blend_weights(blend, components.markov_active, components.motif_active);
    let mut mixed = HashMap::new();
    for stat_key in stat_keys {
        let base = *components.base_probs.get(stat_key).unwrap_or(&0.0);
        let markov = *components.markov_probs.get(stat_key).unwrap_or(&base);
        let motif = *components.motif_probs.get(stat_key).unwrap_or(&base);
        mixed.insert(
            stat_key.clone(),
            base_weight * base + markov_weight * markov + motif_weight * motif,
        );
    }
    normalize_probability_map(&mut mixed);
    mixed
}

fn top_prediction_key<'a>(map: &'a HashMap<String, f64>) -> Option<&'a str> {
    map.iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal))
        .map(|(k, _)| k.as_str())
}

fn default_adaptive_blend(baseline_blend: f64) -> AdaptiveBlendSummary {
    let mut base_weight = (1.0 - baseline_blend * 0.72).clamp(0.30, 0.58);
    let mut markov_weight = (baseline_blend * 0.52).clamp(0.20, 0.40);
    let mut motif_weight = 0.18;
    let total = base_weight + markov_weight + motif_weight;
    base_weight /= total;
    markov_weight /= total;
    motif_weight /= total;
    AdaptiveBlendSummary {
        base_weight,
        markov_weight,
        motif_weight,
        sample_count: 0,
        top1_accuracy: 0.0,
        avg_true_prob: 0.0,
    }
}

fn load_recent_global_sequence(conn: &Connection, limit: usize) -> Result<Vec<String>, String> {
    let limit = limit.max(1) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT stat_key
             FROM (
               SELECT stat_key, game_day, analysis_seq, created_seq
               FROM ordered_events
               ORDER BY game_day DESC, analysis_seq DESC, created_seq DESC
               LIMIT ?1
             )
             ORDER BY game_day ASC, analysis_seq ASC, created_seq ASC",
        )
        .map_err(|e| format!("failed to prepare recent global sequence query: {e}"))?;
    let rows = stmt
        .query_map([limit], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query recent global sequence: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect recent global sequence: {e}"))
}

fn build_component_probabilities(
    seq: &[String],
    stat_keys: &[String],
    config: &BacktestConfig,
) -> ComponentProbabilitySet {
    let mut base_counts: HashMap<String, i64> = HashMap::new();
    for stat_key in seq {
        *base_counts.entry(stat_key.clone()).or_insert(0) += 1;
    }

    let n = seq.len();
    let k = stat_keys.len().max(1) as f64;
    let denom = n as f64 + config.alpha * k;
    let base_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|stat_key| {
            let count = *base_counts.get(stat_key).unwrap_or(&0) as f64;
            (stat_key.clone(), (count + config.alpha) / denom)
        })
        .collect();

    let effective_max_order = if n >= 2 {
        config.max_order.min(n - 1)
    } else {
        1
    };
    let mut markov_models: Vec<HashMap<Vec<String>, HashMap<String, i64>>> = Vec::new();
    for order in 1..=effective_max_order {
        let mut model: HashMap<Vec<String>, HashMap<String, i64>> = HashMap::new();
        if n > order {
            for i in order..n {
                let context = seq[i - order..i].to_vec();
                let next = seq[i].clone();
                *model.entry(context).or_default().entry(next).or_insert(0) += 1;
            }
        }
        markov_models.push(model);
    }

    let mut markov_acc: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut markov_weight_total = 0.0;
    for order in 1..=effective_max_order {
        if n < order {
            continue;
        }
        let context = seq[n - order..n].to_vec();
        if let Some(next_counts) = markov_models[order - 1].get(&context) {
            let total: i64 = next_counts.values().sum();
            if total <= 0 {
                continue;
            }
            let confidence = total as f64 / (total as f64 + 3.0 * order as f64);
            let weight = confidence * (order as f64).powf(1.35);
            if weight <= 0.0 {
                continue;
            }
            for stat_key in stat_keys {
                let count = *next_counts.get(stat_key).unwrap_or(&0) as f64;
                let p = (count + config.alpha) / (total as f64 + config.alpha * k);
                if let Some(acc) = markov_acc.get_mut(stat_key) {
                    *acc += weight * p;
                }
            }
            markov_weight_total += weight;
        }
    }
    let markov_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|stat_key| {
            let p = if markov_weight_total > 0.0 {
                markov_acc.get(stat_key).copied().unwrap_or(0.0) / markov_weight_total
            } else {
                *base_probs.get(stat_key).unwrap_or(&0.0)
            };
            (stat_key.clone(), p)
        })
        .collect();

    let marginals: HashMap<String, f64> = stat_keys
        .iter()
        .map(|stat_key| {
            (
                stat_key.clone(),
                (*base_counts.get(stat_key).unwrap_or(&0) as f64) / n.max(1) as f64,
            )
        })
        .collect();

    let mut exact_patterns_all = Vec::<ReducedExactPattern>::new();
    let mut shape_map: HashMap<(i64, String), (i64, f64)> = HashMap::new();
    for len in 2..=config.max_len.max(2) {
        if len > n {
            continue;
        }
        let windows = (n - len + 1) as i64;
        let mut counts: HashMap<Vec<String>, i64> = HashMap::new();
        for i in 0..=n - len {
            let pattern = seq[i..i + len].to_vec();
            *counts.entry(pattern).or_insert(0) += 1;
        }

        for (pattern, support) in counts {
            if len < config.min_len || support < config.min_support {
                continue;
            }

            let expected_prob = pattern
                .iter()
                .map(|stat_key| *marginals.get(stat_key).unwrap_or(&0.0))
                .product::<f64>();
            let expected_count = expected_prob * windows as f64;
            let lift = if expected_count > 1e-9 {
                support as f64 / expected_count
            } else {
                0.0
            };
            let shape = shape_signature(&pattern);

            exact_patterns_all.push(ReducedExactPattern {
                length: len as i64,
                pattern,
                support,
                window_count: windows,
                lift,
            });

            let entry = shape_map.entry((len as i64, shape)).or_insert((0, 0.0));
            entry.0 += support;
            entry.1 += expected_count;
        }
    }

    let shape_patterns_all = shape_map
        .into_iter()
        .map(|((length, shape), (support, expected_count))| ReducedShapePattern {
            length,
            shape,
            support,
            lift: if expected_count > 1e-9 {
                support as f64 / expected_count
            } else {
                0.0
            },
        })
        .collect::<Vec<_>>();

    let mut matched_exact = Vec::<(String, f64, i64)>::new();
    for row in &exact_patterns_all {
        if row.length < 3 {
            continue;
        }
        let prefix = &row.pattern[..row.pattern.len() - 1];
        if !ends_with_pattern(seq, prefix) {
            continue;
        }
        let Some(next_stat) = row.pattern.last().cloned() else {
            continue;
        };
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let density = row.support as f64 / row.window_count.max(1) as f64;
        let short_penalty = if row.length <= 3 { 0.55 } else { 1.0 };
        let boost = lift_gain
            * (row.length as f64).powf(1.6)
            * (row.support as f64).ln_1p()
            * density.sqrt()
            * short_penalty;
        if boost > 0.0 {
            matched_exact.push((next_stat, boost, row.length));
        }
    }

    let mut matched_shape = Vec::<(String, f64, i64)>::new();
    for row in &shape_patterns_all {
        if row.length < 4 {
            continue;
        }
        let prefix_len = (row.length - 1) as usize;
        if prefix_len == 0 || prefix_len > seq.len() {
            continue;
        }
        let suffix = &seq[seq.len() - prefix_len..];
        let Some(next_stat) = infer_next_stat_from_shape(&row.shape, suffix) else {
            continue;
        };
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let boost = lift_gain * (row.support as f64).ln_1p() * (row.length as f64).powf(1.7);
        if boost > 0.0 {
            matched_shape.push((next_stat, boost, row.length));
        }
    }

    let all_contribs = matched_exact
        .into_iter()
        .chain(matched_shape.into_iter())
        .collect::<Vec<_>>();
    let longest_match_len = all_contribs
        .iter()
        .map(|(_, _, len)| *len)
        .max()
        .unwrap_or(0) as f64;

    let mut raw_boost_map: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    for (stat_key, boost, len) in all_contribs {
        let len_factor = if longest_match_len > 0.0 {
            (len as f64 / longest_match_len).powf(2.2)
        } else {
            1.0
        };
        let adjusted = boost * len_factor;
        if adjusted <= 0.0 {
            continue;
        }
        if let Some(value) = raw_boost_map.get_mut(&stat_key) {
            *value += adjusted;
        }
    }

    let motif_active = raw_boost_map.values().copied().fold(0.0, f64::max) > 1e-9;
    let motif_probs =
        build_motif_probs_from_raw_boosts(stat_keys, &base_probs, &raw_boost_map, config.motif_lambda);

    ComponentProbabilitySet {
        base_probs,
        markov_probs,
        motif_probs,
        markov_active: markov_weight_total > 0.0,
        motif_active,
    }
}

fn fit_backtested_blend(
    conn: &Connection,
    stat_keys: &[String],
    config: &BacktestConfig,
    baseline_blend: f64,
) -> Result<AdaptiveBlendSummary, String> {
    let default_blend = default_adaptive_blend(baseline_blend);
    if stat_keys.is_empty() {
        return Ok(default_blend);
    }

    let backtest_samples = get_setting_i64(conn, "pattern_backtest_samples", 72).clamp(24, 160)
        as usize;
    let history_limit = (config.analysis_window + backtest_samples + 1).max(48);
    let history = load_recent_global_sequence(conn, history_limit)?;
    let warmup = config.min_len.max(6).max(config.max_order + 1);
    if history.len() <= warmup + 1 {
        return Ok(default_blend);
    }

    let last_idx = history.len() - 1;
    let start = warmup.max(last_idx.saturating_sub(backtest_samples));
    let mut samples = Vec::<(ComponentProbabilitySet, String, f64)>::new();
    for idx in start..last_idx {
        let prefix_start = (idx + 1).saturating_sub(config.analysis_window);
        let prefix = &history[prefix_start..=idx];
        if prefix.len() < warmup {
            continue;
        }
        let age = (last_idx - idx - 1) as f64;
        let sample_weight = 0.985_f64.powf(age);
        let components = build_component_probabilities(prefix, stat_keys, config);
        let actual = history[idx + 1].clone();
        samples.push((components, actual, sample_weight));
    }

    if samples.len() < 12 {
        return Ok(default_blend);
    }

    let prior = [
        default_blend.base_weight,
        default_blend.markov_weight,
        default_blend.motif_weight,
    ];
    let mut log_scores = [0.0; 3];
    let mut weighted_total = 0.0;
    for (components, actual, sample_weight) in &samples {
        let expert_maps = [
            &components.base_probs,
            &components.markov_probs,
            &components.motif_probs,
        ];
        for (idx, expert_map) in expert_maps.iter().enumerate() {
            let p = expert_map
                .get(actual)
                .copied()
                .unwrap_or(1e-9)
                .clamp(1e-9, 1.0);
            log_scores[idx] += sample_weight * p.ln();
        }
        weighted_total += sample_weight;
    }

    if weighted_total <= 1e-9 {
        return Ok(default_blend);
    }

    let mean_log_scores = [
        log_scores[0] / weighted_total,
        log_scores[1] / weighted_total,
        log_scores[2] / weighted_total,
    ];
    let best_score = mean_log_scores
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let temperature = 0.35;
    let mut raw_weights = [0.0; 3];
    for idx in 0..3 {
        raw_weights[idx] = prior[idx] * ((mean_log_scores[idx] - best_score) / temperature).exp();
    }
    let raw_total = raw_weights.iter().sum::<f64>().max(1e-9);
    let learned = [
        raw_weights[0] / raw_total,
        raw_weights[1] / raw_total,
        raw_weights[2] / raw_total,
    ];
    let mut final_weights = [
        prior[0] * 0.30 + learned[0] * 0.70,
        prior[1] * 0.30 + learned[1] * 0.70,
        prior[2] * 0.30 + learned[2] * 0.70,
    ];
    let final_total = final_weights.iter().sum::<f64>().max(1e-9);
    for weight in &mut final_weights {
        *weight /= final_total;
    }

    let learned_blend = AdaptiveBlendSummary {
        base_weight: final_weights[0],
        markov_weight: final_weights[1],
        motif_weight: final_weights[2],
        sample_count: samples.len(),
        top1_accuracy: 0.0,
        avg_true_prob: 0.0,
    };

    let mut mix_hits = 0.0;
    let mut mix_true_prob = 0.0;
    for (components, actual, sample_weight) in &samples {
        let mixed = blend_component_probs(stat_keys, components, &learned_blend);
        let p = mixed.get(actual).copied().unwrap_or(0.0);
        mix_true_prob += sample_weight * p;
        if top_prediction_key(&mixed) == Some(actual.as_str()) {
            mix_hits += sample_weight;
        }
    }

    Ok(AdaptiveBlendSummary {
        base_weight: learned_blend.base_weight,
        markov_weight: learned_blend.markov_weight,
        motif_weight: learned_blend.motif_weight,
        sample_count: learned_blend.sample_count,
        top1_accuracy: (mix_hits / weighted_total).clamp(0.0, 1.0),
        avg_true_prob: (mix_true_prob / weighted_total).clamp(0.0, 1.0),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PredictionBucket {
    sample_depth_bucket: String,
    markov_hit_bucket: String,
    motif_hit_bucket: String,
}

impl PredictionBucket {
    fn key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.sample_depth_bucket, self.markov_hit_bucket, self.motif_hit_bucket
        )
    }
}

#[derive(Clone)]
struct V2ComponentBuild {
    base_probs: HashMap<String, f64>,
    markov_probs: HashMap<String, f64>,
    exact_motif_probs: HashMap<String, f64>,
    approx_shape_probs: HashMap<String, f64>,
    auto_cycle_probs: HashMap<String, f64>,
    markov_active: bool,
    exact_motif_active: bool,
    approx_shape_active: bool,
    auto_cycle_active: bool,
    markov_max_order_hit: usize,
    motif_strength: f64,
    approx_strength: f64,
    auto_cycle_strength: f64,
    bucket: PredictionBucket,
    matched_patterns_map: HashMap<String, Vec<String>>,
    matched_experts_map: HashMap<String, Vec<String>>,
}

#[derive(Clone, Copy, Default)]
struct InternalBlendWeights {
    base: f64,
    markov: f64,
    exact_motif: f64,
    approx_shape: f64,
    auto_cycle: f64,
}

#[derive(Clone)]
struct TrainedAdaptiveModel {
    fallback_weights: InternalBlendWeights,
    global_weights: InternalBlendWeights,
    bucket_weights: HashMap<String, (InternalBlendWeights, usize)>,
    bucket_min_samples: usize,
    backtest_summary: PatternBacktestSummary,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredPredictionRow {
    stat_key: String,
    probability: f64,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredPredictionContext {
    bucket: PredictionBucket,
    expert_probs: HashMap<String, HashMap<String, f64>>,
    weight_source: String,
    active_experts: Vec<String>,
    tail_signature: String,
}

fn empty_backtest_summary() -> PatternBacktestSummary {
    PatternBacktestSummary {
        sample_count: 0,
        top1_accuracy: 0.0,
        top3_coverage: 0.0,
        mean_true_prob: 0.0,
        mean_log_loss: 0.0,
    }
}

fn empty_pattern_blend_weights() -> PatternBlendWeights {
    PatternBlendWeights {
        source: "fallback".to_string(),
        sample_depth_bucket: "n/a".to_string(),
        markov_hit_bucket: "none".to_string(),
        motif_hit_bucket: "none".to_string(),
        base: 1.0,
        markov: 0.0,
        exact_motif: 0.0,
        approx_shape: 0.0,
        auto_cycle: 0.0,
        online_adjusted: false,
    }
}

fn resolve_report_model_mode(mode: &str) -> String {
    if mode == "baseline_v1" {
        "baseline_v1".to_string()
    } else {
        "adaptive_v2".to_string()
    }
}

fn normalize_internal_weights(weights: &mut InternalBlendWeights) {
    let total = weights.base
        + weights.markov
        + weights.exact_motif
        + weights.approx_shape
        + weights.auto_cycle;
    if total <= 1e-9 {
        weights.base = 1.0;
        weights.markov = 0.0;
        weights.exact_motif = 0.0;
        weights.approx_shape = 0.0;
        weights.auto_cycle = 0.0;
        return;
    }
    weights.base /= total;
    weights.markov /= total;
    weights.exact_motif /= total;
    weights.approx_shape /= total;
    weights.auto_cycle /= total;
}

fn default_v2_weights(baseline_blend: f64) -> InternalBlendWeights {
    let mut weights = InternalBlendWeights {
        base: (1.0 - baseline_blend * 0.72).clamp(0.24, 0.42),
        markov: (baseline_blend * 0.34).clamp(0.16, 0.28),
        exact_motif: 0.18,
        approx_shape: 0.12,
        auto_cycle: 0.12,
    };
    normalize_internal_weights(&mut weights);
    weights
}

fn resolve_active_v2_weights(
    mut weights: InternalBlendWeights,
    components: &V2ComponentBuild,
) -> InternalBlendWeights {
    if !components.markov_active {
        weights.markov = 0.0;
    }
    if !components.exact_motif_active {
        weights.exact_motif = 0.0;
    }
    if !components.approx_shape_active {
        weights.approx_shape = 0.0;
    }
    if !components.auto_cycle_active {
        weights.auto_cycle = 0.0;
    }
    normalize_internal_weights(&mut weights);
    weights
}

fn internal_weights_to_public(
    weights: InternalBlendWeights,
    source: &str,
    bucket: &PredictionBucket,
    online_adjusted: bool,
) -> PatternBlendWeights {
    PatternBlendWeights {
        source: source.to_string(),
        sample_depth_bucket: bucket.sample_depth_bucket.clone(),
        markov_hit_bucket: bucket.markov_hit_bucket.clone(),
        motif_hit_bucket: bucket.motif_hit_bucket.clone(),
        base: weights.base,
        markov: weights.markov,
        exact_motif: weights.exact_motif,
        approx_shape: weights.approx_shape,
        auto_cycle: weights.auto_cycle,
        online_adjusted,
    }
}

fn push_label(map: &mut HashMap<String, Vec<String>>, stat_key: &str, label: String) {
    map.entry(stat_key.to_string()).or_default().push(label);
}

fn push_source(map: &mut HashMap<String, Vec<String>>, stat_key: &str, source: &str) {
    map.entry(stat_key.to_string())
        .or_default()
        .push(source.to_string());
}

fn stat_display_name(display_map: Option<&HashMap<String, String>>, stat_key: &str) -> String {
    display_map
        .and_then(|map| map.get(stat_key))
        .cloned()
        .unwrap_or_else(|| stat_key.to_string())
}

fn shape_hamming_distance(lhs: &str, rhs: &str) -> Option<usize> {
    let lhs_chars = lhs.chars().collect::<Vec<_>>();
    let rhs_chars = rhs.chars().collect::<Vec<_>>();
    if lhs_chars.len() != rhs_chars.len() {
        return None;
    }
    Some(
        lhs_chars
            .iter()
            .zip(rhs_chars.iter())
            .filter(|(a, b)| a != b)
            .count(),
    )
}

fn infer_next_stat_from_approx_shape(shape: &str, suffix: &[String]) -> Option<(String, usize)> {
    let chars = shape.chars().collect::<Vec<_>>();
    if chars.len() != suffix.len() + 1 {
        return None;
    }
    let prefix = chars[..chars.len() - 1].iter().collect::<String>();
    let suffix_shape = shape_signature(suffix);
    let mismatches = shape_hamming_distance(&prefix, &suffix_shape)?;
    if mismatches > 1 {
        return None;
    }
    let next_symbol = chars[chars.len() - 1];
    for (idx, symbol) in chars[..chars.len() - 1].iter().enumerate() {
        if *symbol == next_symbol {
            return Some((suffix[idx].clone(), mismatches));
        }
    }
    None
}

fn build_tail_signature(seq: &[String]) -> String {
    let tail_len = seq.len().min(8);
    if tail_len == 0 {
        return "empty".to_string();
    }
    seq[seq.len() - tail_len..].join(">")
}

fn build_auto_cycle_expert(
    seq: &[String],
    stat_keys: &[String],
    alpha: f64,
    display_map: Option<&HashMap<String, String>>,
) -> (
    HashMap<String, f64>,
    f64,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
) {
    let mut probs: HashMap<String, f64> = stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut labels = HashMap::<String, Vec<String>>::new();
    let mut sources = HashMap::<String, Vec<String>>::new();
    let mut total_weight = 0.0;
    let k = stat_keys.len().max(1) as f64;

    for cycle_len in 3..=8usize {
        if seq.len() < cycle_len * 2 {
            continue;
        }
        let target_pos = seq.len() % cycle_len;
        for phase in -1..=1 {
            let mut counts: HashMap<String, i64> = HashMap::new();
            let mut total = 0i64;
            for (idx, stat_key) in seq.iter().enumerate() {
                let shifted =
                    (idx as i64 + phase).rem_euclid(cycle_len as i64) as usize;
                if shifted == target_pos {
                    *counts.entry(stat_key.clone()).or_insert(0) += 1;
                    total += 1;
                }
            }
            if total < 2 {
                continue;
            }

            let top_count = counts.values().copied().max().unwrap_or(0);
            let stability = top_count as f64 / total as f64;
            let concentration = counts
                .values()
                .map(|count| {
                    let p = *count as f64 / total as f64;
                    p * p
                })
                .sum::<f64>();
            let support_factor = total as f64 / (total as f64 + 2.0);
            let phase_penalty = if phase == 0 { 1.0 } else { 0.72 };
            let weight = concentration
                * stability
                * support_factor
                * phase_penalty
                * (cycle_len as f64).powf(0.35);
            if weight < 0.18 {
                continue;
            }

            total_weight += weight;
            for stat_key in stat_keys {
                let count = *counts.get(stat_key).unwrap_or(&0) as f64;
                let p = (count + alpha) / (total as f64 + alpha * k);
                if let Some(value) = probs.get_mut(stat_key) {
                    *value += weight * p;
                }
                if count > 0.0 && p >= 0.15 {
                    let label = format!(
                        "自动周期 L{} phase={:+} → {} {:.0}%",
                        cycle_len,
                        phase,
                        stat_display_name(display_map, stat_key),
                        p * 100.0
                    );
                    push_label(&mut labels, stat_key, label);
                    push_source(&mut sources, stat_key, "auto_cycle");
                }
            }
        }
    }

    if total_weight > 1e-9 {
        for value in probs.values_mut() {
            *value /= total_weight;
        }
    }
    normalize_probability_map(&mut probs);
    (probs, total_weight, labels, sources)
}

fn build_v2_components(
    seq: &[String],
    stat_keys: &[String],
    config: &BacktestConfig,
    display_map: Option<&HashMap<String, String>>,
) -> V2ComponentBuild {
    let mut base_counts: HashMap<String, i64> = HashMap::new();
    for stat_key in seq {
        *base_counts.entry(stat_key.clone()).or_insert(0) += 1;
    }

    let n = seq.len();
    let k = stat_keys.len().max(1) as f64;
    let denom = n as f64 + config.alpha * k;
    let base_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|stat_key| {
            let count = *base_counts.get(stat_key).unwrap_or(&0) as f64;
            (stat_key.clone(), (count + config.alpha) / denom.max(1e-9))
        })
        .collect();

    let effective_max_order = if n >= 2 {
        config.max_order.min(n - 1)
    } else {
        1
    };
    let mut markov_models: Vec<HashMap<Vec<String>, HashMap<String, i64>>> = Vec::new();
    for order in 1..=effective_max_order {
        let mut model: HashMap<Vec<String>, HashMap<String, i64>> = HashMap::new();
        if n > order {
            for idx in order..n {
                let context = seq[idx - order..idx].to_vec();
                let next = seq[idx].clone();
                *model.entry(context).or_default().entry(next).or_insert(0) += 1;
            }
        }
        markov_models.push(model);
    }

    let mut markov_acc: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut markov_weight_total = 0.0;
    let mut markov_max_order_hit = 0usize;
    for order in 1..=effective_max_order {
        if n < order {
            continue;
        }
        let context = seq[n - order..n].to_vec();
        if let Some(next_counts) = markov_models[order - 1].get(&context) {
            let total: i64 = next_counts.values().sum();
            if total <= 0 {
                continue;
            }
            let confidence = total as f64 / (total as f64 + 3.0 * order as f64);
            let weight = confidence * (order as f64).powf(1.35);
            if weight <= 0.0 {
                continue;
            }
            markov_max_order_hit = markov_max_order_hit.max(order);
            for stat_key in stat_keys {
                let count = *next_counts.get(stat_key).unwrap_or(&0) as f64;
                let p = (count + config.alpha) / (total as f64 + config.alpha * k);
                if let Some(acc) = markov_acc.get_mut(stat_key) {
                    *acc += weight * p;
                }
            }
            markov_weight_total += weight;
        }
    }
    let markov_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|stat_key| {
            let p = if markov_weight_total > 0.0 {
                markov_acc.get(stat_key).copied().unwrap_or(0.0) / markov_weight_total
            } else {
                *base_probs.get(stat_key).unwrap_or(&0.0)
            };
            (stat_key.clone(), p)
        })
        .collect();

    let marginals: HashMap<String, f64> = stat_keys
        .iter()
        .map(|stat_key| {
            (
                stat_key.clone(),
                (*base_counts.get(stat_key).unwrap_or(&0) as f64) / n.max(1) as f64,
            )
        })
        .collect();

    let mut exact_patterns_all = Vec::<ReducedExactPattern>::new();
    let mut shape_patterns_all = Vec::<ReducedShapePattern>::new();
    let mut shape_map: HashMap<(i64, String), (i64, f64)> = HashMap::new();
    let max_scan_len = config.max_len.max(8).max(4);
    for len in 2..=max_scan_len {
        if len > n {
            continue;
        }
        let windows = (n - len + 1) as i64;
        let mut counts: HashMap<Vec<String>, i64> = HashMap::new();
        for idx in 0..=n - len {
            let pattern = seq[idx..idx + len].to_vec();
            *counts.entry(pattern).or_insert(0) += 1;
        }

        for (pattern, support) in counts {
            if len < config.min_len || support < config.min_support {
                continue;
            }
            let expected_prob = pattern
                .iter()
                .map(|stat_key| *marginals.get(stat_key).unwrap_or(&0.0))
                .product::<f64>();
            let expected_count = expected_prob * windows as f64;
            let lift = if expected_count > 1e-9 {
                support as f64 / expected_count
            } else {
                0.0
            };
            let shape = shape_signature(&pattern);
            exact_patterns_all.push(ReducedExactPattern {
                length: len as i64,
                pattern,
                support,
                window_count: windows,
                lift,
            });
            let entry = shape_map.entry((len as i64, shape)).or_insert((0, 0.0));
            entry.0 += support;
            entry.1 += expected_count;
        }
    }
    for ((length, shape), (support, expected_count)) in shape_map {
        shape_patterns_all.push(ReducedShapePattern {
            length,
            shape,
            support,
            lift: if expected_count > 1e-9 {
                support as f64 / expected_count
            } else {
                0.0
            },
        });
    }

    let mut exact_raw_boost_map: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut matched_patterns_map = HashMap::<String, Vec<String>>::new();
    let mut matched_experts_map = HashMap::<String, Vec<String>>::new();

    for row in &exact_patterns_all {
        if row.length < 3 {
            continue;
        }
        let prefix = &row.pattern[..row.pattern.len() - 1];
        if !ends_with_pattern(seq, prefix) {
            continue;
        }
        let Some(next_stat) = row.pattern.last().cloned() else {
            continue;
        };
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let density = row.support as f64 / row.window_count.max(1) as f64;
        let short_penalty = if row.length <= 3 { 0.55 } else { 1.0 };
        let boost = lift_gain
            * (row.length as f64).powf(1.6)
            * (row.support as f64).ln_1p()
            * density.sqrt()
            * short_penalty;
        if boost <= 0.0 {
            continue;
        }
        if let Some(value) = exact_raw_boost_map.get_mut(&next_stat) {
            *value += boost;
        }
        push_label(
            &mut matched_patterns_map,
            &next_stat,
            format!("精确模式 {} [{}]", row.pattern.join("→"), shape_signature(&row.pattern)),
        );
        push_source(&mut matched_experts_map, &next_stat, "exact_motif");
    }

    for row in &shape_patterns_all {
        if row.length < 4 {
            continue;
        }
        let prefix_len = (row.length - 1) as usize;
        if prefix_len == 0 || prefix_len > seq.len() {
            continue;
        }
        let suffix = &seq[seq.len() - prefix_len..];
        let Some(next_stat) = infer_next_stat_from_shape(&row.shape, suffix) else {
            continue;
        };
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let boost = lift_gain * (row.support as f64).ln_1p() * (row.length as f64).powf(1.7);
        if boost <= 0.0 {
            continue;
        }
        if let Some(value) = exact_raw_boost_map.get_mut(&next_stat) {
            *value += boost;
        }
        push_label(
            &mut matched_patterns_map,
            &next_stat,
            format!("形态 {} [L{},n={}]", row.shape, row.length, row.support),
        );
        push_source(&mut matched_experts_map, &next_stat, "exact_motif");
    }

    let exact_strength = exact_raw_boost_map.values().copied().fold(0.0, f64::max);
    let exact_motif_probs =
        build_motif_probs_from_raw_boosts(stat_keys, &base_probs, &exact_raw_boost_map, config.motif_lambda);

    let mut approx_raw_boost_map: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    for row in &shape_patterns_all {
        if row.length < 4 || row.length > 8 {
            continue;
        }
        let prefix_len = (row.length - 1) as usize;
        if prefix_len == 0 || prefix_len > seq.len() {
            continue;
        }
        let suffix = &seq[seq.len() - prefix_len..];
        let Some((next_stat, mismatches)) = infer_next_stat_from_approx_shape(&row.shape, suffix)
        else {
            continue;
        };
        if mismatches == 0 {
            continue;
        }
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let mismatch_penalty = if mismatches == 0 { 1.0 } else { 0.38 };
        let boost = lift_gain
            * (row.support as f64).ln_1p()
            * (row.length as f64).powf(1.55)
            * mismatch_penalty;
        if boost <= 0.0 {
            continue;
        }
        if let Some(value) = approx_raw_boost_map.get_mut(&next_stat) {
            *value += boost;
        }
        push_label(
            &mut matched_patterns_map,
            &next_stat,
            format!("近似形态 {} [L{},Δ={}]", row.shape, row.length, mismatches),
        );
        push_source(&mut matched_experts_map, &next_stat, "approx_shape");
    }

    let approx_strength = approx_raw_boost_map.values().copied().fold(0.0, f64::max);
    let approx_shape_probs =
        build_motif_probs_from_raw_boosts(stat_keys, &base_probs, &approx_raw_boost_map, config.motif_lambda * 0.9);

    let (auto_cycle_probs, auto_cycle_strength, auto_labels, auto_sources) =
        build_auto_cycle_expert(seq, stat_keys, config.alpha, display_map);
    for (stat_key, labels) in auto_labels {
        matched_patterns_map
            .entry(stat_key)
            .or_default()
            .extend(labels.into_iter());
    }
    for (stat_key, sources) in auto_sources {
        matched_experts_map
            .entry(stat_key)
            .or_default()
            .extend(sources.into_iter());
    }

    let sample_depth_bucket = if n < 40 {
        "<40".to_string()
    } else if n < 100 {
        "40-99".to_string()
    } else {
        "100+".to_string()
    };
    let markov_hit_bucket = if markov_max_order_hit == 0 {
        "none".to_string()
    } else if markov_max_order_hit <= 2 {
        "short".to_string()
    } else {
        "long".to_string()
    };
    let motif_strength = exact_strength.max(approx_strength);
    let motif_hit_bucket = if motif_strength <= 1e-9 {
        "none".to_string()
    } else if motif_strength < 2.0 {
        "weak".to_string()
    } else {
        "strong".to_string()
    };

    V2ComponentBuild {
        base_probs,
        markov_probs,
        exact_motif_probs,
        approx_shape_probs,
        auto_cycle_probs,
        markov_active: markov_max_order_hit > 0,
        exact_motif_active: exact_strength > 1e-9,
        approx_shape_active: approx_strength > 1e-9,
        auto_cycle_active: auto_cycle_strength > 1e-9,
        markov_max_order_hit,
        motif_strength,
        approx_strength,
        auto_cycle_strength,
        bucket: PredictionBucket {
            sample_depth_bucket,
            markov_hit_bucket,
            motif_hit_bucket,
        },
        matched_patterns_map,
        matched_experts_map,
    }
}

fn fit_v2_weights_for_samples(
    samples: &[(V2ComponentBuild, String)],
    baseline_blend: f64,
) -> InternalBlendWeights {
    let prior = default_v2_weights(baseline_blend);
    let experts = ["base", "markov", "exact_motif", "approx_shape", "auto_cycle"];
    let mut log_scores = [0.0; 5];
    let mut sample_count = 0.0;
    for (components, actual) in samples {
        let expert_maps = [
            &components.base_probs,
            &components.markov_probs,
            &components.exact_motif_probs,
            &components.approx_shape_probs,
            &components.auto_cycle_probs,
        ];
        for (idx, expert_map) in expert_maps.iter().enumerate() {
            let p = expert_map
                .get(actual)
                .copied()
                .unwrap_or(1e-9)
                .clamp(1e-9, 1.0);
            log_scores[idx] += p.ln();
        }
        sample_count += 1.0;
    }
    if sample_count <= 1e-9 {
        return prior;
    }
    let mut mean_scores = [0.0; 5];
    for idx in 0..5 {
        mean_scores[idx] = log_scores[idx] / sample_count;
    }
    let best_score = mean_scores
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let temperature = 0.4;
    let priors = [
        prior.base,
        prior.markov,
        prior.exact_motif,
        prior.approx_shape,
        prior.auto_cycle,
    ];
    let mut raw = [0.0; 5];
    for idx in 0..experts.len() {
        raw[idx] = priors[idx] * ((mean_scores[idx] - best_score) / temperature).exp();
    }
    let total = raw.iter().sum::<f64>().max(1e-9);
    let mut weights = InternalBlendWeights {
        base: raw[0] / total,
        markov: raw[1] / total,
        exact_motif: raw[2] / total,
        approx_shape: raw[3] / total,
        auto_cycle: raw[4] / total,
    };
    normalize_internal_weights(&mut weights);
    weights
}

fn blend_v2_probs(
    stat_keys: &[String],
    components: &V2ComponentBuild,
    weights: InternalBlendWeights,
) -> HashMap<String, f64> {
    let resolved = resolve_active_v2_weights(weights, components);
    let mut mixed = HashMap::new();
    for stat_key in stat_keys {
        let base = *components.base_probs.get(stat_key).unwrap_or(&0.0);
        let markov = *components.markov_probs.get(stat_key).unwrap_or(&base);
        let exact = *components.exact_motif_probs.get(stat_key).unwrap_or(&base);
        let approx = *components.approx_shape_probs.get(stat_key).unwrap_or(&base);
        let cycle = *components.auto_cycle_probs.get(stat_key).unwrap_or(&base);
        mixed.insert(
            stat_key.clone(),
            resolved.base * base
                + resolved.markov * markov
                + resolved.exact_motif * exact
                + resolved.approx_shape * approx
                + resolved.auto_cycle * cycle,
        );
    }
    normalize_probability_map(&mut mixed);
    mixed
}

fn select_v2_weights_for_bucket(
    model: &TrainedAdaptiveModel,
    bucket: &PredictionBucket,
) -> (InternalBlendWeights, &'static str) {
    if let Some((weights, sample_count)) = model.bucket_weights.get(&bucket.key()) {
        if *sample_count >= model.bucket_min_samples {
            return (*weights, "bucketed");
        }
    }
    if model.backtest_summary.sample_count as usize >= model.bucket_min_samples {
        (model.global_weights, "global")
    } else {
        (model.fallback_weights, "fallback")
    }
}

fn active_v2_experts(
    components: &V2ComponentBuild,
    weights: &InternalBlendWeights,
) -> Vec<String> {
    let resolved = resolve_active_v2_weights(*weights, components);
    let mut experts = Vec::new();
    if resolved.base > 0.0 {
        experts.push("base".to_string());
    }
    if resolved.markov > 0.0 {
        experts.push("markov".to_string());
    }
    if resolved.exact_motif > 0.0 {
        experts.push("exact_motif".to_string());
    }
    if resolved.approx_shape > 0.0 {
        experts.push("approx_shape".to_string());
    }
    if resolved.auto_cycle > 0.0 {
        experts.push("auto_cycle".to_string());
    }
    experts
}

fn run_walk_forward_backtest(
    conn: &Connection,
    stat_keys: &[String],
    config: &BacktestConfig,
    baseline_blend: f64,
    bucket_min_samples: usize,
) -> Result<TrainedAdaptiveModel, String> {
    let fallback_weights = default_v2_weights(baseline_blend);
    let backtest_samples = get_setting_i64(conn, "pattern_backtest_samples", 96).clamp(24, 192)
        as usize;
    let history_limit = (config.analysis_window + backtest_samples + 1).max(64);
    let history = load_recent_global_sequence(conn, history_limit)?;
    let warmup = config.min_len.max(8).max(config.max_order + 2);
    if history.len() <= warmup + 1 {
        return Ok(TrainedAdaptiveModel {
            fallback_weights,
            global_weights: fallback_weights,
            bucket_weights: HashMap::new(),
            bucket_min_samples,
            backtest_summary: empty_backtest_summary(),
        });
    }

    let last_idx = history.len() - 1;
    let start = warmup.max(last_idx.saturating_sub(backtest_samples));
    let mut samples = Vec::<(V2ComponentBuild, String)>::new();
    for idx in start..last_idx {
        let prefix_start = (idx + 1).saturating_sub(config.analysis_window);
        let prefix = &history[prefix_start..=idx];
        if prefix.len() < warmup {
            continue;
        }
        samples.push((
            build_v2_components(prefix, stat_keys, config, None),
            history[idx + 1].clone(),
        ));
    }

    if samples.len() < 12 {
        return Ok(TrainedAdaptiveModel {
            fallback_weights,
            global_weights: fallback_weights,
            bucket_weights: HashMap::new(),
            bucket_min_samples,
            backtest_summary: empty_backtest_summary(),
        });
    }

    let global_weights = fit_v2_weights_for_samples(&samples, baseline_blend);
    let mut grouped = HashMap::<String, Vec<(V2ComponentBuild, String)>>::new();
    for sample in &samples {
        grouped
            .entry(sample.0.bucket.key())
            .or_default()
            .push(sample.clone());
    }
    let mut bucket_weights = HashMap::<String, (InternalBlendWeights, usize)>::new();
    for (bucket_key, bucket_samples) in grouped {
        if bucket_samples.len() < bucket_min_samples {
            continue;
        }
        bucket_weights.insert(
            bucket_key,
            (
                fit_v2_weights_for_samples(&bucket_samples, baseline_blend),
                bucket_samples.len(),
            ),
        );
    }

    let temp_model = TrainedAdaptiveModel {
        fallback_weights,
        global_weights,
        bucket_weights,
        bucket_min_samples,
        backtest_summary: empty_backtest_summary(),
    };

    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;
    for (components, actual) in &samples {
        let (weights, _) = select_v2_weights_for_bucket(&temp_model, &components.bucket);
        let mixed = blend_v2_probs(stat_keys, components, weights);
        let actual_prob = mixed.get(actual).copied().unwrap_or(1e-9).clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();
        if top_prediction_key(&mixed) == Some(actual.as_str()) {
            top1_hits += 1.0;
        }
        let mut ranked = mixed.iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(Ordering::Equal));
        if ranked.iter().take(3).any(|(stat_key, _)| stat_key.as_str() == actual.as_str()) {
            top3_hits += 1.0;
        }
    }

    let sample_count = samples.len() as f64;
    Ok(TrainedAdaptiveModel {
        fallback_weights,
        global_weights,
        bucket_weights: temp_model.bucket_weights,
        bucket_min_samples,
        backtest_summary: PatternBacktestSummary {
            sample_count: samples.len() as i64,
            top1_accuracy: (top1_hits / sample_count).clamp(0.0, 1.0),
            top3_coverage: (top3_hits / sample_count).clamp(0.0, 1.0),
            mean_true_prob: (true_prob_sum / sample_count).clamp(0.0, 1.0),
            mean_log_loss: (log_loss_sum / sample_count).max(0.0),
        },
    })
}

fn expert_prob_blob(components: &V2ComponentBuild) -> HashMap<String, HashMap<String, f64>> {
    HashMap::from([
        ("base".to_string(), components.base_probs.clone()),
        ("markov".to_string(), components.markov_probs.clone()),
        ("exact_motif".to_string(), components.exact_motif_probs.clone()),
        ("approx_shape".to_string(), components.approx_shape_probs.clone()),
        ("auto_cycle".to_string(), components.auto_cycle_probs.clone()),
    ])
}

fn apply_online_adjustment(
    conn: &Connection,
    bucket: &PredictionBucket,
    weights: InternalBlendWeights,
    components: &V2ComponentBuild,
) -> Result<(InternalBlendWeights, bool), String> {
    if get_setting_i64(conn, "pattern_online_learning", 1) <= 0 {
        return Ok((weights, false));
    }

    let alpha = get_setting_f64(conn, "pattern_online_ewma_alpha", 0.12).clamp(0.02, 0.5);
    let cap = get_setting_f64(conn, "pattern_online_adjust_cap", 0.15).clamp(0.0, 0.4);

    let mut stmt = conn
        .prepare(
            "SELECT context_json, actual_stat_key
             FROM pattern_prediction_runs
             WHERE resolved_at IS NOT NULL
               AND actual_stat_key IS NOT NULL
               AND actual_stat_key <> '__invalid__'
             ORDER BY resolved_at DESC
             LIMIT 160",
        )
        .map_err(|e| format!("failed to prepare online adjustment query: {e}"))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("failed to query online adjustment rows: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect online adjustment rows: {e}"))?;

    let mut matched = Vec::<(StoredPredictionContext, String)>::new();
    for (context_json, actual_stat_key) in rows {
        let Ok(context) = serde_json::from_str::<StoredPredictionContext>(&context_json) else {
            continue;
        };
        if context.bucket.key() != bucket.key() {
            continue;
        }
        matched.push((context, actual_stat_key));
    }

    if matched.len() < 6 {
        return Ok((weights, false));
    }

    matched.reverse();
    let mut scores = HashMap::<String, f64>::new();
    for expert in ["base", "markov", "exact_motif", "approx_shape", "auto_cycle"] {
        scores.insert(expert.to_string(), 0.0);
    }
    let mut initialized = false;
    for (context, actual_stat_key) in matched {
        for expert in ["base", "markov", "exact_motif", "approx_shape", "auto_cycle"] {
            let expert_prob = context
                .expert_probs
                .get(expert)
                .and_then(|probs| probs.get(&actual_stat_key))
                .copied()
                .unwrap_or(1e-9)
                .clamp(1e-9, 1.0)
                .ln();
            let entry = scores.entry(expert.to_string()).or_insert(expert_prob);
            if initialized {
                *entry = alpha * expert_prob + (1.0 - alpha) * *entry;
            } else {
                *entry = expert_prob;
            }
        }
        initialized = true;
    }

    let mut adjusted = resolve_active_v2_weights(weights, components);
    let active_pairs = [
        ("base", adjusted.base),
        ("markov", adjusted.markov),
        ("exact_motif", adjusted.exact_motif),
        ("approx_shape", adjusted.approx_shape),
        ("auto_cycle", adjusted.auto_cycle),
    ]
    .into_iter()
    .filter(|(_, weight)| *weight > 0.0)
    .collect::<Vec<_>>();
    if active_pairs.len() <= 1 {
        return Ok((adjusted, false));
    }

    let mean_score = active_pairs
        .iter()
        .map(|(expert, _)| scores.get(*expert).copied().unwrap_or(0.0))
        .sum::<f64>()
        / active_pairs.len() as f64;
    let factor = |expert: &str| {
        ((scores.get(expert).copied().unwrap_or(mean_score) - mean_score) / 0.55)
            .exp()
            .clamp(1.0 - cap, 1.0 + cap)
    };
    adjusted.base *= factor("base");
    adjusted.markov *= factor("markov");
    adjusted.exact_motif *= factor("exact_motif");
    adjusted.approx_shape *= factor("approx_shape");
    adjusted.auto_cycle *= factor("auto_cycle");
    normalize_internal_weights(&mut adjusted);
    Ok((adjusted, true))
}

fn persist_pattern_prediction_run(
    conn: &Connection,
    game_day: &str,
    seq: &[String],
    weights: &PatternBlendWeights,
    components: &V2ComponentBuild,
    active_experts: &[String],
    suggestions: &[AdaptiveNextSuggestion],
) -> Result<(), String> {
    if get_setting_i64(conn, "pattern_online_learning", 1) <= 0 {
        return Ok(());
    }

    let tail_signature = build_tail_signature(seq);
    let context_hash = format!(
        "{}|{}|{}|adaptive_v2|0",
        game_day,
        seq.len(),
        tail_signature
    );
    let context = StoredPredictionContext {
        bucket: components.bucket.clone(),
        expert_probs: expert_prob_blob(components),
        weight_source: weights.source.clone(),
        active_experts: active_experts.to_vec(),
        tail_signature,
    };
    let predictions = suggestions
        .iter()
        .map(|row| StoredPredictionRow {
            stat_key: row.stat_key.clone(),
            probability: row.probability,
        })
        .collect::<Vec<_>>();

    conn.execute(
        "INSERT OR IGNORE INTO pattern_prediction_runs(
            run_id, context_hash, game_day, seq_len, mode, weights_json,
            predictions_json, context_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            Uuid::new_v4().to_string(),
            context_hash,
            game_day,
            seq.len() as i64,
            "adaptive_v2",
            serde_json::to_string(weights)
                .map_err(|e| format!("failed to serialize pattern weights: {e}"))?,
            serde_json::to_string(&predictions)
                .map_err(|e| format!("failed to serialize pattern predictions: {e}"))?,
            serde_json::to_string(&context)
                .map_err(|e| format!("failed to serialize pattern context: {e}"))?,
            now_rfc3339(),
        ],
    )
    .map_err(|e| format!("failed to persist pattern prediction run: {e}"))?;

    Ok(())
}

pub fn get_daily_pattern_decision_internal(
    conn: &Connection,
    filter: &DailyPatternDecisionFilter,
) -> Result<DailyPatternDecisionReport, String> {
    let filter = filter.clone();

    let analysis_window = get_setting_i64(conn, "analysis_window", 200).max(1) as usize;
    let baseline_blend = get_setting_f64(conn, "baseline_blend", 0.65).clamp(0.05, 0.95);
    let alpha = get_setting_f64(conn, "smoothing_alpha", 1.0).max(1e-9);
    let confidence_level = get_setting_f64(conn, "confidence_level", 0.95).clamp(0.5, 0.999);
    let motif_lambda = 1.15f64;
    let deployment_mode = get_setting_string(conn, "pattern_model_mode", "adaptive_v2");
    let bucket_min_samples = get_setting_i64(conn, "pattern_bucket_min_samples", 12).clamp(4, 64)
        as usize;
    let mut notes = vec!["当前模型按全局序列建模，不区分 Cost/主词条/状态。".to_string()];

    let min_len = filter.min_len.unwrap_or(3).clamp(2, 12) as usize;
    let max_len = filter.max_len.unwrap_or(7).clamp(min_len as i64, 16) as usize;
    let min_support = filter.min_support.unwrap_or(2).clamp(1, 50);
    let max_order = filter.max_order.unwrap_or(5).clamp(1, 8) as usize;
    let top_k = filter.top_k.unwrap_or(10).clamp(1, 30) as usize;

    let manual_start_index = filter.manual_start_index.unwrap_or(0).max(0) as usize;
    let manual_cycle_len = filter.manual_cycle_len.map(|v| v.clamp(2, 20) as usize);

    let mut manual_guess_shapes = Vec::<String>::new();
    let mut seen_shapes = HashSet::<String>::new();
    if let Some(raw_shapes) = &filter.manual_guess_shapes {
        for raw in raw_shapes {
            if let Some(shape) = canonicalize_shape(raw) {
                if seen_shapes.insert(shape.clone()) {
                    manual_guess_shapes.push(shape);
                }
            }
        }
    }

    let enabled_stats = list_enabled_stats(&conn)?;
    let stat_keys: Vec<String> = enabled_stats.iter().map(|(k, _)| k.clone()).collect();
    let display_map: HashMap<String, String> = enabled_stats.into_iter().collect();
    let backtest_config = BacktestConfig {
        analysis_window,
        min_len,
        max_len,
        min_support,
        max_order,
        alpha,
        motif_lambda,
    };
    let v2_model = run_walk_forward_backtest(
        conn,
        &stat_keys,
        &backtest_config,
        baseline_blend,
        bucket_min_samples,
    )?;
    if v2_model.backtest_summary.sample_count > 0 {
        let (global_weights, _) = select_v2_weights_for_bucket(
            &v2_model,
            &PredictionBucket {
                sample_depth_bucket: "100+".to_string(),
                markov_hit_bucket: "long".to_string(),
                motif_hit_bucket: "strong".to_string(),
            },
        );
        notes.push(format!(
            "V2 回测: samples={} · top1={:.1}% · top3={:.1}% · meanP={:.1}% · logloss={:.3} · global B/M/X/A/C = {:.0}/{:.0}/{:.0}/{:.0}/{:.0}",
            v2_model.backtest_summary.sample_count,
            v2_model.backtest_summary.top1_accuracy * 100.0,
            v2_model.backtest_summary.top3_coverage * 100.0,
            v2_model.backtest_summary.mean_true_prob * 100.0,
            v2_model.backtest_summary.mean_log_loss,
            global_weights.base * 100.0,
            global_weights.markov * 100.0,
            global_weights.exact_motif * 100.0,
            global_weights.approx_shape * 100.0,
            global_weights.auto_cycle * 100.0,
        ));
    } else {
        notes.push("V2 历史样本不足，当前使用默认融合权重。".to_string());
    }
    let report_model_mode = resolve_report_model_mode(&deployment_mode);
    let empty_blend_weights = empty_pattern_blend_weights();

    let game_day = resolve_game_day(&conn, &filter)?;
    if game_day.is_empty() {
        return Ok(DailyPatternDecisionReport {
            model_mode: report_model_mode.clone(),
            game_day,
            total_events: 0,
            min_len: min_len as i64,
            max_len: max_len as i64,
            min_support,
            max_order: max_order as i64,
            model_confidence: 0.0,
            blend_weights: empty_blend_weights.clone(),
            backtest_summary: v2_model.backtest_summary.clone(),
            active_experts: Vec::new(),
            exact_patterns: Vec::new(),
            shape_patterns: Vec::new(),
            suggestions: Vec::new(),
            manual_summary: None,
            notes,
        });
    }

    let mut seq = load_day_sequence(&conn, &game_day)?;
    let raw_seq_len = seq.len();
    if seq.len() > analysis_window {
        notes.push(format!(
            "已按 analysis_window={analysis_window} 截断为尾部序列（原始长度 {raw_seq_len}）。"
        ));
        let start = seq.len() - analysis_window;
        seq = seq[start..].to_vec();
    }

    let n = seq.len();
    if n == 0 {
        notes.push("该日无事件，无法识别模式。".to_string());
        return Ok(DailyPatternDecisionReport {
            model_mode: report_model_mode.clone(),
            game_day,
            total_events: 0,
            min_len: min_len as i64,
            max_len: max_len as i64,
            min_support,
            max_order: max_order as i64,
            model_confidence: 0.0,
            blend_weights: empty_blend_weights.clone(),
            backtest_summary: v2_model.backtest_summary.clone(),
            active_experts: Vec::new(),
            exact_patterns: Vec::new(),
            shape_patterns: Vec::new(),
            suggestions: Vec::new(),
            manual_summary: None,
            notes,
        });
    }

    let mut base_counts: HashMap<String, i64> = HashMap::new();
    for stat_key in &seq {
        *base_counts.entry(stat_key.clone()).or_insert(0) += 1;
    }

    let marginals: HashMap<String, f64> = stat_keys
        .iter()
        .map(|k| {
            (
                k.clone(),
                (*base_counts.get(k).unwrap_or(&0) as f64) / (n as f64),
            )
        })
        .collect();

    let baseline_max_len = max_len
        .max(manual_cycle_len.unwrap_or(max_len))
        .max(
            manual_guess_shapes
                .iter()
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(2),
        )
        .max(2);

    let mut exact_patterns_all = Vec::<DailyExactPatternRow>::new();
    let mut shape_map: HashMap<(i64, String), ShapeAggregate> = HashMap::new();
    let mut shape_baseline_counts: HashMap<usize, HashMap<String, i64>> = HashMap::new();
    let mut total_windows_by_len: HashMap<usize, i64> = HashMap::new();

    for len in 2..=baseline_max_len {
        if len > n {
            continue;
        }
        let windows = (n - len + 1) as i64;
        total_windows_by_len.insert(len, windows);

        let mut counts: HashMap<Vec<String>, i64> = HashMap::new();
        for i in 0..=n - len {
            let pattern = seq[i..i + len].to_vec();
            *counts.entry(pattern).or_insert(0) += 1;
        }

        let shape_count_map = shape_baseline_counts.entry(len).or_default();
        for (pattern, support) in counts {
            let shape = shape_signature(&pattern);
            *shape_count_map.entry(shape.clone()).or_insert(0) += support;

            if len < min_len || len > max_len || support < min_support {
                continue;
            }

            let expected_prob = pattern
                .iter()
                .map(|k| *marginals.get(k).unwrap_or(&0.0))
                .product::<f64>();
            let expected_count = expected_prob * windows as f64;
            let lift = if expected_count > 1e-9 {
                support as f64 / expected_count
            } else {
                0.0
            };
            let length_weight = (len as f64).powf(1.55);
            let score = support as f64 * lift.max(1e-9).ln() * length_weight;
            let display_pattern = pattern
                .iter()
                .map(|k| display_map.get(k).cloned().unwrap_or_else(|| k.clone()))
                .collect::<Vec<_>>();

            exact_patterns_all.push(DailyExactPatternRow {
                length: len as i64,
                pattern: pattern.clone(),
                display_pattern: display_pattern.clone(),
                shape: shape.clone(),
                support,
                window_count: windows,
                expected_count,
                lift,
                score,
            });

            let key = (len as i64, shape);
            let entry = shape_map.entry(key).or_default();
            entry.support += support;
            entry.expected_count += expected_count;
            entry
                .examples
                .push((display_pattern.join("→"), support, lift));
        }
    }

    let mut exact_patterns = exact_patterns_all.clone();
    exact_patterns.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.length.cmp(&a.length))
            .then_with(|| b.support.cmp(&a.support))
    });
    exact_patterns.truncate(top_k);

    let mut shape_patterns_all = shape_map
        .into_iter()
        .map(|((length, shape), mut agg)| {
            agg.examples.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal))
            });
            let examples = agg
                .examples
                .iter()
                .take(3)
                .map(|(pat, support, _)| format!("{pat} (n={support})"))
                .collect::<Vec<_>>();
            let lift = if agg.expected_count > 1e-9 {
                agg.support as f64 / agg.expected_count
            } else {
                0.0
            };
            let length_weight = (length as f64).powf(1.55);
            let score = agg.support as f64 * lift.max(1e-9).ln() * length_weight;
            DailyShapePatternRow {
                length,
                shape,
                support: agg.support,
                expected_count: agg.expected_count,
                lift,
                score,
                example_patterns: examples,
            }
        })
        .collect::<Vec<_>>();
    shape_patterns_all.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.length.cmp(&a.length))
            .then_with(|| b.support.cmp(&a.support))
    });
    let mut shape_patterns = shape_patterns_all.clone();
    shape_patterns.truncate(top_k);

    let effective_max_order = if n >= 2 {
        max_order.min(n - 1)
    } else {
        1
    };
    let k = stat_keys.len().max(1) as f64;
    let denom = n as f64 + alpha * k;
    let base_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|s| {
            let count = *base_counts.get(s).unwrap_or(&0) as f64;
            (s.clone(), (count + alpha) / denom)
        })
        .collect();
    let base_ci_map: HashMap<String, (f64, f64)> = stat_keys
        .iter()
        .map(|k| {
            let count = base_counts.get(k).copied().unwrap_or(0);
            let (low, high) = if n > 0 {
                wilson_interval(count, n as i64, confidence_level)
            } else {
                (0.0, 0.0)
            };
            (k.clone(), (low, high))
        })
        .collect();

    let mut markov_models: Vec<HashMap<Vec<String>, HashMap<String, i64>>> = Vec::new();
    for order in 1..=effective_max_order {
        let mut model: HashMap<Vec<String>, HashMap<String, i64>> = HashMap::new();
        if n > order {
            for i in order..n {
                let context = seq[i - order..i].to_vec();
                let next = seq[i].clone();
                *model.entry(context).or_default().entry(next).or_insert(0) += 1;
            }
        }
        markov_models.push(model);
    }

    let mut markov_acc: HashMap<String, f64> = stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut markov_weight_total = 0.0;

    if let Some(best_long_shape) = shape_patterns_all
        .iter()
        .filter(|row| row.length >= 4)
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal))
    {
        notes.push(format!(
            "全局长形态候选: L{} {} (n={}, lift={:.2})",
            best_long_shape.length,
            best_long_shape.shape,
            best_long_shape.support,
            best_long_shape.lift
        ));
    }

    for order in 1..=effective_max_order {
        if n < order {
            continue;
        }
        let context = seq[n - order..n].to_vec();
        if let Some(next_counts) = markov_models[order - 1].get(&context) {
            let total: i64 = next_counts.values().sum();
            if total <= 0 {
                continue;
            }
            let confidence = total as f64 / (total as f64 + 3.0 * order as f64);
            let weight = confidence * (order as f64).powf(1.35);
            if weight <= 0.0 {
                continue;
            }
            for stat_key in &stat_keys {
                let count = *next_counts.get(stat_key).unwrap_or(&0) as f64;
                let p = (count + alpha) / (total as f64 + alpha * k);
                if let Some(acc) = markov_acc.get_mut(stat_key) {
                    *acc += weight * p;
                }
            }
            let ctx_display = context
                .iter()
                .map(|v| display_map.get(v).cloned().unwrap_or_else(|| v.clone()))
                .collect::<Vec<_>>()
                .join("→");
            notes.push(format!(
                "命中 O{order} 上下文：{ctx_display} (样本 {total})"
            ));
            markov_weight_total += weight;
        }
    }

    let markov_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|key| {
            let p = if markov_weight_total > 0.0 {
                markov_acc.get(key).copied().unwrap_or(0.0) / markov_weight_total
            } else {
                *base_probs.get(key).unwrap_or(&0.0)
            };
            (key.clone(), p)
        })
        .collect();
    let baseline_markov_mix = if baseline_blend <= 0.0 {
        0.0
    } else {
        (markov_weight_total / (markov_weight_total + 2.0)).min(baseline_blend)
    };

    let mut cycle_probs: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut cycle_weight = 0.0;
    let mut manual_summary: Option<ManualPatternSummary> = None;
    let mut manual_hint_contribs: Vec<(String, f64, i64, String)> = Vec::new();

    if let Some(cycle_len) = manual_cycle_len {
        if manual_start_index >= n {
            notes.push("手动起点超出当日事件长度，已忽略手动周期分析。".to_string());
        } else {
            let tail = &seq[manual_start_index..];
            let full_cycles = tail.len() / cycle_len;
            let next_cycle_pos = (tail.len() % cycle_len) + 1;

            let mut cycle_shape_counts: HashMap<String, i64> = HashMap::new();
            for cycle_idx in 0..full_cycles {
                let start = cycle_idx * cycle_len;
                let end = start + cycle_len;
                let cycle = &tail[start..end];
                let shape = shape_signature(cycle);
                *cycle_shape_counts.entry(shape).or_insert(0) += 1;
            }
            let mut top_cycle_shapes = cycle_shape_counts.into_iter().collect::<Vec<_>>();
            top_cycle_shapes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            top_cycle_shapes.truncate(top_k);

            let mut pos_counts: HashMap<String, i64> = HashMap::new();
            let mut pos_total = 0i64;
            for (idx, stat) in tail.iter().enumerate() {
                if (idx % cycle_len) + 1 == next_cycle_pos {
                    *pos_counts.entry(stat.clone()).or_insert(0) += 1;
                    pos_total += 1;
                }
            }

            let mut position_suggestions = Vec::<ManualCycleSuggestion>::new();
            if pos_total > 0 {
                cycle_weight = ((full_cycles as f64) / (full_cycles as f64 + 1.5) * 0.45).min(0.45);
                for stat in &stat_keys {
                    let count = *pos_counts.get(stat).unwrap_or(&0);
                    let p = (count as f64 + alpha) / (pos_total as f64 + alpha * k);
                    if let Some(v) = cycle_probs.get_mut(stat) {
                        *v = p;
                    }
                    if count > 0 {
                        position_suggestions.push(ManualCycleSuggestion {
                            stat_key: stat.clone(),
                            display_name: display_map
                                .get(stat)
                                .cloned()
                                .unwrap_or_else(|| stat.clone()),
                            count,
                            probability: p,
                        });
                    }
                }
                position_suggestions.sort_by(|a, b| {
                    b.probability
                        .partial_cmp(&a.probability)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| b.count.cmp(&a.count))
                });
                position_suggestions.truncate(top_k);
            }

            let mut guess_rows = Vec::<ManualGuessVerificationRow>::new();
            for guess in &manual_guess_shapes {
                let length = guess.chars().count();
                let mut opportunities = 0i64;
                let mut support = 0i64;
                let mut matched_cycles = Vec::<i64>::new();

                if full_cycles > 0 && length <= cycle_len {
                    for cycle_idx in 0..full_cycles {
                        let start = cycle_idx * cycle_len;
                        let end = start + cycle_len;
                        let cycle = &tail[start..end];
                        let mut cycle_hit = false;
                        for local in 0..=(cycle_len - length) {
                            let sub = &cycle[local..local + length];
                            opportunities += 1;
                            let sub_refs = sub.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                            let shape = shape_signature_slice(&sub_refs);
                            if shape == *guess {
                                support += 1;
                                cycle_hit = true;
                            }
                        }
                        if cycle_hit {
                            matched_cycles.push(cycle_idx as i64 + 1);
                        }
                    }
                }

                let baseline_rate = if let Some(shape_counts) = shape_baseline_counts.get(&length) {
                    let total = *total_windows_by_len.get(&length).unwrap_or(&0);
                    if total > 0 {
                        *shape_counts.get(guess).unwrap_or(&0) as f64 / total as f64
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                let hit_rate = if opportunities > 0 {
                    support as f64 / opportunities as f64
                } else {
                    0.0
                };
                let lift = if baseline_rate > 1e-9 {
                    hit_rate / baseline_rate
                } else {
                    0.0
                };
                let next_hint = infer_hint_from_guess(&seq, guess);

                if let Some(ref hinted_stat) = next_hint {
                    let strength = (lift - 1.0).max(0.0)
                        * (length as f64).powf(1.5)
                        * if opportunities > 0 {
                            (support as f64 / opportunities as f64).max(0.05)
                        } else {
                            0.05
                        };
                    if strength > 0.0 {
                        manual_hint_contribs.push((
                            hinted_stat.clone(),
                            strength,
                            length as i64,
                            format!("手动猜测 {guess} (lift={lift:.2})"),
                        ));
                    }
                }

                matched_cycles.truncate(12);
                guess_rows.push(ManualGuessVerificationRow {
                    guess_shape: guess.clone(),
                    length: length as i64,
                    support,
                    opportunities,
                    hit_rate,
                    baseline_rate,
                    lift,
                    matched_cycle_indices: matched_cycles,
                    next_stat_hint: next_hint,
                });
            }

            guess_rows.sort_by(|a, b| {
                b.lift
                    .partial_cmp(&a.lift)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| b.length.cmp(&a.length))
                    .then_with(|| b.support.cmp(&a.support))
            });

            if full_cycles > 0 {
                notes.push(format!(
                    "手动周期分析启用: start={}, L={}, full_cycles={}",
                    manual_start_index, cycle_len, full_cycles
                ));
            } else {
                notes.push("手动周期分析启用，但完整周期数量为 0。".to_string());
            }

            manual_summary = Some(ManualPatternSummary {
                start_index: manual_start_index as i64,
                cycle_len: cycle_len as i64,
                full_cycles: full_cycles as i64,
                next_cycle_pos: next_cycle_pos as i64,
                top_cycle_shapes,
                guesses: guess_rows,
                position_suggestions,
            });
        }
    } else if !manual_guess_shapes.is_empty() {
        notes.push("已提供猜测形态，但未设置假设手数，猜测仅用于尾序列提示。".to_string());
        for guess in &manual_guess_shapes {
            if let Some(hinted_stat) = infer_hint_from_guess(&seq, guess) {
                manual_hint_contribs.push((
                    hinted_stat,
                    guess.len() as f64 * 0.15,
                    guess.len() as i64,
                    format!("手动猜测 {guess} (尾序列提示)"),
                ));
            }
        }
    }

    let mut matched_exact = Vec::<(String, f64, i64, String)>::new();
    for row in &exact_patterns_all {
        if row.length < 3 {
            continue;
        }
        let prefix = &row.pattern[..row.pattern.len() - 1];
        if !ends_with_pattern(&seq, prefix) {
            continue;
        }
        let next_stat = row.pattern.last().cloned().unwrap_or_default();
        if next_stat.is_empty() {
            continue;
        }
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let density = row.support as f64 / row.window_count.max(1) as f64;
        let short_penalty = if row.length <= 3 { 0.55 } else { 1.0 };
        let boost = lift_gain
            * (row.length as f64).powf(1.6)
            * (row.support as f64).ln_1p()
            * density.sqrt()
            * short_penalty;
        if boost <= 0.0 {
            continue;
        }
        matched_exact.push((
            next_stat,
            boost,
            row.length,
            format!("{} [{}]", row.display_pattern.join("→"), row.shape),
        ));
    }

    let mut matched_shape = Vec::<(String, f64, i64, String)>::new();
    for row in &shape_patterns_all {
        if row.length < 4 {
            continue;
        }
        let prefix_len = (row.length - 1) as usize;
        if prefix_len == 0 || prefix_len > seq.len() {
            continue;
        }
        let suffix = &seq[seq.len() - prefix_len..];
        let Some(next_stat) = infer_next_stat_from_shape(&row.shape, suffix) else {
            continue;
        };
        let lift_gain = (row.lift - 1.0).max(0.0);
        if lift_gain <= 0.0 {
            continue;
        }
        let boost = lift_gain * (row.support as f64).ln_1p() * (row.length as f64).powf(1.7);
        if boost <= 0.0 {
            continue;
        }
        matched_shape.push((
            next_stat.clone(),
            boost,
            row.length,
            format!("形态 {} [L{},n={}]", row.shape, row.length, row.support),
        ));
    }

    let mut raw_boost_map: HashMap<String, f64> =
        stat_keys.iter().map(|s| (s.clone(), 0.0)).collect();
    let mut matched_patterns_map: HashMap<String, Vec<String>> = HashMap::new();

    let all_contribs = matched_exact
        .into_iter()
        .chain(matched_shape.into_iter())
        .chain(manual_hint_contribs.into_iter())
        .collect::<Vec<_>>();

    let longest_match_len = all_contribs
        .iter()
        .map(|(_, _, len, _)| *len)
        .max()
        .unwrap_or(0) as f64;

    if longest_match_len >= 4.0 {
        notes.push(format!(
            "当前尾序列命中长模式，按最长 L{} 优先融合。",
            longest_match_len as i64
        ));
    }

    for (stat_key, boost, len, label) in all_contribs {
        let len_factor = if longest_match_len > 0.0 {
            (len as f64 / longest_match_len).powf(2.2)
        } else {
            1.0
        };
        let adjusted = boost * len_factor;
        if adjusted <= 0.0 {
            continue;
        }
        if let Some(v) = raw_boost_map.get_mut(&stat_key) {
            *v += adjusted;
        }
        matched_patterns_map
            .entry(stat_key)
            .or_default()
            .push(format!("{label} · w={adjusted:.2}"));
    }

    let max_boost = raw_boost_map.values().copied().fold(0.0, f64::max);
    let v2_components = build_v2_components(&seq, &stat_keys, &backtest_config, Some(&display_map));
    for (stat_key, labels) in &v2_components.matched_patterns_map {
        matched_patterns_map
            .entry(stat_key.clone())
            .or_default()
            .extend(labels.clone());
    }
    let mut matched_experts_map = v2_components.matched_experts_map.clone();
    if let Some(manual) = &manual_summary {
        for row in &manual.position_suggestions {
            push_label(
                &mut matched_patterns_map,
                &row.stat_key,
                format!("手动周期 next@pos {:.0}% (n={})", row.probability * 100.0, row.count),
            );
            push_source(&mut matched_experts_map, &row.stat_key, "manual_cycle");
        }
    }
    let (selected_weights_raw, weight_source) =
        select_v2_weights_for_bucket(&v2_model, &v2_components.bucket);
    let (selected_weights_online, online_adjusted) =
        apply_online_adjustment(conn, &v2_components.bucket, selected_weights_raw, &v2_components)?;
    let selected_weights = resolve_active_v2_weights(selected_weights_online, &v2_components);
    let blend_weights =
        internal_weights_to_public(selected_weights, weight_source, &v2_components.bucket, online_adjusted);
    let active_experts = active_v2_experts(&v2_components, &selected_weights);
    notes.push(format!(
        "当前上下文桶 {} / {} / {} · 权重源 {}{}",
        blend_weights.sample_depth_bucket,
        blend_weights.markov_hit_bucket,
        blend_weights.motif_hit_bucket,
        blend_weights.source,
        if blend_weights.online_adjusted { " · online+" } else { "" }
    ));

    let mut suggestions = Vec::<AdaptiveNextSuggestion>::new();
    let mut total_score = 0.0;
    let mut adaptive_auto_predictions = Vec::<(String, f64)>::new();
    let mut adaptive_auto_total = 0.0;
    let interval_total = n.max(1) as i64;
    for stat_key in &stat_keys {
        let base = *base_probs.get(stat_key).unwrap_or(&0.0);
        let markov = *markov_probs.get(stat_key).unwrap_or(&base);
        let exact_motif = *v2_components.exact_motif_probs.get(stat_key).unwrap_or(&base);
        let approx_shape = *v2_components.approx_shape_probs.get(stat_key).unwrap_or(&base);
        let auto_cycle = *v2_components.auto_cycle_probs.get(stat_key).unwrap_or(&base);
        let cycle = if cycle_weight > 0.0 {
            *cycle_probs.get(stat_key).unwrap_or(&base)
        } else {
            auto_cycle
        };

        let raw_boost = *raw_boost_map.get(stat_key).unwrap_or(&0.0);
        let norm_boost = if max_boost > 1e-9 {
            raw_boost / max_boost
        } else {
            0.0
        };
        let adaptive_mix = selected_weights.base * base
            + selected_weights.markov * markov
            + selected_weights.exact_motif * exact_motif
            + selected_weights.approx_shape * approx_shape
            + selected_weights.auto_cycle * auto_cycle;
        let baseline_auto =
            ((1.0 - baseline_markov_mix) * base + baseline_markov_mix * markov)
                * (1.0 + motif_lambda * norm_boost);
        let adaptive_score = (1.0 - cycle_weight) * adaptive_mix + cycle_weight * cycle;
        let score = if report_model_mode == "adaptive_v2" {
            adaptive_score
        } else {
            (1.0 - cycle_weight) * baseline_auto + cycle_weight * cycle
        };
        adaptive_auto_predictions.push((stat_key.clone(), adaptive_score));
        adaptive_auto_total += adaptive_score;
        total_score += score;
        let pseudo_success = ((score * interval_total as f64).round())
            .clamp(0.0, interval_total as f64) as i64;
        let (ci_low, ci_high) = wilson_interval(pseudo_success, interval_total, confidence_level);
        let ci_width = ci_high - ci_low;
        let (base_ci_low, base_ci_high) = base_ci_map.get(stat_key).copied().unwrap_or((0.0, 0.0));
        let base_uncertainty = (base_ci_high - base_ci_low).clamp(0.0, 1.0);
        let confidence = (1.0 - (ci_width * 0.8 + base_uncertainty * 0.2)).clamp(0.0, 1.0);

        let mut matched = matched_patterns_map.remove(stat_key).unwrap_or_default();
        matched.sort();
        matched.dedup();
        matched.truncate(4);
        let mut matched_experts = matched_experts_map.remove(stat_key).unwrap_or_default();
        matched_experts.sort();
        matched_experts.dedup();

        suggestions.push(AdaptiveNextSuggestion {
            stat_key: stat_key.clone(),
            display_name: display_map
                .get(stat_key)
                .cloned()
                .unwrap_or_else(|| stat_key.clone()),
            probability: score,
            base_probability: base,
            markov_probability: markov,
            cycle_probability: cycle,
            motif_boost: norm_boost,
            confidence,
            probability_ci_low: ci_low,
            probability_ci_high: ci_high,
            matched_patterns: matched,
            matched_experts,
        });
    }

    if total_score > 1e-12 {
        for row in &mut suggestions {
            row.probability /= total_score;
        }
    }
    suggestions.sort_by(|a, b| {
        b.probability
            .partial_cmp(&a.probability)
            .unwrap_or(Ordering::Equal)
    });
    suggestions.truncate(top_k);

    let sample_conf = (n as f64 / (n as f64 + 20.0)).min(1.0);
    let markov_conf = if markov_weight_total > 0.0 {
        blend_weights.markov.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cycle_conf = if cycle_weight > 0.0 {
        (cycle_weight / 0.45).min(1.0)
    } else {
        blend_weights.auto_cycle.clamp(0.0, 1.0)
    };
    let motif_conf = if v2_components.motif_strength > 0.0 {
        ((blend_weights.exact_motif + blend_weights.approx_shape)
            * (1.0 - 1.0 / (1.0 + v2_components.motif_strength.max(v2_components.approx_strength))))
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let backtest_sample_conf =
        v2_model.backtest_summary.sample_count as f64
            / (v2_model.backtest_summary.sample_count as f64 + 24.0);
    let logloss_conf = (1.0 - (v2_model.backtest_summary.mean_log_loss / 1.6)).clamp(0.0, 1.0);
    let backtest_conf = ((v2_model.backtest_summary.top1_accuracy * 0.55
        + v2_model.backtest_summary.top3_coverage * 0.20
        + logloss_conf * 0.25)
        * backtest_sample_conf)
        .clamp(0.0, 1.0);
    let model_confidence = (0.25 * sample_conf
        + 0.20 * markov_conf
        + 0.15 * cycle_conf
        + 0.15 * motif_conf
        + 0.25 * backtest_conf)
        .min(1.0);

    if n < 20 {
        notes.push("当日样本较少，系统会自动回退到基础概率。".to_string());
    }
    if markov_weight_total <= 0.0 {
        notes.push("未命中可用上下文，预测主要由基础分布驱动。".to_string());
    }
    if deployment_mode == "v2_shadow" {
        notes.push("当前 pattern_model_mode=v2_shadow；本地仍输出 adaptive_v2 结果用于观测。".to_string());
    } else if deployment_mode == "v2_canary" {
        notes.push("当前 pattern_model_mode=v2_canary。".to_string());
    }

    let manual_mode = manual_cycle_len.is_some() || !manual_guess_shapes.is_empty();
    if !manual_mode {
        let mut logging_suggestions = if report_model_mode == "adaptive_v2" {
            suggestions.clone()
        } else {
            let mut rows = adaptive_auto_predictions
                .into_iter()
                .map(|(stat_key, probability)| {
                    let normalized = if adaptive_auto_total > 1e-12 {
                        probability / adaptive_auto_total
                    } else {
                        0.0
                    };
                    AdaptiveNextSuggestion {
                        stat_key: stat_key.clone(),
                        display_name: display_map
                            .get(&stat_key)
                            .cloned()
                            .unwrap_or_else(|| stat_key.clone()),
                        probability: normalized,
                        base_probability: *base_probs.get(&stat_key).unwrap_or(&0.0),
                        markov_probability: *markov_probs.get(&stat_key).unwrap_or(&0.0),
                        cycle_probability: *v2_components.auto_cycle_probs.get(&stat_key).unwrap_or(&0.0),
                        motif_boost: *raw_boost_map.get(&stat_key).unwrap_or(&0.0),
                        confidence: 0.0,
                        probability_ci_low: 0.0,
                        probability_ci_high: 0.0,
                        matched_patterns: Vec::new(),
                        matched_experts: Vec::new(),
                    }
                })
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                b.probability
                    .partial_cmp(&a.probability)
                    .unwrap_or(Ordering::Equal)
            });
            rows.truncate(top_k);
            rows
        };
        logging_suggestions.truncate(top_k);
        persist_pattern_prediction_run(
            conn,
            &game_day,
            &seq,
            &blend_weights,
            &v2_components,
            &active_experts,
            &logging_suggestions,
        )?;
    }

    Ok(DailyPatternDecisionReport {
        model_mode: report_model_mode,
        game_day,
        total_events: n as i64,
        min_len: min_len as i64,
        max_len: max_len as i64,
        min_support,
        max_order: effective_max_order as i64,
        model_confidence,
        blend_weights,
        backtest_summary: v2_model.backtest_summary.clone(),
        active_experts,
        exact_patterns,
        shape_patterns,
        suggestions,
        manual_summary,
        notes,
    })
}

#[tauri::command]
pub fn get_daily_pattern_decision(
    state: State<'_, AppState>,
    filter: Option<DailyPatternDecisionFilter>,
) -> Result<DailyPatternDecisionReport, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();
    get_daily_pattern_decision_internal(&conn, &filter)
}

pub fn resolve_prediction_run_after_append(
    tx: &Transaction<'_>,
    game_day: &str,
    seq_len: i64,
    actual_event_id: &str,
    actual_stat_key: &str,
) -> Result<(), String> {
    let mut stmt = tx
        .prepare(
            "SELECT run_id, predictions_json
             FROM pattern_prediction_runs
             WHERE game_day = ?1
               AND seq_len = ?2
               AND actual_stat_key IS NULL",
        )
        .map_err(|e| format!("failed to prepare prediction resolve query: {e}"))?;
    let rows = stmt
        .query_map(params![game_day, seq_len], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("failed to query prediction resolve rows: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect prediction resolve rows: {e}"))?;
    let now = now_rfc3339();

    for (run_id, predictions_json) in rows {
        let predictions =
            serde_json::from_str::<Vec<StoredPredictionRow>>(&predictions_json).unwrap_or_default();
        let actual_prob = predictions
            .iter()
            .find(|row| row.stat_key == actual_stat_key)
            .map(|row| row.probability)
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        let top1_hit = predictions
            .first()
            .map(|row| row.stat_key == actual_stat_key)
            .unwrap_or(false);
        let top3_hit = predictions
            .iter()
            .take(3)
            .any(|row| row.stat_key == actual_stat_key);
        tx.execute(
            "UPDATE pattern_prediction_runs
             SET actual_stat_key = ?2,
                 actual_event_id = ?3,
                 top1_hit = ?4,
                 top3_hit = ?5,
                 log_loss = ?6,
                 resolved_at = ?7
             WHERE run_id = ?1",
            params![
                run_id,
                actual_stat_key,
                actual_event_id,
                if top1_hit { 1 } else { 0 },
                if top3_hit { 1 } else { 0 },
                -actual_prob.ln(),
                now,
            ],
        )
        .map_err(|e| format!("failed to resolve pattern prediction run: {e}"))?;
    }

    Ok(())
}

pub fn invalidate_unresolved_prediction_runs_for_game_day(
    tx: &Transaction<'_>,
    game_day: &str,
) -> Result<(), String> {
    tx.execute(
        "UPDATE pattern_prediction_runs
         SET actual_stat_key = '__invalid__',
             actual_event_id = NULL,
             top1_hit = NULL,
             top3_hit = NULL,
             log_loss = NULL,
             resolved_at = ?2
         WHERE game_day = ?1
           AND actual_stat_key IS NULL",
        params![game_day, now_rfc3339()],
    )
    .map_err(|e| format!("failed to invalidate unresolved pattern prediction runs: {e}"))?;
    Ok(())
}
