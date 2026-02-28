use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};

use chrono::Utc;
use csv::{ReaderBuilder, Writer};
use rusqlite::{types::ValueRef, Connection};
use tauri::State;
use zip::write::SimpleFileOptions;
use zip::ZipArchive;

use crate::commands::analysis::get_global_distribution_internal;
use crate::db::{open_connection, AppState};
use crate::domain::types::{ExportCsvInput, ExportCsvOutput, ImportDataInput, ImportDataOutput};

fn sql_value_to_string(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => String::new(),
        ValueRef::Integer(v) => v.to_string(),
        ValueRef::Real(v) => v.to_string(),
        ValueRef::Text(v) => String::from_utf8_lossy(v).to_string(),
        ValueRef::Blob(v) => format!("0x{}", hex::encode(v)),
    }
}

fn export_table_to_csv(conn: &Connection, table: &str) -> Result<Vec<u8>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table}"))
        .map_err(|e| format!("failed to prepare table export for {table}: {e}"))?;

    let column_names = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    let mut writer = Writer::from_writer(Vec::new());
    writer
        .write_record(&column_names)
        .map_err(|e| format!("failed to write csv header for {table}: {e}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| format!("failed to query rows for {table}: {e}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("failed to iterate rows for {table}: {e}"))?
    {
        let mut record = Vec::with_capacity(column_names.len());
        for idx in 0..column_names.len() {
            let val = row
                .get_ref(idx)
                .map_err(|e| format!("failed to read value for {table}: {e}"))?;
            record.push(sql_value_to_string(val));
        }
        writer
            .write_record(record)
            .map_err(|e| format!("failed to write row for {table}: {e}"))?;
    }

    writer
        .into_inner()
        .map_err(|e| format!("failed to finalize csv for {table}: {e}"))
}

fn distribution_to_csv_bytes(payload: &crate::domain::types::DistributionPayload) -> Result<Vec<u8>, String> {
    let mut writer = Writer::from_writer(Vec::new());
    writer
        .write_record([
            "stat_key",
            "display_name",
            "unit",
            "count",
            "p_global",
            "ci_freq_low",
            "ci_freq_high",
            "bayes_mean",
            "bayes_low",
            "bayes_high",
        ])
        .map_err(|e| format!("failed to write distribution header: {e}"))?;

    for row in &payload.rows {
        writer
            .write_record([
                row.stat_key.clone(),
                row.display_name.clone(),
                row.unit.clone(),
                row.count.to_string(),
                row.p_global.to_string(),
                row.ci_freq_low.to_string(),
                row.ci_freq_high.to_string(),
                row.bayes_mean.to_string(),
                row.bayes_low.to_string(),
                row.bayes_high.to_string(),
            ])
            .map_err(|e| format!("failed to write distribution row: {e}"))?;
    }

    writer
        .into_inner()
        .map_err(|e| format!("failed to finalize distribution csv: {e}"))
}

fn parse_csv_rows(bytes: &[u8]) -> Result<Vec<HashMap<String, String>>, String> {
    let mut reader = ReaderBuilder::new()
        .flexible(true)
        .from_reader(bytes);

    let headers = reader
        .headers()
        .map_err(|e| format!("failed to read csv headers: {e}"))?
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    for rec in reader.records() {
        let record = rec.map_err(|e| format!("failed to read csv row: {e}"))?;
        let mut row = HashMap::new();
        for (idx, value) in record.iter().enumerate() {
            if let Some(key) = headers.get(idx) {
                row.insert(key.clone(), value.to_string());
            }
        }
        rows.push(row);
    }

    Ok(rows)
}

fn read_csv_rows_from_zip(
    archive: &mut ZipArchive<File>,
    file_name: &str,
) -> Result<Option<Vec<HashMap<String, String>>>, String> {
    let mut file = match archive.by_name(file_name) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read {file_name} from zip: {e}"))?;
    parse_csv_rows(&bytes).map(Some)
}

