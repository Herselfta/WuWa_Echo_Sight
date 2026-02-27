use rusqlite::{params, OptionalExtension};
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::db::{
    compute_game_day, get_setting_i64, get_tier_value, now_rfc3339, open_connection, parse_event_time,
    AppState, DEFAULT_DAY_BOUNDARY_HOUR,
};
use crate::domain::types::{
    AppendOrderedEventInput, AppendOrderedEventOutput, EditOrderedEventInput, EditOrderedEventOutput,
    EventHistoryFilter, EventRow,
};

fn reorder_analysis_seq(tx: &rusqlite::Transaction<'_>) -> Result<String, String> {
    let mut stmt = tx
        .prepare(
            "SELECT event_id FROM ordered_events ORDER BY event_time ASC, created_seq ASC, event_id ASC",
        )
        .map_err(|e| format!("failed to prepare reorder query: {e}"))?;
    let event_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to read reorder rows: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to collect reorder rows: {e}"))?;

    for (idx, event_id) in event_ids.iter().enumerate() {
        tx.execute(
            "UPDATE ordered_events SET analysis_seq = ?2 WHERE event_id = ?1",
            params![event_id, (idx as i64) + 1],
        )
        .map_err(|e| format!("failed to update analysis_seq: {e}"))?;
    }

    Ok(format!("all:{}", event_ids.len()))
}

