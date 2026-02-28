use std::collections::HashMap;

use rusqlite::Connection;
use tauri::State;

use crate::db::{open_connection, AppState};
use crate::domain::types::{
    CategoryStreakReport, CategoryStreakRow, HypothesisFilter, SlotStatCell,
    SlotStatDistribution, TransitionCell, TransitionMatrix,
};

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
    ("共鸣区", &["dmg_bonus", "utility"]),    // skill/liberation + energy_regen
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
    let key_idx: HashMap<&str, usize> = key_list.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();

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

    // χ² goodness-of-fit vs independence assumption
    // Under H0: P(to | from) = P(to) (marginal probability)
    let row_sums: Vec<i64> = matrix.iter().map(|row| row.iter().sum()).collect();
    let col_sums: Vec<i64> = (0..n).map(|j| matrix.iter().map(|row| row[j]).sum::<i64>()).collect();

    let mut chi_squared = 0.0f64;

    // Only compute for cells where both row_sum > 0 and col_sum > 0
    // (otherwise observed must be 0 and expected ~0)
    let active_rows: Vec<usize> = (0..n).filter(|&i| row_sums[i] > 0).collect();
    let active_cols: Vec<usize> = (0..n).filter(|&j| col_sums[j] > 0).collect();

    if total_transitions > 0 {
        for &i in &active_rows {
            // Under H0 a from‑stat has been removed from the pool,
            // so the available portion of the remaining candidates should
            // be re‑normalised — but with real data we cannot know which
            // candidates were available per echo. As a first‑order
            // approximation we use the global marginal.
            for &j in &active_cols {
                let expected = (row_sums[i] as f64) * (col_sums[j] as f64)
                    / (total_transitions as f64);
                if expected > 0.0 {
                    let observed = matrix[i][j] as f64;
                    chi_squared += (observed - expected).powi(2) / expected;
                }
            }
        }
    }

    // df = (active_rows - 1) * (active_cols - 1)
    let ar = active_rows.len() as i64;
    let ac = active_cols.len() as i64;
    let df = (ar.max(1) - 1) * (ac.max(1) - 1);

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
            if count > 0 || true {
                // include all for full matrix
                let expected = if total_transitions > 0 && row_sums[i] > 0 {
                    (row_sums[i] as f64) * (col_sums[j] as f64) / (total_transitions as f64)
                } else {
                    0.0
                };
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
    }

    Ok(TransitionMatrix {
        stat_keys: key_list.iter().map(|k| {
            let dn = display_map.get(k).cloned().unwrap_or_else(|| k.clone());
            (k.clone(), dn)
        }).collect(),
        cells,
        total_transitions,
        chi_squared,
        degrees_of_freedom: df,
        p_value,
    })
}

/* ═══════════════════════════════════════════════════════
   2. Per-slot stat distribution — P(stat | slotNo)
   ═══════════════════════════════════════════════════════ */

#[tauri::command]
pub fn get_slot_stat_distribution(
    state: State<'_, AppState>,
    filter: Option<HypothesisFilter>,
) -> Result<SlotStatDistribution, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();
    let stat_keys = list_stat_keys(&conn)?;
    let key_list: Vec<String> = stat_keys.iter().map(|(k, _)| k.clone()).collect();
    let display_map: HashMap<String, String> = stat_keys.into_iter().collect();
    let key_idx: HashMap<&str, usize> =
        key_list.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();

    let sequences = load_echo_sequences(&conn, &filter)?;

    let n = key_list.len();
    // slots 1..5
    let mut grid = vec![vec![0i64; n]; 5];
    let mut slot_totals = vec![0i64; 5];

    for (_echo_id, events) in &sequences {
        for ev in events {
            let si = (ev.slot_no - 1).clamp(0, 4) as usize;
            if let Some(&ki) = key_idx.get(ev.stat_key.as_str()) {
                grid[si][ki] += 1;
                slot_totals[si] += 1;
            }
        }
    }

    let mut cells = Vec::new();
    for slot in 0..5usize {
        for (ki, key) in key_list.iter().enumerate() {
            let count = grid[slot][ki];
            let probability = if slot_totals[slot] > 0 {
                count as f64 / slot_totals[slot] as f64
            } else {
                0.0
            };
            let category = stat_category(key).to_string();
            cells.push(SlotStatCell {
                slot_no: (slot + 1) as i64,
                stat_key: key.clone(),
                display_name: display_map.get(key).cloned().unwrap_or_else(|| key.clone()),
                category,
                count,
                probability,
            });
        }
    }

    // Chi-squared test: is slot_no independent of stat_key?
    let grand_total: i64 = slot_totals.iter().sum();
    let stat_totals: Vec<i64> = (0..n)
        .map(|ki| (0..5).map(|si| grid[si][ki]).sum::<i64>())
        .collect();

    let mut chi_squared = 0.0f64;
    if grand_total > 0 {
        for slot in 0..5 {
            for ki in 0..n {
                let expected =
                    (slot_totals[slot] as f64) * (stat_totals[ki] as f64) / (grand_total as f64);
                if expected > 0.0 {
                    let observed = grid[slot][ki] as f64;
                    chi_squared += (observed - expected).powi(2) / expected;
                }
            }
        }
    }

    let active_slots = slot_totals.iter().filter(|&&t| t > 0).count() as i64;
    let active_stats = stat_totals.iter().filter(|&&t| t > 0).count() as i64;
    let df = (active_slots.max(1) - 1) * (active_stats.max(1) - 1);
    let p_value = if df > 0 {
        chi_sq_p_value(chi_squared, df)
    } else {
        1.0
    };

    Ok(SlotStatDistribution {
        stat_keys: key_list
            .iter()
            .map(|k| {
                let dn = display_map.get(k).cloned().unwrap_or_else(|| k.clone());
                (k.clone(), dn)
            })
            .collect(),
        cells,
        slot_totals,
        total_events: grand_total,
        chi_squared,
        degrees_of_freedom: df,
        p_value,
    })
}

/* ═══════════════════════════════════════════════════════
   3. Category streak / zone analysis
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
    let mut tier_pairs_same_stat: Vec<(i64, i64)> = Vec::new(); // (prev_tier, curr_tier) for consecutive same stat type cross-echo
    let mut tier_stop_count = 0i64; // same tier
    let mut tier_step_count = 0i64; // ±1
    let mut tier_jump_count = 0i64; // > ±1
    let mut tier_total_pairs = 0i64;

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
                let tiers_in_streak: Vec<i64> =
                    events[i..j].iter().map(|e| e.tier_index).collect();
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
                    tier_pairs_same_stat.push((prev_tier, tier_index));
                    tier_total_pairs += 1;
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

    // expected under uniform: P(stop) = 1/N_tiers, P(step) depends on tier
    // With 8 tiers uniform: P(same) = 1/8 = 12.5%, P(±1) ≈ 2/8 = 25% (edges: 1/8)
    // Average P(±1) for uniform 8-tier = (2*6 + 1*2) / (8*8) = 14/64 = 21.875%
    // For 4-tier stats: P(same) = 1/4 = 25%, P(±1) = (2*2 + 1*2) / (4*4) = 6/16 = 37.5%

    // Sort streaks by length descending
    rows.sort_by(|a, b| b.length.cmp(&a.length).then_with(|| a.echo_id.cmp(&b.echo_id)));

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
            + t * (-0.284496736
                + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    poly * (-x * x).exp()
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