fn get_required<'a>(row: &'a HashMap<String, String>, key: &str, table: &str) -> Result<&'a str, String> {
    row.get(key)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("missing required field {key} in {table}"))
}

fn parse_i64(value: &str, key: &str, table: &str) -> Result<i64, String> {
    value
        .parse::<i64>()
        .map_err(|e| format!("invalid i64 for {key} in {table}: {e}"))
}

fn insert_rows_from_zip(conn: &mut Connection, archive: &mut ZipArchive<File>) -> Result<Vec<String>, String> {
    let mut imported_tables = Vec::new();

    let tables = vec![
        "echoes.csv",
        "echo_expectations.csv",
        "echo_current_substats.csv",
        "events.csv",
        "event_edit_logs.csv",
        "probability_snapshots.csv",
        "expectation_presets.csv",
        "expectation_preset_items.csv",
        "app_settings.csv",
    ];

    let mut data: HashMap<&str, Vec<HashMap<String, String>>> = HashMap::new();
    for table in &tables {
        if let Some(rows) = read_csv_rows_from_zip(archive, table)? {
            data.insert(*table, rows);
        }
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start import transaction: {e}"))?;

    tx.execute_batch(
        "DELETE FROM event_edit_logs;
         DELETE FROM ordered_events;
         DELETE FROM echo_current_substats;
         DELETE FROM echo_expectations;
         DELETE FROM echoes;
         DELETE FROM probability_snapshots;
         DELETE FROM expectation_preset_items;
         DELETE FROM expectation_presets;",
    )
    .map_err(|e| format!("failed to clear existing data: {e}"))?;

    if let Some(rows) = data.get("echoes.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO echoes(echo_id, nickname, main_stat_key, cost_class, status, opened_slots_count, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    get_required(row, "echo_id", "echoes.csv")?,
                    row.get("nickname").cloned().filter(|s| !s.is_empty()),
                    get_required(row, "main_stat_key", "echoes.csv")?,
                    parse_i64(get_required(row, "cost_class", "echoes.csv")?, "cost_class", "echoes.csv")?,
                    get_required(row, "status", "echoes.csv")?,
                    parse_i64(
                        get_required(row, "opened_slots_count", "echoes.csv")?,
                        "opened_slots_count",
                        "echoes.csv",
                    )?,
                    get_required(row, "created_at", "echoes.csv")?,
                    get_required(row, "updated_at", "echoes.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import echoes row: {e}"))?;
        }
        imported_tables.push("echoes".to_string());
    }

    if let Some(rows) = data.get("echo_expectations.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO echo_expectations(echo_id, stat_key, rank) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    get_required(row, "echo_id", "echo_expectations.csv")?,
                    get_required(row, "stat_key", "echo_expectations.csv")?,
                    parse_i64(get_required(row, "rank", "echo_expectations.csv")?, "rank", "echo_expectations.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import echo_expectations row: {e}"))?;
        }
        imported_tables.push("echo_expectations".to_string());
    }

    if let Some(rows) = data.get("echo_current_substats.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO echo_current_substats(echo_id, slot_no, stat_key, tier_index, value_scaled, source, event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    get_required(row, "echo_id", "echo_current_substats.csv")?,
                    parse_i64(get_required(row, "slot_no", "echo_current_substats.csv")?, "slot_no", "echo_current_substats.csv")?,
                    get_required(row, "stat_key", "echo_current_substats.csv")?,
                    parse_i64(get_required(row, "tier_index", "echo_current_substats.csv")?, "tier_index", "echo_current_substats.csv")?,
                    parse_i64(get_required(row, "value_scaled", "echo_current_substats.csv")?, "value_scaled", "echo_current_substats.csv")?,
                    get_required(row, "source", "echo_current_substats.csv")?,
                    row.get("event_id").cloned().filter(|s| !s.is_empty()),
                ],
            )
            .map_err(|e| format!("failed to import echo_current_substats row: {e}"))?;
        }
        imported_tables.push("echo_current_substats".to_string());
    }

    if let Some(rows) = data.get("events.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO ordered_events(
                    event_id, echo_id, slot_no, stat_key, tier_index, value_scaled,
                    event_time, created_seq, analysis_seq, game_day, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    get_required(row, "event_id", "events.csv")?,
                    get_required(row, "echo_id", "events.csv")?,
                    parse_i64(get_required(row, "slot_no", "events.csv")?, "slot_no", "events.csv")?,
                    get_required(row, "stat_key", "events.csv")?,
                    parse_i64(get_required(row, "tier_index", "events.csv")?, "tier_index", "events.csv")?,
                    parse_i64(get_required(row, "value_scaled", "events.csv")?, "value_scaled", "events.csv")?,
                    get_required(row, "event_time", "events.csv")?,
                    parse_i64(get_required(row, "created_seq", "events.csv")?, "created_seq", "events.csv")?,
                    parse_i64(get_required(row, "analysis_seq", "events.csv")?, "analysis_seq", "events.csv")?,
                    get_required(row, "game_day", "events.csv")?,
                    get_required(row, "created_at", "events.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import events row: {e}"))?;
        }
        imported_tables.push("ordered_events".to_string());
    }

    if let Some(rows) = data.get("event_edit_logs.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO event_edit_logs(log_id, event_id, before_json, after_json, reorder_mode, edited_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    get_required(row, "log_id", "event_edit_logs.csv")?,
                    get_required(row, "event_id", "event_edit_logs.csv")?,
                    get_required(row, "before_json", "event_edit_logs.csv")?,
                    get_required(row, "after_json", "event_edit_logs.csv")?,
                    get_required(row, "reorder_mode", "event_edit_logs.csv")?,
                    get_required(row, "edited_at", "event_edit_logs.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import event_edit_logs row: {e}"))?;
        }
        imported_tables.push("event_edit_logs".to_string());
    }

    if let Some(rows) = data.get("probability_snapshots.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO probability_snapshots(snapshot_id, created_at, scope_json, distribution_json, echo_probs_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    get_required(row, "snapshot_id", "probability_snapshots.csv")?,
                    get_required(row, "created_at", "probability_snapshots.csv")?,
                    get_required(row, "scope_json", "probability_snapshots.csv")?,
                    get_required(row, "distribution_json", "probability_snapshots.csv")?,
                    get_required(row, "echo_probs_json", "probability_snapshots.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import probability_snapshots row: {e}"))?;
        }
        imported_tables.push("probability_snapshots".to_string());
    }

    if let Some(rows) = data.get("expectation_presets.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO expectation_presets(preset_id, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    get_required(row, "preset_id", "expectation_presets.csv")?,
                    get_required(row, "name", "expectation_presets.csv")?,
                    get_required(row, "created_at", "expectation_presets.csv")?,
                    get_required(row, "updated_at", "expectation_presets.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import expectation_presets row: {e}"))?;
        }
        imported_tables.push("expectation_presets".to_string());
    }

    if let Some(rows) = data.get("expectation_preset_items.csv") {
        for row in rows {
            tx.execute(
                "INSERT INTO expectation_preset_items(preset_id, stat_key, rank)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    get_required(row, "preset_id", "expectation_preset_items.csv")?,
                    get_required(row, "stat_key", "expectation_preset_items.csv")?,
                    parse_i64(
                        get_required(row, "rank", "expectation_preset_items.csv")?,
                        "rank",
                        "expectation_preset_items.csv",
                    )?,
                ],
            )
            .map_err(|e| format!("failed to import expectation_preset_items row: {e}"))?;
        }
        imported_tables.push("expectation_preset_items".to_string());
    }

    if let Some(rows) = data.get("app_settings.csv") {
        tx.execute("DELETE FROM app_settings", [])
            .map_err(|e| format!("failed to clear app_settings: {e}"))?;
        for row in rows {
            tx.execute(
                "INSERT INTO app_settings(key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    get_required(row, "key", "app_settings.csv")?,
                    get_required(row, "value", "app_settings.csv")?,
                ],
            )
            .map_err(|e| format!("failed to import app_settings row: {e}"))?;
        }
        imported_tables.push("app_settings".to_string());
    }

    tx.commit()
        .map_err(|e| format!("failed to commit import transaction: {e}"))?;

    Ok(imported_tables)
}

