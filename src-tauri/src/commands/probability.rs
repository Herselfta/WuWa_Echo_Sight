use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection};
use tauri::State;
use uuid::Uuid;

use crate::db::{get_setting_f64, now_rfc3339, open_connection, AppState};
use crate::domain::types::{
    CreateProbabilitySnapshotInput, CreateProbabilitySnapshotOutput, DistributionFilter,
    DistributionPayload, DistributionRow,
};
use crate::stats::{bayes_interval, wilson_interval};

fn build_distribution_where(filter: &DistributionFilter) -> (String, Vec<rusqlite::types::Value>) {
    let mut conditions: Vec<String> = vec![];
    let mut params_vec: Vec<rusqlite::types::Value> = vec![];

    if let Some(start_time) = &filter.start_time {
        conditions.push("oe.event_time >= ?".to_string());
        params_vec.push(start_time.clone().into());
    }
    if let Some(end_time) = &filter.end_time {
        conditions.push("oe.event_time <= ?".to_string());
        params_vec.push(end_time.clone().into());
    }
    if let Some(main_stat_key) = &filter.main_stat_key {
        conditions.push("e.main_stat_key = ?".to_string());
        params_vec.push(main_stat_key.clone().into());
    }
    if let Some(cost_class) = filter.cost_class {
        conditions.push("e.cost_class = ?".to_string());
        params_vec.push(cost_class.into());
    }
    if let Some(status) = &filter.status {
        conditions.push("e.status = ?".to_string());
        params_vec.push(status.clone().into());
    }

    if conditions.is_empty() {
        (String::new(), params_vec)
    } else {
        (format!("WHERE {}", conditions.join(" AND ")), params_vec)
    }
}

fn load_event_counts(
    conn: &Connection,
    filter: &DistributionFilter,
) -> Result<(i64, HashMap<String, i64>), String> {
    let (where_clause, params_vec) = build_distribution_where(filter);

    let total_query = format!(
        "SELECT COUNT(*)
         FROM ordered_events oe
         JOIN echoes e ON e.echo_id = oe.echo_id
         {}",
        where_clause
    );
    let total: i64 = conn
        .query_row(&total_query, params_from_iter(params_vec.clone()), |row| {
            row.get(0)
        })
        .map_err(|e| format!("failed to query total count: {e}"))?;

    let counts_query = format!(
        "SELECT oe.stat_key, COUNT(*)
         FROM ordered_events oe
         JOIN echoes e ON e.echo_id = oe.echo_id
         {}
         GROUP BY oe.stat_key",
        where_clause
    );
    let mut stmt = conn
        .prepare(&counts_query)
        .map_err(|e| format!("failed to prepare counts query: {e}"))?;
    let rows = stmt
        .query_map(params_from_iter(params_vec), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| format!("failed to query count rows: {e}"))?;

    let mut map = HashMap::new();
    for row in rows {
        let (k, v) = row.map_err(|e| format!("failed to read count row: {e}"))?;
        map.insert(k, v);
    }

    Ok((total, map))
}

fn list_stats(conn: &Connection) -> Result<Vec<(String, String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT stat_key, display_name, unit FROM stat_defs WHERE enabled = 1 ORDER BY rowid",
        )
        .map_err(|e| format!("failed to query stat_defs: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("failed to map stat_defs: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect stat_defs: {e}"))
}

pub fn get_global_distribution_internal(
    conn: &Connection,
    filter: &DistributionFilter,
) -> Result<DistributionPayload, String> {
    let stats = list_stats(conn)?;
    let (total_events, count_map) = load_event_counts(conn, filter)?;
    let confidence = get_setting_f64(conn, "confidence_level", 0.95).clamp(0.5, 0.999);

    let rows = stats
        .iter()
        .map(|(stat_key, display_name, unit)| {
            let count = *count_map.get(stat_key).unwrap_or(&0);
            let p_global = if total_events > 0 {
                count as f64 / total_events as f64
            } else {
                0.0
            };
            let (ci_freq_low, ci_freq_high) = wilson_interval(count, total_events, confidence);
            let (bayes_mean, bayes_low, bayes_high) =
                bayes_interval(count, total_events, confidence);

            DistributionRow {
                stat_key: stat_key.clone(),
                display_name: display_name.clone(),
                unit: unit.clone(),
                count,
                p_global,
                ci_freq_low,
                ci_freq_high,
                bayes_mean,
                bayes_low,
                bayes_high,
            }
        })
        .collect();

    Ok(DistributionPayload { total_events, rows })
}

#[tauri::command]
pub fn get_global_distribution(
    state: State<'_, AppState>,
    filter: Option<DistributionFilter>,
) -> Result<DistributionPayload, String> {
    let conn = open_connection(&state)?;
    get_global_distribution_internal(&conn, &filter.unwrap_or_default())
}

#[tauri::command]
pub fn create_probability_snapshot(
    state: State<'_, AppState>,
    input: CreateProbabilitySnapshotInput,
) -> Result<CreateProbabilitySnapshotOutput, String> {
    let conn = open_connection(&state)?;

    let distribution = get_global_distribution_internal(&conn, &input.scope)?;

    let snapshot_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO probability_snapshots(snapshot_id, created_at, scope_json, distribution_json, echo_probs_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            snapshot_id,
            now_rfc3339(),
            serde_json::to_string(&input.scope)
                .map_err(|e| format!("failed to encode scope json: {e}"))?,
            serde_json::to_string(&distribution)
                .map_err(|e| format!("failed to encode distribution json: {e}"))?,
            "[]"
        ],
    )
    .map_err(|e| format!("failed to insert probability snapshot: {e}"))?;

    Ok(CreateProbabilitySnapshotOutput { snapshot_id })
}
