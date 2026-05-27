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
    ManualGuessVerificationRow, ManualPatternSummary, PatternBenchmarkRow, PatternBacktestSummary,
    PatternBlendWeights, PatternIntervalSignal,
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
        .prepare("SELECT stat_key, display_name FROM stat_defs WHERE enabled = 1 AND stat_key IN (SELECT DISTINCT stat_key FROM stat_tiers) ORDER BY rowid")
        .map_err(|e| format!("failed to prepare stat_defs query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("failed to query stat_defs: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect stat_defs: {e}"))
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

fn top_prediction_key<'a>(map: &'a HashMap<String, f64>) -> Option<&'a str> {
    map.iter()
        .max_by(|a, b| {
            a.1.partial_cmp(b.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.0.cmp(a.0))
        })
        .map(|(k, _)| k.as_str())
}

fn sorted_prediction_ranking<'a>(map: &'a HashMap<String, f64>) -> Vec<(&'a String, &'a f64)> {
    let mut ranked = map.iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(b.0))
    });
    ranked
}

#[derive(Clone, Copy, Default)]
struct FlatBacktestMetrics {
    top1_accuracy: f64,
    top3_coverage: f64,
    mean_true_prob: f64,
    mean_log_loss: f64,
}

fn finalize_flat_metrics(
    sample_count: f64,
    top1_hits: f64,
    top3_hits: f64,
    true_prob_sum: f64,
    log_loss_sum: f64,
) -> FlatBacktestMetrics {
    if sample_count <= 0.0 {
        return FlatBacktestMetrics::default();
    }
    FlatBacktestMetrics {
        top1_accuracy: (top1_hits / sample_count).clamp(0.0, 1.0),
        top3_coverage: (top3_hits / sample_count).clamp(0.0, 1.0),
        mean_true_prob: (true_prob_sum / sample_count).clamp(0.0, 1.0),
        mean_log_loss: (log_loss_sum / sample_count).max(0.0),
    }
}

fn flat_metrics_to_benchmark_row(
    key: &str,
    label: &str,
    metrics: FlatBacktestMetrics,
) -> PatternBenchmarkRow {
    PatternBenchmarkRow {
        key: key.to_string(),
        label: label.to_string(),
        top1_accuracy: metrics.top1_accuracy,
        top3_coverage: metrics.top3_coverage,
        mean_true_prob: metrics.mean_true_prob,
        mean_log_loss: metrics.mean_log_loss,
    }
}

fn evaluate_probability_map_benchmark<F>(
    samples: &[BacktestSample],
    key: &str,
    label: &str,
    mut select_map: F,
) -> PatternBenchmarkRow
where
    F: FnMut(&BacktestSample) -> HashMap<String, f64>,
{
    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;

    for sample in samples {
        let mut masked = select_map(sample);
        apply_blocked_stat_mask(&mut masked, &sample.blocked_stats);
        let actual_prob = masked
            .get(&sample.actual_stat_key)
            .copied()
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();

        let ranked = sorted_prediction_ranking(&masked);
        if ranked
            .first()
            .map(|(stat_key, _)| stat_key.as_str())
            == Some(sample.actual_stat_key.as_str())
        {
            top1_hits += 1.0;
        }
        if ranked
            .iter()
            .take(3)
            .any(|(stat_key, _)| stat_key.as_str() == sample.actual_stat_key.as_str())
        {
            top3_hits += 1.0;
        }
    }

    flat_metrics_to_benchmark_row(
        key,
        label,
        finalize_flat_metrics(
            samples.len() as f64,
            top1_hits,
            top3_hits,
            true_prob_sum,
            log_loss_sum,
        ),
    )
}

fn evaluate_frequency_baseline(samples: &[BacktestSample]) -> FlatBacktestMetrics {
    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;

    for sample in samples {
        let mut masked = sample.components.base_probs.clone();
        apply_blocked_stat_mask(&mut masked, &sample.blocked_stats);
        let actual_prob = masked
            .get(&sample.actual_stat_key)
            .copied()
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();

        let ranked = sorted_prediction_ranking(&masked);
        if ranked
            .first()
            .map(|(stat_key, _)| stat_key.as_str())
            == Some(sample.actual_stat_key.as_str())
        {
            top1_hits += 1.0;
        }
        if ranked
            .iter()
            .take(3)
            .any(|(stat_key, _)| stat_key.as_str() == sample.actual_stat_key.as_str())
        {
            top3_hits += 1.0;
        }
    }

    finalize_flat_metrics(
        samples.len() as f64,
        top1_hits,
        top3_hits,
        true_prob_sum,
        log_loss_sum,
    )
}

fn evaluate_random_baseline(samples: &[BacktestSample]) -> FlatBacktestMetrics {
    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;

    for sample in samples {
        let allowed_count = sample
            .components
            .base_probs
            .len()
            .saturating_sub(sample.blocked_stats.len())
            .max(1) as f64;
        let actual_prob = (1.0 / allowed_count).clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();
        top1_hits += actual_prob;
        top3_hits += (3.0 / allowed_count).min(1.0);
    }

    finalize_flat_metrics(
        samples.len() as f64,
        top1_hits,
        top3_hits,
        true_prob_sum,
        log_loss_sum,
    )
}