#[tauri::command]
pub fn export_csv(
    state: State<'_, AppState>,
    input: ExportCsvInput,
) -> Result<ExportCsvOutput, String> {
    let conn = open_connection(&state)?;

    let export_name = format!("wuwa_echo_sight_export_{}.zip", Utc::now().format("%Y%m%d_%H%M%S"));
    let zip_path = std::env::temp_dir().join(export_name);

    let file = File::create(&zip_path).map_err(|e| format!("failed to create zip file: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let files = vec![
        ("events.csv", "ordered_events"),
        ("event_edit_logs.csv", "event_edit_logs"),
        ("echoes.csv", "echoes"),
        ("echo_current_substats.csv", "echo_current_substats"),
        ("echo_expectations.csv", "echo_expectations"),
        ("expectation_presets.csv", "expectation_presets"),
        ("expectation_preset_items.csv", "expectation_preset_items"),
        ("app_settings.csv", "app_settings"),
    ];

    for (filename, table) in files {
        let bytes = export_table_to_csv(&conn, table)?;
        zip.start_file(filename, options)
            .map_err(|e| format!("failed to add {filename} to zip: {e}"))?;
        zip.write_all(&bytes)
            .map_err(|e| format!("failed to write {filename} bytes: {e}"))?;
    }

    let distribution = get_global_distribution_internal(&conn, &input.scope)?;
    let distribution_bytes = distribution_to_csv_bytes(&distribution)?;
    zip.start_file("distribution_latest.csv", options)
        .map_err(|e| format!("failed to add distribution_latest.csv to zip: {e}"))?;
    zip.write_all(&distribution_bytes)
        .map_err(|e| format!("failed to write distribution_latest.csv bytes: {e}"))?;

    if input.include_snapshots {
        let snapshot_bytes = export_table_to_csv(&conn, "probability_snapshots")?;
        zip.start_file("probability_snapshots.csv", options)
            .map_err(|e| format!("failed to add probability_snapshots.csv to zip: {e}"))?;
        zip.write_all(&snapshot_bytes)
            .map_err(|e| format!("failed to write probability_snapshots.csv bytes: {e}"))?;
    }

    let readme = format!(
        "WuWa Echo Sight Export\nGeneratedAtUTC: {}\nIncludeSnapshots: {}\nScopeJson: {}\n",
        Utc::now().to_rfc3339(),
        input.include_snapshots,
        serde_json::to_string(&input.scope).unwrap_or_else(|_| "{}".to_string())
    );

    zip.start_file("readme.txt", options)
        .map_err(|e| format!("failed to add readme.txt to zip: {e}"))?;
    zip.write_all(readme.as_bytes())
        .map_err(|e| format!("failed to write readme.txt bytes: {e}"))?;

    zip.finish()
        .map_err(|e| format!("failed to finalize zip: {e}"))?;

    Ok(ExportCsvOutput {
        zip_path: zip_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn import_data(
    state: State<'_, AppState>,
    input: ImportDataInput,
) -> Result<ImportDataOutput, String> {
    let mut conn = open_connection(&state)?;

    let file = File::open(&input.zip_path)
        .map_err(|e| format!("failed to open zip file {}: {e}", input.zip_path))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("failed to parse zip archive: {e}"))?;

    let imported_tables = insert_rows_from_zip(&mut conn, &mut archive)?;

    Ok(ImportDataOutput {
        ok: true,
        imported_tables,
    })
}
