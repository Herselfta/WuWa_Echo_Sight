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
pub struct EchoProbFilter {
    pub stat_key: String,
    pub sort_by: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub main_stat_key: Option<String>,
    pub cost_class: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoProbRow {
    pub echo_id: String,
    pub nickname: Option<String>,
    pub main_stat_key: String,
    pub cost_class: i64,
    pub status: String,
    pub opened_slots_count: i64,
    pub expectation_rank_min: i64,
    pub p_next: f64,
    pub p_final: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProbabilitySnapshotInput {
    pub scope: DistributionFilter,
    pub stat_key: Option<String>,
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

/// Cell in the slotNo × stat distribution table
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotStatCell {
    pub slot_no: i64,
    pub stat_key: String,
    pub display_name: String,
    pub category: String,
    pub count: i64,
    pub probability: f64,
}

/// Slot-stat independence test
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotStatDistribution {
    pub stat_keys: Vec<(String, String)>,
    pub cells: Vec<SlotStatCell>,
    pub slot_totals: Vec<i64>,
    pub total_events: i64,
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
}
