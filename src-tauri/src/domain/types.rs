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
