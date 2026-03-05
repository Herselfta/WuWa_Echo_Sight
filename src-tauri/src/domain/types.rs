use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatTier {
    pub tier_index: i64,
    pub value_scaled: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatDef {
    pub stat_key: String,
    pub display_name: String,
    pub unit: String,
    pub tiers: Vec<StatTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectationItem {
    pub stat_key: String,
    pub rank: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectationPreset {
    pub preset_id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub items: Vec<ExpectationItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveExpectationPresetInput {
    pub preset_id: Option<String>,
    pub name: String,
    pub items: Vec<ExpectationItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveExpectationPresetOutput {
    pub preset_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteExpectationPresetInput {
    pub preset_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoSubstatSlot {
    pub slot_no: i64,
    pub stat_key: String,
    pub tier_index: i64,
    pub value_scaled: i64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEchoInput {
    pub nickname: Option<String>,
    pub main_stat_key: String,
    pub cost_class: i64,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEchoOutput {
    pub echo_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEchoInput {
    pub echo_id: String,
    pub nickname: Option<String>,
    pub main_stat_key: Option<String>,
    pub cost_class: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteEchoInput {
    pub echo_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EchoFilter {
    pub status: Option<String>,
    pub main_stat_key: Option<String>,
    pub cost_class: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoSummary {
    pub echo_id: String,
    pub nickname: Option<String>,
    pub main_stat_key: String,
    pub cost_class: i64,
    pub status: String,
    pub opened_slots_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub expectations: Vec<ExpectationItem>,
    pub current_substats: Vec<EchoSubstatSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetExpectationsInput {
    pub echo_id: String,
    pub items: Vec<ExpectationItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillSlotInput {
    pub slot_no: i64,
    pub stat_key: String,
    pub tier_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertBackfillInput {
    pub echo_id: String,
    pub slots: Vec<BackfillSlotInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleOk {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendOrderedEventInput {
    pub echo_id: String,
    pub slot_no: i64,
    pub stat_key: String,
    pub tier_index: i64,
    pub event_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendOrderedEventOutput {
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOrderedEventInput {
    pub event_id: String,
    pub slot_no: Option<i64>,
    pub stat_key: Option<String>,
    pub tier_index: Option<i64>,
    pub event_time: Option<String>,
    pub reorder_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOrderedEventOutput {
    pub ok: bool,
    pub affected_range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOrderedEventInput {
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOrderedEventOutput {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventHistoryFilter {
    pub echo_id: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub event_id: String,
    pub echo_id: String,
    pub echo_nickname: Option<String>,
    pub slot_no: i64,
    pub stat_key: String,
    pub tier_index: i64,
    pub value_scaled: i64,
    pub event_time: String,
    pub created_seq: i64,
    pub analysis_seq: i64,
    pub game_day: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DistributionFilter {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub main_stat_key: Option<String>,
    pub cost_class: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionRow {
    pub stat_key: String,
    pub display_name: String,
    pub unit: String,
    pub count: i64,
    pub p_global: f64,
    pub ci_freq_low: f64,
    pub ci_freq_high: f64,
    pub bayes_mean: f64,
    pub bayes_low: f64,
    pub bayes_high: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionPayload {
    pub total_events: i64,
    pub rows: Vec<DistributionRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProbabilitySnapshotInput {
    pub scope: DistributionFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProbabilitySnapshotOutput {
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCsvInput {
    pub scope: DistributionFilter,
    pub include_snapshots: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCsvOutput {
    pub zip_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDataInput {
    pub zip_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDataOutput {
    pub ok: bool,
    pub imported_tables: Vec<String>,
}

/* ═══════════════════════════════════════════════════════
Daily pattern decision (MVP)
═══════════════════════════════════════════════════════ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DailyPatternDecisionFilter {
    pub game_day: Option<String>,
    pub cost_class: Option<i64>,
    pub main_stat_key: Option<String>,
    pub status: Option<String>,
    pub manual_start_index: Option<i64>,
    pub manual_cycle_len: Option<i64>,
    pub manual_guess_shapes: Option<Vec<String>>,
    pub min_len: Option<i64>,
    pub max_len: Option<i64>,
    pub min_support: Option<i64>,
    pub max_order: Option<i64>,
    pub top_k: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyExactPatternRow {
    pub length: i64,
    pub pattern: Vec<String>,
    pub display_pattern: Vec<String>,
    pub shape: String,
    pub support: i64,
    pub window_count: i64,
    pub expected_count: f64,
    pub lift: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyShapePatternRow {
    pub length: i64,
    pub shape: String,
    pub support: i64,
    pub expected_count: f64,
    pub lift: f64,
    pub score: f64,
    pub example_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptiveNextSuggestion {
    pub stat_key: String,
    pub display_name: String,
    pub probability: f64,
    pub base_probability: f64,
    pub markov_probability: f64,
    pub cycle_probability: f64,
    pub motif_boost: f64,
    pub matched_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualGuessVerificationRow {
    pub guess_shape: String,
    pub length: i64,
    pub support: i64,
    pub opportunities: i64,
    pub hit_rate: f64,
    pub baseline_rate: f64,
    pub lift: f64,
    pub matched_cycle_indices: Vec<i64>,
    pub next_stat_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualCycleSuggestion {
    pub stat_key: String,
    pub display_name: String,
    pub count: i64,
    pub probability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualPatternSummary {
    pub start_index: i64,
    pub cycle_len: i64,
    pub full_cycles: i64,
    pub next_cycle_pos: i64,
    pub top_cycle_shapes: Vec<(String, i64)>,
    pub guesses: Vec<ManualGuessVerificationRow>,
    pub position_suggestions: Vec<ManualCycleSuggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPatternDecisionReport {
    pub game_day: String,
    pub total_events: i64,
    pub min_len: i64,
    pub max_len: i64,
    pub min_support: i64,
    pub max_order: i64,
    pub model_confidence: f64,
    pub exact_patterns: Vec<DailyExactPatternRow>,
    pub shape_patterns: Vec<DailyShapePatternRow>,
    pub suggestions: Vec<AdaptiveNextSuggestion>,
    pub manual_summary: Option<ManualPatternSummary>,
    pub notes: Vec<String>,
}

/* ═══════════════════════════════════════════════════════
Hypothesis verification types
═══════════════════════════════════════════════════════ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HypothesisFilter {
    pub cost_class: Option<i64>,
    pub main_stat_key: Option<String>,
    pub status: Option<String>,
}

/// Cell in the stat→stat transition matrix
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionCell {
    pub from_stat: String,
    pub to_stat: String,
    pub count: i64,
    pub expected: f64,
    pub residual: f64,
}

/// Full transition matrix with χ² test results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionMatrix {
    /// (stat_key, display_name)
    pub stat_keys: Vec<(String, String)>,
    pub cells: Vec<TransitionCell>,
    pub total_transitions: i64,
    pub chi_squared: f64,
    pub degrees_of_freedom: i64,
    pub p_value: f64,
}

/// A detected streak of same-category stats
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryStreakRow {
    pub echo_id: String,
    pub category: String,
    pub start_slot: i64,
    pub end_slot: i64,
    pub length: i64,
    pub stats: Vec<String>,
    pub tiers: Vec<i64>,
    pub possible_zones: Vec<String>,
}

/// Full streak & zone analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryStreakReport {
    pub streaks: Vec<CategoryStreakRow>,
    /// (from_zone, to_zone, count)
    pub zone_transitions: Vec<(String, String, i64)>,
    /// (zone, visit_count)
    pub zone_visits: Vec<(String, i64)>,
    pub tier_total_pairs: i64,
    pub tier_stop_count: i64,
    pub tier_step_count: i64,
    pub tier_jump_count: i64,
    pub tier_stop_ratio: f64,
    pub tier_step_ratio: f64,
    pub tier_jump_ratio: f64,
    pub tier_expected_stop_ratio: Option<f64>,
    pub tier_expected_step_ratio: Option<f64>,
    pub tier_expected_jump_ratio: Option<f64>,
}

/* ═══ Reversion analysis ═══ */

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReversionBucket {
    /// Count of this stat in the previous W events
    pub prev_window_count: i64,
    /// How many samples fell into this bucket
    pub sample_count: i64,
    /// Average occurrence rate in the NEXT W events: occurrences / W
    pub mean_next_freq: f64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatReversionSeries {
    pub stat_key: String,
    pub display_name: String,
    /// Global base frequency (total appearances / total events)
    pub base_freq: f64,
    pub total_count: i64,
    /// Cumulative frequency deviation at each global event position
    /// deviation[i] = (count_so_far / (i+1)) - base_freq
    pub deviations: Vec<f64>,
    /// Inter-arrival gaps (in events) between consecutive appearances
    pub gaps: Vec<i64>,
    pub mean_gap: f64,
    /// 1 / base_freq — expected gap under i.i.d.
    pub expected_gap: f64,
    pub gap_variance: f64,
    /// Var / Mean; Geometric(p) baseline ≈ (1-p)/p
    pub dispersion_index: f64,
    pub geometric_dispersion: f64,
    /// (lag, autocorrelation) pairs
    pub lag_autocorrs: Vec<(i64, f64)>,
    /// Conditional next-window frequency by prior window count bucket
    pub window_buckets: Vec<ReversionBucket>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReversionReport {
    pub total_events: i64,
    /// All analysis_seq values in order (x-axis for deviation chart)
    pub global_seqs: Vec<i64>,
    pub stat_series: Vec<StatReversionSeries>,
}
