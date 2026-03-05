use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use tauri::State;

use crate::db::{open_connection, AppState};
use crate::domain::types::{
    AdaptiveNextSuggestion, DailyExactPatternRow, DailyPatternDecisionFilter,
    DailyPatternDecisionReport, DailyShapePatternRow, ManualCycleSuggestion,
    ManualGuessVerificationRow, ManualPatternSummary,
};

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

pub fn get_daily_pattern_decision_internal(
    conn: &Connection,
    filter: &DailyPatternDecisionFilter,
) -> Result<DailyPatternDecisionReport, String> {
    let filter = filter.clone();

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

    let game_day = resolve_game_day(&conn, &filter)?;
    if game_day.is_empty() {
        return Ok(DailyPatternDecisionReport {
            game_day,
            total_events: 0,
            min_len: min_len as i64,
            max_len: max_len as i64,
            min_support,
            max_order: max_order as i64,
            model_confidence: 0.0,
            exact_patterns: Vec::new(),
            shape_patterns: Vec::new(),
            suggestions: Vec::new(),
            manual_summary: None,
            notes: vec!["当前筛选条件下没有可用数据。".to_string()],
        });
    }

    let seq = load_day_sequence(&conn, &game_day)?;
    let n = seq.len();
    if n == 0 {
        return Ok(DailyPatternDecisionReport {
            game_day,
            total_events: 0,
            min_len: min_len as i64,
            max_len: max_len as i64,
            min_support,
            max_order: max_order as i64,
            model_confidence: 0.0,
            exact_patterns: Vec::new(),
            shape_patterns: Vec::new(),
            suggestions: Vec::new(),
            manual_summary: None,
            notes: vec!["该日无事件，无法识别模式。".to_string()],
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

    let alpha = 1.0f64;
    let k = stat_keys.len().max(1) as f64;
    let denom = n as f64 + alpha * k;
    let base_probs: HashMap<String, f64> = stat_keys
        .iter()
        .map(|s| {
            let count = *base_counts.get(s).unwrap_or(&0) as f64;
            (s.clone(), (count + alpha) / denom)
        })
        .collect();

    let mut markov_models: Vec<HashMap<Vec<String>, HashMap<String, i64>>> = Vec::new();
    for order in 1..=max_order {
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
    let mut notes = vec!["当前模型按全局序列建模，不区分 Cost/主词条/状态。".to_string()];

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

    for order in 1..=max_order {
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
    let markov_blend = (markov_weight_total / (markov_weight_total + 2.0)).min(0.55);

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
    let motif_lambda = 1.15f64;

    let mut suggestions = Vec::<AdaptiveNextSuggestion>::new();
    let mut total_score = 0.0;
    for stat_key in &stat_keys {
        let base = *base_probs.get(stat_key).unwrap_or(&0.0);
        let markov = *markov_probs.get(stat_key).unwrap_or(&base);
        let cycle = if cycle_weight > 0.0 {
            *cycle_probs.get(stat_key).unwrap_or(&base)
        } else {
            base
        };

        let pre_markov = (1.0 - markov_blend) * base + markov_blend * markov;
        let pre = (1.0 - cycle_weight) * pre_markov + cycle_weight * cycle;

        let raw_boost = *raw_boost_map.get(stat_key).unwrap_or(&0.0);
        let norm_boost = if max_boost > 1e-9 {
            raw_boost / max_boost
        } else {
            0.0
        };
        let score = pre * (1.0 + motif_lambda * norm_boost);
        total_score += score;

        let mut matched = matched_patterns_map.remove(stat_key).unwrap_or_default();
        matched.sort();
        matched.dedup();
        matched.truncate(4);

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
            matched_patterns: matched,
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
    let markov_conf = (markov_blend / 0.55).min(1.0);
    let cycle_conf = (cycle_weight / 0.45).min(1.0);
    let motif_conf = if max_boost > 0.0 {
        1.0 - 1.0 / (1.0 + max_boost)
    } else {
        0.0
    };
    let model_confidence =
        (0.35 * sample_conf + 0.25 * markov_conf + 0.20 * cycle_conf + 0.20 * motif_conf).min(1.0);

    if n < 20 {
        notes.push("当日样本较少，系统会自动回退到基础概率。".to_string());
    }
    if markov_weight_total <= 0.0 {
        notes.push("未命中可用上下文，预测主要由基础分布驱动。".to_string());
    }

    Ok(DailyPatternDecisionReport {
        game_day,
        total_events: n as i64,
        min_len: min_len as i64,
        max_len: max_len as i64,
        min_support,
        max_order: max_order as i64,
        model_confidence,
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