#[tauri::command]
pub fn append_ordered_event(
    state: State<'_, AppState>,
    input: AppendOrderedEventInput,
) -> Result<AppendOrderedEventOutput, String> {
    if !(1..=5).contains(&input.slot_no) {
        return Err(format!("slot_no out of range: {}", input.slot_no));
    }

    parse_event_time(&input.event_time)?;

    let mut conn = open_connection(&state)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to begin transaction: {e}"))?;

    let exists = tx
        .query_row(
            "SELECT 1 FROM echoes WHERE echo_id = ?1",
            [&input.echo_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| format!("failed to verify echo existence: {e}"))?;

    if exists.is_none() {
        return Err(format!("echo not found: {}", input.echo_id));
    }

    let slot_exists: Option<String> = tx
        .query_row(
            "SELECT event_id FROM ordered_events WHERE echo_id = ?1 AND slot_no = ?2",
            params![input.echo_id, input.slot_no],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to check slot conflict: {e}"))?;
    if slot_exists.is_some() {
        return Err(format!("slot {} already has an ordered event", input.slot_no));
    }

    let duplicate_stat: Option<i64> = tx
        .query_row(
            "SELECT slot_no FROM echo_current_substats WHERE echo_id = ?1 AND stat_key = ?2",
            params![input.echo_id, input.stat_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to check duplicate stat: {e}"))?;
    if duplicate_stat.is_some() {
        return Err(format!(
            "stat {} already exists on this echo, duplicate substat is not allowed",
            input.stat_key
        ));
    }

    let value_scaled = get_tier_value(&tx, &input.stat_key, input.tier_index)?;
    let created_seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(created_seq), 0) + 1 FROM ordered_events",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("failed to get created_seq: {e}"))?;
    let analysis_seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(analysis_seq), 0) + 1 FROM ordered_events",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("failed to get analysis_seq: {e}"))?;
    let day_boundary = get_setting_i64(&tx, "day_boundary_hour", DEFAULT_DAY_BOUNDARY_HOUR);
    let game_day = compute_game_day(&input.event_time, day_boundary)?;

    let event_id = Uuid::new_v4().to_string();
    let now = now_rfc3339();

    tx.execute(
        "INSERT INTO ordered_events(
            event_id, echo_id, slot_no, stat_key, tier_index, value_scaled,
            event_time, created_seq, analysis_seq, game_day, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event_id,
            input.echo_id,
            input.slot_no,
            input.stat_key,
            input.tier_index,
            value_scaled,
            input.event_time,
            created_seq,
            analysis_seq,
            game_day,
            now
        ],
    )
    .map_err(|e| format!("failed to insert ordered event: {e}"))?;

    tx.execute(
        "INSERT INTO echo_current_substats(echo_id, slot_no, stat_key, tier_index, value_scaled, source, event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, 'ordered_event', ?6)
         ON CONFLICT(echo_id, slot_no)
         DO UPDATE SET stat_key = excluded.stat_key,
                       tier_index = excluded.tier_index,
                       value_scaled = excluded.value_scaled,
                       source = 'ordered_event',
                       event_id = excluded.event_id",
        params![
            input.echo_id,
            input.slot_no,
            input.stat_key,
            input.tier_index,
            value_scaled,
            event_id
        ],
    )
    .map_err(|e| format!("failed to sync current_substats: {e}"))?;

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
    .map_err(|e| format!("failed to update echo after append: {e}"))?;

    tx.commit()
        .map_err(|e| format!("failed to commit append event: {e}"))?;

    Ok(AppendOrderedEventOutput { event_id })
}

#[tauri::command]
pub fn edit_ordered_event(
    state: State<'_, AppState>,
    input: EditOrderedEventInput,
) -> Result<EditOrderedEventOutput, String> {
    let mut conn = open_connection(&state)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to begin transaction: {e}"))?;

    let before = tx
        .query_row(
            "SELECT echo_id, slot_no, stat_key, tier_index, value_scaled, event_time, created_seq, analysis_seq, game_day
             FROM ordered_events
             WHERE event_id = ?1",
            [&input.event_id],
            |row| {
                Ok(json!({
                    "echoId": row.get::<_, String>(0)?,
                    "slotNo": row.get::<_, i64>(1)?,
                    "statKey": row.get::<_, String>(2)?,
                    "tierIndex": row.get::<_, i64>(3)?,
                    "valueScaled": row.get::<_, i64>(4)?,
                    "eventTime": row.get::<_, String>(5)?,
                    "createdSeq": row.get::<_, i64>(6)?,
                    "analysisSeq": row.get::<_, i64>(7)?,
                    "gameDay": row.get::<_, String>(8)?,
                }))
            },
        )
        .optional()
        .map_err(|e| format!("failed to query existing event: {e}"))?
        .ok_or_else(|| format!("event not found: {}", input.event_id))?;

    let echo_id = before["echoId"]
        .as_str()
        .ok_or_else(|| "invalid stored echoId".to_string())?
        .to_string();
    let prev_slot_no = before["slotNo"]
        .as_i64()
        .ok_or_else(|| "invalid stored slotNo".to_string())?;
    let prev_stat_key = before["statKey"]
        .as_str()
        .ok_or_else(|| "invalid stored statKey".to_string())?
        .to_string();
    let prev_tier_index = before["tierIndex"]
        .as_i64()
        .ok_or_else(|| "invalid stored tierIndex".to_string())?;
    let prev_event_time = before["eventTime"]
        .as_str()
        .ok_or_else(|| "invalid stored eventTime".to_string())?
        .to_string();

    let slot_no = input.slot_no.unwrap_or(prev_slot_no);
    let stat_key = input.stat_key.unwrap_or(prev_stat_key);
    let tier_index = input.tier_index.unwrap_or(prev_tier_index);
    let event_time = input.event_time.unwrap_or(prev_event_time);
    let reorder_mode = input.reorder_mode.unwrap_or_else(|| "none".to_string());

    if !(1..=5).contains(&slot_no) {
        return Err(format!("slot_no out of range: {slot_no}"));
    }
    if reorder_mode != "none" && reorder_mode != "time_assist" {
        return Err(format!("invalid reorder_mode: {reorder_mode}"));
    }

    parse_event_time(&event_time)?;

    let conflict_slot: Option<String> = tx
        .query_row(
            "SELECT event_id FROM ordered_events WHERE echo_id = ?1 AND slot_no = ?2 AND event_id <> ?3",
            params![echo_id, slot_no, input.event_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to check slot conflict: {e}"))?;
    if conflict_slot.is_some() {
        return Err(format!("slot {} already used by another event", slot_no));
    }

    let conflict_stat: Option<i64> = tx
        .query_row(
            "SELECT slot_no FROM echo_current_substats
             WHERE echo_id = ?1 AND stat_key = ?2 AND event_id <> ?3",
            params![echo_id, stat_key, input.event_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to check stat conflict: {e}"))?;
    if conflict_stat.is_some() {
        return Err(format!("stat {} already exists in another slot", stat_key));
    }

    let value_scaled = get_tier_value(&tx, &stat_key, tier_index)?;
    let day_boundary = get_setting_i64(&tx, "day_boundary_hour", DEFAULT_DAY_BOUNDARY_HOUR);
    let game_day = compute_game_day(&event_time, day_boundary)?;

    tx.execute(
        "UPDATE ordered_events
         SET slot_no = ?2,
             stat_key = ?3,
             tier_index = ?4,
             value_scaled = ?5,
             event_time = ?6,
             game_day = ?7
         WHERE event_id = ?1",
        params![
            input.event_id,
            slot_no,
            stat_key,
            tier_index,
            value_scaled,
            event_time,
            game_day
        ],
    )
    .map_err(|e| format!("failed to update ordered event: {e}"))?;

    tx.execute(
        "DELETE FROM echo_current_substats WHERE echo_id = ?1 AND source = 'ordered_event' AND event_id = ?2",
        params![echo_id, input.event_id],
    )
    .map_err(|e| format!("failed to remove prior current_substats row: {e}"))?;

    tx.execute(
        "INSERT INTO echo_current_substats(echo_id, slot_no, stat_key, tier_index, value_scaled, source, event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, 'ordered_event', ?6)
         ON CONFLICT(echo_id, slot_no)
         DO UPDATE SET stat_key = excluded.stat_key,
                       tier_index = excluded.tier_index,
                       value_scaled = excluded.value_scaled,
                       source = 'ordered_event',
                       event_id = excluded.event_id",
        params![echo_id, slot_no, stat_key, tier_index, value_scaled, input.event_id],
    )
    .map_err(|e| format!("failed to sync current_substats after edit: {e}"))?;

    let affected_range = if reorder_mode == "time_assist" {
        reorder_analysis_seq(&tx)?
    } else {
        format!("event:{}", input.event_id)
    };

    let opened_slots_count: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(slot_no), 0) FROM echo_current_substats WHERE echo_id = ?1",
            [&echo_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("failed to recompute opened slots: {e}"))?;
    tx.execute(
        "UPDATE echoes SET opened_slots_count = ?2, updated_at = ?3 WHERE echo_id = ?1",
        params![echo_id, opened_slots_count, now_rfc3339()],
    )
    .map_err(|e| format!("failed to update echo after edit: {e}"))?;

    let after = tx
        .query_row(
            "SELECT echo_id, slot_no, stat_key, tier_index, value_scaled, event_time, created_seq, analysis_seq, game_day
             FROM ordered_events
             WHERE event_id = ?1",
            [&input.event_id],
            |row| {
                Ok(json!({
                    "echoId": row.get::<_, String>(0)?,
                    "slotNo": row.get::<_, i64>(1)?,
                    "statKey": row.get::<_, String>(2)?,
                    "tierIndex": row.get::<_, i64>(3)?,
                    "valueScaled": row.get::<_, i64>(4)?,
                    "eventTime": row.get::<_, String>(5)?,
                    "createdSeq": row.get::<_, i64>(6)?,
                    "analysisSeq": row.get::<_, i64>(7)?,
                    "gameDay": row.get::<_, String>(8)?,
                }))
            },
        )
        .map_err(|e| format!("failed to query updated event: {e}"))?;

    tx.execute(
        "INSERT INTO event_edit_logs(log_id, event_id, before_json, after_json, reorder_mode, edited_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            Uuid::new_v4().to_string(),
            input.event_id,
            before.to_string(),
            after.to_string(),
            reorder_mode,
            now_rfc3339()
        ],
    )
    .map_err(|e| format!("failed to insert edit log: {e}"))?;

    tx.commit()
        .map_err(|e| format!("failed to commit edit event: {e}"))?;

    Ok(EditOrderedEventOutput {
        ok: true,
        affected_range,
    })
}

#[tauri::command]
pub fn get_event_history(
    state: State<'_, AppState>,
    filter: Option<EventHistoryFilter>,
) -> Result<Vec<EventRow>, String> {
    let conn = open_connection(&state)?;
    let filter = filter.unwrap_or_default();
    let limit = filter.limit.unwrap_or(200).clamp(1, 5000);

    let (query, bind_echo) = if filter.echo_id.is_some() {
        (
            "SELECT oe.event_id, oe.echo_id, e.nickname, oe.slot_no, oe.stat_key, oe.tier_index,
                    oe.value_scaled, oe.event_time, oe.created_seq, oe.analysis_seq, oe.game_day, oe.created_at
             FROM ordered_events oe
             JOIN echoes e ON e.echo_id = oe.echo_id
             WHERE oe.echo_id = ?1
             ORDER BY oe.analysis_seq DESC
             LIMIT ?2",
            true,
        )
    } else {
        (
            "SELECT oe.event_id, oe.echo_id, e.nickname, oe.slot_no, oe.stat_key, oe.tier_index,
                    oe.value_scaled, oe.event_time, oe.created_seq, oe.analysis_seq, oe.game_day, oe.created_at
             FROM ordered_events oe
             JOIN echoes e ON e.echo_id = oe.echo_id
             ORDER BY oe.analysis_seq DESC
             LIMIT ?1",
            false,
        )
    };

    let mut stmt = conn
        .prepare(query)
        .map_err(|e| format!("failed to prepare history query: {e}"))?;

    if bind_echo {
        let rows = stmt
            .query_map(params![filter.echo_id.unwrap_or_default(), limit], |row| {
                Ok(EventRow {
                    event_id: row.get(0)?,
                    echo_id: row.get(1)?,
                    echo_nickname: row.get(2)?,
                    slot_no: row.get(3)?,
                    stat_key: row.get(4)?,
                    tier_index: row.get(5)?,
                    value_scaled: row.get(6)?,
                    event_time: row.get(7)?,
                    created_seq: row.get(8)?,
                    analysis_seq: row.get(9)?,
                    game_day: row.get(10)?,
                    created_at: row.get(11)?,
                })
            })
            .map_err(|e| format!("failed to query history rows: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to collect history rows: {e}"))
    } else {
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(EventRow {
                    event_id: row.get(0)?,
                    echo_id: row.get(1)?,
                    echo_nickname: row.get(2)?,
                    slot_no: row.get(3)?,
                    stat_key: row.get(4)?,
                    tier_index: row.get(5)?,
                    value_scaled: row.get(6)?,
                    event_time: row.get(7)?,
                    created_seq: row.get(8)?,
                    analysis_seq: row.get(9)?,
                    game_day: row.get(10)?,
                    created_at: row.get(11)?,
                })
            })
            .map_err(|e| format!("failed to query history rows: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to collect history rows: {e}"))
    }
}
