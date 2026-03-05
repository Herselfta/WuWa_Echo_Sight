use std::collections::HashMap;

use rusqlite::Connection;
use tauri::State;

use crate::db::{open_connection, AppState};
use crate::domain::types::{
    CategoryStreakReport, CategoryStreakRow, HypothesisFilter, ReversionBucket, ReversionReport,
    StatReversionSeries, TransitionCell, TransitionMatrix,
};
use crate::stats::expected_tier_adjacency_ratios_for_stat;

/* ═══════════════════════════════════════════════════════
Stat categories for zone hypothesis
═══════════════════════════════════════════════════════ */

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

fn stat_category(stat_key: &str) -> &'static str {
    STAT_CATEGORIES
        .iter()
        .find(|(k, _)| *k == stat_key)
        .map(|(_, c)| *c)
        .unwrap_or("unknown")
}

/* ── Zone labels for the "区间" hypothesis ── */

const ZONE_LABELS: &[(&str, &[&str])] = &[
    ("攻防区", &["atk", "def"]),
    ("攻生区", &["atk", "hp"]),
    ("防生区", &["def", "hp"]),
    ("伤害加成区", &["dmg_bonus"]),
    ("共鸣区", &["dmg_bonus", "utility"]), // skill/liberation + energy_regen
    ("爆区", &["crit"]),
];

fn infer_zone(category: &str) -> Vec<&'static str> {
    ZONE_LABELS
        .iter()
        .filter(|(_, cats)| cats.contains(&category))
        .map(|(label, _)| *label)
        .collect()
}

/* ═══════════════════════════════════════════════════════
Helper: load ordered event sequences grouped by echo
═══════════════════════════════════════════════════════ */

struct EventSlim {
    stat_key: String,
    tier_index: i64,
    slot_no: i64,
}