fn build_benchmark_rows(
    samples: &[BacktestSample],
    model_metrics: FlatBacktestMetrics,
    include_state_context: bool,
) -> Vec<PatternBenchmarkRow> {
    let mut rows = vec![
        flat_metrics_to_benchmark_row("model", "当前模型", model_metrics),
        flat_metrics_to_benchmark_row("freq", "频率基线", evaluate_frequency_baseline(samples)),
        flat_metrics_to_benchmark_row("random", "随机均匀", evaluate_random_baseline(samples)),
        evaluate_probability_map_benchmark(samples, "markov", "Markov 专家", |sample| {
            sample.components.markov_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "exact_motif", "精确 Motif", |sample| {
            sample.components.exact_motif_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "approx_shape", "形状近邻", |sample| {
            sample.components.approx_shape_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "auto_cycle", "自动周期", |sample| {
            sample.components.auto_cycle_probs.clone()
        }),
    ];
    if include_state_context {
        rows.push(evaluate_probability_map_benchmark(
            samples,
            "state_context",
            "状态上下文",
            |sample| sample.components.state_context_probs.clone(),
        ));
    }
    rows
}

#[derive(Clone)]
struct BacktestEventLite {
    echo_id: String,
    stat_key: String,
    tier_index: i64,
    slot_no: i64,
    analysis_seq: i64,
}

#[derive(Clone)]
struct BacktestSample {
    components: V2ComponentBuild,
    actual_stat_key: String,
    blocked_stats: HashSet<String>,
}

fn masked_probability_for_stat(
    map: &HashMap<String, f64>,
    stat_key: &str,
    blocked_stats: &HashSet<String>,
) -> f64 {
    if blocked_stats.contains(stat_key) {
        return 1e-9;
    }
    let allowed_mass = map
        .iter()
        .filter(|(key, _)| !blocked_stats.contains(*key))
        .map(|(_, value)| *value)
        .sum::<f64>();
    if allowed_mass <= 1e-12 {
        return 1e-9;
    }
    (map.get(stat_key).copied().unwrap_or(0.0) / allowed_mass).clamp(1e-9, 1.0)
}

fn apply_blocked_stat_mask(map: &mut HashMap<String, f64>, blocked_stats: &HashSet<String>) {
    if blocked_stats.is_empty() {
        normalize_probability_map(map);
        return;
    }

    let mut remaining_mass = 0.0;
    let mut allowed_count = 0usize;
    for (stat_key, value) in map.iter_mut() {
        if blocked_stats.contains(stat_key) {
            *value = 0.0;
        } else {
            remaining_mass += *value;
            allowed_count += 1;
        }
    }

    if remaining_mass > 1e-12 {
        for value in map.values_mut() {
            *value /= remaining_mass;
        }
        return;
    }

    if allowed_count == 0 {
        return;
    }
    let uniform = 1.0 / allowed_count as f64;
    for (stat_key, value) in map.iter_mut() {
        *value = if blocked_stats.contains(stat_key) {
            0.0
        } else {
            uniform
        };
    }
}

fn apply_blocked_stats_to_suggestions(
    suggestions: &mut Vec<AdaptiveNextSuggestion>,
    blocked_stats: &HashSet<String>,
    top_k: usize,
) {
    if !blocked_stats.is_empty() {
        suggestions.retain(|row| !blocked_stats.contains(&row.stat_key));
        let remaining_mass = suggestions.iter().map(|row| row.probability).sum::<f64>();
        if remaining_mass > 1e-12 {
            for row in suggestions.iter_mut() {
                row.probability = (row.probability / remaining_mass).clamp(0.0, 1.0);
                row.joint_probability = (row.joint_probability / remaining_mass).max(0.0);
                row.probability_ci_low = (row.probability_ci_low / remaining_mass).clamp(0.0, 1.0);
                row.probability_ci_high = (row.probability_ci_high / remaining_mass).clamp(0.0, 1.0);
            }
        }
    }
    suggestions.truncate(top_k);
}

fn load_recent_global_backtest_events(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<BacktestEventLite>, String> {
    let limit = limit.max(1) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT echo_id, stat_key, tier_index, slot_no, analysis_seq
             FROM (
               SELECT echo_id, stat_key, tier_index, slot_no, analysis_seq
               FROM ordered_events
               ORDER BY analysis_seq DESC
               LIMIT ?1
             )
             ORDER BY analysis_seq ASC",
        )
        .map_err(|e| format!("failed to prepare recent backtest event query: {e}"))?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(BacktestEventLite {
                echo_id: row.get::<_, String>(0)?,
                stat_key: row.get::<_, String>(1)?,
                tier_index: row.get::<_, i64>(2)?,
                slot_no: row.get::<_, i64>(3)?,
                analysis_seq: row.get::<_, i64>(4)?,
            })
        })
        .map_err(|e| format!("failed to query recent backtest events: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect recent backtest events: {e}"))
}

fn load_echo_blocked_stats(conn: &Connection, echo_id: &str) -> Result<HashSet<String>, String> {
    let echo_exists = conn
        .query_row("SELECT echo_id FROM echoes WHERE echo_id = ?1", [echo_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|e| format!("failed to query echo context: {e}"))?;
    if echo_exists.is_none() {
        return Ok(HashSet::new()); // Default to no blocked stats if the echo doesn't exist anymore
    }

    let mut stmt = conn
        .prepare("SELECT stat_key FROM echo_current_substats WHERE echo_id = ?1")
        .map_err(|e| format!("failed to prepare echo substat context query: {e}"))?;
    let rows = stmt
        .query_map([echo_id], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query echo substat context: {e}"))?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|e| format!("failed to collect echo substat context: {e}"))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PredictionBucket {
    sample_depth_bucket: String,
    markov_hit_bucket: String,
    motif_hit_bucket: String,
    active_stat_bucket: String,
    tier_signal_bucket: String,
}

impl PredictionBucket {
    fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.sample_depth_bucket,
            self.markov_hit_bucket,
            self.motif_hit_bucket,
            self.active_stat_bucket,
            self.tier_signal_bucket
        )
    }

    fn scoped_key(&self, scope: BucketScope) -> String {
        match scope {
            BucketScope::Full5d => self.key(),
            BucketScope::Active4d => format!(
                "4a:{}|{}|{}|{}",
                self.sample_depth_bucket,
                self.markov_hit_bucket,
                self.motif_hit_bucket,
                self.active_stat_bucket
            ),
            BucketScope::Context4d => format!(
                "4c:{}|{}|{}|{}",
                self.sample_depth_bucket,
                self.markov_hit_bucket,
                self.motif_hit_bucket,
                self.tier_signal_bucket
            ),
            BucketScope::Core3d => format!(
                "3:{}|{}|{}",
                self.sample_depth_bucket,
                self.markov_hit_bucket,
                self.motif_hit_bucket
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum BucketScope {
    Core3d,
    Active4d,
    Context4d,
    Full5d,
}

impl BucketScope {
    fn training_scopes() -> [BucketScope; 4] {
        [
            BucketScope::Full5d,
            BucketScope::Active4d,
            BucketScope::Context4d,
            BucketScope::Core3d,
        ]
    }

    fn blend_scopes() -> [BucketScope; 4] {
        [
            BucketScope::Core3d,
            BucketScope::Active4d,
            BucketScope::Context4d,
            BucketScope::Full5d,
        ]
    }

    fn objective_scopes() -> [BucketScope; 4] {
        [
            BucketScope::Full5d,
            BucketScope::Active4d,
            BucketScope::Context4d,
            BucketScope::Core3d,
        ]
    }

    fn source_label(self) -> &'static str {
        match self {
            BucketScope::Core3d => "bucketed3d",
            BucketScope::Active4d | BucketScope::Context4d => "bucketed4d",
            BucketScope::Full5d => "bucketed5d",
        }
    }

    fn specificity_rank(self) -> usize {
        match self {
            BucketScope::Core3d => 3,
            BucketScope::Active4d | BucketScope::Context4d => 4,
            BucketScope::Full5d => 5,
        }
    }

    fn objective_source_label(self) -> &'static str {
        match self {
            BucketScope::Core3d => "3d",
            BucketScope::Active4d | BucketScope::Context4d => "4d",
            BucketScope::Full5d => "5d",
        }
    }

    fn shrink_multiplier(self) -> f64 {
        match self {
            BucketScope::Core3d => 0.85,
            BucketScope::Active4d | BucketScope::Context4d => 1.05,
            BucketScope::Full5d => 1.35,
        }
    }
}

#[derive(Clone)]
struct V2ComponentBuild {
    base_probs: HashMap<String, f64>,
    markov_probs: HashMap<String, f64>,
    exact_motif_probs: HashMap<String, f64>,
    approx_shape_probs: HashMap<String, f64>,
    auto_cycle_probs: HashMap<String, f64>,
    state_context_probs: HashMap<String, f64>,
    markov_active: bool,
    exact_motif_active: bool,
    approx_shape_active: bool,
    auto_cycle_active: bool,
    state_context_active: bool,
    exact_strength: f64,
    motif_strength: f64,
    approx_strength: f64,
    auto_cycle_strength: f64,
    bucket: PredictionBucket,
    matched_patterns_map: HashMap<String, Vec<String>>,
    matched_experts_map: HashMap<String, Vec<String>>,
    state_signal_map: HashMap<String, Vec<String>>,
    state_summary: crate::pattern_state::SequenceStateFeatures,
}

#[derive(Clone, Copy, Default)]
struct InternalBlendWeights {
    base: f64,
    markov: f64,
    exact_motif: f64,
    approx_shape: f64,
    auto_cycle: f64,
    state_context: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum V2BlendObjective {
    Calibrated,
    Top1,
}

impl V2BlendObjective {
    fn short_label(self) -> &'static str {
        match self {
            V2BlendObjective::Calibrated => "校准优先",
            V2BlendObjective::Top1 => "Top1优先",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BucketChampionRoute {
    Blend,
    PreferMarkov,
    PreferAutoCycle,
}

impl BucketChampionRoute {
    fn short_label(self) -> &'static str {
        match self {
            BucketChampionRoute::Blend => "混合器",
            BucketChampionRoute::PreferMarkov => "Markov",
            BucketChampionRoute::PreferAutoCycle => "AutoCycle",
        }
    }

    fn source_token(self) -> &'static str {
        match self {
            BucketChampionRoute::Blend => "blend",
            BucketChampionRoute::PreferMarkov => "markov",
            BucketChampionRoute::PreferAutoCycle => "auto_cycle",
        }
    }
}

#[derive(Clone)]
struct TrainedAdaptiveModel {
    fallback_weights: InternalBlendWeights,
    global_weights: InternalBlendWeights,
    bucket_weights: HashMap<String, (InternalBlendWeights, usize)>,
    global_expert_regrets: HashMap<String, ExpertRegretEntry>,
    bucket_expert_regrets: HashMap<String, HashMap<String, ExpertRegretEntry>>,
    global_expert_duels: HashMap<String, ExpertDuelEntry>,
    bucket_expert_duels: HashMap<String, HashMap<String, ExpertDuelEntry>>,
    global_objective: V2BlendObjective,
    bucket_objectives: HashMap<String, (V2BlendObjective, usize)>,
    global_bucket_champion: BucketChampionRoute,
    bucket_champions: HashMap<String, (BucketChampionRoute, usize)>,
    bucket_champion_candidates: Vec<BucketChampionCandidateSummary>,
    champion_routing_enabled: bool,
    bucket_min_samples: usize,
    backtest_summary: PatternBacktestSummary,
}

#[derive(Clone, Copy, Default)]
struct ExpertRegretEntry {
    penalty: f64,
    sample_count: usize,
}

#[derive(Clone)]
struct ExpertDuelEntry {
    winner: String,
    loser: String,
    transfer: f64,
    sample_count: usize,
}

#[derive(Clone)]
struct BucketChampionCandidateSummary {
    bucket_key: String,
    route: BucketChampionRoute,
    sample_count: usize,
    top1_gain: f64,
    top3_gain: f64,
    true_gain: f64,
    logloss_gap: f64,
    gain_score: f64,
    qualifies: bool,
    ranking_score: f64,
    min_top1_lift: f64,
    equal_top3_lift: f64,
    qualify_margin: f64,
    positive_signal: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredPredictionRow {
    stat_key: String,
    probability: f64,
    #[serde(default)]
    best_tier_index: Option<i64>,
    #[serde(default)]
    best_tier_probability: f64,
    #[serde(default)]
    joint_probability: f64,
    #[serde(default)]
    tier_suggestions: Vec<crate::domain::types::TierSuggestion>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredPredictionContext {
    bucket: PredictionBucket,
    expert_probs: HashMap<String, HashMap<String, f64>>,
    weight_source: String,
    active_experts: Vec<String>,
    tail_signature: String,
    state_summary: Option<crate::domain::types::PatternStateSummary>,
}

fn empty_backtest_summary() -> PatternBacktestSummary {
    PatternBacktestSummary {
        sample_count: 0,
        top1_accuracy: 0.0,
        top3_coverage: 0.0,
        mean_true_prob: 0.0,
        mean_log_loss: 0.0,
        freq_top1_accuracy: 0.0,
        freq_top3_coverage: 0.0,
        freq_mean_true_prob: 0.0,
        freq_mean_log_loss: 0.0,
        random_top1_accuracy: 0.0,
        random_top3_coverage: 0.0,
        random_mean_true_prob: 0.0,
        random_mean_log_loss: 0.0,
        joint_top1_accuracy: 0.0,
        joint_top3_coverage: 0.0,
        mean_true_joint_prob: 0.0,
        mean_joint_log_loss: 0.0,
        benchmarks: Vec::new(),
    }
}

fn empty_pattern_blend_weights() -> PatternBlendWeights {
    PatternBlendWeights {
        source: "fallback".to_string(),
        sample_depth_bucket: "n/a".to_string(),
        markov_hit_bucket: "none".to_string(),
        motif_hit_bucket: "none".to_string(),
        active_stat_bucket: "n/a".to_string(),
        tier_signal_bucket: "n/a".to_string(),
        bucket_scope: "fallback".to_string(),
        bucket_sample_count: 0,
        bucket_min_samples: 0,
        bucket_trust: 0.0,
        base: 1.0,
        markov: 0.0,
        exact_motif: 0.0,
        approx_shape: 0.0,
        auto_cycle: 0.0,
        state_context: 0.0,
        online_adjusted: false,
    }
}

fn resolve_report_model_mode(mode: &str) -> String {
    match mode {
        "baseline_v1" => "baseline_v1".to_string(),
        "adaptive_v3" => "adaptive_v3".to_string(),
        "adaptive_v3_shadow" => "adaptive_v3_shadow".to_string(),
        "adaptive_v2" | "v2_shadow" | "v2_canary" => "adaptive_v2".to_string(),
        _ => "adaptive_v2".to_string(),
    }
}

fn normalize_internal_weights(weights: &mut InternalBlendWeights) {
    let total = weights.base
        + weights.markov
        + weights.exact_motif
        + weights.approx_shape
        + weights.auto_cycle
        + weights.state_context;
    if total <= 1e-9 {
        weights.base = 1.0;
        weights.markov = 0.0;
        weights.exact_motif = 0.0;
        weights.approx_shape = 0.0;
        weights.auto_cycle = 0.0;
        weights.state_context = 0.0;
        return;
    }
    weights.base /= total;
    weights.markov /= total;
    weights.exact_motif /= total;
    weights.approx_shape /= total;
    weights.auto_cycle /= total;
    weights.state_context /= total;
}

fn default_v2_weights(baseline_blend: f64) -> InternalBlendWeights {
    let mut weights = InternalBlendWeights {
        base: (1.0 - baseline_blend * 0.72).clamp(0.24, 0.42),
        markov: (baseline_blend * 0.34).clamp(0.16, 0.28),
        exact_motif: 0.18,
        approx_shape: 0.12,
        auto_cycle: 0.12,
        state_context: 0.0,
    };
    normalize_internal_weights(&mut weights);
    weights
}

fn default_v3_weights() -> InternalBlendWeights {
    let mut weights = InternalBlendWeights {
        base: 0.22,
        markov: 0.18,
        exact_motif: 0.16,
        approx_shape: 0.14,
        auto_cycle: 0.10,
        state_context: 0.20,
    };
    normalize_internal_weights(&mut weights);
    weights
}

fn bucket_fit_min_samples(bucket_min_samples: usize) -> usize {
    (bucket_min_samples / 2).max(4)
}

fn blend_internal_weights(
    base: InternalBlendWeights,
    local: InternalBlendWeights,
    local_share: f64,
) -> InternalBlendWeights {
    let local_share = local_share.clamp(0.0, 1.0);
    let base_share = 1.0 - local_share;
    let mut mixed = InternalBlendWeights {
        base: base.base * base_share + local.base * local_share,
        markov: base.markov * base_share + local.markov * local_share,
        exact_motif: base.exact_motif * base_share + local.exact_motif * local_share,
        approx_shape: base.approx_shape * base_share + local.approx_shape * local_share,
        auto_cycle: base.auto_cycle * base_share + local.auto_cycle * local_share,
        state_context: base.state_context * base_share + local.state_context * local_share,
    };
    normalize_internal_weights(&mut mixed);
    mixed
}

fn bucket_scope_trust(
    bucket_sample_count: usize,
    bucket_min_samples: usize,
    scope: BucketScope,
) -> f64 {
    let sample_count = bucket_sample_count.max(1) as f64;
    let prior = bucket_min_samples.max(1) as f64 * scope.shrink_multiplier();
    (sample_count / (sample_count + prior)).clamp(0.0, 0.92)
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
    if !components.state_context_active {
        weights.state_context = 0.0;
    }
    if components.auto_cycle_active {
        let cycle_signal = components.auto_cycle_strength.clamp(0.0, 3.5);
        let cycle_boost = (1.0 + 0.16 * cycle_signal.min(2.2)).clamp(1.0, 1.35);
        weights.auto_cycle *= cycle_boost;
        if !components.markov_active {
            weights.auto_cycle *= 1.10;
        }
        if components.bucket.markov_hit_bucket == "short" {
            weights.auto_cycle *= 1.06;
        }
        if components.motif_strength <= 1.0 {
            weights.auto_cycle *= 1.05;
        }
    }
    if components.markov_active && components.bucket.markov_hit_bucket == "short" {
        weights.markov *= 0.95;
    }
    if components.exact_motif_active && components.exact_strength < 0.85 {
        weights.exact_motif *= 0.88;
    }
    if components.approx_shape_active && components.approx_strength < 0.85 {
        weights.approx_shape *= 0.88;
    }
    let flat_like_ctx = matches!(
        components.bucket.tier_signal_bucket.as_str(),
        "flat" | "cycle_weak"
    );
    let noisy_bucket = components.bucket.active_stat_bucket == "high"
        || (components.bucket.active_stat_bucket == "mid"
            && components.bucket.tier_signal_bucket == "flat");
    if components.markov_active
        && components.bucket.markov_hit_bucket == "short"
        && noisy_bucket
    {
        weights.markov *= if flat_like_ctx { 0.76 } else { 0.84 };
    }
    if components.exact_motif_active {
        if components.bucket.motif_hit_bucket != "strong" && noisy_bucket {
            let relief = (components.exact_strength / 2.6).clamp(0.0, 0.18);
            weights.exact_motif *= (if flat_like_ctx { 0.50 } else { 0.62 } + relief)
                .clamp(0.34, 0.86);
        } else if components.bucket.active_stat_bucket == "high" && flat_like_ctx {
            weights.exact_motif *= 0.84;
        }
    }
    if components.approx_shape_active {
        if components.bucket.motif_hit_bucket != "strong" && noisy_bucket {
            let relief = (components.approx_strength / 2.8).clamp(0.0, 0.16);
            weights.approx_shape *= (if flat_like_ctx { 0.60 } else { 0.72 } + relief)
                .clamp(0.42, 0.90);
        } else if components.bucket.active_stat_bucket == "high" && flat_like_ctx {
            weights.approx_shape *= 0.88;
        }
    }
    if noisy_bucket && components.auto_cycle_active && components.auto_cycle_strength >= 1.0 {
        weights.auto_cycle *= 1.06;
    }
    normalize_internal_weights(&mut weights);
    weights
}

fn cap_base_weight(
    mut weights: InternalBlendWeights,
    components: &V2ComponentBuild,
) -> InternalBlendWeights {
    let non_base = [
        (components.markov_active, weights.markov),
        (components.exact_motif_active, weights.exact_motif),
        (components.approx_shape_active, weights.approx_shape),
        (components.auto_cycle_active, weights.auto_cycle),
        (components.state_context_active, weights.state_context),
    ]
    .into_iter()
    .filter(|(active, weight)| *active && *weight > 0.0)
    .collect::<Vec<_>>();
    let base_cap = match components.state_summary.regime_stage.as_str() {
        "new_regime" => 0.22,
        "transitioning" => 0.28,
        _ => 0.35,
    };
    if non_base.len() < 2 || weights.base <= base_cap {
        return weights;
    }

    let overflow = weights.base - base_cap;
    let total_non_base = non_base.iter().map(|(_, weight)| *weight).sum::<f64>();
    weights.base = base_cap;
    if total_non_base > 1e-9 {
        weights.markov += overflow * weights.markov / total_non_base;
        weights.exact_motif += overflow * weights.exact_motif / total_non_base;
        weights.approx_shape += overflow * weights.approx_shape / total_non_base;
        weights.auto_cycle += overflow * weights.auto_cycle / total_non_base;
        weights.state_context += overflow * weights.state_context / total_non_base;
    }
    normalize_internal_weights(&mut weights);
    weights
}

fn internal_weights_to_public(
    weights: InternalBlendWeights,
    source: &str,
    bucket: &PredictionBucket,
    bucket_scope: &str,
    bucket_sample_count: usize,
    bucket_min_samples: usize,
    bucket_trust: f64,
    online_adjusted: bool,
) -> PatternBlendWeights {
    PatternBlendWeights {
        source: source.to_string(),
        sample_depth_bucket: bucket.sample_depth_bucket.clone(),
        markov_hit_bucket: bucket.markov_hit_bucket.clone(),
        motif_hit_bucket: bucket.motif_hit_bucket.clone(),
        active_stat_bucket: bucket.active_stat_bucket.clone(),
        tier_signal_bucket: bucket.tier_signal_bucket.clone(),
        bucket_scope: bucket_scope.to_string(),
        bucket_sample_count: bucket_sample_count as i64,
        bucket_min_samples: bucket_min_samples as i64,
        bucket_trust: bucket_trust.clamp(0.0, 1.0),
        base: weights.base,
        markov: weights.markov,
        exact_motif: weights.exact_motif,
        approx_shape: weights.approx_shape,
        auto_cycle: weights.auto_cycle,
        state_context: weights.state_context,
        online_adjusted,
    }
}

fn public_state_summary(
    features: &crate::pattern_state::SequenceStateFeatures,
) -> crate::domain::types::PatternStateSummary {
    crate::domain::types::PatternStateSummary {
        active_stat_count_recent8: features.active_stat_count_recent8,
        active_stat_count_recent12: features.active_stat_count_recent12,
        active_stat_bucket: features.active_stat_bucket.clone(),
        zone_candidate: features.zone_candidate.clone(),
        zone_confidence: features.zone_confidence,
        out_of_zone_streak: features.out_of_zone_streak,
        crit_signal: features.crit_signal.clone(),
        tier_signal: features.tier_signal.clone(),
        regime_stage: features.regime_stage.clone(),
        regime_shift_score: features.regime_shift_score,
        dominant_category_recent4: features.dominant_category_recent4.clone(),
        dominant_category_recent8: features.dominant_category_recent8.clone(),
        current_category_run_len: features.current_category_run_len,
        reversion_top_stats: features.reversion_top_stats.clone(),
    }
}

fn interval_trigger_keys(events: &[crate::pattern_state::PatternEventLite], stat_keys: &[String]) -> Vec<(String, String)> {
    let mut triggers = Vec::new();
    let Some(last) = events.last() else {
        return triggers;
    };
    if matches!(last.tier_index, 1 | 8) {
        triggers.push(("last_extreme".to_string(), "极值档触发".to_string()));
    }
    if let Some(prev) = events.get(events.len().saturating_sub(2)) {
        if prev.stat_key == last.stat_key {
            let diff = (prev.tier_index - last.tier_index).abs();
            if diff == 0 {
                triggers.push(("same_stat_stop".to_string(), "同词条停档".to_string()));
            } else if diff == 1 {
                triggers.push(("same_stat_step".to_string(), "同词条连档".to_string()));
            } else {
                triggers.push(("same_stat_jump".to_string(), "同词条跳档".to_string()));
            }
        }
        let last_category = crate::pattern_state::stat_category(&last.stat_key);
        if crate::pattern_state::stat_category(&prev.stat_key) == last_category {
            triggers.push((format!("same_cat2_{last_category}"), format!("连续同类 {last_category}")));
        }
    }
    let recent8 = if events.len() > 8 { &events[events.len() - 8..] } else { events };
    let active_recent8 = recent8.iter().map(|event| event.stat_key.as_str()).collect::<HashSet<_>>().len();
    if active_recent8 <= 5 {
        triggers.push(("low_active".to_string(), "低活跃窗口".to_string()));
    }
    let features = crate::pattern_state::compute_sequence_state_features(events, stat_keys);
    if features.zone_candidate != "mixed" && features.zone_confidence >= 0.5 {
        triggers.push((format!("zone_{}", features.zone_candidate), format!("{} 候选", features.zone_candidate)));
    }
    triggers
}

fn future_has_category(
    events: &[BacktestEventLite],
    idx: usize,
    horizon: usize,
    target_category: &str,
) -> bool {
    let end = (idx + 1 + horizon).min(events.len());
    events[idx + 1..end]
        .iter()
        .any(|event| crate::pattern_state::stat_category(&event.stat_key) == target_category)
}

fn build_interval_signals(
    conn: &Connection,
    current_events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
) -> Result<Vec<PatternIntervalSignal>, String> {
    let active_triggers = interval_trigger_keys(current_events, stat_keys);
    if active_triggers.is_empty() {
        return Ok(Vec::new());
    }
    let active_trigger_map = active_triggers.into_iter().collect::<HashMap<_, _>>();
    let history = load_recent_global_backtest_events(conn, 2400)?;
    let horizon = 5usize;
    if history.len() <= horizon + 32 {
        return Ok(Vec::new());
    }

    let targets = [
        ("crit", "未来5手暴区"),
        ("dmg_bonus", "未来5手伤害加成"),
    ];
    let mut baseline = HashMap::<&str, (usize, usize)>::new();
    let mut trigger_counts = HashMap::<(String, &str), (usize, usize)>::new();

    for idx in 8..history.len().saturating_sub(horizon) {
        let prefix_events = history[..=idx]
            .iter()
            .map(|event| crate::pattern_state::PatternEventLite {
                stat_key: event.stat_key.clone(),
                tier_index: event.tier_index,
                analysis_seq: event.analysis_seq,
            })
            .collect::<Vec<_>>();
        let trigger_keys = interval_trigger_keys(&prefix_events, stat_keys)
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        for (target, _) in targets {
            let hit = future_has_category(&history, idx, horizon, target);
            let base_entry = baseline.entry(target).or_insert((0, 0));
            base_entry.1 += 1;
            if hit {
                base_entry.0 += 1;
            }
            for trigger_key in &trigger_keys {
                if !active_trigger_map.contains_key(trigger_key) {
                    continue;
                }
                let entry = trigger_counts.entry((trigger_key.clone(), target)).or_insert((0, 0));
                entry.1 += 1;
                if hit {
                    entry.0 += 1;
                }
            }
        }
    }

    let mut signals = Vec::new();
    for ((trigger_key, target), (hits, samples)) in trigger_counts {
        if samples < 20 {
            continue;
        }
        let Some((baseline_hits, baseline_samples)) = baseline.get(target).copied() else {
            continue;
        };
        if baseline_samples == 0 {
            continue;
        }
        let baseline_rate = baseline_hits as f64 / baseline_samples as f64;
        let observed_rate = hits as f64 / samples as f64;
        let lift = observed_rate - baseline_rate;
        if lift.abs() < 0.04 {
            continue;
        }
        let confidence = ((samples as f64 / 120.0).sqrt().min(1.0) * (lift.abs() / 0.18).min(1.0)).clamp(0.0, 1.0);
        let trigger_label = active_trigger_map
            .get(&trigger_key)
            .cloned()
            .unwrap_or_else(|| trigger_key.clone());
        let target_label = targets
            .iter()
            .find(|(key, _)| *key == target)
            .map(|(_, label)| *label)
            .unwrap_or(target);
        let direction = if lift > 0.0 { "opportunity" } else { "risk" }.to_string();
        let note = if lift > 0.0 {
            format!("历史同触发后 {target_label} 高于基线 {:.1} 个百分点", lift * 100.0)
        } else {
            format!("历史同触发后 {target_label} 低于基线 {:.1} 个百分点", lift.abs() * 100.0)
        };
        signals.push(PatternIntervalSignal {
            key: format!("{trigger_key}:{target}"),
            label: trigger_label,
            horizon: horizon as i64,
            target: target_label.to_string(),
            sample_count: samples as i64,
            baseline_rate,
            observed_rate,
            lift,
            confidence,
            direction,
            note,
        });
    }
    signals.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.lift.abs().partial_cmp(&a.lift.abs()).unwrap_or(Ordering::Equal))
    });
    signals.truncate(4);
    Ok(signals)
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
    let active_stat_count_recent8 = if n > 0 {
        let recent = if n > 8 { &seq[n - 8..] } else { seq };
        recent
            .iter()
            .map(|stat_key| stat_key.as_str())
            .collect::<HashSet<_>>()
            .len()
    } else {
        0
    };
    let active_stat_bucket = if active_stat_count_recent8 <= 3 {
        "low".to_string()
    } else if active_stat_count_recent8 <= 5 {
        "mid".to_string()
    } else {
        "high".to_string()
    };
    let category_run_bucket = if let Some(last_stat) = seq.last() {
        let last_category = crate::pattern_state::stat_category(last_stat);
        let run_len = seq
            .iter()
            .rev()
            .take_while(|stat_key| crate::pattern_state::stat_category(stat_key) == last_category)
            .count();
        if run_len >= 3 { "cat_run" } else { "flat" }
    } else {
        "flat"
    };
    let tier_signal_bucket = if auto_cycle_strength >= 2.4 {
        "cycle_strong".to_string()
    } else if auto_cycle_strength >= 1.0 {
        "cycle_weak".to_string()
    } else {
        category_run_bucket.to_string()
    };

    let state_context_probs = base_probs.clone();

    V2ComponentBuild {
        base_probs,
        markov_probs,
        exact_motif_probs,
        approx_shape_probs,
        auto_cycle_probs,
        state_context_probs,
        markov_active: markov_max_order_hit > 0,
        exact_motif_active: exact_strength > 1e-9,
        approx_shape_active: approx_strength > 1e-9,
        auto_cycle_active: auto_cycle_strength > 1e-9,
        state_context_active: false,
        exact_strength,
        motif_strength,
        approx_strength,
        auto_cycle_strength,
        bucket: PredictionBucket {
            sample_depth_bucket,
            markov_hit_bucket,
            motif_hit_bucket,
            active_stat_bucket,
            tier_signal_bucket,
        },
        matched_patterns_map,
        matched_experts_map,
        state_signal_map: HashMap::new(),
        state_summary: crate::pattern_state::SequenceStateFeatures {
            active_stat_count_recent8: 0,
            active_stat_count_recent12: 0,
            active_stat_bucket: "n/a".to_string(),
            zone_candidate: "mixed".to_string(),
            zone_confidence: 0.0,
            out_of_zone_streak: 0,
            crit_signal: "none".to_string(),
            tier_signal: "n/a".to_string(),
            regime_stage: "stable".to_string(),
            regime_shift_score: 0.0,
            dominant_category_recent4: "mixed".to_string(),
            dominant_category_recent8: "mixed".to_string(),
            current_category_run_len: 0,
            reversion_top_stats: Vec::new(),
            reversion_score_by_stat: HashMap::new(),
        },
    }
}

fn fit_v2_weights_for_samples(
    samples: &[BacktestSample],
    baseline_blend: f64,
    objective: V2BlendObjective,
) -> InternalBlendWeights {
    let prior = default_v2_weights(baseline_blend);
    let experts = ["base", "markov", "exact_motif", "approx_shape", "auto_cycle"];
    let priors = [
        prior.base,
        prior.markov,
        prior.exact_motif,
        prior.approx_shape,
        prior.auto_cycle,
    ];
    let mut log_scores = [0.0; 5];
    let mut active_counts = [0.0; 5];
    let mut sample_count = 0.0;
    for sample in samples {
        let expert_maps = [
            &sample.components.base_probs,
            &sample.components.markov_probs,
            &sample.components.exact_motif_probs,
            &sample.components.approx_shape_probs,
            &sample.components.auto_cycle_probs,
        ];
        let expert_active = [
            true,
            sample.components.markov_active,
            sample.components.exact_motif_active,
            sample.components.approx_shape_active,
            sample.components.auto_cycle_active,
        ];
        for (idx, expert_map) in expert_maps.iter().enumerate() {
            let p = masked_probability_for_stat(
                expert_map,
                &sample.actual_stat_key,
                &sample.blocked_stats,
            );
            log_scores[idx] += p.ln();
            if expert_active[idx] {
                active_counts[idx] += 1.0;
            }
        }
        sample_count += 1.0;
    }
    if sample_count <= 1e-9 {
        return prior;
    }

    let freq_baseline = evaluate_frequency_baseline(samples);
    let random_baseline = evaluate_random_baseline(samples);
    let expert_metrics = [
        evaluate_probability_map_benchmark(samples, "base", "Base", |sample| {
            sample.components.base_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "markov", "Markov", |sample| {
            sample.components.markov_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "exact_motif", "Exact", |sample| {
            sample.components.exact_motif_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "approx_shape", "Approx", |sample| {
            sample.components.approx_shape_probs.clone()
        }),
        evaluate_probability_map_benchmark(samples, "auto_cycle", "AutoCycle", |sample| {
            sample.components.auto_cycle_probs.clone()
        }),
    ];

    let mut mean_scores = [0.0; 5];
    for idx in 0..experts.len() {
        mean_scores[idx] = log_scores[idx] / sample_count;
    }
    let best_score = mean_scores
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let temperature = match objective {
        V2BlendObjective::Calibrated => 0.40,
        V2BlendObjective::Top1 => 0.52,
    };
    let mut raw = [0.0; 5];
    for idx in 0..experts.len() {
        let metrics = &expert_metrics[idx];
        let activity_share = (active_counts[idx] / sample_count).clamp(0.0, 1.0);
        let logloss_gain_vs_freq = freq_baseline.mean_log_loss - metrics.mean_log_loss;
        let top3_gain_vs_freq = metrics.top3_coverage - freq_baseline.top3_coverage;
        let top1_gain_vs_freq = metrics.top1_accuracy - freq_baseline.top1_accuracy;
        let true_prob_gain_vs_freq = metrics.mean_true_prob - freq_baseline.mean_true_prob;
        let reliability = match objective {
            V2BlendObjective::Calibrated => {
                let logloss_gain_vs_random =
                    random_baseline.mean_log_loss - metrics.mean_log_loss;
                let mut value = (1.0
                    + 5.4 * logloss_gain_vs_freq
                    + 2.0 * logloss_gain_vs_random
                    + 1.6 * true_prob_gain_vs_freq
                    + 0.6 * top3_gain_vs_freq
                    + 0.3 * top1_gain_vs_freq)
                    .clamp(0.05, 3.0);
                value *= (0.46 + 0.54 * activity_share).clamp(0.28, 1.0);
                if metrics.mean_log_loss >= freq_baseline.mean_log_loss - 1e-9
                    && metrics.mean_true_prob <= freq_baseline.mean_true_prob + 0.0005
                {
                    value *= 0.30;
                }
                if experts[idx] == "auto_cycle"
                    && metrics.mean_log_loss <= random_baseline.mean_log_loss + 0.004
                {
                    value *= 1.18;
                }
                if experts[idx] == "base"
                    && metrics.mean_log_loss <= freq_baseline.mean_log_loss + 0.002
                {
                    value *= 1.05;
                }
                value
            }
            V2BlendObjective::Top1 => {
                let mut value = (1.0
                    + 4.0 * logloss_gain_vs_freq
                    + 1.8 * top3_gain_vs_freq
                    + 1.6 * top1_gain_vs_freq
                    + 1.0 * true_prob_gain_vs_freq)
                    .clamp(0.05, 3.2);
                value *= (0.40 + 0.60 * activity_share).clamp(0.25, 1.0);

                if metrics.mean_log_loss > random_baseline.mean_log_loss + 0.04
                    && metrics.top3_coverage + 1e-9 < freq_baseline.top3_coverage
                {
                    value *= 0.45;
                }
                if experts[idx] != "base"
                    && metrics.mean_log_loss >= freq_baseline.mean_log_loss - 1e-9
                    && metrics.top3_coverage <= freq_baseline.top3_coverage + 0.015
                    && metrics.top1_accuracy <= freq_baseline.top1_accuracy + 0.002
                {
                    value *= 0.18;
                }
                if experts[idx] == "auto_cycle"
                    && metrics.mean_log_loss <= freq_baseline.mean_log_loss + 0.005
                    && metrics.top3_coverage >= freq_baseline.top3_coverage + 0.08
                {
                    value *= 1.45;
                }
                if experts[idx] == "markov"
                    && metrics.top1_accuracy >= freq_baseline.top1_accuracy + 0.005
                {
                    value *= 1.18;
                }
                if experts[idx] == "base" {
                    value *= 0.92;
                }
                value
            }
        };
        let low_signal_penalty = expert_low_signal_penalty(samples, experts[idx], objective);

        raw[idx] = priors[idx]
            * ((mean_scores[idx] - best_score) / temperature).exp()
            * (reliability * low_signal_penalty).clamp(0.05, 3.2);
    }
    let total = raw.iter().sum::<f64>().max(1e-9);
    let mut weights = InternalBlendWeights {
        base: raw[0] / total,
        markov: raw[1] / total,
        exact_motif: raw[2] / total,
        approx_shape: raw[3] / total,
        auto_cycle: raw[4] / total,
        state_context: 0.0,
    };
    let auto_cycle_metrics = &expert_metrics[4];
    let cycle_floor = match objective {
        V2BlendObjective::Calibrated => {
            if auto_cycle_metrics.mean_log_loss <= random_baseline.mean_log_loss + 0.004 {
                0.18
            } else {
                0.0
            }
        }
        V2BlendObjective::Top1 => {
            if auto_cycle_metrics.mean_log_loss <= random_baseline.mean_log_loss + 0.004
                && auto_cycle_metrics.top3_coverage >= random_baseline.top3_coverage - 1e-9
            {
                0.24
            } else {
                0.0
            }
        }
    };
    if cycle_floor > 0.0 && weights.auto_cycle < cycle_floor {
        let lift = cycle_floor - weights.auto_cycle;
        let donor_total =
            (weights.base + weights.markov + weights.exact_motif + weights.approx_shape).max(1e-9);
        weights.base = (weights.base - lift * weights.base / donor_total).max(0.10);
        weights.markov = (weights.markov - lift * weights.markov / donor_total).max(0.0);
        weights.exact_motif =
            (weights.exact_motif - lift * weights.exact_motif / donor_total).max(0.0);
        weights.approx_shape =
            (weights.approx_shape - lift * weights.approx_shape / donor_total).max(0.0);
        weights.auto_cycle = cycle_floor;
    }
    normalize_internal_weights(&mut weights);
    weights
}

fn blend_v2_probs(
    stat_keys: &[String],
    components: &V2ComponentBuild,
    weights: InternalBlendWeights,
) -> HashMap<String, f64> {
    let resolved = cap_base_weight(resolve_active_v2_weights(weights, components), components);
    let mut mixed = HashMap::new();
    let mut expert_focus = HashMap::new();
    for stat_key in stat_keys {
        let base = *components.base_probs.get(stat_key).unwrap_or(&0.0);
        let markov = *components.markov_probs.get(stat_key).unwrap_or(&base);
        let exact = *components.exact_motif_probs.get(stat_key).unwrap_or(&base);
        let approx = *components.approx_shape_probs.get(stat_key).unwrap_or(&base);
        let cycle = *components.auto_cycle_probs.get(stat_key).unwrap_or(&base);
        let state_context = *components.state_context_probs.get(stat_key).unwrap_or(&base);
        let consensus = if components.markov_active && components.auto_cycle_active {
            (markov * cycle).sqrt()
        } else if components.auto_cycle_active {
            cycle
        } else if components.markov_active {
            markov
        } else {
            0.0
        };
        expert_focus.insert(
            stat_key.clone(),
            (resolved.markov * markov
                + resolved.auto_cycle * cycle
                + resolved.state_context * state_context
                + 0.25 * consensus)
                .max(0.0),
        );
        mixed.insert(
            stat_key.clone(),
            resolved.base * base
                + resolved.markov * markov
                + resolved.exact_motif * exact
                + resolved.approx_shape * approx
                + resolved.auto_cycle * cycle
                + resolved.state_context * state_context,
        );
    }
    if components.markov_active || components.auto_cycle_active || components.state_context_active {
        normalize_probability_map(&mut expert_focus);
        let mut focus_gain = (0.03
            + 0.10 * resolved.auto_cycle
            + 0.08 * resolved.markov
            + 0.05 * resolved.state_context)
            .clamp(0.0, 0.15);
        if components.markov_active && components.auto_cycle_active && components.auto_cycle_strength >= 1.0 {
            focus_gain = (focus_gain + 0.03).clamp(0.0, 0.18);
        }
        if focus_gain > 1e-9 {
            for stat_key in stat_keys {
                let base_mix = mixed.get(stat_key).copied().unwrap_or(0.0);
                let focus_mix = expert_focus.get(stat_key).copied().unwrap_or(0.0);
                mixed.insert(
                    stat_key.clone(),
                    ((1.0 - focus_gain) * base_mix + focus_gain * focus_mix).max(0.0),
                );
            }
        }
    }
    normalize_probability_map(&mut mixed);
    mixed
}

fn apply_bucket_champion_route_probs(
    components: &V2ComponentBuild,
    blend_probs: HashMap<String, f64>,
    blocked_stats: &HashSet<String>,
    route: BucketChampionRoute,
) -> (HashMap<String, f64>, BucketChampionRoute) {
    let mut applied_route = route;
    let mut routed = match route {
        BucketChampionRoute::Blend => blend_probs,
        BucketChampionRoute::PreferMarkov if components.markov_active => {
            components.markov_probs.clone()
        }
        BucketChampionRoute::PreferAutoCycle if components.auto_cycle_active => {
            components.auto_cycle_probs.clone()
        }
        _ => {
            applied_route = BucketChampionRoute::Blend;
            blend_probs
        }
    };
    apply_blocked_stat_mask(&mut routed, blocked_stats);
    (routed, applied_route)
}

fn select_v2_weights_for_bucket_raw(
    fallback_weights: InternalBlendWeights,
    global_weights: InternalBlendWeights,
    bucket_weights: &HashMap<String, (InternalBlendWeights, usize)>,
    bucket_min_samples: usize,
    sample_count: usize,
    bucket: &PredictionBucket,
) -> (InternalBlendWeights, &'static str) {
    let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
    let global_ready = sample_count >= fit_min_samples;
    let mut selected = if global_ready {
        global_weights
    } else {
        fallback_weights
    };
    let mut most_specific_scope = None;

    for scope in BucketScope::blend_scopes() {
        let scoped_key = bucket.scoped_key(scope);
        let Some((local_weights, local_sample_count)) = bucket_weights.get(&scoped_key) else {
            continue;
        };
        if *local_sample_count < fit_min_samples {
            continue;
        }
        let trust = bucket_scope_trust(*local_sample_count, bucket_min_samples, scope);
        if trust <= 1e-9 {
            continue;
        }
        selected = blend_internal_weights(selected, *local_weights, trust);
        most_specific_scope = Some(scope);
    }

    if let Some(scope) = most_specific_scope {
        (selected, scope.source_label())
    } else if global_ready {
        (global_weights, "global")
    } else {
        (fallback_weights, "fallback")
    }
}

fn select_v2_weights_for_bucket(
    model: &TrainedAdaptiveModel,
    bucket: &PredictionBucket,
) -> (InternalBlendWeights, &'static str) {
    select_v2_weights_for_bucket_raw(
        model.fallback_weights,
        model.global_weights,
        &model.bucket_weights,
        model.bucket_min_samples,
        model.backtest_summary.sample_count as usize,
        bucket,
    )
}

fn resolve_bucket_reliability(
    model: &TrainedAdaptiveModel,
    bucket: &PredictionBucket,
) -> (&'static str, usize, usize, f64) {
    let fit_min_samples = bucket_fit_min_samples(model.bucket_min_samples);
    let mut selected = None::<(BucketScope, usize)>;

    for scope in BucketScope::blend_scopes() {
        let scoped_key = bucket.scoped_key(scope);
        let Some((_, local_sample_count)) = model.bucket_weights.get(&scoped_key) else {
            continue;
        };
        if *local_sample_count < fit_min_samples {
            continue;
        }
        selected = Some((scope, *local_sample_count));
    }

    if let Some((scope, sample_count)) = selected {
        let trust = bucket_scope_trust(sample_count, model.bucket_min_samples, scope);
        return (scope.source_label(), sample_count, fit_min_samples, trust);
    }

    let global_sample_count = model.backtest_summary.sample_count.max(0) as usize;
    if global_sample_count >= fit_min_samples {
        let trust = (global_sample_count as f64
            / (global_sample_count as f64 + model.bucket_min_samples.max(1) as f64))
            .clamp(0.0, 0.92);
        ("global", global_sample_count, fit_min_samples, trust)
    } else {
        ("fallback", global_sample_count, fit_min_samples, 0.0)
    }
}

fn select_bucket_champion_route_raw(
    global_route: BucketChampionRoute,
    bucket_champions: &HashMap<String, (BucketChampionRoute, usize)>,
    bucket_min_samples: usize,
    sample_count: usize,
    bucket: &PredictionBucket,
) -> (BucketChampionRoute, &'static str) {
    let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
    let mut best_match = None::<(BucketScope, BucketChampionRoute, usize)>;
    for scope in BucketScope::objective_scopes() {
        let scoped_key = bucket.scoped_key(scope);
        let Some((route, local_sample_count)) = bucket_champions.get(&scoped_key) else {
            continue;
        };
        if *local_sample_count < fit_min_samples {
            continue;
        }
        let replace = best_match
            .map(|(best_scope, _, best_count)| {
                scope.specificity_rank() > best_scope.specificity_rank()
                    || (scope.specificity_rank() == best_scope.specificity_rank()
                        && *local_sample_count > best_count)
            })
            .unwrap_or(true);
        if replace {
            best_match = Some((scope, *route, *local_sample_count));
        }
    }

    if let Some((scope, route, _)) = best_match {
        (route, scope.objective_source_label())
    } else if sample_count >= fit_min_samples {
        (global_route, "global")
    } else {
        (BucketChampionRoute::Blend, "fallback")
    }
}

fn select_bucket_champion_route(
    model: &TrainedAdaptiveModel,
    bucket: &PredictionBucket,
) -> (BucketChampionRoute, &'static str) {
    select_bucket_champion_route_raw(
        model.global_bucket_champion,
        &model.bucket_champions,
        model.bucket_min_samples,
        model.backtest_summary.sample_count as usize,
        bucket,
    )
}

fn format_scoped_bucket_key(bucket_key: &str) -> String {
    if let Some(rest) = bucket_key.strip_prefix("4a:") {
        let parts = rest.split('|').collect::<Vec<_>>();
        if parts.len() == 4 {
            return format!(
                "4d(active) depth={} / markov={} / motif={} / active={}",
                parts[0], parts[1], parts[2], parts[3]
            );
        }
    } else if let Some(rest) = bucket_key.strip_prefix("4c:") {
        let parts = rest.split('|').collect::<Vec<_>>();
        if parts.len() == 4 {
            return format!(
                "4d(ctx) depth={} / markov={} / motif={} / ctx={}",
                parts[0], parts[1], parts[2], parts[3]
            );
        }
    } else if let Some(rest) = bucket_key.strip_prefix("3:") {
        let parts = rest.split('|').collect::<Vec<_>>();
        if parts.len() == 3 {
            return format!(
                "3d depth={} / markov={} / motif={}",
                parts[0], parts[1], parts[2]
            );
        }
    } else {
        let parts = bucket_key.split('|').collect::<Vec<_>>();
        if parts.len() == 5 {
            return format!(
                "5d depth={} / markov={} / motif={} / active={} / ctx={}",
                parts[0], parts[1], parts[2], parts[3], parts[4]
            );
        }
    }
    bucket_key.to_string()
}

fn format_bucket_champion_candidate(candidate: &BucketChampionCandidateSummary) -> String {
    let status = if candidate.qualifies {
        "已达接管阈值".to_string()
    } else if candidate.top1_gain > 1e-9 {
        format!(
            "距接管还差 {:.2}pp Top1",
            ((candidate.min_top1_lift - candidate.top1_gain).max(0.0)) * 100.0
        )
    } else if candidate.top3_gain > 1e-9 && candidate.logloss_gap <= 0.015 {
        format!(
            "距平替接管还差 {:.2}pp Top3",
            ((candidate.equal_top3_lift - candidate.top3_gain).max(0.0)) * 100.0
        )
    } else if candidate.logloss_gap < -1e-9 {
        format!("LogLoss 更低 {:.3}，但 Top1 还没起量", -candidate.logloss_gap)
    } else {
        "与混合器几乎重合".to_string()
    };
    format!(
        "{} -> {} (n={}) · ΔTop1 {:+.2}pp · ΔTop3 {:+.2}pp · ΔP {:+.2}pp · ΔLL {:+.3} · score {:+.3} · {}",
        format_scoped_bucket_key(&candidate.bucket_key),
        candidate.route.short_label(),
        candidate.sample_count,
        candidate.top1_gain * 100.0,
        candidate.top3_gain * 100.0,
        candidate.true_gain * 100.0,
        candidate.logloss_gap,
        candidate.gain_score,
        status
    )
}

fn bucket_champion_candidate_has_signal(candidate: &BucketChampionCandidateSummary) -> bool {
    candidate.positive_signal
}

fn compare_bucket_champion_candidates(
    a: &BucketChampionCandidateSummary,
    b: &BucketChampionCandidateSummary,
) -> Ordering {
    b.qualifies
        .cmp(&a.qualifies)
        .then_with(|| {
            bucket_champion_candidate_has_signal(b)
                .cmp(&bucket_champion_candidate_has_signal(a))
        })
        .then_with(|| {
            b.qualify_margin
                .partial_cmp(&a.qualify_margin)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            b.ranking_score
                .partial_cmp(&a.ranking_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| b.sample_count.cmp(&a.sample_count))
}

fn select_v2_objective_for_bucket_raw(
    global_objective: V2BlendObjective,
    bucket_objectives: &HashMap<String, (V2BlendObjective, usize)>,
    bucket_min_samples: usize,
    sample_count: usize,
    bucket: &PredictionBucket,
) -> (V2BlendObjective, &'static str) {
    let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
    let mut best_match = None::<(BucketScope, V2BlendObjective, usize)>;
    for scope in BucketScope::objective_scopes() {
        let scoped_key = bucket.scoped_key(scope);
        let Some((objective, local_sample_count)) = bucket_objectives.get(&scoped_key) else {
            continue;
        };
        if *local_sample_count < fit_min_samples {
            continue;
        }
        let replace = best_match
            .map(|(best_scope, _, best_count)| {
                scope.specificity_rank() > best_scope.specificity_rank()
                    || (scope.specificity_rank() == best_scope.specificity_rank()
                        && *local_sample_count > best_count)
            })
            .unwrap_or(true);
        if replace {
            best_match = Some((scope, *objective, *local_sample_count));
        }
    }

    if let Some((scope, objective, _)) = best_match {
        (objective, scope.objective_source_label())
    } else if sample_count >= fit_min_samples {
        (global_objective, "global")
    } else {
        (global_objective, "fallback")
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
    if resolved.state_context > 0.0 {
        experts.push("state_context".to_string());
    }
    experts
}

fn evaluate_v2_model_samples(
    samples: &[BacktestSample],
    stat_keys: &[String],
    fallback_weights: InternalBlendWeights,
    global_weights: InternalBlendWeights,
    bucket_weights: &HashMap<String, (InternalBlendWeights, usize)>,
    global_expert_regrets: &HashMap<String, ExpertRegretEntry>,
    bucket_expert_regrets: &HashMap<String, HashMap<String, ExpertRegretEntry>>,
    global_expert_duels: &HashMap<String, ExpertDuelEntry>,
    bucket_expert_duels: &HashMap<String, HashMap<String, ExpertDuelEntry>>,
    bucket_min_samples: usize,
) -> FlatBacktestMetrics {
    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;

    for sample in samples {
        let (weights, _) = select_v2_weights_for_bucket_raw(
            fallback_weights,
            global_weights,
            bucket_weights,
            bucket_min_samples,
            samples.len(),
            &sample.components.bucket,
        );
        let (regret_adjusted, _) = apply_regret_table_to_weights_raw(
            weights,
            global_expert_regrets,
            bucket_expert_regrets,
            bucket_min_samples,
            &sample.components,
        );
        let (duel_adjusted, _) = apply_duel_table_to_weights_raw(
            regret_adjusted,
            global_expert_duels,
            bucket_expert_duels,
            bucket_min_samples,
            &sample.components,
        );
        let mut mixed = blend_v2_probs(stat_keys, &sample.components, duel_adjusted);
        apply_blocked_stat_mask(&mut mixed, &sample.blocked_stats);
        let actual_prob = mixed
            .get(&sample.actual_stat_key)
            .copied()
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();
        if top_prediction_key(&mixed) == Some(sample.actual_stat_key.as_str()) {
            top1_hits += 1.0;
        }
        let ranked = sorted_prediction_ranking(&mixed);
        if ranked
            .iter()
            .take(3)
            .any(|(stat_key, _)| stat_key.as_str() == sample.actual_stat_key.as_str())
        {
            top3_hits += 1.0;
        }
    }

    finalize_flat_metrics(
        samples.len() as f64,
        top1_hits,
        top3_hits,
        true_prob_sum,
        log_loss_sum,
    )
}

fn evaluate_v2_constant_champion_route_samples(
    samples: &[BacktestSample],
    stat_keys: &[String],
    fallback_weights: InternalBlendWeights,
    global_weights: InternalBlendWeights,
    bucket_weights: &HashMap<String, (InternalBlendWeights, usize)>,
    global_expert_regrets: &HashMap<String, ExpertRegretEntry>,
    bucket_expert_regrets: &HashMap<String, HashMap<String, ExpertRegretEntry>>,
    global_expert_duels: &HashMap<String, ExpertDuelEntry>,
    bucket_expert_duels: &HashMap<String, HashMap<String, ExpertDuelEntry>>,
    bucket_min_samples: usize,
    route: BucketChampionRoute,
) -> FlatBacktestMetrics {
    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;

    for sample in samples {
        let (weights, _) = select_v2_weights_for_bucket_raw(
            fallback_weights,
            global_weights,
            bucket_weights,
            bucket_min_samples,
            samples.len(),
            &sample.components.bucket,
        );
        let (regret_adjusted, _) = apply_regret_table_to_weights_raw(
            weights,
            global_expert_regrets,
            bucket_expert_regrets,
            bucket_min_samples,
            &sample.components,
        );
        let (duel_adjusted, _) = apply_duel_table_to_weights_raw(
            regret_adjusted,
            global_expert_duels,
            bucket_expert_duels,
            bucket_min_samples,
            &sample.components,
        );
        let mixed = blend_v2_probs(stat_keys, &sample.components, duel_adjusted);
        let (routed, _) = apply_bucket_champion_route_probs(
            &sample.components,
            mixed,
            &sample.blocked_stats,
            route,
        );
        let actual_prob = routed
            .get(&sample.actual_stat_key)
            .copied()
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();
        if top_prediction_key(&routed) == Some(sample.actual_stat_key.as_str()) {
            top1_hits += 1.0;
        }
        let ranked = sorted_prediction_ranking(&routed);
        if ranked
            .iter()
            .take(3)
            .any(|(stat_key, _)| stat_key.as_str() == sample.actual_stat_key.as_str())
        {
            top3_hits += 1.0;
        }
    }

    finalize_flat_metrics(
        samples.len() as f64,
        top1_hits,
        top3_hits,
        true_prob_sum,
        log_loss_sum,
    )
}

fn evaluate_v2_bucket_champion_model_samples(
    samples: &[BacktestSample],
    stat_keys: &[String],
    fallback_weights: InternalBlendWeights,
    global_weights: InternalBlendWeights,
    bucket_weights: &HashMap<String, (InternalBlendWeights, usize)>,
    global_expert_regrets: &HashMap<String, ExpertRegretEntry>,
    bucket_expert_regrets: &HashMap<String, HashMap<String, ExpertRegretEntry>>,
    global_expert_duels: &HashMap<String, ExpertDuelEntry>,
    bucket_expert_duels: &HashMap<String, HashMap<String, ExpertDuelEntry>>,
    global_route: BucketChampionRoute,
    bucket_champions: &HashMap<String, (BucketChampionRoute, usize)>,
    bucket_min_samples: usize,
) -> FlatBacktestMetrics {
    let mut top1_hits = 0.0;
    let mut top3_hits = 0.0;
    let mut true_prob_sum = 0.0;
    let mut log_loss_sum = 0.0;

    for sample in samples {
        let (weights, _) = select_v2_weights_for_bucket_raw(
            fallback_weights,
            global_weights,
            bucket_weights,
            bucket_min_samples,
            samples.len(),
            &sample.components.bucket,
        );
        let (regret_adjusted, _) = apply_regret_table_to_weights_raw(
            weights,
            global_expert_regrets,
            bucket_expert_regrets,
            bucket_min_samples,
            &sample.components,
        );
        let (duel_adjusted, _) = apply_duel_table_to_weights_raw(
            regret_adjusted,
            global_expert_duels,
            bucket_expert_duels,
            bucket_min_samples,
            &sample.components,
        );
        let mixed = blend_v2_probs(stat_keys, &sample.components, duel_adjusted);
        let (route, _) = select_bucket_champion_route_raw(
            global_route,
            bucket_champions,
            bucket_min_samples,
            samples.len(),
            &sample.components.bucket,
        );
        let (routed, _) = apply_bucket_champion_route_probs(
            &sample.components,
            mixed,
            &sample.blocked_stats,
            route,
        );
        let actual_prob = routed
            .get(&sample.actual_stat_key)
            .copied()
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        true_prob_sum += actual_prob;
        log_loss_sum += -actual_prob.ln();
        if top_prediction_key(&routed) == Some(sample.actual_stat_key.as_str()) {
            top1_hits += 1.0;
        }
        let ranked = sorted_prediction_ranking(&routed);
        if ranked
            .iter()
            .take(3)
            .any(|(stat_key, _)| stat_key.as_str() == sample.actual_stat_key.as_str())
        {
            top3_hits += 1.0;
        }
    }

    finalize_flat_metrics(
        samples.len() as f64,
        top1_hits,
        top3_hits,
        true_prob_sum,
        log_loss_sum,
    )
}

fn expert_active_for_sample(components: &V2ComponentBuild, expert: &str) -> bool {
    match expert {
        "base" => true,
        "markov" => components.markov_active,
        "exact_motif" => components.exact_motif_active,
        "approx_shape" => components.approx_shape_active,
        "auto_cycle" => components.auto_cycle_active,
        "state_context" => components.state_context_active,
        _ => false,
    }
}

fn scale_expert_weight(weights: &mut InternalBlendWeights, expert: &str, factor: f64) {
    let factor = factor.clamp(0.0, 1.25);
    match expert {
        "base" => weights.base *= factor,
        "markov" => weights.markov *= factor,
        "exact_motif" => weights.exact_motif *= factor,
        "approx_shape" => weights.approx_shape *= factor,
        "auto_cycle" => weights.auto_cycle *= factor,
        "state_context" => weights.state_context *= factor,
        _ => {}
    }
}

fn expert_low_signal_bucket(components: &V2ComponentBuild, expert: &str) -> bool {
    match expert {
        "markov" => {
            components.bucket.markov_hit_bucket != "long"
                && (components.bucket.active_stat_bucket != "low"
                    || matches!(
                        components.bucket.tier_signal_bucket.as_str(),
                        "flat" | "cycle_weak"
                    ))
        }
        "exact_motif" => {
            components.bucket.motif_hit_bucket != "strong"
                || (components.bucket.active_stat_bucket == "high"
                    && matches!(
                        components.bucket.tier_signal_bucket.as_str(),
                        "flat" | "cycle_weak"
                    ))
        }
        "approx_shape" => {
            components.bucket.motif_hit_bucket == "none"
                || (components.bucket.motif_hit_bucket == "weak"
                    && components.bucket.active_stat_bucket != "low")
                || (components.bucket.active_stat_bucket == "high"
                    && components.bucket.tier_signal_bucket == "flat")
        }
        _ => false,
    }
}

fn evaluate_expert_subset_metrics(
    samples: &[BacktestSample],
    expert: &str,
) -> PatternBenchmarkRow {
    match expert {
        "base" => evaluate_probability_map_benchmark(samples, "base", "Base", |sample| {
            sample.components.base_probs.clone()
        }),
        "markov" => evaluate_probability_map_benchmark(samples, "markov", "Markov", |sample| {
            sample.components.markov_probs.clone()
        }),
        "exact_motif" => {
            evaluate_probability_map_benchmark(samples, "exact_motif", "Exact", |sample| {
                sample.components.exact_motif_probs.clone()
            })
        }
        "approx_shape" => {
            evaluate_probability_map_benchmark(samples, "approx_shape", "Approx", |sample| {
                sample.components.approx_shape_probs.clone()
            })
        }
        "auto_cycle" => {
            evaluate_probability_map_benchmark(samples, "auto_cycle", "AutoCycle", |sample| {
                sample.components.auto_cycle_probs.clone()
            })
        }
        "state_context" => {
            evaluate_probability_map_benchmark(samples, "state_context", "State", |sample| {
                sample.components.state_context_probs.clone()
            })
        }
        _ => flat_metrics_to_benchmark_row(
            "invalid",
            "Invalid",
            finalize_flat_metrics(samples.len() as f64, 0.0, 0.0, 0.0, 0.0),
        ),
    }
}

fn expert_low_signal_penalty(
    samples: &[BacktestSample],
    expert: &str,
    objective: V2BlendObjective,
) -> f64 {
    if !matches!(expert, "markov" | "exact_motif" | "approx_shape") {
        return 1.0;
    }
    let subset = samples
        .iter()
        .filter(|sample| {
            expert_active_for_sample(&sample.components, expert)
                && expert_low_signal_bucket(&sample.components, expert)
        })
        .cloned()
        .collect::<Vec<_>>();
    if subset.len() < 5 {
        return 1.0;
    }

    let metrics = evaluate_expert_subset_metrics(&subset, expert);
    let freq_baseline = evaluate_frequency_baseline(&subset);
    let random_baseline = evaluate_random_baseline(&subset);
    let subset_evidence = (subset.len() as f64 / (subset.len() as f64 + 8.0)).clamp(0.0, 1.0);
    let logloss_gap_vs_freq = (metrics.mean_log_loss - freq_baseline.mean_log_loss).max(0.0);
    let logloss_gap_vs_random =
        (metrics.mean_log_loss - random_baseline.mean_log_loss).max(0.0);
    let top1_gap_vs_freq = (freq_baseline.top1_accuracy - metrics.top1_accuracy).max(0.0);
    let top3_gap_vs_freq = (freq_baseline.top3_coverage - metrics.top3_coverage).max(0.0);
    let true_prob_gap_vs_freq =
        (freq_baseline.mean_true_prob - metrics.mean_true_prob).max(0.0);

    let mut severity = 6.2 * logloss_gap_vs_freq
        + 3.1 * logloss_gap_vs_random
        + 1.8 * top1_gap_vs_freq
        + 2.1 * top3_gap_vs_freq
        + 1.5 * true_prob_gap_vs_freq;
    if metrics.mean_log_loss >= random_baseline.mean_log_loss + 0.02
        && metrics.top3_coverage <= freq_baseline.top3_coverage + 0.02
    {
        severity += 0.24;
    }
    if expert == "markov" && metrics.top1_accuracy >= freq_baseline.top1_accuracy + 0.01 {
        severity *= 0.74;
    }
    if matches!(expert, "exact_motif" | "approx_shape")
        && metrics.top3_coverage <= freq_baseline.top3_coverage + 0.01
    {
        severity += 0.10;
    }

    let floor = match (objective, expert) {
        (V2BlendObjective::Top1, "markov") => 0.72,
        (V2BlendObjective::Top1, "exact_motif") => 0.34,
        (V2BlendObjective::Top1, "approx_shape") => 0.46,
        (V2BlendObjective::Calibrated, "markov") => 0.78,
        (V2BlendObjective::Calibrated, "exact_motif") => 0.42,
        (V2BlendObjective::Calibrated, "approx_shape") => 0.52,
        _ => 0.80,
    };
    (1.0 - 0.40 * subset_evidence * severity.clamp(0.0, 1.8)).clamp(floor, 1.0)
}

fn compute_expert_regret_entry(
    samples: &[BacktestSample],
    expert: &str,
    min_samples: usize,
) -> Option<ExpertRegretEntry> {
    let subset = samples
        .iter()
        .filter(|sample| expert_active_for_sample(&sample.components, expert))
        .cloned()
        .collect::<Vec<_>>();
    if subset.len() < min_samples.max(4) {
        return None;
    }

    let metrics = evaluate_expert_subset_metrics(&subset, expert);
    let freq_baseline = evaluate_frequency_baseline(&subset);
    let random_baseline = evaluate_random_baseline(&subset);
    let evidence = (subset.len() as f64 / (subset.len() as f64 + (min_samples.max(4) * 2) as f64))
        .clamp(0.0, 1.0);
    let best_logloss = freq_baseline.mean_log_loss.min(random_baseline.mean_log_loss);
    let logloss_regret = (metrics.mean_log_loss - best_logloss).max(0.0);
    let top1_regret = (freq_baseline.top1_accuracy - metrics.top1_accuracy).max(0.0);
    let top3_regret = (freq_baseline.top3_coverage - metrics.top3_coverage).max(0.0);
    let true_prob_regret = (freq_baseline.mean_true_prob - metrics.mean_true_prob).max(0.0);
    let mut severity = 4.8 * logloss_regret
        + 1.8 * top1_regret
        + 1.9 * top3_regret
        + 1.4 * true_prob_regret;
    if metrics.mean_log_loss >= random_baseline.mean_log_loss + 0.02
        && metrics.top3_coverage <= freq_baseline.top3_coverage + 0.02
    {
        severity += 0.16;
    }
    if expert == "auto_cycle"
        && metrics.mean_log_loss <= random_baseline.mean_log_loss + 0.004
        && metrics.top3_coverage >= random_baseline.top3_coverage - 1e-9
    {
        severity *= 0.40;
    }
    if expert == "markov" && metrics.top1_accuracy >= freq_baseline.top1_accuracy + 0.006 {
        severity *= 0.78;
    }
    let floor = match expert {
        "base" => 0.92,
        "markov" => 0.76,
        "exact_motif" => 0.38,
        "approx_shape" => 0.48,
        "auto_cycle" => 0.88,
        "state_context" => 0.86,
        _ => 0.80,
    };
    Some(ExpertRegretEntry {
        penalty: (1.0 - 0.28 * evidence * severity.clamp(0.0, 1.65)).clamp(floor, 1.0),
        sample_count: subset.len(),
    })
}

fn build_expert_regret_table(
    samples: &[BacktestSample],
    min_samples: usize,
) -> HashMap<String, ExpertRegretEntry> {
    let mut table = HashMap::new();
    for expert in [
        "base",
        "markov",
        "exact_motif",
        "approx_shape",
        "auto_cycle",
        "state_context",
    ] {
        if let Some(entry) = compute_expert_regret_entry(samples, expert, min_samples) {
            table.insert(expert.to_string(), entry);
        }
    }
    table
}

fn duel_pairs() -> [(&'static str, &'static str, &'static str); 3] {
    [
        ("cycle_vs_markov", "auto_cycle", "markov"),
        ("exact_vs_base", "exact_motif", "base"),
        ("approx_vs_base", "approx_shape", "base"),
    ]
}

fn compute_expert_duel_entry(
    samples: &[BacktestSample],
    pair_id: &str,
    left: &str,
    right: &str,
    min_samples: usize,
) -> Option<ExpertDuelEntry> {
    let subset = samples
        .iter()
        .filter(|sample| {
            expert_active_for_sample(&sample.components, left)
                && expert_active_for_sample(&sample.components, right)
        })
        .cloned()
        .collect::<Vec<_>>();
    if subset.len() < min_samples.max(4) {
        return None;
    }

    let left_metrics = evaluate_expert_subset_metrics(&subset, left);
    let right_metrics = evaluate_expert_subset_metrics(&subset, right);
    let delta_top1 = left_metrics.top1_accuracy - right_metrics.top1_accuracy;
    let delta_top3 = left_metrics.top3_coverage - right_metrics.top3_coverage;
    let delta_true = left_metrics.mean_true_prob - right_metrics.mean_true_prob;
    let delta_logloss = right_metrics.mean_log_loss - left_metrics.mean_log_loss;
    let evidence =
        (subset.len() as f64 / (subset.len() as f64 + (min_samples.max(4) * 2) as f64)).clamp(0.0, 1.0);

    let score = match pair_id {
        "cycle_vs_markov" => {
            1.8 * delta_logloss + 1.2 * delta_top3 + 1.0 * delta_top1 + 0.6 * delta_true
        }
        "exact_vs_base" | "approx_vs_base" => {
            1.5 * delta_top1 + 0.7 * delta_top3 + 0.5 * delta_true + 1.1 * delta_logloss
        }
        _ => 0.0,
    };
    if score.abs() < 0.012 {
        return None;
    }

    let (winner, loser, cap) = match pair_id {
        "cycle_vs_markov" => {
            if score >= 0.0 {
                (left, right, 0.22)
            } else {
                (right, left, 0.18)
            }
        }
        "exact_vs_base" => {
            if score >= 0.0 {
                (left, right, 0.14)
            } else {
                (right, left, 0.12)
            }
        }
        "approx_vs_base" => {
            if score >= 0.0 {
                (left, right, 0.12)
            } else {
                (right, left, 0.10)
            }
        }
        _ => return None,
    };
    let transfer = (score.abs() * evidence * 1.8).clamp(0.04, cap);
    Some(ExpertDuelEntry {
        winner: winner.to_string(),
        loser: loser.to_string(),
        transfer,
        sample_count: subset.len(),
    })
}

fn build_expert_duel_table(
    samples: &[BacktestSample],
    min_samples: usize,
) -> HashMap<String, ExpertDuelEntry> {
    let mut table = HashMap::new();
    for (pair_id, left, right) in duel_pairs() {
        if let Some(entry) = compute_expert_duel_entry(samples, pair_id, left, right, min_samples) {
            table.insert(pair_id.to_string(), entry);
        }
    }
    table
}

fn select_regret_penalty_for_expert(
    global_regrets: &HashMap<String, ExpertRegretEntry>,
    bucket_regrets: &HashMap<String, HashMap<String, ExpertRegretEntry>>,
    bucket_min_samples: usize,
    bucket: &PredictionBucket,
    expert: &str,
) -> (f64, Option<BucketScope>) {
    let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
    let mut penalty = global_regrets
        .get(expert)
        .filter(|entry| entry.sample_count >= fit_min_samples)
        .map(|entry| entry.penalty)
        .unwrap_or(1.0);
    let mut most_specific_scope = None;
    for scope in BucketScope::blend_scopes() {
        let scoped_key = bucket.scoped_key(scope);
        let Some(expert_table) = bucket_regrets.get(&scoped_key) else {
            continue;
        };
        let Some(entry) = expert_table.get(expert) else {
            continue;
        };
        if entry.sample_count < fit_min_samples {
            continue;
        }
        let trust = (0.65 * bucket_scope_trust(entry.sample_count, bucket_min_samples, scope))
            .clamp(0.0, 0.80);
        penalty = penalty * (1.0 - trust) + entry.penalty * trust;
        most_specific_scope = Some(scope);
    }
    (penalty.clamp(0.35, 1.0), most_specific_scope)
}

fn apply_regret_table_to_weights_raw(
    weights: InternalBlendWeights,
    global_expert_regrets: &HashMap<String, ExpertRegretEntry>,
    bucket_expert_regrets: &HashMap<String, HashMap<String, ExpertRegretEntry>>,
    bucket_min_samples: usize,
    components: &V2ComponentBuild,
) -> (InternalBlendWeights, Vec<String>) {
    let mut adjusted = weights;
    let mut tags = Vec::new();
    for (expert, short) in [
        ("markov", "M"),
        ("exact_motif", "X"),
        ("approx_shape", "A"),
        ("auto_cycle", "C"),
        ("state_context", "S"),
    ] {
        if !expert_active_for_sample(components, expert) {
            continue;
        }
        let (penalty, scope) = select_regret_penalty_for_expert(
            global_expert_regrets,
            bucket_expert_regrets,
            bucket_min_samples,
            &components.bucket,
            expert,
        );
        let runtime_strength = match expert {
            "markov" => 0.42,
            "exact_motif" => 0.72,
            "approx_shape" => 0.64,
            "auto_cycle" => 0.35,
            "state_context" => 0.40,
            _ => 0.50,
        };
        let effective_penalty = 1.0 - runtime_strength * (1.0 - penalty);
        if effective_penalty >= 0.90 {
            continue;
        }
        scale_expert_weight(&mut adjusted, expert, effective_penalty);
        if let Some(scope) = scope {
            tags.push(format!("{}@{} {:.2}", short, scope.objective_source_label(), effective_penalty));
        } else {
            tags.push(format!("{}@g {:.2}", short, effective_penalty));
        }
    }
    normalize_internal_weights(&mut adjusted);
    (adjusted, tags)
}

fn get_expert_weight(weights: InternalBlendWeights, expert: &str) -> f64 {
    match expert {
        "base" => weights.base,
        "markov" => weights.markov,
        "exact_motif" => weights.exact_motif,
        "approx_shape" => weights.approx_shape,
        "auto_cycle" => weights.auto_cycle,
        "state_context" => weights.state_context,
        _ => 0.0,
    }
}

fn select_duel_entry_for_pair(
    global_duels: &HashMap<String, ExpertDuelEntry>,
    bucket_duels: &HashMap<String, HashMap<String, ExpertDuelEntry>>,
    bucket_min_samples: usize,
    bucket: &PredictionBucket,
    pair_id: &str,
) -> Option<(ExpertDuelEntry, &'static str)> {
    let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
    for scope in BucketScope::objective_scopes() {
        let scoped_key = bucket.scoped_key(scope);
        let Some(pair_table) = bucket_duels.get(&scoped_key) else {
            continue;
        };
        let Some(entry) = pair_table.get(pair_id) else {
            continue;
        };
        if entry.sample_count < fit_min_samples {
            continue;
        }
        return Some((entry.clone(), scope.objective_source_label()));
    }
    global_duels
        .get(pair_id)
        .filter(|entry| entry.sample_count >= fit_min_samples)
        .cloned()
        .map(|entry| (entry, "g"))
}

fn apply_duel_table_to_weights_raw(
    weights: InternalBlendWeights,
    global_duels: &HashMap<String, ExpertDuelEntry>,
    bucket_duels: &HashMap<String, HashMap<String, ExpertDuelEntry>>,
    bucket_min_samples: usize,
    components: &V2ComponentBuild,
) -> (InternalBlendWeights, Vec<String>) {
    let mut adjusted = weights;
    let mut tags = Vec::new();
    for (pair_id, _, _) in duel_pairs() {
        let Some((entry, scope_label)) = select_duel_entry_for_pair(
            global_duels,
            bucket_duels,
            bucket_min_samples,
            &components.bucket,
            pair_id,
        ) else {
            continue;
        };
        if !expert_active_for_sample(components, &entry.winner)
            || !expert_active_for_sample(components, &entry.loser)
        {
            continue;
        }
        let runtime_strength = match pair_id {
            "cycle_vs_markov" => 0.72,
            "exact_vs_base" => 0.58,
            "approx_vs_base" => 0.52,
            _ => 0.50,
        };
        let transfer = (entry.transfer * runtime_strength).clamp(0.0, 0.18);
        if transfer < 0.06 {
            continue;
        }
        let loser_weight = get_expert_weight(adjusted, &entry.loser);
        if loser_weight <= 1e-6 {
            continue;
        }
        let move_mass = (loser_weight * transfer).clamp(0.0, loser_weight * 0.60);
        if move_mass <= 1e-6 {
            continue;
        }
        scale_expert_weight(
            &mut adjusted,
            &entry.loser,
            ((loser_weight - move_mass) / loser_weight).clamp(0.0, 1.0),
        );
        match entry.winner.as_str() {
            "base" => adjusted.base += move_mass,
            "markov" => adjusted.markov += move_mass,
            "exact_motif" => adjusted.exact_motif += move_mass,
            "approx_shape" => adjusted.approx_shape += move_mass,
            "auto_cycle" => adjusted.auto_cycle += move_mass,
            "state_context" => adjusted.state_context += move_mass,
            _ => {}
        }
        let winner_short = match entry.winner.as_str() {
            "auto_cycle" => "C",
            "markov" => "M",
            "base" => "B",
            "exact_motif" => "X",
            "approx_shape" => "A",
            "state_context" => "S",
            _ => "?",
        };
        let loser_short = match entry.loser.as_str() {
            "auto_cycle" => "C",
            "markov" => "M",
            "base" => "B",
            "exact_motif" => "X",
            "approx_shape" => "A",
            "state_context" => "S",
            _ => "?",
        };
        tags.push(format!("{}>{}@{} +{:.2}", winner_short, loser_short, scope_label, move_mass));
    }
    normalize_internal_weights(&mut adjusted);
    (adjusted, tags)
}

fn apply_regret_table_to_weights(
    weights: InternalBlendWeights,
    model: &TrainedAdaptiveModel,
    components: &V2ComponentBuild,
) -> (InternalBlendWeights, Vec<String>) {
    apply_regret_table_to_weights_raw(
        weights,
        &model.global_expert_regrets,
        &model.bucket_expert_regrets,
        model.bucket_min_samples,
        components,
    )
}

fn choose_v2_objective(
    calibrated_metrics: FlatBacktestMetrics,
    top1_metrics: FlatBacktestMetrics,
) -> V2BlendObjective {
    if top1_metrics.top1_accuracy >= calibrated_metrics.top1_accuracy + 0.003 {
        return V2BlendObjective::Top1;
    }
    if calibrated_metrics.top1_accuracy >= top1_metrics.top1_accuracy + 0.003 {
        return V2BlendObjective::Calibrated;
    }
    if calibrated_metrics.mean_log_loss + 0.012 < top1_metrics.mean_log_loss {
        return V2BlendObjective::Calibrated;
    }
    if top1_metrics.top3_coverage >= calibrated_metrics.top3_coverage + 0.015 {
        return V2BlendObjective::Top1;
    }
    if calibrated_metrics.top3_coverage >= top1_metrics.top3_coverage + 0.015 {
        return V2BlendObjective::Calibrated;
    }
    if top1_metrics.mean_true_prob >= calibrated_metrics.mean_true_prob + 0.0015 {
        return V2BlendObjective::Top1;
    }
    V2BlendObjective::Calibrated
}

fn score_bucket_champion_candidate(
    route: BucketChampionRoute,
    blend_metrics: FlatBacktestMetrics,
    candidate_metrics: FlatBacktestMetrics,
    sample_count: usize,
    min_samples: usize,
) -> BucketChampionCandidateSummary {
    let evidence =
        (sample_count as f64 / (sample_count as f64 + (min_samples.max(4) * 2) as f64)).clamp(0.0, 1.0);
    let min_top1_lift = (0.012 - 0.006 * evidence).clamp(0.006, 0.012);
    let equal_top3_lift = (0.10 - 0.04 * evidence).clamp(0.05, 0.10);
    let top1_gain = candidate_metrics.top1_accuracy - blend_metrics.top1_accuracy;
    let top3_gain = candidate_metrics.top3_coverage - blend_metrics.top3_coverage;
    let true_gain = candidate_metrics.mean_true_prob - blend_metrics.mean_true_prob;
    let logloss_gap = candidate_metrics.mean_log_loss - blend_metrics.mean_log_loss;
    let gain_score = 5.6 * top1_gain + 1.4 * top3_gain + 0.8 * true_gain - 0.9 * logloss_gap;
    let qualifies = top1_gain >= min_top1_lift
        || (top1_gain > 1e-9 && gain_score >= 0.010 && logloss_gap <= 0.05)
        || (top1_gain.abs() <= 1e-9 && top3_gain >= equal_top3_lift && logloss_gap <= 0.015);
    let top1_margin = top1_gain - min_top1_lift;
    let score_margin = if top1_gain > 1e-9 && logloss_gap <= 0.05 {
        gain_score - 0.010
    } else {
        f64::NEG_INFINITY
    };
    let top3_margin = if top1_gain.abs() <= 1e-9 && logloss_gap <= 0.015 {
        top3_gain - equal_top3_lift
    } else {
        f64::NEG_INFINITY
    };
    let qualify_margin = top1_margin.max(score_margin).max(top3_margin);
    let positive_signal =
        top1_gain > 1e-9 || top3_gain >= 0.015 || true_gain >= 0.001 || logloss_gap <= -0.010;
    let top1_progress = (top1_gain / min_top1_lift).clamp(-1.5, 2.0);
    let top3_progress = (top3_gain / equal_top3_lift).clamp(-1.5, 2.0);
    let mut ranking_score = gain_score
        + 0.24 * top1_progress.max(0.0)
        + 0.10 * top3_progress.max(0.0)
        - 0.35 * logloss_gap.max(0.0);
    ranking_score += 0.08 * (qualify_margin * 100.0).clamp(-4.0, 4.0);
    if qualifies {
        ranking_score += 10.0;
    }
    if positive_signal {
        ranking_score += 0.12;
    } else if top1_gain.abs() <= 1e-9
        && top3_gain.abs() <= 1e-9
        && true_gain.abs() <= 0.001
        && logloss_gap.abs() <= 0.001
    {
        ranking_score -= 0.15;
    }
    if top1_gain < -1e-9 || logloss_gap > 0.08 {
        ranking_score -= 1.0;
    }
    BucketChampionCandidateSummary {
        bucket_key: String::new(),
        route,
        sample_count,
        top1_gain,
        top3_gain,
        true_gain,
        logloss_gap,
        gain_score,
        qualifies,
        ranking_score,
        min_top1_lift,
        equal_top3_lift,
        qualify_margin,
        positive_signal,
    }
}

fn collect_bucket_champion_candidates(
    blend_metrics: FlatBacktestMetrics,
    markov_metrics: FlatBacktestMetrics,
    auto_cycle_metrics: FlatBacktestMetrics,
    sample_count: usize,
    min_samples: usize,
) -> Vec<BucketChampionCandidateSummary> {
    let mut candidates = vec![
        score_bucket_champion_candidate(
            BucketChampionRoute::PreferMarkov,
            blend_metrics,
            markov_metrics,
            sample_count,
            min_samples,
        ),
        score_bucket_champion_candidate(
            BucketChampionRoute::PreferAutoCycle,
            blend_metrics,
            auto_cycle_metrics,
            sample_count,
            min_samples,
        ),
    ];
    candidates.sort_by(compare_bucket_champion_candidates);
    candidates
}

fn choose_bucket_champion_route(
    blend_metrics: FlatBacktestMetrics,
    markov_metrics: FlatBacktestMetrics,
    auto_cycle_metrics: FlatBacktestMetrics,
    sample_count: usize,
    min_samples: usize,
) -> BucketChampionRoute {
    collect_bucket_champion_candidates(
        blend_metrics,
        markov_metrics,
        auto_cycle_metrics,
        sample_count,
        min_samples,
    )
    .into_iter()
    .find(|candidate| candidate.qualifies)
    .map(|candidate| candidate.route)
    .unwrap_or(BucketChampionRoute::Blend)
}

fn should_enable_bucket_champion_routing(
    blend_metrics: FlatBacktestMetrics,
    champion_metrics: FlatBacktestMetrics,
) -> bool {
    let top1_gain = champion_metrics.top1_accuracy - blend_metrics.top1_accuracy;
    let top3_gain = champion_metrics.top3_coverage - blend_metrics.top3_coverage;
    let true_gain = champion_metrics.mean_true_prob - blend_metrics.mean_true_prob;
    let logloss_gap = champion_metrics.mean_log_loss - blend_metrics.mean_log_loss;
    top1_gain >= 0.006 && logloss_gap <= 0.05
        || (top1_gain > 1e-9 && top3_gain >= 0.02 && logloss_gap <= 0.03)
        || (top1_gain.abs() <= 1e-9
            && top3_gain >= 0.10
            && true_gain >= 0.002
            && logloss_gap <= -0.02)
}

fn select_v2_objective_for_bucket(
    model: &TrainedAdaptiveModel,
    bucket: &PredictionBucket,
) -> (V2BlendObjective, &'static str) {
    select_v2_objective_for_bucket_raw(
        model.global_objective,
        &model.bucket_objectives,
        model.bucket_min_samples,
        model.backtest_summary.sample_count as usize,
        bucket,
    )
}

#[derive(Default, Clone)]
struct V3ExpertTracker {
    sample_count: usize,
    log_sums: [f64; 6],
}

impl V3ExpertTracker {
    fn update(
        &mut self,
        components: &V2ComponentBuild,
        actual_stat_key: &str,
        blocked_stats: &HashSet<String>,
    ) {
        let expert_maps = [
            &components.base_probs,
            &components.markov_probs,
            &components.exact_motif_probs,
            &components.approx_shape_probs,
            &components.auto_cycle_probs,
            &components.state_context_probs,
        ];
        for (idx, expert_map) in expert_maps.iter().enumerate() {
            let p = masked_probability_for_stat(expert_map, actual_stat_key, blocked_stats);
            self.log_sums[idx] += p.ln();
        }
        self.sample_count += 1;
    }

    fn to_weights(&self) -> InternalBlendWeights {
        if self.sample_count == 0 {
            return default_v3_weights();
        }
        let prior = default_v3_weights();
        let priors = [
            prior.base,
            prior.markov,
            prior.exact_motif,
            prior.approx_shape,
            prior.auto_cycle,
            prior.state_context,
        ];
        let means = self
            .log_sums
            .iter()
            .map(|score| *score / self.sample_count as f64)
            .collect::<Vec<_>>();
        let mean_score = means.iter().sum::<f64>() / means.len().max(1) as f64;
        let mut raw = [0.0; 6];
        for idx in 0..6 {
            raw[idx] = priors[idx] * ((means[idx] - mean_score) / 0.45).exp();
        }
        let total = raw.iter().sum::<f64>().max(1e-9);
        let mut weights = InternalBlendWeights {
            base: raw[0] / total,
            markov: raw[1] / total,
            exact_motif: raw[2] / total,
            approx_shape: raw[3] / total,
            auto_cycle: raw[4] / total,
            state_context: raw[5] / total,
        };
        normalize_internal_weights(&mut weights);
        weights
    }
}

struct JointPredictionBundle {
    tier_suggestions: HashMap<String, Vec<crate::domain::types::TierSuggestion>>,
    tier_probs: HashMap<String, HashMap<i64, f64>>,
    best_tier_index: HashMap<String, Option<i64>>,
    best_tier_probability: HashMap<String, f64>,
    joint_probability: HashMap<String, f64>,
    ranking: Vec<(String, Option<i64>, f64)>,
}

fn normalize_tier_probability_map(map: &mut HashMap<i64, f64>) {
    let total = map.values().sum::<f64>();
    if total <= 1e-9 {
        let uniform = 1.0 / map.len().max(1) as f64;
        for value in map.values_mut() {
            *value = uniform;
        }
        return;
    }
    for value in map.values_mut() {
        *value /= total;
    }
}

fn load_day_event_sequence(
    conn: &Connection,
    game_day: &str,
) -> Result<Vec<crate::pattern_state::PatternEventLite>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT stat_key, tier_index, analysis_seq
             FROM ordered_events
             WHERE game_day = ?1
             ORDER BY analysis_seq ASC, created_seq ASC",
        )
        .map_err(|e| format!("failed to prepare day event sequence query: {e}"))?;
    let rows = stmt
        .query_map([game_day], |row| {
            Ok(crate::pattern_state::PatternEventLite {
                stat_key: row.get::<_, String>(0)?,
                tier_index: row.get::<_, i64>(1)?,
                analysis_seq: row.get::<_, i64>(2)?,
            })
        })
        .map_err(|e| format!("failed to query day event sequence: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect day event sequence: {e}"))
}

fn build_short_local_context_probs(
    events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
    base_probs: &HashMap<String, f64>,
) -> (
    HashMap<String, f64>,
    bool,
    HashMap<String, Vec<String>>,
) {
    if events.is_empty() {
        return (
            base_probs.clone(),
            false,
            HashMap::<String, Vec<String>>::new(),
        );
    }

    let recent = if events.len() > 6 {
        &events[events.len() - 6..]
    } else {
        events
    };
    let mut stat_recent = HashMap::<String, f64>::new();
    let mut category_recent = HashMap::<String, f64>::new();
    for (idx, event) in recent.iter().enumerate() {
        let weight = 1.0 + idx as f64 * 0.28;
        *stat_recent.entry(event.stat_key.clone()).or_insert(0.0) += weight;
        *category_recent
            .entry(crate::pattern_state::stat_category(&event.stat_key).to_string())
            .or_insert(0.0) += weight;
    }
    let stat_total = stat_recent.values().sum::<f64>().max(1e-9);
    let category_total = category_recent.values().sum::<f64>().max(1e-9);

    let mut category_base_totals = HashMap::<String, f64>::new();
    for stat_key in stat_keys {
        let category = crate::pattern_state::stat_category(stat_key).to_string();
        *category_base_totals.entry(category).or_insert(0.0) +=
            base_probs.get(stat_key).copied().unwrap_or(0.0);
    }

    let dominant_category = category_recent
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
        .map(|(category, weight)| (category.clone(), *weight / category_total))
        .unwrap_or_else(|| ("mixed".to_string(), 0.0));

    let mut probs = HashMap::<String, f64>::new();
    let mut signals = HashMap::<String, Vec<String>>::new();
    for stat_key in stat_keys {
        let recent_stat_p = stat_recent.get(stat_key).copied().unwrap_or(0.0) / stat_total;
        let category = crate::pattern_state::stat_category(stat_key).to_string();
        let recent_category_p = category_recent.get(&category).copied().unwrap_or(0.0) / category_total;
        let within_category = {
            let base_mass = category_base_totals.get(&category).copied().unwrap_or(0.0);
            if base_mass > 1e-9 {
                base_probs.get(stat_key).copied().unwrap_or(0.0) / base_mass
            } else {
                0.0
            }
        };
        let prob = 0.58 * recent_stat_p
            + 0.32 * recent_category_p * within_category
            + 0.10 * base_probs.get(stat_key).copied().unwrap_or(0.0);
        probs.insert(stat_key.clone(), prob.max(1e-9));
        if dominant_category.1 >= 0.52 && category == dominant_category.0 {
            signals
                .entry(stat_key.clone())
                .or_default()
                .push(format!("短窗集中 {} {:.0}%", category, dominant_category.1 * 100.0));
        }
    }
    normalize_probability_map(&mut probs);
    (probs, dominant_category.1 >= 0.52, signals)
}

fn build_day_category_context_probs(
    events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
    base_probs: &HashMap<String, f64>,
    lookback: usize,
) -> (
    HashMap<String, f64>,
    bool,
    HashMap<String, Vec<String>>,
) {
    let focus_events = if events.len() > lookback {
        &events[events.len() - lookback..]
    } else {
        events
    };
    let categories = focus_events
        .iter()
        .map(|event| crate::pattern_state::stat_category(&event.stat_key).to_string())
        .collect::<Vec<_>>();
    if categories.len() < 3 {
        return (
            base_probs.clone(),
            false,
            HashMap::<String, Vec<String>>::new(),
        );
    }

    let unique_categories = categories.iter().cloned().collect::<HashSet<_>>();
    let category_count = unique_categories.len().max(1) as f64;
    let effective_max_order = (categories.len() - 1).min(3);
    let mut category_scores = HashMap::<String, f64>::new();
    let mut total_weight = 0.0;

    for order in 1..=effective_max_order {
        let context = &categories[categories.len() - order..];
        let mut next_counts = HashMap::<String, i64>::new();
        let mut sample_total = 0i64;
        for idx in order..categories.len() {
            if &categories[idx - order..idx] == context {
                *next_counts.entry(categories[idx].clone()).or_insert(0) += 1;
                sample_total += 1;
            }
        }
        if sample_total <= 0 {
            continue;
        }
        let confidence = sample_total as f64 / (sample_total as f64 + 1.8 * order as f64);
        let weight = confidence * (order as f64).powf(1.35);
        total_weight += weight;
        for category in &unique_categories {
            let count = *next_counts.get(category).unwrap_or(&0) as f64;
            let p = (count + 0.35) / (sample_total as f64 + 0.35 * category_count);
            *category_scores.entry(category.clone()).or_insert(0.0) += weight * p;
        }
    }

    if total_weight <= 1e-9 {
        return (
            base_probs.clone(),
            false,
            HashMap::<String, Vec<String>>::new(),
        );
    }
    for value in category_scores.values_mut() {
        *value /= total_weight;
    }

    let mut category_base_totals = HashMap::<String, f64>::new();
    let mut category_member_counts = HashMap::<String, usize>::new();
    for stat_key in stat_keys {
        let category = crate::pattern_state::stat_category(stat_key).to_string();
        *category_base_totals.entry(category.clone()).or_insert(0.0) +=
            base_probs.get(stat_key).copied().unwrap_or(0.0);
        *category_member_counts.entry(category).or_insert(0) += 1;
    }

    let mut probs = HashMap::<String, f64>::new();
    let mut signals = HashMap::<String, Vec<String>>::new();
    for stat_key in stat_keys {
        let category = crate::pattern_state::stat_category(stat_key).to_string();
        let cat_prob = category_scores.get(&category).copied().unwrap_or(0.0);
        let base_mass = category_base_totals.get(&category).copied().unwrap_or(0.0);
        let member_count = category_member_counts.get(&category).copied().unwrap_or(1) as f64;
        let within_category = if base_mass > 1e-9 {
            base_probs.get(stat_key).copied().unwrap_or(0.0) / base_mass
        } else {
            1.0 / member_count
        };
        probs.insert(stat_key.clone(), (cat_prob * within_category).max(1e-9));
        if cat_prob >= 0.28 {
            signals
                .entry(stat_key.clone())
                .or_default()
                .push(format!(
                    "类别窗{} {} {:.0}%",
                    lookback.min(events.len()),
                    category,
                    cat_prob * 100.0
                ));
        }
    }
    normalize_probability_map(&mut probs);
    (
        probs,
        category_scores.values().copied().fold(0.0, f64::max) >= 0.34,
        signals,
    )
}

fn build_day_similarity_context_probs(
    events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
    base_probs: &HashMap<String, f64>,
) -> (
    HashMap<String, f64>,
    bool,
    HashMap<String, Vec<String>>,
) {
    let n = events.len();
    if n < 6 {
        return (
            base_probs.clone(),
            false,
            HashMap::<String, Vec<String>>::new(),
        );
    }
    let window_len = n.min(5);
    let current_window = &events[n - window_len..];
    let current_active = events[n.saturating_sub(6)..]
        .iter()
        .map(|event| event.stat_key.as_str())
        .collect::<HashSet<_>>()
        .len() as i64;
    let current_run_category = crate::pattern_state::stat_category(&events[n - 1].stat_key);
    let current_run_len = events
        .iter()
        .rev()
        .take_while(|event| crate::pattern_state::stat_category(&event.stat_key) == current_run_category)
        .count() as i64;

    let mut stat_scores = stat_keys
        .iter()
        .map(|stat_key| (stat_key.clone(), 0.0))
        .collect::<HashMap<_, _>>();
    let mut matched_contexts = 0i64;

    for next_idx in window_len..n {
        let hist_window = &events[next_idx - window_len..next_idx];
        let hist_active = events[next_idx.saturating_sub(6)..next_idx]
            .iter()
            .map(|event| event.stat_key.as_str())
            .collect::<HashSet<_>>()
            .len() as i64;
        let hist_run_category = crate::pattern_state::stat_category(&hist_window[window_len - 1].stat_key);
        let hist_run_len = hist_window
            .iter()
            .rev()
            .take_while(|event| crate::pattern_state::stat_category(&event.stat_key) == hist_run_category)
            .count() as i64;

        let mut similarity = 0.0;
        for idx in 0..window_len {
            let pos_weight = 1.0 + idx as f64 * 0.28;
            let cur_category = crate::pattern_state::stat_category(&current_window[idx].stat_key);
            let hist_category = crate::pattern_state::stat_category(&hist_window[idx].stat_key);
            if cur_category == hist_category {
                similarity += pos_weight * 1.15;
            }
            if current_window[idx].stat_key == hist_window[idx].stat_key {
                similarity += pos_weight * 0.45;
            }
        }
        let active_diff = (current_active - hist_active).abs() as f64;
        similarity += (1.3 - active_diff * 0.30).max(0.0);
        let run_diff = (current_run_len - hist_run_len).abs() as f64;
        similarity += (1.2 - run_diff * 0.35).max(0.0);
        if current_run_category == hist_run_category {
            similarity += 0.9;
        }

        if similarity < 4.0 {
            continue;
        }
        matched_contexts += 1;
        let recency = 0.78 + 0.22 * (next_idx as f64 / n as f64);
        let weight = (similarity - 3.6).powf(1.28) * recency;
        if let Some(score) = stat_scores.get_mut(&events[next_idx].stat_key) {
            *score += weight;
        }
    }

    let total_score = stat_scores.values().sum::<f64>();
    if total_score <= 1e-9 {
        return (
            base_probs.clone(),
            false,
            HashMap::<String, Vec<String>>::new(),
        );
    }

    let mut probs = HashMap::<String, f64>::new();
    let mut signals = HashMap::<String, Vec<String>>::new();
    for stat_key in stat_keys {
        let local = stat_scores.get(stat_key).copied().unwrap_or(0.0) / total_score;
        let blended = 0.85 * local + 0.15 * base_probs.get(stat_key).copied().unwrap_or(0.0);
        probs.insert(stat_key.clone(), blended.max(1e-9));
        if local >= 0.22 {
            signals
                .entry(stat_key.clone())
                .or_default()
                .push(format!("日内相似窗口 {} 例", matched_contexts));
        }
    }
    normalize_probability_map(&mut probs);
    (probs, matched_contexts >= 2, signals)
}

fn build_state_context_expert(
    events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
    base_probs: &HashMap<String, f64>,
    display_map: Option<&HashMap<String, String>>,
) -> (
    HashMap<String, f64>,
    bool,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    crate::pattern_state::SequenceStateFeatures,
) {
    let summary = crate::pattern_state::compute_sequence_state_features(events, stat_keys);
    let mut zone_reversion_probs = HashMap::new();
    let mut labels = HashMap::<String, Vec<String>>::new();
    let mut sources = HashMap::<String, Vec<String>>::new();
    let mut state_signals = HashMap::<String, Vec<String>>::new();
    let crit_gate_on = summary.active_stat_bucket == "low" && summary.crit_signal != "none";

    for stat_key in stat_keys {
        let base = *base_probs.get(stat_key).unwrap_or(&0.0);
        let mut multiplier = 1.0;
        let mut signals = Vec::<String>::new();

        if summary.zone_candidate != "mixed" {
            if crate::pattern_state::stat_matches_zone(stat_key, &summary.zone_candidate) {
                multiplier *= 1.0 + 0.45 * summary.zone_confidence;
                signals.push(format!(
                    "区间 {} {:.0}%",
                    summary.zone_candidate,
                    summary.zone_confidence * 100.0
                ));
            } else {
                multiplier *= (1.0 - 0.20 * summary.zone_confidence).max(0.55);
            }
        }

        let reversion_score = summary
            .reversion_score_by_stat
            .get(stat_key)
            .copied()
            .unwrap_or(0.0);
        multiplier *= reversion_score.exp();
        if reversion_score > 0.04 {
            signals.push(format!("回归 {:.2}", reversion_score));
        }

        if crit_gate_on && matches!(stat_key.as_str(), "crit_rate" | "crit_dmg") {
            multiplier *= 1.20;
            signals.push("低活跃暴区门控".to_string());
        }

        zone_reversion_probs.insert(stat_key.clone(), (base * multiplier).max(1e-9));
        if !signals.is_empty() {
            state_signals.insert(stat_key.clone(), signals);
        }
    }
    normalize_probability_map(&mut zone_reversion_probs);

    let (short_local_probs, short_local_active, short_local_signals) =
        build_short_local_context_probs(events, stat_keys, base_probs);
    let (short_category_probs, short_category_active, short_category_signals) =
        build_day_category_context_probs(events, stat_keys, base_probs, 12);
    let (medium_category_probs, medium_category_active, medium_category_signals) =
        build_day_category_context_probs(events, stat_keys, base_probs, 24);
    let (similarity_probs, similarity_active, similarity_signals) =
        build_day_similarity_context_probs(events, stat_keys, base_probs);

    let (
        mut zone_weight,
        mut short_local_weight,
        mut short_category_weight,
        mut medium_category_weight,
        mut similarity_weight,
    ): (f64, f64, f64, f64, f64) =
        match summary.regime_stage.as_str() {
            "new_regime" => (0.20, 0.40, 0.10, 0.10, 0.20),
            "transitioning" => (0.15, 0.32, 0.18, 0.15, 0.20),
            _ => (0.15, 0.15, 0.22, 0.23, 0.25),
        };
    if !short_local_active {
        short_local_weight = 0.0;
    }
    if !short_category_active {
        short_category_weight = 0.0;
    }
    if !medium_category_active {
        medium_category_weight = 0.0;
    }
    if !similarity_active {
        similarity_weight = 0.0;
    }
    if short_local_weight + short_category_weight + medium_category_weight + similarity_weight <= 1e-9 {
        zone_weight = 1.0;
    }
    let total_weight = (zone_weight
        + short_local_weight
        + short_category_weight
        + medium_category_weight
        + similarity_weight)
        .max(1e-9_f64);
    zone_weight /= total_weight;
    short_local_weight /= total_weight;
    short_category_weight /= total_weight;
    medium_category_weight /= total_weight;
    similarity_weight /= total_weight;

    let mut probs = HashMap::new();
    for stat_key in stat_keys {
        probs.insert(
            stat_key.clone(),
            zone_weight * zone_reversion_probs.get(stat_key).copied().unwrap_or(0.0)
                + short_local_weight * short_local_probs.get(stat_key).copied().unwrap_or(0.0)
                + short_category_weight * short_category_probs.get(stat_key).copied().unwrap_or(0.0)
                + medium_category_weight * medium_category_probs.get(stat_key).copied().unwrap_or(0.0)
                + similarity_weight * similarity_probs.get(stat_key).copied().unwrap_or(0.0),
        );
    }
    normalize_probability_map(&mut probs);

    for stat_key in stat_keys {
        let mut signals = state_signals.remove(stat_key).unwrap_or_default();
        signals.extend(short_local_signals.get(stat_key).cloned().unwrap_or_default());
        signals.extend(short_category_signals.get(stat_key).cloned().unwrap_or_default());
        signals.extend(medium_category_signals.get(stat_key).cloned().unwrap_or_default());
        signals.extend(similarity_signals.get(stat_key).cloned().unwrap_or_default());
        if summary.regime_stage != "stable" {
            signals.push(format!(
                "机制 {} {:.2}",
                summary.regime_stage,
                summary.regime_shift_score
            ));
        }
        signals.sort();
        signals.dedup();
        if !signals.is_empty() {
            let display_name = stat_display_name(display_map, stat_key);
            labels.insert(
                stat_key.clone(),
                vec![format!("状态上下文 → {} [{}]", display_name, signals.join(" · "))],
            );
            sources.insert(stat_key.clone(), vec!["state_context".to_string()]);
            state_signals.insert(stat_key.clone(), signals);
        }
    }

    let active = short_local_active
        || short_category_active
        || medium_category_active
        || similarity_active
        || summary.zone_confidence >= 0.35
        || summary.crit_signal != "none"
        || summary
            .reversion_score_by_stat
            .values()
            .any(|score| *score > 0.04);
    (probs, active, labels, sources, state_signals, summary)
}

fn enrich_v3_components(
    mut components: V2ComponentBuild,
    events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
    display_map: Option<&HashMap<String, String>>,
) -> V2ComponentBuild {
    let (state_context_probs, state_context_active, labels, sources, state_signals, summary) =
        build_state_context_expert(events, stat_keys, &components.base_probs, display_map);
    for (stat_key, entries) in labels {
        components
            .matched_patterns_map
            .entry(stat_key)
            .or_default()
            .extend(entries.into_iter());
    }
    for (stat_key, entries) in sources {
        components
            .matched_experts_map
            .entry(stat_key)
            .or_default()
            .extend(entries.into_iter());
    }
    components.state_context_probs = state_context_probs;
    components.state_context_active = state_context_active;
    components.state_signal_map = state_signals;
    components.state_summary = summary.clone();
    components.bucket.active_stat_bucket = summary.active_stat_bucket.clone();
    components.bucket.tier_signal_bucket = summary.tier_signal.clone();
    components
}

fn build_tier_distribution_for_stat(
    events: &[crate::pattern_state::PatternEventLite],
    stat_key: &str,
    tier_signal: &str,
) -> (
    HashMap<i64, f64>,
    Vec<crate::domain::types::TierSuggestion>,
    Option<i64>,
    f64,
) {
    let tiers = (1..=8).collect::<Vec<_>>();
    let occurrences = events
        .iter()
        .filter(|event| event.stat_key == stat_key)
        .collect::<Vec<_>>();
    let mut tier_base = tiers
        .iter()
        .map(|tier| (*tier, 0.0))
        .collect::<HashMap<_, _>>();
    for event in &occurrences {
        if let Some(value) = tier_base.get_mut(&event.tier_index) {
            *value += 1.0;
        }
    }
    for value in tier_base.values_mut() {
        *value += 0.5;
    }
    normalize_tier_probability_map(&mut tier_base);

    let mut transition_counts = HashMap::<i64, HashMap<i64, f64>>::new();
    for pair in occurrences.windows(2) {
        *transition_counts
            .entry(pair[0].tier_index)
            .or_default()
            .entry(pair[1].tier_index)
            .or_insert(0.0) += 1.0;
    }
    let last_tier = occurrences.last().map(|event| event.tier_index);
    let mut tier_transition = last_tier
        .and_then(|tier| transition_counts.get(&tier).cloned())
        .unwrap_or_default();
    let transition_total = tier_transition.values().sum::<f64>();
    let transition_enabled = transition_total >= 3.0;
    if transition_enabled {
        for tier in &tiers {
            tier_transition.entry(*tier).or_insert(0.25);
        }
        normalize_tier_probability_map(&mut tier_transition);
    }

    let mut tier_state_prior = tiers
        .iter()
        .map(|tier| (*tier, 0.15))
        .collect::<HashMap<_, _>>();
    if let Some(last_tier) = last_tier {
        match tier_signal {
            "stable" => {
                *tier_state_prior.entry(last_tier).or_insert(0.0) += 1.2;
            }
            "step" => {
                if last_tier > 1 {
                    *tier_state_prior.entry(last_tier - 1).or_insert(0.0) += 1.0;
                }
                if last_tier < 8 {
                    *tier_state_prior.entry(last_tier + 1).or_insert(0.0) += 1.0;
                }
                *tier_state_prior.entry(last_tier).or_insert(0.0) += 0.4;
            }
            _ => {
                for tier in &tiers {
                    if (*tier - last_tier).abs() >= 2 {
                        *tier_state_prior.entry(*tier).or_insert(0.0) += 0.8;
                    }
                }
                *tier_state_prior.entry(1).or_insert(0.0) += 0.4;
                *tier_state_prior.entry(8).or_insert(0.0) += 0.4;
            }
        }
    }
    normalize_tier_probability_map(&mut tier_state_prior);

    let mut tier_probs = tiers
        .iter()
        .map(|tier| (*tier, 0.0))
        .collect::<HashMap<_, _>>();
    let mut base_weight: f64 = 0.45;
    let mut transition_weight: f64 = if transition_enabled { 0.40 } else { 0.0 };
    let mut state_weight: f64 = if last_tier.is_some() { 0.15 } else { 0.0 };
    let total_weight = (base_weight + transition_weight + state_weight).max(1e-9_f64);
    base_weight /= total_weight;
    transition_weight /= total_weight;
    state_weight /= total_weight;
    for tier in &tiers {
        tier_probs.insert(
            *tier,
            base_weight * tier_base.get(tier).copied().unwrap_or(0.0)
                + transition_weight * tier_transition.get(tier).copied().unwrap_or(0.0)
                + state_weight * tier_state_prior.get(tier).copied().unwrap_or(0.0),
        );
    }
    normalize_tier_probability_map(&mut tier_probs);

    let mut tier_suggestions = tier_probs
        .iter()
        .map(|(tier_index, probability)| crate::domain::types::TierSuggestion {
            tier_index: *tier_index,
            probability: *probability,
        })
        .collect::<Vec<_>>();
    tier_suggestions.sort_by(|a, b| {
        b.probability
            .partial_cmp(&a.probability)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.tier_index.cmp(&b.tier_index))
    });
    tier_suggestions.truncate(3);
    let best = tier_suggestions.first().cloned();
    (
        tier_probs,
        tier_suggestions,
        best.as_ref().map(|row| row.tier_index),
        best.as_ref().map(|row| row.probability).unwrap_or(0.0),
    )
}

fn build_joint_predictions(
    events: &[crate::pattern_state::PatternEventLite],
    stat_keys: &[String],
    stat_probs: &HashMap<String, f64>,
    state_summary: &crate::pattern_state::SequenceStateFeatures,
) -> JointPredictionBundle {
    let mut tier_suggestions = HashMap::new();
    let mut tier_probs = HashMap::new();
    let mut best_tier_index = HashMap::new();
    let mut best_tier_probability = HashMap::new();
    let mut joint_probability = HashMap::new();
    let mut ranking = Vec::new();

    for stat_key in stat_keys {
        let (tier_prob_map, suggestions, best_tier, best_prob) =
            build_tier_distribution_for_stat(events, stat_key, &state_summary.tier_signal);
        let stat_probability = stat_probs.get(stat_key).copied().unwrap_or(0.0);
        let joint = stat_probability * best_prob;
        tier_suggestions.insert(stat_key.clone(), suggestions);
        tier_probs.insert(stat_key.clone(), tier_prob_map);
        best_tier_index.insert(stat_key.clone(), best_tier);
        best_tier_probability.insert(stat_key.clone(), best_prob);
        joint_probability.insert(stat_key.clone(), joint);
        ranking.push((stat_key.clone(), best_tier, joint));
    }
    ranking.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal));

    JointPredictionBundle {
        tier_suggestions,
        tier_probs,
        best_tier_index,
        best_tier_probability,
        joint_probability,
        ranking,
    }
}

fn run_walk_forward_backtest_v3(
    conn: &Connection,
    stat_keys: &[String],
    config: &BacktestConfig,
    bucket_min_samples: usize,
) -> Result<TrainedAdaptiveModel, String> {
    let sample_limit = get_setting_i64(conn, "pattern_backtest_samples", 96).clamp(24, 320) as usize;
    let history_limit = sample_limit + config.analysis_window.max(32) + 128;
    let history = load_recent_global_backtest_events(conn, history_limit)?;
    if history.len() < 12 {
        return Ok(TrainedAdaptiveModel {
            fallback_weights: default_v3_weights(),
            global_weights: default_v3_weights(),
            bucket_weights: HashMap::new(),
            global_expert_regrets: HashMap::new(),
            bucket_expert_regrets: HashMap::new(),
            global_expert_duels: HashMap::new(),
            bucket_expert_duels: HashMap::new(),
            global_objective: V2BlendObjective::Calibrated,
            bucket_objectives: HashMap::new(),
            global_bucket_champion: BucketChampionRoute::Blend,
            bucket_champions: HashMap::new(),
            bucket_champion_candidates: Vec::new(),
            champion_routing_enabled: false,
            bucket_min_samples,
            backtest_summary: empty_backtest_summary(),
        });
    }

    let start_idx = history.len().saturating_sub(sample_limit).max(8);
    let mut global_tracker = V3ExpertTracker::default();
    let mut bucket_trackers = HashMap::<String, V3ExpertTracker>::new();
    let mut used_stats_by_echo = HashMap::<String, HashSet<String>>::new();

    let mut top1_hits: f64 = 0.0;
    let mut top3_hits: f64 = 0.0;
    let mut true_prob_sum: f64 = 0.0;
    let mut log_loss_sum: f64 = 0.0;
    let mut joint_top1_hits: f64 = 0.0;
    let mut joint_top3_hits: f64 = 0.0;
    let mut true_joint_prob_sum: f64 = 0.0;
    let mut joint_log_loss_sum: f64 = 0.0;
    let mut sample_count: f64 = 0.0;
    let mut benchmark_samples = Vec::<BacktestSample>::new();

    for idx in 0..history.len() {
        let actual = &history[idx];
        let blocked_stats = used_stats_by_echo
            .get(&actual.echo_id)
            .cloned()
            .unwrap_or_default();
        let has_full_echo_state = blocked_stats.len() as i64 == actual.slot_no.saturating_sub(1);

        if idx < start_idx || idx < 8 || !has_full_echo_state || blocked_stats.contains(&actual.stat_key)
        {
            used_stats_by_echo
                .entry(actual.echo_id.clone())
                .or_default()
                .insert(actual.stat_key.clone());
            continue;
        }

        let prefix_events = history[..idx]
            .iter()
            .map(|event| crate::pattern_state::PatternEventLite {
                stat_key: event.stat_key.clone(),
                tier_index: event.tier_index,
                analysis_seq: event.analysis_seq,
            })
            .collect::<Vec<_>>();
        let prefix_seq = prefix_events
            .iter()
            .map(|event| event.stat_key.clone())
            .collect::<Vec<_>>();
        let components = enrich_v3_components(
            build_v2_components(&prefix_seq, stat_keys, config, None),
            &prefix_events,
            stat_keys,
            None,
        );
        benchmark_samples.push(BacktestSample {
            components: components.clone(),
            actual_stat_key: actual.stat_key.clone(),
            blocked_stats: blocked_stats.clone(),
        });
        let bucket_key = components.bucket.key();
        let tracker_weights = bucket_trackers
            .get(&bucket_key)
            .filter(|tracker| tracker.sample_count >= bucket_min_samples)
            .map(|tracker| tracker.to_weights())
            .or_else(|| {
                if global_tracker.sample_count >= bucket_min_samples {
                    Some(global_tracker.to_weights())
                } else {
                    None
                }
            })
            .unwrap_or_else(default_v3_weights);
        let resolved_weights = cap_base_weight(resolve_active_v2_weights(tracker_weights, &components), &components);
        let mut stat_probs = blend_v2_probs(stat_keys, &components, resolved_weights);
        apply_blocked_stat_mask(&mut stat_probs, &blocked_stats);
        let joint_bundle =
            build_joint_predictions(&prefix_events, stat_keys, &stat_probs, &components.state_summary);

        let actual_stat_prob = stat_probs
            .get(&actual.stat_key)
            .copied()
            .unwrap_or(1e-9)
            .clamp(1e-9, 1.0);
        let actual_joint_prob = actual_stat_prob
            * joint_bundle
                .tier_probs
                .get(&actual.stat_key)
                .and_then(|probs| probs.get(&actual.tier_index))
                .copied()
                .unwrap_or(1e-9)
                .clamp(1e-9, 1.0);
        let mut stat_ranking = stat_probs.iter().collect::<Vec<_>>();
        stat_ranking.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(Ordering::Equal));
        if stat_ranking.first().map(|(key, _)| key.as_str()) == Some(actual.stat_key.as_str()) {
            top1_hits += 1.0;
        }
        if stat_ranking
            .iter()
            .take(3)
            .any(|(key, _)| key.as_str() == actual.stat_key.as_str())
        {
            top3_hits += 1.0;
        }

        if joint_bundle
            .ranking
            .first()
            .map(|(stat_key, tier_index, _)| {
                stat_key == &actual.stat_key && *tier_index == Some(actual.tier_index)
            })
            .unwrap_or(false)
        {
            joint_top1_hits += 1.0;
        }
        if joint_bundle.ranking.iter().take(3).any(|(stat_key, tier_index, _)| {
            stat_key == &actual.stat_key && *tier_index == Some(actual.tier_index)
        }) {
            joint_top3_hits += 1.0;
        }
        true_prob_sum += actual_stat_prob;
        log_loss_sum += -actual_stat_prob.ln();
        true_joint_prob_sum += actual_joint_prob;
        joint_log_loss_sum += -actual_joint_prob.ln();
        sample_count += 1.0;

        global_tracker.update(&components, &actual.stat_key, &blocked_stats);
        bucket_trackers
            .entry(bucket_key)
            .or_default()
            .update(&components, &actual.stat_key, &blocked_stats);
        used_stats_by_echo
            .entry(actual.echo_id.clone())
            .or_default()
            .insert(actual.stat_key.clone());
    }

    if sample_count <= 0.0 {
        return Ok(TrainedAdaptiveModel {
            fallback_weights: default_v3_weights(),
            global_weights: default_v3_weights(),
            bucket_weights: HashMap::new(),
            global_expert_regrets: HashMap::new(),
            bucket_expert_regrets: HashMap::new(),
            global_expert_duels: HashMap::new(),
            bucket_expert_duels: HashMap::new(),
            global_objective: V2BlendObjective::Calibrated,
            bucket_objectives: HashMap::new(),
            global_bucket_champion: BucketChampionRoute::Blend,
            bucket_champions: HashMap::new(),
            bucket_champion_candidates: Vec::new(),
            champion_routing_enabled: false,
            bucket_min_samples,
            backtest_summary: empty_backtest_summary(),
        });
    }

    let bucket_weights = bucket_trackers
        .into_iter()
        .map(|(key, tracker)| (key, (tracker.to_weights(), tracker.sample_count)))
        .collect::<HashMap<_, _>>();
    let model_metrics = finalize_flat_metrics(
        sample_count,
        top1_hits,
        top3_hits,
        true_prob_sum,
        log_loss_sum,
    );
    let freq_baseline = evaluate_frequency_baseline(&benchmark_samples);
    let random_baseline = evaluate_random_baseline(&benchmark_samples);
    let benchmark_rows = build_benchmark_rows(&benchmark_samples, model_metrics, true);

    Ok(TrainedAdaptiveModel {
        fallback_weights: default_v3_weights(),
        global_weights: if global_tracker.sample_count > 0 {
            global_tracker.to_weights()
        } else {
            default_v3_weights()
        },
        bucket_weights,
        global_expert_regrets: HashMap::new(),
        bucket_expert_regrets: HashMap::new(),
        global_expert_duels: HashMap::new(),
        bucket_expert_duels: HashMap::new(),
        global_objective: V2BlendObjective::Calibrated,
        bucket_objectives: HashMap::new(),
        global_bucket_champion: BucketChampionRoute::Blend,
        bucket_champions: HashMap::new(),
        bucket_champion_candidates: Vec::new(),
        champion_routing_enabled: false,
        bucket_min_samples,
        backtest_summary: PatternBacktestSummary {
            sample_count: sample_count as i64,
            top1_accuracy: model_metrics.top1_accuracy,
            top3_coverage: model_metrics.top3_coverage,
            mean_true_prob: model_metrics.mean_true_prob,
            mean_log_loss: model_metrics.mean_log_loss,
            freq_top1_accuracy: freq_baseline.top1_accuracy,
            freq_top3_coverage: freq_baseline.top3_coverage,
            freq_mean_true_prob: freq_baseline.mean_true_prob,
            freq_mean_log_loss: freq_baseline.mean_log_loss,
            random_top1_accuracy: random_baseline.top1_accuracy,
            random_top3_coverage: random_baseline.top3_coverage,
            random_mean_true_prob: random_baseline.mean_true_prob,
            random_mean_log_loss: random_baseline.mean_log_loss,
            joint_top1_accuracy: (joint_top1_hits / sample_count).clamp(0.0, 1.0),
            joint_top3_coverage: (joint_top3_hits / sample_count).clamp(0.0, 1.0),
            mean_true_joint_prob: (true_joint_prob_sum / sample_count).clamp(0.0, 1.0),
            mean_joint_log_loss: (joint_log_loss_sum / sample_count).max(0.0),
            benchmarks: benchmark_rows,
        },
    })
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
    let history_limit = (config.analysis_window + backtest_samples + 128).max(192);
    let history = load_recent_global_backtest_events(conn, history_limit)?;
    let warmup = config.min_len.max(8).max(config.max_order + 2);
    if history.len() <= warmup + 1 {
        return Ok(TrainedAdaptiveModel {
            fallback_weights,
            global_weights: fallback_weights,
            bucket_weights: HashMap::new(),
            global_expert_regrets: HashMap::new(),
            bucket_expert_regrets: HashMap::new(),
            global_expert_duels: HashMap::new(),
            bucket_expert_duels: HashMap::new(),
            global_objective: V2BlendObjective::Calibrated,
            bucket_objectives: HashMap::new(),
            global_bucket_champion: BucketChampionRoute::Blend,
            bucket_champions: HashMap::new(),
            bucket_champion_candidates: Vec::new(),
            champion_routing_enabled: false,
            bucket_min_samples,
            backtest_summary: empty_backtest_summary(),
        });
    }

    let start_idx = history.len().saturating_sub(backtest_samples).max(warmup);
    let mut used_stats_by_echo = HashMap::<String, HashSet<String>>::new();
    let mut samples = Vec::<BacktestSample>::new();
    for idx in 0..history.len() {
        let actual = &history[idx];
        let blocked_stats = used_stats_by_echo
            .get(&actual.echo_id)
            .cloned()
            .unwrap_or_default();
        let has_full_echo_state = blocked_stats.len() as i64 == actual.slot_no.saturating_sub(1);

        if idx < start_idx
            || idx < warmup
            || !has_full_echo_state
            || blocked_stats.contains(&actual.stat_key)
        {
            used_stats_by_echo
                .entry(actual.echo_id.clone())
                .or_default()
                .insert(actual.stat_key.clone());
            continue;
        }

        let prefix_start = idx.saturating_sub(config.analysis_window);
        let prefix = history[prefix_start..idx]
            .iter()
            .map(|event| event.stat_key.clone())
            .collect::<Vec<_>>();
        if prefix.len() < warmup {
            used_stats_by_echo
                .entry(actual.echo_id.clone())
                .or_default()
                .insert(actual.stat_key.clone());
            continue;
        }
        samples.push(BacktestSample {
            components: build_v2_components(&prefix, stat_keys, config, None),
            actual_stat_key: actual.stat_key.clone(),
            blocked_stats: blocked_stats.clone(),
        });
        used_stats_by_echo
            .entry(actual.echo_id.clone())
            .or_default()
            .insert(actual.stat_key.clone());
    }

    if samples.len() < 12 {
        return Ok(TrainedAdaptiveModel {
            fallback_weights,
            global_weights: fallback_weights,
            bucket_weights: HashMap::new(),
            global_expert_regrets: HashMap::new(),
            bucket_expert_regrets: HashMap::new(),
            global_expert_duels: HashMap::new(),
            bucket_expert_duels: HashMap::new(),
            global_objective: V2BlendObjective::Calibrated,
            bucket_objectives: HashMap::new(),
            global_bucket_champion: BucketChampionRoute::Blend,
            bucket_champions: HashMap::new(),
            bucket_champion_candidates: Vec::new(),
            champion_routing_enabled: false,
            bucket_min_samples,
            backtest_summary: empty_backtest_summary(),
        });
    }

    let calibrated_global_weights =
        fit_v2_weights_for_samples(&samples, baseline_blend, V2BlendObjective::Calibrated);
    let top1_global_weights =
        fit_v2_weights_for_samples(&samples, baseline_blend, V2BlendObjective::Top1);
    let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
    let mut grouped = HashMap::<String, Vec<BacktestSample>>::new();
    for sample in &samples {
        for scope in BucketScope::training_scopes() {
            grouped
                .entry(sample.components.bucket.scoped_key(scope))
                .or_default()
                .push(sample.clone());
        }
    }
    let global_expert_regrets = build_expert_regret_table(&samples, fit_min_samples);
    let bucket_expert_regrets = grouped
        .iter()
        .map(|(bucket_key, bucket_samples)| {
            (
                bucket_key.clone(),
                build_expert_regret_table(bucket_samples, fit_min_samples),
            )
        })
        .filter(|(_, table)| !table.is_empty())
        .collect::<HashMap<_, _>>();
    let global_expert_duels = build_expert_duel_table(&samples, fit_min_samples);
    let bucket_expert_duels = grouped
        .iter()
        .map(|(bucket_key, bucket_samples)| {
            (
                bucket_key.clone(),
                build_expert_duel_table(bucket_samples, fit_min_samples),
            )
        })
        .filter(|(_, table)| !table.is_empty())
        .collect::<HashMap<_, _>>();
    let mut calibrated_bucket_weights = HashMap::<String, (InternalBlendWeights, usize)>::new();
    let mut top1_bucket_weights = HashMap::<String, (InternalBlendWeights, usize)>::new();
    let mut selected_bucket_weights = HashMap::<String, (InternalBlendWeights, usize)>::new();
    let mut selected_bucket_objectives = HashMap::<String, (V2BlendObjective, usize)>::new();
    for (bucket_key, bucket_samples) in grouped.iter() {
        if bucket_samples.len() < fit_min_samples {
            continue;
        }
        let calibrated_bucket_weight = fit_v2_weights_for_samples(
            bucket_samples,
            baseline_blend,
            V2BlendObjective::Calibrated,
        );
        let top1_bucket_weight =
            fit_v2_weights_for_samples(bucket_samples, baseline_blend, V2BlendObjective::Top1);
        let selected_bucket_objective = V2BlendObjective::Calibrated;
        let selected_bucket_weight = calibrated_bucket_weight;
        calibrated_bucket_weights.insert(
            bucket_key.clone(),
            (
                calibrated_bucket_weight,
                bucket_samples.len(),
            ),
        );
        top1_bucket_weights.insert(
            bucket_key.clone(),
            (
                top1_bucket_weight,
                bucket_samples.len(),
            ),
        );
        selected_bucket_weights.insert(bucket_key.clone(), (selected_bucket_weight, bucket_samples.len()));
        selected_bucket_objectives.insert(
            bucket_key.clone(),
            (selected_bucket_objective, bucket_samples.len()),
        );
    }

    let calibrated_strategy_metrics = evaluate_v2_model_samples(
        &samples,
        stat_keys,
        fallback_weights,
        calibrated_global_weights,
        &calibrated_bucket_weights,
        &global_expert_regrets,
        &bucket_expert_regrets,
        &global_expert_duels,
        &bucket_expert_duels,
        bucket_min_samples,
    );
    let top1_strategy_metrics = evaluate_v2_model_samples(
        &samples,
        stat_keys,
        fallback_weights,
        top1_global_weights,
        &top1_bucket_weights,
        &global_expert_regrets,
        &bucket_expert_regrets,
        &global_expert_duels,
        &bucket_expert_duels,
        bucket_min_samples,
    );
    let global_objective =
        choose_v2_objective(calibrated_strategy_metrics, top1_strategy_metrics);
    let global_weights = match global_objective {
        V2BlendObjective::Calibrated => calibrated_global_weights,
        V2BlendObjective::Top1 => top1_global_weights,
    };
    let model_metrics = evaluate_v2_model_samples(
        &samples,
        stat_keys,
        fallback_weights,
        global_weights,
        &selected_bucket_weights,
        &global_expert_regrets,
        &bucket_expert_regrets,
        &global_expert_duels,
        &bucket_expert_duels,
        bucket_min_samples,
    );
    let global_bucket_champion = choose_bucket_champion_route(
        model_metrics,
        evaluate_v2_constant_champion_route_samples(
            &samples,
            stat_keys,
            fallback_weights,
            global_weights,
            &selected_bucket_weights,
            &global_expert_regrets,
            &bucket_expert_regrets,
            &global_expert_duels,
            &bucket_expert_duels,
            bucket_min_samples,
            BucketChampionRoute::PreferMarkov,
        ),
        evaluate_v2_constant_champion_route_samples(
            &samples,
            stat_keys,
            fallback_weights,
            global_weights,
            &selected_bucket_weights,
            &global_expert_regrets,
            &bucket_expert_regrets,
            &global_expert_duels,
            &bucket_expert_duels,
            bucket_min_samples,
            BucketChampionRoute::PreferAutoCycle,
        ),
        samples.len(),
        fit_min_samples,
    );
    let mut bucket_champions = HashMap::<String, (BucketChampionRoute, usize)>::new();
    let mut bucket_champion_candidates = Vec::<BucketChampionCandidateSummary>::new();
    for (bucket_key, bucket_samples) in grouped.iter() {
        if bucket_samples.len() < fit_min_samples {
            continue;
        }
        let blend_bucket_metrics = evaluate_v2_model_samples(
            bucket_samples,
            stat_keys,
            fallback_weights,
            global_weights,
            &selected_bucket_weights,
            &global_expert_regrets,
            &bucket_expert_regrets,
            &global_expert_duels,
            &bucket_expert_duels,
            bucket_min_samples,
        );
        let markov_bucket_metrics = evaluate_v2_constant_champion_route_samples(
            bucket_samples,
            stat_keys,
            fallback_weights,
            global_weights,
            &selected_bucket_weights,
            &global_expert_regrets,
            &bucket_expert_regrets,
            &global_expert_duels,
            &bucket_expert_duels,
            bucket_min_samples,
            BucketChampionRoute::PreferMarkov,
        );
        let auto_cycle_bucket_metrics = evaluate_v2_constant_champion_route_samples(
            bucket_samples,
            stat_keys,
            fallback_weights,
            global_weights,
            &selected_bucket_weights,
            &global_expert_regrets,
            &bucket_expert_regrets,
            &global_expert_duels,
            &bucket_expert_duels,
            bucket_min_samples,
            BucketChampionRoute::PreferAutoCycle,
        );
        let mut candidates = collect_bucket_champion_candidates(
            blend_bucket_metrics,
            markov_bucket_metrics,
            auto_cycle_bucket_metrics,
            bucket_samples.len(),
            fit_min_samples,
        );
        for candidate in &mut candidates {
            candidate.bucket_key = bucket_key.clone();
        }
        if let Some(winner) = candidates.iter().find(|candidate| candidate.qualifies) {
            bucket_champions.insert(bucket_key.clone(), (winner.route, bucket_samples.len()));
        }
        bucket_champion_candidates.extend(candidates);
    }
    bucket_champion_candidates.sort_by(compare_bucket_champion_candidates);
    bucket_champion_candidates.truncate(24);
    let champion_strategy_metrics = evaluate_v2_bucket_champion_model_samples(
        &samples,
        stat_keys,
        fallback_weights,
        global_weights,
        &selected_bucket_weights,
        &global_expert_regrets,
        &bucket_expert_regrets,
        &global_expert_duels,
        &bucket_expert_duels,
        global_bucket_champion,
        &bucket_champions,
        bucket_min_samples,
    );
    let champion_routing_enabled =
        should_enable_bucket_champion_routing(model_metrics, champion_strategy_metrics);
    let deployed_metrics = if champion_routing_enabled {
        champion_strategy_metrics
    } else {
        model_metrics
    };

    let temp_model = TrainedAdaptiveModel {
        fallback_weights,
        global_weights,
        bucket_weights: selected_bucket_weights.clone(),
        global_expert_regrets: global_expert_regrets.clone(),
        bucket_expert_regrets: bucket_expert_regrets.clone(),
        global_expert_duels: global_expert_duels.clone(),
        bucket_expert_duels: bucket_expert_duels.clone(),
        global_objective,
        bucket_objectives: selected_bucket_objectives.clone(),
        global_bucket_champion,
        bucket_champions: bucket_champions.clone(),
        bucket_champion_candidates: bucket_champion_candidates.clone(),
        champion_routing_enabled,
        bucket_min_samples,
        backtest_summary: empty_backtest_summary(),
    };
    let freq_baseline = evaluate_frequency_baseline(&samples);
    let random_baseline = evaluate_random_baseline(&samples);
    let mut benchmark_rows = build_benchmark_rows(&samples, deployed_metrics, true);
    if let Some(current_row) = benchmark_rows.first_mut() {
        current_row.key = if champion_routing_enabled {
            "model_bucket_champion".to_string()
        } else {
            "model_bucket_calibrated".to_string()
        };
        current_row.label = if champion_routing_enabled {
            "当前模型 (按桶冠军切路)".to_string()
        } else {
            "当前模型 (bucket 校准优先)".to_string()
        };
    }
    let mut insert_idx = 1usize;
    if !bucket_champions.is_empty() || global_bucket_champion != BucketChampionRoute::Blend {
        let alternate_row = if champion_routing_enabled {
            flat_metrics_to_benchmark_row("model_bucket_blend", "模型 (按桶混合)", model_metrics)
        } else {
            flat_metrics_to_benchmark_row(
                "model_bucket_champion",
                "模型 (按桶冠军切路)",
                champion_strategy_metrics,
            )
        };
        benchmark_rows.insert(insert_idx, alternate_row);
        insert_idx += 1;
    }
    benchmark_rows.insert(
        insert_idx,
        flat_metrics_to_benchmark_row(
            "model_all_top1",
            "模型 (全桶Top1优先)",
            top1_strategy_metrics,
        ),
    );
    benchmark_rows.insert(
        insert_idx + 1,
        flat_metrics_to_benchmark_row(
            "model_all_calibrated",
            "模型 (全桶校准优先)",
            calibrated_strategy_metrics,
        ),
    );
    Ok(TrainedAdaptiveModel {
        fallback_weights,
        global_weights: temp_model.global_weights,
        bucket_weights: temp_model.bucket_weights,
        global_expert_regrets: temp_model.global_expert_regrets,
        bucket_expert_regrets: temp_model.bucket_expert_regrets,
        global_expert_duels: temp_model.global_expert_duels,
        bucket_expert_duels: temp_model.bucket_expert_duels,
        global_objective: temp_model.global_objective,
        bucket_objectives: temp_model.bucket_objectives,
        global_bucket_champion: temp_model.global_bucket_champion,
        bucket_champions: temp_model.bucket_champions,
        bucket_champion_candidates: temp_model.bucket_champion_candidates,
        champion_routing_enabled: temp_model.champion_routing_enabled,
        bucket_min_samples,
        backtest_summary: PatternBacktestSummary {
            sample_count: samples.len() as i64,
            top1_accuracy: deployed_metrics.top1_accuracy,
            top3_coverage: deployed_metrics.top3_coverage,
            mean_true_prob: deployed_metrics.mean_true_prob,
            mean_log_loss: deployed_metrics.mean_log_loss,
            freq_top1_accuracy: freq_baseline.top1_accuracy,
            freq_top3_coverage: freq_baseline.top3_coverage,
            freq_mean_true_prob: freq_baseline.mean_true_prob,
            freq_mean_log_loss: freq_baseline.mean_log_loss,
            random_top1_accuracy: random_baseline.top1_accuracy,
            random_top3_coverage: random_baseline.top3_coverage,
            random_mean_true_prob: random_baseline.mean_true_prob,
            random_mean_log_loss: random_baseline.mean_log_loss,
            joint_top1_accuracy: deployed_metrics.top1_accuracy,
            joint_top3_coverage: deployed_metrics.top3_coverage,
            mean_true_joint_prob: deployed_metrics.mean_true_prob,
            mean_joint_log_loss: deployed_metrics.mean_log_loss,
            benchmarks: benchmark_rows,
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
        ("state_context".to_string(), components.state_context_probs.clone()),
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
    for expert in [
        "base",
        "markov",
        "exact_motif",
        "approx_shape",
        "auto_cycle",
        "state_context",
    ] {
        scores.insert(expert.to_string(), 0.0);
    }
    let mut initialized = false;
    for (context, actual_stat_key) in matched {
        for expert in [
            "base",
            "markov",
            "exact_motif",
            "approx_shape",
            "auto_cycle",
            "state_context",
        ] {
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
        ("state_context", adjusted.state_context),
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
    adjusted.state_context *= factor("state_context");
    normalize_internal_weights(&mut adjusted);
    Ok((adjusted, true))
}

fn persist_pattern_prediction_run(
    conn: &Connection,
    mode: &str,
    game_day: &str,
    seq: &[String],
    weights: &PatternBlendWeights,
    components: &V2ComponentBuild,
    active_experts: &[String],
    suggestions: &[AdaptiveNextSuggestion],
    state_summary: Option<&crate::domain::types::PatternStateSummary>,
) -> Result<(), String> {
    if get_setting_i64(conn, "pattern_online_learning", 1) <= 0 {
        return Ok(());
    }

    let tail_signature = build_tail_signature(seq);
    let context_hash = format!(
        "{}|{}|{}|{}|0",
        game_day,
        seq.len(),
        tail_signature,
        mode
    );
    let context = StoredPredictionContext {
        bucket: components.bucket.clone(),
        expert_probs: expert_prob_blob(components),
        weight_source: weights.source.clone(),
        active_experts: active_experts.to_vec(),
        tail_signature,
        state_summary: state_summary.cloned(),
    };
    let predictions = suggestions
        .iter()
        .map(|row| StoredPredictionRow {
            stat_key: row.stat_key.clone(),
            probability: row.probability,
            best_tier_index: row.best_tier_index,
            best_tier_probability: row.best_tier_probability,
            joint_probability: row.joint_probability,
            tier_suggestions: row.tier_suggestions.clone(),
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
            mode,
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
    let deployment_mode = match get_setting_string(conn, "pattern_model_mode", "adaptive_v2").as_str() {
        "baseline_v1" => "baseline_v1".to_string(),
        "adaptive_v2" => "adaptive_v2".to_string(),
        "adaptive_v3_shadow" => "adaptive_v3_shadow".to_string(),
        "adaptive_v3" => "adaptive_v3".to_string(),
        "v2_shadow" => "v2_shadow".to_string(),
        "v2_canary" => "v2_canary".to_string(),
        _ => "adaptive_v2".to_string(),
    };
    let v3_enabled = matches!(deployment_mode.as_str(), "adaptive_v3" | "adaptive_v3_shadow");
    let bucket_min_samples = get_setting_i64(conn, "pattern_bucket_min_samples", 12).clamp(4, 64)
        as usize;
    let mut notes = vec![
        "当前模型按全局序列建模，并叠加日内类别上下文与相似窗口检索；不区分 Cost/主词条/状态。"
            .to_string(),
    ];

    let min_len = filter.min_len.unwrap_or(4).clamp(2, 12) as usize;
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
    let echo_blocked_stats = if let Some(echo_id) = filter
        .echo_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(load_echo_blocked_stats(conn, echo_id)?)
    } else {
        None
    };
    if let Some(blocked_stats) = echo_blocked_stats.as_ref() {
        notes.push(format!(
            "当前建议已按所选声骸做无放回过滤：屏蔽 {} 个已有词条，候选剩余 {} 个。",
            blocked_stats.len(),
            stat_keys.len().saturating_sub(blocked_stats.len())
        ));
    }
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
    let v3_model = if v3_enabled {
        Some(run_walk_forward_backtest_v3(
            conn,
            &stat_keys,
            &backtest_config,
            bucket_min_samples,
        )?)
    } else {
        None
    };
    if v2_model.backtest_summary.sample_count > 0 {
        let global_weights = v2_model.global_weights;
        notes.push(format!(
            "V2 回测: samples={} · top1={:.1}% · top3={:.1}% · meanP={:.1}% · logloss={:.3} · global={} · B/M/X/A/C = {:.0}/{:.0}/{:.0}/{:.0}/{:.0}",
            v2_model.backtest_summary.sample_count,
            v2_model.backtest_summary.top1_accuracy * 100.0,
            v2_model.backtest_summary.top3_coverage * 100.0,
            v2_model.backtest_summary.mean_true_prob * 100.0,
            v2_model.backtest_summary.mean_log_loss,
            v2_model.global_objective.short_label(),
            global_weights.base * 100.0,
            global_weights.markov * 100.0,
            global_weights.exact_motif * 100.0,
            global_weights.approx_shape * 100.0,
            global_weights.auto_cycle * 100.0,
        ));
        let fit_min_samples = bucket_fit_min_samples(bucket_min_samples);
        let calibrated_bucket_count = v2_model
            .bucket_objectives
            .values()
            .filter(|(objective, sample_count)| {
                *objective == V2BlendObjective::Calibrated
                    && *sample_count >= fit_min_samples
            })
            .count();
        let exact_calibrated_bucket_count = v2_model
            .bucket_objectives
            .iter()
            .filter(|(key, (objective, sample_count))| {
                !key.contains(':')
                    && *objective == V2BlendObjective::Calibrated
                    && *sample_count >= fit_min_samples
            })
            .count();
        let parent_bucket_count = v2_model
            .bucket_objectives
            .keys()
            .filter(|key| key.contains(':'))
            .count();
        if !v2_model.bucket_objectives.is_empty() {
            notes.push(format!(
                "V2 bucket 权重采用校准优先：5D 校准桶 {} 个 · 父桶策略 {} 组（有效桶 {}）",
                exact_calibrated_bucket_count,
                parent_bucket_count,
                calibrated_bucket_count
            ));
            notes.push("V2 已启用分层桶回退：5D/4D/3D 桶会按样本量收缩混合，再回落到全局权重。".to_string());
            notes.push("V2 已启用低信号桶专家负反馈：会额外压低短 Markov 与弱 Motif 在噪声上下文里的权重。".to_string());
            notes.push("V2 已启用按桶 expert regret table：会按 5D/4D/3D 桶记录专家相对基线的后悔值，并在实时选权重后做二次缩放。".to_string());
            notes.push("V2 已启用 bucket 内专家对打 gating：优先学习 auto_cycle vs markov、motif vs base 在具体桶里谁该让位。".to_string());
            let duel_rule_count = v2_model
                .bucket_expert_duels
                .values()
                .map(|table| table.len())
                .sum::<usize>();
            if duel_rule_count > 0 || !v2_model.global_expert_duels.is_empty() {
                notes.push(format!(
                    "V2 duel 规则：global {} 条 · bucket {} 条（覆盖 {} 桶）",
                    v2_model.global_expert_duels.len(),
                    duel_rule_count,
                    v2_model.bucket_expert_duels.len()
                ));
            }
            let champion_rule_count = v2_model.bucket_champions.len();
            let champion_markov_count = v2_model
                .bucket_champions
                .values()
                .filter(|(route, _)| *route == BucketChampionRoute::PreferMarkov)
                .count();
            let champion_cycle_count = v2_model
                .bucket_champions
                .values()
                .filter(|(route, _)| *route == BucketChampionRoute::PreferAutoCycle)
                .count();
            notes.push(format!(
                "V2 champion 路由：global {} · bucket {} 条（Markov {} / AutoCycle {}）",
                v2_model.global_bucket_champion.short_label(),
                champion_rule_count,
                champion_markov_count,
                champion_cycle_count
            ));
            if champion_rule_count == 0 && v2_model.global_bucket_champion == BucketChampionRoute::Blend {
                notes.push("V2 champion 切路框架已接入，但当前样本还没学出值得接管的桶，所以继续使用混合器主路由。".to_string());
            } else if v2_model.champion_routing_enabled {
                notes.push("V2 已启用按桶 champion 切路：当具体桶里单专家比混合器更稳时，会直接让 Markov / AutoCycle 接管该桶预测。".to_string());
            } else {
                notes.push("V2 已训练按桶 champion 切路，但整体回测还没有超过当前混合器，所以暂不接管主输出。".to_string());
            }
            let champion_preview = if champion_rule_count > 0 {
                v2_model
                    .bucket_champion_candidates
                    .iter()
                    .filter(|candidate| candidate.qualifies)
                    .take(3)
                    .map(format_bucket_champion_candidate)
                    .collect::<Vec<_>>()
            } else {
                let mut near_miss_candidates = v2_model
                    .bucket_champion_candidates
                    .iter()
                    .filter(|candidate| bucket_champion_candidate_has_signal(candidate))
                    .collect::<Vec<_>>();
                near_miss_candidates.sort_by(|a, b| compare_bucket_champion_candidates(a, b));
                if near_miss_candidates.is_empty() {
                    v2_model
                        .bucket_champion_candidates
                        .iter()
                        .take(1)
                        .map(format_bucket_champion_candidate)
                        .collect::<Vec<_>>()
                } else {
                    near_miss_candidates
                        .into_iter()
                        .take(3)
                        .map(format_bucket_champion_candidate)
                        .collect::<Vec<_>>()
                }
            };
            if !champion_preview.is_empty() {
                notes.push(format!(
                    "V2 champion 候选细节：{}",
                    champion_preview.join(" || ")
                ));
            }
        }
    } else {
        notes.push("V2 历史样本不足，当前使用默认融合权重。".to_string());
    }
    notes.push("回测口径已切换为单声骸开孔任务：每一步都会屏蔽该声骸已有词条后再评估命中率与 logloss。".to_string());
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
            state_summary: None,
            shadow_comparison: None,
            interval_signals: Vec::new(),
            active_experts: Vec::new(),
            exact_patterns: Vec::new(),
            shape_patterns: Vec::new(),
            suggestions: Vec::new(),
            manual_summary: None,
            notes,
        });
    }

    let mut day_events = load_day_event_sequence(&conn, &game_day)?;
    let raw_seq_len = day_events.len();
    if day_events.len() > analysis_window {
        notes.push(format!(
            "已按 analysis_window={analysis_window} 截断为尾部序列（原始长度 {raw_seq_len}）。"
        ));
        let start = day_events.len() - analysis_window;
        day_events = day_events[start..].to_vec();
    }
    let seq = day_events
        .iter()
        .map(|event| event.stat_key.clone())
        .collect::<Vec<_>>();

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
            state_summary: None,
            shadow_comparison: None,
            interval_signals: Vec::new(),
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
    let (bucket_objective, objective_source) =
        select_v2_objective_for_bucket(&v2_model, &v2_components.bucket);
    let (selected_weights_regret, regret_tags) =
        apply_regret_table_to_weights(selected_weights_raw, &v2_model, &v2_components);
    let (selected_weights_duel, duel_tags) =
        apply_duel_table_to_weights_raw(
            selected_weights_regret,
            &v2_model.global_expert_duels,
            &v2_model.bucket_expert_duels,
            v2_model.bucket_min_samples,
            &v2_components,
        );
    let (selected_weights_online, online_adjusted) =
        apply_online_adjustment(conn, &v2_components.bucket, selected_weights_duel, &v2_components)?;
    let selected_weights =
        cap_base_weight(resolve_active_v2_weights(selected_weights_online, &v2_components), &v2_components);
    let (bucket_scope, bucket_sample_count, bucket_min_samples, bucket_trust) =
        resolve_bucket_reliability(&v2_model, &v2_components.bucket);
    let mut blend_weights = internal_weights_to_public(
        selected_weights,
        weight_source,
        &v2_components.bucket,
        bucket_scope,
        bucket_sample_count,
        bucket_min_samples,
        bucket_trust,
        online_adjusted,
    );
    let mut active_experts = active_v2_experts(&v2_components, &selected_weights);
    let (bucket_champion_route, champion_source) =
        select_bucket_champion_route(&v2_model, &v2_components.bucket);
    notes.push(format!(
        "当前上下文桶 depth={} / markov={} / motif={} / active={} / ctx={} · 权重源 {}{}",
        blend_weights.sample_depth_bucket,
        blend_weights.markov_hit_bucket,
        blend_weights.motif_hit_bucket,
        blend_weights.active_stat_bucket,
        blend_weights.tier_signal_bucket,
        blend_weights.source,
        if blend_weights.online_adjusted { " · online+" } else { "" }
    ));
    notes.push(format!(
        "当前 bucket 目标：{} ({})",
        bucket_objective.short_label(),
        objective_source,
    ));
    if !regret_tags.is_empty() {
        notes.push(format!("当前 regret 惩罚：{}", regret_tags.join(" · ")));
    }
    if !duel_tags.is_empty() {
        notes.push(format!("当前 expert 对打：{}", duel_tags.join(" · ")));
    }
    let empty_blocked_stats = HashSet::<String>::new();
    let runtime_blocked_stats = echo_blocked_stats
        .as_ref()
        .unwrap_or(&empty_blocked_stats);
    let mut adaptive_stat_probs = blend_v2_probs(&stat_keys, &v2_components, selected_weights);
    apply_blocked_stat_mask(&mut adaptive_stat_probs, runtime_blocked_stats);
    if v2_model.champion_routing_enabled {
        let (routed_probs, applied_route) = apply_bucket_champion_route_probs(
            &v2_components,
            adaptive_stat_probs,
            runtime_blocked_stats,
            bucket_champion_route,
        );
        adaptive_stat_probs = routed_probs;
        if applied_route != BucketChampionRoute::Blend {
            notes.push(format!(
                "当前 champion 路由：{} ({})",
                applied_route.short_label(),
                champion_source
            ));
            blend_weights.source =
                format!("{}+champion:{}", blend_weights.source, applied_route.source_token());
            active_experts = vec![applied_route.source_token().to_string()];
        } else if bucket_champion_route != BucketChampionRoute::Blend {
            notes.push(format!(
                "当前 champion 候选：{} ({})，但本次信号未激活，回退混合器。",
                bucket_champion_route.short_label(),
                champion_source
            ));
        }
    }
    if cycle_weight > 0.0 {
        for stat_key in &stat_keys {
            let adaptive_prob = adaptive_stat_probs.get(stat_key).copied().unwrap_or(0.0);
            let cycle_prob = cycle_probs.get(stat_key).copied().unwrap_or(adaptive_prob);
            adaptive_stat_probs.insert(
                stat_key.clone(),
                ((1.0 - cycle_weight) * adaptive_prob + cycle_weight * cycle_prob).max(0.0),
            );
        }
        apply_blocked_stat_mask(&mut adaptive_stat_probs, runtime_blocked_stats);
    }

    let mut suggestions = Vec::<AdaptiveNextSuggestion>::new();
    let mut total_score = 0.0;
    let mut adaptive_auto_predictions = Vec::<(String, f64)>::new();
    let mut adaptive_auto_total = 0.0;
    let interval_total = n.max(1) as i64;
    for stat_key in &stat_keys {
        let base = *base_probs.get(stat_key).unwrap_or(&0.0);
        let markov = *markov_probs.get(stat_key).unwrap_or(&base);
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
        let adaptive_score = adaptive_stat_probs.get(stat_key).copied().unwrap_or(0.0);
        let baseline_auto =
            ((1.0 - baseline_markov_mix) * base + baseline_markov_mix * markov)
                * (1.0 + motif_lambda * norm_boost);
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
            joint_probability: score,
            base_probability: base,
            markov_probability: markov,
            cycle_probability: cycle,
            motif_boost: norm_boost,
            best_tier_index: None,
            best_tier_probability: 0.0,
            tier_suggestions: Vec::new(),
            confidence,
            probability_ci_low: ci_low,
            probability_ci_high: ci_high,
            matched_patterns: matched,
            matched_experts,
            state_matched_signals: Vec::new(),
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
    if let Some(blocked_stats) = echo_blocked_stats.as_ref() {
        apply_blocked_stats_to_suggestions(&mut suggestions, blocked_stats, top_k);
    } else {
        suggestions.truncate(top_k);
    }

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
    let contextual_prediction_mode =
        manual_cycle_len.is_some() || !manual_guess_shapes.is_empty() || echo_blocked_stats.is_some();
    let mut report_backtest_summary = v2_model.backtest_summary.clone();
    let mut report_blend_weights = blend_weights.clone();
    let mut report_active_experts = active_experts.clone();
    let mut report_suggestions = suggestions.clone();
    let mut report_model_confidence = model_confidence;
    let mut report_state_summary: Option<crate::domain::types::PatternStateSummary> = None;
    let mut report_shadow_comparison: Option<crate::domain::types::PatternShadowComparison> = None;

    if let Some(v3_model) = &v3_model {
        let v3_components = enrich_v3_components(
            v2_components.clone(),
            &day_events,
            &stat_keys,
            Some(&display_map),
        );
        let (v3_weights_raw, v3_weight_source) =
            select_v2_weights_for_bucket(v3_model, &v3_components.bucket);
        let (v3_weights_regret, _) =
            apply_regret_table_to_weights(v3_weights_raw, v3_model, &v3_components);
        let (v3_weights_duel, _) = apply_duel_table_to_weights_raw(
            v3_weights_regret,
            &v3_model.global_expert_duels,
            &v3_model.bucket_expert_duels,
            v3_model.bucket_min_samples,
            &v3_components,
        );
        let (v3_weights_online, v3_online_adjusted) =
            apply_online_adjustment(conn, &v3_components.bucket, v3_weights_duel, &v3_components)?;
        let v3_selected_weights = cap_base_weight(
            resolve_active_v2_weights(v3_weights_online, &v3_components),
            &v3_components,
        );
        let (v3_bucket_scope, v3_bucket_sample_count, v3_bucket_min_samples, v3_bucket_trust) =
            resolve_bucket_reliability(v3_model, &v3_components.bucket);
        let v3_blend_weights = internal_weights_to_public(
            v3_selected_weights,
            v3_weight_source,
            &v3_components.bucket,
            v3_bucket_scope,
            v3_bucket_sample_count,
            v3_bucket_min_samples,
            v3_bucket_trust,
            v3_online_adjusted,
        );
        let v3_active_experts = active_v2_experts(&v3_components, &v3_selected_weights);
        let mut v3_stat_probs = blend_v2_probs(&stat_keys, &v3_components, v3_selected_weights);
        if cycle_weight > 0.0 {
            for stat_key in &stat_keys {
                let auto_prob = v3_stat_probs.get(stat_key).copied().unwrap_or(0.0);
                let cycle_prob = cycle_probs.get(stat_key).copied().unwrap_or(auto_prob);
                v3_stat_probs.insert(
                    stat_key.clone(),
                    (1.0 - cycle_weight) * auto_prob + cycle_weight * cycle_prob,
                );
            }
            normalize_probability_map(&mut v3_stat_probs);
        }
        if let Some(blocked_stats) = echo_blocked_stats.as_ref() {
            apply_blocked_stat_mask(&mut v3_stat_probs, blocked_stats);
        }
        let joint_bundle =
            build_joint_predictions(&day_events, &stat_keys, &v3_stat_probs, &v3_components.state_summary);
        let mut v3_matched_patterns_map = v3_components.matched_patterns_map.clone();
        let mut v3_matched_experts_map = v3_components.matched_experts_map.clone();
        if let Some(manual) = &manual_summary {
            for row in &manual.position_suggestions {
                push_label(
                    &mut v3_matched_patterns_map,
                    &row.stat_key,
                    format!("手动周期 next@pos {:.0}% (n={})", row.probability * 100.0, row.count),
                );
                push_source(&mut v3_matched_experts_map, &row.stat_key, "manual_cycle");
            }
        }

        let mut v3_suggestions = Vec::<AdaptiveNextSuggestion>::new();
        for stat_key in &stat_keys {
            let stat_probability = v3_stat_probs.get(stat_key).copied().unwrap_or(0.0);
            let pseudo_success = ((stat_probability * interval_total as f64).round())
                .clamp(0.0, interval_total as f64) as i64;
            let (ci_low, ci_high) = wilson_interval(pseudo_success, interval_total, confidence_level);
            let ci_width = ci_high - ci_low;
            let (base_ci_low, base_ci_high) = base_ci_map.get(stat_key).copied().unwrap_or((0.0, 0.0));
            let base_uncertainty = (base_ci_high - base_ci_low).clamp(0.0, 1.0);
            let confidence = (1.0 - (ci_width * 0.8 + base_uncertainty * 0.2)).clamp(0.0, 1.0);

            let mut matched = v3_matched_patterns_map.remove(stat_key).unwrap_or_default();
            matched.sort();
            matched.dedup();
            matched.truncate(5);
            let mut matched_experts = v3_matched_experts_map.remove(stat_key).unwrap_or_default();
            matched_experts.sort();
            matched_experts.dedup();
            let mut state_signals = v3_components
                .state_signal_map
                .get(stat_key)
                .cloned()
                .unwrap_or_default();
            state_signals.sort();
            state_signals.dedup();

            let cycle_probability = if cycle_weight > 0.0 {
                *cycle_probs.get(stat_key).unwrap_or(&0.0)
            } else {
                *v2_components.auto_cycle_probs.get(stat_key).unwrap_or(&0.0)
            };

            v3_suggestions.push(AdaptiveNextSuggestion {
                stat_key: stat_key.clone(),
                display_name: display_map
                    .get(stat_key)
                    .cloned()
                    .unwrap_or_else(|| stat_key.clone()),
                probability: stat_probability,
                joint_probability: joint_bundle
                    .joint_probability
                    .get(stat_key)
                    .copied()
                    .unwrap_or(0.0),
                base_probability: *base_probs.get(stat_key).unwrap_or(&0.0),
                markov_probability: *markov_probs.get(stat_key).unwrap_or(&0.0),
                cycle_probability,
                motif_boost: *v3_components
                    .state_context_probs
                    .get(stat_key)
                    .unwrap_or(&0.0),
                best_tier_index: joint_bundle
                    .best_tier_index
                    .get(stat_key)
                    .copied()
                    .flatten(),
                best_tier_probability: joint_bundle
                    .best_tier_probability
                    .get(stat_key)
                    .copied()
                    .unwrap_or(0.0),
                tier_suggestions: joint_bundle
                    .tier_suggestions
                    .get(stat_key)
                    .cloned()
                    .unwrap_or_default(),
                confidence,
                probability_ci_low: ci_low,
                probability_ci_high: ci_high,
                matched_patterns: matched,
                matched_experts,
                state_matched_signals: state_signals,
            });
        }
        v3_suggestions.sort_by(|a, b| {
            b.joint_probability
                .partial_cmp(&a.joint_probability)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    b.probability
                        .partial_cmp(&a.probability)
                        .unwrap_or(Ordering::Equal)
                })
        });
        if let Some(blocked_stats) = echo_blocked_stats.as_ref() {
            apply_blocked_stats_to_suggestions(&mut v3_suggestions, blocked_stats, top_k);
        } else {
            v3_suggestions.truncate(top_k);
        }

        let v3_sample_conf = (n as f64 / (n as f64 + 20.0)).min(1.0);
        let v3_logloss_conf =
            (1.0 - (v3_model.backtest_summary.mean_joint_log_loss / 1.6)).clamp(0.0, 1.0);
        let v3_backtest_conf = ((v3_model.backtest_summary.joint_top1_accuracy * 0.45
            + v3_model.backtest_summary.joint_top3_coverage * 0.20
            + v3_model.backtest_summary.top1_accuracy * 0.15
            + v3_logloss_conf * 0.20)
            * (v3_model.backtest_summary.sample_count as f64
                / (v3_model.backtest_summary.sample_count as f64 + 24.0)))
            .clamp(0.0, 1.0);
        let v3_model_confidence = (0.20 * v3_sample_conf
            + 0.15 * v3_blend_weights.markov
            + 0.10 * v3_blend_weights.auto_cycle
            + 0.15 * v3_blend_weights.state_context
            + 0.40 * v3_backtest_conf)
            .clamp(0.0, 1.0);

        report_state_summary = Some(public_state_summary(&v3_components.state_summary));
        report_shadow_comparison = Some(crate::domain::types::PatternShadowComparison {
            primary_model_mode: "adaptive_v2".to_string(),
            shadow_model_mode: "adaptive_v3".to_string(),
            primary_top1_accuracy: v2_model.backtest_summary.top1_accuracy,
            shadow_top1_accuracy: v3_model.backtest_summary.top1_accuracy,
            primary_joint_top1_accuracy: v2_model.backtest_summary.joint_top1_accuracy,
            shadow_joint_top1_accuracy: v3_model.backtest_summary.joint_top1_accuracy,
            primary_mean_log_loss: v2_model.backtest_summary.mean_log_loss,
            shadow_mean_log_loss: v3_model.backtest_summary.mean_log_loss,
            primary_mean_joint_log_loss: v2_model.backtest_summary.mean_joint_log_loss,
            shadow_mean_joint_log_loss: v3_model.backtest_summary.mean_joint_log_loss,
        });

        if deployment_mode == "adaptive_v3" {
            report_backtest_summary = v3_model.backtest_summary.clone();
            report_blend_weights = v3_blend_weights.clone();
            report_active_experts = v3_active_experts.clone();
            report_suggestions = v3_suggestions.clone();
            report_model_confidence = v3_model_confidence;
            notes.push("当前 pattern_model_mode=adaptive_v3，建议按词条+档位联合概率排序。".to_string());
        } else {
            notes.push("当前 pattern_model_mode=adaptive_v3_shadow；界面仍显示 V2 建议，V3 仅做影子评估与日志。".to_string());
        }

        if !contextual_prediction_mode {
            persist_pattern_prediction_run(
                conn,
                deployment_mode.as_str(),
                &game_day,
                &seq,
                &v3_blend_weights,
                &v3_components,
                &v3_active_experts,
                &v3_suggestions,
                report_state_summary.as_ref(),
            )?;
        }
    }

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
    if report_backtest_summary.sample_count > 0 {
        if let Some(current_row) = report_backtest_summary.benchmarks.first() {
            if current_row.key == "model_bucket_calibrated" {
                notes.push(
                    "V2 当前采用 bucket 校准优先；Top1 优先作为对照继续并行回测。"
                        .to_string(),
                );
            }
        }
        if report_backtest_summary.top1_accuracy + 1e-9
            < report_backtest_summary.random_top1_accuracy
        {
            notes.push(format!(
                "当前回测 Top1 {:.2}% 仍低于随机均匀 {:.2}%，先别急着堆更多特征，优先压缩错误专家权重并校准概率输出。",
                report_backtest_summary.top1_accuracy * 100.0,
                report_backtest_summary.random_top1_accuracy * 100.0,
            ));
        }
        if let Some(best_benchmark) = report_backtest_summary
            .benchmarks
            .iter()
            .filter(|row| row.key != "model_bucket_calibrated")
            .min_by(|a, b| {
                a.mean_log_loss
                    .partial_cmp(&b.mean_log_loss)
                    .unwrap_or(Ordering::Equal)
            })
        {
            if best_benchmark.mean_log_loss + 1e-9 < report_backtest_summary.mean_log_loss {
                notes.push(format!(
                    "当前最稳的对照是 {}（LogLoss {:.3}），已经优于当前模型 {:.3}。",
                    best_benchmark.label,
                    best_benchmark.mean_log_loss,
                    report_backtest_summary.mean_log_loss,
                ));
            }
        }
    }

    let interval_signals = build_interval_signals(conn, &day_events, &stat_keys)?;

    if !contextual_prediction_mode && !v3_enabled {
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
                        joint_probability: normalized,
                        base_probability: *base_probs.get(&stat_key).unwrap_or(&0.0),
                        markov_probability: *markov_probs.get(&stat_key).unwrap_or(&0.0),
                        cycle_probability: *v2_components.auto_cycle_probs.get(&stat_key).unwrap_or(&0.0),
                        motif_boost: *raw_boost_map.get(&stat_key).unwrap_or(&0.0),
                        best_tier_index: None,
                        best_tier_probability: 0.0,
                        tier_suggestions: Vec::new(),
                        confidence: 0.0,
                        probability_ci_low: 0.0,
                        probability_ci_high: 0.0,
                        matched_patterns: Vec::new(),
                        matched_experts: Vec::new(),
                        state_matched_signals: Vec::new(),
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
            "adaptive_v2",
            &game_day,
            &seq,
            &blend_weights,
            &v2_components,
            &active_experts,
            &logging_suggestions,
            None,
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
        model_confidence: report_model_confidence,
        blend_weights: report_blend_weights,
        backtest_summary: report_backtest_summary,
        state_summary: report_state_summary,
        shadow_comparison: report_shadow_comparison,
        interval_signals,
        active_experts: report_active_experts,
        exact_patterns,
        shape_patterns,
        suggestions: report_suggestions,
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
    actual_tier_index: i64,
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
                 actual_tier_index = ?4,
                 top1_hit = ?5,
                 top3_hit = ?6,
                 log_loss = ?7,
                 resolved_at = ?8
             WHERE run_id = ?1",
            params![
                run_id,
                actual_stat_key,
                actual_event_id,
                actual_tier_index,
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
             actual_tier_index = NULL,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_database;
    use rusqlite::{params, Connection};
    use std::fs;
    use std::path::PathBuf;

    const TEST_GAME_DAY: &str = "2026-05-26";

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "wuwa-echo-sight-{name}-{}.sqlite3",
            Uuid::new_v4()
        ))
    }

    fn set_setting(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO app_settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .expect("setting should be written");
    }

    fn seed_repeating_opening_sequence(conn: &mut Connection, echo_count: usize) {
        let cycle = [
            ("crit_rate", 1_i64),
            ("crit_dmg", 1_i64),
            ("energy_regen", 1_i64),
            ("atk_pct", 1_i64),
            ("hp_pct", 1_i64),
        ];
        let tx = conn.transaction().expect("transaction should start");
        let mut seq = 0_i64;

        for echo_idx in 0..echo_count {
            let echo_id = format!("metric-test-echo-{echo_idx:03}");
            tx.execute(
                "INSERT INTO echoes(
                    echo_id, nickname, main_stat_key, cost_class, status,
                    opened_slots_count, created_at, updated_at
                 ) VALUES (?1, ?2, 'atk_pct', 1, 'tracking', 5, ?3, ?3)",
                params![&echo_id, format!("metric echo {echo_idx}"), "2026-05-26T12:00:00+08:00"],
            )
            .expect("echo should insert");

            for (slot_idx, &(stat_key, tier_index)) in cycle.iter().enumerate() {
                seq += 1;
                let value_scaled = tx
                    .query_row(
                        "SELECT value_scaled FROM stat_tiers WHERE stat_key = ?1 AND tier_index = ?2",
                        params![stat_key, tier_index],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("tier value should exist");
                tx.execute(
                    "INSERT INTO ordered_events(
                        event_id, echo_id, slot_no, stat_key, tier_index, value_scaled,
                        event_time, created_seq, analysis_seq, game_day, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?7)",
                    params![
                        format!("metric-test-event-{seq:04}"),
                        &echo_id,
                        slot_idx as i64 + 1,
                        stat_key,
                        tier_index,
                        value_scaled,
                        "2026-05-26T12:00:00+08:00",
                        seq,
                        TEST_GAME_DAY,
                    ],
                )
                .expect("event should insert");
            }
        }

        tx.commit().expect("seed transaction should commit");
    }

    fn assert_unit_metric(name: &str, value: f64) {
        assert!(value.is_finite(), "{name} should be finite, got {value}");
        assert!((0.0..=1.0).contains(&value), "{name} should be in [0, 1], got {value}");
    }

    #[test]
    fn prediction_metrics_are_validated_from_seeded_backend_events() {
        let db_path = temp_db_path("prediction-metrics");
        init_database(&db_path).expect("database should initialize");
        let mut conn = Connection::open(&db_path).expect("database should open");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys should enable");
        set_setting(&conn, "analysis_window", "400");
        set_setting(&conn, "pattern_backtest_samples", "192");
        set_setting(&conn, "pattern_bucket_min_samples", "4");
        set_setting(&conn, "pattern_model_mode", "adaptive_v3_shadow");
        seed_repeating_opening_sequence(&mut conn, 80);

        let report = get_daily_pattern_decision_internal(
            &conn,
            &DailyPatternDecisionFilter {
                game_day: Some(TEST_GAME_DAY.to_string()),
                min_len: Some(2),
                max_len: Some(6),
                min_support: Some(2),
                max_order: Some(5),
                top_k: Some(10),
                ..DailyPatternDecisionFilter::default()
            },
        )
        .expect("decision report should be computed from backend data");

        let metrics = &report.backtest_summary;
        println!(
            "prediction metrics: samples={} top1={:.3} top3={:.3} logloss={:.3} joint_top1={:.3} joint_top3={:.3} joint_logloss={:.3} random_top1={:.3} random_logloss={:.3}",
            metrics.sample_count,
            metrics.top1_accuracy,
            metrics.top3_coverage,
            metrics.mean_log_loss,
            metrics.joint_top1_accuracy,
            metrics.joint_top3_coverage,
            metrics.mean_joint_log_loss,
            metrics.random_top1_accuracy,
            metrics.random_mean_log_loss,
        );
        assert_eq!(report.total_events, 400);
        assert!(metrics.sample_count >= 120, "expected many backend backtest samples, got {}", metrics.sample_count);
        assert_unit_metric("top1_accuracy", metrics.top1_accuracy);
        assert_unit_metric("top3_coverage", metrics.top3_coverage);
        assert_unit_metric("joint_top1_accuracy", metrics.joint_top1_accuracy);
        assert_unit_metric("joint_top3_coverage", metrics.joint_top3_coverage);
        assert!(metrics.mean_log_loss.is_finite(), "mean_log_loss should be finite");
        assert!(metrics.mean_joint_log_loss.is_finite(), "mean_joint_log_loss should be finite");
        assert!(metrics.top1_accuracy > metrics.random_top1_accuracy + 0.25);
        assert!(metrics.mean_log_loss < metrics.random_mean_log_loss);
        assert!(metrics.top1_accuracy >= 0.80, "expected deterministic sequence Top1 accuracy, got {:.3}", metrics.top1_accuracy);
        assert!(metrics.joint_top3_coverage >= metrics.joint_top1_accuracy);
        assert!(!metrics.benchmarks.is_empty(), "benchmark rows should be produced");

        let shadow = report
            .shadow_comparison
            .as_ref()
            .expect("adaptive_v3_shadow should produce backend shadow metrics");
        assert_unit_metric("shadow_top1_accuracy", shadow.shadow_top1_accuracy);
        assert_unit_metric("shadow_joint_top1_accuracy", shadow.shadow_joint_top1_accuracy);
        assert!(shadow.shadow_mean_log_loss.is_finite());
        assert!(shadow.shadow_mean_joint_log_loss.is_finite());
        assert!((shadow.primary_top1_accuracy - metrics.top1_accuracy).abs() < 1e-9);
        assert!((shadow.primary_mean_log_loss - metrics.mean_log_loss).abs() < 1e-9);
        assert!(report.blend_weights.bucket_sample_count >= report.blend_weights.bucket_min_samples);
        assert!(report.blend_weights.bucket_trust > 0.0);
        assert!(!report.suggestions.is_empty(), "backend suggestions should be produced");

        drop(conn);
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(db_path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(db_path.with_extension("sqlite3-shm"));
    }

    #[test]
    #[ignore]
    fn local_prediction_metrics_report() {
        let db_path = std::env::var("WUWA_ECHO_SIGHT_DB")
            .expect("set WUWA_ECHO_SIGHT_DB to a copied local sqlite database path");
        let conn = Connection::open(&db_path).expect("local metrics database should open");
        set_setting(&conn, "pattern_model_mode", "adaptive_v3_shadow");
        set_setting(&conn, "pattern_backtest_samples", "192");

        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ordered_events", [], |row| row.get(0))
            .expect("event count should query");
        let day_range: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT MIN(game_day), MAX(game_day) FROM ordered_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("day range should query");
        let mut best_report = None;
        let mut best_score = f64::NEG_INFINITY;
        for min_len in [2_i64, 3, 4] {
            for max_order in [2_i64, 3, 5, 8] {
                let report = get_daily_pattern_decision_internal(
                    &conn,
                    &DailyPatternDecisionFilter {
                        min_len: Some(min_len),
                        max_len: Some(7),
                        min_support: Some(2),
                        max_order: Some(max_order),
                        top_k: Some(10),
                        ..DailyPatternDecisionFilter::default()
                    },
                )
                .expect("local decision report should compute");
                let metrics = &report.backtest_summary;
                let score = metrics.top1_accuracy
                    + 0.25 * metrics.top3_coverage
                    - 0.05 * metrics.mean_log_loss;
                println!(
                    "grid min_len={min_len} max_order={max_order} samples={} top1={:.4} top3={:.4} logloss={:.4} lift_freq_top1={:+.4} lift_freq_top3={:+.4}",
                    metrics.sample_count,
                    metrics.top1_accuracy,
                    metrics.top3_coverage,
                    metrics.mean_log_loss,
                    metrics.top1_accuracy - metrics.freq_top1_accuracy,
                    metrics.top3_coverage - metrics.freq_top3_coverage,
                );
                if score > best_score {
                    best_score = score;
                    best_report = Some(report);
                }
            }
        }
        let report = best_report.expect("at least one local report should compute");
        let metrics = &report.backtest_summary;
        let shadow = report
            .shadow_comparison
            .as_ref()
            .expect("adaptive_v3_shadow should produce local shadow metrics");

        println!("local events={event_count} days={:?}->{:?} best_report_day={} day_events={}", day_range.0, day_range.1, report.game_day, report.total_events);
        println!("local interval_signals={}", report.interval_signals.len());
        for signal in &report.interval_signals {
            println!(
                "interval {} target={} n={} base={:.4} obs={:.4} lift={:+.4} confidence={:.4} direction={}",
                signal.label,
                signal.target,
                signal.sample_count,
                signal.baseline_rate,
                signal.observed_rate,
                signal.lift,
                signal.confidence,
                signal.direction,
            );
        }
        println!(
            "local V2: samples={} top1={:.4} top3={:.4} meanP={:.4} logloss={:.4} joint_top1={:.4} joint_top3={:.4} joint_meanP={:.4} joint_logloss={:.4}",
            metrics.sample_count,
            metrics.top1_accuracy,
            metrics.top3_coverage,
            metrics.mean_true_prob,
            metrics.mean_log_loss,
            metrics.joint_top1_accuracy,
            metrics.joint_top3_coverage,
            metrics.mean_true_joint_prob,
            metrics.mean_joint_log_loss,
        );
        println!(
            "local baselines: freq_top1={:.4} freq_top3={:.4} freq_logloss={:.4} random_top1={:.4} random_top3={:.4} random_logloss={:.4}",
            metrics.freq_top1_accuracy,
            metrics.freq_top3_coverage,
            metrics.freq_mean_log_loss,
            metrics.random_top1_accuracy,
            metrics.random_top3_coverage,
            metrics.random_mean_log_loss,
        );
        println!(
            "local V3 shadow: top1={:.4} top3_delta=n/a logloss={:.4} joint_top1={:.4} joint_logloss={:.4}",
            shadow.shadow_top1_accuracy,
            shadow.shadow_mean_log_loss,
            shadow.shadow_joint_top1_accuracy,
            shadow.shadow_mean_joint_log_loss,
        );
        println!(
            "local lift: V2_top1_vs_freq={:+.4} V2_top1_vs_random={:+.4} V2_logloss_vs_freq={:+.4} V2_logloss_vs_random={:+.4} V3_top1_vs_V2={:+.4} V3_logloss_vs_V2={:+.4} V3_joint_top1_vs_V2={:+.4} V3_joint_logloss_vs_V2={:+.4}",
            metrics.top1_accuracy - metrics.freq_top1_accuracy,
            metrics.top1_accuracy - metrics.random_top1_accuracy,
            metrics.mean_log_loss - metrics.freq_mean_log_loss,
            metrics.mean_log_loss - metrics.random_mean_log_loss,
            shadow.shadow_top1_accuracy - shadow.primary_top1_accuracy,
            shadow.shadow_mean_log_loss - shadow.primary_mean_log_loss,
            shadow.shadow_joint_top1_accuracy - shadow.primary_joint_top1_accuracy,
            shadow.shadow_mean_joint_log_loss - shadow.primary_mean_joint_log_loss,
        );
        for row in &metrics.benchmarks {
            println!(
                "benchmark {} ({}) top1={:.4} top3={:.4} meanP={:.4} logloss={:.4}",
                row.key,
                row.label,
                row.top1_accuracy,
                row.top3_coverage,
                row.mean_true_prob,
                row.mean_log_loss,
            );
        }
    }
}
