use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use rusqlite::{params, params_from_iter, Connection};
use tauri::State;
use uuid::Uuid;

use crate::db::{get_setting_f64, get_setting_i64, now_rfc3339, open_connection, AppState};
use crate::domain::types::{
    CreateProbabilitySnapshotInput, CreateProbabilitySnapshotOutput, DistributionFilter,
    DistributionPayload, DistributionRow, EchoProbFilter, EchoProbRow,
};
use crate::stats::{
    bayes_interval, blended_stat_probabilities, monte_carlo_final_prob, wilson_interval,
};

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

pub fn get_echoes_for_stat_internal(
    conn: &Connection,
    filter: &EchoProbFilter,
) -> Result<Vec<EchoProbRow>, String> {
    let scope = DistributionFilter {
        start_time: filter.start_time.clone(),
        end_time: filter.end_time.clone(),
        main_stat_key: filter.main_stat_key.clone(),
        cost_class: filter.cost_class,
        status: filter.status.clone(),
    };

    let stats = list_stats(conn)?;
    let all_stat_keys: Vec<String> = stats.iter().map(|(k, _, _)| k.clone()).collect();

    let (_, count_map) = load_event_counts(conn, &scope)?;
    let smoothing_alpha = get_setting_f64(conn, "smoothing_alpha", 1.0).max(0.0001);
    let baseline_blend = get_setting_f64(conn, "baseline_blend", 0.65).clamp(0.0, 1.0);
    let mc_iterations = get_setting_i64(conn, "mc_iterations", 20000).clamp(500, 50000) as usize;

    // Use a stable baseline-first prior (uniform over enabled stats),
    // then blend observed frequencies to avoid purely drift-driven guidance.
    let weight_map =
        blended_stat_probabilities(&all_stat_keys, &count_map, baseline_blend, smoothing_alpha);

    let mut conditions = vec![
        "e.opened_slots_count < 5".to_string(),
        "NOT EXISTS (SELECT 1 FROM echo_current_substats cs WHERE cs.echo_id = e.echo_id AND cs.stat_key = ?1)"
            .to_string(),
        "e.status NOT IN ('paused', 'abandoned', 'completed')".to_string(),
    ];
    let mut params_vec: Vec<rusqlite::types::Value> = vec![filter.stat_key.clone().into()];

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

    let query = format!(
        "SELECT e.echo_id, e.nickname, e.main_stat_key, e.cost_class, e.status, e.opened_slots_count, MIN(ex.rank) AS expectation_rank_min
         FROM echoes e
         JOIN echo_expectations ex ON ex.echo_id = e.echo_id AND ex.stat_key = ?1
         WHERE {}
         GROUP BY e.echo_id, e.nickname, e.main_stat_key, e.cost_class, e.status, e.opened_slots_count",
        conditions.join(" AND ")
    );

    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("failed to prepare echoes_for_stat query: {e}"))?;
    let base_rows = stmt
        .query_map(params_from_iter(params_vec), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(|e| format!("failed to query echoes_for_stat rows: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect echoes_for_stat rows: {e}"))?;

    let mut owned_stmt = conn
        .prepare("SELECT stat_key FROM echo_current_substats WHERE echo_id = ?1")
        .map_err(|e| format!("failed to prepare owned stats query: {e}"))?;

    let mut signature_cache: HashMap<String, f64> = HashMap::new();
    let mut result = Vec::new();

    for (
        echo_id,
        nickname,
        main_stat_key,
        cost_class,
        status,
        opened_slots_count,
        expectation_rank_min,
    ) in base_rows
    {
        let owned_stats = owned_stmt
            .query_map([&echo_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("failed to query owned stats for {echo_id}: {e}"))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| format!("failed to collect owned stats for {echo_id}: {e}"))?;

        let candidates: Vec<String> = all_stat_keys
            .iter()
            .filter(|s| !owned_stats.contains(*s))
            .cloned()
            .collect();

        let p_next = if candidates.iter().any(|s| s == &filter.stat_key) {
            let target_weight = *weight_map.get(&filter.stat_key).unwrap_or(&0.0);
            let total_weight: f64 = candidates
                .iter()
                .map(|k| *weight_map.get(k).unwrap_or(&0.0))
                .sum();
            if total_weight > 0.0 {
                target_weight / total_weight
            } else {
                0.0
            }
        } else {
            0.0
        };

        let remaining_draws = 5 - opened_slots_count;
        let mut sorted_candidates = candidates.clone();
        sorted_candidates.sort();
        let signature = format!(
            "{}|{}|{}",
            sorted_candidates.join(","),
            remaining_draws,
            filter.stat_key
        );

        let p_final = if let Some(cached) = signature_cache.get(&signature) {
            *cached
        } else {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            signature.hash(&mut hasher);
            echo_id.hash(&mut hasher);
            let seed = hasher.finish();
            let value = monte_carlo_final_prob(
                &all_stat_keys,
                &owned_stats,
                &filter.stat_key,
                &weight_map,
                remaining_draws,
                mc_iterations,
                seed,
            );
            signature_cache.insert(signature.clone(), value);
            value
        };

        result.push(EchoProbRow {
            echo_id,
            nickname,
            main_stat_key,
            cost_class,
            status,
            opened_slots_count,
            expectation_rank_min,
            p_next,
            p_final,
        });
    }

    let sort_by = filter
        .sort_by
        .clone()
        .unwrap_or_else(|| "pFinal".to_string())
        .to_lowercase();

    match sort_by.as_str() {
        "pnext" => result.sort_by(|a, b| {
            b.p_next
                .partial_cmp(&a.p_next)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.p_final.partial_cmp(&a.p_final).unwrap_or(Ordering::Equal))
        }),
        "rank" => result.sort_by(|a, b| {
            a.expectation_rank_min
                .cmp(&b.expectation_rank_min)
                .then_with(|| b.p_final.partial_cmp(&a.p_final).unwrap_or(Ordering::Equal))
        }),
        "slots" => result.sort_by(|a, b| {
            a.opened_slots_count
                .cmp(&b.opened_slots_count)
                .then_with(|| b.p_final.partial_cmp(&a.p_final).unwrap_or(Ordering::Equal))
        }),
        _ => result.sort_by(|a, b| {
            b.p_final
                .partial_cmp(&a.p_final)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.p_next.partial_cmp(&a.p_next).unwrap_or(Ordering::Equal))
        }),
    }

    Ok(result)
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
pub fn get_echoes_for_stat(
    state: State<'_, AppState>,
    filter: EchoProbFilter,
) -> Result<Vec<EchoProbRow>, String> {
    let conn = open_connection(&state)?;
    get_echoes_for_stat_internal(&conn, &filter)
}

