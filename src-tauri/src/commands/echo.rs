use std::collections::HashSet;

use rusqlite::{params, params_from_iter, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::db::{get_tier_value, now_rfc3339, open_connection, AppState};
use crate::domain::types::{
    BackfillSlotInput, CreateEchoInput, CreateEchoOutput, DeleteEchoInput,
    DeleteExpectationPresetInput, EchoFilter, EchoSubstatSlot, EchoSummary, ExpectationItem,
    ExpectationPreset, SaveExpectationPresetInput, SaveExpectationPresetOutput,
    SetExpectationsInput, SimpleOk, StatDef, StatTier, UpdateEchoInput, UpsertBackfillInput,
};

fn ensure_status(status: &str) -> Result<(), String> {
    match status {
        "tracking" | "paused" | "abandoned" | "completed" => Ok(()),
        _ => Err(format!("invalid status: {status}")),
    }
}

fn ensure_cost_class(cost_class: i64) -> Result<(), String> {
    match cost_class {
        1 | 3 | 4 => Ok(()),
        _ => Err(format!("invalid cost_class: {cost_class}")),
    }
}

fn ensure_expectation_items(items: &[ExpectationItem]) -> Result<(), String> {
    let mut stat_set = HashSet::new();
    for item in items {
        if item.rank < 1 {
            return Err(format!("rank must be >= 1, got {}", item.rank));
        }
        if !stat_set.insert(item.stat_key.clone()) {
            return Err(format!(
                "duplicate stat_key in expectation items: {}",
                item.stat_key
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn list_stat_defs(state: State<'_, AppState>) -> Result<Vec<StatDef>, String> {
    let conn = open_connection(&state)?;
    let mut stmt = conn
        .prepare(
            "SELECT stat_key, display_name, unit FROM stat_defs WHERE enabled = 1 ORDER BY rowid",
        )
        .map_err(|e| format!("failed to query stat_defs: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("failed to read stat_defs: {e}"))?;

    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|e| format!("row error: {e}"))? {
        let stat_key: String = row.get(0).map_err(|e| format!("row field error: {e}"))?;
        let display_name: String = row.get(1).map_err(|e| format!("row field error: {e}"))?;
        let unit: String = row.get(2).map_err(|e| format!("row field error: {e}"))?;

        let mut tier_stmt = conn
            .prepare(
                "SELECT tier_index, value_scaled FROM stat_tiers WHERE stat_key = ?1 ORDER BY tier_index",
            )
            .map_err(|e| format!("failed to query tiers: {e}"))?;
        let tiers = tier_stmt
            .query_map([&stat_key], |r| {
                Ok(StatTier {
                    tier_index: r.get(0)?,
                    value_scaled: r.get(1)?,
                })
            })
            .map_err(|e| format!("tier map error: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("tier collect error: {e}"))?;

        result.push(StatDef {
            stat_key,
            display_name,
            unit,
            tiers,
        });
    }

    Ok(result)
}

#[tauri::command]
pub fn create_echo(
    state: State<'_, AppState>,
    input: CreateEchoInput,
) -> Result<CreateEchoOutput, String> {
    ensure_cost_class(input.cost_class)?;
    let status = input.status.unwrap_or_else(|| "tracking".to_string());
    ensure_status(&status)?;

    let conn = open_connection(&state)?;
    let now = now_rfc3339();
    let echo_id = Uuid::new_v4().to_string();
    let nickname = input.nickname.and_then(|n| {
        let trimmed = n.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    conn.execute(
        "INSERT INTO echoes(echo_id, nickname, main_stat_key, cost_class, status, opened_slots_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6)",
        params![echo_id, nickname, input.main_stat_key, input.cost_class, status, now],
    )
    .map_err(|e| format!("failed to create echo: {e}"))?;

    Ok(CreateEchoOutput { echo_id })
}

#[tauri::command]
pub fn update_echo(state: State<'_, AppState>, input: UpdateEchoInput) -> Result<SimpleOk, String> {
    let conn = open_connection(&state)?;

    let existing = conn
        .query_row(
            "SELECT nickname, main_stat_key, cost_class, status FROM echoes WHERE echo_id = ?1",
            [&input.echo_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("failed to query echo: {e}"))?
        .ok_or_else(|| format!("echo not found: {}", input.echo_id))?;

    let nickname = input.nickname.map(|n| {
        let trimmed = n.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let main_stat_key = input.main_stat_key.unwrap_or(existing.1);
    let cost_class = input.cost_class.unwrap_or(existing.2);
    let status = input.status.unwrap_or(existing.3);
    ensure_cost_class(cost_class)?;
    ensure_status(&status)?;

    conn.execute(
        "UPDATE echoes
         SET nickname = ?2,
             main_stat_key = ?3,
             cost_class = ?4,
             status = ?5,
             updated_at = ?6
         WHERE echo_id = ?1",
        params![
            input.echo_id,
            nickname.unwrap_or(existing.0),
            main_stat_key,
            cost_class,
            status,
            now_rfc3339()
        ],
    )
    .map_err(|e| format!("failed to update echo: {e}"))?;

    Ok(SimpleOk { ok: true })
}

#[tauri::command]
pub fn delete_echo(state: State<'_, AppState>, input: DeleteEchoInput) -> Result<SimpleOk, String> {
    let mut conn = open_connection(&state)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start delete transaction: {e}"))?;

    let exists: Option<String> = tx
        .query_row(
            "SELECT echo_id FROM echoes WHERE echo_id = ?1",
            [&input.echo_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to verify echo existence: {e}"))?;
    if exists.is_none() {
        return Err(format!("echo not found: {}", input.echo_id));
    }

    tx.execute(
        "DELETE FROM event_edit_logs
         WHERE event_id IN (SELECT event_id FROM ordered_events WHERE echo_id = ?1)",
        [&input.echo_id],
    )
    .map_err(|e| format!("failed to delete event_edit_logs: {e}"))?;

    tx.execute(
        "DELETE FROM ordered_events WHERE echo_id = ?1",
        [&input.echo_id],
    )
    .map_err(|e| format!("failed to delete ordered_events: {e}"))?;
    tx.execute(
        "DELETE FROM echo_current_substats WHERE echo_id = ?1",
        [&input.echo_id],
    )
    .map_err(|e| format!("failed to delete current_substats: {e}"))?;
    tx.execute(
        "DELETE FROM echo_expectations WHERE echo_id = ?1",
        [&input.echo_id],
    )
    .map_err(|e| format!("failed to delete expectations: {e}"))?;
    tx.execute("DELETE FROM echoes WHERE echo_id = ?1", [&input.echo_id])
        .map_err(|e| format!("failed to delete echo: {e}"))?;

    tx.commit()
        .map_err(|e| format!("failed to commit delete echo transaction: {e}"))?;
    Ok(SimpleOk { ok: true })
}

#[tauri::command]
pub fn list_echoes(
    state: State<'_, AppState>,
    filter: Option<EchoFilter>,
) -> Result<Vec<EchoSummary>, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();

    let mut conditions: Vec<String> = vec![];
    let mut params_vec: Vec<rusqlite::types::Value> = vec![];

    if let Some(status) = &filter.status {
        conditions.push("e.status = ?".to_string());
        params_vec.push(status.clone().into());
    }
    if let Some(main_stat_key) = &filter.main_stat_key {
        conditions.push("e.main_stat_key = ?".to_string());
        params_vec.push(main_stat_key.clone().into());
    }
    if let Some(cost_class) = filter.cost_class {
        conditions.push("e.cost_class = ?".to_string());
        params_vec.push(cost_class.into());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let query = format!(
        "SELECT e.echo_id, e.nickname, e.main_stat_key, e.cost_class, e.status, e.opened_slots_count, e.created_at, e.updated_at
         FROM echoes e
         {}
         ORDER BY e.updated_at DESC",
        where_clause
    );

    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("failed to prepare list query: {e}"))?;

    let base_rows = stmt
        .query_map(params_from_iter(params_vec), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(|e| format!("failed to query echoes: {e}"))?;

    let mut output = Vec::new();

    for row in base_rows {
        let (
            echo_id,
            nickname,
            main_stat_key,
            cost_class,
            status,
            opened_slots_count,
            created_at,
            updated_at,
        ) = row.map_err(|e| format!("failed to read echo row: {e}"))?;

        let mut exp_stmt = conn
            .prepare(
                "SELECT stat_key, rank FROM echo_expectations WHERE echo_id = ?1 ORDER BY rank ASC, stat_key ASC",
            )
            .map_err(|e| format!("failed to prepare expectations query: {e}"))?;
        let expectations = exp_stmt
            .query_map([&echo_id], |r| {
                Ok(ExpectationItem {
                    stat_key: r.get(0)?,
                    rank: r.get(1)?,
                })
            })
            .map_err(|e| format!("failed to map expectations: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to collect expectations: {e}"))?;

        let mut cs_stmt = conn
            .prepare(
                "SELECT slot_no, stat_key, tier_index, value_scaled, source
                 FROM echo_current_substats
                 WHERE echo_id = ?1
                 ORDER BY slot_no ASC",
            )
            .map_err(|e| format!("failed to prepare current_substats query: {e}"))?;
        let current_substats = cs_stmt
            .query_map([&echo_id], |r| {
                Ok(EchoSubstatSlot {
                    slot_no: r.get(0)?,
                    stat_key: r.get(1)?,
                    tier_index: r.get(2)?,
                    value_scaled: r.get(3)?,
                    source: r.get(4)?,
                })
            })
            .map_err(|e| format!("failed to map current_substats: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to collect current_substats: {e}"))?;

        output.push(EchoSummary {
            echo_id,
            nickname,
            main_stat_key,
            cost_class,
            status,
            opened_slots_count,
            created_at,
            updated_at,
            expectations,
            current_substats,
        });
    }

    Ok(output)
}

#[tauri::command]
pub fn set_expectations(
    state: State<'_, AppState>,
    input: SetExpectationsInput,
) -> Result<SimpleOk, String> {
    ensure_expectation_items(&input.items)?;
    let mut conn = open_connection(&state)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start transaction: {e}"))?;

    tx.execute(
        "DELETE FROM echo_expectations WHERE echo_id = ?1",
        [&input.echo_id],
    )
    .map_err(|e| format!("failed to clear expectations: {e}"))?;

    for item in input.items {
        tx.execute(
            "INSERT INTO echo_expectations(echo_id, stat_key, rank) VALUES (?1, ?2, ?3)",
            params![input.echo_id, item.stat_key, item.rank],
        )
        .map_err(|e| format!("failed to insert expectation: {e}"))?;
    }

    tx.execute(
        "UPDATE echoes SET updated_at = ?2 WHERE echo_id = ?1",
        params![input.echo_id, now_rfc3339()],
    )
    .map_err(|e| format!("failed to update timestamp: {e}"))?;

    tx.commit()
        .map_err(|e| format!("failed to commit expectations: {e}"))?;

    Ok(SimpleOk { ok: true })
}

fn ensure_backfill_slots(slots: &[BackfillSlotInput]) -> Result<(), String> {
    let mut slot_set = HashSet::new();
    let mut stat_set = HashSet::new();

    for slot in slots {
        if !(1..=5).contains(&slot.slot_no) {
            return Err(format!("slot_no out of range: {}", slot.slot_no));
        }
        if !slot_set.insert(slot.slot_no) {
            return Err(format!("duplicate slot_no in backfill: {}", slot.slot_no));
        }
        if !stat_set.insert(slot.stat_key.clone()) {
            return Err(format!("duplicate stat_key in backfill: {}", slot.stat_key));
        }
    }

    Ok(())
}

#[tauri::command]
pub fn upsert_backfill_state(
    state: State<'_, AppState>,
    input: UpsertBackfillInput,
) -> Result<SimpleOk, String> {
    ensure_backfill_slots(&input.slots)?;

    let mut conn = open_connection(&state)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start transaction: {e}"))?;

    let exists: Option<String> = tx
        .query_row(
            "SELECT echo_id FROM echoes WHERE echo_id = ?1",
            [&input.echo_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to verify echo existence: {e}"))?;
    if exists.is_none() {
        return Err(format!("echo not found: {}", input.echo_id));
    }

    tx.execute(
        "DELETE FROM echo_current_substats WHERE echo_id = ?1 AND source = 'backfill'",
        [&input.echo_id],
    )
    .map_err(|e| format!("failed to clear backfill rows: {e}"))?;

    for slot in input.slots {
        let ordered_exists: Option<String> = tx
            .query_row(
                "SELECT event_id FROM ordered_events WHERE echo_id = ?1 AND slot_no = ?2",
                params![input.echo_id, slot.slot_no],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("failed to check ordered slot: {e}"))?;
        if ordered_exists.is_some() {
            return Err(format!(
                "slot {} already has ordered event; edit the event instead of backfill",
                slot.slot_no
            ));
        }

        let value_scaled = get_tier_value(&tx, &slot.stat_key, slot.tier_index)?;
        tx.execute(
            "INSERT INTO echo_current_substats(echo_id, slot_no, stat_key, tier_index, value_scaled, source, event_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 'backfill', NULL)
             ON CONFLICT(echo_id, slot_no)
             DO UPDATE SET stat_key = excluded.stat_key,
                           tier_index = excluded.tier_index,
                           value_scaled = excluded.value_scaled,
                           source = 'backfill',
                           event_id = NULL",
            params![
                input.echo_id,
                slot.slot_no,
                slot.stat_key,
                slot.tier_index,
                value_scaled
            ],
        )
        .map_err(|e| format!("failed to upsert backfill slot: {e}"))?;
    }

    let opened_slots_count: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(slot_no), 0) FROM echo_current_substats WHERE echo_id = ?1",
            [&input.echo_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("failed to compute opened slots: {e}"))?;

    tx.execute(
        "UPDATE echoes SET opened_slots_count = ?2, updated_at = ?3 WHERE echo_id = ?1",
        params![input.echo_id, opened_slots_count, now_rfc3339()],
    )
    .map_err(|e| format!("failed to update echo after backfill: {e}"))?;

    tx.commit()
        .map_err(|e| format!("failed to commit backfill: {e}"))?;

    Ok(SimpleOk { ok: true })
}

#[tauri::command]
pub fn list_expectation_presets(
    state: State<'_, AppState>,
) -> Result<Vec<ExpectationPreset>, String> {
    let conn = open_connection(&state)?;
    let mut preset_stmt = conn
        .prepare(
            "SELECT preset_id, name, created_at, updated_at
             FROM expectation_presets
             ORDER BY updated_at DESC, created_at DESC",
        )
        .map_err(|e| format!("failed to prepare preset query: {e}"))?;
    let preset_rows = preset_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| format!("failed to query preset rows: {e}"))?;

    let mut item_stmt = conn
        .prepare(
            "SELECT stat_key, rank
             FROM expectation_preset_items
             WHERE preset_id = ?1
             ORDER BY rank ASC, stat_key ASC",
        )
        .map_err(|e| format!("failed to prepare preset items query: {e}"))?;

    let mut presets = Vec::new();
    for row in preset_rows {
        let (preset_id, name, created_at, updated_at) =
            row.map_err(|e| format!("failed to read preset row: {e}"))?;
        let items = item_stmt
            .query_map([&preset_id], |r| {
                Ok(ExpectationItem {
                    stat_key: r.get(0)?,
                    rank: r.get(1)?,
                })
            })
            .map_err(|e| format!("failed to query preset items: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to collect preset items: {e}"))?;
        presets.push(ExpectationPreset {
            preset_id,
            name,
            created_at,
            updated_at,
            items,
        });
    }
    Ok(presets)
}

#[tauri::command]
pub fn save_expectation_preset(
    state: State<'_, AppState>,
    input: SaveExpectationPresetInput,
) -> Result<SaveExpectationPresetOutput, String> {
    ensure_expectation_items(&input.items)?;
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("preset name cannot be empty".to_string());
    }

    let mut conn = open_connection(&state)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start preset transaction: {e}"))?;
    let now = now_rfc3339();

    let preset_id = match input.preset_id {
        Some(preset_id) => {
            let exists: Option<String> = tx
                .query_row(
                    "SELECT preset_id FROM expectation_presets WHERE preset_id = ?1",
                    [&preset_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("failed to check existing preset: {e}"))?;
            if exists.is_some() {
                tx.execute(
                    "UPDATE expectation_presets
                     SET name = ?2, updated_at = ?3
                     WHERE preset_id = ?1",
                    params![preset_id, name, now],
                )
                .map_err(|e| format!("failed to update preset: {e}"))?;
                preset_id
            } else {
                tx.execute(
                    "INSERT INTO expectation_presets(preset_id, name, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?3)",
                    params![preset_id, name, now],
                )
                .map_err(|e| format!("failed to create preset: {e}"))?;
                preset_id
            }
        }
        None => {
            let preset_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO expectation_presets(preset_id, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![preset_id, name, now],
            )
            .map_err(|e| format!("failed to create preset: {e}"))?;
            preset_id
        }
    };

    tx.execute(
        "DELETE FROM expectation_preset_items WHERE preset_id = ?1",
        [&preset_id],
    )
    .map_err(|e| format!("failed to clear preset items: {e}"))?;

    for item in input.items {
        tx.execute(
            "INSERT INTO expectation_preset_items(preset_id, stat_key, rank)
             VALUES (?1, ?2, ?3)",
            params![preset_id, item.stat_key, item.rank],
        )
        .map_err(|e| format!("failed to insert preset item: {e}"))?;
    }

    tx.commit()
        .map_err(|e| format!("failed to commit preset save: {e}"))?;

    Ok(SaveExpectationPresetOutput { preset_id })
}

#[tauri::command]
pub fn delete_expectation_preset(
    state: State<'_, AppState>,
    input: DeleteExpectationPresetInput,
) -> Result<SimpleOk, String> {
    let conn = open_connection(&state)?;
    conn.execute(
        "DELETE FROM expectation_presets WHERE preset_id = ?1",
        [&input.preset_id],
    )
    .map_err(|e| format!("failed to delete preset: {e}"))?;
    Ok(SimpleOk { ok: true })
}
