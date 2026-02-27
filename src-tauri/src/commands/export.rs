use std::fs::File;
use std::io::Write;

use chrono::Utc;
use csv::Writer;
use rusqlite::{types::ValueRef, Connection};
use tauri::State;
use zip::write::SimpleFileOptions;

use crate::commands::analysis::get_global_distribution_internal;
use crate::db::{open_connection, AppState};
use crate::domain::types::{ExportCsvInput, ExportCsvOutput};

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
    while let Some(row) = rows.next().map_err(|e| format!("failed to iterate rows for {table}: {e}"))? {
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
        ("echoes.csv", "echoes"),
        ("echo_current_substats.csv", "echo_current_substats"),
        ("echo_expectations.csv", "echo_expectations"),
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