#[tauri::command]
pub fn create_probability_snapshot(
    state: State<'_, AppState>,
    input: CreateProbabilitySnapshotInput,
) -> Result<CreateProbabilitySnapshotOutput, String> {
    let conn = open_connection(&state)?;

    let distribution = get_global_distribution_internal(&conn, &input.scope)?;
    let echo_probs = if let Some(stat_key) = &input.stat_key {
        get_echoes_for_stat_internal(
            &conn,
            &EchoProbFilter {
                stat_key: stat_key.clone(),
                sort_by: Some("pFinal".to_string()),
                start_time: input.scope.start_time.clone(),
                end_time: input.scope.end_time.clone(),
                main_stat_key: input.scope.main_stat_key.clone(),
                cost_class: input.scope.cost_class,
                status: input.scope.status.clone(),
            },
        )?
    } else {
        Vec::new()
    };

    let snapshot_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO probability_snapshots(snapshot_id, created_at, scope_json, distribution_json, echo_probs_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            snapshot_id,
            now_rfc3339(),
            serde_json::to_string(&input.scope).map_err(|e| format!("failed to encode scope json: {e}"))?,
            serde_json::to_string(&distribution)
                .map_err(|e| format!("failed to encode distribution json: {e}"))?,
            serde_json::to_string(&echo_probs).map_err(|e| format!("failed to encode echo probs json: {e}"))?
        ],
    )
    .map_err(|e| format!("failed to insert probability snapshot: {e}"))?;

    Ok(CreateProbabilitySnapshotOutput { snapshot_id })
}