fn load_echo_sequences(
    conn: &Connection,
    filter: &HypothesisFilter,
) -> Result<Vec<(String, Vec<EventSlim>)>, String> {
    let mut conditions = Vec::new();
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(cost_class) = filter.cost_class {
        conditions.push("e.cost_class = ?".to_string());
        params_vec.push(cost_class.into());
    }
    if let Some(main_stat_key) = &filter.main_stat_key {
        conditions.push("e.main_stat_key = ?".to_string());
        params_vec.push(main_stat_key.clone().into());
    }
    if let Some(status) = &filter.status {
        conditions.push("e.status = ?".to_string());
        params_vec.push(status.clone().into());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let query = format!(
        "SELECT oe.echo_id, oe.stat_key, oe.tier_index, oe.slot_no
         FROM ordered_events oe
         JOIN echoes e ON e.echo_id = oe.echo_id
         {}
         ORDER BY oe.echo_id, oe.slot_no ASC",
        where_clause
    );

    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("failed to prepare sequences query: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params_vec), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| format!("failed to query sequences: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect sequences: {e}"))?;

    let mut grouped: Vec<(String, Vec<EventSlim>)> = Vec::new();
    let mut current_echo: Option<String> = None;

    for (echo_id, stat_key, tier_index, slot_no) in rows {
        match &current_echo {
            Some(cur) if cur == &echo_id => {
                grouped.last_mut().unwrap().1.push(EventSlim {
                    stat_key,
                    tier_index,
                    slot_no,
                });
            }
            _ => {
                current_echo = Some(echo_id.clone());
                grouped.push((
                    echo_id,
                    vec![EventSlim {
                        stat_key,
                        tier_index,
                        slot_no,
                    }],
                ));
            }
        }
    }

    Ok(grouped)
}

fn list_stat_keys(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT stat_key, display_name FROM stat_defs WHERE enabled = 1 ORDER BY rowid")
        .map_err(|e| format!("failed to query stat_defs: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("failed to map stat_defs: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect stat_defs: {e}"))
}

fn expected_transition_without_replacement(
    from_idx: usize,
    to_idx: usize,
    row_total: i64,
    col_probs: &[f64],
) -> f64 {
    if row_total <= 0
        || from_idx == to_idx
        || from_idx >= col_probs.len()
        || to_idx >= col_probs.len()
    {
        return 0.0;
    }
    let from_mass = col_probs[from_idx];
    let denom = 1.0 - from_mass;
    if denom <= 1e-12 {
        return 0.0;
    }
    let conditional_p = col_probs[to_idx] / denom;
    row_total as f64 * conditional_p
}

/* ═══════════════════════════════════════════════════════
1. Transition matrix — stat(slot_i) → stat(slot_{i+1})
═══════════════════════════════════════════════════════ */

#[tauri::command]
pub fn get_transition_matrix(
    state: State<'_, AppState>,
    filter: Option<HypothesisFilter>,
) -> Result<TransitionMatrix, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();
    let stat_keys = list_stat_keys(&conn)?;
    let key_list: Vec<String> = stat_keys.iter().map(|(k, _)| k.clone()).collect();
    let display_map: HashMap<String, String> = stat_keys.into_iter().collect();

    let sequences = load_echo_sequences(&conn, &filter)?;

    let n = key_list.len();
    let key_idx: HashMap<&str, usize> = key_list
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();

    // Count transitions
    let mut matrix = vec![vec![0i64; n]; n];
    let mut total_transitions: i64 = 0;

    for (_echo_id, events) in &sequences {
        for window in events.windows(2) {
            let from = &window[0].stat_key;
            let to = &window[1].stat_key;
            if let (Some(&fi), Some(&ti)) = (key_idx.get(from.as_str()), key_idx.get(to.as_str())) {
                matrix[fi][ti] += 1;
                total_transitions += 1;
            }
        }
    }

    // χ² goodness-of-fit under structural "no duplicate substat per echo":
    // expected(from=i,to=i) is forced to 0; non-diagonal cells are
    // re-normalized from global marginals as first-order approximation.
    let row_sums: Vec<i64> = matrix.iter().map(|row| row.iter().sum()).collect();
    let col_sums: Vec<i64> = (0..n)
        .map(|j| matrix.iter().map(|row| row[j]).sum::<i64>())
        .collect();
    let col_probs: Vec<f64> = if total_transitions > 0 {
        col_sums
            .iter()
            .map(|&v| v as f64 / total_transitions as f64)
            .collect()
    } else {
        vec![0.0; n]
    };

    let mut chi_squared = 0.0f64;
    let mut effective_cells = 0i64;

    // Only compute for cells where both row_sum > 0 and col_sum > 0
    // (otherwise observed must be 0 and expected ~0)
    let active_rows: Vec<usize> = (0..n).filter(|&i| row_sums[i] > 0).collect();
    let active_cols: Vec<usize> = (0..n).filter(|&j| col_sums[j] > 0).collect();

    if total_transitions > 0 {
        for &i in &active_rows {
            for &j in &active_cols {
                let expected =
                    expected_transition_without_replacement(i, j, row_sums[i], &col_probs);
                if expected > 0.0 {
                    effective_cells += 1;
                    let observed = matrix[i][j] as f64;
                    chi_squared += (observed - expected).powi(2) / expected;
                }
            }
        }
    }

    // Approximate df for sparse contingency with structural zeros.
    let ar = active_rows.len() as i64;
    let ac = active_cols.len() as i64;
    let df = if effective_cells > 0 {
        (effective_cells - ar - ac + 1).max(1)
    } else {
        0
    };

    // approximate p-value using normal approximation for large df
    let p_value = if df > 0 {
        chi_sq_p_value(chi_squared, df)
    } else {
        1.0
    };

    // Build cells
    let mut cells = Vec::new();
    for (i, from_key) in key_list.iter().enumerate() {
        for (j, to_key) in key_list.iter().enumerate() {
            let count = matrix[i][j];
            let expected = expected_transition_without_replacement(i, j, row_sums[i], &col_probs);
            let residual = if expected > 0.0 {
                (count as f64 - expected) / expected.sqrt()
            } else {
                0.0
            };
            cells.push(TransitionCell {
                from_stat: from_key.clone(),
                to_stat: to_key.clone(),
                count,
                expected,
                residual,
            });
        }
    }

    Ok(TransitionMatrix {
        stat_keys: key_list
            .iter()
            .map(|k| {
                let dn = display_map.get(k).cloned().unwrap_or_else(|| k.clone());
                (k.clone(), dn)
            })
            .collect(),
        cells,
        total_transitions,
        chi_squared,
        degrees_of_freedom: df,
        p_value,
    })
}

/* ═══════════════════════════════════════════════════════
2. Category streak / zone analysis
═══════════════════════════════════════════════════════ */

#[tauri::command]
pub fn get_category_streak_analysis(
    state: State<'_, AppState>,
    filter: Option<HypothesisFilter>,
) -> Result<CategoryStreakReport, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();
    let sequences = load_echo_sequences(&conn, &filter)?;

    let mut rows = Vec::new();
    let mut zone_transition_counts: HashMap<(String, String), i64> = HashMap::new();
    let mut zone_visit_counts: HashMap<String, i64> = HashMap::new();

    // tier analysis accumulators
    let mut tier_stop_count = 0i64; // same tier
    let mut tier_step_count = 0i64; // ±1
    let mut tier_jump_count = 0i64; // > ±1
    let mut tier_total_pairs = 0i64;
    let mut tier_pair_counts_by_stat: HashMap<String, i64> = HashMap::new();

    for (echo_id, events) in &sequences {
        if events.is_empty() {
            continue;
        }

        // build per-echo sequence info
        let cats: Vec<&str> = events.iter().map(|e| stat_category(&e.stat_key)).collect();
        let zones: Vec<Vec<&str>> = cats.iter().map(|c| infer_zone(c)).collect();

        // detect streaks of same category
        let mut i = 0;
        while i < cats.len() {
            let cat = cats[i];
            let mut j = i + 1;
            while j < cats.len() && cats[j] == cat {
                j += 1;
            }
            let streak_len = j - i;
            if streak_len >= 2 {
                let stats_in_streak: Vec<String> =
                    events[i..j].iter().map(|e| e.stat_key.clone()).collect();
                let tiers_in_streak: Vec<i64> = events[i..j].iter().map(|e| e.tier_index).collect();
                rows.push(CategoryStreakRow {
                    echo_id: echo_id.clone(),
                    category: cat.to_string(),
                    start_slot: events[i].slot_no,
                    end_slot: events[j - 1].slot_no,
                    length: streak_len as i64,
                    stats: stats_in_streak,
                    tiers: tiers_in_streak,
                    possible_zones: zones[i].iter().map(|s| s.to_string()).collect(),
                });
            }
            i = j;
        }

        // zone transitions
        // flatten zones to primary zone per slot, then track transitions
        let primary_zones: Vec<String> = zones
            .iter()
            .map(|zlist| zlist.first().unwrap_or(&"未知").to_string())
            .collect();
        for pz in &primary_zones {
            *zone_visit_counts.entry(pz.clone()).or_insert(0) += 1;
        }
        for w in primary_zones.windows(2) {
            let key = (w[0].clone(), w[1].clone());
            *zone_transition_counts.entry(key).or_insert(0) += 1;
        }
    }

    // tier adjacency from all cross-slot pairs within each echo (same stat = skip since no-replace)
    // Instead: consecutive events across all echoes by analysis_seq for same stat_key
    {
        // group all events by stat_key, ordered by analysis_seq
        let mut conditions = Vec::new();
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(cost_class) = filter.cost_class {
            conditions.push("e.cost_class = ?".to_string());
            params_vec.push(cost_class.into());
        }
        if let Some(main_stat_key) = &filter.main_stat_key {
            conditions.push("e.main_stat_key = ?".to_string());
            params_vec.push(main_stat_key.clone().into());
        }
        if let Some(status) = &filter.status {
            conditions.push("e.status = ?".to_string());
            params_vec.push(status.clone().into());
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let query = format!(
            "SELECT oe.stat_key, oe.tier_index
             FROM ordered_events oe
             JOIN echoes e ON e.echo_id = oe.echo_id
             {}
             ORDER BY oe.stat_key, oe.analysis_seq ASC",
            where_clause
        );
        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| format!("failed to prepare tier query: {e}"))?;
        let tier_rows = stmt
            .query_map(rusqlite::params_from_iter(params_vec), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("failed to query tiers: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to collect tiers: {e}"))?;

        let mut prev: Option<(String, i64)> = None;
        for (stat_key, tier_index) in tier_rows {
            if let Some((ref prev_stat, prev_tier)) = prev {
                if prev_stat == &stat_key {
                    tier_total_pairs += 1;
                    *tier_pair_counts_by_stat
                        .entry(stat_key.clone())
                        .or_insert(0) += 1;
                    let diff = (tier_index - prev_tier).abs();
                    if diff == 0 {
                        tier_stop_count += 1;
                    } else if diff == 1 {
                        tier_step_count += 1;
                    } else {
                        tier_jump_count += 1;
                    }
                }
            }
            prev = Some((stat_key, tier_index));
        }
    }

    // Weighted baseline expectation by observed pair composition per stat.
    let mut expected_stop_weighted = 0.0f64;
    let mut expected_step_weighted = 0.0f64;
    let mut expected_jump_weighted = 0.0f64;
    let mut expected_weight_total = 0.0f64;
    for (stat_key, pair_count) in &tier_pair_counts_by_stat {
        let Some((exp_stop, exp_step, exp_jump)) =
            expected_tier_adjacency_ratios_for_stat(stat_key)
        else {
            continue;
        };
        let w = (*pair_count).max(0) as f64;
        if w <= 0.0 {
            continue;
        }
        expected_stop_weighted += exp_stop * w;
        expected_step_weighted += exp_step * w;
        expected_jump_weighted += exp_jump * w;
        expected_weight_total += w;
    }

    let (tier_expected_stop_ratio, tier_expected_step_ratio, tier_expected_jump_ratio) =
        if expected_weight_total > 0.0 {
            (
                Some(expected_stop_weighted / expected_weight_total),
                Some(expected_step_weighted / expected_weight_total),
                Some(expected_jump_weighted / expected_weight_total),
            )
        } else {
            (None, None, None)
        };

    // Sort streaks by length descending
    rows.sort_by(|a, b| {
        b.length
            .cmp(&a.length)
            .then_with(|| a.echo_id.cmp(&b.echo_id))
    });

    // format zone_transitions into vec
    let mut zone_transitions: Vec<(String, String, i64)> = zone_transition_counts
        .into_iter()
        .map(|((f, t), c)| (f, t, c))
        .collect();
    zone_transitions.sort_by(|a, b| b.2.cmp(&a.2));

    let mut zone_visits: Vec<(String, i64)> = zone_visit_counts.into_iter().collect();
    zone_visits.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(CategoryStreakReport {
        streaks: rows,
        zone_transitions,
        zone_visits,
        tier_total_pairs,
        tier_stop_count,
        tier_step_count,
        tier_jump_count,
        tier_stop_ratio: if tier_total_pairs > 0 {
            tier_stop_count as f64 / tier_total_pairs as f64
        } else {
            0.0
        },
        tier_step_ratio: if tier_total_pairs > 0 {
            tier_step_count as f64 / tier_total_pairs as f64
        } else {
            0.0
        },
        tier_jump_ratio: if tier_total_pairs > 0 {
            tier_jump_count as f64 / tier_total_pairs as f64
        } else {
            0.0
        },
        tier_expected_stop_ratio,
        tier_expected_step_ratio,
        tier_expected_jump_ratio,
    })
}

/* ═══════════════════════════════════════════════════════
χ² p-value approximation
═══════════════════════════════════════════════════════ */

fn chi_sq_p_value(chi2: f64, df: i64) -> f64 {
    // Use Wilson–Hilferty normal approximation for chi-squared CDF
    // P(X > chi2) ≈ 1 - Φ(z) where z = ((chi2/df)^(1/3) - (1 - 2/(9*df))) / sqrt(2/(9*df))
    if df <= 0 || chi2 <= 0.0 {
        return 1.0;
    }
    let k = df as f64;
    let term = 2.0 / (9.0 * k);
    let z = ((chi2 / k).powf(1.0 / 3.0) - (1.0 - term)) / term.sqrt();

    // 1 - Φ(z) using error function approximation
    0.5 * erfc_approx(z / std::f64::consts::SQRT_2)
}

/// Approximate erfc using Horner form (Abramowitz & Stegun 7.1.26, max error 1.5e-7)
fn erfc_approx(x: f64) -> f64 {
    if x < 0.0 {
        return 2.0 - erfc_approx(-x);
    }
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let poly = t
        * (0.254829592
            + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    poly * (-x * x).exp()
}

/* ═══════════════════════════════════════════════════════
3. Mean-reversion analysis
   ─ Flatten all events into global analysis_seq order
   ─ Per stat: running cumulative frequency deviation,
     inter-arrival gap statistics, autocorrelation,
     and window-conditional frequency
═══════════════════════════════════════════════════════ */

/// Load all events in global analysis_seq order (regardless of echo grouping).
/// Returns (analysis_seq, stat_key) pairs.
fn load_flat_timeline(
    conn: &Connection,
    filter: &HypothesisFilter,
) -> Result<Vec<(i64, String)>, String> {
    let mut conditions = Vec::<String>::new();
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(cost_class) = filter.cost_class {
        conditions.push("e.cost_class = ?".to_string());
        params_vec.push(cost_class.into());
    }
    if let Some(main_stat_key) = &filter.main_stat_key {
        conditions.push("e.main_stat_key = ?".to_string());
        params_vec.push(main_stat_key.clone().into());
    }
    if let Some(status) = &filter.status {
        conditions.push("e.status = ?".to_string());
        params_vec.push(status.clone().into());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let query = format!(
        "SELECT oe.analysis_seq, oe.stat_key
         FROM ordered_events oe
         JOIN echoes e ON e.echo_id = oe.echo_id
         {}
         ORDER BY oe.analysis_seq ASC",
        where_clause
    );

    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("prepare flat timeline: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params_vec), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("query flat timeline: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("collect flat timeline: {e}"))?;
    Ok(rows)
}

/// Sample Pearson autocorrelation of a binary Vec<f64> (0.0 / 1.0) at given lag.
fn binary_autocorr(series: &[f64], lag: usize) -> f64 {
    let n = series.len();
    if n <= lag + 1 {
        return f64::NAN;
    }
    let mean: f64 = series.iter().sum::<f64>() / n as f64;
    let var: f64 = series.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    if var < 1e-12 {
        return f64::NAN;
    }
    let cov: f64 = (0..(n - lag))
        .map(|i| (series[i] - mean) * (series[i + lag] - mean))
        .sum::<f64>()
        / n as f64;
    cov / var
}

#[tauri::command]
pub fn get_reversion_analysis(
    state: State<'_, AppState>,
    filter: Option<HypothesisFilter>,
    window_size: Option<i64>,
) -> Result<ReversionReport, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();
    let w = window_size.unwrap_or(10).max(2).min(40) as usize;

    let stat_keys = list_stat_keys(&conn)?;
    let timeline = load_flat_timeline(&conn, &filter)?;

    let n = timeline.len();
    if n == 0 {
        return Ok(ReversionReport {
            total_events: 0,
            global_seqs: vec![],
            stat_series: vec![],
        });
    }

    let global_seqs: Vec<i64> = timeline.iter().map(|(s, _)| *s).collect();
    // dense index: position 0..n maps to analysis_seq
    let flat_stats: Vec<&str> = timeline.iter().map(|(_, k)| k.as_str()).collect();

    // Base frequency from full data
    let mut base_counts: HashMap<String, usize> = HashMap::new();
    for (_, sk) in &timeline {
        *base_counts.entry(sk.clone()).or_default() += 1;
    }

    let mut stat_series: Vec<StatReversionSeries> = Vec::new();

    for (stat_key, display_name) in &stat_keys {
        let total_count = *base_counts.get(stat_key).unwrap_or(&0);
        let base_freq = total_count as f64 / n as f64;

        // Binary indicator series (0.0 / 1.0), length n
        let indicator: Vec<f64> = flat_stats
            .iter()
            .map(|k| if *k == stat_key.as_str() { 1.0 } else { 0.0 })
            .collect();

        // Running cumulative frequency deviation at each global position
        let mut cum_count = 0usize;
        let deviations: Vec<f64> = (0..n)
            .map(|i| {
                if indicator[i] > 0.5 {
                    cum_count += 1;
                }
                cum_count as f64 / (i + 1) as f64 - base_freq
            })
            .collect();

        // Positions (0-indexed) where this stat appeared
        let positions: Vec<usize> = (0..n).filter(|&i| indicator[i] > 0.5).collect();

        // Inter-arrival gaps (in events between consecutive appearances)
        let gaps: Vec<i64> = positions.windows(2).map(|w| (w[1] - w[0]) as i64).collect();

        let mean_gap = if gaps.is_empty() {
            0.0
        } else {
            gaps.iter().sum::<i64>() as f64 / gaps.len() as f64
        };
        let expected_gap = if base_freq > 0.0 {
            1.0 / base_freq
        } else {
            0.0
        };
        let gap_variance = if gaps.len() > 1 {
            let m = mean_gap;
            gaps.iter().map(|&g| (g as f64 - m).powi(2)).sum::<f64>() / (gaps.len() - 1) as f64
        } else {
            f64::NAN
        };
        // Index of Dispersion: Var/Mean. For Geometric(p): (1-p)/p ≈ expected_gap - 1
        let dispersion_index = if mean_gap > 0.0 && !gap_variance.is_nan() {
            gap_variance / mean_gap
        } else {
            f64::NAN
        };
        // Geometric baseline dispersion
        let geometric_dispersion = if base_freq > 0.0 && base_freq < 1.0 {
            (1.0 - base_freq) / base_freq
        } else {
            f64::NAN
        };

        // Autocorrelation at lags 1, 5, 10, 13
        let lag_autocorrs: Vec<(i64, f64)> = [1usize, 5, 10, 13]
            .iter()
            .map(|&lag| (lag as i64, binary_autocorr(&indicator, lag)))
            .filter(|(_, v)| !v.is_nan())
            .collect();

        // Window conditional: for each appearance, record how many occurrences
        // in the previous `w` events (exclusive), bucket into 0,1,2,3+
        // Then also record how many occurrences in the NEXT `w` events.
        let mut buckets: HashMap<usize, (i64, i64)> = HashMap::new(); // bucket → (total_next_occ, sample_count)
        for &pos in &positions {
            let prev_start = pos.saturating_sub(w);
            let prev_count = (prev_start..pos).filter(|&i| indicator[i] > 0.5).count();
            let next_end = (pos + 1 + w).min(n);
            let next_count = ((pos + 1)..next_end)
                .filter(|&i| indicator[i] > 0.5)
                .count();
            let bucket = prev_count.min(3);
            let e = buckets.entry(bucket).or_insert((0, 0));
            e.0 += next_count as i64;
            e.1 += 1;
        }
        let mut window_buckets: Vec<ReversionBucket> = buckets
            .iter()
            .map(|(&prev, &(next_sum, count))| ReversionBucket {
                prev_window_count: prev as i64,
                sample_count: count,
                mean_next_freq: if count > 0 {
                    next_sum as f64 / (count as f64 * w as f64)
                } else {
                    0.0
                },
            })
            .collect();
        window_buckets.sort_by_key(|b| b.prev_window_count);

        stat_series.push(StatReversionSeries {
            stat_key: stat_key.clone(),
            display_name: display_name.clone(),
            base_freq,
            total_count: total_count as i64,
            deviations,
            gaps,
            mean_gap,
            expected_gap,
            gap_variance,
            dispersion_index,
            geometric_dispersion,
            lag_autocorrs,
            window_buckets,
        });
    }

    // Sort by total_count descending so most common stats come first
    stat_series.sort_by(|a, b| b.total_count.cmp(&a.total_count));

    Ok(ReversionReport {
        total_events: n as i64,
        global_seqs,
        stat_series,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chi_sq_p_value_smoke() {
        // chi2 = 0 with any df should give p = 1
        let p = chi_sq_p_value(0.0, 10);
        assert!((p - 1.0).abs() < 0.1);

        // very large chi2 should give p ≈ 0
        let p2 = chi_sq_p_value(200.0, 10);
        assert!(p2 < 0.001);
    }

    #[test]
    fn stat_category_mapping() {
        assert_eq!(stat_category("crit_rate"), "crit");
        assert_eq!(stat_category("atk_flat"), "atk");
        assert_eq!(stat_category("skill_dmg"), "dmg_bonus");
        assert_eq!(stat_category("energy_regen"), "utility");
    }
}
